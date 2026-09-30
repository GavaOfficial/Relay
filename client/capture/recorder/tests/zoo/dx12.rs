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
            Foundation::{HWND, RECT},
            Graphics::{
                Direct3D::*,
                Direct3D11::*,
                Direct3D11on12::*,
                Direct3D12::*,
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
    let class = wide("RelayZooDX12");
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
        wide("Relay zoo DX12").as_ptr(),
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
    let mut d12: Option<ID3D12Device> = None;
    D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut d12)?;
    let d12 = d12.unwrap();
    let queue: ID3D12CommandQueue = d12.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default())?;
    let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 768,
        Height: 480,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let swap: IDXGISwapChain3 = factory
        .CreateSwapChainForHwnd(&queue, HWND(hwnd), &desc, None, None)?
        .cast()?;
    let mut device = None;
    let mut context = None;
    D3D11On12CreateDevice(
        &d12,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT.0,
        None,
        Some(&[Some(queue.cast()?)]),
        0,
        Some(&mut device),
        Some(&mut context),
        None,
    )?;
    let device = device.unwrap();
    let bridge: ID3D11On12Device = device.cast()?;
    let context: ID3D11DeviceContext1 = context.unwrap().cast()?;
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
        let back: ID3D12Resource = swap.GetBuffer(swap.GetCurrentBackBufferIndex())?;
        let mut tex: Option<ID3D11Texture2D> = None;
        bridge.CreateWrappedResource(
            &back,
            &D3D11_RESOURCE_FLAGS {
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
            &mut tex,
        )?;
        let tex = tex.unwrap();
        let resources = [Some(tex.cast::<ID3D11Resource>()?)];
        bridge.AcquireWrappedResources(&resources);
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
                    left: bit * 768 / 24,
                    top: 0,
                    right: (bit + 1) * 768 / 24,
                    bottom: 480,
                }]),
            );
        }
        context.ClearView(
            &view,
            &[0.0, 1.0, 0.0, 1.0],
            Some(&[RECT {
                left: 0,
                top: 0,
                right: 768,
                bottom: 16,
            }]),
        );
        bridge.ReleaseWrappedResources(&resources);
        context.Flush();
        let swap1: IDXGISwapChain1 = swap.cast()?;
        swap1
            .Present1(1, DXGI_PRESENT(0), &DXGI_PRESENT_PARAMETERS::default())
            .ok()?;
        std::thread::sleep(Duration::from_millis(2));
    }
    swap.SetFullscreenState(false, None)?;
    DestroyWindow(hwnd);
    Ok(())
}
