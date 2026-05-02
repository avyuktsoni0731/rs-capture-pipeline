//! WGC capture + OpenH264 video + WASAPI system-audio WAV, with optional final ffmpeg mux.

use std::io::Write;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Context;
use audio::AudioCapture;
use capture::{
    create_d3d11_device, default_display_id, frame_to_texture, D3d11Context, WgcSession,
};
use encoder::{self, VideoEncoder};
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
        .unwrap_or(0);
    let audio_enabled = std::env::args()
        .nth(3)
        .map(|s| s != "noaudio")
        .unwrap_or(true);
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create_dir_all {out_dir}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop_flag = Arc::clone(&stop);
        ctrlc::set_handler(move || {
            stop_flag.store(true, Ordering::SeqCst);
        })
        .context("install Ctrl+C handler")?;
    }

    info!("Creating D3D11 device…");
    let D3d11Context { device, context } = create_d3d11_device()?;

    let converter = BgraToNv12Converter::new(&device).context("BgraToNv12Converter")?;

    let display_id = default_display_id().context("enumerate displays")?;
    info!("Starting WGC for default display…");
    let wgc = WgcSession::new_for_display(&device, display_id)?;

    let mut saved: u32 = 0;
    // Wall-clock origin for encoder timestamps (set on first encoded frame).
    let mut video_t0: Option<Instant> = None;
    // Previous frame wall time for variable MP4 sample durations.
    let mut last_frame_wall: Option<Instant> = None;

    let mut pool_and_size: Option<(TexturePool, FrameSize)> = None;
    let mut video_enc: Option<Box<dyn VideoEncoder>> = None;
    let mut h264_out: Option<std::fs::File> = None;
    let mut mp4_out: Option<output::Mp4H264File> = None;
    let mut audio_cap: Option<audio::WasapiLoopbackCapture> = None;
    let mut audio_wav: Option<audio::WavFileWriter> = None;
    let mut audio_samples_total: u64 = 0;

    info!(
        "Capturing at {} FPS, limit={} (0 means unlimited), system-audio={}",
        FPS, frame_count, audio_enabled
    );

    while !stop.load(Ordering::SeqCst) && (frame_count == 0 || saved < frame_count) {
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

                    // Start system audio only once video is live so WAV/MUX align with frame 0.
                    if audio_enabled && audio_cap.is_none() {
                        let cap = audio::WasapiLoopbackCapture::new().context("WasapiLoopbackCapture::new")?;
                        info!(
                            "WASAPI format: {} Hz, {} ch, {} bits",
                            cap.sample_rate(),
                            cap.channels(),
                            cap.bits_per_sample()
                        );
                        let wav_path = format!("{out_dir}/audio.wav");
                        audio_wav = Some(
                            audio::WavFileWriter::create(&wav_path, cap.sample_rate(), cap.channels())
                                .with_context(|| format!("create {wav_path}"))?,
                        );
                        info!("Writing system audio to WAV: {wav_path}");
                        audio_cap = Some(cap);
                    }
                }

                let (pool, _) = pool_and_size.as_ref().unwrap();
                let targets = pool.acquire().expect("pool empty");
                converter
                    .convert(&context, &device, &tex, &targets.y, &targets.uv)
                    .context("BgraToNv12Converter::convert")?;

                let (_yw, _yh, y_bytes) =
                    copy_r8_texture_to_bytes(&device, &context, &targets.y)?;
                let (_uvw, _uvh, uv_bytes) =
                    copy_rg8_uint_texture_to_bytes(&device, &context, &targets.uv)?;

                let (pool, size) = pool_and_size.as_ref().unwrap();
                let i420 = encoder::nv12_readback_to_i420(
                    &y_bytes,
                    &uv_bytes,
                    size.width,
                    size.height,
                )
                    .context("nv12_readback_to_i420")?;

                let wall_now = Instant::now();
                let t0 = video_t0.get_or_insert(wall_now);
                let ts_us = wall_now
                    .duration_since(*t0)
                    .as_micros()
                    .min(u128::from(u64::MAX)) as u64;

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

                let mp4 = mp4_out.as_mut().unwrap();
                let v_ts = mp4.video_timescale();
                let nominal = std::cmp::max(1u32, v_ts / FPS);
                let max_dur = nominal.saturating_mul(10);
                let duration_ts = if saved == 0 {
                    nominal
                } else {
                    let prev = last_frame_wall.context("last_frame_wall after first frame")?;
                    let delta = wall_now.saturating_duration_since(prev);
                    wall_duration_to_ts_units(delta, v_ts)
                        .min(max_dur)
                        .max(1)
                };
                last_frame_wall = Some(wall_now);

                mp4.write_annex_b_frame_with_duration(&pkt.data, pkt.is_keyframe, duration_ts)
                    .context("write clip.mp4")?;

                if let Some(cap) = audio_cap.as_mut() {
                    while let Some(chunk) = cap.try_read_chunk().context("try_read_chunk")? {
                        audio_samples_total += chunk.samples_f32.len() as u64;
                        if let Some(wav) = audio_wav.as_mut() {
                            wav.write_f32_interleaved(&chunk.samples_f32)
                                .context("write audio.wav")?;
                        }
                    }
                }

                pool.release(targets);

                saved += 1;
                if saved % 300 == 0 {
                    info!("Recorded {} video frames...", saved);
                }
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
    if let Some(w) = audio_wav {
        w.finalize().context("finalize audio.wav")?;
    }

    let video_mp4 = format!("{out_dir}/clip.mp4");
    let audio_wav = format!("{out_dir}/audio.wav");
    let muxed_mp4 = format!("{out_dir}/clip_with_audio.mp4");
    if audio_enabled {
        match try_mux_with_ffmpeg(&video_mp4, &audio_wav, &muxed_mp4) {
            Ok(true) => info!("Muxed final file with audio: {muxed_mp4}"),
            Ok(false) => info!(
                "ffmpeg not found; keeping separate files: {video_mp4} + {audio_wav}"
            ),
            Err(e) => info!(
                "ffmpeg mux failed ({e}); keeping separate files: {video_mp4} + {audio_wav}"
            ),
        }
    }

    info!(
        "Done — {out_dir}/clip.h264 + {out_dir}/clip.mp4, frames={}, audio_samples={}",
        saved,
        audio_samples_total
    );
    Ok(())
}

fn try_mux_with_ffmpeg(video_mp4: &str, audio_wav: &str, output_mp4: &str) -> anyhow::Result<bool> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            video_mp4,
            "-i",
            audio_wav,
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
            output_mp4,
        ])
        .status();

    match status {
        Ok(s) => {
            anyhow::ensure!(s.success(), "ffmpeg exited with status {s}");
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context("spawn ffmpeg"),
    }
}

fn wall_duration_to_ts_units(d: Duration, timescale_hz: u32) -> u32 {
    let micros = d.as_micros().min(u128::from(u64::MAX)) as u64;
    let v = micros
        .saturating_mul(u64::from(timescale_hz))
        .saturating_div(1_000_000);
    u32::try_from(v).unwrap_or(u32::MAX).max(1)
}
