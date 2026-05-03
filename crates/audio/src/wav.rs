use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Context;

/// Minimal WAV (PCM16) file writer for debugging captured audio.
pub struct WavFileWriter {
    file: File,
    sample_rate: u32,
    channels: u16,
    data_bytes: u32,
}

impl WavFileWriter {
    pub fn create(path: impl AsRef<Path>, sample_rate: u32, channels: u16) -> anyhow::Result<Self> {
        anyhow::ensure!(sample_rate > 0, "sample_rate must be > 0");
        anyhow::ensure!(channels > 0, "channels must be > 0");

        let mut file = File::create(path.as_ref())
            .with_context(|| format!("create {}", path.as_ref().display()))?;
        // Placeholder 44-byte header; rewritten in finalize().
        file.write_all(&[0u8; 44]).context("write wav header placeholder")?;
        Ok(Self {
            file,
            sample_rate,
            channels,
            data_bytes: 0,
        })
    }

    pub fn write_f32_interleaved(&mut self, samples: &[f32]) -> anyhow::Result<()> {
        for &s in samples {
            let q = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            self.file
                .write_all(&q.to_le_bytes())
                .context("write wav pcm16 sample")?;
            self.data_bytes = self
                .data_bytes
                .saturating_add(2);
        }
        Ok(())
    }

    pub fn finalize(mut self) -> anyhow::Result<()> {
        self.file
            .seek(SeekFrom::Start(0))
            .context("seek wav header start")?;

        let bits_per_sample = 16u16;
        let block_align = self.channels * (bits_per_sample / 8);
        let byte_rate = self.sample_rate * u32::from(block_align);
        let riff_size = 36u32.saturating_add(self.data_bytes);

        self.file.write_all(b"RIFF").context("write RIFF")?;
        self.file
            .write_all(&riff_size.to_le_bytes())
            .context("write RIFF size")?;
        self.file.write_all(b"WAVE").context("write WAVE")?;
        self.file.write_all(b"fmt ").context("write fmt chunk id")?;
        self.file
            .write_all(&16u32.to_le_bytes())
            .context("write fmt chunk size")?;
        self.file
            .write_all(&1u16.to_le_bytes())
            .context("write PCM format tag")?;
        self.file
            .write_all(&self.channels.to_le_bytes())
            .context("write channels")?;
        self.file
            .write_all(&self.sample_rate.to_le_bytes())
            .context("write sample rate")?;
        self.file
            .write_all(&byte_rate.to_le_bytes())
            .context("write byte rate")?;
        self.file
            .write_all(&block_align.to_le_bytes())
            .context("write block align")?;
        self.file
            .write_all(&bits_per_sample.to_le_bytes())
            .context("write bits per sample")?;
        self.file.write_all(b"data").context("write data chunk id")?;
        self.file
            .write_all(&self.data_bytes.to_le_bytes())
            .context("write data size")?;
        self.file.flush().context("flush wav file")?;
        Ok(())
    }
}

