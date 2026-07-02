# Game capture benchmark (Forza / OBS vs pipeline)

Simple workflow: **two separate recordings** (never run OBS and the pipeline at the same time), same game segment, then **side-by-side in an editor**.

## The easy mental model

| What | Tool |
|------|------|
| **Gameplay + capture quality** | OBS recording *or* `capture-pipeline` |
| **On-screen FPS / GPU / CPU (both tools)** | **MSI Afterburner + RivaTuner** (same overlay for fair compare) |
| **Pipeline-only CPU/RAM/FPS** | **`capture_out/metrics.csv`** (built into the pipeline) |
| **Live numbers on 2nd monitor** | `scripts/live_metrics.ps1` |
| **OBS-only dropped frames** | OBS **Stats** dock |

You do **not** need Cursor or the terminal in the recording — launch the game fullscreen, hide the terminal, use Afterburner overlay on the game.

---

## 1. One-time: MSI Afterburner (for OBS **and** pipeline runs)

1. Install [MSI Afterburner](https://www.msi.com/Landing/afterburner) + bundled **RivaTuner Statistics Server**.
2. Afterburner → **Settings → Monitoring** — enable and check **Show in On-Screen Display** for:
   - Framerate
   - GPU usage %
   - CPU usage %
   - RAM usage (optional)
3. Assign a hotkey for OSD (e.g. **Ctrl+Shift+O**).
4. In-game you’ll see the same overlay whether OBS or the pipeline is recording.

For **OBS**, also open **View → Stats** and watch **Dropped frames** / **Encoding lag** (screenshot after the run).

---

## 2. Pipeline run (Forza + metrics.csv)

```powershell
cd C:\Users\Avyukt\Desktop\afas_codebases\rs-capture-pipeline

$env:RS_CAPTURE_FPS="60"
$env:RS_CAPTURE_VIDEO_BITRATE="45000000"
# metrics.csv is ON by default; disable with: $env:RS_CAPTURE_METRICS="0"

cargo run --release -p capture-pipeline-app -- forza_pipeline 0
```

- **`0`** = stop with **Ctrl+C** when you’re done (e.g. 2–3 minutes of driving).
- Outputs: `forza_pipeline\clip.mp4`, `audio.wav`, **`forza_pipeline\metrics.csv`**.

**Optional — live box on second monitor** (not in the game recording):

```powershell
.\scripts\live_metrics.ps1 forza_pipeline\metrics.csv
```

**Before you record:** start Forza, hide/minimize the terminal, enable Afterburner OSD in-game.

---

## 3. OBS run (same settings idea)

Match the pipeline where possible:

- **Display capture** (WGC on Win11)
- **NVENC H.264**, **CBR 45000 kbps**, **60 fps**, 1080p
- **Desktop audio** on (WASAPI)
- Record **MKV/MP4** for the same kind of driving session

Use the **same Afterburner overlay**. Note OBS Stats at the end.

---

## 4. What `metrics.csv` contains (pipeline only)

One row per second:

```csv
elapsed_s,frames,video_fps,cpu_percent,memory_mb,pid,process
```

- **`cpu_percent` / `memory_mb`** — **only the capture-pipeline process**, not the whole PC.
- **`video_fps`** — frames encoded per second (capture throughput).

Open in Excel or paste into your edit as a chart for the comparison video.

Disable file: `RS_CAPTURE_METRICS=0`.

---

## 5. Side-by-side video in post

1. Trim both clips to the same start (countdown or flash).
2. Stack left = OBS, right = pipeline (or vice versa).
3. Add text labels and a small chart from `metrics.csv` vs OBS Stats screenshot.

FFmpeg example (adjust paths):

```powershell
ffmpeg -i obs_forza.mp4 -i forza_pipeline\clip.mp4 -filter_complex "[0:v]scale=960:540[left];[1:v]scale=960:540[right];[left][right]hstack[v]" -map "[v]" -map 0:a? comparison.mp4
```

---

## 6. Fair test rules

- Same resolution, FPS, encoder (NVENC), and bitrate.
- **One capture app at a time** (game gets full GPU).
- Same car / same route / similar weather when possible.
- Reboot or idle GPU between runs if you want clean numbers.

---

## 7. What we don’t measure in-code yet

- **Whole-system GPU** (game + capture) — use Afterburner.
- **NVENC engine %** — Afterburner or GPU-Z log.
- **OBS process CPU** — Task Manager → Details → `obs64.exe` during OBS run, or HWiNFO log.

A future optional overlay window could be added; for now **Afterburner + metrics.csv** is the simplest honest compare.
