use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use anyhow::{Context, Result};
use windows::{
    core::{IInspectable, Interface, Ref},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, Common::DXGI_SAMPLE_DESC, IDXGIDevice},
            Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
};

use super::{device::Gpu, window::Handle, Unsync};

#[derive(Default)]
pub struct Latest {
    pub texture: Option<Unsync<ID3D11Texture2D>>,
    pub size: (u32, u32),
    pub frames: u64,
    pub last: Option<Instant>,
}

pub struct Source {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pub latest: Arc<Mutex<Latest>>,
    closed: Arc<AtomicBool>,
}

pub fn supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

fn direct3d(gpu: &Gpu) -> Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = gpu.device.cast()?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?;
    Ok(inspectable.cast()?)
}

impl Source {
    pub fn window(gpu: &Gpu, window: Handle) -> Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window.hwnd()) }
            .context("la finestra del gioco non si puo' catturare")?;
        Self::start(gpu, item, Some(window))
    }

    pub fn monitor(gpu: &Gpu, monitor: isize) -> Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(HMONITOR(monitor as _)) }
            .context("lo schermo non si puo' catturare")?;
        Self::start(gpu, item, None)
    }

    fn start(gpu: &Gpu, item: GraphicsCaptureItem, window: Option<Handle>) -> Result<Self> {
        let d3d = direct3d(gpu)?;
        let size = item.Size()?;
        let format = DirectXPixelFormat::B8G8R8A8UIntNormalized;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, format, 2, size)?;
        let d3d = Unsync(d3d);
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);

        let latest = Arc::new(Mutex::new(Latest::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let pool_size = Arc::new(Mutex::new(size));
        let gpu_ref = Unsync(gpu.clone());
        let latest_ref = latest.clone();
        pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                move |sender: Ref<Direct3D11CaptureFramePool>, _| {
                    let (d3d, gpu_ref) = (&d3d, &gpu_ref);
                    let pool = sender.ok()?;
                    let frame = pool.TryGetNextFrame()?;
                    let content = frame.ContentSize()?;
                    let surface = frame.Surface()?;
                    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe { texture.GetDesc(&mut desc) };
                    let visible = (
                        (content.Width.max(1) as u32).min(desc.Width),
                        (content.Height.max(1) as u32).min(desc.Height),
                    );
                    let area = match window {
                        Some(w) => super::window::capture_box(w, visible),
                        None => Some(crate::layout::Rect {
                            x: 0,
                            y: 0,
                            width: visible.0,
                            height: visible.1,
                        }),
                    };
                    if let Some(area) = area {
                        copy_frame(&gpu_ref.0, &latest_ref, &texture, area);
                    }
                    let _ = frame.Close();
                    let mut current = pool_size.lock().unwrap();
                    if content.Width > 0
                        && content.Height > 0
                        && (content.Width != current.Width || content.Height != current.Height)
                    {
                        let fresh = SizeInt32 {
                            Width: content.Width,
                            Height: content.Height,
                        };
                        pool.Recreate(&d3d.0, format, 2, fresh)?;
                        *current = fresh;
                    }
                    Ok(())
                },
            ),
        )?;
        let closed_ref = closed.clone();
        item.Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                closed_ref.store(true, Ordering::Relaxed);
                Ok(())
            }),
        )?;
        session.StartCapture()?;
        Ok(Self {
            pool,
            session,
            latest,
            closed,
        })
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn frames(&self) -> u64 {
        self.latest.lock().unwrap().frames
    }

    pub fn last_frame(&self) -> Option<Instant> {
        self.latest.lock().unwrap().last
    }

    pub fn frame(&self) -> Option<(ID3D11Texture2D, (u32, u32))> {
        let l = self.latest.lock().unwrap();
        l.texture.as_ref().map(|t| (t.0.clone(), l.size))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn copy_frame(
    gpu: &Gpu,
    latest: &Mutex<Latest>,
    texture: &ID3D11Texture2D,
    area: crate::layout::Rect,
) {
    let mut l = latest.lock().unwrap();
    let size = (area.width, area.height);
    if l.texture.is_none() || l.size != size {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut fresh = None;
        if unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut fresh)) }.is_err() {
            return;
        }
        let Some(fresh) = fresh else {
            return;
        };
        l.texture = Some(Unsync(fresh));
        l.size = size;
    }
    let target = &l.texture.as_ref().unwrap().0;
    let region = D3D11_BOX {
        left: area.x,
        top: area.y,
        front: 0,
        right: area.x + area.width,
        bottom: area.y + area.height,
        back: 1,
    };
    unsafe {
        gpu.context
            .CopySubresourceRegion(target, 0, 0, 0, 0, texture, 0, Some(&region));
    }
    l.frames += 1;
    l.last = Some(Instant::now());
}
