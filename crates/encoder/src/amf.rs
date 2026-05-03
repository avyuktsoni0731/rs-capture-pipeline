//! AMD GPU DXGI probe (telemetry / future native AMF hook).
//!
//! When an AMD adapter is present, NVIDIA NVENC init fails and `mf_h264_hw` still enumerates
//! **hardware** MF encoders — many systems expose **AMF/VCE** through that catalog.

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

const AMD_VENDOR_ID: u32 = 0x1002;

/// `true` if any DXGI adapter reports **AMD** (`VendorId == 0x1002`).
pub fn amd_adapter_present() -> bool {
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
        if desc.VendorId == AMD_VENDOR_ID {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_probe_does_not_panic() {
        let _ = amd_adapter_present();
    }
}
