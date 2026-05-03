//! Opus encoder (libopus via `audiopus`): 48 kHz, 20 ms frames (960 samples/channel).

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};

/// Samples **per channel** per encoded packet at 48 kHz (`RS_CAPTURE_OPUS_FRAME_MS=20`).
pub const OPUS_SAMPLES_PER_CHANNEL_FRAME_48K: u32 = 960;

fn channels_opus(channels: u16) -> anyhow::Result<Channels> {
    match channels {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        _ => anyhow::bail!("Opus encoder: only mono/stereo supported (got {channels})"),
    }
}

/// Linear resample for WASAPI 44.1 kHz → 48 kHz (compact interval/interleaved `f32`).
fn resample_interleaved_linear(
    channels: usize,
    in_rate: u32,
    out_rate: u32,
    input: &[f32],
) -> Vec<f32> {
    if in_rate == out_rate {
        return input.to_vec();
    }
    let frames_in = input.len() / channels;
    if frames_in == 0 {
        return Vec::new();
    }
    let frames_out =
        ((frames_in as u64 * out_rate as u64 + in_rate as u64 / 2) / in_rate as u64) as usize;
    let mut out = vec![0.0f32; frames_out * channels];
    for fo in 0..frames_out {
        let t = fo as f64 * in_rate as f64 / out_rate as f64;
        let fi = t.floor() as usize;
        let frac = t - fi as f64;
        for c in 0..channels {
            let base_in = fi.min(frames_in.saturating_sub(1)) * channels + c;
            let base_next = (fi + 1).min(frames_in.saturating_sub(1)) * channels + c;
            let s0 = input[base_in];
            let s1 = input[base_next];
            out[fo * channels + c] = (s0 as f64 * (1.0 - frac) + s1 as f64 * frac) as f32;
        }
    }
    out
}

pub struct OpusEncoder {
    enc: Encoder,
    pending: Vec<f32>,
    channels: usize,
    frame_samples_per_channel: usize,
    input_sample_rate: u32,
}

impl OpusEncoder {
    /// `input_sample_rate` is usually WASAPI mix rate (44100 or 48000). Internally always encodes 48 kHz Opus.
    pub fn new(input_sample_rate: u32, channels: u16, bitrate_bps: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(
            matches!(input_sample_rate, 44_100 | 48_000),
            "Opus encoder: only 44100 or 48000 Hz input supported (got {input_sample_rate})"
        );
        let ch = channels_opus(channels)?;
        let ch_n = channels as usize;
        let mut enc = Encoder::new(SampleRate::Hz48000, ch, Application::Audio)
            .map_err(|e| anyhow::anyhow!("opus Encoder::new: {e}"))?;
        enc.set_bitrate(Bitrate::BitsPerSecond(bitrate_bps as i32))
            .map_err(|e| anyhow::anyhow!("opus set_bitrate: {e}"))?;

        Ok(Self {
            enc,
            pending: Vec::new(),
            channels: ch_n,
            frame_samples_per_channel: OPUS_SAMPLES_PER_CHANNEL_FRAME_48K as usize,
            input_sample_rate,
        })
    }

    /// WASAPI-rate PCM → 48 kHz internally; buffers until a 20 ms frame is complete.
    pub fn push_interleaved_f32(&mut self, interleaved: &[f32]) -> anyhow::Result<Vec<Vec<u8>>> {
        if interleaved.is_empty() {
            return Ok(Vec::new());
        }
        let at_48k = match self.input_sample_rate {
            48_000 => interleaved.to_vec(),
            44_100 => resample_interleaved_linear(self.channels, 44_100, 48_000, interleaved),
            _ => anyhow::bail!("OpusEncoder: unsupported input_sample_rate {}", self.input_sample_rate),
        };
        self.pending.extend(at_48k);
        self.drain_frames()
    }

    /// Pad with silence to whole frames and drain.
    pub fn flush(&mut self) -> anyhow::Result<Vec<Vec<u8>>> {
        let need = self
            .frame_samples_per_channel
            .saturating_mul(self.channels);
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let r = self.pending.len() % need;
        if r != 0 {
            self.pending.resize(self.pending.len() + (need - r), 0.0);
        }
        self.drain_frames()
    }

    fn drain_frames(&mut self) -> anyhow::Result<Vec<Vec<u8>>> {
        let need = self
            .frame_samples_per_channel
            .saturating_mul(self.channels);
        let mut packets = Vec::new();
        let mut out_buf = vec![0u8; 4000];
        while self.pending.len() >= need {
            let frame: Vec<f32> = self.pending.drain(..need).collect();
            let n = self
                .enc
                .encode_float(&frame, &mut out_buf)
                .map_err(|e| anyhow::anyhow!("opus encode_float: {e}"))?;
            packets.push(out_buf[..n].to_vec());
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_encoder_roundtrip_frame_size() {
        let mut enc = OpusEncoder::new(48_000, 2, 128_000).expect("encoder");
        let mut pcm = vec![0.0f32; 960 * 2];
        pcm[0] = 0.01;
        let pkts = enc.push_interleaved_f32(&pcm).expect("encode");
        assert_eq!(pkts.len(), 1);
        assert!(!pkts[0].is_empty());
    }
}
