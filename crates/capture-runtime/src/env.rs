//! Environment-driven defaults mapped into [`PipelineParams`].

use std::path::PathBuf;

use tracing::{info, warn};

use crate::params::PipelineParams;

/// Build [`PipelineParams`] from CLI arguments plus `RS_CAPTURE_*` env overrides.
pub fn pipeline_params_from_cli_and_env(
    output_directory: impl Into<PathBuf>,
    frame_limit: u32,
    capture_system_audio: bool,
) -> PipelineParams {
    let fps = capture_fps_from_env();
    let video_bitrate_bps = video_bitrate_bps_from_env();
    let frame_pacing = frame_pacing_from_env();
    let async_nvenc = async_nvenc_encode_from_env();
    let cfr_mux = cfr_mux_from_env();
    let av_drift_threshold_pcm_frames = av_drift_threshold_frames_from_env();
    let force_software_encoder_only = std::env::var("RS_CAPTURE_ENCODER")
        .map(|s| s.eq_ignore_ascii_case("openh264"))
        .unwrap_or(false);
    let remux_with_ffmpeg = std::env::var("RS_CAPTURE_FFMPEG_MUX").ok().as_deref() == Some("1");

    PipelineParams {
        output_directory: output_directory.into(),
        frame_limit,
        capture_system_audio,
        fps,
        video_bitrate_bps,
        frame_pacing,
        async_nvenc,
        cfr_mux,
        av_drift_threshold_pcm_frames,
        force_software_encoder_only,
        remux_with_ffmpeg,
    }
}

/// Log effective capture settings (call from the binary after building params).
pub fn log_pipeline_startup(p: &PipelineParams) {
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
