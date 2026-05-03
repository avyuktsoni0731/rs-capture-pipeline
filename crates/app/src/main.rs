//! Thin CLI for [`capture_runtime::run_file_recording`] — COM init, priority, Ctrl+C, env-backed [`PipelineParams`].

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use anyhow::Context;
use capture_runtime::{log_pipeline_startup, pipeline_params_from_cli_and_env, run_file_recording};
use tracing::{info, warn};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS,
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
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
    let frame_limit: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let capture_system_audio = std::env::args()
        .nth(3)
        .map(|s| s != "noaudio")
        .unwrap_or(true);

    let params = pipeline_params_from_cli_and_env(out_dir, frame_limit, capture_system_audio);
    log_pipeline_startup(&params);

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop_flag = Arc::clone(&stop);
        ctrlc::set_handler(move || {
            stop_flag.store(true, Ordering::SeqCst);
        })
        .context("install Ctrl+C handler")?;
    }

    let t0 = Instant::now();
    let stats = run_file_recording(&params, stop)?;
    let elapsed = t0.elapsed().as_secs_f64();
    info!(
        "Finished in {:.1}s — {} frames, {} PCM samples",
        elapsed, stats.frames_captured, stats.audio_samples_total
    );
    Ok(())
}
