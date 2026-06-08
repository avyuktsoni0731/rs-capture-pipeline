# rs-capture-pipeline

Windows screen and system-audio capture with hardware-friendly H.264 encoding, optional AAC or Opus, and MP4/H.264 output. Ships as a **library** (`capture-runtime`) for embedding in CLIs, tray apps, or native publishers, plus a small reference binary.

## Features

- **Video:** Windows Graphics Capture (WGC), D3D11 path, **NVENC** (prefer) with **OpenH264** fallback, optional async encode queue.
- **Audio:** WASAPI loopback; multichannel mixes **downmixed to stereo** for AAC/Opus; tunable fold-down and optional presence emphasis.
- **Mux:** Progressive MP4 with H.264 + AAC-LC (Media Foundation), raw `clip.h264`, WAV (`audio.wav`). A/V timeline aligned to encoder PTS when muxing.
- **Streaming:** Optional `crossbeam_channel` sinks emit **`VideoPacket`** (Annex-B H.264) and **`AudioChunk`** (AAC access units or Opus payloads) for hosts that bridge to WebRTC, RTMP, etc.

## Requirements

- **OS:** Windows **64-bit** (capture and runtime are implemented for Windows only today).
- **Rust:** Toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (currently **1.85**). Use `rustup` so the repo picks it up automatically.
- **GPU / drivers:** For NVENC, a supported NVIDIA driver and hardware encode capability; otherwise OpenH264 (CPU) is used.
- **Optional:** [FFmpeg](https://ffmpeg.org/) on `PATH` if you use `RS_CAPTURE_FFMPEG_MUX=1` to build `clip_with_audio.mp4` from `clip.mp4` + `audio.wav`.

## Quick start

```bash
cd rs-capture-pipeline
cargo build --release
```

Run the reference CLI (COM + above-normal priority + Ctrl+C stop):

```bash
cargo run --release -p capture-pipeline-app -- [OUT_DIR] [FRAME_LIMIT] [noaudio]
```

- **`OUT_DIR`** — output directory (default: `capture_out`). Created if missing.
- **`FRAME_LIMIT`** — stop after this many frames; `0` = until Ctrl+C.
- **Third arg `noaudio`** — disable system audio capture.

Example: 10 seconds at 30 fps ≈ 300 frames:

```bash
cargo run --release -p capture-pipeline-app -- capture_out 300
```

Logging uses `RUST_LOG` (e.g. `RUST_LOG=info` or `debug`).

## Examples (embedding)

Minimal hosts live on the **`capture-runtime`** crate:

```bash
# Record to disk (same outputs as the CLI, fewer extras)
cargo run --example record_to_dir -p capture-runtime -- my_out 300

# Stream-only: print video/audio packet rates (no WebRTC — your app owns transport)
cargo run --example stream_stats -p capture-runtime -- 300
RS_CAPTURE_AUDIO_CODEC=opus cargo run --example stream_stats -p capture-runtime -- 300
```

Full integration guide: **[`docs/INTEGRATION.md`](docs/INTEGRATION.md)**.

## Output files

When writing to a directory (default CLI path), you typically get:

| File | Description |
|------|-------------|
| `clip.h264` | Elementary H.264 (Annex B). |
| `clip.mp4` | H.264 in MP4; AAC track when MF AAC is available. |
| `audio.wav` | Float32 stereo (or mono) PCM from the mix that feeds the encoder. |
| `clip_with_audio.mp4` | Only if `RS_CAPTURE_FFMPEG_MUX=1` and FFmpeg succeeds. |

## Workspace layout

| Crate | Role |
|-------|------|
| [`crates/capture`](crates/capture) | WGC session, D3D11, frame → texture. |
| [`crates/pipeline`](crates/pipeline) | BGRA → NV12 / I420, texture pool, staging. |
| [`crates/encoder`](crates/encoder) | NVENC, OpenH264, encoder registry and Windows preferences. |
| [`crates/audio`](crates/audio) | WASAPI loopback, WAV writer, downmix, presence emphasis. |
| [`crates/audio_encoder`](crates/audio_encoder) | MF AAC-LC, Opus (48 kHz; 44.1 kHz resampled in-encoder). |
| [`crates/output`](crates/output) | MP4 writer (H.264 + optional AAC). |
| [`crates/capture-runtime`](crates/capture-runtime) | **Public API:** `run_recording`, `SessionConfig`, `PipelineParams`, stream channels. |
| [`crates/app`](crates/app) | Binary `capture-pipeline` (thin wrapper around `capture-runtime`). |
| [`vendor/nvenc`](vendor/nvenc) | Patched `nvenc` crate (see root `Cargo.toml` `[patch]`). |

## Using as a library

Add a path dependency to your `Cargo.toml`:

```toml
[dependencies]
capture-runtime = { path = "path/to/rs-capture-pipeline/crates/capture-runtime", features = ["serde_config"] }
```

- Call **`run_file_recording`** / **`run_recording`** with **`PipelineParams`** and an **`Arc<AtomicBool>`** stop flag.
- Build params from **`pipeline_params_from_cli_and_env`**, **`SessionConfig`**, or **`PipelineParams::try_from_session_config`**.
- For **stream-only** output, use **`capture_runtime::stream_pair`**, attach senders to **`SessionConfig::with_stream_endpoints`**, and consume **`VideoPacket`** / **`AudioChunk`** on the receivers.
- Initialize **COM** on the thread that uses Media Foundation / WASAPI (see `crates/app/src/main.rs`).

This repository does **not** implement WebRTC, RTP, or a full WebRTC stack—only encoded media and timestamps for a host to forward.

## Environment variables

Behavior is driven by `RS_CAPTURE_*` variables. The full set is implemented in [`crates/capture-runtime/src/env.rs`](crates/capture-runtime/src/env.rs) and encoder selection in [`crates/encoder/src/registry.rs`](crates/encoder/src/registry.rs). Commonly used:

| Variable | Purpose |
|----------|---------|
| `RS_CAPTURE_FPS` | Nominal frame rate (1–240, default 30). |
| `RS_CAPTURE_VIDEO_BITRATE` | Video bitrate in bps. |
| `RS_CAPTURE_ENCODER` / `RS_CAPTURE_NVENC` / `RS_CAPTURE_NVENC_REQUIRED` | Encoder selection and NVENC policy. |
| `RS_CAPTURE_ASYNC_ENCODE` | NVENC async worker path. |
| `RS_CAPTURE_FRAME_PACING` | Sleep to pace compositor polling toward nominal FPS. |
| `RS_CAPTURE_CFR` | CFR-style MP4 video sample timing. |
| `RS_CAPTURE_AUDIO_CODEC` | `aac` (default) or `opus` for encoded stream/file behavior. |
| `RS_CAPTURE_AAC_BITRATE` | AAC bitrate (default 192000). |
| `RS_CAPTURE_OPUS_BITRATE` | Opus bitrate when Opus is selected. |
| `RS_CAPTURE_PRESENCE_EMPHASIS` | Stereo mid/vocal tilt (`0` / `off` to disable). |
| `RS_CAPTURE_STREAM_BACKPRESSURE` | `block` vs `drop` for bounded stream channels. |
| `RS_CAPTURE_AV_DRIFT_SAMPLES` | Optional A/V drift trim/pad threshold (0 = off). |
| `RS_CAPTURE_FFMPEG_MUX` | Set to `1` to mux WAV + MP4 with FFmpeg when installed. |
| `RS_CAPTURE_NO_PRIORITY_BOOST` | Set to `1` to skip raising process priority (CLI). |

## Documentation

- **[`docs/INTEGRATION.md`](docs/INTEGRATION.md)** — **start here** when embedding in another app.
- **[`CURSOR_CONTEXT.md`](CURSOR_CONTEXT.md)** — architecture notes, env overview, and pitfalls for editors/agents.
- **[`CHANGELOG.md`](CHANGELOG.md)** — API / example changes.
- Crate-level rustdoc: run `cargo doc -p capture-runtime --open`.

## License

`capture-runtime` declares `MIT OR Apache-2.0` in its manifest; other crates may follow the same unless noted in their `Cargo.toml`. Confirm per crate before redistribution.
