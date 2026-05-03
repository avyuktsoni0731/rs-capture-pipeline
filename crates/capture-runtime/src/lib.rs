//! Embeddable capture **runtime API** for hosts such as `capture-pipeline` CLI, MyCord, or tray apps.
//!
//! ## Integration model
//!
//! - **Today:** The heavy implementation still lives in the `capture-pipeline-app` binary (`main.rs`).
//!   This crate defines **stable types and contracts** other projects depend on. The next step is to
//!   move the loop into a `runner` module (same repo) and call it from both the CLI and your app.
//!
//! - **MyCord path:** Call [`stream_pair`], build [`SessionConfig::with_stream_endpoints`], then run
//!   the session (once wired). Encoded **Annex-B H.264** ([`VideoPacket`]) and audio ([`AudioChunk`])
//!   are sent on bounded channels; your WebSocket task reads from the receivers.
//!
//! - **Extra video backends** (AMF, QSV, x264, …): stay behind the existing `encoder::VideoEncoder`
//!   trait in the `encoder` crate; the runner selects backends from [`VideoCodecPreference`].
//!
//! ## Cargo.toml in another repo (path dependency)
//!
//! ```toml
//! [dependencies]
//! capture-runtime = { path = "../rs-capture-pipeline/crates/capture-runtime", features = ["serde_config"] }
//! ```

pub mod config;
pub mod error;
pub mod events;

pub use config::{
    stream_pair, AudioCodecChoice, OutputTarget, SessionConfig, VideoCodecPreference,
};
pub use error::RuntimeError;
pub use events::{AudioChunk, StreamClock, VideoPacket};
