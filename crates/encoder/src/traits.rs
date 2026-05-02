/// Target codec (NVENC/AMF wire later; OpenH264 returns [`VideoCodec::H264`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
}

/// One encoded slice (Annex-B NAL aggregation for our OpenH264 path).
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub is_keyframe: bool,
    pub codec: VideoCodec,
}

/// Encoder configuration (subset of `CURSOR_CONTEXT.md` — extend as needed).
#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

impl EncoderConfig {
    pub fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Self {
        Self {
            width,
            height,
            fps,
            bitrate_bps,
        }
    }
}

/// Encode one I420 4:2:0 frame (`3/2 * width * height` bytes, planar Y/U/V).
pub trait VideoEncoder: Send {
    fn encode_i420(
        &mut self,
        i420: &[u8],
        timestamp_us: u64,
    ) -> anyhow::Result<EncodedPacket>;

    fn codec(&self) -> VideoCodec;
}
