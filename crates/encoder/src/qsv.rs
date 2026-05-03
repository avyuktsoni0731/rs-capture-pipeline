//! Intel **Quick Sync Video** path: DXGI probe + Media Foundation **hardware** H.264 MFT
//! (NVENC → **MF HW H.264 on Intel** → OpenH264).
//!
//! [`intel_adapter_present`] is for telemetry/UI only; selection always tries MF hardware after NVENC.
//! [`try_create_qsv_encoder`] wraps `MfH264HwEncoder` (NV12-in / Annex-B out).

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

/// Try Intel Quick Sync–class encode: **MF hardware H.264** with NV12 input (see [`crate::mf_h264_hw::MfH264HwEncoder`]).
///
/// `device` is passed into Media Foundation as an DXGI device manager when supported (`MFT_MESSAGE_SET_D3D_MANAGER`).
pub(crate) fn try_create_qsv_encoder(
    device: &ID3D11Device,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    let enc = crate::mf_h264_hw::MfH264HwEncoder::try_new(config, Some(device))?;
    Ok(Box::new(enc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_probe_does_not_panic() {
        let _ = intel_adapter_present();
    }
}
