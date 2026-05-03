//! Video encoders: NVENC (Windows, D3D11) when available, otherwise OpenH264 (software).

mod annex_b;
mod i420;
mod nvenc;
mod openh264_enc;
pub mod registry;
mod traits;

pub use i420::nv12_readback_to_i420;
pub use registry::{create_windows_encoder, WindowsEncoderPreference};
pub use openh264_enc::OpenH264VideoEncoder;
pub use traits::{EncodedPacket, EncoderConfig, VideoCodec, VideoEncoder};

use windows::Win32::Graphics::Direct3D11::ID3D11Device;

/// Prefer NVENC when `device` is provided and the driver stack is available; fall back to OpenH264.
///
/// Uses [`WindowsEncoderPreference::from_env`] (same rules as before):
/// - `RS_CAPTURE_ENCODER=openh264` — force software encoding only.
/// - `RS_CAPTURE_NVENC=0` — skip NVENC (same effect as OpenH264-only for the video path).
///
/// Embeddable hosts should call [`create_windows_encoder`] with an explicit [`WindowsEncoderPreference`]
/// so settings do not depend on process environment.
///
/// When NVENC is used, the app registers the internal BGRA D3D texture (OBS-style) instead of
/// host I420. Encoder defaults aim at OBS-like recording: **HighQuality** tuning, preset **P7→P2**
/// (pick best), bitrate from the app (default ~45 Mbps — see `RS_CAPTURE_VIDEO_BITRATE`).
/// If `encode_bgra_texture` / registration fails, the app can swap to OpenH264 automatically
/// (see `capture-pipeline-app` main loop).
pub fn create_best_encoder(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    create_encoder_with_preference(device, config, registry::WindowsEncoderPreference::from_env())
}

pub fn create_encoder_with_preference(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
    preference: registry::WindowsEncoderPreference,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    use registry::WindowsEncoderPreference::*;

    if matches!(preference, SoftwareOnly) {
        tracing::info!("Windows encoder: OpenH264 only (software-only preference)");
        return openh264_encoder_from_config(config);
    }

    if let Some(dev) = device {
        match nvenc::NvencVideoEncoder::try_new(dev, config) {
            Ok(enc) => {
                tracing::info!(
                    "Using NVENC H.264 at {}x{} @ {} fps, {} bps (VBR; preset/tuning logged by NVENC init)",
                    config.width,
                    config.height,
                    config.fps,
                    config.bitrate_bps
                );
                return Ok(Box::new(enc));
            }
            Err(e) => {
                let prefer = matches!(preference, PreferNvenc);
                tracing::warn!(
                    error = %e,
                    prefer_nvenc = prefer,
                    "NVENC init failed; falling back to OpenH264"
                );
            }
        }
    } else if matches!(preference, PreferNvenc) {
        tracing::warn!("PreferNvenc but no D3D11 device; using OpenH264");
    }

    openh264_encoder_from_config(config)
}

/// OpenH264 only (no NVENC), for fallbacks and forced software encoding.
pub fn openh264_encoder_from_config(config: &EncoderConfig) -> anyhow::Result<Box<dyn VideoEncoder>> {
    tracing::info!(
        "Using OpenH264 (software) at {}x{} @ {} fps, {} bps",
        config.width,
        config.height,
        config.fps,
        config.bitrate_bps
    );
    Ok(Box::new(OpenH264VideoEncoder::new(
        config.width,
        config.height,
        config.bitrate_bps,
    )?))
}
