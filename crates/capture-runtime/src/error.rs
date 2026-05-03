//! Errors for the public runtime API (orchestration only — encoder/capture errors stay as `anyhow` inside the runner).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("capture session already running")]
    AlreadyRunning,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("runtime not implemented: use capture-pipeline-app binary until runner is extracted")]
    NotImplemented,
}
