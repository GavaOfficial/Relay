use anyhow::{Context, Result};
use minhook::MinHook;
use relay_hook_gpu::Sender;
use relay_hook_protocol::{ipc::Channel, D3D9};
use std::{
    cell::Cell,
    ffi::c_void,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, HRESULT},
    Win32::{
        Foundation::{HANDLE, HWND, RECT},
        Graphics::{Direct3D11::*, Direct3D9::*, Dxgi::Common::*},
    },
};
type Raw = *mut c_void;
static PRESENT: AtomicUsize = AtomicUsize::new(0);
static EX: AtomicUsize = AtomicUsize::new(0);
static RESET: AtomicUsize = AtomicUsize::new(0);
static RESETEX: AtomicUsize = AtomicUsize::new(0);
static SWAP: AtomicUsize = AtomicUsize::new(0);
static TARGETS: OnceLock<Vec<usize>> = OnceLock::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
thread_local! {static INSIDE:Cell<bool>=const{Cell::new(false)};}
struct State {
    channel: Channel,
    config: relay_hook_protocol::Header,
    due: Instant,
    capture: Option<Capture>,
}
struct Slot {
    surface: IDirect3DSurface9,
    import: ID3D11Texture2D,
    query9: IDirect3DQuery9,
    query11: ID3D11Query,
    stage: u8,
    serial: u64,
}
struct Capture {
    device: usize,
    width: u32,
    height: u32,
    format: D3DFORMAT,
    context: ID3D11DeviceContext,
    sender: Sender,
    slots: Vec<Slot>,
    serial: u64,
}
unsafe impl Send for Capture {}
unsafe fn capture(device: &IDirect3DDevice9, source: &IDirect3DSurface9, hwnd: HWND) -> Result<()> {
    let Ok(mut guard) = STATE.try_lock() else {
        return Ok(());
    };
    let Some(s) = guard.as_mut() else {
        return Ok(());
    };
    let now = Instant::now();
    if now < s.due {
        return Ok(());
    }
    if !s.channel.alive() {
        s.capture = None;
        return Ok(());
    }
    if !s.channel.accepts_window(hwnd.0 as u64) {
        return Ok(());
    }
    let mut desc = D3DSURFACE_DESC::default();
    source.GetDesc(&mut desc)?;
    if s.capture.as_ref().is_none_or(|c| {
        c.device != device.as_raw() as usize
            || c.width != desc.Width
            || c.height != desc.Height
            || c.format != desc.Format
    }) {
        let _: IDirect3DDevice9Ex = device.cast()?;
        let mut creation = D3DDEVICE_CREATION_PARAMETERS::default();
        device.GetCreationParameters(&mut creation)?;
        anyhow::ensure!(
            creation.BehaviorFlags & D3DCREATE_MULTITHREADED as u32 != 0,
            "D3D9 device non multithread"
        );
        let format = match desc.Format {
            D3DFMT_A8R8G8B8 => DXGI_FORMAT_B8G8R8A8_UNORM,
            D3DFMT_X8R8G8B8 => DXGI_FORMAT_B8G8R8X8_UNORM,
            _ => anyhow::bail!("formato D3D9"),
        };
        let (d11, context) = relay_hook_gpu::device(s.config.adapter_luid)?;
        let sender = Sender::new(&d11, desc.Width, desc.Height, format)?;
        let mut slots = Vec::new();
        for _ in 0..3 {
            let mut handle = HANDLE::default();
            let mut texture = None;
            device.CreateTexture(
                desc.Width,
                desc.Height,
                1,
                D3DUSAGE_RENDERTARGET as u32,
                desc.Format,
                D3DPOOL_DEFAULT,
                &mut texture,
                &mut handle,
            )?;
            let texture = texture.context("D3D9 shared texture")?;
            let surface = texture.GetSurfaceLevel(0)?;
            let mut import = None;
            d11.OpenSharedResource(handle, &mut import)?;
            let mut query11 = None;
            d11.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query11),
            )?;
            slots.push(Slot {
                surface,
                import: import.context("D3D9 import")?,
                query9: device.CreateQuery(D3DQUERYTYPE_EVENT)?,
                query11: query11.context("D3D11 query")?,
                stage: 0,
                serial: 0,
            });
        }
        s.capture = Some(Capture {
            device: device.as_raw() as usize,
            width: desc.Width,
            height: desc.Height,
            format: desc.Format,
            context,
            sender,
            slots,
            serial: 0,
        });
    }
    let c = s.capture.as_mut().unwrap();
    let mut order = [0, 1, 2];
    order.sort_by_key(|i| c.slots[*i].serial);
    for index in order {
        let slot = &mut c.slots[index];
        let mut done = 0u32;
        if slot.stage == 2 {
            let status = (c.context.vtable().GetData)(
                c.context.as_raw(),
                slot.query11.as_raw(),
                (&mut done as *mut u32).cast(),
                4,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            );
            if status.0 == 0 && done != 0 {
                slot.stage = 0
            }
        } else if slot.stage == 1 {
            let status = (slot.query9.vtable().GetData)(
                slot.query9.as_raw(),
                (&mut done as *mut u32).cast(),
                4,
                0,
            );
            if status.0 != 0 || done == 0 {
                break;
            }
            {
                c.sender.copy(&c.context, &slot.import, &s.channel, D3D9)?;
                c.context.End(&slot.query11);
                c.context.Flush();
                slot.stage = 2;
            }
        }
    }
    if now < s.due {
        return Ok(());
    }
    let interval = Duration::from_secs_f64(1.0 / s.config.fps.clamp(1, 120) as f64);
    s.due += interval;
    if s.due <= now {
        s.due = now + interval
    }
    if let Some(slot) = c.slots.iter_mut().find(|s| s.stage == 0) {
        device.StretchRect(
            source,
            std::ptr::null(),
            &slot.surface,
            std::ptr::null(),
            D3DTEXF_NONE,
        )?;
        slot.query9.Issue(D3DISSUE_END)?;
        c.serial += 1;
        slot.serial = c.serial;
        slot.stage = 1;
    }
    Ok(())
}
unsafe fn before(raw: Raw, override_window: HWND) {
    let _ = std::panic::catch_unwind(|| {
        let result = (|| -> Result<()> {
            let device = IDirect3DDevice9::from_raw_borrowed(&raw).context("D3D9 device")?;
            let mut params = D3DDEVICE_CREATION_PARAMETERS::default();
            device.GetCreationParameters(&mut params)?;
            let source = device.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
            capture(
                device,
                &source,
                if override_window.0.is_null() {
                    params.hFocusWindow
                } else {
                    override_window
                },
            )
        })();
        if let Err(e) = result {
            if std::env::var_os("RELAY_CAPTURE_DIAGNOSTICS").is_some() {
                eprintln!("Relay D3D9: {e:#}");
            }
            stop()
        }
    });
}
fn stop() {
    if let Ok(s) = STATE.try_lock() {
        if let Some(s) = s.as_ref() {
            s.channel.stop()
        }
    }
}
fn clear() {
    if let Ok(mut s) = STATE.lock() {
        if let Some(s) = s.as_mut() {
            s.capture = None
        }
    }
}
unsafe extern "system" fn present(
    raw: Raw,
    a: *const RECT,
    b: *const RECT,
    h: HWND,
    r: Raw,
) -> HRESULT {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        before(raw, h)
    }
    let f: unsafe extern "system" fn(Raw, *const RECT, *const RECT, HWND, Raw) -> HRESULT =
        std::mem::transmute(PRESENT.load(Ordering::Acquire));
    let result = f(raw, a, b, h, r);
    INSIDE.with(|v| v.set(nested));
    result
}
unsafe extern "system" fn present_ex(
    raw: Raw,
    a: *const RECT,
    b: *const RECT,
    h: HWND,
    r: Raw,
    flags: u32,
) -> HRESULT {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        before(raw, h)
    }
    let f: unsafe extern "system" fn(Raw, *const RECT, *const RECT, HWND, Raw, u32) -> HRESULT =
        std::mem::transmute(EX.load(Ordering::Acquire));
    let result = f(raw, a, b, h, r, flags);
    INSIDE.with(|v| v.set(nested));
    result
}
unsafe extern "system" fn swap(
    raw: Raw,
    a: *const RECT,
    b: *const RECT,
    h: HWND,
    r: Raw,
    flags: u32,
) -> HRESULT {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        let _ = std::panic::catch_unwind(|| {
            let result = (|| -> Result<()> {
                let chain = IDirect3DSwapChain9::from_raw_borrowed(&raw).context("swap")?;
                let mut params = D3DPRESENT_PARAMETERS::default();
                chain.GetPresentParameters(&mut params)?;
                capture(
                    &chain.GetDevice()?,
                    &chain.GetBackBuffer(0, D3DBACKBUFFER_TYPE_MONO)?,
                    if h.0.is_null() {
                        params.hDeviceWindow
                    } else {
                        h
                    },
                )
            })();
            if let Err(e) = result {
                if std::env::var_os("RELAY_CAPTURE_DIAGNOSTICS").is_some() {
                    eprintln!("Relay D3D9: {e:#}");
                }
                stop()
            }
        });
    }
    let f: unsafe extern "system" fn(Raw, *const RECT, *const RECT, HWND, Raw, u32) -> HRESULT =
        std::mem::transmute(SWAP.load(Ordering::Acquire));
    let result = f(raw, a, b, h, r, flags);
    INSIDE.with(|v| v.set(nested));
    result
}
unsafe extern "system" fn reset(raw: Raw, p: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    clear();
    let f: unsafe extern "system" fn(Raw, *mut D3DPRESENT_PARAMETERS) -> HRESULT =
        std::mem::transmute(RESET.load(Ordering::Acquire));
    f(raw, p)
}
unsafe extern "system" fn reset_ex(
    raw: Raw,
    p: *mut D3DPRESENT_PARAMETERS,
    m: *mut D3DDISPLAYMODEEX,
) -> HRESULT {
    clear();
    let f: unsafe extern "system" fn(
        Raw,
        *mut D3DPRESENT_PARAMETERS,
        *mut D3DDISPLAYMODEEX,
    ) -> HRESULT = std::mem::transmute(RESETEX.load(Ordering::Acquire));
    f(raw, p, m)
}
unsafe fn discover() -> Result<Vec<usize>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = CreateWindowExW(
        0,
        relay_hook_protocol::ipc::wide("STATIC").as_ptr(),
        std::ptr::null(),
        0,
        0,
        0,
        2,
        2,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    anyhow::ensure!(!hwnd.is_null(), "probe window");
    let result = (|| -> Result<Vec<usize>> {
        let d3d = Direct3DCreate9Ex(D3D_SDK_VERSION)?;
        let mut pp = D3DPRESENT_PARAMETERS {
            BackBufferWidth: 2,
            BackBufferHeight: 2,
            BackBufferFormat: D3DFMT_A8R8G8B8,
            BackBufferCount: 1,
            SwapEffect: D3DSWAPEFFECT_DISCARD,
            hDeviceWindow: HWND(hwnd),
            Windowed: true.into(),
            ..Default::default()
        };
        let mut device = None;
        d3d.CreateDeviceEx(
            0,
            D3DDEVTYPE_HAL,
            HWND(hwnd),
            D3DCREATE_HARDWARE_VERTEXPROCESSING as u32,
            &mut pp,
            std::ptr::null_mut(),
            &mut device,
        )?;
        let device = device.context("D3D9 probe")?;
        let mut targets = Vec::new();
        super::dxgi::install(
            device.vtable().base__.Present as usize,
            present as _,
            &PRESENT,
            &mut targets,
        );
        super::dxgi::install(
            device.vtable().PresentEx as usize,
            present_ex as _,
            &EX,
            &mut targets,
        );
        super::dxgi::install(
            device.vtable().base__.Reset as usize,
            reset as _,
            &RESET,
            &mut targets,
        );
        super::dxgi::install(
            device.vtable().ResetEx as usize,
            reset_ex as _,
            &RESETEX,
            &mut targets,
        );
        let chain = device.GetSwapChain(0)?;
        super::dxgi::install(
            chain.vtable().Present as usize,
            swap as _,
            &SWAP,
            &mut targets,
        );
        Ok(targets)
    })();
    DestroyWindow(hwnd);
    result
}
pub fn run(channel: Channel) {
    let Some(config) = channel.snapshot() else {
        return;
    };
    if config.reserved == relay_hook_protocol::CPU_ONLY {
        return;
    }
    let targets = TARGETS.get_or_init(|| unsafe { discover().unwrap_or_default() });
    if targets.is_empty() {
        return;
    }
    if let Ok(mut s) = STATE.lock() {
        *s = Some(State {
            channel,
            config,
            due: Instant::now(),
            capture: None,
        })
    } else {
        return;
    }
    for t in targets {
        let _ = unsafe { MinHook::enable_hook(*t as _) };
    }
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if !STATE
            .lock()
            .ok()
            .is_some_and(|s| s.as_ref().is_some_and(|s| s.channel.alive()))
        {
            break;
        }
    }
    clear();
    if let Ok(mut s) = STATE.lock() {
        *s = None
    }
    for t in targets {
        let _ = unsafe { MinHook::disable_hook(*t as _) };
    }
}
