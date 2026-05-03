//! Windows encoder **selection order** for [`crate::VideoEncoder`].
//!
//! ## Today
//! **NVENC** (NVIDIA + D3D11, when allowed) → **Media Foundation hardware H.264** (`mf_h264_hw`;
//! Intel QSV / AMD AMF when exposed as sync hardware MFTs) → **OpenH264** (software).
//!
//! GPU/back-end smoke: run the capture app on a machine with the target GPU and confirm logs show the
//! expected encoder; there is no automated GPU encode integration test in CI.
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
        Self::from_env_tokens(
            std::env::var("RS_CAPTURE_ENCODER").ok().as_deref(),
            std::env::var("RS_CAPTURE_NVENC").ok().as_deref(),
            std::env::var("RS_CAPTURE_NVENC_REQUIRED").ok().as_deref(),
        )
    }

    /// Pure mapping for tests and embedders that supply explicit strings instead of reading `std::env`.
    pub fn from_env_tokens(
        rs_capture_encoder: Option<&str>,
        rs_capture_nvenc: Option<&str>,
        rs_capture_nvenc_required: Option<&str>,
    ) -> Self {
        let force_sw = rs_capture_encoder
            .map(|s| s.eq_ignore_ascii_case("openh264"))
            .unwrap_or(false);
        let skip_nvenc = rs_capture_nvenc
            .map(|s| s == "0" || s.eq_ignore_ascii_case("off"))
            .unwrap_or(false);
        if force_sw || skip_nvenc {
            Self::SoftwareOnly
        } else if rs_capture_nvenc_required
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

#[cfg(test)]
mod from_env_tests {
    use super::WindowsEncoderPreference;

    #[test]
    fn cleared_tokens_default_to_auto() {
        assert_eq!(
            WindowsEncoderPreference::from_env_tokens(None, None, None),
            WindowsEncoderPreference::Auto
        );
    }

    #[test]
    fn rs_capture_encoder_openh264_forces_software_only() {
        assert_eq!(
            WindowsEncoderPreference::from_env_tokens(Some("openh264"), None, None),
            WindowsEncoderPreference::SoftwareOnly
        );
    }

    #[test]
    fn rs_capture_nvenc_off_forces_software_only() {
        assert_eq!(
            WindowsEncoderPreference::from_env_tokens(None, Some("0"), None),
            WindowsEncoderPreference::SoftwareOnly
        );
    }

    #[test]
    fn rs_capture_nvenc_required_sets_require_nvenc() {
        assert_eq!(
            WindowsEncoderPreference::from_env_tokens(None, None, Some("1")),
            WindowsEncoderPreference::RequireNvenc
        );
    }

    #[test]
    fn software_only_wins_over_require_nvenc() {
        assert_eq!(
            WindowsEncoderPreference::from_env_tokens(Some("openh264"), None, Some("1")),
            WindowsEncoderPreference::SoftwareOnly
        );
    }
}
