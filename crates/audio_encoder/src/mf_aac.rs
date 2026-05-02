//! AAC-LC encoder via Windows Media Foundation (`AACMFTEncoder` MFT).

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use windows::Win32::Media::MediaFoundation::{
    AACMFTEncoder, IMFMediaType, IMFTransform, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFShutdown, MFStartup, MF_E_NO_MORE_TYPES,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_MT_USER_DATA, MF_VERSION,
    MFMediaType_Audio, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFSTARTUP_FULL,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

static MF_INIT_COUNT: AtomicU32 = AtomicU32::new(0);

/// AAC-LC (raw access units) using the system AAC encoder MFT.
pub struct MfAacLcEncoder {
    mft: IMFTransform,
    sample_rate: u32,
    channels: u16,
    /// PCM16 samples per channel per `ProcessInput` (1024 for LC @ 48k).
    frame_samples: u32,
    pending_pcm: Vec<i16>,
    /// Presentation time in MF 100-ns units for next input frame.
    next_input_hns: i64,
}

impl MfAacLcEncoder {
    /// `bitrate_bps` is total audio bitrate (e.g. 128_000).
    pub fn new(sample_rate: u32, channels: u16, bitrate_bps: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(sample_rate, 44_100 | 48_000),
            "MF AAC-LC encoder: only 44100 or 48000 Hz supported (got {sample_rate})"
        );
        anyhow::ensure!(
            matches!(channels, 1 | 2),
            "MF AAC-LC encoder: only mono/stereo supported (got {channels})"
        );

        if MF_INIT_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
            unsafe {
                MFStartup(MF_VERSION, MFSTARTUP_FULL).context("MFStartup")?;
            }
        }

        let mft: IMFTransform = unsafe {
            CoCreateInstance(&AACMFTEncoder, None, CLSCTX_INPROC_SERVER).context("CoCreateInstance AACMFTEncoder")?
        };

        unsafe {
            mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).ok();

            let input: IMFMediaType = MFCreateMediaType().context("MFCreateMediaType input")?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)?;
            input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(channels))?;
            let block_align = u32::from(channels) * 2;
            input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_align)?;
            input.SetUINT32(
                &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                sample_rate * block_align,
            )?;
            mft.SetInputType(0, &input, 0).context("SetInputType PCM")?;

            let mut out_idx = 0u32;
            let mut set = false;
            loop {
                let out_partial = match mft.GetOutputAvailableType(0, out_idx) {
                    Ok(t) => t,
                    Err(e) if e.code() == MF_E_NO_MORE_TYPES => break,
                    Err(e) => return Err(e).context("GetOutputAvailableType")?,
                };
                let st = out_partial.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                if st == MFAudioFormat_AAC {
                    let out: IMFMediaType = MFCreateMediaType().context("MFCreateMediaType AAC out")?;
                    out_partial
                        .CopyAllItems(&out)
                        .context("CopyAllItems AAC partial → full")?;
                    out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
                        .ok();
                    out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, u32::from(channels))
                        .ok();
                    // Compressed bitrate: bytes per second (see AAC encoder MFT docs).
                    let avg_bytes = std::cmp::max(1, bitrate_bps / 8);
                    out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, avg_bytes)
                        .context("MF_MT_AUDIO_AVG_BYTES_PER_SECOND")?;
                    // 0 = raw AAC payload (no ADTS) for MP4 `mp4a` sample descriptions.
                    out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)
                        .context("MF_MT_AAC_PAYLOAD_TYPE")?;
                    let asc = aac_audio_specific_config(sample_rate, channels)?;
                    out.SetBlob(&MF_MT_USER_DATA, &asc)
                        .context("MF_MT_USER_DATA (AudioSpecificConfig)")?;
                    mft.SetOutputType(0, &out, 0).context("SetOutputType AAC")?;
                    set = true;
                    break;
                }
                out_idx += 1;
            }
            anyhow::ensure!(set, "no AAC output type on MF AAC encoder");

            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .context("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING")?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .context("MFT_MESSAGE_NOTIFY_START_OF_STREAM")?;
        }

        Ok(Self {
            mft,
            sample_rate,
            channels,
            frame_samples: 1024,
            pending_pcm: Vec::new(),
            next_input_hns: 0,
        })
    }

    /// Append float samples (interleaved) and return any completed AAC access units (raw, no ADTS).
    pub fn push_interleaved_f32(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Vec<u8>>> {
        let reserve = samples.len();
        self.pending_pcm.reserve(reserve);
        for &s in samples {
            let q = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.pending_pcm.push(q);
        }
        let mut out_aus = Vec::new();
        let frame_len = (self.frame_samples as usize) * (self.channels as usize);
        while self.pending_pcm.len() >= frame_len {
            let chunk: Vec<i16> = self.pending_pcm[..frame_len].to_vec();
            self.pending_pcm.drain(..frame_len);
            let aus = self.encode_pcm_frame(&chunk)?;
            out_aus.extend(aus);
        }
        Ok(out_aus)
    }

    /// Pad with silence and drain encoder after last PCM.
    pub fn flush(&mut self) -> anyhow::Result<Vec<Vec<u8>>> {
        let frame_len = (self.frame_samples as usize) * (self.channels as usize);
        if !self.pending_pcm.is_empty() {
            self.pending_pcm.resize(frame_len, 0);
        }
        let mut out = Vec::new();
        if self.pending_pcm.len() == frame_len {
            let chunk = std::mem::take(&mut self.pending_pcm);
            out.extend(self.encode_pcm_frame(&chunk)?);
        }
        unsafe {
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .ok();
            self.mft
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .context("MFT_MESSAGE_COMMAND_DRAIN")?;
        }
        out.extend(self.drain_all_output()?);
        Ok(out)
    }

    fn encode_pcm_frame(&mut self, interleaved_i16: &[i16]) -> anyhow::Result<Vec<Vec<u8>>> {
        let byte_len = interleaved_i16.len() * 2;
        let frame_hns = (self.frame_samples as i64)
            .saturating_mul(10_000_000)
            .saturating_div(i64::from(self.sample_rate));

        unsafe {
            let buf = MFCreateMemoryBuffer(byte_len as u32).context("MFCreateMemoryBuffer")?;
            {
                let mut ptr: *mut u8 = std::ptr::null_mut();
                buf.Lock(&mut ptr, None, None).context("IMFMediaBuffer::Lock")?;
                std::ptr::copy_nonoverlapping(
                    interleaved_i16.as_ptr() as *const u8,
                    ptr,
                    byte_len,
                );
                buf.Unlock().ok();
                buf.SetCurrentLength(byte_len as u32).context("SetCurrentLength")?;
            }

            let sample = MFCreateSample().context("MFCreateSample")?;
            sample.AddBuffer(&buf).context("AddBuffer")?;
            sample
                .SetSampleTime(self.next_input_hns)
                .context("SetSampleTime")?;
            sample
                .SetSampleDuration(frame_hns)
                .context("SetSampleDuration")?;

            self.mft
                .ProcessInput(0, &sample, 0)
                .context("ProcessInput AAC MFT")?;
        }

        self.next_input_hns = self.next_input_hns.saturating_add(frame_hns);

        self.drain_all_output()
    }

    fn drain_all_output(&mut self) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut v = Vec::new();
        loop {
            let mut buf = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            let hr = unsafe { self.mft.ProcessOutput(0, std::slice::from_mut(&mut buf), &mut status) };
            if let Err(e) = hr {
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    break;
                }
                return Err(e).context("ProcessOutput");
            }

            let sample_opt = unsafe { ManuallyDrop::take(&mut buf.pSample) };
            let Some(sample) = sample_opt else {
                continue;
            };
            let au = unsafe {
                let buf0 = sample.GetBufferByIndex(0).context("GetBufferByIndex")?;
                let len = buf0.GetCurrentLength().context("GetCurrentLength")?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                buf0.Lock(&mut ptr, None, None).context("output Lock")?;
                let mut out = vec![0u8; len as usize];
                if len > 0 && !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), len as usize);
                }
                buf0.Unlock().ok();
                out
            };
            if !au.is_empty() {
                v.push(au);
            }
        }
        Ok(v)
    }
}

/// ISO/IEC 14496-3 AudioSpecificConfig for AAC-LC (minimal 2-byte form).
pub(crate) fn aac_audio_specific_config(sample_rate: u32, channels: u16) -> anyhow::Result<[u8; 2]> {
    let sf_idx = match sample_rate {
        48_000 => 3u8,
        44_100 => 4u8,
        _ => anyhow::bail!("unsupported sample_rate for ASC: {sample_rate}"),
    };
    let ch_cfg = match channels {
        1 => 1u8,
        2 => 2u8,
        _ => anyhow::bail!("unsupported channels for ASC: {channels}"),
    };
    let aot: u8 = 2; // AAC-LC
    let b0 = (aot << 3) | (sf_idx >> 1);
    let b1 = ((sf_idx & 1) << 7) | (ch_cfg << 3);
    Ok([b0, b1])
}

#[cfg(test)]
mod tests {
    use super::aac_audio_specific_config;

    #[test]
    fn asc_bytes_match_iso_packing() {
        assert_eq!(aac_audio_specific_config(48_000, 2).unwrap(), [0x11, 0x90]);
        assert_eq!(aac_audio_specific_config(44_100, 2).unwrap(), [0x12, 0x10]);
        assert_eq!(aac_audio_specific_config(48_000, 1).unwrap(), [0x11, 0x88]);
    }
}

impl Drop for MfAacLcEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        if MF_INIT_COUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }
}
