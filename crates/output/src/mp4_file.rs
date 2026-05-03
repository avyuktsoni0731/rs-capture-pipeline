use std::fs::File;
use std::path::Path;
use std::str::FromStr;

use anyhow::Context;
use bytes::Bytes;
use mp4::{
    AacConfig, AudioObjectType, AvcConfig, ChannelConfig, MediaConfig, Mp4Config, Mp4Sample,
    Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
};

use crate::annexb::{nal_type, nal_units, nal_units_to_avcc_sample};

/// Writes one H.264 Annex-B video track to a progressive MP4 (`avc1`).
pub struct Mp4H264File {
    writer: Mp4Writer<File>,
    width: u16,
    height: u16,
    timescale: u32,
    frame_duration: u32,
    next_start_time: u64,
    next_track_id: u32,
    video_track_id: Option<u32>,
    audio_track_id: Option<u32>,
    audio_timescale: Option<u32>,
    audio_next_start_time: u64,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl Mp4H264File {
    pub fn create(path: impl AsRef<Path>, width: u16, height: u16, fps: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(fps > 0, "fps must be > 0");
        let timescale = 30_000u32;
        let frame_duration = std::cmp::max(1, timescale / fps);

        let file = File::create(path.as_ref()).with_context(|| format!("create {}", path.as_ref().display()))?;

        let config = Mp4Config {
            major_brand: FromStr::from_str("isom").unwrap(),
            minor_version: 512,
            compatible_brands: vec![
                FromStr::from_str("isom").unwrap(),
                FromStr::from_str("iso2").unwrap(),
                FromStr::from_str("avc1").unwrap(),
                FromStr::from_str("mp41").unwrap(),
            ],
            timescale,
        };

        let writer = Mp4Writer::write_start(file, &config).context("Mp4Writer::write_start")?;

        Ok(Self {
            writer,
            width,
            height,
            timescale,
            frame_duration,
            next_start_time: 0,
            next_track_id: 1,
            video_track_id: None,
            audio_track_id: None,
            audio_timescale: None,
            audio_next_start_time: 0,
            sps: Vec::new(),
            pps: Vec::new(),
        })
    }

    /// Enables one optional AAC audio track.
    ///
    /// `aac_frame_samples` is typically 1024 for AAC-LC.
    pub fn enable_aac(
        &mut self,
        sample_rate: u32,
        channels: u16,
        bitrate_bps: u32,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.audio_track_id.is_none(), "AAC track already enabled");
        let freq_index = sample_rate_to_index(sample_rate)?;
        let chan_conf = channels_to_config(channels)?;
        let cfg = AacConfig {
            bitrate: bitrate_bps,
            profile: AudioObjectType::AacLowComplexity,
            freq_index,
            chan_conf,
        };
        let track = TrackConfig {
            track_type: TrackType::Audio,
            timescale: sample_rate,
            language: "und".into(),
            media_conf: MediaConfig::AacConfig(cfg),
        };
        self.writer.add_track(&track).context("add AAC track")?;
        self.audio_track_id = Some(self.next_track_id);
        self.next_track_id += 1;
        self.audio_timescale = Some(sample_rate);
        Ok(())
    }

    /// Track timescale for the video track (same as [`Mp4H264File::create`] movie timescale).
    pub fn video_timescale(&self) -> u32 {
        self.timescale
    }

    /// One encoded frame (`Annex-B` with start codes). `is_keyframe` marks sync samples for STSS.
    ///
    /// Uses a fixed per-sample duration derived from `fps` passed to [`Self::create`].
    pub fn write_annex_b_frame(&mut self, annex_b: &[u8], is_keyframe: bool) -> anyhow::Result<()> {
        self.write_annex_b_frame_with_duration(annex_b, is_keyframe, self.frame_duration)
    }

    /// Same as [`Self::write_annex_b_frame`], but `duration_ts` is the sample duration in **video
    /// track timescale** units (see [`Self::video_timescale`], typically 30000 ticks per second).
    pub fn write_annex_b_frame_with_duration(
        &mut self,
        annex_b: &[u8],
        is_keyframe: bool,
        duration_ts: u32,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(duration_ts > 0, "video sample duration must be > 0");
        let nals = nal_units(annex_b);
        for nal in &nals {
            match nal_type(nal.as_slice()) {
                Some(7) if self.sps.is_empty() => self.sps = nal.clone(),
                Some(8) if self.pps.is_empty() => self.pps = nal.clone(),
                _ => {}
            }
        }

        if self.video_track_id.is_none() {
            anyhow::ensure!(
                !self.sps.is_empty() && !self.pps.is_empty(),
                "need SPS/PPS NALs before first video sample"
            );
            let avc = AvcConfig {
                width: self.width,
                height: self.height,
                seq_param_set: self.sps.clone(),
                pic_param_set: self.pps.clone(),
            };
            let track = TrackConfig {
                track_type: TrackType::Video,
                timescale: self.timescale,
                language: "und".into(),
                media_conf: MediaConfig::AvcConfig(avc),
            };
            self.writer.add_track(&track).context("add_track")?;
            self.video_track_id = Some(self.next_track_id);
            self.next_track_id += 1;
        }

        let mut vcl: Vec<Vec<u8>> = Vec::new();
        for nal in nals {
            let t = nal_type(&nal).unwrap_or(0);
            if t == 1 || t == 5 {
                vcl.push(nal);
            }
        }
        anyhow::ensure!(!vcl.is_empty(), "frame had no VCL NALs (types 1/5)");

        let payload = nal_units_to_avcc_sample(&vcl);
        let sample = Mp4Sample {
            start_time: self.next_start_time,
            duration: duration_ts,
            rendering_offset: 0,
            is_sync: is_keyframe,
            bytes: Bytes::from(payload),
        };
        self.next_start_time = self
            .next_start_time
            .saturating_add(u64::from(duration_ts));

        self.writer
            .write_sample(self.video_track_id.unwrap_or(1), &sample)
            .context("write_sample")?;
        Ok(())
    }

    /// Writes one raw AAC access unit payload (no ADTS header bytes).
    ///
    /// For AAC-LC this is usually one AU per 1024 PCM samples.
    pub fn write_aac_access_unit(
        &mut self,
        aac_payload: &[u8],
        samples_per_access_unit: u32,
    ) -> anyhow::Result<()> {
        let track_id = self
            .audio_track_id
            .context("AAC track is not enabled; call enable_aac first")?;
        anyhow::ensure!(!aac_payload.is_empty(), "AAC payload must not be empty");
        anyhow::ensure!(
            samples_per_access_unit > 0,
            "samples_per_access_unit must be > 0"
        );

        let sample = Mp4Sample {
            start_time: self.audio_next_start_time,
            duration: samples_per_access_unit,
            rendering_offset: 0,
            is_sync: true,
            bytes: Bytes::copy_from_slice(aac_payload),
        };
        self.audio_next_start_time = self
            .audio_next_start_time
            .saturating_add(u64::from(samples_per_access_unit));
        self.writer
            .write_sample(track_id, &sample)
            .context("write AAC sample")?;
        Ok(())
    }

    pub fn finish(mut self) -> anyhow::Result<()> {
        self.writer.write_end().context("Mp4Writer::write_end")?;
        Ok(())
    }
}

fn sample_rate_to_index(sample_rate: u32) -> anyhow::Result<SampleFreqIndex> {
    match sample_rate {
        96_000 => Ok(SampleFreqIndex::Freq96000),
        88_200 => Ok(SampleFreqIndex::Freq88200),
        64_000 => Ok(SampleFreqIndex::Freq64000),
        48_000 => Ok(SampleFreqIndex::Freq48000),
        44_100 => Ok(SampleFreqIndex::Freq44100),
        32_000 => Ok(SampleFreqIndex::Freq32000),
        24_000 => Ok(SampleFreqIndex::Freq24000),
        22_050 => Ok(SampleFreqIndex::Freq22050),
        16_000 => Ok(SampleFreqIndex::Freq16000),
        12_000 => Ok(SampleFreqIndex::Freq12000),
        11_025 => Ok(SampleFreqIndex::Freq11025),
        8_000 => Ok(SampleFreqIndex::Freq8000),
        7_350 => Ok(SampleFreqIndex::Freq7350),
        _ => anyhow::bail!("unsupported AAC sample rate for mp4 crate: {sample_rate}"),
    }
}

fn channels_to_config(channels: u16) -> anyhow::Result<ChannelConfig> {
    match channels {
        1 => Ok(ChannelConfig::Mono),
        2 => Ok(ChannelConfig::Stereo),
        3 => Ok(ChannelConfig::Three),
        4 => Ok(ChannelConfig::Four),
        5 => Ok(ChannelConfig::Five),
        6 => Ok(ChannelConfig::FiveOne),
        8 => Ok(ChannelConfig::SevenOne),
        _ => anyhow::bail!("unsupported AAC channel count for mp4 crate: {channels}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{channels_to_config, sample_rate_to_index};
    use mp4::{ChannelConfig, SampleFreqIndex};

    #[test]
    fn maps_sample_rate_to_aac_index() {
        assert_eq!(
            sample_rate_to_index(48_000).expect("48 kHz must map"),
            SampleFreqIndex::Freq48000
        );
        assert!(sample_rate_to_index(50_000).is_err());
    }

    #[test]
    fn maps_channels_to_aac_config() {
        assert_eq!(
            channels_to_config(2).expect("stereo must map"),
            ChannelConfig::Stereo
        );
        assert!(channels_to_config(7).is_err());
    }
}
