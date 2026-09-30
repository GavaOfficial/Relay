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
    use windows::{
        core::Interface,
        Win32::{
            Foundation::{HMODULE, HWND, RECT},
            Graphics::{
                Direct3D::*,
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
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
    let class = wide("RelayZooDXGI");
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
        wide("Relay zoo DXGI").as_ptr(),
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
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let mut swap = None;
    let mut device = None;
    let mut context = None;
    D3D11CreateDeviceAndSwapChain(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&desc),
        Some(&mut swap),
        Some(&mut device),
        None,
        Some(&mut context),
    )?;
    let swap = swap.unwrap();
    let device = device.unwrap();
    let context: ID3D11DeviceContext1 = context.unwrap().cast()?;
    if std::env::args().any(|a| a == "--exclusive") {
        swap.SetFullscreenState(true, None)?;
    }
    println!("{} {}", std::process::id(), hwnd as usize);
    let start = Instant::now();
    let mut msg: MSG = std::mem::zeroed();
    let vsync = std::env::args().any(|s| s == "--vsync");
    let resize_test = std::env::var_os("RELAY_ZOO_RESIZE").is_some();
    let mut size = (768, 480);
    'draw: while start.elapsed() < Duration::from_secs(if vsync { 90 } else { 30 }) {
        while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                break 'draw;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let desired = if resize_test && start.elapsed() >= Duration::from_secs(6) {
            (640, 400)
        } else if resize_test && start.elapsed() >= Duration::from_secs(3) {
            (960, 600)
        } else {
            (768, 480)
        };
        if size != desired {
            context.ClearState();
            context.Flush();
            swap.ResizeBuffers(
                2,
                desired.0 as u32,
                desired.1 as u32,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            let mut r = windows_sys::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: desired.0,
                bottom: desired.1,
            };
            AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, 0);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOMOVE | SWP_NOZORDER,
            );
            size = desired;
        }
        let tex: ID3D11Texture2D = swap.GetBuffer(0)?;
        let mut view = None;
        device.CreateRenderTargetView(&tex, None, Some(&mut view))?;
        let view = view.unwrap();
        let count = (start.elapsed().as_millis() / 100) as u32;
        for bit in 0..24 {
            let color = if count & (1 << bit) != 0 {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 1.0, 1.0]
            };
            context.ClearView(
                &view,
                &color,
                Some(&[RECT {
                    left: bit * size.0 / 24,
                    top: 0,
                    right: (bit + 1) * size.0 / 24,
                    bottom: size.1,
                }]),
            );
        }
        context.ClearView(
            &view,
            &[0.0, 1.0, 0.0, 1.0],
            Some(&[RECT {
                left: 0,
                top: 0,
                right: size.0,
                bottom: 16,
            }]),
        );
        if vsync {
            swap.Present(1, DXGI_PRESENT(0)).ok()?;
        } else {
            let swap1: IDXGISwapChain1 = swap.cast()?;
            swap1
                .Present1(1, DXGI_PRESENT(0), &DXGI_PRESENT_PARAMETERS::default())
                .ok()?;
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    swap.SetFullscreenState(false, None)?;
    DestroyWindow(hwnd);
    Ok(())
}
