//! Embeddable capture **runtime API** for hosts such as `capture-pipeline` CLI, MyCord, or tray apps.
//!
//! ## Library recording entrypoint (Windows)
//!
//! ```ignore
//! let params = capture_runtime::pipeline_params_from_cli_and_env("capture_out", 0, true);
//! let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
//! let stats = capture_runtime::run_file_recording(&params, stop)?;
//! ```
//!
//! ## Cargo.toml in another repo (path dependency)
//!
//! ```toml
//! [dependencies]
//! capture-runtime = { path = "../rs-capture-pipeline/crates/capture-runtime", features = ["serde_config"] }
//! ```

pub mod config;
pub mod env;
pub mod error;
pub mod events;
pub mod params;

#[cfg(windows)]
pub mod encode_async;

#[cfg(windows)]
mod run_win;

pub use config::{
    stream_pair, AudioCodecChoice, OutputTarget, SessionConfig, VideoCodecPreference,
};
pub use env::{log_pipeline_startup, pipeline_params_from_cli_and_env};
pub use error::RuntimeError;
pub use events::{AudioChunk, StreamClock, VideoPacket};
pub use params::{PipelineParams, RunStats};

/// Run a full **file** recording session (WGC → encode → `clip.mp4` / `clip.h264` / `audio.wav`).
///
/// **Windows only.** Call after COM init on the main thread if you use audio/MF (see CLI binary).
#[cfg(windows)]
pub fn run_file_recording(
    params: &PipelineParams,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    run_win::run_file_recording(params, stop)
}

#[cfg(not(windows))]
pub fn run_file_recording(
    _params: &PipelineParams,
    _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    anyhow::bail!("capture-runtime file recording is only implemented on Windows")
}
