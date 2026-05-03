//! H.264 encoding via **Media Foundation hardware** MFT (Intel Quick Sync, AMD, etc.).
//!
//! Uses **system-memory NV12** input — pairs with the capture pipeline’s existing `encode_i420` path
//! (I420 → NV12 → MFT). Does not use `encode_bgra_texture` (NVENC-style).
//!
//! When a D3D11 device is supplied (Quick Sync path), the MFT receives an DXGI device manager via
//! `MFT_MESSAGE_SET_D3D_MANAGER` so the driver can match GPU residency. [`encode_i420`] requests
//! periodic IDRs through [`CODECAPI_AVEncVideoForceKeyFrame`] when [`ICodecAPI`] is exposed.
//! `MF_E_TRANSFORM_STREAM_CHANGE` on input/output is handled by re-applying NV12/H.264 types.

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncVideoForceKeyFrame, ICodecAPI, IMFActivate, IMFMediaType, IMFTransform,
    IMFDXGIDeviceManager, MFCreateAlignedMemoryBuffer, MFCreateDXGIDeviceManager,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFShutdown, MFStartup,
    MF_E_NO_MORE_TYPES, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_VERSION, MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER,
    MFT_OUTPUT_DATA_BUFFER_NO_SAMPLE, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFSTARTUP_FULL,
    MFTEnumEx,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::{InitVariantFromBooleanArray, VariantClear};
use windows::core::{BOOL, Interface};

use crate::traits::{EncodedPacket, VideoCodec, VideoEncoder};

static MF_INIT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Hardware / MF H.264 encoder (Quick Sync path on Intel when the driver registers an HW MFT).
/// MF APIs are not `Send`; capture runs this encoder on one thread (same as NVENC policy).
pub struct MfH264HwEncoder {
    codec_api: Option<ICodecAPI>,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    frame_index: u64,
    next_input_hns: i64,
    caller_output_sample: bool,
    output_buffer_capacity: u32,
    output_buffer_alignment: u32,
    gop_frames: u32,
    /// DXGI device manager for `MFT_MESSAGE_SET_D3D_MANAGER`; released after [`Self::mft`] (field order).
    _dxgi_manager: Option<IMFDXGIDeviceManager>,
    mft: IMFTransform,
}

unsafe impl Send for MfH264HwEncoder {}

impl MfH264HwEncoder {
    /// Build an MF hardware H.264 transform.
    ///
    /// Pass `d3d_device` when the pipeline already holds the capture D3D11 device so the encoder MFT
    /// can bind through DXGI (`MFT_MESSAGE_SET_D3D_MANAGER`). Omit on hosts where only system-memory
    /// NV12 is available.
    pub fn try_new(
        config: &crate::EncoderConfig,
        d3d_device: Option<&ID3D11Device>,
    ) -> anyhow::Result<Self> {
        let width = config.width;
        let height = config.height;
        let fps = config.fps.max(1);
        let bitrate_bps = config.bitrate_bps;
        let gop_frames = fps.saturating_mul(2).max(30);

        anyhow::ensure!(
            width % 2 == 0 && height % 2 == 0,
            "MF H.264: width and height must be even"
        );

        if MF_INIT_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
            unsafe {
                MFStartup(MF_VERSION, MFSTARTUP_FULL).context("MFStartup")?;
            }
        }

        let dxgi_manager = if let Some(dev) = d3d_device {
            Some(unsafe { create_dxgi_device_manager(dev)? })
        } else {
            None
        };

        let dxgi_ref = dxgi_manager.as_ref();
        let mft = unsafe {
            enum_activate_hardware_encoder(width, height, fps, bitrate_bps, dxgi_ref)?
        };

        let codec_api = mft.cast::<ICodecAPI>().ok();

        let caller_output_sample;
        let output_buffer_capacity;
        let output_buffer_alignment;

        unsafe {
            mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).ok();

            let out_info = mft.GetOutputStreamInfo(0).context("GetOutputStreamInfo")?;
            let flags = out_info.dwFlags;
            let mft_provides = (flags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let mft_can_provide = (flags & MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            caller_output_sample = !mft_provides && !mft_can_provide;
            output_buffer_capacity = if out_info.cbSize == 0 {
                width
                    .saturating_mul(height)
                    .saturating_mul(3)
                    .saturating_div(2)
                    .saturating_add(65_536)
            } else {
                out_info.cbSize
            };
            output_buffer_alignment = out_info.cbAlignment.max(1);

            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .context("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING")?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .context("MFT_MESSAGE_NOTIFY_START_OF_STREAM")?;
        }

        tracing::info!(
            "Using Media Foundation hardware H.264 at {}x{} @ {} fps, {} bps (NV12 in, Annex-B out{}, GOP ~{} frames)",
            width,
            height,
            fps,
            bitrate_bps,
            if dxgi_manager.is_some() {
                ", DXGI device manager"
            } else {
                ""
            },
            gop_frames
        );

        Ok(Self {
            codec_api,
            width,
            height,
            fps,
            bitrate_bps,
            frame_index: 0,
            next_input_hns: 0,
            caller_output_sample,
            output_buffer_capacity,
            output_buffer_alignment,
            gop_frames,
            _dxgi_manager: dxgi_manager,
            mft,
        })
    }

    fn refresh_output_allocation(&mut self) -> anyhow::Result<()> {
        unsafe {
            let out_info = self
                .mft
                .GetOutputStreamInfo(0)
                .context("GetOutputStreamInfo after stream change")?;
            let flags = out_info.dwFlags;
            let mft_provides = (flags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let mft_can_provide = (flags & MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            self.caller_output_sample = !mft_provides && !mft_can_provide;
            self.output_buffer_capacity = if out_info.cbSize == 0 {
                self.width
                    .saturating_mul(self.height)
                    .saturating_mul(3)
                    .saturating_div(2)
                    .saturating_add(65_536)
            } else {
                out_info.cbSize
            };
            self.output_buffer_alignment = out_info.cbAlignment.max(1);
        }
        Ok(())
    }

    /// Re-apply NV12 input + H.264 output after `MF_E_TRANSFORM_STREAM_CHANGE`.
    fn reconfigure_streams(&mut self) -> anyhow::Result<()> {
        unsafe {
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).ok();
            apply_nv12_input_type(&self.mft, self.width, self.height, self.fps)
                .context("stream change: SetInputType NV12")?;
            try_set_h264_output_type(&self.mft, self.width, self.height, self.fps, self.bitrate_bps)
                .context("stream change: SetOutputType H264")?;
            self.refresh_output_allocation()?;
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .context("stream change: NOTIFY_BEGIN_STREAMING")?;
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .context("stream change: NOTIFY_START_OF_STREAM")?;
        }
        Ok(())
    }
}

unsafe fn create_dxgi_device_manager(device: &ID3D11Device) -> anyhow::Result<IMFDXGIDeviceManager> {
    let mut reset_token: u32 = 0;
    let mut manager: Option<IMFDXGIDeviceManager> = None;
    MFCreateDXGIDeviceManager(&mut reset_token, &mut manager).context("MFCreateDXGIDeviceManager")?;
    let mgr = manager.context("MFCreateDXGIDeviceManager returned null")?;
    mgr.ResetDevice(device, reset_token).context("IMFDXGIDeviceManager::ResetDevice")?;
    Ok(mgr)
}

/// Enumerate hardware video encoders; pick the first H.264 MFT that accepts our NV12 → H264 types.
unsafe fn enum_activate_hardware_encoder(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    dxgi_manager: Option<&IMFDXGIDeviceManager>,
) -> anyhow::Result<IMFTransform> {
    let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;

    let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count: u32 = 0;

    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        flags,
        None,
        None,
        &mut ptr,
        &mut count,
    )
    .context("MFTEnumEx hardware encoders")?;

    if count == 0 || ptr.is_null() {
        if !ptr.is_null() {
            CoTaskMemFree(Some(ptr as *const _));
        }
        anyhow::bail!("no hardware H.264 encoder MFTs found");
    }

    let mut last_err = anyhow::anyhow!("no hardware H.264 MFT accepted NV12→H.264 types");

    for i in 0..count {
        let act = match (*ptr.add(i as usize)).take() {
            Some(a) => a,
            None => continue,
        };

        match try_configure_mft(&act, width, height, fps, bitrate_bps, dxgi_manager) {
            Ok(mft) => {
                for j in (i + 1)..count {
                    let _ = (*ptr.add(j as usize)).take();
                }
                CoTaskMemFree(Some(ptr as *const _));
                return Ok(mft);
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    for j in 0..count {
        let _ = (*ptr.add(j as usize)).take();
    }
    CoTaskMemFree(Some(ptr as *const _));

    Err(last_err)
}

unsafe fn apply_nv12_input_type(
    mft: &IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
) -> anyhow::Result<()> {
    let input: IMFMediaType = MFCreateMediaType().context("MFCreateMediaType input")?;
    input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
    let fs = (u64::from(width) << 32) | u64::from(height);
    input.SetUINT64(&MF_MT_FRAME_SIZE, fs)?;
    let fr = (u64::from(fps) << 32) | 1u64;
    input.SetUINT64(&MF_MT_FRAME_RATE, fr)?;
    input.SetUINT32(
        &MF_MT_INTERLACE_MODE,
        MFVideoInterlace_Progressive.0 as u32,
    )?;
    mft.SetInputType(0, &input, 0)
        .context("SetInputType NV12")?;
    Ok(())
}

unsafe fn try_set_h264_output_type(
    mft: &IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
) -> anyhow::Result<()> {
    let fs = (u64::from(width) << 32) | u64::from(height);
    let fr = (u64::from(fps) << 32) | 1u64;
    let mut out_idx = 0u32;
    loop {
        let out_partial = match mft.GetOutputAvailableType(0, out_idx) {
            Ok(t) => t,
            Err(e) if e.code() == MF_E_NO_MORE_TYPES => {
                anyhow::bail!("MFT has no H.264 compressed output type");
            }
            Err(e) => return Err(e).context("GetOutputAvailableType")?,
        };
        let st = out_partial.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
        if st == MFVideoFormat_H264 {
            let out: IMFMediaType = MFCreateMediaType().context("MFCreateMediaType H264 out")?;
            out_partial
                .CopyAllItems(&out)
                .context("CopyAllItems H264")?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, fs)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, fr)?;
            out.SetUINT32(
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0 as u32,
            )?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)
                .context("MF_MT_AVG_BITRATE")?;
            mft.SetOutputType(0, &out, 0).context("SetOutputType H264")?;
            return Ok(());
        }
        out_idx += 1;
    }
}

unsafe fn try_configure_mft(
    act: &IMFActivate,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    dxgi_manager: Option<&IMFDXGIDeviceManager>,
) -> anyhow::Result<IMFTransform> {
    let mft: IMFTransform = act
        .ActivateObject::<IMFTransform>()
        .context("ActivateObject IMFTransform")?;

    mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).ok();

    if let Some(dm) = dxgi_manager {
        mft
            .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, dm.as_raw() as usize)
            .context("MFT_MESSAGE_SET_D3D_MANAGER")?;
    }

    apply_nv12_input_type(&mft, width, height, fps)
        .context("SetInputType NV12")?;
    try_set_h264_output_type(&mft, width, height, fps, bitrate_bps)?;

    Ok(mft)
}

/// Planar I420 → NV12 (same chroma subsampling).
pub(crate) fn i420_to_nv12(i420: &[u8], width: usize, height: usize) -> anyhow::Result<Vec<u8>> {
    let y_size = width
        .checked_mul(height)
        .context("i420_to_nv12 size")?;
    let c_sz = (width / 2)
        .checked_mul(height / 2)
        .context("chroma size")?;
    anyhow::ensure!(
        i420.len() >= y_size + 2 * c_sz,
        "I420 buffer too small: {} < {}",
        i420.len(),
        y_size + 2 * c_sz
    );

    let y = &i420[..y_size];
    let u = &i420[y_size..y_size + c_sz];
    let v = &i420[y_size + c_sz..y_size + 2 * c_sz];

    let mut nv12 = Vec::with_capacity(y_size + 2 * c_sz);
    nv12.extend_from_slice(y);
    for row in 0..height / 2 {
        for col in 0..width / 2 {
            let ix = row * (width / 2) + col;
            nv12.push(u[ix]);
            nv12.push(v[ix]);
        }
    }
    Ok(nv12)
}

/// Convert length-prefixed (AVCC-style) access units to Annex-B; pass-through if already start-coded.
fn compressed_to_annex_b(data: &[u8]) -> Vec<u8> {
    if data.windows(4).any(|w| w == [0, 0, 0, 1]) || data.windows(3).any(|w| w == [0, 0, 1]) {
        return data.to_vec();
    }

    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let len = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if len == 0 {
            continue;
        }
        if i + len > data.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[i..i + len]);
        i += len;
    }
    if out.is_empty() && !data.is_empty() {
        data.to_vec()
    } else {
        out
    }
}

fn annex_b_keyframe(annex_b: &[u8]) -> bool {
    let mut i = 0usize;
    while i < annex_b.len() {
        let (nal_start, _) = if annex_b[i..].starts_with(&[0, 0, 0, 1]) {
            (i + 4, 4usize)
        } else if annex_b[i..].starts_with(&[0, 0, 1]) {
            (i + 3, 3usize)
        } else {
            i += 1;
            continue;
        };
        if nal_start < annex_b.len() {
            let h = annex_b[nal_start];
            let nal_type = h & 0x1f;
            if nal_type == 5 {
                return true;
            }
        }
        i = nal_start + 1;
    }
    false
}

unsafe fn request_idr_from_codec_api(codec_api: &ICodecAPI) -> anyhow::Result<()> {
    let mut v = InitVariantFromBooleanArray(&[BOOL::from(true)]).context("InitVariant IDR")?;
    let hr = codec_api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &v);
    let _ = VariantClear(&mut v);
    hr.context("CODECAPI_AVEncVideoForceKeyFrame")?;
    Ok(())
}

impl VideoEncoder for MfH264HwEncoder {
    fn encode_i420(&mut self, i420: &[u8], timestamp_us: u64) -> anyhow::Result<EncodedPacket> {
        let w = self.width as usize;
        let h = self.height as usize;
        let nv12 = i420_to_nv12(i420, w, h)?;

        let frame_hns = (10_000_000i64 / i64::from(self.fps)).max(1);

        let want_idr = self.frame_index == 0
            || self.frame_index % u64::from(self.gop_frames.max(1)) == 0;
        if want_idr {
            if let Some(ref api) = self.codec_api {
                unsafe { request_idr_from_codec_api(api)? };
            }
        }

        unsafe {
            let buf = MFCreateMemoryBuffer(nv12.len() as u32).context("MFCreateMemoryBuffer NV12")?;
            {
                let mut p: *mut u8 = std::ptr::null_mut();
                buf.Lock(&mut p, None, None).context("NV12 Lock")?;
                std::ptr::copy_nonoverlapping(nv12.as_ptr(), p, nv12.len());
                buf.Unlock().ok();
                buf.SetCurrentLength(nv12.len() as u32)?;
            }

            let sample = MFCreateSample().context("MFCreateSample")?;
            sample.AddBuffer(&buf)?;
            sample.SetSampleTime(self.next_input_hns)?;
            sample.SetSampleDuration(frame_hns)?;

            let mut submitted = false;
            for _ in 0..8 {
                match self.mft.ProcessInput(0, &sample, 0) {
                    Ok(()) => {
                        submitted = true;
                        break;
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.reconfigure_streams()?;
                    }
                    Err(e) => return Err(e).context("ProcessInput H264 MFT"),
                }
            }
            anyhow::ensure!(
                submitted,
                "MF H.264: ProcessInput did not succeed after stream-change retries"
            );
        }

        self.next_input_hns = self
            .next_input_hns
            .saturating_add(frame_hns);

        let mut accumulated = Vec::new();
        let mut output_stream_change_recovery = 0u32;
        loop {
            let mut buf = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            if self.caller_output_sample {
                unsafe {
                    let mbuf = if self.output_buffer_alignment > 1 {
                        MFCreateAlignedMemoryBuffer(
                            self.output_buffer_capacity,
                            self.output_buffer_alignment,
                        )?
                    } else {
                        MFCreateMemoryBuffer(self.output_buffer_capacity)?
                    };
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&mbuf)?;
                    buf.pSample = ManuallyDrop::new(Some(sample));
                }
            }

            let mut status = 0u32;
            let hr = unsafe {
                self.mft.ProcessOutput(0, std::slice::from_mut(&mut buf), &mut status)
            };

            drop(ManuallyDrop::into_inner(std::mem::replace(
                &mut buf.pEvents,
                ManuallyDrop::new(None),
            )));

            if let Err(e) = hr {
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    break;
                }
                if e.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                    output_stream_change_recovery += 1;
                    anyhow::ensure!(
                        output_stream_change_recovery <= 8,
                        "MF H.264: too many stream-change recoveries on ProcessOutput"
                    );
                    self.reconfigure_streams()?;
                    continue;
                }
                return Err(e).context("ProcessOutput H264 MFT");
            }

            let sample_opt = ManuallyDrop::into_inner(std::mem::replace(
                &mut buf.pSample,
                ManuallyDrop::new(None),
            ));

            let no_sample = (buf.dwStatus & MFT_OUTPUT_DATA_BUFFER_NO_SAMPLE.0 as u32) != 0;
            if no_sample {
                continue;
            }

            if let Some(sample) = sample_opt {
                unsafe {
                    let b0 = sample.GetBufferByIndex(0)?;
                    let len = b0.GetCurrentLength()?;
                    let max_len = b0.GetMaxLength().unwrap_or(len);
                    let copy_len = (len.min(max_len)) as usize;
                    let mut p: *mut u8 = std::ptr::null_mut();
                    b0.Lock(&mut p, None, None)?;
                    let mut chunk = vec![0u8; copy_len];
                    if copy_len > 0 && !p.is_null() {
                        std::ptr::copy_nonoverlapping(p, chunk.as_mut_ptr(), copy_len);
                    }
                    b0.Unlock().ok();
                    if !chunk.is_empty() {
                        accumulated.extend_from_slice(&chunk);
                    }
                }
            }
        }

        anyhow::ensure!(
            !accumulated.is_empty(),
            "MF H.264: empty bitstream after encode"
        );

        let annex_b = compressed_to_annex_b(&accumulated);
        let is_keyframe = annex_b_keyframe(&annex_b);
        self.frame_index += 1;

        Ok(EncodedPacket {
            data: annex_b,
            timestamp_us,
            is_keyframe,
            codec: VideoCodec::H264,
        })
    }

    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }
}

impl Drop for MfH264HwEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        if MF_INIT_COUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compressed_to_annex_b, i420_to_nv12};

    #[test]
    fn i420_nv12_roundtrip_size() {
        let w = 32usize;
        let h = 32usize;
        let y = w * h;
        let c = (w / 2) * (h / 2);
        let mut i420 = vec![0u8; y + 2 * c];
        i420[0] = 16;
        i420[y] = 128;
        i420[y + c] = 128;
        let nv12 = i420_to_nv12(&i420, w, h).unwrap();
        assert_eq!(nv12.len(), y + 2 * c);
    }

    #[test]
    fn length_prefixed_to_annex_b() {
        let nal = [0x09, 0x10];
        let mut v = vec![0u8, 0, 0, 2];
        v.extend_from_slice(&nal);
        let out = compressed_to_annex_b(&v);
        assert!(out.windows(4).any(|w| w == [0, 0, 0, 1]));
    }
}
