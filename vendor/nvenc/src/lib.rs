#![doc = include_str!("../README.md")]

use std::{mem::MaybeUninit, sync::OnceLock};

use libloading::Library;

use crate::sys::{
    function_table::NVencFunctionList,
    result::{NVencError, NVencResult},
    version::NVENC_API_LIST_VERSION,
};

mod safe;
pub use safe::*;
/// Contains the original C Types, functions, enums, and constants
pub mod sys;

#[cfg(all(target_arch = "x86_64", target_os = "windows"))]
pub const NVENC_DLL: &str = "nvEncodeAPI64.dll";
#[cfg(all(target_arch = "x86", target_os = "windows"))]
pub const NVENC_DLL: &str = "nvEncodeAPI.dll";

#[cfg(target_os = "linux")]
/// Platform specific DLL name, `libnvidia-encode.so.1` on Linux, `nvEncodeAPI64.dll` on Windows
pub const NVENC_DLL: &str = "libnvidia-encode.so.1";

#[cfg(all(target_arch = "x86", windows))]
#[macro_export]
macro_rules! stdcall {
        (fn $args:tt $(-> $ret:tt)?) => { unsafe extern "stdcall" fn $args $(-> $ret)? };
}

#[doc(hidden)]
#[cfg(not(all(target_arch = "x86", windows)))]
#[macro_export]
macro_rules! stdcall {
        (fn $args:tt $(-> $ret:ty)?) => { unsafe extern "C" fn $args $(-> $ret)? };
}

pub(crate) static LIBRARY: OnceLock<NVENCLibrary> = OnceLock::new();

/// Library handle plus `NvEncodeAPICreateInstance` / `NvEncodeAPIGetMaxSupportedVersion` entry points.
pub struct NVENCLibrary {
    #[allow(unused)]
    // Lib must stay alive for nvenc to be used
    lib: Library,
    init: stdcall!(fn(list: *mut MaybeUninit<NVencFunctionList>) -> NVencResult),
    get_max_version: stdcall!(fn(version: *mut u32) -> NVencResult),
}

/// Attempt to init NVENC, returns an error if the library or loading functions cannot be found
pub fn nvenc_init() -> Result<&'static NVENCLibrary, libloading::Error> {
    if let Some(lib) = LIBRARY.get() {
        Ok(lib)
    } else {
        let lib = unsafe { Library::new(NVENC_DLL) }?;
        let init: stdcall!(fn(function_list: *mut MaybeUninit<NVencFunctionList>) -> NVencResult) =
            *unsafe { lib.get(b"NvEncodeAPICreateInstance") }?;
        let get_max_version: stdcall!(fn(version: *mut u32) -> NVencResult) =
            *unsafe { lib.get(b"NvEncodeAPIGetMaxSupportedVersion") }?;

        Ok(LIBRARY.get_or_init(|| NVENCLibrary {
            lib,
            init,
            get_max_version,
        }))
    }
}

impl NVENCLibrary {
    /// Create a new instance of the NVencFunctionList,
    pub fn create_instance(&self) -> Result<NVencFunctionList, NVencError> {
        let mut list = MaybeUninit::zeroed();
        unsafe { *(list.as_mut_ptr() as *mut u32) = NVENC_API_LIST_VERSION };
        unsafe { (self.init)(&raw mut list) }.into_error().unwrap();
        Ok(unsafe { list.assume_init() })
    }

    /// Raw value from `NvEncodeAPIGetMaxSupportedVersion`: **4 LSB = minor**, upper bits = major
    /// (NVIDIA Video Codec SDK). Do not mangle; use for `(major<<4)|minor` comparisons only.
    pub fn max_supported_version_packed(&self) -> Result<u32, NVencError> {
        let mut version = 0u32;
        unsafe { (self.get_max_version)(&raw mut version) }.into_error()?;
        Ok(version)
    }
}
