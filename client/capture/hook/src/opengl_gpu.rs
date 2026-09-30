use anyhow::{ensure, Context, Result};
use relay_hook_protocol::ipc::{Channel, Handle};
use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};
use windows_sys::Win32::Graphics::OpenGL::*;

type Raw = *mut c_void;
type Open = unsafe extern "system" fn(Raw) -> Raw;
type Close = unsafe extern "system" fn(Raw) -> i32;
type Register = unsafe extern "system" fn(Raw, Raw, u32, u32, u32) -> Raw;
type Unregister = unsafe extern "system" fn(Raw, Raw) -> i32;
type Lock = unsafe extern "system" fn(Raw, i32, *const Raw) -> i32;
type Gen = unsafe extern "system" fn(i32, *mut u32);
type Delete = unsafe extern "system" fn(i32, *const u32);
type Bind = unsafe extern "system" fn(u32, u32);
type Attach = unsafe extern "system" fn(u32, u32, u32, u32, i32);
type Check = unsafe extern "system" fn(u32) -> u32;
type Blit = unsafe extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32);

struct Functions {
    open: Open,
    close: Close,
    register: Register,
    unregister: Unregister,
    lock: Lock,
    unlock: Lock,
    gen: Gen,
    delete: Delete,
    bind: Bind,
    attach: Attach,
    check: Check,
    blit: Blit,
}
impl Functions {
    unsafe fn load() -> Result<Self> {
        unsafe fn address(name: &std::ffi::CStr) -> Result<usize> {
            let p = wglGetProcAddress(name.as_ptr().cast())
                .context("estensione OpenGL non disponibile")? as usize;
            ensure!(
                ![1, 2, 3, usize::MAX].contains(&p),
                "estensione OpenGL non disponibile"
            );
            Ok(p)
        }
        macro_rules! f {
            ($name:literal, $ty:ty) => {
                std::mem::transmute::<usize, $ty>(address($name)?)
            };
        }
        Ok(Self {
            open: f!(c"wglDXOpenDeviceNV", Open),
            close: f!(c"wglDXCloseDeviceNV", Close),
            register: f!(c"wglDXRegisterObjectNV", Register),
            unregister: f!(c"wglDXUnregisterObjectNV", Unregister),
            lock: f!(c"wglDXLockObjectsNV", Lock),
            unlock: f!(c"wglDXUnlockObjectsNV", Lock),
            gen: f!(c"glGenFramebuffers", Gen),
            delete: f!(c"glDeleteFramebuffers", Delete),
            bind: f!(c"glBindFramebuffer", Bind),
            attach: f!(c"glFramebufferTexture2D", Attach),
            check: f!(c"glCheckFramebufferStatus", Check),
            blit: f!(c"glBlitFramebuffer", Blit),
        })
    }
}

struct Inner {
    functions: Functions,
    rc: usize,
    dc: usize,
    size: (u32, u32),
    epoch: u64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    local: ID3D11Texture2D,
    shared: ID3D11Texture2D,
    mutex: IDXGIKeyedMutex,
    handle: Handle,
    gl_device: Raw,
    object: Raw,
    texture: u32,
    fbo: u32,
    query: ID3D11Query,
    pending: bool,
}
unsafe impl Send for Inner {}
static RETIRED: Mutex<Vec<Inner>> = Mutex::new(Vec::new());
static RETIRED_PENDING: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(1);
struct Slot(Option<Inner>);
unsafe impl Send for Slot {}

impl Inner {
    unsafe fn destroy(self) {
        if !self.object.is_null() {
            (self.functions.unregister)(self.gl_device, self.object);
        }
        if !self.gl_device.is_null() {
            (self.functions.close)(self.gl_device);
        }
        if self.texture != 0 {
            glDeleteTextures(1, &self.texture);
        }
        if self.fbo != 0 {
            (self.functions.delete)(1, &self.fbo);
        }
    }
}

pub fn collect() {
    if !has_retired() {
        return;
    }
    let rc = unsafe { wglGetCurrentContext() } as usize;
    if rc == 0 {
        return;
    }
    let mut ready = Vec::new();
    if let Ok(mut retired) = RETIRED.try_lock() {
        let mut i = 0;
        while i < retired.len() {
            if retired[i].rc == rc {
                ready.push(retired.swap_remove(i));
            } else {
                i += 1;
            }
        }
        RETIRED_PENDING.store(!retired.is_empty(), Ordering::Release);
    }
    for resource in ready {
        unsafe {
            resource.destroy();
        }
    }
}
pub fn has_retired() -> bool {
    RETIRED_PENDING.load(Ordering::Acquire)
}

pub unsafe fn deleting_context(rc: usize) {
    let previous_rc = wglGetCurrentContext();
    let previous_dc = wglGetCurrentDC();
    let dc = RETIRED
        .lock()
        .ok()
        .and_then(|r| r.iter().find(|i| i.rc == rc).map(|i| i.dc));
    let Some(dc) = dc else {
        return;
    };
    let switched = previous_rc as usize != rc;
    if !switched || wglMakeCurrent(dc as _, rc as _) != 0 {
        collect();
        if switched {
            wglMakeCurrent(previous_dc, previous_rc);
        }
    }
    if let Ok(mut r) = RETIRED.lock() {
        let mut index = 0;
        while index < r.len() {
            if r[index].rc == rc {
                std::mem::forget(r.swap_remove(index));
            } else {
                index += 1;
            }
        }
        RETIRED_PENDING.store(!r.is_empty(), Ordering::Release);
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(inner) = self.0.take() {
            if unsafe { wglGetCurrentContext() } as usize == inner.rc {
                unsafe {
                    inner.destroy();
                }
            } else if let Ok(mut retired) = RETIRED.lock() {
                retired.push(inner);
                RETIRED_PENDING.store(true, Ordering::Release);
            } else {
                std::mem::forget(inner);
            }
        }
    }
}

impl Slot {
    pub fn context_is(&self, rc: usize) -> bool {
        self.0.as_ref().is_some_and(|i| i.rc == rc)
    }
    pub fn matches(&self, size: (u32, u32)) -> bool {
        self.0
            .as_ref()
            .is_some_and(|i| i.size == size && i.rc == unsafe { wglGetCurrentContext() } as usize)
    }
    unsafe fn new(size: (u32, u32), luid: u64, reuse: Option<(ID3D11Device, ID3D11DeviceContext)>) -> Result<Self> {
        let functions = Functions::load()?;
        let (device, context) = if let Some(pair) = reuse { pair } else {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut adapter = None;
        for index in 0.. {
            let Ok(a) = factory.EnumAdapters1(index) else {
                break;
            };
            let l = a.GetDesc1()?.AdapterLuid;
            let bits = ((l.HighPart as u32 as u64) << 32) | l.LowPart as u64;
            if bits == luid {
                adapter = Some(a.cast::<IDXGIAdapter>()?);
                break;
            }
        }
        let adapter = adapter.context("scheda video del recorder non trovata")?;
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
        let device = device.context("dispositivo hook")?;
        let context = context.context("contesto hook")?;
        (device, context)
        };
        let mut query = None;
        device.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query))?;
        let query = query.context("query OpenGL")?;
        let mut desc = D3D11_TEXTURE2D_DESC {
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
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            ..Default::default()
        };
        let mut local = None;
        device.CreateTexture2D(&desc, None, Some(&mut local))?;
        let local = local.context("texture interop")?;
        desc.MiscFlags = (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
            | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
        let mut shared = None;
        device.CreateTexture2D(&desc, None, Some(&mut shared))?;
        let shared = shared.context("texture condivisa")?;
        let resource: IDXGIResource1 = shared.cast()?;
        let handle = Handle::checked(
            resource
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    PCWSTR::null(),
                )?
                .0,
        )?;
        let mutex = shared.cast()?;
        let mut capture = Self(Some(Inner {
            functions,
            rc: wglGetCurrentContext() as usize,
            dc: wglGetCurrentDC() as usize,
            size,
            epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
            device,
            context,
            local,
            shared,
            mutex,
            handle,
            gl_device: ptr::null_mut(),
            object: ptr::null_mut(),
            texture: 0,
            fbo: 0,
            query,
            pending: false,
        }));
        let i = capture.0.as_mut().unwrap();
        i.gl_device = (i.functions.open)(i.device.as_raw());
        ensure!(
            !i.gl_device.is_null(),
            "interop OpenGL/D3D11 non disponibile su questa scheda"
        );
        glGenTextures(1, &mut i.texture);
        i.object =
            (i.functions.register)(i.gl_device, i.local.as_raw(), i.texture, GL_TEXTURE_2D, 2);
        ensure!(
            !i.object.is_null(),
            "registrazione texture OpenGL/D3D11 fallita"
        );
        (i.functions.gen)(1, &mut i.fbo);
        ensure!(i.fbo != 0, "framebuffer hook mancante");
        Ok(capture)
    }

    unsafe fn enqueue(&mut self) -> Result<bool> {
        let i = self.0.as_mut().context("cattura GPU chiusa")?;
        let acquired = (i.mutex.vtable().AcquireSync)(i.mutex.as_raw(), 0, 0).0;
        if acquired != 0 {
            if (i.mutex.vtable().AcquireSync)(i.mutex.as_raw(), 1, 0).0 != 0 {
                return Ok(false);
            }
        }
        if (i.functions.lock)(i.gl_device, 1, &i.object) == 0 {
            let _ = i.mutex.ReleaseSync(0);
            anyhow::bail!("lock interop fallito");
        }
        let mut draw = 0;
        let mut read = 0;
        let mut read_buffer = 0;
        glGetIntegerv(0x8ca6, &mut draw);
        glGetIntegerv(0x8caa, &mut read);
        let scissor = glIsEnabled(GL_SCISSOR_TEST);
        glDisable(GL_SCISSOR_TEST);
        (i.functions.bind)(0x8ca8, 0);
        glGetIntegerv(GL_READ_BUFFER, &mut read_buffer);
        (i.functions.bind)(0x8ca9, i.fbo);
        (i.functions.attach)(0x8ca9, 0x8ce0, GL_TEXTURE_2D, i.texture, 0);
        let complete = (i.functions.check)(0x8ca9) == 0x8cd5;
        if complete {
            let mut double = 0;
            glGetIntegerv(GL_DOUBLEBUFFER, &mut double);
            glReadBuffer(if double != 0 { GL_BACK } else { GL_FRONT });
            glDrawBuffer(0x8ce0);
            let (w, h) = (i.size.0 as i32, i.size.1 as i32);
            (i.functions.blit)(0, 0, w, h, 0, h, w, 0, GL_COLOR_BUFFER_BIT, GL_NEAREST);
        }
        glReadBuffer(read_buffer as u32);
        (i.functions.bind)(0x8ca8, read as u32);
        (i.functions.bind)(0x8ca9, draw as u32);
        if scissor != 0 {
            glEnable(GL_SCISSOR_TEST);
        }
        let unlocked = (i.functions.unlock)(i.gl_device, 1, &i.object) != 0;
        if !complete || !unlocked {
            let _ = i.mutex.ReleaseSync(0);
            anyhow::bail!("copie OpenGL/D3D11 non disponibili");
        }
        i.context.CopyResource(&i.shared, &i.local);
        i.context.End(&i.query);
        i.context.Flush();
        i.pending = true;
        Ok(true)
    }
}
pub struct Capture {
    slots: [Slot; 3],
    next: usize,
    published: Option<usize>,
    pending: std::collections::VecDeque<usize>,
}
impl Capture {
    pub unsafe fn new(size: (u32, u32), luid: u64) -> Result<Self> {
        let a = Slot::new(size, luid, None)?;
        let i = a.0.as_ref().unwrap();
        let pair = (i.device.clone(), i.context.clone());
        let b = Slot::new(size, luid, Some(pair.clone()))?;
        let c = Slot::new(size, luid, Some(pair))?;
        Ok(Self { slots: [a,b,c], next: 0, published: None, pending: std::collections::VecDeque::with_capacity(3) })
    }
    pub fn context_is(&self, rc: usize) -> bool { self.slots[0].context_is(rc) }
    pub fn matches(&self, size: (u32,u32)) -> bool { self.slots[0].matches(size) }
    pub unsafe fn publish(&mut self, channel: &Channel) -> Result<bool> {
        let mut published = false;
        while let Some(&index) = self.pending.front() {
            let i = self.slots[index].0.as_mut().unwrap();
            let mut done = 0u32;
            let status = (i.context.vtable().GetData)(i.context.as_raw(), i.query.as_raw(), (&mut done as *mut u32).cast(), 4, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32);
            status.ok()?;
            if status.0 != 0 || done == 0 { break; }
            i.mutex.ReleaseSync(1)?;
            if !channel.publish_texture(i.size.0, i.size.1, i.handle.0 as u64, i.epoch) {
                let status = (i.mutex.vtable().AcquireSync)(i.mutex.as_raw(), 1, 0);
                ensure!(status.0 == 0, "ownership della texture non pubblicata");
                break;
            }
            i.pending = false;
            self.pending.pop_front();
            self.published = Some(index);
            published = true;
        }
        for offset in 0..3 {
            let index = (self.next + offset) % 3;
            if self.published == Some(index) || self.slots[index].0.as_ref().unwrap().pending { continue; }
            if self.slots[index].enqueue()? {
                self.pending.push_back(index);
                self.next = (index + 1) % 3;
                break;
            }
        }
        Ok(published)
    }
}
