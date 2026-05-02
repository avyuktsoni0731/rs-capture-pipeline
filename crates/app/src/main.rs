//! Phase 1: capture 10 WGC frames from the default display and save PNGs.

use std::time::{Duration, Instant};

use anyhow::Context;
use capture::{
    copy_texture_to_rgba, create_d3d11_device, default_display_id, frame_to_texture, D3d11Context,
    WgcSession,
};
use image::{ImageBuffer, Rgba};
use tracing::{debug, info};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "capture_out".to_string());
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create_dir_all {out_dir}"))?;

    info!("Creating D3D11 device…");
    let D3d11Context { device, context } = create_d3d11_device()?;

    let display_id = default_display_id().context("enumerate displays")?;
    info!("Starting WGC for default display…");
    let wgc = WgcSession::new_for_display(&device, display_id)?;

    let mut saved: u32 = 0;
    let started = Instant::now();
    let deadline = Duration::from_secs(30);

    while saved < 10 {
        if started.elapsed() > deadline {
            anyhow::bail!("timed out waiting for 10 frames (got {saved})");
        }

        match wgc.try_next_frame() {
            Ok(frame) => {
                let tex = frame_to_texture(&frame).context("frame_to_texture")?;
                let (w, h, rgba) =
                    copy_texture_to_rgba(&device, &context, &tex).context("copy_texture_to_rgba")?;

                let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                    ImageBuffer::from_raw(w, h, rgba).context("ImageBuffer::from_raw")?;

                let path = format!("{out_dir}/frame_{saved:02}.png");
                img.save_with_format(&path, image::ImageFormat::Png)
                    .with_context(|| format!("save {path}"))?;
                info!("Wrote {path} ({w}x{h})");
                saved += 1;
            }
            Err(e) => {
                debug!(code = ?e.code(), "no frame yet, retrying…");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    info!("Done — saved {saved} PNGs under {out_dir}/");
    Ok(())
}
