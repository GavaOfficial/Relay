#![cfg(windows)]
#![allow(clippy::missing_safety_doc)]
use anyhow::{ensure, Context, Result};
use relay_hook_protocol::{
    ipc::{Channel, Handle},
    BGRA8, BGRX8, RGBA8,
};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};
static EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn wire_format(format: DXGI_FORMAT) -> Result<(DXGI_FORMAT, u32)> {
    Ok(match format {
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            (DXGI_FORMAT_B8G8R8A8_UNORM, BGRA8)
        }
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            (DXGI_FORMAT_R8G8B8A8_UNORM, RGBA8)
        }
        DXGI_FORMAT_B8G8R8X8_UNORM | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB => {
            (DXGI_FORMAT_B8G8R8X8_UNORM, BGRX8)
        }
        _ => anyhow::bail!("formato hook non supportato: {}", format.0),
    })
}
pub unsafe fn device(luid: u64) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    for index in 0.. {
        let Ok(adapter) = factory.EnumAdapters1(index) else {
            break;
        };
        let id = adapter.GetDesc1()?.AdapterLuid;
        if ((id.HighPart as u32 as u64) << 32) | id.LowPart as u64 != luid {
            continue;
        }
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        return Ok((device.context("device")?, context.context("context")?));
    }
    anyhow::bail!("adattatore recorder non trovato")
}
pub unsafe fn check_adapter(device: &ID3D11Device, luid: u64) -> Result<()> {
    let d: IDXGIDevice = device.cast()?;
    let id = d.GetAdapter()?.GetDesc()?.AdapterLuid;
    ensure!(
        ((id.HighPart as u32 as u64) << 32) | id.LowPart as u64 == luid,
        "adattatore diverso dal recorder"
    );
    Ok(())
}
pub struct Sender {
    pub texture: ID3D11Texture2D,
    pub mutex: IDXGIKeyedMutex,
    pub handle: Handle,
    pub desc: D3D11_TEXTURE2D_DESC,
    epoch: u64,
    format: u32,
}
impl Sender {
    pub unsafe fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<Self> {
        ensure!(
            width > 0 && height > 0 && width <= 8192 && height <= 8192,
            "dimensioni hook"
        );
        let (format, wire) = wire_format(format)?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
            ..Default::default()
        };
        let mut texture = None;
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        let texture = texture.context("texture condivisa")?;
        let resource: IDXGIResource1 = texture.cast()?;
        let handle = Handle::checked(
            resource
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    PCWSTR::null(),
                )?
                .0,
        )?;
        Ok(Self {
            mutex: texture.cast()?,
            texture,
            handle,
            desc,
            epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
            format: wire,
        })
    }
    pub unsafe fn acquire(&self) -> bool {
        (self.mutex.vtable().AcquireSync)(self.mutex.as_raw(), 0, 0).0 == 0
            || (self.mutex.vtable().AcquireSync)(self.mutex.as_raw(), 1, 0).0 == 0
    }
    pub fn publish(&self, channel: &Channel, api: u32) -> bool {
        channel.publish_gpu(
            self.desc.Width,
            self.desc.Height,
            self.handle.0 as u64,
            self.epoch,
            api,
            self.format,
        )
    }
    pub unsafe fn copy(
        &self,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        channel: &Channel,
        api: u32,
    ) -> Result<bool> {
        if !self.acquire() {
            return Ok(false);
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut desc);
        if desc.SampleDesc.Count > 1 {
            context.ResolveSubresource(&self.texture, 0, source, 0, self.desc.Format)
        } else {
            context.CopyResource(&self.texture, source)
        }
        context.Flush();
        let sent = self.publish(channel, api);
        self.mutex.ReleaseSync(if sent { 1 } else { 0 })?;
        Ok(sent)
    }
}
