use anyhow::{Context, Result};
use minhook::MinHook;
use relay_hook_gpu::Sender;
use relay_hook_protocol::{ipc::Channel, DXGI};
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
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::*,
            Direct3D10::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};
type Raw = *mut c_void;
static PRESENT: AtomicUsize = AtomicUsize::new(0);
static PRESENT1: AtomicUsize = AtomicUsize::new(0);
static RESIZE: AtomicUsize = AtomicUsize::new(0);
static RESIZE1: AtomicUsize = AtomicUsize::new(0);
static TARGETS: OnceLock<Vec<usize>> = OnceLock::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
thread_local! {static INSIDE:Cell<bool>=const{Cell::new(false)};}
struct State {
    channel: Channel,
    config: relay_hook_protocol::Header,
    due: Instant,
    capture: Option<Capture>,
}
struct Capture {
    chain: usize,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    kind: Kind,
    sender: Sender,
}
enum Kind {
    D11 {
        device: ID3D11Device,
        context: ID3D11DeviceContext,
    },
    D10(super::d3d10::Capture),
}
unsafe fn capture(raw: Raw) -> Result<()> {
    let Ok(mut slot) = STATE.try_lock() else {
        return Ok(());
    };
    let Some(s) = slot.as_mut() else {
        return Ok(());
    };
    let now = Instant::now();
    if now < s.due {return Ok(())}
    if !s.channel.alive() {s.capture=None;return Ok(())}
    let chain = IDXGISwapChain::from_raw_borrowed(&raw).context("swapchain")?;
    let desc = chain.GetDesc()?;
    if !s.channel.accepts_window(desc.OutputWindow.0 as u64) {
        return Ok(());
    }
    let interval = Duration::from_secs_f64(1.0 / s.config.fps.clamp(1, 120) as f64);
    s.due += interval;
    if s.due <= now {
        s.due = now + interval
    }
    if !s.capture.as_ref().is_some_and(|c| c.chain==raw as usize && matches!(c.kind,Kind::D10(_))) {
        if let Ok(source) = chain.GetBuffer::<ID3D11Texture2D>(0) {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut d);
            if s.capture.as_ref().is_none_or(|c| {
                c.chain != raw as usize
                    || c.width != d.Width
                    || c.height != d.Height
                    || c.format != d.Format
            }) {
                let device: ID3D11Device = source.GetDevice().context("DXGI device11")?;
                relay_hook_gpu::check_adapter(&device, s.config.adapter_luid)?;
                let context = device.GetImmediateContext()?;
                match Sender::new(&device,d.Width,d.Height,d.Format) {
                    Ok(sender)=>s.capture=Some(Capture{chain:raw as usize,width:d.Width,height:d.Height,format:d.Format,kind:Kind::D11{device,context},sender}),
                    Err(_) if chain.GetDevice::<ID3D10Device>().is_ok()=>{s.capture=None;},
                    Err(e)=>return Err(e),
                }
            }
            if let Some(Capture {
                kind: Kind::D11 { device, context },
                sender,
                ..
            }) = &s.capture
            {
                let _ = device;
                sender.copy(context, &source, &s.channel, DXGI)?;
                return Ok(());
            }
        }
    }
    if let Ok(source) = chain.GetBuffer::<ID3D10Texture2D>(0) {
        let mut d = D3D10_TEXTURE2D_DESC::default();
        source.GetDesc(&mut d);
        if s.capture.as_ref().is_none_or(|c| {
            c.chain != raw as usize
                || c.width != d.Width
                || c.height != d.Height
                || c.format != d.Format
        }) {
            let (capture, sender) =
                super::d3d10::Capture::new(source.GetDevice()?, d, s.config.adapter_luid)?;
            s.capture = Some(Capture {
                chain: raw as usize,
                width: d.Width,
                height: d.Height,
                format: d.Format,
                kind: Kind::D10(capture),
                sender,
            });
        }
        if let Some(Capture {
            kind: Kind::D10(capture),
            sender,
            ..
        }) = &mut s.capture
        {
            capture.publish(&source, d, sender, &s.channel)?;
        }
        return Ok(());
    }
    super::d3d12::capture(chain, &s.channel, s.config.adapter_luid)
}
unsafe fn before(raw: Raw, flags: u32) {
    if flags & DXGI_PRESENT_TEST.0 != 0 {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        if let Err(e) = capture(raw) {
            if std::env::var_os("RELAY_CAPTURE_DIAGNOSTICS").is_some() {
                eprintln!("Relay DXGI: {e:#}");
            }
            if let Ok(s) = STATE.try_lock() {
                if let Some(s) = s.as_ref() {
                    s.channel.stop();
                }
            }
        }
    });
}
unsafe extern "system" fn present(raw: Raw, sync: u32, flags: DXGI_PRESENT) -> HRESULT {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        before(raw, flags.0)
    }
    let original: unsafe extern "system" fn(Raw, u32, DXGI_PRESENT) -> HRESULT =
        std::mem::transmute(PRESENT.load(Ordering::Acquire));
    if !nested {super::d3d12::enter(raw as usize);}
    let r = original(raw, sync, flags);
    if !nested {super::d3d12::leave();}
    INSIDE.with(|v| v.set(nested));
    r
}
unsafe extern "system" fn present1(
    raw: Raw,
    sync: u32,
    flags: DXGI_PRESENT,
    params: *const DXGI_PRESENT_PARAMETERS,
) -> HRESULT {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        before(raw, flags.0)
    }
    let original: unsafe extern "system" fn(
        Raw,
        u32,
        DXGI_PRESENT,
        *const DXGI_PRESENT_PARAMETERS,
    ) -> HRESULT = std::mem::transmute(PRESENT1.load(Ordering::Acquire));
    if !nested {super::d3d12::enter(raw as usize);}
    let r = original(raw, sync, flags, params);
    if !nested {super::d3d12::leave();}
    INSIDE.with(|v| v.set(nested));
    r
}
unsafe extern "system" fn resize(
    raw: Raw,
    count: u32,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    flags: u32,
) -> HRESULT {
    if let Ok(mut s) = STATE.lock() {
        if let Some(s) = s.as_mut() {
            s.capture = None
        }
    }
    super::d3d12::reset(raw as usize);
    let original: unsafe extern "system" fn(Raw, u32, u32, u32, DXGI_FORMAT, u32) -> HRESULT =
        std::mem::transmute(RESIZE.load(Ordering::Acquire));
    original(raw, count, w, h, format, flags)
}
unsafe extern "system" fn resize1(raw:Raw,count:u32,w:u32,h:u32,format:DXGI_FORMAT,flags:u32,masks:*const u32,queues:*const Raw)->HRESULT{
    if let Ok(mut s)=STATE.lock(){if let Some(s)=s.as_mut(){s.capture=None}}
    super::d3d12::reset(raw as usize);
    let original:unsafe extern "system" fn(Raw,u32,u32,u32,DXGI_FORMAT,u32,*const u32,*const Raw)->HRESULT=std::mem::transmute(RESIZE1.load(Ordering::Acquire));
    original(raw,count,w,h,format,flags,masks,queues)
}
pub unsafe fn install(
    address: usize,
    detour: Raw,
    original: &AtomicUsize,
    targets: &mut Vec<usize>,
) {
    if targets.contains(&address) {
        return;
    }
    if let Ok(p) = MinHook::create_hook(address as _, detour) {
        original.store(p as usize, Ordering::Release);
        targets.push(address)
    }
}
pub fn run(channel: Channel) {
    let Some(config) = channel.snapshot() else {
        return;
    };
    if config.reserved==relay_hook_protocol::CPU_ONLY{return}
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
    if let Ok(mut s) = STATE.lock() {
        *s = None
    }
    super::d3d12::reset(0);
    for t in targets {
        let _ = unsafe { MinHook::disable_hook(*t as _) };
    }
}
unsafe fn discover() -> Result<Vec<usize>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = CreateWindowExW(
        0,
        relay_hook_protocol::ipc::wide("STATIC").as_ptr(),
        relay_hook_protocol::ipc::wide("Relay hook probe").as_ptr(),
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
    anyhow::ensure!(!hwnd.is_null(), "finestra probe");
    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: 2,
            Height: 2,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ..Default::default()
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        OutputWindow: HWND(hwnd),
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        ..Default::default()
    };
    let mut chain = None;
    let result = D3D11CreateDeviceAndSwapChain(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&desc),
        Some(&mut chain),
        None,
        None,
        None,
    );
    let mut targets = Vec::new();
    if result.is_ok() {
        if let Some(chain) = chain {
            install(
                chain.vtable().Present as usize,
                present as _,
                &PRESENT,
                &mut targets,
            );
            install(
                chain.vtable().ResizeBuffers as usize,
                resize as _,
                &RESIZE,
                &mut targets,
            );
            if let Ok(c)=chain.cast::<IDXGISwapChain3>(){install(c.vtable().ResizeBuffers1 as usize,resize1 as _,&RESIZE1,&mut targets);}
            if let Ok(c) = chain.cast::<IDXGISwapChain1>() {
                install(
                    c.vtable().Present1 as usize,
                    present1 as _,
                    &PRESENT1,
                    &mut targets,
                )
            }
        }
    }
    DestroyWindow(hwnd);
    super::d3d12::install(&mut targets);
    Ok(targets)
}
