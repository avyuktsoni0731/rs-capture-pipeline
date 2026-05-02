use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameType, UsageType};
use openh264::formats::YUVBuffer;
use openh264::{OpenH264API, Timestamp};

use crate::annex_b::encoded_bitstream_to_annex_b;
use crate::traits::{EncodedPacket, VideoCodec, VideoEncoder};

/// OpenH264 software encoder (Phase 3 fallback until NVENC FFI lands).
pub struct OpenH264VideoEncoder {
    inner: Encoder,
    width: usize,
    height: usize,
}

impl OpenH264VideoEncoder {
    pub fn new(width: u32, height: u32, bitrate_bps: u32) -> anyhow::Result<Self> {
        let api = OpenH264API::from_source();
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .target_bitrate(BitRate::from_bps(bitrate_bps));

        let inner = Encoder::with_api_config(api, config).map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(Self {
            inner,
            width: width as usize,
            height: height as usize,
        })
    }
}

impl VideoEncoder for OpenH264VideoEncoder {
    fn encode_i420(&mut self, i420: &[u8], timestamp_us: u64) -> anyhow::Result<EncodedPacket> {
        let expected = 3 * self.width * self.height / 2;
        anyhow::ensure!(
            i420.len() >= expected,
            "I420 size {} < expected {}",
            i420.len(),
            expected
        );

        let yuv = YUVBuffer::from_vec(i420[..expected].to_vec(), self.width, self.height);

        let ts = Timestamp::from_millis(timestamp_us / 1000);
        let bs = self
            .inner
            .encode_at(&yuv, ts)
            .map_err(|e| anyhow::anyhow!("openh264 encode: {e}"))?;

        let data = encoded_bitstream_to_annex_b(&bs);
        let is_keyframe = matches!(
            bs.frame_type(),
            FrameType::IDR | FrameType::I
        );

        Ok(EncodedPacket {
            data,
            timestamp_us,
            is_keyframe,
            codec: VideoCodec::H264,
        })
    }

    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }
}
