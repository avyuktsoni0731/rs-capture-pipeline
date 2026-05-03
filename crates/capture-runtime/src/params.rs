//! Resolved recording parameters (CLI/env → [`PipelineParams`]).
//!
//! Use [`PipelineParams::try_from_session_config`] to convert a declarative [`crate::SessionConfig`].

use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;

use crate::config::{AudioCodecChoice, OutputTarget, SessionConfig, VideoCodecPreference};
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

impl PipelineParams {
    /// Build runner params from a [`SessionConfig`] (e.g. serialized host settings).
    ///
    /// - [`VideoCodecPreference::PreferSoftware`] sets [`PipelineParams::force_software_encoder_only`].
    /// - [`AudioCodecChoice::AacLcMf`] is required today; other audio modes return an error until implemented.
    pub fn try_from_session_config(
        session: SessionConfig,
        remux_with_ffmpeg: bool,
    ) -> anyhow::Result<Self> {
        match session.audio_codec {
            AudioCodecChoice::AacLcMf => {}
            AudioCodecChoice::Opus => {
                anyhow::bail!("SessionConfig.audio_codec Opus is not wired to the recording pipeline yet")
            }
            AudioCodecChoice::PcmOnly => {
                anyhow::bail!("SessionConfig.audio_codec PcmOnly is not supported for MP4/AAC recording")
            }
        }

        let outputs = match session.output {
            OutputTarget::Files { directory } => RecordingOutputs::Files { directory },
            OutputTarget::StreamOnly { video, audio } => RecordingOutputs::Stream {
                video_tx: video,
                audio_tx: audio,
            },
            OutputTarget::FilesAndStream {
                directory,
                video,
                audio,
            } => RecordingOutputs::FilesAndStream {
                directory,
                video_tx: video,
                audio_tx: audio,
            },
        };

        let force_software_encoder_only =
            matches!(session.video_preference, VideoCodecPreference::PreferSoftware);

        Ok(Self {
            outputs,
            frame_limit: session.limit_frames.unwrap_or(0),
            capture_system_audio: session.capture_system_audio,
            fps: session.fps,
            video_bitrate_bps: session.video_bitrate_bps,
            frame_pacing: session.frame_pacing,
            async_nvenc: session.async_nvenc,
            cfr_mux: session.cfr_mux,
            av_drift_threshold_pcm_frames: session.av_drift_threshold_pcm_frames,
            force_software_encoder_only,
            remux_with_ffmpeg,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SessionConfig;

    #[test]
    fn session_files_maps_to_pipeline() {
        let mut s = SessionConfig::files_only("out");
        s.fps = 60;
        s.video_bitrate_bps = 10_000_000;
        s.frame_pacing = false;
        s.async_nvenc = false;
        s.cfr_mux = true;
        s.limit_frames = Some(100);
        s.capture_system_audio = false;
        let p = PipelineParams::try_from_session_config(s, false).expect("ok");
        assert!(p.outputs.directory().is_some());
        assert_eq!(p.fps, 60);
        assert_eq!(p.frame_limit, 100);
        assert!(!p.force_software_encoder_only);
    }

    #[test]
    fn prefer_software_sets_flag() {
        let mut s = SessionConfig::files_only("x");
        s.video_preference = crate::config::VideoCodecPreference::PreferSoftware;
        let p = PipelineParams::try_from_session_config(s, false).expect("ok");
        assert!(p.force_software_encoder_only);
    }
}
