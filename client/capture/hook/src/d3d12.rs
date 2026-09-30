use anyhow::{Context, Result};
use relay_hook_gpu::Sender;
use relay_hook_protocol::{ipc::Channel, DXGI};
use std::{
    cell::Cell,
    ffi::c_void,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use windows::{
    core::Interface,
    Win32::Graphics::{Direct3D::*, Direct3D11::*, Direct3D11on12::*, Direct3D12::*, Dxgi::*},
};
thread_local! {static PRESENTING:Cell<usize>=const{Cell::new(0)};}
static EXECUTE: AtomicUsize = AtomicUsize::new(0);
static STATE: Mutex<State> = Mutex::new(State {
    queues: Vec::new(),
    capture: None,
});
struct State {
    queues: Vec<(usize, ID3D12CommandQueue)>,
    capture: Option<Capture>,
}
struct Capture {
    chain: usize,
    native_device: usize,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    bridge: ID3D11On12Device,
    sender: Sender,
}
pub fn enter(chain: usize) {
    PRESENTING.with(|c| c.set(chain))
}
pub fn leave() {
    PRESENTING.with(|c| c.set(0))
}
pub fn reset(chain: usize) {
    if let Ok(mut s) = STATE.lock() {
        if chain == 0 || s.capture.as_ref().is_some_and(|c| c.chain == chain) {
            s.capture = None
        }
        s.queues.retain(|(id, _)| chain != 0 && *id != chain);
    }
}
unsafe extern "system" fn execute(raw: *mut c_void, count: u32, lists: *const *mut c_void) {
    let chain = PRESENTING.with(Cell::get);
    if chain != 0 {
        let _ = std::panic::catch_unwind(|| {
            if let Some(queue) = ID3D12CommandQueue::from_raw_borrowed(&raw) {
                if queue.GetDesc().Type == D3D12_COMMAND_LIST_TYPE_DIRECT {
                    if let Ok(mut s) = STATE.try_lock() {
                        if let Some((_, previous)) = s.queues.iter_mut().find(|(c, _)| *c == chain)
                        {
                            if previous.as_raw() != queue.as_raw() {
                                *previous = queue.clone();
                                s.capture = None;
                            }
                        } else if s.queues.len() < 16 {
                            s.queues.push((chain, queue.clone()))
                        }
                    }
                }
            }
        });
    }
    let original: unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void) =
        std::mem::transmute(EXECUTE.load(Ordering::Acquire));
    original(raw, count, lists)
}
pub unsafe fn install(targets: &mut Vec<usize>) {
    let mut device: Option<ID3D12Device> = None;
    if D3D12CreateDevice(
        None,
        D3D_FEATURE_LEVEL_11_0,
        &mut device as *mut Option<ID3D12Device>,
    )
    .is_err()
    {
        return;
    }
    if let Some(device) = device {
        if let Ok(queue) =
            device.CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC::default())
        {
            super::dxgi::install(
                queue.vtable().ExecuteCommandLists as usize,
                execute as _,
                &EXECUTE,
                targets,
            );
        }
    }
}
pub unsafe fn capture(chain: &IDXGISwapChain, channel: &Channel, luid: u64) -> Result<()> {
    let Ok(device12) = chain.GetDevice::<ID3D12Device>() else {
        return Ok(());
    };
    let Ok(mut s) = STATE.try_lock() else {
        return Ok(());
    };
    let id = chain.as_raw() as usize;
    let desc = chain.GetDesc()?;
    let format = relay_hook_gpu::wire_format(desc.BufferDesc.Format)?.0;
    if s.capture.as_ref().is_none_or(|c| {
        c.chain != id
            || c.native_device != device12.as_raw() as usize
            || c.sender.desc.Width != desc.BufferDesc.Width
            || c.sender.desc.Height != desc.BufferDesc.Height
            || c.sender.desc.Format != format
    }) {
        let Some((_, queue)) = s.queues.iter().find(|(c, _)| *c == id) else {
            return Ok(());
        };
        let mut device = None;
        let mut context = None;
        D3D11On12CreateDevice(
            &device12,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT.0,
            None,
            Some(&[Some(queue.cast()?)]),
            0,
            Some(&mut device),
            Some(&mut context),
            None,
        )?;
        let device = device.context("11on12 device")?;
        relay_hook_gpu::check_adapter(&device, luid)?;
        let desc = chain.GetDesc()?;
        let sender = Sender::new(
            &device,
            desc.BufferDesc.Width,
            desc.BufferDesc.Height,
            desc.BufferDesc.Format,
        )?;
        s.capture = Some(Capture {
            chain: id,
            native_device: device12.as_raw() as usize,
            context: context.context("11on12 context")?,
            bridge: device.cast()?,
            device,
            sender,
        });
    }
    let c = s.capture.as_ref().unwrap();
    let swap3: IDXGISwapChain3 = chain.cast()?;
    let buffer: ID3D12Resource = chain.GetBuffer(swap3.GetCurrentBackBufferIndex())?;
    let mut wrapped: Option<ID3D11Texture2D> = None;
    c.bridge.CreateWrappedResource(
        &buffer,
        &D3D11_RESOURCE_FLAGS::default(),
        D3D12_RESOURCE_STATE_PRESENT,
        D3D12_RESOURCE_STATE_PRESENT,
        &mut wrapped as *mut Option<ID3D11Texture2D>,
    )?;
    let wrapped = wrapped.context("wrapped buffer")?;
    let resources = [Some(wrapped.cast::<ID3D11Resource>()?)];
    c.bridge.AcquireWrappedResources(&resources);
    let result = c.sender.copy(&c.context, &wrapped, channel, DXGI);
    c.bridge.ReleaseWrappedResources(&resources);
    c.context.Flush();
    let _ = &c.device;
    result?;
    Ok(())
}
