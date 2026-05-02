//! Audio capture abstractions (WASAPI/mic implementations to be added).
mod wasapi;
mod wav;

/// Interleaved PCM audio chunk.
#[derive(Clone, Debug)]
pub struct PcmChunk {
    pub timestamp_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples_f32: Vec<f32>,
}

/// Audio capture source contract.
pub trait AudioCapture {
    /// Polls one chunk if available. Non-blocking implementations can return `Ok(None)`.
    fn try_read_chunk(&mut self) -> anyhow::Result<Option<PcmChunk>>;
}

pub use wasapi::WasapiLoopbackCapture;
pub use wav::WavFileWriter;

/// Temporary source that emits silence for plumbing/integration tests.
pub struct SilenceAudioCapture {
    sample_rate: u32,
    channels: u16,
    chunk_frames: u32,
    next_timestamp_us: u64,
}

impl SilenceAudioCapture {
    pub fn new(sample_rate: u32, channels: u16, chunk_frames: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(sample_rate > 0, "sample_rate must be > 0");
        anyhow::ensure!(channels > 0, "channels must be > 0");
        anyhow::ensure!(chunk_frames > 0, "chunk_frames must be > 0");
        Ok(Self {
            sample_rate,
            channels,
            chunk_frames,
            next_timestamp_us: 0,
        })
    }
}

impl AudioCapture for SilenceAudioCapture {
    fn try_read_chunk(&mut self) -> anyhow::Result<Option<PcmChunk>> {
        let sample_count = self.chunk_frames as usize * self.channels as usize;
        let chunk = PcmChunk {
            timestamp_us: self.next_timestamp_us,
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples_f32: vec![0.0; sample_count],
        };
        self.next_timestamp_us +=
            (u64::from(self.chunk_frames) * 1_000_000) / u64::from(self.sample_rate);
        Ok(Some(chunk))
    }
}
