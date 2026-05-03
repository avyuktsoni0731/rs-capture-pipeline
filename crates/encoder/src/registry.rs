//! Windows encoder **selection order** for [`crate::VideoEncoder`].
//!
//! ## Today
//! **NVENC** (if NVIDIA stack + D3D11 and env allow) → **OpenH264** (software).
//!
//! ## Planned (same selection API)
//! **AMD AMF (VCE)** and **Intel Quick Sync** will be probed between NVENC and OpenH264 using the
//! same `EncoderConfig` + `ID3D11Device` surface path where applicable.

use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::{create_best_encoder, EncoderConfig, VideoEncoder};

/// Creates the best available H.264 encoder for the current GPU/driver stack.
///
/// This is the single entry point hosts should use; today it forwards to [`create_best_encoder`].
/// When AMF/QSV backends land, they will be chained here without changing call sites.
pub fn create_windows_encoder(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    create_best_encoder(device, config)
}
