use anyhow::Context;
use windows::Win32::Foundation::S_FALSE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DESC,
    D3D11_QUERY_EVENT, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext,
    ID3D11Query, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8_SNORM, DXGI_FORMAT_R8G8_UINT, DXGI_FORMAT_R8G8_UNORM,
    DXGI_FORMAT_R8_SNORM, DXGI_FORMAT_R8_UINT, DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC,
};

fn bytes_per_pixel(format: DXGI_FORMAT) -> anyhow::Result<u32> {
    match format {
        DXGI_FORMAT_R8_UINT | DXGI_FORMAT_R8_UNORM | DXGI_FORMAT_R8_SNORM => Ok(1),
        DXGI_FORMAT_R8G8_UINT | DXGI_FORMAT_R8G8_UNORM | DXGI_FORMAT_R8G8_SNORM => Ok(2),
        _ => anyhow::bail!("unsupported texture format for CPU readback: {format:?}"),
    }
}

/// Wait until all GPU commands **before** the paired `End` complete (used after `CopyResource`).
fn wait_gpu_idle_after_copy(device: &ID3D11Device, context: &ID3D11DeviceContext) -> anyhow::Result<()> {
    let desc = D3D11_QUERY_DESC {
        Query: D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };
    let mut query: Option<ID3D11Query> = None;
    unsafe {
        device
            .CreateQuery(&desc, Some(&mut query))
            .map_err(|e| anyhow::anyhow!("CreateQuery(EVENT): {e}"))?;
    }
    let query = query.context("CreateQuery returned null")?;
    unsafe {
        context.End(&query);
        context.Flush();
    }

    loop {
        match unsafe { context.GetData(&query, None, 0, 0) } {
            Ok(()) => return Ok(()),
            Err(e) => {
                let c = e.code();
                if c == S_FALSE || c == DXGI_ERROR_WAS_STILL_DRAWING {
                    std::thread::yield_now();
                } else {
                    anyhow::bail!("GetData(EVENT) after readback copy: {e}");
                }
            }
        }
    }
}

/// Staging readback of an R8-family texture → one byte per pixel, tight rows.
pub fn copy_r8_texture_to_bytes(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { src.GetDesc(&mut desc) };
    anyhow::ensure!(
        desc.SampleDesc.Count == 1,
        "readback needs non-MSAA texture (sample count {})",
        desc.SampleDesc.Count
    );
    let bpp = bytes_per_pixel(desc.Format)?;
    copy_format_texture_to_bytes(device, context, src, desc.Format, bpp)
}

/// Staging readback of RG8-family → two bytes per pixel (interleaved chroma).
pub fn copy_rg8_uint_texture_to_bytes(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { src.GetDesc(&mut desc) };
    anyhow::ensure!(
        desc.SampleDesc.Count == 1,
        "readback needs non-MSAA texture (sample count {})",
        desc.SampleDesc.Count
    );
    let bpp = bytes_per_pixel(desc.Format)?;
    copy_format_texture_to_bytes(device, context, src, desc.Format, bpp)
}

fn copy_format_texture_to_bytes(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
    format: DXGI_FORMAT,
    bpp: u32,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { src.GetDesc(&mut desc) };
    let w = desc.Width;
    let h = desc.Height;

    let staging = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut staging_tex = None;
    unsafe {
        device
            .CreateTexture2D(&staging, None, Some(&mut staging_tex))
            .map_err(|e| {
                anyhow::anyhow!(
                    "CreateTexture2D staging readback {w}x{h} format={format:?}: {e}"
                )
            })?;
    }
    let staging_tex = staging_tex.context("staging null")?;

    unsafe {
        context.CopyResource(&staging_tex, src);
    }
    wait_gpu_idle_after_copy(device, context)?;

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(
                &staging_tex,
                0,
                D3D11_MAP_READ,
                0,
                Some(&mut mapped),
            )
            .map_err(|e| anyhow::anyhow!("Map staging texture for CPU read: {e}"))?;
    }

    let row_pitch = mapped.RowPitch as usize;
    let out_w = w * bpp;
    let mut out = vec![0u8; (out_w * h) as usize];
    for row in 0..(h as usize) {
        let src_row = unsafe { (mapped.pData as *const u8).add(row * row_pitch) };
        let dst = &mut out[(row * out_w as usize)..((row + 1) * out_w as usize)];
        unsafe {
            std::ptr::copy_nonoverlapping(src_row, dst.as_mut_ptr(), out_w as usize);
        }
    }

    unsafe {
        context.Unmap(&staging_tex, 0);
    }

    Ok((w, h, out))
}
