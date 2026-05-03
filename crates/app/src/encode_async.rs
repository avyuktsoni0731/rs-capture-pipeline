//! Optional **async NVENC** path: capture copies BGRA into ping-pong textures; a worker thread runs
//! `encode_bgra_texture` so the next WGC frame can be pulled while NVENC finishes.

use std::thread::JoinHandle;

use anyhow::Context;
use crossbeam_channel::{bounded, Receiver, Sender};
use encoder::{EncodedPacket, VideoEncoder};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_CPU_ACCESS_NONE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};

/// One frame for the encode worker (`slot` is 0 or 1 into the ping textures).
pub struct VideoEncodeJob {
    pub slot: u8,
    pub ts_us: u64,
}

fn encode_worker_loop(
    mut encoder: Box<dyn VideoEncoder>,
    pings: [ID3D11Texture2D; 2],
    job_rx: Receiver<VideoEncodeJob>,
    pkt_tx: Sender<anyhow::Result<EncodedPacket>>,
) {
    while let Ok(job) = job_rx.recv() {
        let tex = &pings[job.slot as usize];
        let r = encoder.encode_bgra_texture(tex, job.ts_us);
        if pkt_tx.send(r).is_err() {
            break;
        }
    }
}

/// Ping-pong BGRA textures (same layout as the converter internal copy).
pub fn create_nvenc_ping_textures(
    device: &ID3D11Device,
    w: u32,
    h: u32,
) -> anyhow::Result<[ID3D11Texture2D; 2]> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: DXGI_CPU_ACCESS_NONE.0 as u32,
        MiscFlags: 0,
    };
    let mut a = None;
    let mut b = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut a))
            .ok()
            .context("CreateTexture2D NVENC ping A")?;
        device
            .CreateTexture2D(&desc, None, Some(&mut b))
            .ok()
            .context("CreateTexture2D NVENC ping B")?;
    }
    Ok([a.context("ping A null")?, b.context("ping B null")?])
}

pub fn spawn_nvenc_worker(
    encoder: Box<dyn VideoEncoder>,
    pings: [ID3D11Texture2D; 2],
    job_cap: usize,
) -> anyhow::Result<(
    Sender<VideoEncodeJob>,
    Receiver<anyhow::Result<EncodedPacket>>,
    JoinHandle<()>,
)> {
    let (job_tx, job_rx) = bounded::<VideoEncodeJob>(job_cap);
    let (pkt_tx, pkt_rx) = bounded::<anyhow::Result<EncodedPacket>>(job_cap);

    let join = std::thread::Builder::new()
        .name("nvenc-encode".to_string())
        .spawn(move || encode_worker_loop(encoder, pings, job_rx, pkt_tx))
        .context("spawn nvenc-encode")?;

    Ok((job_tx, pkt_rx, join))
}

pub fn copy_bgra_to_ping(
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
    dst: &ID3D11Texture2D,
) -> anyhow::Result<()> {
    unsafe {
        context.CopyResource(dst, src);
        context.Flush();
    }
    Ok(())
}
