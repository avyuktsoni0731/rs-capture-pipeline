use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

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

/// Encoder configuration width/height/fps/bitrate (see workspace `CURSOR_CONTEXT.md`).
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

    /// NVENC path: feed the capture/internal BGRA texture registered with the encoder (OBS-style).
    fn supports_bgra_gpu_encode(&self) -> bool {
        false
    }

    fn encode_bgra_texture(
        &mut self,
        _tex: &ID3D11Texture2D,
        _timestamp_us: u64,
    ) -> anyhow::Result<EncodedPacket> {
        anyhow::bail!("GPU BGRA encoding not supported for this encoder")
    }
}
