//! GPU frame pipeline: BGRA→NV12 compute, texture pools, queues (later).

mod color_convert;
mod frame;
mod queue;
mod readback;
mod texture_pool;

pub use color_convert::BgraToNv12Converter;
pub use frame::FrameSize;
pub use queue::{stage_channel, StageRx, StageTx, Timed};
pub use readback::{copy_r8_texture_to_bytes, copy_rg8_uint_texture_to_bytes};
pub use texture_pool::{Nv12Targets, TexturePool};
