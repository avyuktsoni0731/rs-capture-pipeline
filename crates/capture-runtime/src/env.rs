//! Environment-driven defaults mapped into [`PipelineParams`].

use std::path::PathBuf;

use tracing::{info, warn};

use crate::config::{StreamBackpressure, VideoCodecPreference};
use crate::params::{PipelineParams, RecordingOutputs};

/// Build [`PipelineParams`] from CLI arguments plus `RS_CAPTURE_*` env overrides (file output only).
pub fn pipeline_params_from_cli_and_env(
    output_directory: impl Into<PathBuf>,
    frame_limit: u32,
    capture_system_audio: bool,
) -> PipelineParams {
    build_pipeline_params(
        RecordingOutputs::Files {
            directory: output_directory.into(),
        },
        frame_limit,
        capture_system_audio,
    )
}

fn build_pipeline_params(
    outputs: RecordingOutputs,
    frame_limit: u32,
    capture_system_audio: bool,
) -> PipelineParams {
    let fps = capture_fps_from_env();
    let video_bitrate_bps = video_bitrate_bps_from_env();
    let frame_pacing = frame_pacing_from_env();
    let async_nvenc = async_nvenc_encode_from_env();
    let cfr_mux = cfr_mux_from_env();
    let av_drift_threshold_pcm_frames = av_drift_threshold_frames_from_env();
    let video_codec_preference = video_codec_preference_from_env();
    let stream_backpressure = stream_backpressure_from_env();
    let remux_with_ffmpeg = std::env::var("RS_CAPTURE_FFMPEG_MUX").ok().as_deref() == Some("1");

    PipelineParams {
        outputs,
        frame_limit,
        capture_system_audio,
        fps,
        video_bitrate_bps,
        frame_pacing,
        async_nvenc,
        cfr_mux,
        av_drift_threshold_pcm_frames,
        video_codec_preference,
        stream_backpressure,
        remux_with_ffmpeg,
    }
}

/// Stream-only session (e.g. MyCord host created channels via [`crate::stream_pair`]).
pub fn pipeline_params_stream_only(
    video_tx: crossbeam_channel::Sender<crate::events::VideoPacket>,
    audio_tx: crossbeam_channel::Sender<crate::events::AudioChunk>,
    frame_limit: u32,
    capture_system_audio: bool,
) -> PipelineParams {
    build_pipeline_params(
        RecordingOutputs::Stream {
            video_tx,
            audio_tx,
        },
        frame_limit,
        capture_system_audio,
    )
}

/// Record to disk and duplicate encoded packets to `video_tx` / `audio_tx`.
pub fn pipeline_params_files_and_stream(
    directory: impl Into<PathBuf>,
    video_tx: crossbeam_channel::Sender<crate::events::VideoPacket>,
    audio_tx: crossbeam_channel::Sender<crate::events::AudioChunk>,
    frame_limit: u32,
    capture_system_audio: bool,
) -> PipelineParams {
    build_pipeline_params(
        RecordingOutputs::FilesAndStream {
            directory: directory.into(),
            video_tx,
            audio_tx,
        },
        frame_limit,
        capture_system_audio,
    )
}

/// Log effective capture settings (call from the binary after building params).
pub fn log_pipeline_startup(p: &PipelineParams) {
    match &p.outputs {
        RecordingOutputs::Files { directory } => {
            info!("Output: files under {}", directory.display());
        }
        RecordingOutputs::Stream { .. } => {
            info!("Output: stream-only (no clip.h264 / clip.mp4 / audio.wav on disk)");
        }
        RecordingOutputs::FilesAndStream { directory, .. } => {
            info!(
                "Output: files under {} + duplicate packet stream",
                directory.display()
            );
        }
    }
    if p.outputs.stream_senders().is_some() {
        match p.stream_backpressure {
            StreamBackpressure::Block => {
                info!("Stream backpressure: block (wait for consumer; use bounded stream_pair for backpressure)");
            }
            StreamBackpressure::DropWhenFull => {
                info!("Stream backpressure: drop when full (try_send; counts *_dropped_full in RunStats)");
            }
        }
    }
    if p.fps != 30 {
        info!(
            "RS_CAPTURE_FPS={}: encoder and MP4 use this nominal rate (default 30)",
            p.fps
        );
    }
    info!(
        "Video bitrate {} bps ({})",
        p.video_bitrate_bps,
        if std::env::var("RS_CAPTURE_VIDEO_BITRATE").is_ok() {
            "from RS_CAPTURE_VIDEO_BITRATE"
        } else {
            "default ~OBS-class 45 Mbps; set RS_CAPTURE_VIDEO_BITRATE to override"
        }
    );
    if !p.frame_pacing {
        info!("RS_CAPTURE_FRAME_PACING=0: no sleep between frames (max capture rate, rougher VFR)");
    } else {
        info!(
            "Frame pacing on (RS_CAPTURE_FRAME_PACING=0 to disable) — targets ~{} pulls/sec wall clock",
            p.fps
        );
    }
    if p.cfr_mux {
        info!("RS_CAPTURE_CFR=1: MP4 video uses fixed sample duration with duplicate frames for gaps");
    }
    if p.av_drift_threshold_pcm_frames == 0 {
        info!("A/V drift watchdog off (default; audio timeline follows video ts_us). Set RS_CAPTURE_AV_DRIFT_SAMPLES to enable trim/pad");
    } else {
        info!(
            "A/V drift watchdog: trim/pad when drift exceeds threshold (~{} + one AAC frame margin); may affect sound quality",
            p.av_drift_threshold_pcm_frames
        );
    }
}

fn stream_backpressure_from_env() -> StreamBackpressure {
    let Ok(raw) = std::env::var("RS_CAPTURE_STREAM_BACKPRESSURE") else {
        return StreamBackpressure::Block;
    };
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "block" => StreamBackpressure::Block,
        "drop" | "drop_when_full" | "try" => StreamBackpressure::DropWhenFull,
        other => {
            warn!(
                "RS_CAPTURE_STREAM_BACKPRESSURE={other:?} unknown; use block or drop; using block"
            );
            StreamBackpressure::Block
        }
    }
}

fn video_codec_preference_from_env() -> VideoCodecPreference {
    video_codec_preference_from_tokens(
        std::env::var("RS_CAPTURE_ENCODER").ok().as_deref(),
        std::env::var("RS_CAPTURE_NVENC").ok().as_deref(),
        std::env::var("RS_CAPTURE_NVENC_REQUIRED").ok().as_deref(),
    )
}

/// Same token rules as `encoder::WindowsEncoderPreference::from_env_tokens` (aligned `RS_CAPTURE_*` vars).
fn video_codec_preference_from_tokens(
    rs_capture_encoder: Option<&str>,
    rs_capture_nvenc: Option<&str>,
    rs_capture_nvenc_required: Option<&str>,
) -> VideoCodecPreference {
    let force_sw = rs_capture_encoder
        .map(|s| s.eq_ignore_ascii_case("openh264"))
        .unwrap_or(false);
    let skip_nvenc = rs_capture_nvenc
        .map(|s| s == "0" || s.eq_ignore_ascii_case("off"))
        .unwrap_or(false);
    if force_sw || skip_nvenc {
        return VideoCodecPreference::PreferSoftware;
    }
    if rs_capture_nvenc_required
        .map(|s| {
            let t = s.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
    {
        VideoCodecPreference::RequireNvenc
    } else {
        VideoCodecPreference::Auto
    }
}

fn capture_fps_from_env() -> u32 {
    const DEFAULT: u32 = 30;
    let Ok(s) = std::env::var("RS_CAPTURE_FPS") else {
        return DEFAULT;
    };
    match s.parse::<u32>() {
        Ok(n) if (1..=240).contains(&n) => n,
        Ok(n) => {
            warn!("RS_CAPTURE_FPS={n} is out of range 1–240; using {DEFAULT}");
            DEFAULT
        }
        Err(_) => {
            warn!("RS_CAPTURE_FPS={s:?} is not a number; using {DEFAULT}");
            DEFAULT
        }
    }
}

fn video_bitrate_bps_from_env() -> u32 {
    const DEFAULT: u32 = 45_000_000;
    let Ok(s) = std::env::var("RS_CAPTURE_VIDEO_BITRATE") else {
        return DEFAULT;
    };
    if s.is_empty() {
        return DEFAULT;
    }
    match s.parse::<u64>() {
        Ok(n) if (500_000..=200_000_000).contains(&n) => n as u32,
        Ok(n) => {
            warn!("RS_CAPTURE_VIDEO_BITRATE={n} out of range 500000–200000000; using {DEFAULT}");
            DEFAULT
        }
        Err(_) => {
            warn!("RS_CAPTURE_VIDEO_BITRATE={s:?} not an integer; using {DEFAULT}");
            DEFAULT
        }
    }
}

fn frame_pacing_from_env() -> bool {
    match std::env::var("RS_CAPTURE_FRAME_PACING") {
        Ok(s) if s == "0" || s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("false") => {
            false
        }
        Ok(_) => true,
        Err(_) => true,
    }
}

fn async_nvenc_encode_from_env() -> bool {
    match std::env::var("RS_CAPTURE_ASYNC_ENCODE") {
        Ok(s) if s == "0" || s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("false") => {
            false
        }
        Ok(_) => true,
        Err(_) => true,
    }
}

fn cfr_mux_from_env() -> bool {
    match std::env::var("RS_CAPTURE_CFR") {
        Ok(s) if s == "1" || s.eq_ignore_ascii_case("on") || s.eq_ignore_ascii_case("true") => true,
        Ok(_) => false,
        Err(_) => false,
    }
}

fn av_drift_threshold_frames_from_env() -> u64 {
    match std::env::var("RS_CAPTURE_AV_DRIFT_SAMPLES") {
        Ok(s) if s.trim().is_empty() => 0,
        Ok(s) => s.parse::<u64>().unwrap_or(0),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod video_codec_env_tests {
    use super::video_codec_preference_from_tokens;
    use crate::config::VideoCodecPreference;

    #[test]
    fn token_defaults_match_auto() {
        assert_eq!(
            video_codec_preference_from_tokens(None, None, None),
            VideoCodecPreference::Auto
        );
    }

    #[test]
    fn openh264_maps_to_prefer_software() {
        assert_eq!(
            video_codec_preference_from_tokens(Some("openh264"), None, None),
            VideoCodecPreference::PreferSoftware
        );
    }

    #[test]
    fn nvenc_required_maps_to_require_nvenc() {
        assert_eq!(
            video_codec_preference_from_tokens(None, None, Some("yes")),
            VideoCodecPreference::RequireNvenc
        );
    }
}
