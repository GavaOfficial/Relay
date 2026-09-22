use serde::{Deserialize, Serialize};

use crate::capture::WindowSel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,

    pub exe: String,
}

const OWN_EXES: [&str; 2] = ["relay-app.exe", "relay-agent.exe"];

pub fn matches(sel: &WindowSel, w: &WindowInfo) -> bool {
    match sel {
        WindowSel::Exe(e) => w.exe.eq_ignore_ascii_case(e),
        WindowSel::TitleRegex(t) => regex::Regex::new(t)
            .map(|r| r.is_match(&w.title))
            .unwrap_or(false),
        WindowSel::Monitor(_) => false,
    }
}

pub fn find_window(sel: &WindowSel) -> Option<WindowInfo> {
    if let WindowSel::Monitor(index) = sel {
        return Some(WindowInfo {
            title: format!("Monitor {}", index + 1),
            exe: "Schermo intero".into(),
        });
    }
    list_windows().into_iter().find(|w| matches(sel, w))
}

#[cfg(windows)]
pub fn find_pid(sel: &WindowSel) -> Option<u32> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let hwnd = list_raw().into_iter().find(|(w, _)| matches(sel, w))?.1 as HWND;
    let mut pid: u32 = 0;

    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (pid != 0).then_some(pid)
}

#[cfg(not(windows))]
pub fn find_pid(_sel: &WindowSel) -> Option<u32> {
    None
}

pub fn window_exists(sel: &WindowSel) -> bool {
    find_window(sel).is_some()
}

#[cfg(windows)]
pub fn window_monitor_name(sel: &WindowSel) -> Option<String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
    };

    let hwnd = list_raw().into_iter().find(|(w, _)| matches(sel, w))?.1 as HWND;
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(
            monitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        ) == 0
        {
            return None;
        }
        let len = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        Some(String::from_utf16_lossy(&info.szDevice[..len]))
    }
}

#[cfg(not(windows))]
pub fn window_monitor_name(_sel: &WindowSel) -> Option<String> {
    None
}

impl WindowInfo {
    pub fn label(&self) -> String {
        let t: String = self.title.chars().take(80).collect();
        if t.trim().is_empty() {
            self.exe.clone()
        } else {
            format!("{} - {}", self.exe, t)
        }
    }
}

#[cfg(windows)]
pub fn list_windows() -> Vec<WindowInfo> {
    list_raw().into_iter().map(|(w, _)| w).collect()
}

#[cfg(windows)]
fn list_raw() -> Vec<(WindowInfo, isize)> {
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        let out = unsafe { &mut *(lparam as *mut Vec<(WindowInfo, isize)>) };
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 {
                return 1;
            }

            let mut cloaked: u32 = 0;
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                &mut cloaked as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            ) == 0
                && cloaked != 0
            {
                return 1;
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return 1;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return 1;
            }
            let title = String::from_utf16_lossy(&buf[..n as usize]);

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return 1;
            }
            let mut path = vec![0u16; 1024];
            let mut plen = path.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, path.as_mut_ptr(), &mut plen);
            CloseHandle(h);
            if ok == 0 {
                return 1;
            }
            let full = String::from_utf16_lossy(&path[..plen as usize]);
            let exe = full.rsplit(['\\', '/']).next().unwrap_or("").to_string();
            if exe.is_empty() || OWN_EXES.iter().any(|o| exe.eq_ignore_ascii_case(o)) {
                return 1;
            }
            out.push((WindowInfo { title, exe }, hwnd as isize));
        }
        1
    }

    let mut out: Vec<(WindowInfo, isize)> = Vec::new();

    unsafe {
        EnumWindows(
            Some(each),
            &mut out as *mut Vec<(WindowInfo, isize)> as LPARAM,
        );
    }
    out.sort_by(|a, b| {
        a.0.exe
            .to_lowercase()
            .cmp(&b.0.exe.to_lowercase())
            .then(a.0.title.cmp(&b.0.title))
    });
    out
}

#[cfg(not(windows))]
pub fn list_windows() -> Vec<WindowInfo> {
    Vec::new()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(windows)]
pub fn display_rects() -> Vec<ScreenRect> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    unsafe extern "system" fn each(_: HMONITOR, _: HDC, r: *mut RECT, lparam: LPARAM) -> i32 {
        let out = unsafe { &mut *(lparam as *mut Vec<ScreenRect>) };
        let r = unsafe { *r };
        out.push(ScreenRect {
            left: r.left,
            top: r.top,
            width: (r.right - r.left).max(0) as u32,
            height: (r.bottom - r.top).max(0) as u32,
        });
        1
    }
    let mut out: Vec<ScreenRect> = Vec::new();

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(each),
            &mut out as *mut Vec<ScreenRect> as LPARAM,
        );
    }
    out
}

#[cfg(not(windows))]
pub fn display_rects() -> Vec<ScreenRect> {
    Vec::new()
}

#[cfg(windows)]
pub fn fullscreen_screen(sel: &WindowSel) -> Option<ScreenRect> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

    let hwnd = list_raw().into_iter().find(|(w, _)| matches(sel, w))?.1 as HWND;

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        if IsIconic(hwnd) != 0 {
            return None;
        }
        let mut wr: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut wr) == 0 {
            return None;
        }
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if mon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let m = mi.rcMonitor;
        let covers =
            wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom;
        covers.then(|| ScreenRect {
            left: m.left,
            top: m.top,
            width: (m.right - m.left) as u32,
            height: (m.bottom - m.top) as u32,
        })
    }
}

#[cfg(not(windows))]
pub fn fullscreen_screen(_sel: &WindowSel) -> Option<ScreenRect> {
    None
}

pub fn output_index(
    screen: ScreenRect,
    displays: &[ScreenRect],
    outputs: &[(u32, u32, u32)],
) -> Option<u32> {
    let same_size = |w: u32, h: u32| w == screen.width && h == screen.height;
    let candidates: Vec<u32> = outputs
        .iter()
        .filter(|o| same_size(o.1, o.2))
        .map(|o| o.0)
        .collect();
    let pos = displays
        .iter()
        .filter(|d| same_size(d.width, d.height))
        .position(|d| d.left == screen.left && d.top == screen.top)
        .unwrap_or(0);
    candidates.get(pos).or(candidates.first()).copied()
}

pub fn display_index(
    output: u32,
    displays: &[ScreenRect],
    outputs: &[(u32, u32, u32)],
) -> Option<u32> {
    let (_, w, h) = *outputs.iter().find(|o| o.0 == output)?;
    let pos = outputs
        .iter()
        .filter(|o| o.1 == w && o.2 == h)
        .position(|o| o.0 == output)?;
    displays
        .iter()
        .enumerate()
        .filter(|(_, d)| d.width == w && d.height == h)
        .nth(pos)
        .or_else(|| {
            displays
                .iter()
                .enumerate()
                .find(|(_, d)| d.width == w && d.height == h)
        })
        .map(|(i, _)| i as u32)
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_index_maps_dxgi_output_to_windows_order() {
        let r = |l, t, w, h| ScreenRect {
            left: l,
            top: t,
            width: w,
            height: h,
        };

        let displays = [
            r(-6, 1440, 2560, 720),
            r(0, 0, 2560, 1440),
            r(2560, 0, 1920, 1080),
        ];
        let outputs = [(0, 2560, 1440), (1, 1920, 1080), (2, 2560, 720)];
        assert_eq!(display_index(0, &displays, &outputs), Some(1));
        assert_eq!(display_index(1, &displays, &outputs), Some(2));
        assert_eq!(display_index(2, &displays, &outputs), Some(0));
        assert_eq!(display_index(9, &displays, &outputs), None);
    }

    use super::*;

    fn w(title: &str, exe: &str) -> WindowInfo {
        WindowInfo {
            title: title.into(),
            exe: exe.into(),
        }
    }

    #[test]
    fn exe_match_ignores_case() {
        assert!(matches(
            &WindowSel::Exe("Game.EXE".into()),
            &w("Mio Gioco", "game.exe")
        ));
        assert!(!matches(
            &WindowSel::Exe("game.exe".into()),
            &w("x", "game2.exe")
        ));
    }

    #[test]
    fn title_regex_match() {
        let sel = WindowSel::TitleRegex("(?i)^mio gioco".into());
        assert!(matches(&sel, &w("Mio Gioco - livello 3", "a.exe")));
        assert!(!matches(&sel, &w("Altro", "a.exe")));

        assert!(!matches(
            &WindowSel::TitleRegex("(".into()),
            &w("x", "a.exe")
        ));
    }

    #[test]
    fn output_index_matches_by_size_then_order() {
        let r = |l, w, h| ScreenRect {
            left: l,
            top: 0,
            width: w,
            height: h,
        };
        let displays = [r(0, 2560, 1440), r(2560, 1920, 1080), r(4480, 2560, 720)];
        let outputs = [(0, 2560, 1440), (1, 1920, 1080), (2, 2560, 720)];
        assert_eq!(output_index(displays[1], &displays, &outputs), Some(1));
        assert_eq!(output_index(displays[2], &displays, &outputs), Some(2));

        let d2 = [r(0, 1920, 1080), r(1920, 1920, 1080)];
        let o2 = [(0, 1920, 1080), (1, 1920, 1080)];
        assert_eq!(output_index(d2[1], &d2, &o2), Some(1));
        assert_eq!(output_index(r(0, 800, 600), &d2, &o2), None);
    }

    #[test]
    fn label_is_short_and_readable() {
        assert_eq!(w("Mio Gioco", "game.exe").label(), "game.exe - Mio Gioco");
        assert_eq!(w("  ", "game.exe").label(), "game.exe");
        assert!(w(&"x".repeat(300), "a.exe").label().chars().count() <= 88);
    }

    #[test]
    fn listing_does_not_panic_and_hides_own_programs() {
        let l = list_windows();
        assert!(l.iter().all(|w| !w.title.is_empty() && !w.exe.is_empty()));
        assert!(l
            .iter()
            .all(|w| !OWN_EXES.iter().any(|o| w.exe.eq_ignore_ascii_case(o))));
    }
}
