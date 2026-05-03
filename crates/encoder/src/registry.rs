//! Windows encoder **selection order** for [`crate::VideoEncoder`].
//!
//! ## Today
//! **NVENC** (NVIDIA + D3D11, when allowed) → **Media Foundation hardware H.264** (`mf_h264_hw`;
//! Intel QSV / AMD AMF when exposed as sync hardware MFTs) → **OpenH264** (software).
//!
//! ## Planned (same selection API)
//! Optional native **AMF** / **oneVPL** paths if MF enumeration is insufficient on some drivers.

use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::{create_encoder_with_preference, EncoderConfig, VideoEncoder};

/// Host-facing hint for Windows H.264 backend selection (align with your embedder’s codec preference enum).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowsEncoderPreference {
    /// NVENC when available, otherwise OpenH264.
    #[default]
    Auto,
    /// Same NVENC→OpenH264 order as [`Self::Auto`]; reserved for future tuning (bitrate, AMF priority).
    PreferNvenc,
    /// NVENC only: [`crate::create_encoder_with_preference`] returns an error if NVENC cannot be opened.
    RequireNvenc,
    /// OpenH264 only (no NVENC attempt).
    SoftwareOnly,
}

impl WindowsEncoderPreference {
    /// Maps `RS_CAPTURE_ENCODER=openh264` and `RS_CAPTURE_NVENC=0` to [`Self::SoftwareOnly`].
    pub fn from_env() -> Self {
        let force_sw = std::env::var("RS_CAPTURE_ENCODER")
            .map(|s| s.eq_ignore_ascii_case("openh264"))
            .unwrap_or(false);
        let skip_nvenc = std::env::var("RS_CAPTURE_NVENC")
            .map(|s| s == "0" || s.eq_ignore_ascii_case("off"))
            .unwrap_or(false);
        if force_sw || skip_nvenc {
            Self::SoftwareOnly
        } else if std::env::var("RS_CAPTURE_NVENC_REQUIRED")
            .map(|s| {
                let t = s.trim();
                t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
            })
            .unwrap_or(false)
        {
            Self::RequireNvenc
        } else {
            Self::Auto
        }
    }
}

/// Creates an H.264 encoder using an explicit preference (embedders; avoids relying on process env).
pub fn create_windows_encoder(
    device: Option<&ID3D11Device>,
    config: &EncoderConfig,
    preference: WindowsEncoderPreference,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    create_encoder_with_preference(device, config, preference)
}
