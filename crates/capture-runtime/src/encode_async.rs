//! Optional **async NVENC** path: capture copies BGRA into ping-pong textures; a worker thread runs
//! `encode_bgra_texture` so the next WGC pull can proceed while NVENC finishes.

use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context;
use crossbeam_channel::{bounded, Receiver, Sender};
use encoder::{EncodedPacket, VideoEncoder};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// One frame for the encode worker (`slot` is 0 or 1 into the ping textures).
pub struct VideoEncodeJob {
    pub slot: u8,
    pub ts_us: u64,
}

pub struct NvencAsync {
    pub job_tx: Sender<VideoEncodeJob>,
    pub pkt_rx: Receiver<anyhow::Result<EncodedPacket>>,
    join: JoinHandle<()>,
    pub pings: Arc<[ID3D11Texture2D; 2]>,
    pub slot_next: u8,
}

impl NvencAsync {
    pub fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        encoder: Box<dyn VideoEncoder>,
        queue_depth: usize,
    ) -> anyhow::Result<Self> {
        let arr = create_nvenc_ping_textures(device, width, height)?;
        let pings = Arc::new(arr);
        let pw = Arc::clone(&pings);
        let (job_tx, job_rx) = bounded::<VideoEncodeJob>(queue_depth);
        let (pkt_tx, pkt_rx) = bounded::<anyhow::Result<EncodedPacket>>(queue_depth);

        let join = std::thread::Builder::new()
            .name("nvenc-encode".to_string())
            .spawn(move || encode_worker_loop(encoder, pw, job_rx, pkt_tx))
            .context("spawn nvenc-encode")?;

        Ok(Self {
            job_tx,
            pkt_rx,
            join,
            pings,
            slot_next: 0,
        })
    }

    pub fn shutdown(self) -> anyhow::Result<()> {
        drop(self.job_tx);
        self.join
            .join()
            .map_err(|_| anyhow::anyhow!("nvenc encode worker panicked"))?;
        Ok(())
    }
}

fn encode_worker_loop(
    mut encoder: Box<dyn VideoEncoder>,
    pings: Arc<[ID3D11Texture2D; 2]>,
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

fn create_nvenc_ping_textures(
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
        CPUAccessFlags: 0,
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
