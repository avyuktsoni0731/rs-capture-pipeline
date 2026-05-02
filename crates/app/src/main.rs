//! WGC capture → BGRA PNGs, GPU BGRA→NV12, software H.264 (OpenH264) to `clip.h264` (Annex-B).

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use capture::{
    copy_texture_to_rgba, create_d3d11_device, default_display_id, frame_to_texture, D3d11Context,
    WgcSession,
};
use encoder::{self, VideoEncoder};
use image::{ImageBuffer, Rgba};
use pipeline::{
    copy_r8_texture_to_bytes, copy_rg8_uint_texture_to_bytes, BgraToNv12Converter, FrameSize,
    TexturePool,
};
use tracing::{debug, info};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const FPS: u32 = 30;

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
    let frame_count: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(300);
    anyhow::ensure!(frame_count > 0, "frame_count must be > 0");
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create_dir_all {out_dir}"))?;

    info!("Creating D3D11 device…");
    let D3d11Context { device, context } = create_d3d11_device()?;

    let converter = BgraToNv12Converter::new(&device).context("BgraToNv12Converter")?;

    let display_id = default_display_id().context("enumerate displays")?;
    info!("Starting WGC for default display…");
    let wgc = WgcSession::new_for_display(&device, display_id)?;

    let mut saved: u32 = 0;
    let started = Instant::now();
    let deadline = Duration::from_secs(u64::from(frame_count).saturating_mul(2) / u64::from(FPS) + 120);

    let mut pool_and_size: Option<(TexturePool, FrameSize)> = None;
    let mut video_enc: Option<Box<dyn VideoEncoder>> = None;
    let mut h264_out: Option<std::fs::File> = None;
    let mut mp4_out: Option<output::Mp4H264File> = None;

    info!("Capturing {} frames at {} FPS", frame_count, FPS);

    while saved < frame_count {
        if started.elapsed() > deadline {
            anyhow::bail!("timed out waiting for {frame_count} frames (got {saved})");
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

                    let enc_cfg =
                        encoder::EncoderConfig::new(size.width, size.height, FPS, 8_000_000);
                    video_enc = Some(encoder::create_best_encoder(&enc_cfg)?);
                    let path = format!("{out_dir}/clip.h264");
                    h264_out = Some(
                        std::fs::File::create(&path)
                            .with_context(|| format!("create {path}"))?,
                    );
                    let mp4_path = format!("{out_dir}/clip.mp4");
                    mp4_out = Some(
                        output::Mp4H264File::create(
                            &mp4_path,
                            size.width as u16,
                            size.height as u16,
                            FPS,
                        )
                        .with_context(|| format!("create {mp4_path}"))?,
                    );
                    info!(
                        "GPU NV12 pool + OpenH264 at {}x{} → {} (Annex-B) + {} (MP4 avc1)",
                        size.width, size.height, path, mp4_path
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

                let (pool, _) = pool_and_size.as_ref().unwrap();
                let targets = pool.acquire().expect("pool empty");
                converter
                    .convert(&context, &device, &tex, &targets.y, &targets.uv)
                    .context("BgraToNv12Converter::convert")?;

                let (_yw, _yh, y_bytes) =
                    copy_r8_texture_to_bytes(&device, &context, &targets.y)?;
                let (_uvw, _uvh, uv_bytes) =
                    copy_rg8_uint_texture_to_bytes(&device, &context, &targets.uv)?;

                if saved == 0 {
                    let yw = w;
                    let yh = h;
                    let mut y_rgba = Vec::with_capacity((yw * yh * 4) as usize);
                    for &v in &y_bytes {
                        y_rgba.extend_from_slice(&[v, v, v, 255]);
                    }
                    let y_img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                        ImageBuffer::from_raw(yw, yh, y_rgba).context("Y ImageBuffer")?;
                    let y_path = format!("{out_dir}/nv12_y_{saved:02}.png");
                    y_img
                        .save_with_format(&y_path, image::ImageFormat::Png)
                        .with_context(|| format!("save {y_path}"))?;
                    info!("Wrote {y_path} (luma GPU path)");

                    let uw = (w + 1) / 2;
                    let uh = (h + 1) / 2;
                    let mut uv_rgba = Vec::with_capacity((uw * uh * 4) as usize);
                    for chunk in uv_bytes.chunks_exact(2) {
                        uv_rgba.extend_from_slice(&[chunk[0], chunk[1], 0, 255]);
                    }
                    let uv_img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                        ImageBuffer::from_raw(uw, uh, uv_rgba).context("UV ImageBuffer")?;
                    let uv_path = format!("{out_dir}/nv12_uv_{saved:02}.png");
                    uv_img
                        .save_with_format(&uv_path, image::ImageFormat::Png)
                        .with_context(|| format!("save {uv_path}"))?;
                    info!("Wrote {uv_path} (Cb/Cr as R/G)");
                }

                let i420 = encoder::nv12_readback_to_i420(&y_bytes, &uv_bytes, w, h)
                    .context("nv12_readback_to_i420")?;
                let ts_us = u64::from(saved) * 1_000_000 / u64::from(FPS);
                let pkt = video_enc
                    .as_mut()
                    .unwrap()
                    .encode_i420(&i420, ts_us)
                    .context("encode_i420")?;
                h264_out
                    .as_mut()
                    .unwrap()
                    .write_all(&pkt.data)
                    .context("write clip.h264")?;
                mp4_out
                    .as_mut()
                    .unwrap()
                    .write_annex_b_frame(&pkt.data, pkt.is_keyframe)
                    .context("write clip.mp4")?;

                pool.release(targets);

                saved += 1;
            }
            Err(e) => {
                debug!(code = ?e.code(), "no frame yet, retrying…");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    if let Some(m) = mp4_out {
        m.finish().context("finalize clip.mp4")?;
    }

    info!(
        "Done — PNGs + nv12 debug + {out_dir}/clip.h264 + {out_dir}/clip.mp4 (OpenH264), {} frames",
        frame_count
    );
    Ok(())
}
