# rs-capture-pipeline — project context

Single source of truth for this workspace: a **Windows-first** screen and system-audio capture stack in Rust, with an embeddable runtime (`capture-runtime`) for CLIs and hosts (e.g. MyCord-style apps).

---

## Philosophy

- **Prefer the GPU** where the stack supports it: WGC produces GPU textures; NVENC consumes registered BGRA textures without a CPU color conversion in the hot path.
- **Stay honest about CPU paths**: OpenH264 and Media Foundation hardware H.264 (`mf_h264_hw`) use **I420 readback** from NV12 GPU buffers today — not an all-GPU path for those backends.
- **Modular crates**: capture → pipeline (convert / pools / readback) → encoder → output; orchestration lives in `capture-runtime`.
- **`windows` crate** (`windows-rs`) for WinRT/COM/D3D/Media Foundation.

---

## Workspace layout (actual)

```
rs-capture-pipeline/
├── Cargo.toml                  # Workspace root (members below)
├── CURSOR_CONTEXT.md           # This file
├── shaders/
│   └── color_convert.hlsl      # BGRA→NV12 compute (see pipeline build)
├── vendor/
│   └── nvenc/                  # Patched nvenc dependency (see root [patch.crates-io])
└── crates/
    ├── capture/                # WGC session, D3D11 device, DXGI helpers, monitor pick
    ├── audio/                  # WASAPI loopback (+ WAV helpers)
    ├── pipeline/               # BgraToNv12Converter, TexturePool, readback, stage queues
    ├── encoder/                # NVENC, MF HW H.264, OpenH264, registry
    ├── audio_encoder/          # MF AAC-LC for MP4
    ├── output/                 # Annex-B helpers, MP4 (H.264 + AAC) — not ffmpeg-next muxer
    ├── capture-runtime/        # SessionConfig, PipelineParams, env, run_recording (Windows)
    └── app/                    # Thin CLI → pipeline_params_from_cli_and_env + run_file_recording
```

There is **no** top-level `docs/` or `tests/` tree in this repo today; tests live beside crates (`#[cfg(test)]` and unit modules).

---

## Workspace dependencies (authoritative)

See root **`Cargo.toml`**. Highlights:

- **`windows` 0.62** with features including Graphics Capture, D3D11/DXGI, Media Foundation, WASAPI, COM, Ole/Variant (CodecAPI), etc.
- **`crossbeam-channel`**, **`tokio`**, **`tracing`**, **`anyhow`**, **`serde`**
- **`nvenc`** is **`[patch.crates-io]`** → `vendor/nvenc` (EOS flush / Drop fixes).

This workspace does **not** currently list `ffmpeg-next`, `audiopus`, or `x264` in `[workspace.dependencies]`; MP4 writing is implemented in `crates/output` (see `mp4_file.rs`).

---

## Encoder crate (`crates/encoder`)

### `VideoEncoder` trait (real)

Defined in **`crates/encoder/src/traits.rs`**:

- **`encode_i420(&mut self, i420, timestamp_us)`** — planar I420 input (software encoders + MF hardware path after CPU readback).
- **`encode_bgra_texture`** — optional; NVENC returns **`supports_bgra_gpu_encode() == true`** and uses registered textures (OBS-style).
- **`codec()`** — returns **`VideoCodec`** (`H264` / `H265` / `Av1` enum; practical paths today are H.264).

### Backends

| Module / entry | Role |
|----------------|------|
| **`nvenc.rs`** | NVIDIA NVENC via patched **`nvenc`** crate + D3D11 registration |
| **`mf_h264_hw.rs`** | Media Foundation **synchronous hardware** H.264 MFT (NV12 in, Annex-B-ish out); optional DXGI device manager + CodecAPI keyframes + stream-change handling |
| **`qsv.rs`** | Intel Quick Sync–class: **`try_create_qsv_encoder`** wraps **`MfH264HwEncoder::try_new(config, Some(device))`**; **`intel_adapter_present`** for DXGI probe |
| **`amf.rs`** | **Probe only** (`amd_adapter_present`) — AMD MF encoders are picked via the same MF enumeration as Intel; **no native AMF SDK** in-tree |
| **`openh264_enc.rs`** | Software H.264 (**OpenH264** crate), not x264 |
| **`registry.rs`** | **`WindowsEncoderPreference`**, **`create_windows_encoder`**, **`from_env` / `from_env_tokens`** |

### Selection order (`create_encoder_with_preference`)

1. **`SoftwareOnly`** → OpenH264 only.
2. **`RequireNvenc`** → NVENC only (fails if no device or init fails).
3. With a D3D11 device: **NVENC** if init succeeds.
4. Else **MF hardware H.264** (`qsv::try_create_qsv_encoder`) — any vendor exposing a suitable sync hardware MFT (Intel / AMD / others), not Intel-only.
5. Else **OpenH264**.

Environment (same token rules as **`capture-runtime`**):

- `RS_CAPTURE_ENCODER=openh264` → force software path (encoder: `SoftwareOnly`).
- `RS_CAPTURE_NVENC=0` / `off` → skip NVENC (same).
- `RS_CAPTURE_NVENC_REQUIRED=1` → NVENC required when not overridden by software forcing.

Embedders should pass an explicit **`WindowsEncoderPreference`** via **`create_windows_encoder`** instead of relying on env.

---

## Capture runtime (`crates/capture-runtime`)

- **`SessionConfig`** / **`OutputTarget`** / **`VideoCodecPreference`** / **`StreamBackpressure`** — **`crates/capture-runtime/src/config.rs`**
- **`PipelineParams`** / **`RecordingOutputs`** / **`RunStats`** — **`params.rs`**
- **`pipeline_params_from_cli_and_env`**, **`pipeline_params_stream_only`**, etc. — **`env.rs`** (`RS_CAPTURE_*`: fps, bitrate, frame pacing, async NVENC, CFR remux, stream backpressure, video codec preference, …)
- **`PipelineParams::try_from_session_config`** maps session + env overrides (software force / require NVENC) into resolved params; on Windows, **`windows_encoder_preference()`** maps **`VideoCodecPreference`** → **`encoder::WindowsEncoderPreference`**
- **`run_file_recording` / `run_recording`** — **`run_win.rs`** (Windows): WGC, optional NVENC async encode thread, MF/OpenH264 I420 path, WASAPI loopback, MF AAC-LC **or** Opus (`SessionConfig.audio_codec` / `RS_CAPTURE_AUDIO_CODEC`), disk + optional bounded stream channels with block vs drop policy (Opus: MP4 video-only; packets on stream as **`AudioChunk::OpusPacket`**)

---

## Pipeline (`crates/pipeline`)

- **`BgraToNv12Converter`** — GPU compute path for color conversion
- **`TexturePool`**, **`Nv12Targets`**
- **`readback`** — pitch-aware copies for I420 assembly for CPU encoders
- **`stage_channel`** — timed staging between pipeline steps

---

## Output (`crates/output`)

- **`Mp4H264File`** — H.264 + AAC in MP4
- **`annexb`** — NAL parsing / AVCC helpers

---

## Application entrypoints

- **`crates/app`**: COM init, optional process priority boost, Ctrl+C stop flag, **`pipeline_params_from_cli_and_env`**, **`run_file_recording`**
- Hosts link **`capture-runtime`** and build **`PipelineParams`** from **`SessionConfig`** or env helpers

---

## Environment variables (non-exhaustive)

Documented in code: **`crates/capture-runtime/src/env.rs`** and encoder **`registry.rs`**.

| Variable | Effect (high level) |
|----------|------------------------|
| `RS_CAPTURE_ENCODER=openh264` | Force software video encoder |
| `RS_CAPTURE_NVENC=0` | Skip NVENC |
| `RS_CAPTURE_NVENC_REQUIRED=1` | Require NVENC (when not forced to software) |
| `RS_CAPTURE_FPS`, `RS_CAPTURE_VIDEO_BITRATE` | Defaults for CLI/env pipeline |
| `RS_CAPTURE_STREAM_BACKPRESSURE` | `block` vs `drop` for bounded stream queues |
| `RS_CAPTURE_ASYNC_ENCODE` | Async NVENC encode path |
| `RS_CAPTURE_AUDIO_CODEC` | `aac` (default) or `opus` — sets `PipelineParams.audio_codec` from CLI env |
| `RS_CAPTURE_OPUS_BITRATE` | Opus bits/sec (default 128000) |
| `RS_CAPTURE_NO_PRIORITY_BOOST=1` | Skip raising process priority (CLI) |

---

## Pitfalls (aligned with this codebase)

1. **MF / OpenH264 paths** use **readback → I420**; do not assume zero-copy CPU video for those backends.
2. **WGC** frame pools have limited slots — keep capture/completion tight (see capture crate usage in `run_win.rs`).
3. **NVENC** path uses **BGRA texture registration** when async encode is enabled — distinct from I420 MF encode.
4. **COM** must be initialized on threads that use MF/WASAPI (CLI initializes MTA on main thread; audio thread also calls `CoInitializeEx`).
5. **Annex-B vs length-prefixed**: MF HW encoder normalizes for downstream; know what your muxer expects (`output` helpers exist).

---

## Relationship to MyCord (product context)

MyCord is a motivating use case: browser-first RTC with optional **native capture** for higher quality / system audio on Windows. This repo does **not** implement WebRTC or a localhost WebSocket bridge yet; **`OutputTarget::StreamOnly`** / **`FilesAndStream`** send **`VideoPacket`** / **`AudioChunk`** over **`crossbeam_channel`** for embedding.

---

## References

- **OBS** — practical capture/encode patterns
- **windows-rs** — [microsoft/windows-rs](https://github.com/microsoft/windows-rs)
- **NVENC** — vendor SDK docs (encode session uses in-tree **`nvenc`** bindings)

---

## Maintenance

When adding crates, public types, or env vars, update **this file** and the crate-level docs that point here (`encoder::EncoderConfig` doc still mentions this filename — keep behavior descriptions consistent).
