//! WGC capture + OpenH264 + WASAPI: H.264 + AAC in `clip.mp4` (Media Foundation), WAV debug, optional ffmpeg remux.

use std::io::Write;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Context;
use audio::{AudioCapture, PcmChunk};
use capture::{
    create_d3d11_device, default_display_id, frame_to_texture, D3d11Context, WgcSession,
};
use encoder::{self, VideoEncoder};
use pipeline::{
    copy_r8_texture_to_bytes, copy_rg8_uint_texture_to_bytes, BgraToNv12Converter, FrameSize,
    TexturePool,
};
use audio_encoder::MfAacLcEncoder;
use crossbeam_channel::{unbounded, Receiver, Sender};
use tracing::{debug, info, warn};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
    ABOVE_NORMAL_PRIORITY_CLASS, THREAD_PRIORITY_ABOVE_NORMAL,
};

/// Nominal video FPS for encoder config + MP4 sample timing (actual capture rate follows WGC /
/// display refresh). Override per run: `RS_CAPTURE_FPS=60` (default 30).
fn capture_fps_from_env() -> u32 {
    const DEFAULT: u32 = 30;
    let Ok(s) = std::env::var("RS_CAPTURE_FPS") else {
        return DEFAULT;
    };
    match s.parse::<u32>() {
        Ok(n) if (1..=240).contains(&n) => n,
        Ok(n) => {
            warn!("RS_CAPTURE_FPS={n} is out of range 1–240; using {DEFAULT}");
            DEFAULT
        }
        Err(_) => {
            warn!("RS_CAPTURE_FPS={s:?} is not a number; using {DEFAULT}");
            DEFAULT
        }
    }
}

/// Target VBR bitrate for video (NVENC + OpenH264). Default **45 Mbps** (~OBS “high quality”
/// 1080p class). Lighter files: `RS_CAPTURE_VIDEO_BITRATE=8000000`.
fn video_bitrate_bps() -> u32 {
    const DEFAULT: u32 = 45_000_000;
    let Ok(s) = std::env::var("RS_CAPTURE_VIDEO_BITRATE") else {
        return DEFAULT;
    };
    if s.is_empty() {
        return DEFAULT;
    }
    match s.parse::<u64>() {
        Ok(n) if (500_000..=200_000_000).contains(&n) => n as u32,
        Ok(n) => {
            warn!("RS_CAPTURE_VIDEO_BITRATE={n} out of range 500000–200000000; using {DEFAULT}");
            DEFAULT
        }
        Err(_) => {
            warn!("RS_CAPTURE_VIDEO_BITRATE={s:?} not an integer; using {DEFAULT}");
            DEFAULT
        }
    }
}

enum AudioMsg {
    Ready {
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
    },
    Chunk(PcmChunk),
    Error(String),
}

fn audio_loopback_thread(stop: Arc<AtomicBool>, tx: Sender<AudioMsg>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let run = || -> anyhow::Result<()> {
        let mut cap = audio::WasapiLoopbackCapture::new()?;
        tx.send(AudioMsg::Ready {
            sample_rate: cap.sample_rate(),
            channels: cap.channels(),
            bits_per_sample: cap.bits_per_sample(),
        })
        .map_err(|_| anyhow::anyhow!("capture-pipeline dropped audio receiver before Ready"))?;

        while !stop.load(Ordering::SeqCst) {
            match cap.try_read_chunk() {
                Ok(Some(chunk)) => {
                    if tx.send(AudioMsg::Chunk(chunk)).is_err() {
                        break;
                    }
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(1)),
                Err(e) => {
                    let _ = tx.send(AudioMsg::Error(format!("{e:#}")));
                    break;
                }
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        let _ = tx.send(AudioMsg::Error(format!("{e:#}")));
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        // Under heavy CPU/GPU load (e.g. 4K YouTube seek), default scheduling can starve capture.
        // ABOVE_NORMAL is gentler than HIGH but still improves timekeeping vs browser decode.
        if std::env::var("RS_CAPTURE_NO_PRIORITY_BOOST").ok().as_deref() == Some("1") {
            info!("RS_CAPTURE_NO_PRIORITY_BOOST=1: process priority left at NORMAL");
        } else if SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS).is_ok() {
            info!("Process priority: ABOVE_NORMAL (set RS_CAPTURE_NO_PRIORITY_BOOST=1 to skip)");
        } else {
            warn!("SetPriorityClass(ABOVE_NORMAL) failed; capture may jitter more under load");
        }
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
    let fps = capture_fps_from_env();
    let bitrate_bps = video_bitrate_bps();
    if fps != 30 {
        info!("RS_CAPTURE_FPS={fps}: encoder and MP4 use this nominal rate (default 30)");
    }
    info!(
        "Video bitrate {} bps ({})",
        bitrate_bps,
        if std::env::var("RS_CAPTURE_VIDEO_BITRATE").is_ok() {
            "from RS_CAPTURE_VIDEO_BITRATE"
        } else {
            "default ~OBS-class 45 Mbps; set RS_CAPTURE_VIDEO_BITRATE to override"
        }
    );
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

    let mut converter = BgraToNv12Converter::new(&device).context("BgraToNv12Converter")?;

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
    let mut audio_rx: Option<Receiver<AudioMsg>> = None;
    let mut audio_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut audio_sample_rate: u32 = 0;
    let mut audio_pcm_channels: u16 = 2;
    let mut audio_wav: Option<audio::WavFileWriter> = None;
    let mut aac_enc: Option<MfAacLcEncoder> = None;
    let mut audio_samples_total: u64 = 0;
    // Drop this many PCM frames (per-channel time slots) from the start so MP4 audio matches frame 0.
    let mut pending_audio_frame_skip: u64 = 0;
    let mut audio_frame_skip_bootstrapped = false;
    // If NVENC fails with a device/API mismatch, swap to OpenH264 once (do not match generic
    // `encode_picture` errors — e.g. NeedMoreInput was wrongly swapping to CPU encode).
    let mut nvenc_swapped_to_openh264 = false;

    info!(
        "Capturing at {} fps, limit={} (0 = unlimited), system-audio={}",
        fps, frame_count, audio_enabled
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

                    let enc_cfg_boot =
                        encoder::EncoderConfig::new(size.width, size.height, fps, bitrate_bps);
                    video_enc = Some(encoder::create_best_encoder(Some(&device), &enc_cfg_boot)?);

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
                            fps,
                        )
                        .with_context(|| format!("create {mp4_path}"))?,
                    );
                    info!(
                        "GPU NV12 pool at {}x{} → {} (Annex-B) + {} (MP4 avc1); encoder selected at startup (NVENC uses GPU BGRA when available)",
                        size.width, size.height, path, mp4_path
                    );
                }

                let (pool, size) = pool_and_size.as_ref().unwrap();
                let enc_cfg =
                    encoder::EncoderConfig::new(size.width, size.height, fps, bitrate_bps);

                let use_gpu_nvenc = video_enc
                    .as_ref()
                    .expect("encoder initialized with pool")
                    .supports_bgra_gpu_encode();

                let mut targets_opt = None;
                if use_gpu_nvenc {
                    converter
                        .convert(&context, &device, &tex, None)
                        .context("BgraToNv12Converter::convert")?;
                } else {
                    let targets = pool.acquire().expect("pool empty");
                    converter
                        .convert(
                            &context,
                            &device,
                            &tex,
                            Some((&targets.y, &targets.uv)),
                        )
                        .context("BgraToNv12Converter::convert")?;
                    targets_opt = Some(targets);
                }
                unsafe {
                    context.Flush();
                }

                let wall_now = Instant::now();
                let first_wall_anchor = video_t0.is_none();
                let t0 = video_t0.get_or_insert(wall_now);

                // Start loopback **after** the first wall-clock anchor so PTS 0 matches frame 0,
                // and so we do not accumulate a second of preroll in the WASAPI ring buffer.
                if first_wall_anchor && audio_enabled && audio_rx.is_none() {
                    let (tx, rx) = unbounded::<AudioMsg>();
                    let stop_a = Arc::clone(&stop);
                    let join = std::thread::spawn(move || audio_loopback_thread(stop_a, tx));

                    match rx.recv() {
                        Ok(AudioMsg::Ready {
                            sample_rate,
                            channels,
                            bits_per_sample,
                        }) => {
                            audio_sample_rate = sample_rate;
                            audio_pcm_channels = channels;
                            info!(
                                "WASAPI format: {} Hz, {} ch, {} bits (dedicated capture thread)",
                                sample_rate, channels, bits_per_sample
                            );
                            let wav_path = format!("{out_dir}/audio.wav");
                            audio_wav = Some(
                                audio::WavFileWriter::create(&wav_path, sample_rate, channels)
                                    .with_context(|| format!("create {wav_path}"))?,
                            );
                            info!("Writing system audio to WAV: {wav_path}");
                            let aac_br = 128_000u32;
                            match MfAacLcEncoder::new(sample_rate, channels, aac_br) {
                                Ok(enc) => {
                                    if let Some(mp4) = mp4_out.as_mut() {
                                        mp4
                                            .enable_aac(sample_rate, channels, aac_br)
                                            .with_context(|| "Mp4H264File::enable_aac")?;
                                    }
                                    aac_enc = Some(enc);
                                    info!("In-process AAC (MF AAC-LC) muxed into clip.mp4");
                                }
                                Err(e) => {
                                    warn!(
                                        "MF AAC-LC unavailable ({e:#}); clip.mp4 stays video-only, audio remains in audio.wav"
                                    );
                                }
                            }
                            audio_rx = Some(rx);
                            audio_thread = Some(join);
                        }
                        Ok(AudioMsg::Error(e)) => {
                            warn!("WASAPI loopback failed to start: {e}");
                            let _ = join.join();
                        }
                        Ok(AudioMsg::Chunk(_)) => {
                            anyhow::bail!("internal error: first audio message was Chunk");
                        }
                        Err(_) => {
                            warn!("audio capture thread disconnected before Ready");
                            let _ = join.join();
                        }
                    }
                }

                let ts_us = wall_now
                    .duration_since(*t0)
                    .as_micros()
                    .min(u128::from(u64::MAX)) as u64;

                let allow_nvenc_runtime_fallback = std::env::var("RS_CAPTURE_ENCODER")
                    .map(|s| !s.eq_ignore_ascii_case("openh264"))
                    .unwrap_or(true);

                let pkt = if use_gpu_nvenc {
                    let bgra = converter
                        .bgra_copy_texture()
                        .context("internal BGRA copy texture for NVENC")?;
                    match video_enc
                        .as_mut()
                        .unwrap()
                        .encode_bgra_texture(bgra, ts_us)
                    {
                        Ok(p) => p,
                        Err(e) => {
                            let msg = format!("{e:#}");
                            if allow_nvenc_runtime_fallback
                                && !nvenc_swapped_to_openh264
                                && (msg.contains("InvalidDevice")
                                    || msg.contains("Device passed to the API")
                                    || msg.contains("InvalidEncoderDevice")
                                    || msg.contains("register_resource")
                                    || msg.contains("ResourceRegisterFailed"))
                            {
                                warn!(
                                    "NVENC GPU encode failed ({msg}); switching to OpenH264 for the rest of this run. \
                                     To skip NVENC up front: RS_CAPTURE_NVENC=0 or RS_CAPTURE_ENCODER=openh264"
                                );
                                let targets = pool.acquire().expect("pool empty");
                                converter
                                    .convert(
                                        &context,
                                        &device,
                                        &tex,
                                        Some((&targets.y, &targets.uv)),
                                    )
                                    .context("BgraToNv12Converter::convert (fallback)")?;
                                unsafe {
                                    context.Flush();
                                }
                                let (_yw, _yh, y_bytes) =
                                    copy_r8_texture_to_bytes(&device, &context, &targets.y)?;
                                let (_uvw, _uvh, uv_bytes) =
                                    copy_rg8_uint_texture_to_bytes(&device, &context, &targets.uv)?;
                                let i420 = encoder::nv12_readback_to_i420(
                                    &y_bytes,
                                    &uv_bytes,
                                    size.width,
                                    size.height,
                                )
                                .context("nv12_readback_to_i420 (fallback)")?;
                                pool.release(targets);
                                video_enc = Some(encoder::openh264_encoder_from_config(&enc_cfg)?);
                                nvenc_swapped_to_openh264 = true;
                                video_enc
                                    .as_mut()
                                    .unwrap()
                                    .encode_i420(&i420, ts_us)
                                    .context("encode_i420 (OpenH264 after NVENC failure)")?
                            } else {
                                return Err(e).context("encode_bgra_texture");
                            }
                        }
                    }
                } else {
                    let targets = targets_opt.as_ref().context("NV12 pool targets")?;
                    let (_yw, _yh, y_bytes) =
                        copy_r8_texture_to_bytes(&device, &context, &targets.y)?;
                    let (_uvw, _uvh, uv_bytes) =
                        copy_rg8_uint_texture_to_bytes(&device, &context, &targets.uv)?;
                    let i420 = encoder::nv12_readback_to_i420(
                        &y_bytes,
                        &uv_bytes,
                        size.width,
                        size.height,
                    )
                    .context("nv12_readback_to_i420")?;
                    video_enc
                        .as_mut()
                        .unwrap()
                        .encode_i420(&i420, ts_us)
                        .context("encode_i420")?
                };
                h264_out
                    .as_mut()
                    .unwrap()
                    .write_all(&pkt.data)
                    .context("write clip.h264")?;

                let mp4 = mp4_out.as_mut().unwrap();
                let v_ts = mp4.video_timescale();
                let nominal = std::cmp::max(1u32, v_ts / fps);
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

                if let Some(rx) = audio_rx.as_ref() {
                    if !audio_frame_skip_bootstrapped {
                        if let Some(t0) = video_t0.as_ref() {
                            if audio_sample_rate > 0 {
                                let elapsed = Instant::now().saturating_duration_since(*t0);
                                pending_audio_frame_skip =
                                    wall_duration_to_pcm_frames(elapsed, audio_sample_rate);
                                audio_frame_skip_bootstrapped = true;
                            }
                        }
                    }
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            AudioMsg::Chunk(mut chunk) => {
                                trim_interleaved_f32_frames_front(
                                    &mut chunk.samples_f32,
                                    audio_pcm_channels,
                                    &mut pending_audio_frame_skip,
                                );
                                if chunk.samples_f32.is_empty() {
                                    continue;
                                }
                                audio_samples_total += chunk.samples_f32.len() as u64;
                                if let Some(wav) = audio_wav.as_mut() {
                                    wav.write_f32_interleaved(&chunk.samples_f32)
                                        .context("write audio.wav")?;
                                }
                                if let Some(enc) = aac_enc.as_mut() {
                                    let aus = enc
                                        .push_interleaved_f32(&chunk.samples_f32)
                                        .context("AAC encode")?;
                                    for au in aus {
                                        mp4
                                            .write_aac_access_unit(&au, 1024)
                                            .context("write AAC to MP4")?;
                                    }
                                }
                            }
                            AudioMsg::Error(e) => {
                                warn!("audio capture thread: {e}");
                                break;
                            }
                            AudioMsg::Ready { .. } => {
                                warn!("unexpected duplicate WASAPI Ready message; ignoring");
                            }
                        }
                    }
                }

                if let Some(targets) = targets_opt {
                    pool.release(targets);
                }

                saved += 1;
                if saved > 0 && saved % (fps * 10).max(1) == 0 {
                    info!("Recorded {} video frames...", saved);
                }
            }
            Err(e) => {
                debug!(code = ?e.code(), "no frame yet, retrying…");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    stop.store(true, Ordering::SeqCst);
    if let Some(rx) = audio_rx.take() {
        if let Some(mp4) = mp4_out.as_mut() {
            for msg in rx.try_iter() {
                match msg {
                    AudioMsg::Chunk(mut chunk) => {
                        trim_interleaved_f32_frames_front(
                            &mut chunk.samples_f32,
                            audio_pcm_channels,
                            &mut pending_audio_frame_skip,
                        );
                        if chunk.samples_f32.is_empty() {
                            continue;
                        }
                        audio_samples_total += chunk.samples_f32.len() as u64;
                        if let Some(wav) = audio_wav.as_mut() {
                            wav.write_f32_interleaved(&chunk.samples_f32)
                                .context("shutdown drain: audio.wav")?;
                        }
                        if let Some(enc) = aac_enc.as_mut() {
                            let aus = enc
                                .push_interleaved_f32(&chunk.samples_f32)
                                .context("shutdown drain: AAC encode")?;
                            for au in aus {
                                mp4
                                    .write_aac_access_unit(&au, 1024)
                                    .context("shutdown drain: MP4 AAC")?;
                            }
                        }
                    }
                    AudioMsg::Error(e) => warn!("audio thread (shutdown drain): {e}"),
                    AudioMsg::Ready { .. } => {}
                }
            }
        }
    }
    if let Some(j) = audio_thread.take() {
        let _ = j.join();
    }

    if let Some(enc) = aac_enc.as_mut() {
        if let Some(mp4) = mp4_out.as_mut() {
            for au in enc.flush().context("AAC flush")? {
                mp4
                    .write_aac_access_unit(&au, 1024)
                    .context("write final AAC samples")?;
            }
        }
    }
    if let Some(m) = mp4_out {
        m.finish().context("finalize clip.mp4")?;
    }
    if let Some(w) = audio_wav {
        w.finalize().context("finalize audio.wav")?;
    }

    if audio_enabled && std::env::var("RS_CAPTURE_FFMPEG_MUX").ok().as_deref() == Some("1") {
        let video_mp4 = format!("{out_dir}/clip.mp4");
        let audio_wav = format!("{out_dir}/audio.wav");
        let muxed_mp4 = format!("{out_dir}/clip_with_audio.mp4");
        match try_mux_with_ffmpeg(&video_mp4, &audio_wav, &muxed_mp4) {
            Ok(true) => info!("RS_CAPTURE_FFMPEG_MUX: wrote {muxed_mp4}"),
            Ok(false) => info!("RS_CAPTURE_FFMPEG_MUX set but ffmpeg not found"),
            Err(e) => info!("RS_CAPTURE_FFMPEG_MUX: ffmpeg failed ({e})"),
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

/// Whole PCM frames (one time slot across all channels) to drop for a wall-clock duration.
fn wall_duration_to_pcm_frames(d: Duration, sample_rate: u32) -> u64 {
    let nanos = d.as_nanos().min(u128::from(u64::MAX) as u128);
    (nanos
        .saturating_mul(u128::from(sample_rate))
        / 1_000_000_000) as u64
}

fn trim_interleaved_f32_frames_front(
    samples: &mut Vec<f32>,
    channels: u16,
    skip_frames: &mut u64,
) {
    let ch = channels as usize;
    if ch == 0 || samples.is_empty() || *skip_frames == 0 {
        return;
    }
    let frame_count = samples.len() / ch;
    let take = (*skip_frames as usize).min(frame_count);
    if take > 0 {
        samples.drain(0..take * ch);
        *skip_frames = skip_frames.saturating_sub(take as u64);
    }
}
