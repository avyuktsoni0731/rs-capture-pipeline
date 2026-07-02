# Blog posts

Markdown sources for the project website. Copy the post body into your CMS or static site generator.

| File | Title | Status |
|------|-------|--------|
| [`forza-benchmark-vs-obs.md`](forza-benchmark-vs-obs.md) | Building a Rust capture pipeline — and benchmarking it against OBS in Forza | Draft — add media where marked |

## Media checklist (Forza post)

Place these assets where the post says `[MEDIA: …]`:

1. **Hero** — Side-by-side Forza benchmark summary (Rust vs OBS in-game overlay).
2. **Benchmark tables** — FPS / Low 1% / Low 0.1% comparison graphic.
3. **Video metadata** — Bitrate, fps, audio table.
4. **Resources** — GPU / CPU / memory comparison.
5. **Optional** — Short screen recording of `clip.mp4` playback or pipeline diagram from project docs.

## Suggested front matter (YAML)

```yaml
title: "Building a Rust capture pipeline — and benchmarking it against OBS"
description: "A Windows-native screen + system audio capture module in Rust, early Forza Horizon 6 benchmarks vs OBS, and who it's actually for."
date: 2026-06-29
tags: [rust, windows, capture, gamedev, benchmarking]
author: Your Name
```

## Publishing notes

- Keep the **53 vs 60 fps file output** caveat visible — it builds trust.
- Link to your repo when public; until then, “DM for early access” or similar.
- Alt text for each image: describe what the chart proves, not just “benchmark results.”
