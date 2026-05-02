//! Windows screen capture (WGC primary, DXGI fallback later).

mod d3d11;
mod dxgi;
mod monitor;
mod wgc;

pub use d3d11::{copy_texture_to_rgba, create_d3d11_device, D3d11Context};
pub use monitor::{default_display_id, list_display_ids};
pub use wgc::WgcSession;

pub mod dxgi_pub {
    pub use crate::dxgi::*;
}
