use std::collections::HashMap;

use anyhow::Context;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_TEX2D_SRV, D3D11_TEX2D_UAV,
    D3D11_TEXTURE2D_DESC, D3D11_UNORDERED_ACCESS_VIEW_DESC, D3D11_USAGE_DEFAULT,
    D3D11_UAV_DIMENSION_TEXTURE2D, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8_UINT,
    DXGI_FORMAT_R8_UINT,
};

const CS_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/color_convert.cso"));

/// BGRA texture → R8 Y + R8G8 UV (NV12-style) via compute.
///
/// WGC frame textures often lack `D3D11_BIND_SHADER_RESOURCE` and may carry `SHARED` /
/// `SHARED_KEYEDMUTEX` misc flags, so [`Self::convert`] copies into an internal BGRA texture
/// before creating an SRV (matches how capture stacks avoid `CreateShaderResourceView` failures).
pub struct BgraToNv12Converter {
    cs: ID3D11ComputeShader,
    bgra_copy: Option<ID3D11Texture2D>,
    bgra_srv: Option<ID3D11ShaderResourceView>,
    cached_w: u32,
    cached_h: u32,
    /// `(ID3D11Texture2D* for Y, ID3D11Texture2D* for UV)` → UAVs reused across frames (pool ping-pong).
    y_uv_uavs: HashMap<(usize, usize), (ID3D11UnorderedAccessView, ID3D11UnorderedAccessView)>,
}

impl BgraToNv12Converter {
    /// Internal BGRA copy after [`Self::convert`] (same dimensions as the last converted frame).
    /// Used for NVENC registered-resource encode without CPU readback.
    #[must_use]
    pub fn bgra_copy_texture(&self) -> Option<&ID3D11Texture2D> {
        self.bgra_copy.as_ref()
    }

    fn ensure_bgra_srv(&mut self, device: &ID3D11Device) -> anyhow::Result<&ID3D11ShaderResourceView> {
        if self.bgra_srv.is_some() {
            return Ok(self.bgra_srv.as_ref().unwrap());
        }
        let bgra_copy = self
            .bgra_copy
            .as_ref()
            .context("BGRA copy texture (internal)")?;
        let mut bgra_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { bgra_copy.GetDesc(&mut bgra_desc) };
        let srv_format = if bgra_desc.Format == DXGI_FORMAT_B8G8R8A8_TYPELESS {
            DXGI_FORMAT_B8G8R8A8_UNORM
        } else {
            bgra_desc.Format
        };
        let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
        d.Format = srv_format;
        d.ViewDimension = D3D_SRV_DIMENSION_TEXTURE2D;
        d.Anonymous.Texture2D = D3D11_TEX2D_SRV {
            MostDetailedMip: 0,
            MipLevels: 1,
        };
        let mut srv = None;
        unsafe {
            device
                .CreateShaderResourceView(bgra_copy, Some(&d), Some(&mut srv))
                .ok()
                .context("CreateShaderResourceView BGRA (on internal copy)")?;
        }
        self.bgra_srv = Some(srv.context("SRV null")?);
        Ok(self.bgra_srv.as_ref().unwrap())
    }

    pub fn new(device: &ID3D11Device) -> anyhow::Result<Self> {
        let mut cs = None;
        unsafe {
            device
                .CreateComputeShader(CS_BLOB, None, Some(&mut cs))
                .ok()
                .context("CreateComputeShader (embed must be fxc/DXBC cs_5_0; see pipeline/build.rs)")?;
        }
        let cs = cs.context("compute shader null")?;
        Ok(Self {
            cs,
            bgra_copy: None,
            bgra_srv: None,
            cached_w: 0,
            cached_h: 0,
            y_uv_uavs: HashMap::new(),
        })
    }

    /// Copies WGC `bgra` into an internal BGRA texture. If `out_nv12` is [`Some`], runs the
    /// compute shader to fill `out_y` / `out_uv` (NV12-style). If [`None`], skips NV12 (OBS-style
    /// path when encoding BGRA with NVENC).
    pub fn convert(
        &mut self,
        context: &ID3D11DeviceContext,
        device: &ID3D11Device,
        bgra: &ID3D11Texture2D,
        out_nv12: Option<(&ID3D11Texture2D, &ID3D11Texture2D)>,
    ) -> anyhow::Result<()> {
        let mut bgra_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { bgra.GetDesc(&mut bgra_desc) };

        unsafe {
            device
                .GetDeviceRemovedReason()
                .ok()
                .context("D3D11 device removed/suspended (check NVENC teardown / driver)")?;
            // NVENC drop + ping-pong pool textures: reset bindings so UAV/SRV creates are not
            // fighting stale compute state from the previous frame.
            context.ClearState();
        }

        if bgra_desc.Width != self.cached_w || bgra_desc.Height != self.cached_h {
            self.bgra_srv = None;
            self.bgra_copy = None;
            self.y_uv_uavs.clear();
            self.cached_w = bgra_desc.Width;
            self.cached_h = bgra_desc.Height;

            let mut copy_desc = bgra_desc;
            copy_desc.Usage = D3D11_USAGE_DEFAULT;
            copy_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            copy_desc.CPUAccessFlags = 0;
            copy_desc.MiscFlags = 0;

            let mut copy_tex = None;
            unsafe {
                device
                    .CreateTexture2D(&copy_desc, None, Some(&mut copy_tex))
                    .ok()
                    .context("CreateTexture2D BGRA copy (SRV-capable)")?;
            }
            self.bgra_copy = Some(copy_tex.context("BGRA copy texture null")?);
        }

        let bgra_copy = self.bgra_copy.as_ref().context("BGRA copy texture (internal)")?;

        unsafe {
            context.CopyResource(bgra_copy, bgra);
        }

        if out_nv12.is_none() {
            return Ok(());
        }
        let (out_y, out_uv) = out_nv12.expect("checked");

        let srv = self.ensure_bgra_srv(device)?;

        let y_key = out_y.as_raw() as usize;
        let uv_key = out_uv.as_raw() as usize;
        let (uav_y, uav_uv) = if let Some((u, v)) = self.y_uv_uavs.get(&(y_key, uv_key)) {
            (u.clone(), v.clone())
        } else {
            let uav_y = {
                let mut d = D3D11_UNORDERED_ACCESS_VIEW_DESC::default();
                d.Format = DXGI_FORMAT_R8_UINT;
                d.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
                d.Anonymous.Texture2D = D3D11_TEX2D_UAV { MipSlice: 0 };
                let mut uav = None;
                unsafe {
                    device
                        .CreateUnorderedAccessView(out_y, Some(&d), Some(&mut uav))
                        .ok()
                        .context("CreateUnorderedAccessView Y")?;
                }
                uav.context("UAV Y null")?
            };
            let uav_uv = {
                let mut d = D3D11_UNORDERED_ACCESS_VIEW_DESC::default();
                d.Format = DXGI_FORMAT_R8G8_UINT;
                d.ViewDimension = D3D11_UAV_DIMENSION_TEXTURE2D;
                d.Anonymous.Texture2D = D3D11_TEX2D_UAV { MipSlice: 0 };
                let mut uav = None;
                unsafe {
                    device
                        .CreateUnorderedAccessView(out_uv, Some(&d), Some(&mut uav))
                        .ok()
                        .context("CreateUnorderedAccessView UV")?;
                }
                uav.context("UAV UV null")?
            };
            self.y_uv_uavs
                .insert((y_key, uv_key), (uav_y.clone(), uav_uv.clone()));
            (uav_y, uav_uv)
        };

        let gx = (bgra_desc.Width + 15) / 16;
        let gy = (bgra_desc.Height + 15) / 16;

        unsafe {
            context.CSSetShader(Some(&self.cs), None);
            context.CSSetShaderResources(0, Some(&[Some(srv.clone())]));
            let uavs = [Some(uav_y), Some(uav_uv)];
            context.CSSetUnorderedAccessViews(0, 2, Some(uavs.as_ptr()), None);
            context.Dispatch(gx, gy, 1);

            context.CSSetShader(None, None);
            context.CSSetShaderResources(0, Some(&[None::<ID3D11ShaderResourceView>]));
            let clear_uav = [
                None::<ID3D11UnorderedAccessView>,
                None::<ID3D11UnorderedAccessView>,
            ];
            context.CSSetUnorderedAccessViews(0, 2, Some(clear_uav.as_ptr()), None);
        }

        Ok(())
    }
}
