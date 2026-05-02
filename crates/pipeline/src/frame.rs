//! Shared dimensions for GPU frames (matches capture size).

/// Pixel dimensions of a full-resolution frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub fn chroma_size(&self) -> (u32, u32) {
        ((self.width + 1) / 2, (self.height + 1) / 2)
    }
}
