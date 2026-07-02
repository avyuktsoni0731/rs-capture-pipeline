# Integrating `capture-runtime`

This guide is for **hosts** that embed the Windows capture SDK: streaming apps, recorders, desktop agents, or collab tools. Transport (WebRTC, RTMP, S3, etc.) stays **in your repo**; this library produces files and/or encoded packets.

## Supported public API

Treat these as the **stable contract** (semver targets `capture-runtime`):

| Item | Role |
|------|------|
| `run_recording` / `run_file_recording` | Blocking session loop (Windows). |
| `SessionConfig`, `OutputTarget` | Declarative session (files / stream / both). |
| `PipelineParams`, `RecordingOutputs`, `RunStats` | Resolved runtime parameters and stats. |
| `stream_pair` | Bounded `crossbeam_channel` queues for video + audio. |
| `VideoPacket`, `AudioChunk` | Encoded output types + `timestamp_us`. |
| `pipeline_params_from_cli_and_env` | Files + `RS_CAPTURE_*` env overrides. |
| `pipeline_params_stream_only` | Stream-only params from existing senders. |
| `pipeline_params_files_and_stream` | Disk + duplicate stream. |
| `PipelineParams::try_from_session_config` | Host-built `SessionConfig` → params. |
| `log_pipeline_startup` | Log effective settings at session start. |

Internal modules (`run_win`, encoder internals, etc.) may change without notice.

## Platform requirements

- **Windows 64-bit** only for `run_recording` today.
- **COM (MTA)** on the thread that runs the capture loop (Media Foundation, WASAPI).
- **NVIDIA driver** optional; NVENC used when available unless env forces software OpenH264.

## Minimal file recording

See **`crates/capture-runtime/examples/record_to_dir.rs`**.

```bash
cargo run --example record_to_dir -p capture-runtime -- my_out 300
```

Steps your binary must perform:

1. `CoInitializeEx(None, COINIT_MULTITHREADED)` (Windows).
2. Build `PipelineParams` (e.g. `pipeline_params_from_cli_and_env(dir, frame_limit, true)`).
3. `Arc<AtomicBool>` stop flag + Ctrl+C or UI handler.
4. `run_recording(&params, stop)?` on a thread that has COM initialized.
5. Read `clip.h264`, `clip.mp4`, `audio.wav` under the output directory.

## Stream-only (your transport owns the wire)

See **`crates/capture-runtime/examples/stream_stats.rs`**.

```bash
cargo run --example stream_stats -p capture-runtime -- 300
RS_CAPTURE_AUDIO_CODEC=opus cargo run --example stream_stats -p capture-runtime -- 300
```

Pattern:

```rust
let (video_tx, audio_tx, video_rx, audio_rx) = stream_pair(64, 128);
let params = pipeline_params_stream_only(video_tx, audio_tx, frame_limit, true);

// Capture thread (COM on this thread):
std::thread::spawn(move || run_recording(&params, stop.clone()));

// Your thread: recv VideoPacket / AudioChunk, forward to WebRTC / RTMP / etc.
while running {
    if let Ok(vp) = video_rx.try_recv() { /* annex_b, timestamp_us, is_keyframe */ }
    if let Ok(ac) = audio_rx.try_recv() { /* AacRaw or OpusPacket */ }
}
```

### `VideoPacket`

- `annex_b`: H.264 with start codes (Annex B).
- `timestamp_us`: encoder timeline (µs since session anchor).
- `is_keyframe`: sync sample hint for muxers / RTP.

### `AudioChunk`

| Variant | When |
|---------|------|
| `AacRaw` | Default `RS_CAPTURE_AUDIO_CODEC=aac`; raw AAC-LC access units (no ADTS). |
| `OpusPacket` | `RS_CAPTURE_AUDIO_CODEC=opus`; 48 kHz timeline, 20 ms frames from encoder. |
| `PcmF32Interleaved` | Rare on stream path; PCM debug. |

MP4 file mux uses AAC; Opus is intended for **stream** consumers (WebRTC-style).

## SessionConfig (programmatic hosts)

```rust
use capture_runtime::{stream_pair, AudioCodecChoice, PipelineParams, SessionConfig};

let (vtx, atx, _vrx, _arx) = stream_pair(64, 128);
let mut session = SessionConfig::with_stream_endpoints(vtx, atx);
session.fps = 60;
session.video_bitrate_bps = 8_000_000;
session.audio_codec = AudioCodecChoice::Opus;
session.capture_system_audio = true;

let params = PipelineParams::try_from_session_config(session, false)?;
```

Helpers: `SessionConfig::files_only(dir)`, `with_stream_endpoints`, `files_and_stream`.

## Threading and lifecycle

| Concern | Recommendation |
|---------|----------------|
| **COM** | Initialize on the **capture** thread before `run_recording`. |
| **Blocking** | `run_recording` blocks until `stop` or `frame_limit`. |
| **Stop** | Set `stop.store(true, SeqCst)`; runner drains audio and flushes encoders. |
| **Consumer slow** | Use **bounded** `stream_pair`; set `StreamBackpressure::DropWhenFull` or increase capacity. |
| **Stats** | `RunStats` reports frames, PCM samples, stream send/drop counts. |

Do **not** call `run_recording` from multiple threads concurrently for one session.

## Environment variables

Hosts can honor the same `RS_CAPTURE_*` knobs as the reference CLI (see root `README.md` and `crates/capture-runtime/src/env.rs`). Common:

- `RS_CAPTURE_FPS`, `RS_CAPTURE_VIDEO_BITRATE`
- `RS_CAPTURE_AUDIO_CODEC=opus` for stream Opus
- `RS_CAPTURE_STREAM_BACKPRESSURE=drop` when the consumer must not block capture

## What this library does **not** do

- WebRTC, ICE, RTP packetization, LiveKit room join
- Signaling, tokens, cloud upload
- UI, window picker (default display capture today)
- macOS capture (planned / separate milestone)

Your product implements transport and UX; this crate is the **Windows capture + encode** layer.

## Reference binaries

| Binary / example | Purpose |
|------------------|---------|
| `capture-pipeline` (`crates/app`) | Full-featured CLI reference host |
| `record_to_dir` example | Minimal file output |
| `stream_stats` example | Minimal stream consumer |

## Pitfalls

1. **Annex B vs AVCC** — stream is Annex B; some APIs need length-prefixed NALs.
2. **Surround WASAPI** — downmixed to stereo before AAC/Opus.
3. **A/V sync** — audio mux paced to video `ts_us`; use `RS_CAPTURE_AV_DRIFT_SAMPLES` only if you need trim/pad watchdog.
4. **Opus + MP4** — MP4 remains video-only when using Opus; mux AAC for file-first workflows.

## Next steps for your repo

1. Path-depend on `capture-runtime` in your `Cargo.toml`.
2. Copy the `record_to_dir` or `stream_stats` pattern.
3. Forward `VideoPacket` / `AudioChunk` to your SFU or storage.
4. Keep LiveKit/WebRTC code **outside** this workspace to preserve module independence.
