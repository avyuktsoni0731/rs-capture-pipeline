# 🎥 CaptureCore — OBS-Like Screen Capture Pipeline
## Cursor IDE Project Context & Master Reference

> This document is the single source of truth for building CaptureCore — a high-performance,
> system-level screen/audio capture and encoding pipeline in Rust (primary) or C++, targeting
> Windows first, designed to eventually power MyCord (a personal WebRTC communication app).

---

## 🧠 Project Philosophy

- **Stay on the GPU.** Frames should go: GPU (capture) → GPU (color convert) → GPU encoder (NVENC/AMF).
  Never touch CPU RAM unless absolutely forced to (e.g. software encoding fallback).
- **Don't fight the OS.** Use the APIs the OS provides (WGC, WASAPI, NVENC). Don't try to
  out-clever them.
- **Modular pipeline.** Every stage (capture → convert → encode → output) is a separate module
  with a clean interface. Stages communicate through thread-safe queues.
- **Honest about limitations.** System audio on non-Windows = not supported in web context.
  Say so clearly. Don't silently fail.
- **Target language: Rust** using `windows-rs` crate for all WinRT/COM/DirectX APIs.
  C++ is acceptable if Rust FFI overhead becomes a problem for a specific module.

---

## 🗂️ Project Structure

```
capture-core/
├── Cargo.toml                    # Workspace root
├── CURSOR_CONTEXT.md             # This file
│
├── crates/
│   ├── capture/                  # Screen/window capture
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── wgc.rs            # Windows.Graphics.Capture (primary)
│   │   │   ├── dxgi.rs           # DXGI Desktop Duplication (fallback/low-level)
│   │   │   └── monitor.rs        # Monitor/window enumeration
│   │   └── Cargo.toml
│   │
│   ├── audio/                    # Audio capture
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── wasapi.rs         # WASAPI loopback capture
│   │   │   └── mic.rs            # Microphone input
│   │   └── Cargo.toml
│   │
│   ├── pipeline/                 # Frame pipeline & queues
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── frame.rs          # Frame type definitions
│   │   │   ├── queue.rs          # Thread-safe bounded frame queue
│   │   │   ├── texture_pool.rs   # Reusable GPU texture pool
│   │   │   └── color_convert.rs  # BGRA→NV12 GPU compute shader
│   │   └── Cargo.toml
│   │
│   ├── encoder/                  # Video encoding
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── traits.rs         # VideoEncoder trait
│   │   │   ├── nvenc.rs          # NVIDIA NVENC (primary)
│   │   │   ├── amf.rs            # AMD AMF
│   │   │   ├── qsv.rs            # Intel QuickSync
│   │   │   └── x264.rs           # Software fallback
│   │   └── Cargo.toml
│   │
│   ├── audio_encoder/            # Audio encoding
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── opus.rs           # Opus (for WebRTC)
│   │   │   └── aac.rs            # AAC (for file output)
│   │   └── Cargo.toml
│   │
│   ├── output/                   # Output sinks
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── traits.rs         # OutputSink trait
│   │   │   ├── file.rs           # MP4/MKV via ffmpeg-sys
│   │   │   ├── rtmp.rs           # RTMP streaming
│   │   │   └── webrtc.rs         # libdatachannel WebRTC output
│   │   └── Cargo.toml
│   │
│   └── app/                      # CLI / orchestrator
│       ├── src/
│       │   ├── main.rs
│       │   ├── session.rs        # Capture session management
│       │   └── config.rs         # Config structs
│       └── Cargo.toml
│
├── shaders/
│   └── color_convert.hlsl        # BGRA→NV12 HLSL compute shader
│
├── docs/
│   ├── architecture.md
│   ├── apis.md
│   └── webrtc_integration.md
│
└── tests/
    ├── capture_test.rs
    ├── encode_test.rs
    └── pipeline_test.rs
```

---

## 📦 Dependencies (Cargo.toml workspace)

```toml
[workspace]
members = [
  "crates/capture",
  "crates/audio",
  "crates/pipeline",
  "crates/encoder",
  "crates/audio_encoder",
  "crates/output",
  "crates/app",
]

[workspace.dependencies]
# Windows APIs — core dependency for everything
windows = { version = "0.58", features = [
  # DirectX
  "Win32_Graphics_Direct3D11",
  "Win32_Graphics_Direct3D",
  "Win32_Graphics_Dxgi",
  "Win32_Graphics_Dxgi_Common",
  "Win32_Graphics_Direct3D_Fxc",
  # Windows Graphics Capture (WGC)
  "Graphics_Capture",
  "Graphics_DirectX",
  "Graphics_DirectX_Direct3D11",
  "Win32_System_WinRT_Direct3D11",
  "Win32_System_WinRT_Graphics_Capture",
  # WASAPI audio
  "Win32_Media_Audio",
  "Win32_Media_Audio_Endpoints",
  "Win32_Devices_Properties",
  # Core Win32
  "Win32_Foundation",
  "Win32_System_Com",
  "Win32_System_Threading",
  "Win32_UI_WindowsAndMessaging",
]}

# Threading & async
tokio = { version = "1", features = ["full"] }
crossbeam-channel = "0.5"
parking_lot = "0.12"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
anyhow = "1"
thiserror = "1"

# FFmpeg bindings (for muxing output)
ffmpeg-next = "7"          # High-level Rust FFmpeg bindings
ffmpeg-sys-next = "7"      # Low-level if needed

# Opus audio encoding
audiopus = "0.3"

# Serialization (config)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## 🔌 External C Libraries / SDKs (link manually)

These are not on crates.io — you link them via `build.rs` or `bindgen`:

| SDK | Purpose | Where to get |
|-----|---------|--------------|
| **NVENC SDK** | NVIDIA hardware encoding | developer.nvidia.com/nvidia-video-codec-sdk |
| **AMF SDK** | AMD hardware encoding | github.com/GPUOpen-LibrariesAndSDKs/AMF |
| **libdatachannel** | C++ WebRTC (native) | github.com/paullouisageneau/libdatachannel |
| **x264** | Software H.264 encoding | videolan.org/developers/x264.html |

For NVENC and AMF, you load them at runtime via `LoadLibrary` / `GetProcAddress` —
this way the app doesn't crash on machines without NVIDIA/AMD GPUs.

---

## 🪟 Layer 1: Screen Capture

### Primary: Windows.Graphics.Capture (WGC)

**What it does:** Hooks into Desktop Window Manager (DWM) and gives you a direct
GPU texture reference per frame. Zero CPU involvement. Captures hardware-accelerated
content, games, HDR. Available Windows 10 1903+.

**Key types:**
- `GraphicsCaptureItem` — what you're capturing (window or monitor)
- `Direct3D11CaptureFramePool` — pool of GPU textures frames arrive into
- `GraphicsCaptureSession` — the active capture
- `Direct3D11CaptureFrame` — one captured frame (wraps an `IDirect3DSurface`)

**Rust implementation pattern:**

```rust
// crates/capture/src/wgc.rs
use windows::{
    Graphics::Capture::*,
    Graphics::DirectX::Direct3D11::*,
    Graphics::DirectX::DirectXPixelFormat,
    Win32::Graphics::Direct3D11::*,
    Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
};

pub struct WgcCapture {
    item: GraphicsCaptureItem,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
}

impl WgcCapture {
    pub fn new(
        hwnd: HWND,           // or HMONITOR for full monitor
        d3d_device: &ID3D11Device,
    ) -> anyhow::Result<Self> {
        // 1. Create capture item from window
        let item = GraphicsCaptureItem::CreateFromWindowId(
            WindowId { Value: hwnd.0 as u64 }
        )?;

        // 2. Wrap D3D11 device in WinRT interface
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let rt_device = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?
        };
        let rt_device: IDirect3DDevice = rt_device.cast()?;

        // 3. Create frame pool (2 buffers = low latency)
        let size = item.Size()?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &rt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,      // buffer count
            size,
        )?;

        // 4. Create and start session
        let session = frame_pool.CreateCaptureSession(&item)?;
        
        // Optional: disable yellow border (Windows 11)
        session.SetIsBorderRequired(false)?;
        // Optional: capture cursor or not
        session.SetIsCursorCaptureEnabled(true)?;
        
        session.StartCapture()?;

        Ok(Self { item, frame_pool, session })
    }

    // Call this in a loop on your capture thread
    pub fn try_get_frame(&self) -> anyhow::Result<Option<Direct3D11CaptureFrame>> {
        Ok(self.frame_pool.TryGetNextFrame()?)
    }
    
    // Get the underlying ID3D11Texture2D from a frame
    pub fn get_texture(frame: &Direct3D11CaptureFrame) -> anyhow::Result<ID3D11Texture2D> {
        let surface = frame.Surface()?;
        // Access the underlying DX11 texture via interop
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        Ok(texture)
    }
}
```

**Important WGC notes:**
- Frames arrive on whatever thread the frame pool handler is on.
- Always call `frame_pool.TryGetNextFrame()` quickly — don't do heavy work in the handler.
- If the capture target is destroyed (window closed), `FrameArrived` stops firing.
- The `Direct3D11CaptureFrame` must be dropped before the next frame is accepted.
- For monitors: use `GraphicsCaptureItem::CreateFromDisplayId(HMONITOR)`.

---

### Fallback: DXGI Desktop Duplication

Use when WGC isn't available or for lower-level access. Does NOT capture
hardware overlays or protected content.

```rust
// crates/capture/src/dxgi.rs
use windows::Win32::Graphics::{Dxgi::*, Direct3D11::*};

pub struct DxgiCapture {
    duplication: IDXGIOutputDuplication,
    device: ID3D11Device,
}

impl DxgiCapture {
    pub fn acquire_frame(&self, timeout_ms: u32) -> anyhow::Result<ID3D11Texture2D> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        
        unsafe {
            self.duplication.AcquireNextFrame(
                timeout_ms,
                &mut frame_info,
                &mut resource,
            )?;
        }
        
        let resource = resource.unwrap();
        let texture: ID3D11Texture2D = resource.cast()?;
        Ok(texture)
    }

    pub fn release_frame(&self) -> anyhow::Result<()> {
        unsafe { self.duplication.ReleaseFrame()? };
        Ok(())
    }
}
```

---

## 🎨 Layer 2: GPU Frame Pipeline

### Color Space Conversion: BGRA → NV12

This MUST happen on the GPU. NV12 is what all hardware encoders (NVENC, AMF, QSV) want.
Never do this on CPU — it's thousands of times slower.

**HLSL Compute Shader (shaders/color_convert.hlsl):**

```hlsl
// BGRA (from WGC) → NV12 (for encoders)
// BT.709 color space, limited range (16-235 for Y, 16-240 for UV)

Texture2D<float4> InputBGRA : register(t0);
RWTexture2D<uint> OutputY   : register(u0);   // Luma plane (full res)
RWTexture2D<uint2> OutputUV : register(u1);   // Chroma plane (half res)

[numthreads(16, 16, 1)]
void CSMain(uint3 id : SV_DispatchThreadID)
{
    float4 bgra = InputBGRA[id.xy];
    float b = bgra.b, g = bgra.g, r = bgra.r;

    // BT.709 full range → limited range
    float y  = 16.0  + (65.481 * r + 128.553 * g + 24.966 * b);
    float cb = 128.0 + (-37.797 * r - 74.203 * g + 112.0  * b);
    float cr = 128.0 + (112.0  * r - 93.786 * g - 18.214 * b);

    // Write Y plane (every pixel)
    OutputY[id.xy] = (uint)clamp(y, 16.0, 235.0);

    // Write UV plane (every 2x2 block, only one thread does it)
    if ((id.x % 2 == 0) && (id.y % 2 == 0)) {
        uint2 uvCoord = id.xy / 2;
        OutputUV[uvCoord] = uint2(
            (uint)clamp(cb, 16.0, 240.0),
            (uint)clamp(cr, 16.0, 240.0)
        );
    }
}
```

**Compile shaders at build time in build.rs:**

```rust
// build.rs
fn main() {
    // Compile HLSL at build time using fxc or dxc
    // Or embed pre-compiled .cso files
    println!("cargo:rerun-if-changed=shaders/color_convert.hlsl");
}
```

### GPU Texture Pool

Allocate a pool of NV12 textures upfront. Reuse them instead of allocating per frame.

```rust
// crates/pipeline/src/texture_pool.rs
use std::collections::VecDeque;
use windows::Win32::Graphics::Direct3D11::*;
use parking_lot::Mutex;

pub struct TexturePool {
    available: Mutex<VecDeque<ID3D11Texture2D>>,
}

impl TexturePool {
    pub fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        count: usize,
        format: DXGI_FORMAT,   // DXGI_FORMAT_NV12 for encoder input
        bind_flags: D3D11_BIND_FLAG,
    ) -> anyhow::Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flags.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32, // shared for NVENC
        };

        let mut pool = VecDeque::with_capacity(count);
        for _ in 0..count {
            let mut texture = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
            pool.push_back(texture.unwrap());
        }

        Ok(Self { available: Mutex::new(pool) })
    }

    pub fn acquire(&self) -> Option<ID3D11Texture2D> {
        self.available.lock().pop_front()
    }

    pub fn release(&self, texture: ID3D11Texture2D) {
        self.available.lock().push_back(texture);
    }
}
```

### Thread-Safe Frame Queue

```rust
// crates/pipeline/src/queue.rs
use crossbeam_channel::{bounded, Receiver, Sender};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

pub struct GpuFrame {
    pub texture: ID3D11Texture2D,
    pub timestamp_us: u64,
    pub width: u32,
    pub height: u32,
}

// The pipeline uses two queues:
// 1. Capture thread → Converter thread (BGRA frames)
// 2. Converter thread → Encoder thread (NV12 frames)
pub fn frame_channel(capacity: usize) -> (Sender<GpuFrame>, Receiver<GpuFrame>) {
    bounded(capacity)
}
```

---

## 🎬 Layer 3: Video Encoding

### Encoder Trait (all encoders implement this)

```rust
// crates/encoder/src/traits.rs
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub is_keyframe: bool,
    pub codec: VideoCodec,
}

pub enum VideoCodec { H264, H265, AV1 }

pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: GpuFrame) -> anyhow::Result<Option<EncodedPacket>>;
    fn flush(&mut self) -> anyhow::Result<Vec<EncodedPacket>>;
    fn codec(&self) -> VideoCodec;
}
```

### NVENC (NVIDIA Hardware Encoder)

NVENC SDK must be loaded dynamically at runtime.

```rust
// crates/encoder/src/nvenc.rs
// NVENC is accessed via C API — use bindgen or manual FFI

// Key NVENC concepts:
// 1. Open encode session tied to your D3D11 device
// 2. Register D3D11 textures directly with NVENC (no copy!)
// 3. Map registered resource → encode → get bitstream
// 4. Unmap and release

// NVENC recommended settings for screen capture:
// - Codec: H.264 (NV_ENC_CODEC_H264_GUID) or H.265
// - Preset: P4 (balanced) or P6 (higher quality, more GPU)
// - Rate control: CBR for streaming, VBR for recording
// - B-frames: 0 for low latency (screen capture)
// - Ref frames: 1-2 for low latency
// - Infinite GOP: for streaming; fixed GOP (e.g. 60) for recording
// - Look-ahead: disable for lowest latency

pub struct NvencConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    pub codec: NvencCodec,
    pub rate_control: NvencRateControl,
    pub low_latency: bool,     // disables B-frames, reduces buffering
    pub quality_preset: u8,   // 1 (fastest) to 7 (best quality)
}

pub enum NvencCodec { H264, H265 }
pub enum NvencRateControl { CBR, VBR, CQP(u8) }
```

### AMF (AMD Hardware Encoder)

```rust
// crates/encoder/src/amf.rs
// AMF SDK: github.com/GPUOpen-LibrariesAndSDKs/AMF
// Load AMF factory via LoadLibrary("amfrt64.dll") at runtime
// Key AMF properties for H.264:
// AMF_VIDEO_ENCODER_FRAMESIZE
// AMF_VIDEO_ENCODER_FRAMERATE
// AMF_VIDEO_ENCODER_TARGET_BITRATE
// AMF_VIDEO_ENCODER_PEAK_BITRATE
// AMF_VIDEO_ENCODER_USAGE → AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY (for streaming)
//                        → AMF_VIDEO_ENCODER_USAGE_TRANSCODING (for recording)
// AMF_VIDEO_ENCODER_QUALITY_PRESET → AMF_VIDEO_ENCODER_QUALITY_PRESET_SPEED
//                                 → AMF_VIDEO_ENCODER_QUALITY_PRESET_QUALITY
```

### Encoder Selection Logic

```rust
// crates/encoder/src/lib.rs
pub fn create_best_encoder(
    device: &ID3D11Device,
    config: &EncoderConfig,
) -> anyhow::Result<Box<dyn VideoEncoder>> {
    // Try NVENC first
    if let Ok(enc) = NvencEncoder::new(device, config) {
        tracing::info!("Using NVENC encoder");
        return Ok(Box::new(enc));
    }
    // Try AMF
    if let Ok(enc) = AmfEncoder::new(device, config) {
        tracing::info!("Using AMF encoder");
        return Ok(Box::new(enc));
    }
    // Try QuickSync
    if let Ok(enc) = QsvEncoder::new(device, config) {
        tracing::info!("Using QuickSync encoder");
        return Ok(Box::new(enc));
    }
    // Software fallback
    tracing::warn!("No hardware encoder found, using x264 (expect higher CPU usage)");
    Ok(Box::new(X264Encoder::new(config)?))
}
```

---

## 🔊 Layer 4: Audio Capture (WASAPI)

WASAPI loopback captures the final mixed system audio output — same mix going to speakers.
This is a kernel-level API. Browsers cannot access it. Native apps can.

```rust
// crates/audio/src/wasapi.rs
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::Audio::Endpoints::*;

pub struct WasapiLoopback {
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl WasapiLoopback {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            // 1. Get default audio render endpoint (speakers/headphones)
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(
                &MMDeviceEnumerator, None, CLSCTX_ALL
            )?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

            // 2. Activate IAudioClient
            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

            // 3. Get the mix format (what the system is currently mixing at)
            let mix_format = audio_client.GetMixFormat()?;

            // 4. Initialize in LOOPBACK mode — this is what makes it capture output
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,  // ← KEY: capture what's playing
                10_000_000,  // 1 second buffer (100ns units)
                0,
                mix_format,
                None,
            )?;

            let capture_client: IAudioCaptureClient =
                audio_client.GetService()?;
            audio_client.Start()?;

            let fmt = &*mix_format;
            Ok(Self {
                audio_client,
                capture_client,
                sample_rate: fmt.nSamplesPerSec,
                channels: fmt.nChannels,
                bits_per_sample: fmt.wBitsPerSample,
            })
        }
    }

    // Call this in your audio thread loop (~every 10ms)
    pub fn read_samples(&self) -> anyhow::Result<Vec<f32>> {
        let mut samples = Vec::new();
        unsafe {
            loop {
                let mut data_ptr = std::ptr::null_mut();
                let mut frames_available = 0u32;
                let mut flags = 0u32;

                match self.capture_client.GetBuffer(
                    &mut data_ptr,
                    &mut frames_available,
                    &mut flags,
                    None, None,
                ) {
                    Ok(()) => {
                        if frames_available == 0 { break; }
                        // Convert raw bytes to f32 samples
                        let byte_count = (frames_available * self.channels as u32 * 4) as usize;
                        let float_slice = std::slice::from_raw_parts(
                            data_ptr as *const f32,
                            byte_count / 4,
                        );
                        samples.extend_from_slice(float_slice);
                        self.capture_client.ReleaseBuffer(frames_available)?;
                    }
                    Err(_) => break,
                }
            }
        }
        Ok(samples)
    }
}
```

### Audio Encoding: Opus (for WebRTC/MyCord)

```rust
// crates/audio_encoder/src/opus.rs
// Use the `audiopus` crate or raw libopus bindings

// Opus settings for voice/screen audio:
// - Sample rate: 48000 Hz (required by Opus)
// - Channels: 2 (stereo)
// - Bitrate: 96–128 kbps for music/system audio
//            32–64 kbps for voice only
// - Application: VOIP (voice) or Audio (system audio/music)
// - Frame size: 20ms = 960 samples at 48kHz (standard)

pub struct OpusEncoder {
    encoder: audiopus::coder::Encoder,
    frame_size: usize,     // 960 for 20ms at 48kHz
    input_buffer: Vec<f32>,
}

impl OpusEncoder {
    pub fn new(bitrate_kbps: u32, for_voice: bool) -> anyhow::Result<Self> {
        use audiopus::*;
        let app = if for_voice { Application::Voip } else { Application::Audio };
        let mut encoder = coder::Encoder::new(
            SampleRate::Hz48000,
            Channels::Stereo,
            app,
        )?;
        encoder.set_bitrate(Bitrate::BitsPerSecond((bitrate_kbps * 1000) as i32))?;

        Ok(Self {
            encoder,
            frame_size: 960,  // 20ms at 48kHz
            input_buffer: Vec::new(),
        })
    }

    pub fn push_samples(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Vec<u8>>> {
        self.input_buffer.extend_from_slice(samples);
        let mut packets = Vec::new();
        
        while self.input_buffer.len() >= self.frame_size * 2 { // *2 for stereo
            let frame: Vec<f32> = self.input_buffer.drain(..self.frame_size * 2).collect();
            let mut output = vec![0u8; 4000];
            let len = self.encoder.encode_float(&frame, &mut output)?;
            output.truncate(len);
            packets.push(output);
        }
        Ok(packets)
    }
}
```

---

## 📤 Layer 5: Output Sinks

### Output Trait

```rust
// crates/output/src/traits.rs
pub trait OutputSink: Send {
    fn write_video_packet(&mut self, packet: &EncodedPacket) -> anyhow::Result<()>;
    fn write_audio_packet(&mut self, data: &[u8], timestamp_us: u64) -> anyhow::Result<()>;
    fn flush(&mut self) -> anyhow::Result<()>;
}
```

### File Output (MP4/MKV via FFmpeg)

```rust
// crates/output/src/file.rs
// Use ffmpeg-next crate for muxing
// Key steps:
// 1. avformat_alloc_output_context2 → AVFormatContext
// 2. avcodec_find_encoder(AV_CODEC_ID_H264) → add stream
// 3. avformat_write_header
// 4. Per packet: av_packet_from_data + av_write_frame
// 5. av_write_trailer on close

// For MP4: use "mp4" format string
// For MKV: use "matroska" format string
// MKV is better for recording (survives crashes, no moov atom issue)
```

### WebRTC Output (for MyCord integration)

```rust
// crates/output/src/webrtc.rs
// Use libdatachannel Rust bindings: github.com/wladwm/str0m
// OR connect via localhost WebSocket to the MyCord browser client
//
// Architecture option A (native WebRTC):
//   CaptureCore <--libdatachannel--> Browser (MyCord web UI)
//
// Architecture option B (localhost pipe — simpler):
//   CaptureCore → localhost:9001 (H.264 RTP) → Browser via WebCodecs API
//
// For MyCord Phase 1, use option B:
// - CaptureCore sends H.264 NAL units via localhost WebSocket
// - Browser receives them via WebSocket, decodes via WebCodecs API
// - Browser sends via WebRTC to remote peer as usual

pub struct LocalhostOutput {
    // Send encoded H.264 to browser via localhost WebSocket
    // Browser then feeds into WebRTC track via MediaStreamTrackGenerator (Insertable Streams API)
}
```

---

## 🔄 The Complete Pipeline (Threading Model)

```
Thread 1: Capture Thread
  WgcCapture::try_get_frame() in loop
  → Sends BGRA GpuFrame to channel [capacity: 3]

Thread 2: Convert Thread  
  Receives BGRA frame
  → Runs color_convert HLSL compute shader (GPU)
  → Acquires NV12 texture from TexturePool
  → Sends NV12 GpuFrame to channel [capacity: 3]

Thread 3: Video Encode Thread
  Receives NV12 frame
  → nvenc.encode(frame) / amf.encode(frame)
  → Returns EncodedPacket (H.264 NAL units)
  → Sends to Output channel

Thread 4: Audio Capture Thread
  WasapiLoopback::read_samples() every 10ms
  → opus_encoder.push_samples()
  → Sends Opus packets to Output channel

Thread 5: Output Thread
  Receives video + audio packets
  → Muxes to file (FFmpeg) OR sends via WebSocket to browser
```

**Rule:** Every channel has a bounded capacity (2-4). If the encoder can't keep up,
the capture thread drops frames rather than buffering forever. This is correct behavior.

---

## ⚙️ Config System

```rust
// crates/app/src/config.rs
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct CaptureConfig {
    pub target: CaptureTarget,
    pub video: VideoConfig,
    pub audio: AudioConfig,
    pub output: OutputConfig,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum CaptureTarget {
    Monitor { index: u32 },
    Window { title: String },
    WindowHandle { hwnd: u64 },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: f32,
    pub codec: String,        // "h264", "h265"
    pub encoder: String,      // "auto", "nvenc", "amf", "qsv", "x264"
    pub low_latency: bool,
    pub capture_cursor: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AudioConfig {
    pub enabled: bool,
    pub source: AudioSource,
    pub bitrate_kbps: u32,
    pub codec: String,        // "opus", "aac"
}

#[derive(Serialize, Deserialize, Clone)]
pub enum AudioSource {
    SystemOutput,   // WASAPI loopback — captures everything playing
    Microphone,     // WASAPI input
    Both,           // Mix of both
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OutputConfig {
    pub mode: OutputMode,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum OutputMode {
    File { path: String, container: String },  // "mp4", "mkv"
    Rtmp { url: String },
    Localhost { port: u16 },  // For MyCord browser integration
    WebRtc { signaling_url: String },
}
```

---

## 🚀 Build Phase Order (Start Here)

### Phase 1 — Proof of Concept (Week 1-2)
- [ ] Set up Cargo workspace
- [ ] Get D3D11 device created and working
- [ ] WGC capture working — frames arriving on a thread
- [ ] Dump raw frames as PNG files to verify capture works
- [ ] Basic `main.rs` that captures 10 frames and saves them

### Phase 2 — GPU Pipeline (Week 2-3)
- [ ] HLSL color convert shader compiling and running
- [ ] NV12 texture pool working
- [ ] BGRA frame successfully converted to NV12 on GPU
- [ ] Verify NV12 output is correct (dump to YUV file, open in YUView)

### Phase 3 — Encoding (Week 3-5)
- [ ] NVENC encoder initialized and encoding frames
- [ ] H.264 NAL units coming out
- [ ] Pipe NAL units into a .h264 file, verify it plays in VLC
- [ ] Add AMF encoder (same interface, different backend)
- [ ] Add x264 software fallback

### Phase 4 — Audio (Week 4-5)
- [ ] WASAPI loopback capture working
- [ ] Raw PCM samples verified (play them back as a .wav)
- [ ] Opus encoder working, packets coming out
- [ ] Audio + video synchronized by timestamp

### Phase 5 — File Output (Week 5-6)
- [ ] FFmpeg muxer: H.264 + Opus → MKV file
- [ ] Record a 30-second clip, verify quality and sync

### Phase 6 — MyCord Integration (Week 6-8)
- [ ] Localhost WebSocket output: stream H.264 NAL units to browser
- [ ] Browser receives via WebSocket, decodes via WebCodecs API
- [ ] Browser feeds decoded frames into WebRTC track using Insertable Streams
- [ ] Full end-to-end: CaptureCore → MyCord browser → remote peer

---

## 🌐 MyCord Integration Detail

This is how CaptureCore connects to the MyCord web app instead of Electron:

```
[CaptureCore .exe]                    [MyCord Browser Tab]
     │                                        │
     │  H.264 NAL units (binary)              │
     └────── ws://localhost:9001 ─────────────┘
                                              │
                                    WebCodecs VideoDecoder
                                              │
                                    MediaStreamTrackGenerator
                                              │
                                       WebRTC Track
                                              │
                                     Remote peer (other user)
```

**Browser side (JavaScript):**

```javascript
// Connect to CaptureCore local server
const ws = new WebSocket('ws://localhost:9001');
ws.binaryType = 'arraybuffer';

// Create a track generator — lets us push frames into a MediaStream
const generator = new MediaStreamTrackGenerator({ kind: 'video' });
const writer = generator.writable.getWriter();

// WebCodecs decoder
const decoder = new VideoDecoder({
  output: async (videoFrame) => {
    await writer.write(videoFrame);
    videoFrame.close();
  },
  error: (e) => console.error('Decode error:', e),
});

decoder.configure({
  codec: 'avc1.640028',  // H.264 High Profile
  optimizeForLatency: true,
});

ws.onmessage = (event) => {
  const nalUnit = new Uint8Array(event.data);
  const chunk = new EncodedVideoChunk({
    type: isKeyframe(nalUnit) ? 'key' : 'delta',
    timestamp: performance.now() * 1000,
    data: nalUnit,
  });
  decoder.decode(chunk);
};

// Use this track in WebRTC
const stream = new MediaStream([generator]);
const sender = peerConnection.addTrack(generator, stream);
```

---

## 🛠️ Key Windows APIs Reference

| API | Header/Crate | Used For |
|-----|-------------|---------|
| `Windows.Graphics.Capture` | `windows` crate (Graphics::Capture) | Screen/window capture |
| `ID3D11Device` / `ID3D11DeviceContext` | `windows` (Graphics::Direct3D11) | GPU device + commands |
| `IDXGIOutputDuplication` | `windows` (Graphics::Dxgi) | DXGI capture fallback |
| `IAudioClient` | `windows` (Media::Audio) | WASAPI audio init |
| `IAudioCaptureClient` | `windows` (Media::Audio) | WASAPI read audio |
| `AUDCLNT_STREAMFLAGS_LOOPBACK` | `windows` | System audio loopback |
| NVENC SDK | manual FFI / bindgen | NVIDIA H.264 encoding |
| AMF SDK | manual FFI / bindgen | AMD H.264 encoding |
| `avformat_*` | ffmpeg-next crate | MP4/MKV muxing |

---

## ⚠️ Common Pitfalls to Avoid

1. **Never copy frames to CPU RAM** unless doing software encoding. The entire video path
   should be GPU texture → GPU encoder → bitstream.

2. **WGC frames must be consumed quickly.** The frame pool only has 2 slots. If you hold
   a frame too long, new frames get dropped. Get the texture out, release the frame object.

3. **NVENC requires shared textures.** When creating your NV12 texture pool, set
   `D3D11_RESOURCE_MISC_SHARED` flag. NVENC needs to share textures across devices.

4. **WASAPI format is not always 44100 Hz.** Call `GetMixFormat()` and use whatever
   sample rate Windows is mixing at (usually 44100 or 48000). Resample to 48000 for Opus
   if needed.

5. **Audio and video timestamps must come from the same clock.** Use `QueryPerformanceCounter`
   for both and convert to microseconds. Don't use separate system clocks.

6. **H.264 NAL units need start codes** for some muxers. NVENC outputs them with 4-byte
   start codes (0x00 0x00 0x00 0x01). Some APIs want AVCC format (length-prefixed) instead.
   Know which format each consumer expects.

7. **NVENC is not available without a registered NVIDIA GPU.** Always runtime-check and
   fall through to AMF → QSV → x264.

8. **COM must be initialized** before using any Windows audio or COM-based API.
   Call `CoInitializeEx(None, COINIT_MULTITHREADED)` on every thread that uses COM.

---

## 📚 Resources

- **WGC C# samples** (easiest to understand the API):
  github.com/microsoft/Windows.UI.Composition-Win32-Samples
- **OBS source** (best real-world reference):
  github.com/obsproject/obs-studio — see `plugins/win-capture/` and `plugins/obs-nvenc/`
- **NVENC SDK samples**:
  developer.nvidia.com/nvidia-video-codec-sdk (requires free registration)
- **AMF SDK**:
  github.com/GPUOpen-LibrariesAndSDKs/AMF
- **windows-rs examples**:
  github.com/microsoft/windows-rs/tree/master/crates/samples
- **YUView** (verify NV12 output visually):
  github.com/IENT/YUView
- **WebCodecs explainer** (for browser integration):
  w3c.github.io/webcodecs/
- **Insertable Streams API** (feed frames into WebRTC):
  w3c.github.io/mediacapture-transform/

---

## 🔗 Relationship to MyCord

MyCord is the end-user application this pipeline will eventually power.
MyCord context:
- Two-person voice/video call and screen share web app
- Currently uses browser `getDisplayMedia()` — limited quality, no system audio on macOS/Firefox
- Signaling server: `wss://mycord-server.onrender.com` (Node.js + Socket.io)
- WebRTC P2P with TURN fallback
- Stack: React + TypeScript frontend, Node.js backend

CaptureCore replaces `getDisplayMedia()` on the sender side:
- CaptureCore runs as a lightweight tray app on the user's PC
- Captures screen via WGC (better quality, system audio support)
- Streams H.264 to MyCord browser tab via localhost WebSocket
- MyCord browser feeds it into WebRTC using WebCodecs + Insertable Streams
- Receiver side unchanged — still regular WebRTC video track

This gives MyCord OBS-level capture quality without requiring Electron.
