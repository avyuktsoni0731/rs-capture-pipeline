//! Per-process CPU/RAM sampling for benchmark videos (`metrics.csv` next to clip outputs).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Background thread: append one CSV row per second until `stop` is set.
pub fn spawn_metrics_csv_logger(
    out_dir: PathBuf,
    stop: Arc<AtomicBool>,
    frames_captured: Arc<AtomicU32>,
) -> anyhow::Result<JoinHandle<()>> {
    let path = out_dir.join("metrics.csv");
    let pid = std::process::id();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "capture-pipeline".to_string());

    let mut file = BufWriter::new(
        File::create(&path).map_err(|e| anyhow::anyhow!("create {}: {e}", path.display()))?,
    );
    writeln!(
        file,
        "elapsed_s,frames,video_fps,cpu_percent,memory_mb,pid,process"
    )?;
    file.flush()?;

    let handle = thread::Builder::new()
        .name("rs-capture-metrics".into())
        .spawn(move || {
            let mut system = System::new();
            let session_start = Instant::now();
            let mut last_frames = 0u32;
            let mut last_sample = Instant::now();

            while !stop.load(Ordering::Relaxed) {
                thread::sleep(SAMPLE_INTERVAL);

                let now = Instant::now();
                let elapsed_s = session_start.elapsed().as_secs_f64();
                let dt = now.duration_since(last_sample).as_secs_f64().max(0.001);
                last_sample = now;

                let frames = frames_captured.load(Ordering::Relaxed);
                let video_fps = (frames.saturating_sub(last_frames)) as f64 / dt;
                last_frames = frames;

                system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
                let (cpu_percent, memory_mb) = system
                    .process(Pid::from_u32(pid))
                    .map(|p| (p.cpu_usage() as f64, p.memory() as f64 / (1024.0 * 1024.0)))
                    .unwrap_or((0.0, 0.0));

                if writeln!(
                    file,
                    "{elapsed_s:.1},{frames},{video_fps:.2},{cpu_percent:.1},{memory_mb:.1},{pid},{exe}"
                )
                .is_err()
                {
                    break;
                }
                let _ = file.flush();
            }
        })?;

    Ok(handle)
}
