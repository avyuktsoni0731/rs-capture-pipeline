//! Stream-only host: consume [`VideoPacket`] / [`AudioChunk`] and print throughput (no disk, no WebRTC).
//!
//! ```text
//! cargo run --example stream_stats -- [FRAME_LIMIT]
//! ```
//!
//! Use `RS_CAPTURE_AUDIO_CODEC=opus` to receive [`AudioChunk::OpusPacket`] instead of AAC.
//! Your product wires the same receivers to RTP, RTMP, or a WebRTC stack — this example only counts packets.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use capture_runtime::{
    log_pipeline_startup, pipeline_params_stream_only, run_recording, stream_pair, AudioChunk,
    VideoPacket,
};
use tracing::info;

#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const VIDEO_QUEUE_CAP: usize = 64;
const AUDIO_QUEUE_CAP: usize = 128;

struct StreamCounters {
    video_packets: u64,
    video_bytes: u64,
    video_keyframes: u64,
    audio_chunks: u64,
    audio_payload_bytes: u64,
    last_video_ts_us: u64,
    last_audio_ts_us: u64,
}

impl StreamCounters {
    fn on_video(&mut self, p: &VideoPacket) {
        self.video_packets += 1;
        self.video_bytes += p.annex_b.len() as u64;
        if p.is_keyframe {
            self.video_keyframes += 1;
        }
        self.last_video_ts_us = p.timestamp_us;
    }

    fn on_audio(&mut self, c: &AudioChunk) {
        self.audio_chunks += 1;
        self.last_audio_ts_us = match c {
            AudioChunk::PcmF32Interleaved { timestamp_us, .. }
            | AudioChunk::AacRaw { timestamp_us, .. }
            | AudioChunk::OpusPacket { timestamp_us, .. } => *timestamp_us,
        };
        self.audio_payload_bytes += match c {
            AudioChunk::PcmF32Interleaved { samples, .. } => (samples.len() * 4) as u64,
            AudioChunk::AacRaw { payload, .. } | AudioChunk::OpusPacket { payload, .. } => {
                payload.len() as u64
            }
        };
    }
}

fn drain_video(rx: &crossbeam_channel::Receiver<VideoPacket>, counters: &mut StreamCounters) {
    while let Ok(p) = rx.try_recv() {
        counters.on_video(&p);
    }
}

fn drain_audio(rx: &crossbeam_channel::Receiver<AudioChunk>, counters: &mut StreamCounters) {
    while let Ok(c) = rx.try_recv() {
        counters.on_audio(&c);
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    #[cfg(not(windows))]
    {
        anyhow::bail!("stream_stats example requires Windows (capture-runtime runner is Windows-only)");
    }

    #[cfg(windows)]
    {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .context("CoInitializeEx(MTA)")?;
        }

        let frame_limit: u32 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let capture_system_audio = std::env::args().nth(2).as_deref() != Some("noaudio");

        let (video_tx, audio_tx, video_rx, audio_rx) = stream_pair(VIDEO_QUEUE_CAP, AUDIO_QUEUE_CAP);
        let params = pipeline_params_stream_only(video_tx, audio_tx, frame_limit, capture_system_audio);
        log_pipeline_startup(&params);

        let stop = Arc::new(AtomicBool::new(false));
        {
            let stop_flag = Arc::clone(&stop);
            ctrlc::set_handler(move || {
                stop_flag.store(true, Ordering::SeqCst);
            })
            .context("install Ctrl+C handler")?;
        }

        let capture_stop = Arc::clone(&stop);
        let capture_params = params;
        let capture_handle = thread::spawn(move || run_recording(&capture_params, capture_stop));

        info!(
            "stream_stats running (frame_limit={}, audio={}, queues {}/{}). Ctrl+C to stop.",
            frame_limit, capture_system_audio, VIDEO_QUEUE_CAP, AUDIO_QUEUE_CAP
        );

        let mut counters = StreamCounters {
            video_packets: 0,
            video_bytes: 0,
            video_keyframes: 0,
            audio_chunks: 0,
            audio_payload_bytes: 0,
            last_video_ts_us: 0,
            last_audio_ts_us: 0,
        };
        let mut last_report = Instant::now();
        let mut last_video_pkts = 0u64;
        let mut last_audio_chunks = 0u64;
        let session_start = Instant::now();

        while !stop.load(Ordering::SeqCst) && !capture_handle.is_finished() {
            drain_video(&video_rx, &mut counters);
            drain_audio(&audio_rx, &mut counters);

            if last_report.elapsed() >= Duration::from_secs(1) {
                let elapsed = session_start.elapsed().as_secs_f64().max(0.001);
                let v_delta = counters.video_packets - last_video_pkts;
                let a_delta = counters.audio_chunks - last_audio_chunks;
                println!(
                    "[stream_stats] +{v_delta} vid/s +{a_delta} aud/s | total {} vid ({} key), {} aud | last ts vid={}us aud={}us | {:.0}s session",
                    counters.video_packets,
                    counters.video_keyframes,
                    counters.audio_chunks,
                    counters.last_video_ts_us,
                    counters.last_audio_ts_us,
                    elapsed
                );
                last_video_pkts = counters.video_packets;
                last_audio_chunks = counters.audio_chunks;
                last_report = Instant::now();
            }

            thread::sleep(Duration::from_millis(10));
        }

        stop.store(true, Ordering::SeqCst);
        let stats = capture_handle
            .join()
            .map_err(|_| anyhow::anyhow!("capture thread panicked"))??;

        drain_video(&video_rx, &mut counters);
        drain_audio(&audio_rx, &mut counters);

        println!("--- final ---");
        println!(
            "captured {} frames | stream sent {} video / {} audio | dropped full {} vid / {} aud",
            stats.frames_captured,
            stats.stream_video_packets_sent,
            stats.stream_audio_chunks_sent,
            stats.stream_video_packets_dropped_full,
            stats.stream_audio_chunks_dropped_full
        );
        println!(
            "consumer saw {} video packets ({} bytes), {} audio chunks ({} payload bytes)",
            counters.video_packets,
            counters.video_bytes,
            counters.audio_chunks,
            counters.audio_payload_bytes
        );
    }

    Ok(())
}
