//! Embeddable capture **runtime API** for hosts such as `capture-pipeline` CLI, MyCord, or tray apps.
//!
//! ## Library recording entrypoint (Windows)
//!
//! ```ignore
//! let params = capture_runtime::pipeline_params_from_cli_and_env("capture_out", 0, true);
//! let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
//! let stats = capture_runtime::run_recording(&params, stop)?;
//! ```
//!
//! Or build from a [`SessionConfig`] (e.g. from JSON with `serde_config`):
//! `PipelineParams::try_from_session_config(session, /* remux */ false)?`.
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
mod process_metrics;

#[cfg(windows)]
mod run_win;

pub use config::{
    stream_pair, AudioCodecChoice, OutputTarget, SessionConfig, StreamBackpressure,
    VideoCodecPreference,
};
pub use env::{
    log_pipeline_startup, pipeline_params_files_and_stream, pipeline_params_from_cli_and_env,
    pipeline_params_stream_only,
};
pub use error::RuntimeError;
pub use events::{AudioChunk, StreamClock, VideoPacket};
pub use params::{PipelineParams, RecordingOutputs, RunStats};

/// Run a recording session (WGC → encode → optional disk + optional [`RecordingOutputs::Stream`]).
///
/// **Windows only.** Call after COM init on the main thread if you use audio/MF (see CLI binary).
#[cfg(windows)]
pub fn run_file_recording(
    params: &PipelineParams,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    run_win::run_file_recording(params, stop)
}

/// Alias for [`run_file_recording`] (records to files and/or stream sinks).
#[cfg(windows)]
#[inline]
pub fn run_recording(
    params: &PipelineParams,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    run_file_recording(params, stop)
}

#[cfg(not(windows))]
pub fn run_file_recording(
    _params: &PipelineParams,
    _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    anyhow::bail!("capture-runtime file recording is only implemented on Windows")
}

#[cfg(not(windows))]
#[inline]
pub fn run_recording(
    params: &PipelineParams,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<RunStats> {
    run_file_recording(params, stop)
}
