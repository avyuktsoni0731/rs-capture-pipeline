//! Windows file recording implementation (`PipelineParams` → disk + optional ffmpeg remux).

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
use crate::events::{AudioChunk, VideoPacket};
use crate::params::{PipelineParams, RunStats};
use tracing::{debug, info, warn};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};

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

pub(crate) fn run_file_recording(
    params: &PipelineParams,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<RunStats> {
    if let Some(dir) = params.outputs.directory() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create_dir_all {}", dir.display()))?;
    }

    let mut stream_video_packets_sent: u64 = 0;
    let mut stream_audio_chunks_sent: u64 = 0;

    info!("Creating D3D11 device…");
    let D3d11Context { device, context } = create_d3d11_device()?;

    let mut converter = BgraToNv12Converter::new(&device).context("BgraToNv12Converter")?;

    let display_id = default_display_id().context("enumerate displays")?;
    info!("Starting WGC for default display…");
    let wgc = WgcSession::new_for_display(&device, display_id)?;

    let mut saved: u32 = 0;
    // Wall-clock origin for encoder timestamps (set on first encoded frame).
    let mut video_t0: Option<Instant> = None;
    // Previous frame cumulative timestamp (µs since video_t0); drives MP4 sample durations so they
    // match the encoder PTS and stay aligned with the audio (sample-count) timeline.
    let mut last_ts_us: u64 = 0;

    let mut pool_and_size: Option<(TexturePool, FrameSize)> = None;
    let mut nvenc_async: Option<crate::encode_async::NvencAsync> = None;
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
    // Cumulative video track duration in MP4 timescale ticks (sum of each written sample duration).
    let mut muxed_video_duration_ts: u64 = 0;
    // AAC timeline in PCM samples (per channel), i.e. sum of `samples_per_access_unit` muxed.
    let mut muxed_audio_samples: u64 = 0;
    // Pad this many all-zero PCM frames at the start of the next chunk(s) to slow audio vs video.
    let mut pending_silence_pcm_frames: u64 = 0;
    // If NVENC fails with a device/API mismatch, swap to OpenH264 once (do not match generic
    // `encode_picture` errors — e.g. NeedMoreInput was wrongly swapping to CPU encode).
    let mut nvenc_swapped_to_openh264 = false;

    info!(
        "Capturing at {} fps, limit={} (0 = unlimited), system-audio={}",
        params.fps, params.frame_limit, params.capture_system_audio
    );

    while !stop.load(Ordering::SeqCst)
        && (params.frame_limit == 0 || saved < params.frame_limit)
    {
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

                    let enc_cfg_boot = encoder::EncoderConfig::new(
                        size.width,
                        size.height,
                        params.fps,
                        params.video_bitrate_bps,
                    );
                    video_enc = Some(encoder::create_windows_encoder(Some(&device), &enc_cfg_boot)?);

                    if video_enc
                        .as_ref()
                        .unwrap()
                        .supports_bgra_gpu_encode()
                        && params.async_nvenc
                    {
                        let enc = video_enc.take().unwrap();
                        nvenc_async = Some(
                            crate::encode_async::NvencAsync::new(
                                &device,
                                size.width,
                                size.height,
                                enc,
                                4,
                            )
                            .context("NVENC async worker")?,
                        );
                        info!(
                            "NVENC async encode worker (bounded queue=4); RS_CAPTURE_ASYNC_ENCODE=0 for sync capture + OpenH264 fallback"
                        );
                    } else if video_enc
                        .as_ref()
                        .is_some_and(|e| e.supports_bgra_gpu_encode())
                        && !params.async_nvenc
                    {
                        info!(
                            "RS_CAPTURE_ASYNC_ENCODE=0: NVENC runs on capture thread (runtime OpenH264 fallback enabled)"
                        );
                    }

                    if params.outputs.writes_video_files() {
                        let dir = params.outputs.directory().expect("writes_video_files implies dir");
                        let h264_path = dir.join("clip.h264");
                        h264_out = Some(std::fs::File::create(&h264_path).with_context(|| {
                            format!("create {}", h264_path.display())
                        })?);
                        let mp4_path = dir.join("clip.mp4");
                        mp4_out = Some(
                            output::Mp4H264File::create(
                                &mp4_path,
                                size.width as u16,
                                size.height as u16,
                                params.fps,
                            )
                            .with_context(|| format!("create {}", mp4_path.display()))?,
                        );
                        info!(
                            "GPU NV12 pool at {}x{} → {} (Annex-B) + {} (MP4 avc1); encoder at startup (NVENC uses GPU BGRA when available)",
                            size.width,
                            size.height,
                            h264_path.display(),
                            mp4_path.display()
                        );
                    } else {
                        info!(
                            "GPU NV12 pool at {}x{}; stream output (no clip.h264 / clip.mp4 on disk)",
                            size.width, size.height
                        );
                    }
                }

                let (pool, size) = pool_and_size.as_ref().unwrap();
                let enc_cfg = encoder::EncoderConfig::new(
                    size.width,
                    size.height,
                    params.fps,
                    params.video_bitrate_bps,
                );

                let gpu_bgra = nvenc_async.is_some()
                    || video_enc
                        .as_ref()
                        .is_some_and(|e| e.supports_bgra_gpu_encode());

                let mut targets_opt = None;
                if gpu_bgra {
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
                if nvenc_async.is_none() {
                    unsafe {
                        context.Flush();
                    }
                }

                let wall_now = Instant::now();
                let first_wall_anchor = video_t0.is_none();
                let t0 = video_t0.get_or_insert(wall_now);

                // Start loopback **after** the first wall-clock anchor so PTS 0 matches frame 0,
                // and so we do not accumulate a second of preroll in the WASAPI ring buffer.
                let want_encoded_audio =
                    mp4_out.is_some() || params.outputs.stream_senders().is_some();
                if first_wall_anchor && params.capture_system_audio && want_encoded_audio && audio_rx.is_none()
                {
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
                            audio_wav = if let Some(dir) = params.outputs.directory() {
                                let wav_path = dir.join("audio.wav");
                                info!(
                                    "Writing system audio to WAV: {}",
                                    wav_path.display()
                                );
                                Some(
                                    audio::WavFileWriter::create(&wav_path, sample_rate, channels)
                                        .with_context(|| {
                                            format!("create {}", wav_path.display())
                                        })?,
                                )
                            } else {
                                info!("Stream-only: WAV debug file skipped");
                                None
                            };
                            let aac_br = 128_000u32;
                            match MfAacLcEncoder::new(sample_rate, channels, aac_br) {
                                Ok(enc) => {
                                    if let Some(mp4) = mp4_out.as_mut() {
                                        mp4
                                            .enable_aac(sample_rate, channels, aac_br)
                                            .with_context(|| "Mp4H264File::enable_aac")?;
                                    }
                                    aac_enc = Some(enc);
                                    if mp4_out.is_some() {
                                        info!("In-process AAC (MF AAC-LC) muxed into clip.mp4");
                                    } else {
                                        info!("In-process AAC (MF AAC-LC) for stream output");
                                    }
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

                let allow_nvenc_runtime_fallback = !params.force_software_encoder_only;

                let sync_nvenc = video_enc
                    .as_ref()
                    .is_some_and(|e| e.supports_bgra_gpu_encode());

                let pkt = if let Some(na) = nvenc_async.as_mut() {
                    let bgra = converter
                        .bgra_copy_texture()
                        .context("internal BGRA copy for NVENC async")?;
                    let slot = na.slot_next;
                    crate::encode_async::copy_bgra_to_ping(
                        &context,
                        bgra,
                        &na.pings[slot as usize],
                    )?;
                    na.job_tx
                        .send(crate::encode_async::VideoEncodeJob { slot, ts_us })
                        .context("NVENC async job channel closed")?;
                    na.slot_next ^= 1;
                    let pkt_enc = na
                        .pkt_rx
                        .recv()
                        .context("NVENC async output channel closed")?;
                    pkt_enc.context("nvenc async encode")?
                } else if sync_nvenc {
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
                let v_ts = mp4_out
                    .as_ref()
                    .map(|m| m.video_timescale())
                    .unwrap_or(30_000u32);
                let nominal = std::cmp::max(1u32, v_ts / params.fps);
                let max_dur = nominal.saturating_mul(10);
                let gap_ts = if saved == 0 {
                    nominal
                } else {
                    duration_ts_from_delta_us(ts_us.saturating_sub(last_ts_us), v_ts, max_dur)
                };
                last_ts_us = ts_us;

                let (slot_count, dur_ts) = if params.cfr_mux {
                    let max_slots = params.fps.saturating_mul(600).max(1);
                    let slots_u64 =
                        (u64::from(gap_ts) + u64::from(nominal) - 1) / u64::from(nominal);
                    let slots = u32::try_from(slots_u64.max(1))
                        .unwrap_or(u32::MAX)
                        .min(max_slots);
                    (slots, nominal)
                } else {
                    (1u32, gap_ts)
                };

                for slot_idx in 0..slot_count {
                    let key = pkt.is_keyframe && slot_idx == 0;
                    if let Some(h264) = h264_out.as_mut() {
                        h264
                            .write_all(&pkt.data)
                            .context("write clip.h264")?;
                    }
                    if let Some(mp4) = mp4_out.as_mut() {
                        mp4
                            .write_annex_b_frame_with_duration(&pkt.data, key, dur_ts)
                            .context("write clip.mp4")?;
                    }
                    if let Some((vtx, _)) = params.outputs.stream_senders() {
                        let vp = VideoPacket {
                            annex_b: pkt.data.clone(),
                            timestamp_us: pkt.timestamp_us,
                            is_keyframe: key,
                        };
                        if vtx.send(vp).is_ok() {
                            stream_video_packets_sent += 1;
                        }
                    }
                    muxed_video_duration_ts =
                        muxed_video_duration_ts.saturating_add(u64::from(dur_ts));
                }

                if let Some(rx) = audio_rx.as_ref() {
                    if !audio_frame_skip_bootstrapped && audio_sample_rate > 0 {
                        pending_audio_frame_skip = ts_us_to_pcm_frames(ts_us, audio_sample_rate);
                        audio_frame_skip_bootstrapped = true;
                    }
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            AudioMsg::Chunk(mut chunk) => {
                                let trimmed = trim_interleaved_f32_frames_front(
                                    &mut chunk.samples_f32,
                                    audio_pcm_channels,
                                    &mut pending_audio_frame_skip,
                                );
                                let prepended = prepend_silence_pcm_frames_front(
                                    &mut chunk.samples_f32,
                                    audio_pcm_channels,
                                    &mut pending_silence_pcm_frames,
                                );
                                smooth_pcm_chunk_edges(
                                    &mut chunk.samples_f32,
                                    audio_pcm_channels,
                                    prepended,
                                    trimmed,
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
                                        let au_start = muxed_audio_samples;
                                        muxed_audio_samples =
                                            muxed_audio_samples.saturating_add(1024);
                                        let ts_audio_us = (au_start as u128
                                            * 1_000_000
                                            / u128::from(audio_sample_rate))
                                            as u64;
                                        if let Some(mp4) = mp4_out.as_mut() {
                                            mp4
                                                .write_aac_access_unit(&au, 1024)
                                                .context("write AAC to MP4")?;
                                        }
                                        if let Some((_, atx)) =
                                            params.outputs.stream_senders()
                                        {
                                            let chunk_a = AudioChunk::AacRaw {
                                                sample_rate: audio_sample_rate,
                                                channels: audio_pcm_channels,
                                                timestamp_us: ts_audio_us,
                                                payload: au.clone(),
                                            };
                                            if atx.send(chunk_a).is_ok() {
                                                stream_audio_chunks_sent += 1;
                                            }
                                        }
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

                if params.av_drift_threshold_pcm_frames > 0
                    && audio_sample_rate > 0
                    && aac_enc.is_some()
                {
                    reconcile_mux_av_drift(
                        muxed_video_duration_ts,
                        muxed_audio_samples,
                        v_ts,
                        audio_sample_rate,
                        params.av_drift_threshold_pcm_frames,
                        &mut pending_silence_pcm_frames,
                        &mut pending_audio_frame_skip,
                    );
                }

                if let Some(targets) = targets_opt {
                    pool.release(targets);
                }

                saved += 1;

                // Pace wall-clock spacing toward nominal FPS so we don't burst-process compositor
                // frames faster than ~fps/sec (smoother sample spacing vs pure VFR). Cannot invent
                // GPU frames; heavy scenes still drop below target FPS.
                if params.frame_pacing && params.fps > 0 {
                    if let Some(anchor) = video_t0 {
                        let target_end =
                            anchor + Duration::from_secs_f64(saved as f64 / params.fps as f64);
                        let now = Instant::now();
                        if let Some(delay) = target_end.checked_duration_since(now) {
                            std::thread::sleep(delay);
                        }
                    }
                }

                if saved > 0 && saved % (params.fps * 10).max(1) == 0 {
                    info!("Recorded {} video frames...", saved);
                }
            }
            Err(e) => {
                debug!(code = ?e.code(), "no frame yet, retrying…");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    if let Some(a) = nvenc_async.take() {
        a.shutdown().context("NVENC async worker shutdown")?;
    }

    stop.store(true, Ordering::SeqCst);
    if let Some(rx) = audio_rx.take() {
        let v_ts_shutdown = mp4_out
            .as_ref()
            .map(|m| m.video_timescale())
            .unwrap_or(30_000u32);
        for msg in rx.try_iter() {
            match msg {
                AudioMsg::Chunk(mut chunk) => {
                    let trimmed = trim_interleaved_f32_frames_front(
                        &mut chunk.samples_f32,
                        audio_pcm_channels,
                        &mut pending_audio_frame_skip,
                    );
                    let prepended = prepend_silence_pcm_frames_front(
                        &mut chunk.samples_f32,
                        audio_pcm_channels,
                        &mut pending_silence_pcm_frames,
                    );
                    smooth_pcm_chunk_edges(
                        &mut chunk.samples_f32,
                        audio_pcm_channels,
                        prepended,
                        trimmed,
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
                            let au_start = muxed_audio_samples;
                            muxed_audio_samples =
                                muxed_audio_samples.saturating_add(1024);
                            let ts_audio_us = (au_start as u128 * 1_000_000
                                / u128::from(audio_sample_rate))
                                as u64;
                            if let Some(mp4) = mp4_out.as_mut() {
                                mp4
                                    .write_aac_access_unit(&au, 1024)
                                    .context("shutdown drain: MP4 AAC")?;
                            }
                            if let Some((_, atx)) = params.outputs.stream_senders() {
                                let chunk_a = AudioChunk::AacRaw {
                                    sample_rate: audio_sample_rate,
                                    channels: audio_pcm_channels,
                                    timestamp_us: ts_audio_us,
                                    payload: au.clone(),
                                };
                                if atx.send(chunk_a).is_ok() {
                                    stream_audio_chunks_sent += 1;
                                }
                            }
                        }
                    }
                }
                AudioMsg::Error(e) => warn!("audio thread (shutdown drain): {e}"),
                AudioMsg::Ready { .. } => {}
            }
        }
        if params.av_drift_threshold_pcm_frames > 0
            && audio_sample_rate > 0
            && aac_enc.is_some()
        {
            reconcile_mux_av_drift(
                muxed_video_duration_ts,
                muxed_audio_samples,
                v_ts_shutdown,
                audio_sample_rate,
                params.av_drift_threshold_pcm_frames,
                &mut pending_silence_pcm_frames,
                &mut pending_audio_frame_skip,
            );
        }
    }
    if let Some(j) = audio_thread.take() {
        let _ = j.join();
    }

    if let Some(enc) = aac_enc.as_mut() {
        for au in enc.flush().context("AAC flush")? {
            let au_start = muxed_audio_samples;
            muxed_audio_samples = muxed_audio_samples.saturating_add(1024);
            let ts_audio_us =
                (au_start as u128 * 1_000_000 / u128::from(audio_sample_rate)) as u64;
            if let Some(mp4) = mp4_out.as_mut() {
                mp4
                    .write_aac_access_unit(&au, 1024)
                    .context("write final AAC samples")?;
            }
            if let Some((_, atx)) = params.outputs.stream_senders() {
                let chunk_a = AudioChunk::AacRaw {
                    sample_rate: audio_sample_rate,
                    channels: audio_pcm_channels,
                    timestamp_us: ts_audio_us,
                    payload: au.clone(),
                };
                if atx.send(chunk_a).is_ok() {
                    stream_audio_chunks_sent += 1;
                }
            }
        }
    }
    if let Some(m) = mp4_out {
        m.finish().context("finalize clip.mp4")?;
    }
    if let Some(w) = audio_wav {
        w.finalize().context("finalize audio.wav")?;
    }

    if params.capture_system_audio && params.remux_with_ffmpeg {
        if let Some(dir) = params.outputs.directory() {
        let video_mp4 = dir.join("clip.mp4");
        let audio_wav = dir.join("audio.wav");
        let muxed_mp4 = dir.join("clip_with_audio.mp4");
        match try_mux_with_ffmpeg(
            video_mp4
                .to_str()
                .context("clip.mp4 path must be valid UTF-8 for ffmpeg")?,
            audio_wav
                .to_str()
                .context("audio.wav path must be valid UTF-8 for ffmpeg")?,
            muxed_mp4
                .to_str()
                .context("output path must be valid UTF-8 for ffmpeg")?,
        ) {
            Ok(true) => info!("RS_CAPTURE_FFMPEG_MUX: wrote {}", muxed_mp4.display()),
            Ok(false) => info!("RS_CAPTURE_FFMPEG_MUX set but ffmpeg not found"),
            Err(e) => info!("RS_CAPTURE_FFMPEG_MUX: ffmpeg failed ({e})"),
        }
        }
    }

    let done_files = match params.outputs.directory() {
        Some(dir) => format!(
            "{}clip.h264 + {}clip.mp4",
            dir.display(),
            dir.display()
        ),
        None => "(no files)".to_string(),
    };
    info!(
        "Done — {}, frames={}, audio_samples={}, stream_video_packets={}, stream_audio_chunks={}",
        done_files,
        saved,
        audio_samples_total,
        stream_video_packets_sent,
        stream_audio_chunks_sent
    );
    Ok(RunStats {
        frames_captured: saved,
        audio_samples_total,
        stream_video_packets_sent,
        stream_audio_chunks_sent,
    })
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

/// Pad interleaved PCM with leading silence (`pending_frames` whole PCM frames across channels).
/// Returns how many **whole** PCM frames were prepended (for edge smoothing).
fn prepend_silence_pcm_frames_front(
    samples: &mut Vec<f32>,
    channels: u16,
    pending_frames: &mut u64,
) -> u64 {
    let ch = channels as usize;
    if *pending_frames == 0 || ch == 0 {
        return 0;
    }
    let take = (*pending_frames as usize).min(48_000);
    let mut prefix = vec![0.0f32; take * ch];
    if samples.is_empty() {
        *samples = prefix;
    } else {
        prefix.append(samples);
        *samples = prefix;
    }
    *pending_frames -= take as u64;
    take as u64
}

/// Linear fade so hard boundaries (silence insert, time-skip trim) do not click or hiss in AAC.
/// ~1 ms at 48 kHz.
const PCM_EDGE_FADE_FRAMES: usize = 48;

fn smooth_pcm_chunk_edges(
    samples: &mut [f32],
    channels: u16,
    prepended_frames: u64,
    trimmed_frames: u64,
) {
    let ch = channels as usize;
    if ch == 0 || samples.is_empty() {
        return;
    }
    if prepended_frames > 0 {
        let ps = (prepended_frames as usize).saturating_mul(ch);
        if ps >= samples.len() {
            return;
        }
        let n_frames = (samples.len() - ps) / ch;
        let fade = PCM_EDGE_FADE_FRAMES.min(n_frames).max(1);
        for fi in 0..fade {
            let g = (fi + 1) as f32 / fade as f32;
            let base = ps + fi * ch;
            for c in 0..ch {
                let i = base + c;
                if i < samples.len() {
                    samples[i] *= g;
                }
            }
        }
    } else if trimmed_frames > 0 {
        let n_frames = samples.len() / ch;
        let fade = PCM_EDGE_FADE_FRAMES.min(n_frames).max(1);
        for fi in 0..fade {
            let g = (fi + 1) as f32 / fade as f32;
            let base = fi * ch;
            for c in 0..ch {
                let i = base + c;
                if i < samples.len() {
                    samples[i] *= g;
                }
            }
        }
    }
}

/// Keeps muxed AAC timeline aligned with muxed video duration (round-trip remainder vs AAC framing).
const DRIFT_CORRECT_MAX_STEP: u64 = 4800;

fn reconcile_mux_av_drift(
    muxed_video_duration_ts: u64,
    muxed_audio_samples: u64,
    v_ts: u32,
    sample_rate: u32,
    threshold: u64,
    pending_silence_pcm_frames: &mut u64,
    pending_audio_frame_skip: &mut u64,
) {
    if threshold == 0 || sample_rate == 0 || v_ts == 0 {
        return;
    }
    let want = (muxed_video_duration_ts as u128 * sample_rate as u128 / v_ts as u128) as u64;
    let delta = want as i128 - muxed_audio_samples as i128;
    // AAC advances mux in 1024-sample steps; comparing to a smooth `want` curve causes tiny
    // oscillating pad/trim ("static") unless we require a margin past one LC frame.
    let margin = threshold.saturating_add(1024) as i128;
    if delta > margin {
        let add = (delta as u64).min(DRIFT_CORRECT_MAX_STEP);
        *pending_silence_pcm_frames = pending_silence_pcm_frames.saturating_add(add);
    } else if delta < -margin {
        let trim = ((-delta) as u64).min(DRIFT_CORRECT_MAX_STEP);
        *pending_audio_frame_skip = pending_audio_frame_skip.saturating_add(trim);
    }
}

fn duration_ts_from_delta_us(delta_us: u64, timescale_hz: u32, max_dur: u32) -> u32 {
    let v =
        (delta_us as u128 * u128::from(timescale_hz) + 500_000) / 1_000_000;
    let capped = v.min(u128::from(max_dur)).max(1);
    u32::try_from(capped).unwrap_or(max_dur)
}

/// Whole PCM frames (one time slot across all channels) to skip so muxed audio aligns with `ts_us`.
fn ts_us_to_pcm_frames(ts_us: u64, sample_rate: u32) -> u64 {
    (ts_us as u128 * u128::from(sample_rate) / 1_000_000) as u64
}

fn trim_interleaved_f32_frames_front(
    samples: &mut Vec<f32>,
    channels: u16,
    skip_frames: &mut u64,
) -> u64 {
    let ch = channels as usize;
    if ch == 0 || samples.is_empty() || *skip_frames == 0 {
        return 0;
    }
    let available_frames = samples.len() / ch;
    let take = (*skip_frames as usize).min(available_frames);
    if take > 0 {
        samples.drain(0..take * ch);
        *skip_frames = skip_frames.saturating_sub(take as u64);
    }
    take as u64
}
