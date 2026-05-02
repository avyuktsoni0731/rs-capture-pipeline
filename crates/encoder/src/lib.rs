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
/// Set `RS_CAPTURE_ENCODER=openh264` to force software encoding. Pass `device: None` on platforms
/// without D3D11 (OpenH264 only).
pub fn create_best_encoder(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    let force_sw = std::env::var("RS_CAPTURE_ENCODER")
        .map(|s| s.eq_ignore_ascii_case("openh264"))
        .unwrap_or(false);

    if !force_sw {
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
        tracing::info!("RS_CAPTURE_ENCODER=openh264: using OpenH264");
    }

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
