//! Phase 1 + 2: WGC capture → PNG dump; first frame also runs GPU BGRA→NV12 and saves Y/UV debug PNGs.

use std::time::{Duration, Instant};

use anyhow::Context;
use capture::{
    copy_texture_to_rgba, create_d3d11_device, default_display_id, frame_to_texture, D3d11Context,
    WgcSession,
};
use image::{ImageBuffer, Rgba};
use pipeline::{
    copy_r8_texture_to_bytes, copy_rg8_uint_texture_to_bytes, BgraToNv12Converter, FrameSize,
    TexturePool,
};
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

    let converter = BgraToNv12Converter::new(&device).context("BgraToNv12Converter")?;

    let display_id = default_display_id().context("enumerate displays")?;
    info!("Starting WGC for default display…");
    let wgc = WgcSession::new_for_display(&device, display_id)?;

    let mut saved: u32 = 0;
    let started = Instant::now();
    let deadline = Duration::from_secs(45);

    let mut pool_and_size: Option<(TexturePool, FrameSize)> = None;

    while saved < 10 {
        if started.elapsed() > deadline {
            anyhow::bail!("timed out waiting for 10 frames (got {saved})");
        }

        match wgc.try_next_frame() {
            Ok(frame) => {
                let tex = frame_to_texture(&frame).context("frame_to_texture")?;

                if pool_and_size.is_none() {
                    let mut d = Default::default();
                    unsafe { tex.GetDesc(&mut d) };
                    let size = FrameSize {
                        width: d.Width,
                        height: d.Height,
                    };
                    let pool = TexturePool::new(&device, size, 2)
                        .with_context(|| format!("TexturePool {}x{}", size.width, size.height))?;
                    pool_and_size = Some((pool, size));
                    info!(
                        "GPU NV12 pool + converter ready at {}x{}",
                        size.width, size.height
                    );
                }

                let (w, h, rgba) =
                    copy_texture_to_rgba(&device, &context, &tex).context("copy_texture_to_rgba")?;

                let path = format!("{out_dir}/frame_{saved:02}.png");
                let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                    ImageBuffer::from_raw(w, h, rgba).context("ImageBuffer::from_raw")?;
                img.save_with_format(&path, image::ImageFormat::Png)
                    .with_context(|| format!("save {path}"))?;
                info!("Wrote {path} ({w}x{h})");

                if saved == 0 {
                    let (pool, _) = pool_and_size.as_ref().unwrap();
                    let targets = pool.acquire().expect("pool empty");
                    converter
                        .convert(&context, &device, &tex, &targets.y, &targets.uv)
                        .context("BgraToNv12Converter::convert")?;

                    let (yw, yh, y_bytes) =
                        copy_r8_texture_to_bytes(&device, &context, &targets.y)?;
                    let mut y_rgba = Vec::with_capacity((yw * yh * 4) as usize);
                    for &v in &y_bytes {
                        let g = v;
                        y_rgba.extend_from_slice(&[g, g, g, 255]);
                    }
                    let y_img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(yw, yh, y_rgba)
                        .context("Y ImageBuffer")?;
                    let y_path = format!("{out_dir}/nv12_y_{saved:02}.png");
                    y_img
                        .save_with_format(&y_path, image::ImageFormat::Png)
                        .with_context(|| format!("save {y_path}"))?;
                    info!("Wrote {y_path} (luma from GPU NV12 path)");

                    let (uvw, uvh, uv_bytes) =
                        copy_rg8_uint_texture_to_bytes(&device, &context, &targets.uv)?;
                    let mut uv_rgba = Vec::with_capacity((uvw * uvh * 4) as usize);
                    for chunk in uv_bytes.chunks_exact(2) {
                        let cb = chunk[0];
                        let cr = chunk[1];
                        uv_rgba.extend_from_slice(&[cb, cr, 0, 255]);
                    }
                    let uv_img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                        ImageBuffer::from_raw(uvw, uvh, uv_rgba).context("UV ImageBuffer")?;
                    let uv_path = format!("{out_dir}/nv12_uv_{saved:02}.png");
                    uv_img
                        .save_with_format(&uv_path, image::ImageFormat::Png)
                        .with_context(|| format!("save {uv_path}"))?;
                    info!("Wrote {uv_path} (Cb→R, Cr→G for inspection)");

                    pool.release(targets);
                }

                saved += 1;
            }
            Err(e) => {
                debug!(code = ?e.code(), "no frame yet, retrying…");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    info!("Done — saved {saved} BGRA PNGs + nv12_y_00 / nv12_uv_00 under {out_dir}/");
    Ok(())
}
