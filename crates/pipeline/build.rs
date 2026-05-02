//! Compile `shaders/color_convert.hlsl` to **DXBC** for D3D11.
//! Modern `dxc` often targets SM 6 + DXIL, which `ID3D11Device::CreateComputeShader` does not load.
//! The legacy `fxc` front-end in the Windows SDK produces SM 5.0 DXBC (header `DXBC`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let shader = manifest_dir
        .join("../../shaders/color_convert.hlsl")
        .canonicalize()
        .expect("canonicalize shader path");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let cso = out_dir.join("color_convert.cso");

    println!("cargo:rerun-if-changed={}", shader.display());
    println!("cargo:rerun-if-env-changed=FXC");

    let fxc = find_fxc().unwrap_or_else(|| {
        panic!(
            "Could not find fxc.exe (Windows SDK). Install the Windows SDK or set FXC to its path."
        )
    });

    let status = Command::new(&fxc)
        .args([
            "/T",
            "cs_5_0",
            "/E",
            "CSMain",
            "/Fo",
            cso.to_str().expect("utf8 out path"),
            shader.to_str().expect("utf8 shader path"),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run {:?}: {e}", fxc.display()));

    if !status.success() {
        panic!("fxc failed compiling {}", shader.display());
    }

    let meta = std::fs::metadata(&cso).expect("cso metadata");
    if meta.len() < 32 {
        panic!("color_convert.cso is too small ({} bytes)", meta.len());
    }
}

fn find_fxc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FXC") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(bin) = std::env::var("WindowsSdkVerBinPath") {
        candidates.push(PathBuf::from(bin).join("x64").join("fxc.exe"));
    }

    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(read) = std::fs::read_dir(kits) {
        for e in read.flatten() {
            let p = e.path().join("x64").join("fxc.exe");
            if p.exists() {
                candidates.push(p);
            }
        }
    }

    candidates.sort();
    candidates.pop()
}
