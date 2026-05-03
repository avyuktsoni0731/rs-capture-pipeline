//! Types crossing the process boundary to **consumers** (file muxer, WebSocket, WebRTC bridge).
//!
//! Delivery semantics come from [`crate::StreamBackpressure`] on [`crate::PipelineParams`] when using stream outputs.

/// Monotonic clock for correlating video and audio (microseconds from capture start / first frame).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct StreamClock;

/// One video access unit: Annex-B H.264 (start codes), as produced today by NVENC / OpenH264.
#[derive(Clone, Debug)]
pub struct VideoPacket {
    /// Annex-B byte stream (may contain multiple NALs).
    pub annex_b: Vec<u8>,
    /// Encoder timeline, microseconds (same family as today’s `ts_us`).
    pub timestamp_us: u64,
    pub is_keyframe: bool,
}

/// Audio payload for streaming hosts. File recording may use PCM → AAC internally instead.
#[derive(Clone, Debug)]
pub enum AudioChunk {
    /// Interleaved `f32` PCM (e.g. WASAPI loopback), `samples.len() == frames * channels`.
    PcmF32Interleaved {
        sample_rate: u32,
        channels: u16,
        timestamp_us: u64,
        samples: Vec<f32>,
    },
    /// Raw AAC-LC access unit (no ADTS), 1024 samples per channel per AU at typical rates.
    AacRaw {
        sample_rate: u32,
        channels: u16,
        timestamp_us: u64,
        payload: Vec<u8>,
    },
    /// Opus packet (RFC 6716), 48 kHz decode timeline; 20 ms frames (960 samples/channel) from our encoder.
    OpusPacket {
        channels: u16,
        timestamp_us: u64,
        payload: Vec<u8>,
    },
}
