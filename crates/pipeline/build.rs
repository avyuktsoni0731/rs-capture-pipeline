//! Compile `shaders/color_convert.hlsl` with the DirectX Shader Compiler (dxc).

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
    println!("cargo:rerun-if-env-changed=DXC");

    let dxc = find_dxc().unwrap_or_else(|| {
        panic!(
            "Could not find dxc.exe. Install the Windows SDK or set DXC to the full path to dxc.exe."
        )
    });

    let status = Command::new(&dxc)
        .args([
            "-T",
            "cs_5_0",
            "-E",
            "CSMain",
            "-Fo",
            cso.to_str().expect("utf8 out path"),
            shader.to_str().expect("utf8 shader path"),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run {:?}: {e}", dxc.display()));

    if !status.success() {
        panic!("dxc failed compiling {}", shader.display());
    }
}

fn find_dxc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DXC") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(bin) = std::env::var("WindowsSdkVerBinPath") {
        candidates.push(PathBuf::from(bin).join("x64").join("dxc.exe"));
    }

    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(read) = std::fs::read_dir(kits) {
        for e in read.flatten() {
            let p = e.path().join("x64").join("dxc.exe");
            if p.exists() {
                candidates.push(p);
            }
        }
    }

    candidates.sort();
    candidates.pop()
}
