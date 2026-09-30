#![cfg(windows)]
#![allow(clippy::missing_safety_doc)]
#![allow(non_snake_case)]
mod capture;
use ash::{vk, vk::Handle};
use relay_hook_protocol::ipc::Channel;
use std::{
    collections::HashMap,
    ffi::{c_void, CStr},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
type Gipa = vk::PFN_vkGetInstanceProcAddr;
type Gdpa = vk::PFN_vkGetDeviceProcAddr;
type SetData = unsafe extern "system" fn(vk::Device, *mut c_void) -> vk::Result;
#[repr(C)]
struct Link {
    next: *mut Link,
    gipa: Gipa,
    proc: usize,
}
#[repr(C)]
struct LayerInfo {
    ty: vk::StructureType,
    next: *const c_void,
    function: u32,
    data: *mut Link,
}
#[repr(C)]
pub struct Negotiation {
    ty: u32,
    next: *mut c_void,
    version: u32,
    gipa: Option<Gipa>,
    gdpa: Option<Gdpa>,
    physical: Option<Gipa>,
}
struct Instance {
    api: ash::Instance,
    gipa: Gipa,
    physical_proc: Option<Gipa>,
    version: u32,
    surfaces: Mutex<HashMap<u64, u64>>,
}
struct Device {
    api: ash::Device,
    gdpa: Gdpa,
    instance: Arc<Instance>,
    physical: vk::PhysicalDevice,
    graphics: Vec<u32>,
    loader: Option<SetData>,
    enabled: bool,
    queues: Mutex<HashMap<u64, u32>>,
    chains: Mutex<HashMap<u64, Swapchain>>,
    retired: Mutex<Vec<capture::Capture>>,
}
struct Swapchain {
    window: u64,
    info: SwapInfo,
    session: Option<Session>,
    retry: Instant,
    disabled: bool,
}
#[derive(Clone)]
struct SwapInfo {
    format: vk::Format,
    extent: vk::Extent2D,
    images: Vec<vk::Image>,
    sharing: vk::SharingMode,
}
struct Session {
    channel: Option<Channel>,
    config: relay_hook_protocol::Header,
    gpu: capture::Capture,
    due: Instant,
}
static INSTANCES: OnceLock<Mutex<HashMap<usize, Arc<Instance>>>> = OnceLock::new();
static DEVICES: OnceLock<Mutex<HashMap<usize, Arc<Device>>>> = OnceLock::new();
static MARKER: OnceLock<relay_hook_protocol::ipc::Handle> = OnceLock::new();
fn instances() -> &'static Mutex<HashMap<usize, Arc<Instance>>> {
    INSTANCES.get_or_init(Default::default)
}
fn devices() -> &'static Mutex<HashMap<usize, Arc<Device>>> {
    DEVICES.get_or_init(Default::default)
}
unsafe fn key<T: Handle>(h: T) -> usize {
    let raw = h.as_raw();
    if raw == 0 {
        0
    } else {
        *(raw as *const usize)
    }
}
unsafe fn instance<T: Handle>(h: T) -> Option<Arc<Instance>> {
    instances().lock().ok()?.get(&key(h)).cloned()
}
unsafe fn device<T: Handle>(h: T) -> Option<Arc<Device>> {
    devices().lock().ok()?.get(&key(h)).cloned()
}
unsafe fn chain_info(
    mut next: *const c_void,
    ty: vk::StructureType,
    function: u32,
) -> *mut LayerInfo {
    while !next.is_null() {
        let header = &*(next as *const vk::BaseInStructure);
        if header.s_type == ty {
            let info = next as *mut LayerInfo;
            if (*info).function == function {
                return info;
            }
        }
        next = header.p_next.cast();
    }
    std::ptr::null_mut()
}
unsafe fn erase<T: Copy>(f: T) -> vk::PFN_vkVoidFunction {
    Some(std::mem::transmute_copy(&f))
}
#[no_mangle]
pub unsafe extern "system" fn vkNegotiateLoaderLayerInterfaceVersion(
    n: *mut Negotiation,
) -> vk::Result {
    if n.is_null() || (*n).version < 2 {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    (*n).version = 2;
    (*n).gipa = Some(vkGetInstanceProcAddr);
    (*n).gdpa = Some(vkGetDeviceProcAddr);
    (*n).physical = Some(physical_proc);
    vk::Result::SUCCESS
}
unsafe extern "system" fn physical_proc(
    i: vk::Instance,
    name: *const i8,
) -> vk::PFN_vkVoidFunction {
    instance(i).and_then(|s| (s.physical_proc.unwrap_or(s.gipa))(i, name))
}
#[no_mangle]
pub unsafe extern "system" fn vkGetInstanceProcAddr(
    i: vk::Instance,
    name: *const i8,
) -> vk::PFN_vkVoidFunction {
    if name.is_null() {
        return None;
    }
    match CStr::from_ptr(name).to_bytes() {
        b"vkGetInstanceProcAddr" => erase(vkGetInstanceProcAddr as Gipa),
        b"vkGetDeviceProcAddr" => erase(vkGetDeviceProcAddr as Gdpa),
        b"vkCreateInstance" => erase(create_instance as vk::PFN_vkCreateInstance),
        b"vkDestroyInstance" => erase(destroy_instance as vk::PFN_vkDestroyInstance),
        b"vkCreateDevice" => erase(create_device as vk::PFN_vkCreateDevice),
        b"vkCreateWin32SurfaceKHR" => erase(create_surface as vk::PFN_vkCreateWin32SurfaceKHR),
        b"vkDestroySurfaceKHR" => erase(destroy_surface as vk::PFN_vkDestroySurfaceKHR),
        _ => {
            if let Some(f) = device_hook(name) {
                f
            } else {
                instance(i).and_then(|s| (s.gipa)(i, name))
            }
        }
    }
}
unsafe fn device_hook(name: *const i8) -> Option<vk::PFN_vkVoidFunction> {
    Some(match CStr::from_ptr(name).to_bytes() {
        b"vkDeviceWaitIdle" => erase(device_idle as vk::PFN_vkDeviceWaitIdle),
        b"vkQueueWaitIdle" => erase(queue_idle as vk::PFN_vkQueueWaitIdle),
        b"vkDestroyDevice" => erase(destroy_device as vk::PFN_vkDestroyDevice),
        b"vkGetDeviceQueue" => erase(get_queue as vk::PFN_vkGetDeviceQueue),
        b"vkGetDeviceQueue2" => erase(get_queue2 as vk::PFN_vkGetDeviceQueue2),
        b"vkCreateSwapchainKHR" => erase(create_swapchain as vk::PFN_vkCreateSwapchainKHR),
        b"vkDestroySwapchainKHR" => erase(destroy_swapchain as vk::PFN_vkDestroySwapchainKHR),
        b"vkQueuePresentKHR" => erase(present as vk::PFN_vkQueuePresentKHR),
        _ => return None,
    })
}
#[no_mangle]
pub unsafe extern "system" fn vkGetDeviceProcAddr(
    d: vk::Device,
    name: *const i8,
) -> vk::PFN_vkVoidFunction {
    if name.is_null() {
        return None;
    }
    let s = device(d)?;
    let next = (s.gdpa)(d, name);
    next?;
    device_hook(name).unwrap_or(next)
}
unsafe extern "system" fn create_instance(
    info: *const vk::InstanceCreateInfo,
    a: *const vk::AllocationCallbacks,
    out: *mut vk::Instance,
) -> vk::Result {
    let link = chain_info(
        (*info).p_next,
        vk::StructureType::LOADER_INSTANCE_CREATE_INFO,
        0,
    );
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let node = (*link).data;
    let gipa = (*node).gipa;
    let physical_proc: Option<Gipa> = if (*node).proc == 0 {
        None
    } else {
        Some(std::mem::transmute::<usize, Gipa>((*node).proc))
    };
    (*link).data = (*node).next;
    let Some(f) = gipa(vk::Instance::null(), c"vkCreateInstance".as_ptr()) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let f: vk::PFN_vkCreateInstance = std::mem::transmute(f);
    let result = f(info, a, out);
    if result == vk::Result::SUCCESS {
        let api = ash::Instance::load(
            &ash::StaticFn {
                get_instance_proc_addr: gipa,
            },
            *out,
        );
        let version = if (*info).p_application_info.is_null() {
            vk::API_VERSION_1_0
        } else {
            (*(*info).p_application_info).api_version
        };
        if let Ok(mut map) = instances().lock() {
            map.insert(
                key(*out),
                Arc::new(Instance {
                    api,
                    gipa,
                    physical_proc,
                    version,
                    surfaces: Mutex::new(HashMap::new()),
                }),
            );
        }
        if MARKER.get().is_none() {
            if let Ok(marker) = relay_hook_protocol::ipc::vulkan_marker(std::process::id()) {
                let _ = MARKER.set(marker);
            }
        }
    }
    result
}
unsafe extern "system" fn destroy_instance(i: vk::Instance, a: *const vk::AllocationCallbacks) {
    let k = key(i);
    let s = instances().lock().ok().and_then(|mut m| m.remove(&k));
    if let Some(s) = s {
        s.api.destroy_instance(a.as_ref());
    }
}
unsafe extern "system" fn create_surface(
    i: vk::Instance,
    info: *const vk::Win32SurfaceCreateInfoKHR,
    a: *const vk::AllocationCallbacks,
    out: *mut vk::SurfaceKHR,
) -> vk::Result {
    let Some(s) = instance(i) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let f: vk::PFN_vkCreateWin32SurfaceKHR =
        std::mem::transmute((s.gipa)(i, c"vkCreateWin32SurfaceKHR".as_ptr()).unwrap());
    let result = f(i, info, a, out);
    if result == vk::Result::SUCCESS {
        if let Ok(mut m) = s.surfaces.lock() {
            m.insert((*out).as_raw(), (*info).hwnd as u64);
        }
    }
    result
}
unsafe extern "system" fn destroy_surface(
    i: vk::Instance,
    surface: vk::SurfaceKHR,
    a: *const vk::AllocationCallbacks,
) {
    if let Some(s) = instance(i) {
        if let Ok(mut m) = s.surfaces.lock() {
            m.remove(&surface.as_raw());
        }
        let f: vk::PFN_vkDestroySurfaceKHR =
            std::mem::transmute((s.gipa)(i, c"vkDestroySurfaceKHR".as_ptr()).unwrap());
        f(i, surface, a);
    }
}
unsafe extern "system" fn create_device(
    p: vk::PhysicalDevice,
    info: *const vk::DeviceCreateInfo,
    a: *const vk::AllocationCallbacks,
    out: *mut vk::Device,
) -> vk::Result {
    let Some(inst) = instance(p) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let link = chain_info(
        (*info).p_next,
        vk::StructureType::LOADER_DEVICE_CREATE_INFO,
        0,
    );
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let callback = chain_info(
        (*info).p_next,
        vk::StructureType::LOADER_DEVICE_CREATE_INFO,
        1,
    );
    let loader = if callback.is_null() {
        None
    } else {
        Some(std::mem::transmute::<*mut Link, SetData>((*callback).data))
    };
    let node = (*link).data;
    let gipa = (*node).gipa;
    let gdpa: Gdpa = std::mem::transmute((*node).proc);
    (*link).data = (*node).next;
    let Some(f) = gipa(inst.api.handle(), c"vkCreateDevice".as_ptr()) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let f: vk::PFN_vkCreateDevice = std::mem::transmute(f);
    let required = [
        ash::khr::external_memory_win32::NAME,
        ash::khr::win32_keyed_mutex::NAME,
    ];
    let supported = inst
        .api
        .enumerate_device_extension_properties(p)
        .unwrap_or_default();
    let name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let enabled = inst.version >= vk::API_VERSION_1_1
        && loader.is_some()
        && !relay_hook_protocol::blocked_name(&name)
        && required.iter().all(|n| {
            supported
                .iter()
                .any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == *n)
        });
    let mut adjusted = *info;
    let mut extensions = if (*info).enabled_extension_count == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(
            (*info).pp_enabled_extension_names,
            (*info).enabled_extension_count as usize,
        )
        .to_vec()
    };
    if enabled {
        for n in required {
            if !extensions.iter().any(|p| CStr::from_ptr(*p) == n) {
                extensions.push(n.as_ptr())
            }
        }
        adjusted.enabled_extension_count = extensions.len() as u32;
        adjusted.pp_enabled_extension_names = extensions.as_ptr();
    }
    let result = f(p, &adjusted, a, out);
    if result == vk::Result::SUCCESS {
        let api = ash::Device::load_with(
            |name| gdpa(*out, name.as_ptr()).map_or(std::ptr::null(), |f| f as *const c_void),
            *out,
        );
        let families = inst.api.get_physical_device_queue_family_properties(p);
        let graphics = std::slice::from_raw_parts(
            (*info).p_queue_create_infos,
            (*info).queue_create_info_count as usize,
        )
        .iter()
        .filter(|q| {
            families[q.queue_family_index as usize]
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS)
        })
        .map(|q| q.queue_family_index)
        .collect();
        if let Ok(mut m) = devices().lock() {
            m.insert(
                key(*out),
                Arc::new(Device {
                    api,
                    gdpa,
                    instance: inst,
                    physical: p,
                    graphics,
                    loader,
                    enabled,
                    queues: Mutex::new(HashMap::new()),
                    chains: Mutex::new(HashMap::new()),
                    retired: Mutex::new(Vec::new()),
                }),
            );
        }
    }
    result
}
unsafe extern "system" fn destroy_device(d: vk::Device, a: *const vk::AllocationCallbacks) {
    let k = key(d);
    let s = devices().lock().ok().and_then(|mut m| m.remove(&k));
    if let Some(s) = s {
        if let Ok(mut c) = s.chains.lock() {
            c.clear()
        }
        if let Ok(mut retired) = s.retired.lock() {
            retired.clear();
        }
        s.api.destroy_device(a.as_ref())
    }
}
unsafe extern "system" fn get_queue(d: vk::Device, family: u32, index: u32, out: *mut vk::Queue) {
    if let Some(s) = device(d) {
        (s.api.fp_v1_0().get_device_queue)(d, family, index, out);
        if let Ok(mut m) = s.queues.lock() {
            m.insert((*out).as_raw(), family);
        }
    }
}
unsafe extern "system" fn get_queue2(
    d: vk::Device,
    info: *const vk::DeviceQueueInfo2,
    out: *mut vk::Queue,
) {
    if let Some(s) = device(d) {
        (s.api.fp_v1_1().get_device_queue2)(d, info, out);
        if (*info).flags.is_empty() {
            if let Ok(mut m) = s.queues.lock() {
                m.insert((*out).as_raw(), (*info).queue_family_index);
            }
        }
    }
}
unsafe extern "system" fn create_swapchain(
    d: vk::Device,
    info: *const vk::SwapchainCreateInfoKHR,
    a: *const vk::AllocationCallbacks,
    out: *mut vk::SwapchainKHR,
) -> vk::Result {
    let Some(s) = device(d) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let f: vk::PFN_vkCreateSwapchainKHR =
        std::mem::transmute((s.gdpa)(d, c"vkCreateSwapchainKHR".as_ptr()).unwrap());
    let mut adjusted = *info;
    let get_caps: vk::PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR = std::mem::transmute(
        (s.instance.gipa)(
            s.instance.api.handle(),
            c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR".as_ptr(),
        )
        .unwrap(),
    );
    let mut caps = vk::SurfaceCapabilitiesKHR::default();
    let caps_ok = get_caps(s.physical, (*info).surface, &mut caps) == vk::Result::SUCCESS;
    let allowed = s.enabled
        && (*info).flags.is_empty()
        && (*info).image_array_layers == 1
        && caps_ok
        && caps
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_SRC);
    if allowed {
        adjusted.image_usage |= vk::ImageUsageFlags::TRANSFER_SRC
    }
    let result = f(d, &adjusted, a, out);
    if result == vk::Result::SUCCESS && allowed {
        let loader = ash::khr::swapchain::Device::new(&s.instance.api, &s.api);
        if let Ok(images) = loader.get_swapchain_images(*out) {
            let window = s
                .instance
                .surfaces
                .lock()
                .ok()
                .and_then(|m| m.get(&(*info).surface.as_raw()).copied())
                .unwrap_or(0);
            if window != 0 {
                if let Ok(mut m) = s.chains.lock() {
                    m.insert(
                        (*out).as_raw(),
                        Swapchain {
                            window,
                            info: SwapInfo {
                                format: (*info).image_format,
                                extent: (*info).image_extent,
                                images,
                                sharing: (*info).image_sharing_mode,
                            },
                            session: None,
                            retry: Instant::now(),
                            disabled: false,
                        },
                    );
                }
            }
        }
    }
    result
}
unsafe extern "system" fn destroy_swapchain(
    d: vk::Device,
    chain: vk::SwapchainKHR,
    a: *const vk::AllocationCallbacks,
) {
    if let Some(s) = device(d) {
        let removed = s
            .chains
            .lock()
            .ok()
            .and_then(|mut m| m.remove(&chain.as_raw()));
        if let Some(session) = removed.and_then(|chain| chain.session) {
            if let Ok(mut retired) = s.retired.lock() {
                retired.push(session.gpu);
            } else {
                std::mem::forget(session.gpu);
            }
        }
        let f: vk::PFN_vkDestroySwapchainKHR =
            std::mem::transmute((s.gdpa)(d, c"vkDestroySwapchainKHR".as_ptr()).unwrap());
        f(d, chain, a);
    }
}
unsafe extern "system" fn device_idle(d: vk::Device) -> vk::Result {
    let Some(s) = device(d) else {
        return vk::Result::ERROR_DEVICE_LOST;
    };
    let mut retired = s.retired.lock().ok();
    let result = (s.api.fp_v1_0().device_wait_idle)(d);
    if result == vk::Result::SUCCESS {
        if let Some(r) = retired.as_mut() {
            r.clear();
        }
    }
    result
}
unsafe extern "system" fn queue_idle(q: vk::Queue) -> vk::Result {
    let Some(s) = device(q) else {
        return vk::Result::ERROR_DEVICE_LOST;
    };
    let mut retired = s.retired.lock().ok();
    let result = (s.api.fp_v1_0().queue_wait_idle)(q);
    if result == vk::Result::SUCCESS {
        if let Some(r) = retired.as_mut() {
            r.retain(|c| !c.uses_queue(q));
        }
    }
    result
}
unsafe extern "system" fn present(queue: vk::Queue, info: *const vk::PresentInfoKHR) -> vk::Result {
    let Some(s) = device(queue) else {
        return vk::Result::ERROR_DEVICE_LOST;
    };
    let f: vk::PFN_vkQueuePresentKHR =
        std::mem::transmute((s.gdpa)(s.api.handle(), c"vkQueuePresentKHR".as_ptr()).unwrap());
    let mut signal = None;
    if !info.is_null() && (*info).swapchain_count == 1 && (*info).p_next.is_null() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let family = s
                .queues
                .lock()
                .ok()
                .and_then(|m| m.get(&queue.as_raw()).copied());
            let Some(family) = family else { return };
            let Ok(mut map) = s.chains.try_lock() else {
                return;
            };
            let Some(chain) = map.get_mut(&(*(*info).p_swapchains).as_raw()) else {
                return;
            };
            let now = Instant::now();
            if let Some(c) = chain.session.as_mut() {
                if c.channel.as_ref().is_some_and(|c| !c.alive()) {
                    c.channel = None;
                }
            }
            if chain.session.as_ref().is_none_or(|c| c.channel.is_none())
                && !chain.disabled
                && now >= chain.retry
            {
                chain.retry = now + Duration::from_millis(250);
                if let Ok(channel) = Channel::open(std::process::id()) {
                    if let Some(config) = channel.snapshot() {
                        if channel.accepts_window(chain.window) && channel.alive() {
                            if relay_hook_protocol::policy::allow(
                                std::process::id(),
                                chain.window as usize,
                            )
                            .is_err()
                            {
                                chain.disabled = true;
                                return;
                            }
                            if let Some(c) = chain.session.as_mut() {
                                if c.config.adapter_luid != config.adapter_luid {
                                    channel.stop();
                                    return;
                                }
                                c.channel = Some(channel);
                                c.config = config;
                                c.due = now;
                            } else {
                                if s.retired.lock().map_or(true, |r| r.len() >= 4) {
                                    channel.stop();
                                    chain.disabled = true;
                                    return;
                                }
                                match capture::Capture::new(
                                    &s,
                                    &chain.info,
                                    family,
                                    config.adapter_luid,
                                ) {
                                    Ok(gpu) => {
                                        chain.session = Some(Session {
                                            channel: Some(channel),
                                            config,
                                            gpu,
                                            due: now,
                                        })
                                    }
                                    Err(e) => {
                                        if std::env::var_os("RELAY_CAPTURE_DIAGNOSTICS").is_some() {
                                            eprintln!("Relay Vulkan: {e:#}");
                                        }
                                        channel.stop();
                                        chain.disabled = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(c) = chain.session.as_mut() {
                let Some(channel) = c.channel.as_ref() else {
                    return;
                };
                if !c.gpu.supports_family(family) {
                    channel.stop();
                    return;
                }
                if now < c.due {
                    return;
                }
                let dt = Duration::from_secs_f64(1.0 / c.config.fps.clamp(1, 120) as f64);
                c.due += dt;
                if c.due <= now {
                    c.due = now + dt
                }
                let waits = if (*info).wait_semaphore_count == 0 {
                    &[]
                } else {
                    std::slice::from_raw_parts(
                        (*info).p_wait_semaphores,
                        (*info).wait_semaphore_count as usize,
                    )
                };
                match c
                    .gpu
                    .submit(queue, *(*info).p_image_indices, waits, channel)
                {
                    Ok(sem) => signal = sem,
                    Err(e) => {
                        if std::env::var_os("RELAY_CAPTURE_DIAGNOSTICS").is_some() {
                            eprintln!("Relay Vulkan: {e:#}");
                        }
                        channel.stop();
                        chain.disabled = true;
                    }
                }
            }
        }));
    }
    if let Some(semaphore) = signal {
        let mut adjusted = *info;
        adjusted.wait_semaphore_count = 1;
        adjusted.p_wait_semaphores = &semaphore;
        f(queue, &adjusted)
    } else {
        f(queue, info)
    }
}
