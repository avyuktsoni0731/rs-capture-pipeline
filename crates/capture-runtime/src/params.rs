//! Resolved recording parameters (CLI/env → [`PipelineParams`]).

use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;

use crate::events::{AudioChunk, VideoPacket};

/// Where muxed artifacts go: disk, bounded channels (MyCord / preview), or both.
#[derive(Debug)]
pub enum RecordingOutputs {
    /// `clip.h264`, `clip.mp4`, `audio.wav` under `directory`.
    Files { directory: PathBuf },
    /// Encoded samples only (no `clip.*` on disk). Host reads [`crate::events`] types from receivers.
    Stream {
        video_tx: Sender<VideoPacket>,
        audio_tx: Sender<AudioChunk>,
    },
    /// File outputs plus duplicate stream for localhost relay / dual recording.
    FilesAndStream {
        directory: PathBuf,
        video_tx: Sender<VideoPacket>,
        audio_tx: Sender<AudioChunk>,
    },
}

impl RecordingOutputs {
    pub fn directory(&self) -> Option<&Path> {
        match self {
            RecordingOutputs::Files { directory } => Some(directory.as_path()),
            RecordingOutputs::Stream { .. } => None,
            RecordingOutputs::FilesAndStream { directory, .. } => Some(directory.as_path()),
        }
    }

    pub fn stream_senders(&self) -> Option<(&Sender<VideoPacket>, &Sender<AudioChunk>)> {
        match self {
            RecordingOutputs::Files { .. } => None,
            RecordingOutputs::Stream {
                video_tx,
                audio_tx,
            } => Some((video_tx, audio_tx)),
            RecordingOutputs::FilesAndStream {
                video_tx,
                audio_tx,
                ..
            } => Some((video_tx, audio_tx)),
        }
    }

    pub fn writes_video_files(&self) -> bool {
        matches!(
            self,
            RecordingOutputs::Files { .. } | RecordingOutputs::FilesAndStream { .. }
        )
    }
}

/// Fully resolved options for a Windows recording session.
#[derive(Debug)]
pub struct PipelineParams {
    pub outputs: RecordingOutputs,
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

#[derive(Clone, Copy, Debug)]
pub struct RunStats {
    pub frames_captured: u32,
    pub audio_samples_total: u64,
    pub stream_video_packets_sent: u64,
    pub stream_audio_chunks_sent: u64,
}
