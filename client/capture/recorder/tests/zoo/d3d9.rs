#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    unsafe { run() }
}
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
unsafe fn run() -> anyhow::Result<()> {
    use relay_hook_protocol::ipc::wide;
    use std::{
        ptr,
        time::{Duration, Instant},
    };
    use windows::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Direct3D9::*,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn wnd(
        h: windows_sys::Win32::Foundation::HWND,
        m: u32,
        w: usize,
        l: isize,
    ) -> isize {
        if m == WM_CLOSE || (m == WM_KEYDOWN && w == 27) {
            PostQuitMessage(0);
            0
        } else {
            DefWindowProcW(h, m, w, l)
        }
    }
    let class = wide("RelayZooD3D9");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&wc);
    let mut wr = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 768,
        bottom: 480,
    };
    AdjustWindowRect(&mut wr, WS_OVERLAPPEDWINDOW, 0);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("Relay zoo D3D9Ex").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        0,
        0,
        wr.right - wr.left,
        wr.bottom - wr.top,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null(),
    );
    anyhow::ensure!(!hwnd.is_null(), "window");
    let d3d = Direct3DCreate9Ex(D3D_SDK_VERSION)?;
    let mut pp = D3DPRESENT_PARAMETERS {
        BackBufferWidth: 768,
        BackBufferHeight: 480,
        BackBufferFormat: D3DFMT_A8R8G8B8,
        BackBufferCount: 1,
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        hDeviceWindow: HWND(hwnd),
        Windowed: true.into(),
        PresentationInterval: D3DPRESENT_INTERVAL_ONE as u32,
        ..Default::default()
    };
    let mut device = None;
    d3d.CreateDeviceEx(
        0,
        D3DDEVTYPE_HAL,
        HWND(hwnd),
        (D3DCREATE_HARDWARE_VERTEXPROCESSING | D3DCREATE_MULTITHREADED) as u32,
        &mut pp,
        ptr::null_mut(),
        &mut device,
    )?;
    let device = device.unwrap();
    println!("{} {}", std::process::id(), hwnd as usize);
    let start = Instant::now();
    let mut msg: MSG = std::mem::zeroed();
    'draw: while start.elapsed() < Duration::from_secs(30) {
        while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                break 'draw;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let tex = device.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
        let count = (start.elapsed().as_millis() / 100) as u32;
        for bit in 0..24 {
            let color = if count & (1 << bit) != 0 {
                0xffff0000
            } else {
                0xff0000ff
            };
            device.ColorFill(
                &tex,
                &RECT {
                    left: bit * 768 / 24,
                    top: 0,
                    right: (bit + 1) * 768 / 24,
                    bottom: 480,
                },
                color,
            )?;
        }
        device.ColorFill(
            &tex,
            &RECT {
                left: 0,
                top: 0,
                right: 768,
                bottom: 16,
            },
            0xff00ff00,
        )?;
        device.PresentEx(ptr::null(), ptr::null(), HWND::default(), ptr::null(), 0)?;
        std::thread::sleep(Duration::from_millis(2));
    }
    DestroyWindow(hwnd);
    Ok(())
}
