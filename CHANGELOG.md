# Changelog

All notable changes to the embeddable **`capture-runtime`** API and examples are documented here.

## [0.1.0] - 2026-05-03

### Added

- **`docs/INTEGRATION.md`** — host integration guide (COM, threading, stream API).
- **`capture-runtime` examples:**
  - `record_to_dir` — minimal file recording host.
  - `stream_stats` — stream-only packet throughput demo (no transport).
- Root **`README.md`** for workspace overview.

### Notes

- Windows-only `run_recording` implementation.
- Public API surface: `SessionConfig`, `PipelineParams`, `VideoPacket`, `AudioChunk`, `stream_pair`, env helpers.
