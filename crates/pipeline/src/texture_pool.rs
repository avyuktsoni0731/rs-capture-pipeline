use anyhow::Context;
use parking_lot::Mutex;
use std::collections::VecDeque;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8_UINT, DXGI_FORMAT_R8_UINT, DXGI_SAMPLE_DESC};

use crate::frame::FrameSize;

/// Y (R8) + interleaved UV (RG8) textures matching NV12 plane layout (encoder wiring comes later).
pub struct Nv12Targets {
    pub y: ID3D11Texture2D,
    pub uv: ID3D11Texture2D,
}

/// Pool of reusable NV12 plane pairs at fixed resolution.
pub struct TexturePool {
    free: Mutex<VecDeque<Nv12Targets>>,
    size: FrameSize,
}

impl TexturePool {
    pub fn new(device: &ID3D11Device, size: FrameSize, count: usize) -> anyhow::Result<Self> {
        let bind =
            D3D11_BIND_UNORDERED_ACCESS.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32;

        let mut free = VecDeque::with_capacity(count);
        for _ in 0..count {
            let y_desc = D3D11_TEXTURE2D_DESC {
                Width: size.width,
                Height: size.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8_UINT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let (uw, uh) = size.chroma_size();
            let uv_desc = D3D11_TEXTURE2D_DESC {
                Width: uw,
                Height: uh,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8_UINT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let mut y_tex = None;
            let mut uv_tex = None;
            unsafe {
                device
                    .CreateTexture2D(&y_desc, None, Some(&mut y_tex))
                    .ok()
                    .context("CreateTexture2D Y")?;
                device
                    .CreateTexture2D(&uv_desc, None, Some(&mut uv_tex))
                    .ok()
                    .context("CreateTexture2D UV")?;
            }
            free.push_back(Nv12Targets {
                y: y_tex.context("Y null")?,
                uv: uv_tex.context("UV null")?,
            });
        }

        Ok(Self {
            free: Mutex::new(free),
            size,
        })
    }

    pub fn size(&self) -> FrameSize {
        self.size
    }

    pub fn acquire(&self) -> Option<Nv12Targets> {
        self.free.lock().pop_front()
    }

    pub fn release(&self, targets: Nv12Targets) {
        self.free.lock().push_back(targets);
    }
}
