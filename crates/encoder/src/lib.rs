//! Video encoders: NVENC (Windows, D3D11) when available, otherwise OpenH264 (software).

mod annex_b;
mod i420;
mod nvenc;
mod openh264_enc;
mod traits;

pub use i420::nv12_readback_to_i420;
pub use openh264_enc::OpenH264VideoEncoder;
pub use traits::{EncodedPacket, EncoderConfig, VideoCodec, VideoEncoder};

use windows::Win32::Graphics::Direct3D11::ID3D11Device;

/// Prefer NVENC when `device` is provided and the driver stack is available; fall back to OpenH264.
///
/// - `RS_CAPTURE_ENCODER=openh264` — force software encoding only.
/// - `RS_CAPTURE_NVENC=0` — skip NVENC (same effect as OpenH264-only for the video path).
///
/// If NVENC initializes but the first `encode_picture` fails on your stack, the app can swap to
/// OpenH264 automatically (see `capture-pipeline-app` main loop).
pub fn create_best_encoder(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    let force_sw = std::env::var("RS_CAPTURE_ENCODER")
        .map(|s| s.eq_ignore_ascii_case("openh264"))
        .unwrap_or(false);
    let skip_nvenc = std::env::var("RS_CAPTURE_NVENC")
        .map(|s| s == "0" || s.eq_ignore_ascii_case("off"))
        .unwrap_or(false);

    if !force_sw && !skip_nvenc {
        if let Some(dev) = device {
            match nvenc::NvencVideoEncoder::try_new(dev, config) {
                Ok(enc) => {
                    tracing::info!(
                        "Using NVENC H.264 at {}x{} @ {} fps, {} bps (VBR, low latency)",
                        config.width,
                        config.height,
                        config.fps,
                        config.bitrate_bps
                    );
                    return Ok(Box::new(enc));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "NVENC init failed; falling back to OpenH264"
                    );
                }
            }
        }
    } else {
        if force_sw {
            tracing::info!("RS_CAPTURE_ENCODER=openh264: using OpenH264");
        }
        if skip_nvenc {
            tracing::info!("RS_CAPTURE_NVENC=0: skipping NVENC, using OpenH264");
        }
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
