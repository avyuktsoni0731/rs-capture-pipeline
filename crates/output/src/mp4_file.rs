use std::fs::File;
use std::path::Path;
use std::str::FromStr;

use anyhow::Context;
use bytes::Bytes;
use mp4::{
    AvcConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType,
};

use crate::annexb::{nal_type, nal_units, nal_units_to_avcc_sample};

/// Writes one H.264 Annex-B video track to a progressive MP4 (`avc1`).
pub struct Mp4H264File {
    writer: Mp4Writer<File>,
    width: u16,
    height: u16,
    timescale: u32,
    frame_duration: u32,
    track_added: bool,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl Mp4H264File {
    pub fn create(path: impl AsRef<Path>, width: u16, height: u16, fps: u32) -> anyhow::Result<Self> {
        anyhow::ensure!(fps > 0, "fps must be > 0");
        let timescale = 30_000u32;
        let frame_duration = timescale / fps;

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
            track_added: false,
            sps: Vec::new(),
            pps: Vec::new(),
        })
    }

    /// One encoded frame (`Annex-B` with start codes). `is_keyframe` marks sync samples for STSS.
    pub fn write_annex_b_frame(&mut self, annex_b: &[u8], is_keyframe: bool) -> anyhow::Result<()> {
        let nals = nal_units(annex_b);
        for nal in &nals {
            match nal_type(nal.as_slice()) {
                Some(7) if self.sps.is_empty() => self.sps = nal.clone(),
                Some(8) if self.pps.is_empty() => self.pps = nal.clone(),
                _ => {}
            }
        }

        if !self.track_added {
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
            self.track_added = true;
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
            start_time: 0,
            duration: self.frame_duration,
            rendering_offset: 0,
            is_sync: is_keyframe,
            bytes: Bytes::from(payload),
        };

        self.writer
            .write_sample(1, &sample)
            .context("write_sample")?;
        Ok(())
    }

    pub fn finish(mut self) -> anyhow::Result<()> {
        self.writer.write_end().context("Mp4Writer::write_end")?;
        Ok(())
    }
}
