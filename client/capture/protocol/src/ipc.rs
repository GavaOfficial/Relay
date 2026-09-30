use crate::*;
use anyhow::{bail, ensure, Context, Result};
use std::{mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::*,
    System::{Memory::*, Threading::*},
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub struct Handle(pub HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Handle {
    pub fn checked(h: HANDLE) -> Result<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            bail!(std::io::Error::last_os_error());
        }
        Ok(Self(h))
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

const BYTES: usize = size_of::<Header>() + CAPACITY;
pub struct Channel {
    _mapping: Handle,
    view: *mut u8,
    mutex: Handle,
    pub ready: Handle,
    pub resized: Handle,
    pub detach: Handle,
    owner: Option<Handle>,
    pid: u32,
}
unsafe impl Send for Channel {}

struct Guard<'a>(&'a Channel);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.mutex.0);
        }
    }
}
impl Guard<'_> {
    pub fn header(&self) -> Header {
        unsafe { ptr::read(self.0.view.cast()) }
    }
    pub fn set_header(&mut self, h: Header) {
        unsafe {
            ptr::write(self.0.view.cast(), h);
        }
    }
    pub fn pixels(&self, bytes: usize) -> &[u8] {
        assert!(bytes <= CAPACITY);
        unsafe { std::slice::from_raw_parts(self.0.view.add(size_of::<Header>()), bytes) }
    }
    pub fn pixels_mut(&mut self, bytes: usize) -> &mut [u8] {
        assert!(bytes <= CAPACITY);
        unsafe { std::slice::from_raw_parts_mut(self.0.view.add(size_of::<Header>()), bytes) }
    }
}
impl Channel {
    pub fn create(pid: u32) -> Result<Self> {
        Self::create_for_window(pid, 0)
    }
    pub fn create_for_window(pid: u32, hwnd: u64) -> Result<Self> {
        Self::create_with_capacity(pid, hwnd, CAPACITY)
    }
    pub fn create_gpu(pid: u32, hwnd: u64) -> Result<Self> {
        Self::create_with_capacity(pid, hwnd, 0)
    }
    fn create_with_capacity(pid: u32, hwnd: u64, capacity: usize) -> Result<Self> {
        let c = Self::connect(pid, true, capacity)?;
        let mut g = c.lock(1000).context("inizializzazione finestra hook")?;
        let mut h = g.header();
        h.target_hwnd = hwnd;
        g.set_header(h);
        drop(g);
        Ok(c)
    }
    pub fn open(pid: u32) -> Result<Self> {
        Self::connect(pid, false, 0)
    }
    pub fn configure(&self, mode: u32, fps: u32, adapter_luid: u64) -> Result<()> {
        let mut g = self.lock(1000).context("configurazione hook occupata")?;
        let mut h = g.header();
        ensure!(h.frames == 0, "hook gia' avviato");
        h.reserved = mode;
        h.fps = fps.clamp(1, 120);
        h.adapter_luid = adapter_luid;
        g.set_header(h);
        Ok(())
    }
    pub fn snapshot(&self) -> Option<Header> {
        let h = self.lock(0)?.header();
        h.valid(self.pid).then_some(h)
    }
    pub fn publish_texture(&self, width: u32, height: u32, handle: u64, epoch: u64) -> bool {
        self.publish_gpu(width, height, handle, epoch, OPENGL, BGRA8)
    }
    pub fn publish_gpu(
        &self,
        width: u32,
        height: u32,
        handle: u64,
        epoch: u64,
        api: u32,
        format: u32,
    ) -> bool {
        let Some(mut g) = self.lock(0) else {
            return false;
        };
        let old = g.header();
        if !old.valid(self.pid) {
            return false;
        }
        let h = Header {
            width,
            height,
            stride: 0,
            format,
            api,
            flipped: 0,
            texture_handle: handle,
            texture_epoch: epoch,
            payload_bytes: 0,
            frames: old.frames.wrapping_add(1),
            ..old
        };
        if !h.gpu_valid() {
            return false;
        }
        g.set_header(h);
        drop(g);
        unsafe {
            if (old.width, old.height) != (width, height) {
                SetEvent(self.resized.0);
            }
            SetEvent(self.ready.0);
        }
        true
    }
    fn connect(pid: u32, create: bool, capacity: usize) -> Result<Self> {
        unsafe {
            let n = wide(&name(pid, ""));
            let mapping = if create {
                let h = Handle::checked(CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE,
                    0,
                    (size_of::<Header>() + capacity) as u32,
                    n.as_ptr(),
                ))?;
                ensure!(
                    GetLastError() != ERROR_ALREADY_EXISTS,
                    "cattura hook gia' attiva per {pid}"
                );
                h
            } else {
                Handle::checked(OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, n.as_ptr()))?
            };
            let mutex = Handle::checked(CreateMutexW(
                ptr::null(),
                0,
                wide(&name(pid, "_lock")).as_ptr(),
            ))?;
            let event = |suffix, manual| {
                Handle::checked(CreateEventW(
                    ptr::null(),
                    manual,
                    0,
                    wide(&name(pid, suffix)).as_ptr(),
                ))
            };
            let ready = event("_ready", 0)?;
            let resized = event("_resize", 0)?;
            let detach = event("_detach", 1)?;
            let view = MapViewOfFile(
                mapping.0,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                size_of::<Header>() + capacity,
            )
            .Value
            .cast::<u8>();
            ensure!(
                !view.is_null(),
                "mappatura della memoria hook: {}",
                std::io::Error::last_os_error()
            );
            let mut c = Self {
                _mapping: mapping,
                view,
                mutex,
                ready,
                resized,
                detach,
                owner: None,
                pid,
            };
            let mut g = c.lock(1000).context("protocollo hook occupato")?;
            if create {
                g.set_header(Header {
                    magic: MAGIC,
                    version: VERSION,
                    header_bytes: size_of::<Header>() as u32,
                    capacity: capacity as u32,
                    owner_pid: GetCurrentProcessId(),
                    target_pid: pid,
                    ..Default::default()
                });
            }
            let h = g.header();
            ensure!(
                h.valid(pid),
                "versione o dimensione del protocollo hook incompatibile"
            );
            drop(g);
            if !create && h.capacity > 0 {
                let full = MapViewOfFile(c._mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, BYTES)
                    .Value
                    .cast::<u8>();
                ensure!(
                    !full.is_null(),
                    "dimensione effettiva della memoria hook non valida"
                );
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: c.view.cast(),
                });
                c.view = full;
            }
            c.owner = Some(Handle::checked(OpenProcess(
                PROCESS_SYNCHRONIZE,
                0,
                h.owner_pid,
            ))?);
            Ok(c)
        }
    }
    fn lock(&self, timeout: u32) -> Option<Guard<'_>> {
        let status = unsafe { WaitForSingleObject(self.mutex.0, timeout) };
        if status == WAIT_ABANDONED {
            unsafe {
                ReleaseMutex(self.mutex.0);
                SetEvent(self.detach.0);
            }
            return None;
        }
        (status == WAIT_OBJECT_0).then_some(Guard(self))
    }
    pub fn alive(&self) -> bool {
        unsafe {
            WaitForSingleObject(self.detach.0, 0) == WAIT_TIMEOUT
                && self
                    .owner
                    .as_ref()
                    .is_some_and(|p| WaitForSingleObject(p.0, 0) == WAIT_TIMEOUT)
        }
    }
    pub fn stop(&self) {
        unsafe {
            SetEvent(self.detach.0);
        }
    }
    pub fn accepts_window(&self, hwnd: u64) -> bool {
        self.lock(0).is_some_and(|g| {
            let h = g.header();
            h.valid(self.pid) && (h.target_hwnd == 0 || h.target_hwnd == hwnd)
        })
    }
    pub fn publish(&self, width: u32, height: u32, api: u32, flipped: bool, pixels: &[u8]) -> bool {
        if !self.alive() {
            return false;
        }
        let Some(mut g) = self.lock(0) else {
            return false;
        };
        let old = g.header();
        if !old.valid(self.pid) {
            return false;
        }
        let h = Header {
            width,
            height,
            stride: width.saturating_mul(4),
            format: BGRA8,
            api,
            flipped: flipped as u32,
            texture_handle: 0,
            frames: old.frames.wrapping_add(1),
            payload_bytes: pixels.len() as u32,
            ..old
        };
        if h.cpu_bytes() != Some(pixels.len()) {
            return false;
        }
        g.pixels_mut(pixels.len()).copy_from_slice(pixels);
        g.set_header(h);
        drop(g);
        unsafe {
            if (old.width, old.height) != (width, height) {
                SetEvent(self.resized.0);
            }
            SetEvent(self.ready.0);
        }
        true
    }
    pub fn read(&self, after: u64) -> Option<(Header, Vec<u8>)> {
        let g = self.lock(0)?;
        let h = g.header();
        if !h.valid(self.pid) || h.frames == 0 || h.frames == after {
            return None;
        }
        if h.gpu_valid() {
            Some((h, Vec::new()))
        } else {
            Some((h, g.pixels(h.cpu_bytes()?).to_vec()))
        }
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
        }
    }
}

pub fn vulkan_marker(pid: u32) -> Result<Handle> {
    unsafe {
        Handle::checked(CreateEventW(
            ptr::null(),
            1,
            0,
            wide(&format!("Local\\RelayVulkan_{pid}")).as_ptr(),
        ))
    }
}
pub fn vulkan_loaded(pid: u32) -> bool {
    unsafe {
        Handle::checked(OpenEventW(
            0x00100000,
            0,
            wide(&format!("Local\\RelayVulkan_{pid}")).as_ptr(),
        ))
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipc_roundtrip_resize_detach_and_duplicate_owner() {
        let pid = unsafe { GetCurrentProcessId() };
        let consumer = Channel::create(pid).unwrap();
        assert!(Channel::create(pid).is_err());
        let producer = Channel::open(pid).unwrap();
        assert!(producer.alive());
        assert!(!producer.publish(1, 1, OPENGL, true, &[0; 3]));
        assert!(producer.publish(1, 1, OPENGL, true, &[1, 2, 3, 255]));
        let (h, bytes) = consumer.read(0).unwrap();
        assert_eq!(h.frames, 1);
        assert_eq!(bytes, [1, 2, 3, 255]);
        assert!(consumer.read(1).is_none());
        assert_eq!(
            unsafe { WaitForSingleObject(consumer.resized.0, 0) },
            WAIT_OBJECT_0
        );
        assert!(producer.publish(2, 1, OPENGL, false, &[4; 8]));
        assert_eq!(consumer.read(1).unwrap().0.width, 2);
        consumer.stop();
        assert!(!producer.alive());
        assert!(!producer.publish(1, 1, OPENGL, true, &[0; 4]));
        drop(producer);
        drop(consumer);
        let consumer = Channel::create_gpu(pid, 42).unwrap();
        consumer.configure(GPU_ONLY, 30, 7).unwrap();
        let producer = Channel::open(pid).unwrap();
        assert_eq!(producer.snapshot().unwrap().capacity, 0);
        assert!(!producer.publish(1, 1, OPENGL, false, &[0; 4]));
        assert!(producer.publish_texture(1280, 720, 123, 1));
        let (header, bytes) = consumer.read(0).unwrap();
        assert!(header.gpu_valid());
        assert!(bytes.is_empty());
        assert_eq!(header.fps, 30);
        assert!(consumer.configure(CPU_ONLY, 60, 0).is_err());
    }
}
