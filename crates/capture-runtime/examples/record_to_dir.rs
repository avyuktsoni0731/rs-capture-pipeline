//! Minimal embeddable host: write `clip.h264`, `clip.mp4`, and `audio.wav` under a directory.
//!
//! ```text
//! cargo run --example record_to_dir -- [OUT_DIR] [FRAME_LIMIT]
//! ```
//!
//! - `FRAME_LIMIT`: `0` = until Ctrl+C (default).
//! - Third arg `noaudio` disables system audio.
//!
//! See [`docs/INTEGRATION.md`](../../../docs/INTEGRATION.md) for embedding in your own binary.

use std::fs;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use anyhow::Context;
use capture_runtime::{log_pipeline_startup, pipeline_params_from_cli_and_env, run_recording};
use tracing::info;

#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    #[cfg(not(windows))]
    {
        anyhow::bail!("record_to_dir example requires Windows (capture-runtime runner is Windows-only)");
    }

    #[cfg(windows)]
    {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .context("CoInitializeEx(MTA)")?;
        }

        let out_dir = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "example_capture_out".to_string());
        let frame_limit: u32 = std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let capture_system_audio = std::env::args().nth(3).as_deref() != Some("noaudio");

        fs::create_dir_all(&out_dir).with_context(|| format!("create_dir_all {out_dir}"))?;

        let params = pipeline_params_from_cli_and_env(&out_dir, frame_limit, capture_system_audio);
        log_pipeline_startup(&params);

        let stop = Arc::new(AtomicBool::new(false));
        {
            let stop_flag = Arc::clone(&stop);
            ctrlc::set_handler(move || {
                stop_flag.store(true, Ordering::SeqCst);
            })
            .context("install Ctrl+C handler")?;
        }

        info!(
            "Recording to {} (frame_limit={}, audio={}) — Ctrl+C to stop",
            out_dir, frame_limit, capture_system_audio
        );
        let t0 = Instant::now();
        let stats = run_recording(&params, stop)?;
        info!(
            "Done in {:.1}s — {} frames, {} PCM samples written",
            t0.elapsed().as_secs_f64(),
            stats.frames_captured,
            stats.audio_samples_total
        );
        info!("Outputs: {out_dir}/clip.h264, clip.mp4, audio.wav");
    }

    Ok(())
}
