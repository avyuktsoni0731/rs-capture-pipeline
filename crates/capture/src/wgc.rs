use anyhow::Context;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::DisplayId;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};

/// Active WGC session for one [`GraphicsCaptureItem`].
pub struct WgcSession {
    #[allow(dead_code)]
    pub item: GraphicsCaptureItem,
    pub frame_pool: Direct3D11CaptureFramePool,
    #[allow(dead_code)]
    pub session: GraphicsCaptureSession,
    _rt_device: IDirect3DDevice,
}

impl WgcSession {
    /// Capture the given display (see [`crate::monitor::default_display_id`]).
    pub fn new_for_display(d3d: &ID3D11Device, display_id: DisplayId) -> anyhow::Result<Self> {
        let item = GraphicsCaptureItem::TryCreateFromDisplayId(display_id)
            .context("GraphicsCaptureItem::TryCreateFromDisplayId")?;

        let dxgi_device: IDXGIDevice = d3d.cast().context("ID3D11Device -> IDXGIDevice")?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
            .context("CreateDirect3D11DeviceFromDXGIDevice")?;
        let rt_device: IDirect3DDevice = inspectable
            .cast()
            .context("IInspectable -> IDirect3DDevice")?;

        let size = item.Size().context("GraphicsCaptureItem::Size")?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &rt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .context("Direct3D11CaptureFramePool::CreateFreeThreaded")?;

        let session = frame_pool
            .CreateCaptureSession(&item)
            .context("CreateCaptureSession")?;

        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);

        session
            .StartCapture()
            .context("GraphicsCaptureSession::StartCapture")?;

        Ok(Self {
            item,
            frame_pool,
            session,
            _rt_device: rt_device,
        })
    }

    /// Non-blocking; returns an error if no frame is ready yet.
    pub fn try_next_frame(&self) -> windows::core::Result<Direct3D11CaptureFrame> {
        self.frame_pool.TryGetNextFrame()
    }
}

/// WinRT surface → D3D11 texture for the captured frame.
pub fn frame_to_texture(frame: &Direct3D11CaptureFrame) -> anyhow::Result<ID3D11Texture2D> {
    let surface = frame.Surface().context("Direct3D11CaptureFrame::Surface")?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .context("IDirect3DSurface -> IDirect3DDxgiInterfaceAccess")?;
    unsafe { access.GetInterface() }.context("IDirect3DDxgiInterfaceAccess::GetInterface")
}
