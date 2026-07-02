# Building a Rust capture pipeline — and benchmarking it against OBS in Forza

*Early results from a Windows-native screen + system audio capture module — and why I'm not trying to replace OBS for everyone.*

---

[MEDIA: Hero image — side-by-side Forza in-game benchmark summary. Left: Rust capture pipeline run. Right: OBS run. Caption: *Same machine, same route, separate recording sessions.*]

---

## The short version

I've been building **rs-capture-pipeline**: a Rust library that captures your screen and system audio on Windows, encodes video with hardware when possible (NVENC), and hands you either **files** (MP4, H.264, WAV) or **encoded packets** your app can forward to a streaming stack.

It's not a consumer app. It's not OBS with a new skin. It's the **capture-and-encode layer** you'd embed inside a collab tool, a session recorder, a clip app, or anything that needs native capture without asking users to install a full broadcaster.

I ran an early benchmark in **Forza Horizon 6** — same car, same route, two separate sessions: one recording with OBS, one with this pipeline. The numbers are promising enough that I wanted to share them honestly, caveats included.

---

## Why this exists (when OBS already exists)

OBS is excellent at what it does: scenes, plugins, streaming, production workflows. Millions of creators rely on it for good reason.

But when you're **building a product**, you often don't need OBS-the-application. You need:

- Reliable **display capture** on Windows (without fragile screen-grab hacks)
- **System audio** that actually shows up in the recording
- **Hardware encoding** that doesn't torch the CPU
- A **library** you can ship inside your installer, not a separate tool users configure

That's the gap I'm aiming at: **less overhead, more embeddable**, for teams who own the UX and the transport (WebRTC, LiveKit, upload to S3, whatever).

I'm also dogfooding it for a collab-style project — but the pipeline is meant to stay **independent**. One module, many hosts.

---

## What it actually is (without the jargon pile)

At a high level, the stack looks like this:

1. **Capture** — Windows Graphics Capture (WGC) for the display, WASAPI loopback for desktop audio  
2. **Convert** — GPU path from captured frames toward NV12 / encoder-friendly layouts  
3. **Encode** — NVENC when available, sensible fallbacks when not; AAC or Opus for audio  
4. **Output** — Write to disk **or** push `VideoPacket` / `AudioChunk` structs over channels for your app to consume  

The public API lives in a crate called **`capture-runtime`**. The repo also ships a small CLI (`capture-pipeline`) that's basically a reference host — proof that the library works, not the product itself.

[MEDIA: Architecture diagram — capture → pipeline → encoder → output / stream channels. Optional: link to your repo's flowchart if you publish one on the site.]

**What it does *not* include (today):** WebRTC, signaling, RTP, or a LiveKit room join. Those belong in the **host app**. This project stops at "here is timestamped H.264 and audio payloads — you ship them."

---

## How I ran the benchmark

I cared about two different questions:

1. **While recording, how much does capture hurt the game?** (Forza's built-in benchmark + on-screen overlay)  
2. **What comes out in the recording file?** (resolution, bitrate, audio)

### Rules I tried to follow

- **One capture tool at a time** — never OBS and the pipeline simultaneously  
- **Same resolution target** (1080p), **NVENC H.264**, similar bitrate intent (~45 Mbps)  
- **Same-ish driving segment** in Forza (not a perfect lab test, but real gameplay)  
- **MSI Afterburner overlay** on both runs for FPS / GPU / CPU on screen  

The pipeline also writes a **`metrics.csv`** during file recording (CPU and RAM for the capture process only). OBS gets its **Stats** panel for dropped frames. I'll publish more on that workflow in a follow-up.

**Disclaimer:** This is one game, one PC, early software. Treat it as directional, not a white paper.

---

## Results: in-game performance (the part I care about most)

Forza's performance summary compares CPU simulation, CPU render, and GPU frame rates — including **1% lows** and **0.1% lows**, which tell you how bad the worst moments get.

[MEDIA: Full benchmark comparison table — FPS, Low 1%, Low 0.1% for CPU Simulation, CPU Render, GPU. Rust vs OBS with checkmarks.]

### Average FPS (benchmark summary)

| Metric | Rust pipeline | OBS |
|--------|---------------|-----|
| CPU simulation | **297.4** | 296.8 |
| CPU render | **123.0** | 121.1 |
| GPU | **112.8** | 104.2 |

The GPU line is the stand-out: roughly **8% higher** GPU FPS during the benchmark on the pipeline side. CPU simulation is basically a tie — the game logic isn't the story here. CPU render favors the pipeline modestly.

### Stability (1% and 0.1% lows)

| Metric | Rust pipeline | OBS |
|--------|---------------|-----|
| GPU low 1% | **98.6** | 90.2 |
| GPU low 0.1% | **94.5** | 87.7 |
| CPU render low 0.1% | **80.4** | 69.6 |

Higher lows mean fewer "ouch" frames when the scene gets busy. That's the difference you *feel* as a player — less micro-stutter while something is recording in the background.

### Achieved FPS & overlay (real run)

[MEDIA: Screenshot — side-by-side Performance Summary windows with overlay FPS and GPU %.]

On the run I captured for the comparison video:

| | Rust pipeline | OBS |
|---|---------------|-----|
| Achieved FPS | **159** | 149 |
| Overlay (example) | ~104 FPS @ ~90% GPU | ~96 FPS @ ~93% GPU |
| Average latency | **22.2 ms** | 23.0 ms |
| Stutter count | 3 | 2 |

So: **higher FPS with slightly lower GPU utilization** on the overlay — capture isn't free, but the pipeline appears to tax the GPU a bit less for a better result. Stutter count was marginally higher on my pipeline run (3 vs 2); I'd want more sessions before calling that significant.

---

## Results: system resources

[MEDIA: Resources table — GPU ~97% both, CPU ~53% vs ~52%, Memory ~6150 MB vs ~6290 MB.]

| Resource | Rust pipeline | OBS |
|----------|---------------|-----|
| GPU | ~97% | ~97% |
| CPU | ~53% | ~52% |
| Memory | **~6150 MB** | ~6290 MB |

GPU pegged on both — Forza is still the main consumer. CPU is effectively the same. Memory was **~140 MB lower** on the pipeline run. Not revolutionary, but it's the kind of small win that matters when your app is one of several heavy processes.

---

## Results: the recording files (quality vs frame rate)

[MEDIA: Video/audio metadata table — resolution, bitrate, fps, audio bitrate, channels, sample rate.]

| | Rust pipeline | OBS |
|---|---------------|-----|
| Resolution | 1920×1080 | 1920×1080 |
| Video bitrate | **~39.5 Mbps** | ~32.4 Mbps |
| Frames in file | **~53.4 fps** | 60 fps (hard cap) |
| Audio | 192 kbps stereo, 48 kHz | 190 kbps stereo, 48 kHz |

Here's the honest nuance:

- The pipeline recording carried **more bits per second** in the exported file — generally good for detail and fewer compression artifacts.  
- OBS **locked 60 fps** in the output. My file landed around **53 fps** — that's a **muxing / pacing / capture cadence** tuning issue on my side, not something I'm proud of yet. In-game performance was *better*; the *file* frame rate still needs work for parity with a hard-capped 60 fps workflow.

I'll be tightening CFR-style output and frame pacing — the benchmark already showed the hard part (runtime overhead) is moving in the right direction.

[MEDIA: Optional — 10–15 second embedded video or GIF: side-by-side playback of OBS vs pipeline footage. Mute or low music; label each side.]

---

## Who this is for (and who it isn't)

### Good fit

- **Teams building screen share or session recording** into a desktop app  
- **Streaming / RTC products** that want a Windows native publisher instead of browser-only capture  
- **Clip or replay tools** that need NVENC + system audio in-process  
- **Developers** who'd rather depend on a crate than maintain FFmpeg + gdigrab scripts forever  

### Probably not a fit (and that's fine)

- **Solo streamers** who want scenes, plugins, and a mature ecosystem — use OBS  
- **Anyone who needs macOS/Linux today** — Windows is where this lives right now  
- **Anyone who wants WebRTC in the box** — you'll bridge transport in your host; this stops at encoded media  

---

## What's next

Near-term roadmap for the **module** (not a consumer app):

- Examples and integration docs for embedders (`record_to_dir`, `stream_stats`, `INTEGRATION.md` in the repo)  
- Display / source selection APIs  
- Tighter **60 fps file output** to match benchmark-quality in-game performance  
- Optional bridges (LiveKit, RTMP) as **separate** crates or host code — keeping the core transport-agnostic  
- Longer soak tests (10+ minute sessions, drift, reconnect stories)  

[MEDIA: Optional — screenshot of CLI output, `metrics.csv` in Excel, or `stream_stats` terminal demo.]

---

## Try it / get involved

The project is an embeddable Rust workspace: **`capture-runtime`** is the API surface; the CLI is a thin reference host.

If you're building something that needs native capture:

- I'd love to hear what **API shape** you'd need to actually integrate  
- What **trust bar** you'd have (metrics, licensing, CI on Windows, long-session reports)  
- Whether you'd pick this over **OBS**, **FFmpeg**, or **vendor SDKs** — and why  

**[CTA: Link to GitHub repo / waitlist / contact email / "Reply on LinkedIn"]**

---

## Feedback welcome

This is early. The Forza numbers made me optimistic; they didn't make me complacent. If you've shipped capture in production — or you've been burned by A/V sync, system audio on Windows, or "why does screen share destroy FPS" — **tell me what I should measure next** or what would make you trust a module like this.

Drop a comment, open an issue, or reach out directly. Brutal honesty beats polite silence.

---

*Thanks for reading. More technical deep-dives (WGC vs DXGI, audio pacing, downmix for surround WASAPI) coming if people want them.*

---

### Suggested meta description (for SEO)

```
A Rust Windows capture pipeline (WGC + NVENC + system audio) benchmarked against OBS in Forza Horizon 6. Early results show lower game overhead and stronger frame-time lows — plus honest caveats on file fps and who this embeddable module is actually for.
```

### Suggested URL slug

```
/blog/rust-capture-pipeline-forza-benchmark-vs-obs
```
