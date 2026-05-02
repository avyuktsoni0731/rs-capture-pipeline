use anyhow::Context;
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_TEX2D_SRV, D3D11_TEX2D_UAV, D3D11_UNORDERED_ACCESS_VIEW_DESC,
    D3D11_UAV_DIMENSION_TEXTURE2D, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8_UINT, DXGI_FORMAT_R8_UINT,
};

const CS_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/color_convert.cso"));

/// BGRA texture → R8 Y + R8G8 UV (NV12-style) via compute.
pub struct BgraToNv12Converter {
    cs: ID3D11ComputeShader,
}

impl BgraToNv12Converter {
    pub fn new(device: &ID3D11Device) -> anyhow::Result<Self> {
        let mut cs = None;
        unsafe {
            device
                .CreateComputeShader(CS_BLOB, None, Some(&mut cs))
                .ok()
                .context("CreateComputeShader color_convert.cso")?;
        }
        let cs = cs.context("compute shader null")?;
        Ok(Self { cs })
    }

    /// Runs the compute pass: `bgra` must match dimensions of `out_y` / `out_uv`.
    pub fn convert(
        &self,
        context: &ID3D11DeviceContext,
        device: &ID3D11Device,
        bgra: &ID3D11Texture2D,
        out_y: &ID3D11Texture2D,
        out_uv: &ID3D11Texture2D,
    ) -> anyhow::Result<()> {
        let mut bgra_desc = Default::default();
        unsafe { bgra.GetDesc(&mut bgra_desc) };

        let srv: ID3D11ShaderResourceView = {
            let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: unsafe { std::mem::zeroed() },
            };
            unsafe {
                d.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                };
            }
            let mut srv = None;
            unsafe {
                device
                    .CreateShaderResourceView(bgra, Some(&d), Some(&mut srv))
                    .ok()
                    .context("CreateShaderResourceView BGRA")?;
            }
            srv.context("SRV null")?
        };

        let uav_y: ID3D11UnorderedAccessView = {
            let mut d = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R8_UINT,
                ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                Anonymous: unsafe { std::mem::zeroed() },
            };
            unsafe {
                d.Anonymous.Texture2D = D3D11_TEX2D_UAV { MipSlice: 0 };
            }
            let mut uav = None;
            unsafe {
                device
                    .CreateUnorderedAccessView(out_y, Some(&d), Some(&mut uav))
                    .ok()
                    .context("CreateUnorderedAccessView Y")?;
            }
            uav.context("UAV Y null")?
        };

        let uav_uv: ID3D11UnorderedAccessView = {
            let mut d = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8_UINT,
                ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                Anonymous: unsafe { std::mem::zeroed() },
            };
            unsafe {
                d.Anonymous.Texture2D = D3D11_TEX2D_UAV { MipSlice: 0 };
            }
            let mut uav = None;
            unsafe {
                device
                    .CreateUnorderedAccessView(out_uv, Some(&d), Some(&mut uav))
                    .ok()
                    .context("CreateUnorderedAccessView UV")?;
            }
            uav.context("UAV UV null")?
        };

        let gx = (bgra_desc.Width + 15) / 16;
        let gy = (bgra_desc.Height + 15) / 16;

        unsafe {
            context.CSSetShader(Some(&self.cs), None);
            context.CSSetShaderResources(0, Some(&[Some(srv)]));
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
