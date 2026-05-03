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

/// How the runner pushes into [`OutputTarget::StreamOnly`] / [`OutputTarget::FilesAndStream`] channels.
///
/// Use with **bounded** channels from [`stream_pair`]: [`Self::Block`] matches [`crossbeam_channel::Sender::send`]
/// (wait for capacity); [`Self::DropWhenFull`] matches [`crossbeam_channel::Sender::try_send`] and drops the
/// outgoing packet if the queue is full (keeps capture from stalling when the consumer is slow).
///
/// With **unbounded** channels, [`Self::Block`] never waits for capacity and [`Self::DropWhenFull`] never sees a full queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde_config", derive(Serialize, Deserialize))]
pub enum StreamBackpressure {
    /// Block the capture thread until the peer receives ([`Sender::send`]).
    #[default]
    Block,
    /// Drop this packet/chunk when the bounded queue is full; see [`crate::params::RunStats`].
    DropWhenFull,
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
    /// Used when [`OutputTarget`] includes stream senders; ignored for file-only output.
    pub stream_backpressure: StreamBackpressure,
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
            stream_backpressure: StreamBackpressure::default(),
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
            stream_backpressure: StreamBackpressure::default(),
            audio_codec: AudioCodecChoice::AacLcMf,
            limit_frames: None,
            capture_system_audio: true,
        }
    }

    /// Disk + stream (same defaults as [`Self::files_only`]) for relay/preview while recording.
    pub fn files_and_stream(
        directory: impl Into<PathBuf>,
        video: Sender<VideoPacket>,
        audio: Sender<AudioChunk>,
    ) -> Self {
        Self {
            output: OutputTarget::FilesAndStream {
                directory: directory.into(),
                video,
                audio,
            },
            fps: 30,
            video_bitrate_bps: 45_000_000,
            frame_pacing: true,
            async_nvenc: true,
            cfr_mux: false,
            av_drift_threshold_pcm_frames: 0,
            video_preference: VideoCodecPreference::Auto,
            stream_backpressure: StreamBackpressure::default(),
            audio_codec: AudioCodecChoice::AacLcMf,
            limit_frames: None,
            capture_system_audio: true,
        }
    }
}

/// Create bounded channels for streaming; returns `(video_tx, audio_tx, video_rx, audio_rx)`.
///
/// Capacity applies per queue; combine with [`SessionConfig::stream_backpressure`] / [`crate::params::PipelineParams::stream_backpressure`]:
/// [`StreamBackpressure::Block`] waits when full; [`StreamBackpressure::DropWhenFull`] drops with bounded queues.
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
