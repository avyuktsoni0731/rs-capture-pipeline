use anyhow::Context;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D, D3D11CreateDevice, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

/// D3D11 device + immediate context.
pub struct D3d11Context {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

/// Create a hardware D3D11 device with BGRA support (required for WGC frame format).
pub fn create_d3d11_device() -> anyhow::Result<D3d11Context> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let mut fl = D3D_FEATURE_LEVEL::default();
    let levels = [D3D_FEATURE_LEVEL_11_0];
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut fl),
            Some(&mut context),
        )
        .ok()
        .context("D3D11CreateDevice")?;
    }
    let device = device.context("D3D11 device null")?;
    let context = context.context("D3D11 context null")?;
    Ok(D3d11Context { device, context })
}

/// Copy a BGRA texture to a CPU buffer (RGBA8, tight rows) for debugging / PNG export.
pub fn copy_texture_to_rgba(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
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
        Format: desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut staging_tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&staging, None, Some(&mut staging_tex))
            .ok()
            .context("CreateTexture2D staging")?;
    }
    let staging_tex = staging_tex.context("staging texture null")?;

    unsafe {
        context.CopyResource(&staging_tex, src);
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
            .ok()
            .context("Map staging texture")?;
    }

    let row_pitch = mapped.RowPitch as usize;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let height = h as usize;
    let width_bytes = (w * 4) as usize;

    for row in 0..height {
        let src_row = unsafe {
            (mapped.pData as *const u8).add(row * row_pitch)
        };
        let dst_row = &mut rgba[row * width_bytes..(row + 1) * width_bytes];
        for col in 0..(w as usize) {
            let o = col * 4;
            // BGRA -> RGBA
            unsafe {
                let b = *src_row.add(o);
                let g = *src_row.add(o + 1);
                let r = *src_row.add(o + 2);
                let a = *src_row.add(o + 3);
                dst_row[o] = r;
                dst_row[o + 1] = g;
                dst_row[o + 2] = b;
                dst_row[o + 3] = a;
            }
        }
    }

    unsafe {
        context.Unmap(&staging_tex, 0);
    }

    Ok((w, h, rgba))
}
