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
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D10::*,
            Dxgi::{Common::*, *},
        },
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
    let class = wide("RelayZooDX10");
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
        wide("Relay zoo DX10").as_ptr(),
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
    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: 768,
            Height: 480,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
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
    let mut swap = None;
    let mut device = None;
    D3D10CreateDeviceAndSwapChain1(
        None,
        D3D10_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        0,
        D3D10_FEATURE_LEVEL_10_1,
        32,
        Some(&desc),
        Some(&mut swap),
        Some(&mut device),
    )?;
    let swap = swap.unwrap();
    let device = device.unwrap();
    let mut pixels = vec![0u8; 768 * 480 * 4];
    if std::env::args().any(|a| a == "--exclusive") {
        swap.SetFullscreenState(true, None)?;
    }
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
        let tex: ID3D10Texture2D = swap.GetBuffer(0)?;
        let count = (start.elapsed().as_millis() / 100) as u32;
        for y in 0..480 {
            for x in 0..768 {
                let i = (y * 768 + x) * 4;
                let color = if y < 16 {
                    [0, 255, 0, 255]
                } else if count & (1 << (x * 24 / 768)) != 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                pixels[i..i + 4].copy_from_slice(&color);
            }
        }
        device.UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), 768 * 4, 0);
        swap.Present(1, DXGI_PRESENT(0)).ok()?;
        std::thread::sleep(Duration::from_millis(2));
    }
    swap.SetFullscreenState(false, None)?;
    DestroyWindow(hwnd);
    Ok(())
}
