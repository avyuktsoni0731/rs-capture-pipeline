//! Intel **Quick Sync Video** hook: DXGI probe + encoder factory slot (NVENC → **QSV** → OpenH264).
//!
//! Full H.264 encode via oneVPL / D3D11 is not wired yet; [`try_create_qsv_encoder`] is the
//! extension point. [`intel_adapter_present`] is cheap and safe to call for UI or telemetry.

use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

use crate::traits::{EncoderConfig, VideoEncoder};

/// `true` if any DXGI adapter reports **Intel** (`VendorId == 0x8086`).
pub fn intel_adapter_present() -> bool {
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(_) => return false,
    };
    for idx in 0u32..32 {
        let adapter = match unsafe { factory.EnumAdapters1(idx) } {
            Ok(a) => a,
            Err(_) => break,
        };
        let desc = match unsafe { adapter.GetDesc1() } {
            Ok(d) => d,
            Err(_) => continue,
        };
        if desc.VendorId == 0x8086 {
            return true;
        }
    }
    false
}

/// Try to open an Intel QSV-backed H.264 encoder on the capture [`ID3D11Device`].
///
/// Today this **always** returns an error (encode path not implemented). The registry still calls
/// it after NVENC fails when [`intel_adapter_present`] is true so the fallback order and logging
/// match the final design.
pub(crate) fn try_create_qsv_encoder(
    _device: &ID3D11Device,
    _config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    anyhow::bail!(
        "Intel QSV H.264 encoder not implemented (oneVPL / D3D11 path pending); use OpenH264 fallback"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_probe_does_not_panic() {
        let _ = intel_adapter_present();
    }
}
