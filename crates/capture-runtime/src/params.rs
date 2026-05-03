//! Resolved recording parameters (CLI/env → [`PipelineParams`]).

use std::path::PathBuf;

/// Fully resolved options for a Windows file-recording session.
#[derive(Clone, Debug)]
pub struct PipelineParams {
    pub output_directory: PathBuf,
    /// `0` = unlimited frames.
    pub frame_limit: u32,
    pub capture_system_audio: bool,
    pub fps: u32,
    pub video_bitrate_bps: u32,
    pub frame_pacing: bool,
    pub async_nvenc: bool,
    pub cfr_mux: bool,
    pub av_drift_threshold_pcm_frames: u64,
    /// When true, behave like `RS_CAPTURE_ENCODER=openh264` (skip NVENC selection).
    pub force_software_encoder_only: bool,
    pub remux_with_ffmpeg: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RunStats {
    pub frames_captured: u32,
    pub audio_samples_total: u64,
}
