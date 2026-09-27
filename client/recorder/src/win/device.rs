use anyhow::{Context, Result};
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HMODULE, LUID},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0,
                D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
            },
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                D3D11_SDK_VERSION,
            },
            Dxgi::{
                CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
                DXGI_ADAPTER_FLAG_SOFTWARE,
            },
        },
        Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager},
    },
};

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

#[derive(Clone)]
pub struct Adapter {
    pub adapter: IDXGIAdapter1,
    pub luid: LUID,
    pub vendor: u32,
    pub name: String,
}

pub fn adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    let mut i = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        i += 1;
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        out.push(Adapter {
            adapter,
            luid: desc.AdapterLuid,
            vendor: desc.VendorId,
            name: String::from_utf16_lossy(&desc.Description[..len]),
        });
    }
    out
}

#[derive(Clone)]
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub manager: IMFDXGIDeviceManager,
}

pub fn create(adapter: Option<&Adapter>) -> Result<Gpu> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let dxgi: Option<IDXGIAdapter> = adapter.and_then(|a| a.adapter.cast().ok());
    let driver = if dxgi.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            dxgi.as_ref(),
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .context("creazione del dispositivo Direct3D 11")?;
    let device = device.context("Direct3D 11 senza dispositivo")?;
    let context = context.context("Direct3D 11 senza contesto")?;
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    let mut token = 0u32;
    let mut manager = None;
    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }
        .context("creazione del gestore DXGI di Media Foundation")?;
    let manager = manager.context("gestore DXGI mancante")?;
    unsafe { manager.ResetDevice(&device, token) }.context("collegamento del gestore DXGI")?;
    Ok(Gpu {
        device,
        context,
        manager,
    })
}
