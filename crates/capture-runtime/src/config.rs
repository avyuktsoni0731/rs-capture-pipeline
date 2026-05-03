//! Session-wide configuration for embeddable runs.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};

#[cfg(feature = "serde_config")]
use serde::{Deserialize, Serialize};

use crate::events::{AudioChunk, VideoPacket};

/// Where encoded media goes. Extend with `Rtmp`, `Webrtc`, etc. as you add sinks.
///
/// Not serialized as a whole when using [`OutputTarget::StreamOnly`] (contains channel senders).
#[derive(Debug)]
pub enum OutputTarget {
    /// Write `clip.h264`, `clip.mp4`, `audio.wav` under this directory (current CLI behavior).
    Files {
        directory: PathBuf,
    },
    /// Push encoded packets to these channels (MyCord WebSocket task owns the receivers).
    StreamOnly {
        #[cfg_attr(feature = "serde_config", serde(skip))]
        video: Sender<VideoPacket>,
        #[cfg_attr(feature = "serde_config", serde(skip))]
        audio: Sender<AudioChunk>,
    },
    /// Record to disk **and** push to channels (local file + preview / relay).
    FilesAndStream {
        directory: PathBuf,
        #[cfg_attr(feature = "serde_config", serde(skip))]
        video: Sender<VideoPacket>,
        #[cfg_attr(feature = "serde_config", serde(skip))]
        audio: Sender<AudioChunk>,
    },
}

/// Hint for encoder selection order once multiple backends exist (`encoder` crate).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde_config", derive(Serialize, Deserialize))]
pub enum VideoCodecPreference {
    #[default]
    Auto,
    PreferNvenc,
    PreferSoftware,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde_config", derive(Serialize, Deserialize))]
pub enum AudioCodecChoice {
    #[default]
    /// AAC-LC via Media Foundation for MP4 (today’s path).
    AacLcMf,
    /// Reserved for WebRTC / low latency (Phase 2).
    Opus,
    PcmOnly,
}

/// Everything needed to start a session (host fills this; runner maps to encoder structs later).
#[derive(Debug)]
pub struct SessionConfig {
    pub output: OutputTarget,
    pub fps: u32,
    pub video_bitrate_bps: u32,
    pub frame_pacing: bool,
    pub async_nvenc: bool,
    pub cfr_mux: bool,
    /// `0` = drift watchdog off (recommended).
    pub av_drift_threshold_pcm_frames: u64,
    pub video_preference: VideoCodecPreference,
    pub audio_codec: AudioCodecChoice,
    pub limit_frames: Option<u32>,
    pub capture_system_audio: bool,
}

impl SessionConfig {
    /// MP4 + WAV under `directory` (CLI default).
    pub fn files_only(directory: impl Into<PathBuf>) -> Self {
        Self {
            output: OutputTarget::Files {
                directory: directory.into(),
            },
            fps: 30,
            video_bitrate_bps: 45_000_000,
            frame_pacing: true,
            async_nvenc: true,
            cfr_mux: false,
            av_drift_threshold_pcm_frames: 0,
            video_preference: VideoCodecPreference::Auto,
            audio_codec: AudioCodecChoice::AacLcMf,
            limit_frames: None,
            capture_system_audio: true,
        }
    }

    /// MyCord-style: host keeps [`Receiver`]s and forwards packets (see [`crate::stream_pair`]).
    pub fn with_stream_endpoints(video: Sender<VideoPacket>, audio: Sender<AudioChunk>) -> Self {
        Self {
            output: OutputTarget::StreamOnly { video, audio },
            fps: 30,
            video_bitrate_bps: 45_000_000,
            frame_pacing: true,
            async_nvenc: true,
            cfr_mux: false,
            av_drift_threshold_pcm_frames: 0,
            video_preference: VideoCodecPreference::Auto,
            audio_codec: AudioCodecChoice::AacLcMf,
            limit_frames: None,
            capture_system_audio: true,
        }
    }
}

/// Create bounded channels for streaming; returns `(config_fragment_senders, video_rx, audio_rx)`.
///
/// Build [`SessionConfig`] with [`SessionConfig::with_stream_endpoints`] using the senders, or
/// attach senders to [`OutputTarget::FilesAndStream`].
pub fn stream_pair(
    video_cap: usize,
    audio_cap: usize,
) -> (
    Sender<VideoPacket>,
    Sender<AudioChunk>,
    Receiver<VideoPacket>,
    Receiver<AudioChunk>,
) {
    let (vtx, vrx) = crossbeam_channel::bounded(video_cap);
    let (atx, arx) = crossbeam_channel::bounded(audio_cap);
    (vtx, atx, vrx, arx)
}
