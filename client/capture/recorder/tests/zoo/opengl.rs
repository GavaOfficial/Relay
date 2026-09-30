#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    unsafe { win::run() }
}
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
mod win {
    use anyhow::{ensure, Context};
    use relay_hook_protocol::ipc::wide;
    use std::{
        ptr,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Graphics::{Gdi::*, OpenGL::*},
        System::LibraryLoader::*,
        UI::WindowsAndMessaging::*,
    };
    unsafe extern "system" fn wnd(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_CLOSE || (msg == WM_KEYDOWN && w == 27) {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
    struct Display(bool);
    impl Drop for Display {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    ChangeDisplaySettingsW(ptr::null(), 0);
                }
            }
        }
    }
    pub unsafe fn run() -> anyhow::Result<()> {
        let exclusive = std::env::args().any(|a| a == "--exclusive");
        let mut mode: DEVMODEW = std::mem::zeroed();
        mode.dmSize = std::mem::size_of_val(&mode) as u16;
        let (width, height) = if exclusive {
            ensure!(
                EnumDisplaySettingsW(ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) != 0,
                "lettura modo video"
            );
            ensure!(
                ChangeDisplaySettingsW(&mode, CDS_FULLSCREEN) == DISP_CHANGE_SUCCESSFUL,
                "schermo esclusivo non disponibile"
            );
            (mode.dmPelsWidth as i32, mode.dmPelsHeight as i32)
        } else {
            (768, 480)
        };
        let _display = Display(exclusive);
        let instance = GetModuleHandleW(ptr::null());
        let class = wide("RelayZooOpenGL");
        let wc = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(wnd),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        ensure!(RegisterClassW(&wc) != 0, "classe zoo");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Relay zoo OpenGL - Esc per uscire").as_ptr(),
            WS_VISIBLE
                | if exclusive {
                    WS_POPUP
                } else {
                    WS_OVERLAPPEDWINDOW
                },
            0,
            0,
            width,
            height,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        ensure!(!hwnd.is_null(), "finestra zoo");
        let dc = GetDC(hwnd);
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            ..std::mem::zeroed()
        };
        let pf = ChoosePixelFormat(dc, &pfd);
        ensure!(
            pf != 0 && SetPixelFormat(dc, pf, &pfd) != 0,
            "formato pixel OpenGL"
        );
        let rc = wglCreateContext(dc);
        ensure!(
            !rc.is_null() && wglMakeCurrent(dc, rc) != 0,
            "contesto OpenGL"
        );
        SetForegroundWindow(hwnd);
        println!("{} {}", std::process::id(), hwnd as usize);
        let start = Instant::now();
        let benchmark = std::env::args().any(|a| a == "--benchmark");
        let vsync = std::env::args().any(|s| s == "--vsync");
        if vsync {
            type Set = unsafe extern "system" fn(i32) -> i32;
            type Get = unsafe extern "system" fn() -> i32;
            let set = wglGetProcAddress(c"wglSwapIntervalEXT".as_ptr().cast()).context("wglSwapIntervalEXT assente")?;
            let get = wglGetProcAddress(c"wglGetSwapIntervalEXT".as_ptr().cast()).context("wglGetSwapIntervalEXT assente")?;
            let set: Set = std::mem::transmute(set);
            let get: Get = std::mem::transmute(get);
            ensure!(set(1) != 0 && get() == 1, "v-sync OpenGL non attivo");
        }
        let mut timings = Vec::new();
        let mut msg: MSG = std::mem::zeroed();
        'draw: while start.elapsed() < Duration::from_secs(if vsync { 90 } else { 25 }) {
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'draw;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            let (w, h) = (rect.right, rect.bottom);
            let count = (start.elapsed().as_millis() / 100) as u32;
            glViewport(0, 0, w, h);
            glEnable(GL_SCISSOR_TEST);
            for bit in 0..24 {
                let a = bit * w / 24;
                let b = (bit + 1) * w / 24;
                glScissor(a, 0, b - a, h);
                if count & (1 << bit) != 0 {
                    glClearColor(1.0, 0.0, 0.0, 1.0);
                } else {
                    glClearColor(0.0, 0.0, 1.0, 1.0);
                }
                glClear(GL_COLOR_BUFFER_BIT);
            }
            glScissor(0, h - 16, w, 16);
            glClearColor(0.0, 1.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            glDisable(GL_SCISSOR_TEST);
            glPixelStorei(GL_PACK_ALIGNMENT, 8);
            glPixelStorei(GL_PACK_ROW_LENGTH, 31);
            let swap_start = Instant::now();
            SwapBuffers(dc);
            if benchmark && start.elapsed() >= Duration::from_secs(3) {
                timings.push(swap_start.elapsed().as_secs_f64() * 1_000_000.0);
            }
            let mut row = 0;
            glGetIntegerv(GL_PACK_ROW_LENGTH, &mut row);
            ensure!(row == 31, "l'hook ha alterato lo stato GL_PACK_ROW_LENGTH");
            if !vsync { std::thread::sleep(Duration::from_millis(10)); }
        }
        wglMakeCurrent(ptr::null_mut(), ptr::null_mut());
        wglDeleteContext(rc);
        ReleaseDC(hwnd, dc);
        DestroyWindow(hwnd);
        if !timings.is_empty() {
            timings.sort_by(f64::total_cmp);
            let n = timings.len();
            use std::io::Write;
            let _ = writeln!(
                std::io::stdout(),
                "swap samples={} mean_us={:.2} p95_us={:.2} p99_us={:.2}",
                n,
                timings.iter().sum::<f64>() / n as f64,
                timings[(n - 1) * 95 / 100],
                timings[(n - 1) * 99 / 100]
            );
        }
        Ok(())
    }
}
