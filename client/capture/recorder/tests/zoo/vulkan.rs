#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    unsafe { run() }
}
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
unsafe fn run() -> anyhow::Result<()> {
    use ash::{vk, Entry};
    use relay_hook_protocol::ipc::wide;
    use std::{
        ptr,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
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
    let class = wide("RelayZooVulkan");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&wc);
    let mut rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 768,
        bottom: 480,
    };
    AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0);
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        wide("Relay zoo Vulkan").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        0,
        0,
        rect.right - rect.left,
        rect.bottom - rect.top,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null(),
    );
    anyhow::ensure!(!hwnd.is_null(), "window");
    let entry = Entry::load()?;
    let extensions = [
        ash::khr::surface::NAME.as_ptr(),
        ash::khr::win32_surface::NAME.as_ptr(),
    ];
    let app = vk::ApplicationInfo::default()
        .application_name(c"Relay zoo Vulkan")
        .api_version(vk::API_VERSION_1_1);
    let instance = entry.create_instance(
        &vk::InstanceCreateInfo::default()
            .application_info(&app)
            .enabled_extension_names(&extensions),
        None,
    )?;
    let surface_api = ash::khr::surface::Instance::new(&entry, &instance);
    let surface = ash::khr::win32_surface::Instance::new(&entry, &instance).create_win32_surface(
        &vk::Win32SurfaceCreateInfoKHR::default()
            .hinstance(GetModuleHandleW(ptr::null()) as isize)
            .hwnd(hwnd as isize),
        None,
    )?;
    let mut chosen = None;
    for p in instance.enumerate_physical_devices()? {
        for (i, f) in instance
            .get_physical_device_queue_family_properties(p)
            .iter()
            .enumerate()
        {
            if f.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                && surface_api.get_physical_device_surface_support(p, i as u32, surface)?
            {
                chosen = Some((p, i as u32));
                break;
            }
        }
        if chosen.is_some() {
            break;
        }
    }
    let (physical, family) = chosen.ok_or_else(|| anyhow::anyhow!("no Vulkan GPU"))?;
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let extensions = [ash::khr::swapchain::NAME.as_ptr()];
    let device = instance.create_device(
        physical,
        &vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions),
        None,
    )?;
    let queue = device.get_device_queue(family, 0);
    let swap_api = ash::khr::swapchain::Device::new(&instance, &device);
    let caps = surface_api.get_physical_device_surface_capabilities(physical, surface)?;
    let formats = surface_api.get_physical_device_surface_formats(physical, surface)?;
    let format = formats
        .iter()
        .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
        .unwrap_or(&formats[0]);
    let extent = if caps.current_extent.width == u32::MAX {
        vk::Extent2D {
            width: 768,
            height: 480,
        }
    } else {
        caps.current_extent
    };
    let swap = swap_api.create_swapchain(
        &vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(
                (caps.min_image_count + 1).min(if caps.max_image_count == 0 {
                    u32::MAX
                } else {
                    caps.max_image_count
                }),
            )
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true),
        None,
    )?;
    let images = swap_api.get_swapchain_images(swap)?;
    let attachments = [vk::AttachmentDescription::default()
        .format(format.format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::DONT_CARE)
        .store_op(vk::AttachmentStoreOp::STORE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
    let color = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color)];
    let render = device.create_render_pass(
        &vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses),
        None,
    )?;
    let mut views = Vec::new();
    let mut framebuffers = Vec::new();
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    for image in &images {
        let view = device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .subresource_range(range),
            None,
        )?;
        views.push(view);
        framebuffers.push(
            device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render)
                    .attachments(&[view])
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )?,
        );
    }
    let pool = device.create_command_pool(
        &vk::CommandPoolCreateInfo::default()
            .queue_family_index(family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
        None,
    )?;
    let command = device.allocate_command_buffers(
        &vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .command_buffer_count(1),
    )?[0];
    let acquired = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
    let mut ready = Vec::new();
    for _ in &images {
        ready.push(device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?)
    }
    let fence = device.create_fence(
        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
        None,
    )?;
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
        device.wait_for_fences(&[fence], true, u64::MAX)?;
        let (index, _) =
            swap_api.acquire_next_image(swap, u64::MAX, acquired, vk::Fence::null())?;
        device.reset_fences(&[fence])?;
        device.reset_command_buffer(command, vk::CommandBufferResetFlags::empty())?;
        device.begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())?;
        device.cmd_begin_render_pass(
            command,
            &vk::RenderPassBeginInfo::default()
                .render_pass(render)
                .framebuffer(framebuffers[index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent,
                }),
            vk::SubpassContents::INLINE,
        );
        let count = (start.elapsed().as_millis() / 100) as u32;
        for bit in 0..24 {
            let value = if count & (1 << bit) != 0 {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 1.0, 1.0]
            };
            let x = bit * extent.width / 24;
            let width = (bit + 1) * extent.width / 24 - x;
            device.cmd_clear_attachments(
                command,
                &[vk::ClearAttachment::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .color_attachment(0)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: value },
                    })],
                &[vk::ClearRect::default()
                    .rect(vk::Rect2D {
                        offset: vk::Offset2D { x: x as i32, y: 0 },
                        extent: vk::Extent2D {
                            width,
                            height: extent.height,
                        },
                    })
                    .layer_count(1)],
            );
        }
        device.cmd_clear_attachments(
            command,
            &[vk::ClearAttachment::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .color_attachment(0)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue {
                        float32: [0.0, 1.0, 0.0, 1.0],
                    },
                })],
            &[vk::ClearRect::default()
                .rect(vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent: vk::Extent2D {
                        width: extent.width,
                        height: 16,
                    },
                })
                .layer_count(1)],
        );
        device.cmd_end_render_pass(command);
        device.end_command_buffer(command)?;
        device.queue_submit(
            queue,
            &[vk::SubmitInfo::default()
                .wait_semaphores(&[acquired])
                .wait_dst_stage_mask(&[vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT])
                .command_buffers(&[command])
                .signal_semaphores(&[ready[index as usize]])],
            fence,
        )?;
        swap_api.queue_present(
            queue,
            &vk::PresentInfoKHR::default()
                .wait_semaphores(&[ready[index as usize]])
                .swapchains(&[swap])
                .image_indices(&[index]),
        )?;
        std::thread::sleep(Duration::from_millis(2));
    }
    device.device_wait_idle()?;
    device.destroy_fence(fence, None);
    device.destroy_semaphore(acquired, None);
    for sem in ready {
        device.destroy_semaphore(sem, None)
    }
    device.destroy_command_pool(pool, None);
    for f in framebuffers {
        device.destroy_framebuffer(f, None)
    }
    for v in views {
        device.destroy_image_view(v, None)
    }
    device.destroy_render_pass(render, None);
    swap_api.destroy_swapchain(swap, None);
    device.destroy_device(None);
    surface_api.destroy_surface(surface, None);
    instance.destroy_instance(None);
    DestroyWindow(hwnd);
    Ok(())
}
