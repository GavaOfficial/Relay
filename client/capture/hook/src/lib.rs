#![cfg(windows)]
mod d3d10;
mod d3d12;
mod d3d9;
mod dxgi;
mod opengl;
mod opengl_gpu;

use relay_hook_protocol::ipc::Channel;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

static STARTED: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub unsafe extern "system" fn RelayStart(_: *mut std::ffi::c_void) -> u32 {
    if STARTED.swap(true, Ordering::AcqRel) {
        return 0;
    }
    let mut module = std::ptr::null_mut();
    if GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
        RelayStart as *const () as *const u16,
        &mut module,
    ) == 0
    {
        STARTED.store(false, Ordering::Release);
        return 1;
    }
    if std::thread::Builder::new()
        .name("relay-hook".into())
        .spawn(|| {
            let result = std::panic::catch_unwind(|| {
                let Ok(channel) = Channel::open(unsafe { GetCurrentProcessId() }) else {
                    return;
                };
                std::thread::scope(|scope| {
                    scope.spawn(|| {
                        if let Ok(c) = Channel::open(unsafe { GetCurrentProcessId() }) {
                            dxgi::run(c);
                        }
                    });
                    scope.spawn(|| {
                        if let Ok(c) = Channel::open(unsafe { GetCurrentProcessId() }) {
                            d3d9::run(c);
                        }
                    });
                    opengl::run(channel);
                });
            });
            let _ = result;
            STARTED.store(false, Ordering::Release);
        })
        .is_err()
    {
        STARTED.store(false, Ordering::Release);
        return 2;
    }
    0
}

#[no_mangle]
pub unsafe extern "system" fn RelayHookProc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        RelayStart(std::ptr::null_mut());
    }
    CallNextHookEx(std::ptr::null_mut(), code, w, l)
}
