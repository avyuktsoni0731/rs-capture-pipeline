//! Audio encoder abstractions (AAC/Opus implementations plug in here).

mod mf_aac;
mod opus;

pub use mf_aac::MfAacLcEncoder;
pub use opus::{OpusEncoder, OPUS_SAMPLES_PER_CHANNEL_FRAME_48K};

use audio::PcmChunk;

#[derive(Clone, Debug)]
pub struct EncodedAudioPacket {
    pub timestamp_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub data: Vec<u8>,
}

pub trait AudioEncoder: Send {
    fn encode_chunk(&mut self, chunk: &PcmChunk) -> anyhow::Result<Vec<EncodedAudioPacket>>;
}

/// Placeholder encoder: no compression, for pipeline plumbing only.
///
/// This is intentionally *not* a real AAC encoder. It allows us to wire queue/timing
/// and swap in a real encoder backend without touching call sites.
pub struct PcmPassthroughEncoder;

impl AudioEncoder for PcmPassthroughEncoder {
    fn encode_chunk(&mut self, chunk: &PcmChunk) -> anyhow::Result<Vec<EncodedAudioPacket>> {
        let mut bytes = Vec::with_capacity(chunk.samples_f32.len() * 4);
        for s in &chunk.samples_f32 {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        Ok(vec![EncodedAudioPacket {
            timestamp_us: chunk.timestamp_us,
            sample_rate: chunk.sample_rate,
            channels: chunk.channels,
            data: bytes,
        }])
    }
}
