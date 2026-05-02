use anyhow::Context;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8_UINT, DXGI_FORMAT_R8_UINT, DXGI_SAMPLE_DESC};

/// Staging readback of an R8 texture → one byte per pixel, tight rows.
pub fn copy_r8_texture_to_bytes(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    copy_format_texture_to_bytes(device, context, src, DXGI_FORMAT_R8_UINT, 1)
}

/// Staging readback of RG8_UINT → two bytes per pixel (interleaved U,V).
pub fn copy_rg8_uint_texture_to_bytes(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    copy_format_texture_to_bytes(device, context, src, DXGI_FORMAT_R8G8_UINT, 2)
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
            .ok()
            .context("CreateTexture2D readback staging")?;
    }
    let staging_tex = staging_tex.context("staging null")?;

    unsafe {
        context.CopyResource(&staging_tex, src);
        // Ensure the copy reaches the staging texture before CPU Map (same-queue ordering is not
        // always enough under GPU load / concurrent NVENC on some drivers).
        context.Flush();
    }

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
