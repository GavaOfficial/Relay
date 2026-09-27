use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
            Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
            Gdi::{
                ClientToScreen, GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO,
                MONITOR_DEFAULTTONEAREST,
            },
        },
        System::Threading::{
            GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::{
                EnumWindows, GetClientRect, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
                GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
                GWL_EXSTYLE, WS_EX_TOOLWINDOW,
            },
        },
    },
};

use crate::layout::{client_box, Rect};

pub fn dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(pub isize);

impl Handle {
    pub fn hwnd(self) -> HWND {
        HWND(self.0 as _)
    }
}

#[derive(Debug, Clone)]
pub struct Found {
    pub handle: Handle,
    pub pid: u32,
    pub exe: String,
    pub title: String,
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
    list.push(hwnd.0 as isize);
    true.into()
}

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        res.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }
}

fn title_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn cloaked(hwnd: HWND) -> bool {
    let mut value = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut value as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && value != 0
}

pub fn list(include_minimized: bool) -> Vec<Found> {
    let mut handles: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut handles as *mut Vec<isize> as isize),
        );
    }
    let me = unsafe { GetCurrentProcessId() };
    handles
        .into_iter()
        .filter_map(|h| {
            let hwnd = HWND(h as _);
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() || cloaked(hwnd) {
                    return None;
                }
                if !include_minimized && IsIconic(hwnd).as_bool() {
                    return None;
                }
                if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                    return None;
                }
            }
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == 0 || pid == me {
                return None;
            }
            let title = title_of(hwnd);
            if title.trim().is_empty() {
                return None;
            }
            Some(Found {
                handle: Handle(h),
                pid,
                exe: exe_of(pid)?,
                title,
            })
        })
        .collect()
}

pub fn find(exe: &str) -> Option<Found> {
    let mut all: Vec<Found> = list(true)
        .into_iter()
        .filter(|w| w.exe.eq_ignore_ascii_case(exe))
        .collect();
    all.sort_by_key(|w| {
        let minimized = is_minimized(w.handle);
        let area = client_size(w.handle).map_or(0, |(a, b)| a as u64 * b as u64);
        (minimized, std::cmp::Reverse(area))
    });
    all.into_iter().next()
}

pub fn exists(h: Handle) -> bool {
    unsafe { IsWindow(Some(h.hwnd())).as_bool() }
}

pub fn is_minimized(h: Handle) -> bool {
    unsafe { IsIconic(h.hwnd()).as_bool() }
}

pub fn is_foreground(h: Handle) -> bool {
    unsafe { GetForegroundWindow() == h.hwnd() }
}

pub fn client_size(h: Handle) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(h.hwnd(), &mut rect) }.ok()?;
    let w = (rect.right - rect.left).max(0) as u32;
    let hgt = (rect.bottom - rect.top).max(0) as u32;
    (w > 0 && hgt > 0).then_some((w, hgt))
}

pub fn capture_box(h: Handle, texture: (u32, u32)) -> Option<Rect> {
    if is_minimized(h) {
        return None;
    }
    let size = client_size(h)?;
    let mut frame = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            h.hwnd(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    let mut origin = POINT::default();
    if !unsafe { ClientToScreen(h.hwnd(), &mut origin) }.as_bool() {
        return None;
    }
    client_box((frame.left, frame.top), (origin.x, origin.y), size, texture)
}

fn monitor_rect(monitor: HMONITOR) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then_some(info.rcMonitor)
}

pub fn monitor_of(h: Handle) -> isize {
    unsafe { MonitorFromWindow(h.hwnd(), MONITOR_DEFAULTTONEAREST).0 as isize }
}

pub fn covers_monitor(h: Handle) -> bool {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(h.hwnd(), &mut rect) }.is_err() {
        return false;
    }
    let Some(m) = monitor_rect(HMONITOR(monitor_of(h) as _)) else {
        return false;
    };
    rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
}

#[derive(Debug, Clone)]
pub struct Monitor {
    pub index: u32,
    pub handle: isize,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

pub fn monitors() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    let mut a = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a) } {
        a += 1;
        let mut o = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o) } {
            o += 1;
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if !desc.AttachedToDesktop.as_bool()
                || out.iter().any(|m| m.handle == desc.Monitor.0 as isize)
            {
                continue;
            }
            let len = desc
                .DeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.DeviceName.len());
            let r = desc.DesktopCoordinates;
            out.push(Monitor {
                index: out.len() as u32,
                handle: desc.Monitor.0 as isize,
                name: String::from_utf16_lossy(&desc.DeviceName[..len]),
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
            });
        }
    }
    out
}

pub fn pick_monitor(
    all: &[Monitor],
    window: Option<Handle>,
    name: Option<&str>,
    index: Option<u32>,
) -> Option<Monitor> {
    window
        .map(monitor_of)
        .and_then(|h| all.iter().find(|m| m.handle == h))
        .or_else(|| name.and_then(|n| all.iter().find(|m| m.name.eq_ignore_ascii_case(n))))
        .or_else(|| index.and_then(|i| all.get(i as usize)))
        .or_else(|| all.first())
        .cloned()
}
