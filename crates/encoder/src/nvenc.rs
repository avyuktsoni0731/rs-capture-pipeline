//! NVIDIA NVENC H.264 via D3D11: **registered DX11 texture** + `encode_picture` (same pattern as
//! NVIDIA samples / OBS), not host IYUV `NvEncCreateInputBuffer`.

use std::mem::ManuallyDrop;

use anyhow::Context;
use nvenc::bitstream::BitStream;
use nvenc::session::{InitParams, Session};
use nvenc::sys::enums::{
    NVencBufferFormat, NVencParamsRcMode, NVencPicStruct, NVencPicType, NVencTuningInfo,
};
use nvenc::sys::guids::{
    NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P2_GUID, NV_ENC_PRESET_P3_GUID, NV_ENC_PRESET_P4_GUID,
    NV_ENC_PRESET_P5_GUID, NV_ENC_PRESET_P6_GUID, NV_ENC_PRESET_P7_GUID,
};
use nvenc::sys::result::NVencError;
use nvenc::sys::structs::Guid;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};

use crate::traits::{EncodedPacket, VideoCodec, VideoEncoder};

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Prefer slow / high-quality presets first (OBS commonly uses **P5** “slow / good quality”).
fn pick_h264_preset(codecs: &[Guid], presets: &[Guid]) -> anyhow::Result<(Guid, &'static str)> {
    anyhow::ensure!(
        codecs.iter().any(|g| *g == NV_ENC_CODEC_H264_GUID),
        "NVENC session does not advertise H.264"
    );
    for (candidate, label) in [
        (NV_ENC_PRESET_P7_GUID, "P7"),
        (NV_ENC_PRESET_P6_GUID, "P6"),
        (NV_ENC_PRESET_P5_GUID, "P5"),
        (NV_ENC_PRESET_P4_GUID, "P4"),
        (NV_ENC_PRESET_P3_GUID, "P3"),
        (NV_ENC_PRESET_P2_GUID, "P2"),
    ] {
        if presets.iter().any(|g| *g == candidate) {
            return Ok((candidate, label));
        }
    }
    anyhow::bail!("no supported NVENC H.264 preset (P2–P7)")
}

fn tuning_info_from_env() -> NVencTuningInfo {
    match std::env::var("RS_CAPTURE_NVENC_TUNING") {
        Ok(s) => {
            let x = s.to_ascii_lowercase();
            match x.as_str() {
                "low_latency" | "lowlatency" => NVencTuningInfo::LowLatency,
                "ultra_low_latency" => NVencTuningInfo::UltraLowLatency,
                "ultra_high_quality" | "uhq" => NVencTuningInfo::UltraHighQuality,
                "high_quality" | "hq" | "" => NVencTuningInfo::HighQuality,
                other => {
                    tracing::warn!(
                        "RS_CAPTURE_NVENC_TUNING={other:?} unknown; use high_quality|low_latency|ultra_high_quality — using HighQuality"
                    );
                    NVencTuningInfo::HighQuality
                }
            }
        }
        Err(_) => NVencTuningInfo::HighQuality,
    }
}

/// `ManuallyDrop` + custom [`Drop`] so we EOS-flush with a valid bitstream, then unregister DX
/// resources before destroying the encoder (see workspace `vendor/nvenc` patch).
struct NvencInner {
    bitstream: ManuallyDrop<BitStream>,
    /// Lazily registered against the converter's internal BGRA texture; cleared before encoder destroy.
    registered: Option<nvenc::encoder::RegisteredResource>,
    encoder: ManuallyDrop<nvenc::encoder::Encoder>,
}

impl Drop for NvencInner {
    fn drop(&mut self) {
        let enc: &nvenc::encoder::Encoder = &self.encoder;
        let bs: &BitStream = &self.bitstream;
        let _ = enc.flush_eos(bs);
        self.registered = None;
        unsafe {
            ManuallyDrop::drop(&mut self.bitstream);
            ManuallyDrop::drop(&mut self.encoder);
        }
    }
}

/// Hardware H.264 encoder (NVENC). Requires NVIDIA driver and matching `nvEncodeAPI64.dll`.
///
/// Input is **BGRA** in a `ID3D11Texture2D` on the **same** `ID3D11Device` as capture (required on
/// many drivers for `NvEncEncodePicture`).
///
/// `inner` is last so it is dropped first (reverse field drop order) when not using custom Drop;
/// we use custom [`Drop`] on [`NvencInner`] for ordering.
pub struct NvencVideoEncoder {
    width: u32,
    frame_idx: usize,
    gop_frames: u32,
    /// COM identity of the texture last passed to `register_resource_dx11` (invalidates registration when it changes).
    registered_tex: Option<*mut std::ffi::c_void>,
    inner: NvencInner,
}

/// Capture uses one thread for encode; the `nvenc` safe wrappers omit `Send`.
unsafe impl Send for NvencVideoEncoder {}

impl NvencVideoEncoder {
    pub fn try_new(capture_device: &ID3D11Device, config: &crate::EncoderConfig) -> anyhow::Result<Self> {
        nvenc::nvenc_init().context("load nvEncodeAPI64.dll / NvEncodeAPICreateInstance")?;

        let width = config.width;
        let height = config.height;
        anyhow::ensure!(
            width % 2 == 0 && height % 2 == 0,
            "NVENC needs even width/height"
        );

        let session: Session<nvenc::session::NeedsConfig> =
            Session::open_dx(capture_device).map_err(|e| anyhow::anyhow!("NVENC open_dx: {e:?}"))?;

        let codecs = session
            .get_encode_codecs()
            .map_err(|e| anyhow::anyhow!("get_encode_codecs: {e:?}"))?;
        let preset_list = session
            .get_encode_presets(NV_ENC_CODEC_H264_GUID)
            .map_err(|e| anyhow::anyhow!("get_encode_presets: {e:?}"))?;
        let (preset_guid, preset_label) = pick_h264_preset(&codecs, &preset_list)?;

        let tuning = tuning_info_from_env();

        let (session, mut preset_config) = session
            .get_encode_preset_config_ex(
                NV_ENC_CODEC_H264_GUID,
                preset_guid.clone(),
                tuning,
            )
            .map_err(|e| anyhow::anyhow!("get_encode_preset_config_ex: {e:?}"))?;

        let g = gcd(width, height).max(1);
        let dar = [width / g, height / g];

        let gop_frames = config.fps.saturating_mul(2).max(30);
        preset_config.preset_cfg.gop_len = gop_frames;
        preset_config.preset_cfg.frame_interval_p = 1;
        preset_config.preset_cfg.rc_params.rate_control_mode = NVencParamsRcMode::VBR;
        preset_config.preset_cfg.rc_params.average_bit_rate = config.bitrate_bps;
        // One input frame must produce one bitstream for our synchronous capture API. HighQuality
        // presets can enable lookahead, which makes the first `encode_picture` return
        // `NeedMoreInput` with no output until more frames are fed — that broke the app and
        // triggered an OpenH264 fallback (catastrophic for 1080p60).
        preset_config.preset_cfg.rc_params.look_ahead_depth = 0;

        let tuning_label = match tuning {
            NVencTuningInfo::HighQuality => "HighQuality",
            NVencTuningInfo::LowLatency => "LowLatency",
            NVencTuningInfo::UltraLowLatency => "UltraLowLatency",
            NVencTuningInfo::UltraHighQuality => "UltraHighQuality",
            _ => "other",
        };
        tracing::info!(
            "NVENC encode: preset {} (best of P7→P2 on this GPU), tuning {tuning_label} (RS_CAPTURE_NVENC_TUNING), {} bps VBR",
            preset_label,
            config.bitrate_bps
        );

        // Match `vendor/nvenc/examples/simple_encode.rs` (DX11 texture → register → encode).
        let init = InitParams {
            encode_guid: NV_ENC_CODEC_H264_GUID,
            preset_guid,
            resolution: [width, height],
            aspect_ratio: dar,
            frame_rate: [config.fps.max(1), 1],
            tuning_info: tuning,
            buffer_format: NVencBufferFormat::ARGB,
            encode_config: &mut preset_config.preset_cfg,
            enable_ptd: true,
            max_encoder_resolution: [0, 0],
        };

        let encoder = session
            .init_encoder(init)
            .map_err(|e| anyhow::anyhow!("init_encoder: {e:?}"))?;

        let bitstream = encoder
            .create_bitstream_buffer()
            .map_err(|e| anyhow::anyhow!("create_bitstream_buffer: {e:?}"))?;

        Ok(Self {
            width,
            frame_idx: 0,
            gop_frames,
            registered_tex: None,
            inner: NvencInner {
                bitstream: ManuallyDrop::new(bitstream),
                registered: None,
                encoder: ManuallyDrop::new(encoder),
            },
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

    fn ensure_registered(&mut self, tex: &ID3D11Texture2D) -> anyhow::Result<()> {
        let p = tex.as_raw();
        if self.registered_tex == Some(p) && self.inner.registered.is_some() {
            return Ok(());
        }
        self.inner.registered = None;
        self.registered_tex = None;

        let pitch = self.width.saturating_mul(4);
        let reg = self
            .inner
            .encoder
            .register_resource_dx11(tex, NVencBufferFormat::ARGB, pitch)
            .map_err(|e| anyhow::anyhow!("register_resource_dx11: {e:?}"))?;
        self.inner.registered = Some(reg);
        self.registered_tex = Some(p);
        Ok(())
    }
}

impl VideoEncoder for NvencVideoEncoder {
    fn encode_i420(&mut self, _i420: &[u8], _timestamp_us: u64) -> anyhow::Result<EncodedPacket> {
        anyhow::bail!(
            "NVENC uses the GPU BGRA path (encode_bgra_texture); do not call encode_i420 for NVENC"
        )
    }

    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }

    fn supports_bgra_gpu_encode(&self) -> bool {
        true
    }

    fn encode_bgra_texture(
        &mut self,
        tex: &ID3D11Texture2D,
        timestamp_us: u64,
    ) -> anyhow::Result<EncodedPacket> {
        self.ensure_registered(tex)?;

        let reg = self
            .inner
            .registered
            .as_ref()
            .context("NVENC registered resource missing after ensure_registered")?;

        let force_idr = (self.frame_idx == 0)
            || (self.gop_frames > 0 && (self.frame_idx as u32) % self.gop_frames == 0);
        let pic_ty = if force_idr {
            NVencPicType::IDR
        } else {
            NVencPicType::P
        };

        let enc = self
            .inner
            .encoder
            .encode_picture(
                reg,
                &*self.inner.bitstream,
                self.frame_idx,
                timestamp_us,
                NVencBufferFormat::ARGB,
                NVencPicStruct::Frame,
                pic_ty,
                None,
            );
        match enc {
            Ok(()) | Err(NVencError::NeedMoreInput) => {}
            Err(e) => anyhow::bail!("encode_picture: {e:?}"),
        }

        self.frame_idx += 1;

        let guard = self.lock_bitstream_wait()?;
        let data = guard.as_slice();
        if data.is_empty() {
            anyhow::bail!(
                "NVENC bitstream empty after encode (lookahead/reorder?); try RS_CAPTURE_NVENC_TUNING=low_latency"
            );
        }
        let data = data.to_vec();
        drop(guard);

        Ok(EncodedPacket {
            data,
            timestamp_us,
            is_keyframe: force_idr,
            codec: VideoCodec::H264,
        })
    }
}
