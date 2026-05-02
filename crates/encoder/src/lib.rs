//! Video encoders: trait layer + OpenH264 (software) path while NVENC/AMF bindings are added.

mod annex_b;
mod i420;
mod nvenc;
mod openh264_enc;
mod traits;

pub use i420::nv12_readback_to_i420;
pub use openh264_enc::OpenH264VideoEncoder;
pub use traits::{EncodedPacket, EncoderConfig, VideoCodec, VideoEncoder};

/// Prefer hardware later; today returns OpenH264 H.264.
pub fn create_best_encoder(config: &EncoderConfig) -> anyhow::Result<Box<dyn VideoEncoder>> {
    tracing::info!(
        "Using OpenH264 (software) at {}x{} @ {} fps, {} bps — NVENC integration pending",
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
