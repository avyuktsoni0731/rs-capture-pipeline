//! NVIDIA NVENC H.264 via D3D11 session (`nvenc` crate). Feeds I420 by copying into a host IYUV input buffer.

use std::mem::ManuallyDrop;

use anyhow::Context;
use nvenc::bitstream::BitStream;
use nvenc::session::{InitParams, Session};
use nvenc::sys::enums::{
    NVencBufferFormat, NVencMemoryHeap, NVencParamsRcMode, NVencPicStruct, NVencPicType,
    NVencTuningInfo,
};
use nvenc::sys::guids::{
    NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P2_GUID, NV_ENC_PRESET_P3_GUID, NV_ENC_PRESET_P4_GUID,
};
use nvenc::sys::result::NVencError;
use nvenc::sys::structs::Guid;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::traits::{EncodedPacket, VideoCodec, VideoEncoder};

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Copy tightly packed I420 (`Y`, then `U`, then `V`) into NVENC IYUV host buffer layout for lock pitch `pitch`.
unsafe fn copy_i420_to_nvenc_iyuv(
    i420: &[u8],
    dst: *mut u8,
    width: u32,
    height: u32,
    pitch: u32,
) -> anyhow::Result<()> {
    let w = width as usize;
    let h = height as usize;
    let y_sz = w * h;
    let c_w = w / 2;
    let c_h = h / 2;
    let c_sz = c_w * c_h;
    anyhow::ensure!(
        i420.len() >= y_sz + 2 * c_sz,
        "I420 buffer too small for {}x{}",
        width,
        height
    );

    let pitch = pitch as usize;
    let chroma_pitch = pitch / 2;
    anyhow::ensure!(
        pitch >= w && chroma_pitch >= c_w,
        "NVENC pitch {} too small for width {}",
        pitch,
        width
    );

    for row in 0..h {
        let src = i420[row * w..row * w + w].as_ptr();
        let d = dst.add(row * pitch);
        std::ptr::copy_nonoverlapping(src, d, w);
    }

    let u_src = &i420[y_sz..y_sz + c_sz];
    let v_src = &i420[y_sz + c_sz..y_sz + 2 * c_sz];
    let y_plane_bytes = pitch * h;
    let mut u_dst = dst.add(y_plane_bytes);
    let mut v_dst = dst.add(y_plane_bytes + chroma_pitch * c_h);

    for row in 0..c_h {
        std::ptr::copy_nonoverlapping(
            u_src[row * c_w..row * c_w + c_w].as_ptr(),
            u_dst,
            c_w,
        );
        std::ptr::copy_nonoverlapping(
            v_src[row * c_w..row * c_w + c_w].as_ptr(),
            v_dst,
            c_w,
        );
        u_dst = u_dst.add(chroma_pitch);
        v_dst = v_dst.add(chroma_pitch);
    }

    Ok(())
}

fn pick_h264_preset(codecs: &[Guid], presets: &[Guid]) -> anyhow::Result<Guid> {
    anyhow::ensure!(
        codecs.iter().any(|g| *g == NV_ENC_CODEC_H264_GUID),
        "NVENC session does not advertise H.264"
    );
    for candidate in [
        NV_ENC_PRESET_P4_GUID,
        NV_ENC_PRESET_P3_GUID,
        NV_ENC_PRESET_P2_GUID,
    ] {
        if presets.iter().any(|g| *g == candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("no supported NVENC H.264 preset (P2–P4)")
}

/// `ManuallyDrop` + custom [`Drop`] so we can EOS-flush with a valid bitstream, then free resources
/// in a driver-friendly order (see workspace `vendor/nvenc` patch).
struct NvencInner {
    bitstream: ManuallyDrop<BitStream>,
    input: ManuallyDrop<nvenc::input_buffer::InputBuffer>,
    encoder: ManuallyDrop<nvenc::encoder::Encoder>,
}

impl Drop for NvencInner {
    fn drop(&mut self) {
        let enc: &nvenc::encoder::Encoder = &self.encoder;
        let bs: &BitStream = &self.bitstream;
        let _ = enc.flush_eos(bs);
        unsafe {
            ManuallyDrop::drop(&mut self.bitstream);
            ManuallyDrop::drop(&mut self.input);
            ManuallyDrop::drop(&mut self.encoder);
        }
    }
}

/// Hardware H.264 encoder (NVENC). Requires NVIDIA driver and matching `nvEncodeAPI64.dll`.
pub struct NvencVideoEncoder {
    inner: NvencInner,
    width: u32,
    height: u32,
    frame_idx: usize,
    gop_frames: u32,
}

/// Capture uses one thread for encode; the `nvenc` safe wrappers omit `Send`.
unsafe impl Send for NvencVideoEncoder {}

impl NvencVideoEncoder {
    pub fn try_new(device: &ID3D11Device, config: &crate::EncoderConfig) -> anyhow::Result<Self> {
        nvenc::nvenc_init().context("load nvEncodeAPI64.dll / NvEncodeAPICreateInstance")?;

        let width = config.width;
        let height = config.height;
        anyhow::ensure!(width % 2 == 0 && height % 2 == 0, "NVENC I420 needs even width/height");

        let session: Session<nvenc::session::NeedsConfig> =
            Session::open_dx(device).map_err(|e| anyhow::anyhow!("NVENC open_dx: {e:?}"))?;

        let codecs = session
            .get_encode_codecs()
            .map_err(|e| anyhow::anyhow!("get_encode_codecs: {e:?}"))?;
        let preset_list = session
            .get_encode_presets(NV_ENC_CODEC_H264_GUID)
            .map_err(|e| anyhow::anyhow!("get_encode_presets: {e:?}"))?;
        let preset_guid = pick_h264_preset(&codecs, &preset_list)?;

        let (session, mut preset_config) = session
            .get_encode_preset_config_ex(
                NV_ENC_CODEC_H264_GUID,
                preset_guid.clone(),
                NVencTuningInfo::LowLatency,
            )
            .map_err(|e| anyhow::anyhow!("get_encode_preset_config_ex: {e:?}"))?;

        let g = gcd(width, height).max(1);
        let dar = [width / g, height / g];

        let gop_frames = config.fps.saturating_mul(2).max(30);
        preset_config.preset_cfg.gop_len = gop_frames;
        preset_config.preset_cfg.frame_interval_p = 1;
        preset_config.preset_cfg.rc_params.rate_control_mode = NVencParamsRcMode::VBR;
        preset_config.preset_cfg.rc_params.average_bit_rate = config.bitrate_bps;

        let init = InitParams {
            encode_guid: NV_ENC_CODEC_H264_GUID,
            preset_guid,
            resolution: [width, height],
            aspect_ratio: dar,
            frame_rate: [config.fps.max(1), 1],
            tuning_info: NVencTuningInfo::LowLatency,
            buffer_format: NVencBufferFormat::IYUV,
            encode_config: &mut preset_config.preset_cfg,
            enable_ptd: true,
            max_encoder_resolution: [0, 0],
        };

        let encoder = session
            .init_encoder(init)
            .map_err(|e| anyhow::anyhow!("init_encoder: {e:?}"))?;

        let input = encoder
            .create_input_buffer(
                width,
                height,
                NVencMemoryHeap::SystemUncached,
                NVencBufferFormat::IYUV,
            )
            .map_err(|e| anyhow::anyhow!("create_input_buffer: {e:?}"))?;

        {
            let lock = input.lock().map_err(|e| anyhow::anyhow!("lock input (probe): {e:?}"))?;
            let pitch = lock.pitch();
            anyhow::ensure!(
                pitch == width,
                "NVENC input pitch {} != width {} (nvenc InputBuffer pitch bug risk); use OpenH264",
                pitch,
                width
            );
        }

        let bitstream = encoder
            .create_bitstream_buffer()
            .map_err(|e| anyhow::anyhow!("create_bitstream_buffer: {e:?}"))?;

        Ok(Self {
            inner: NvencInner {
                bitstream: ManuallyDrop::new(bitstream),
                input: ManuallyDrop::new(input),
                encoder: ManuallyDrop::new(encoder),
            },
            width,
            height,
            frame_idx: 0,
            gop_frames,
        })
    }

    fn lock_bitstream_wait(&self) -> anyhow::Result<nvenc::bitstream::BitStreamLockGuard<'_>> {
        loop {
            match self.inner.bitstream.try_lock(true) {
                Ok(g) => return Ok(g),
                Err(NVencError::LockBusy) => std::thread::yield_now(),
                Err(e) => anyhow::bail!("lock bitstream: {e:?}"),
            }
        }
    }
}

impl VideoEncoder for NvencVideoEncoder {
    fn encode_i420(&mut self, i420: &[u8], timestamp_us: u64) -> anyhow::Result<EncodedPacket> {
        let expected = (self.width * self.height * 3 / 2) as usize;
        anyhow::ensure!(
            i420.len() >= expected,
            "I420 size {} < expected {}",
            i420.len(),
            expected
        );

        {
            let lock = (&*self.inner.input)
                .lock()
                .map_err(|e| anyhow::anyhow!("lock input: {e:?}"))?;
            unsafe {
                copy_i420_to_nvenc_iyuv(
                    &i420[..expected],
                    lock.data_ptr(),
                    self.width,
                    self.height,
                    lock.pitch(),
                )?;
            }
        }

        let force_idr = (self.frame_idx == 0)
            || (self.gop_frames > 0 && (self.frame_idx as u32) % self.gop_frames == 0);
        let pic_ty = if force_idr {
            NVencPicType::IDR
        } else {
            NVencPicType::P
        };

        self.inner
            .encoder
            .encode_picture(
                &*self.inner.input,
                &*self.inner.bitstream,
                self.frame_idx,
                timestamp_us,
                NVencBufferFormat::IYUV,
                NVencPicStruct::Frame,
                pic_ty,
                None,
            )
            .map_err(|e| anyhow::anyhow!("encode_picture: {e:?}"))?;

        self.frame_idx += 1;

        let guard = self.lock_bitstream_wait()?;
        let data = guard.as_slice().to_vec();
        drop(guard);

        Ok(EncodedPacket {
            data,
            timestamp_us,
            is_keyframe: force_idr,
            codec: VideoCodec::H264,
        })
    }

    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }
}
