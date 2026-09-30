use minhook::MinHook;
use relay_hook_protocol::{
    ipc::{wide, Channel},
    OPENGL,
};
use std::{
    cell::Cell,
    ffi::c_void,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Graphics::{Gdi::*, OpenGL::*},
    System::LibraryLoader::*,
    UI::WindowsAndMessaging::GetClientRect,
};

struct Capture {
    channel: Channel,
    pixels: Vec<u8>,
    next_due: Instant,
    config: relay_hook_protocol::Header,
    gpu: Option<super::opengl_gpu::Capture>,
    gpu_attempted: bool,
}
static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);
static WGL: AtomicUsize = AtomicUsize::new(0);
static GDI: AtomicUsize = AtomicUsize::new(0);
static DELETE: AtomicUsize = AtomicUsize::new(0);
static TARGETS: OnceLock<Vec<usize>> = OnceLock::new();
thread_local! { static INSIDE: Cell<bool> = const { Cell::new(false) }; }
type Swap = unsafe extern "system" fn(HDC) -> i32;

unsafe fn present(dc: HDC, original: &AtomicUsize) -> i32 {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        let _ = std::panic::catch_unwind(|| {
            super::opengl_gpu::collect();
            capture(dc);
        });
    }
    let f: Swap = std::mem::transmute(original.load(Ordering::Acquire));
    let result = f(dc);
    INSIDE.with(|v| v.set(nested));
    result
}
unsafe extern "system" fn wgl(dc: HDC) -> i32 {
    present(dc, &WGL)
}
unsafe extern "system" fn gdi(dc: HDC) -> i32 {
    present(dc, &GDI)
}
unsafe extern "system" fn delete_context(rc: HGLRC) -> i32 {
    let nested = INSIDE.with(|v| v.replace(true));
    if !nested {
        let _ = std::panic::catch_unwind(|| {
            if let Ok(mut slot) = CAPTURE.lock() {
                if let Some(s) = slot.as_mut() {
                    if s.gpu.as_ref().is_some_and(|g| g.context_is(rc as usize)) {
                        s.gpu = None;
                        s.gpu_attempted = false;
                    }
                }
            }
            super::opengl_gpu::deleting_context(rc as usize);
        });
    }
    let original = std::mem::transmute::<usize, unsafe extern "system" fn(HGLRC) -> i32>(
        DELETE.load(Ordering::Acquire),
    );
    let result = original(rc);
    INSIDE.with(|v| v.set(nested));
    result
}

pub fn run(channel: Channel) {
    let Some(config) = channel.snapshot() else {
        return;
    };
    let targets = TARGETS.get_or_init(|| unsafe {
        let mut targets = Vec::new();
        if GetModuleHandleW(wide("opengl32.dll").as_ptr()).is_null() {
            return targets;
        }
        for (module, symbol, detour, original) in [
            (
                "opengl32.dll",
                b"wglSwapBuffers\0".as_slice(),
                wgl as *mut c_void,
                &WGL,
            ),
            (
                "gdi32.dll",
                b"SwapBuffers\0".as_slice(),
                gdi as *mut c_void,
                &GDI,
            ),
        ] {
            let module = GetModuleHandleW(wide(module).as_ptr());
            let Some(f) = GetProcAddress(module, symbol.as_ptr()) else {
                continue;
            };
            let address = f as usize;
            if targets.contains(&address) {
                continue;
            }
            if let Ok(trampoline) = MinHook::create_hook(address as _, detour) {
                original.store(trampoline as usize, Ordering::Release);
                targets.push(address);
            }
        }
        let module = GetModuleHandleW(wide("opengl32.dll").as_ptr());
        if let Some(f) = GetProcAddress(module, c"wglDeleteContext".as_ptr().cast()) {
            let address = f as usize;
            if let Ok(trampoline) =
                MinHook::create_hook(address as _, delete_context as *mut c_void)
            {
                DELETE.store(trampoline as usize, Ordering::Release);
                targets.push(address);
            }
        }
        targets
    });
    if targets.is_empty() {
        return;
    }
    if let Ok(mut slot) = CAPTURE.lock() {
        *slot = Some(Capture {
            channel,
            pixels: Vec::new(),
            next_due: Instant::now(),
            config,
            gpu: None,
            gpu_attempted: false,
        });
    } else {
        return;
    }
    for &t in targets {
        let _ = unsafe { MinHook::enable_hook(t as _) };
    }
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if !CAPTURE
            .lock()
            .ok()
            .is_some_and(|s| s.as_ref().is_some_and(|s| s.channel.alive()))
        {
            break;
        }
    }
    if let Ok(mut s) = CAPTURE.lock() {
        *s = None;
    }
    while super::opengl_gpu::has_retired() {
        std::thread::sleep(Duration::from_millis(100));
    }
    for &t in targets {
        let _ = unsafe { MinHook::disable_hook(t as _) };
    }
}

unsafe fn extension(name: &[u8]) -> Option<unsafe extern "system" fn(u32, u32)> {
    let p = wglGetProcAddress(name.as_ptr())?;
    if [1, 2, 3, usize::MAX].contains(&(p as usize)) {
        return None;
    }
    Some(std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        unsafe extern "system" fn(u32, u32),
    >(p))
}

fn capture(dc: HDC) {
    let Ok(mut slot) = CAPTURE.try_lock() else {
        return;
    };
    let Some(s) = slot.as_mut() else {
        return;
    };
    let now = Instant::now();
    if now < s.next_due {
        return;
    }
    if !s.channel.alive() {
        s.gpu = None;
        return;
    }
    let interval = Duration::from_secs_f64(1.0 / s.config.fps.clamp(1, 120) as f64);
    s.next_due += interval;
    if s.next_due <= now {
        s.next_due = now + interval;
    }
    unsafe {
        if wglGetCurrentContext().is_null() || wglGetCurrentDC() != dc {
            return;
        }
        let window = WindowFromDC(dc);
        if !s.channel.accepts_window(window as u64) {
            return;
        }
        let mut rect = std::mem::zeroed();
        if window.is_null() || GetClientRect(window, &mut rect) == 0 {
            return;
        }
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if width <= 0 || height <= 0 || width > 8192 || height > 8192 {
            return;
        }
        super::opengl_gpu::collect();
        if s.config.reserved != relay_hook_protocol::CPU_ONLY {
            let size = (width as u32, height as u32);
            if s.gpu.as_ref().is_some_and(|g| !g.matches(size)) {
                s.gpu = None;
                s.gpu_attempted = false;
            }
            if !s.gpu_attempted {
                s.gpu_attempted = true;
                s.gpu = super::opengl_gpu::Capture::new(size, s.config.adapter_luid).ok();
            }
            let Capture { gpu, channel, .. } = &mut *s;
            if let Some(gpu) = gpu {
                match gpu.publish(channel) {
                    Ok(_) => return,
                    Err(_) => {
                        s.gpu = None;
                    }
                }
            }
            if s.config.reserved == relay_hook_protocol::GPU_ONLY {
                s.channel.stop();
                return;
            }
        }
        if width <= 0 || height <= 0 || width > 3840 || height > 2160 {
            return;
        }
        let bytes = width as usize * height as usize * 4;
        s.pixels.resize(bytes, 0);
        let version_ptr = glGetString(GL_VERSION);
        if version_ptr.is_null() {
            return;
        }
        let version = std::ffi::CStr::from_ptr(version_ptr.cast()).to_string_lossy();
        let mut parts = version.split(['.', ' ']);
        let major = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1);
        let minor = parts
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let bind_buffer = if (major, minor) >= (2, 1) {
            extension(b"glBindBuffer\0")
        } else {
            None
        };
        let bind_framebuffer = if major >= 3 {
            extension(b"glBindFramebuffer\0")
        } else {
            None
        };
        let mut pack_buffer = 0;
        let mut framebuffer = 0;
        if let Some(bind) = bind_buffer {
            glGetIntegerv(0x88ed, &mut pack_buffer);
            bind(0x88eb, 0);
        }
        if let Some(bind) = bind_framebuffer {
            glGetIntegerv(0x8caa, &mut framebuffer);
            bind(0x8ca8, 0);
        }
        let mut read = 0;
        glGetIntegerv(GL_READ_BUFFER, &mut read);
        let states = [
            GL_PACK_ALIGNMENT,
            GL_PACK_ROW_LENGTH,
            GL_PACK_SKIP_ROWS,
            GL_PACK_SKIP_PIXELS,
            GL_PACK_SWAP_BYTES,
            GL_PACK_LSB_FIRST,
        ];
        let mut saved = [0; 6];
        for (i, &key) in states.iter().enumerate() {
            glGetIntegerv(key, &mut saved[i]);
            glPixelStorei(key, if key == GL_PACK_ALIGNMENT { 1 } else { 0 });
        }
        let mut double_buffer = 0;
        glGetIntegerv(GL_DOUBLEBUFFER, &mut double_buffer);
        glReadBuffer(if double_buffer != 0 {
            GL_BACK
        } else {
            GL_FRONT
        });
        let bgra = (major, minor) >= (1, 2);
        glReadPixels(
            0,
            0,
            width,
            height,
            if bgra { 0x80e1 } else { GL_RGBA },
            GL_UNSIGNED_BYTE,
            s.pixels.as_mut_ptr().cast(),
        );
        glReadBuffer(read as u32);
        for (i, &key) in states.iter().enumerate() {
            glPixelStorei(key, saved[i]);
        }
        if let Some(bind) = bind_framebuffer {
            bind(0x8ca8, framebuffer as u32);
        }
        if let Some(bind) = bind_buffer {
            bind(0x88eb, pack_buffer as u32);
        }
        if !bgra {
            for p in s.pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        s.channel
            .publish(width as u32, height as u32, OPENGL, true, &s.pixels);
    }
}
