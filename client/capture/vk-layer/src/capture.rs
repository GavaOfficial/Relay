use super::{Device, SwapInfo};
use anyhow::{ensure, Context, Result};
use ash::{vk, vk::Handle};
use relay_hook_gpu::Sender;
use relay_hook_protocol::{ipc::Channel, VULKAN};
use windows::Win32::Graphics::Dxgi::Common::*;
struct Frame {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    ready: vk::Semaphore,
}
pub struct Capture {
    device: ash::Device,
    sender: Sender,
    pool: vk::CommandPool,
    image: vk::Image,
    memory: vk::DeviceMemory,
    frames: Vec<Frame>,
    info: SwapInfo,
    family: u32,
    queue: Option<vk::Queue>,
}
impl Capture {
    pub fn uses_queue(&self,queue:vk::Queue)->bool{self.queue.is_none_or(|q|q==queue)}
    pub fn supports_family(&self,family:u32)->bool{self.family==family}
    pub unsafe fn new(s: &Device, info: &SwapInfo, family: u32, luid: u64) -> Result<Self> {
        let mut ids = vk::PhysicalDeviceIDProperties::default();
        let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
        s.instance
            .api
            .get_physical_device_properties2(s.physical, &mut props);
        ensure!(
            ids.device_luid_valid != 0 && u64::from_le_bytes(ids.device_luid) == luid,
            "GPU Vulkan diversa dal recorder"
        );
        ensure!(
            info.sharing == vk::SharingMode::CONCURRENT || s.graphics.as_slice() == [family],
            "ownership swapchain su piu famiglie di code"
        );
        let families = s
            .instance
            .api
            .get_physical_device_queue_family_properties(s.physical);
        ensure!(
            families
                .get(family as usize)
                .is_some_and(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)),
            "coda senza transfer"
        );
        let format = match info.format {
            vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => DXGI_FORMAT_B8G8R8A8_UNORM,
            vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => DXGI_FORMAT_R8G8B8A8_UNORM,
            _ => anyhow::bail!("formato Vulkan non supportato"),
        };
        let (d11, _) = relay_hook_gpu::device(luid)?;
        let sender = Sender::new(&d11, info.extent.width, info.extent.height, format)?;
        let device = s.api.clone();
        let mut c = Self {
            device,
            sender,
            pool: vk::CommandPool::null(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            frames: Vec::new(),
            info: info.clone(),
            family,
            queue:None,
        };
        let handle_type = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
        let vk_format = if format == DXGI_FORMAT_B8G8R8A8_UNORM {
            vk::Format::B8G8R8A8_UNORM
        } else {
            vk::Format::R8G8B8A8_UNORM
        };
        let mut external_info =
            vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(handle_type);
        let query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk_format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST)
            .push_next(&mut external_info);
        let mut external_props = vk::ExternalImageFormatProperties::default();
        let mut image_props = vk::ImageFormatProperties2::default().push_next(&mut external_props);
        s.instance
            .api
            .get_physical_device_image_format_properties2(s.physical, &query, &mut image_props)?;
        ensure!(
            external_props
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE),
            "texture D3D11 non importabile"
        );
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_type);
        c.image = c.device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk_format)
                .extent(vk::Extent3D {
                    width: info.extent.width,
                    height: info.extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .push_next(&mut external),
            None,
        )?;
        let requirements = c.device.get_image_memory_requirements(c.image);
        let extension = ash::khr::external_memory_win32::Device::new(&s.instance.api, &c.device);
        let mut handle_props = vk::MemoryWin32HandlePropertiesKHR::default();
        extension.get_memory_win32_handle_properties(
            handle_type,
            c.sender.handle.0 as isize,
            &mut handle_props,
        )?;
        let bits = requirements.memory_type_bits & handle_props.memory_type_bits;
        ensure!(bits != 0, "memoria Vulkan incompatibile");
        let mut import = vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(handle_type)
            .handle(c.sender.handle.0 as isize);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(c.image);
        c.memory = c.device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(bits.trailing_zeros())
                .push_next(&mut dedicated)
                .push_next(&mut import),
            None,
        )?;
        c.device.bind_image_memory(c.image, c.memory, 0)?;
        c.pool = c.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )?;
        let commands = c.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(c.pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(info.images.len() as u32),
        )?;
        for command in commands {
            ensure!(
                (s.loader.context("loader callback")?)(
                    c.device.handle(),
                    command.as_raw() as *mut std::ffi::c_void
                ) == vk::Result::SUCCESS,
                "dispatch command buffer"
            );
            let fence = c.device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )?;
            c.frames.push(Frame {
                command,
                fence,
                ready: vk::Semaphore::null(),
            });
            c.frames.last_mut().unwrap().ready = c
                .device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
        }
        Ok(c)
    }
    pub unsafe fn submit(
        &mut self,
        queue: vk::Queue,
        index: u32,
        waits: &[vk::Semaphore],
        channel: &Channel,
    ) -> Result<Option<vk::Semaphore>> {
        ensure!(self.uses_queue(queue),"coda di presentazione cambiata");
        self.queue=Some(queue);
        for f in &self.frames {
            if !self.device.get_fence_status(f.fence)? {
                return Ok(None);
            }
        }
        let f = self
            .frames
            .get(index as usize)
            .context("indice swapchain")?;
        if !self.sender.acquire() {
            return Ok(None);
        }
        let record = (|| -> Result<()> {
            self.device
                .reset_command_buffer(f.command, vk::CommandBufferResetFlags::empty())?;
            self.device.begin_command_buffer(
                f.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let source = vk::ImageMemoryBarrier::default()
                .image(self.info.images[index as usize])
                .subresource_range(range)
                .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
            let dest = vk::ImageMemoryBarrier::default()
                .image(self.image)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(self.family);
            self.device.cmd_pipeline_barrier(
                f.command,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[source, dest],
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            self.device.cmd_copy_image(
                f.command,
                self.info.images[index as usize],
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(layers)
                    .dst_subresource(layers)
                    .extent(vk::Extent3D {
                        width: self.info.extent.width,
                        height: self.info.extent.height,
                        depth: 1,
                    })],
            );
            let source = vk::ImageMemoryBarrier::default()
                .image(self.info.images[index as usize])
                .subresource_range(range)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
            let dest = vk::ImageMemoryBarrier::default()
                .image(self.image)
                .subresource_range(range)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(self.family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL);
            self.device.cmd_pipeline_barrier(
                f.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[source, dest],
            );
            self.device.end_command_buffer(f.command)?;
            self.device.reset_fences(&[f.fence])?;
            Ok(())
        })();
        if let Err(e) = record {
            let _ = self.sender.mutex.ReleaseSync(0);
            return Err(e);
        }
        self.sender.mutex.ReleaseSync(2)?;
        let memory = [self.memory];
        let acquire = [2u64];
        let release = [1u64];
        let timeouts = [0u32];
        let mut keyed = vk::Win32KeyedMutexAcquireReleaseInfoKHR::default()
            .acquire_syncs(&memory)
            .acquire_keys(&acquire)
            .acquire_timeouts(&timeouts)
            .release_syncs(&memory)
            .release_keys(&release);
        let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
        let commands = [f.command];
        let signal = [f.ready];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(waits)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&commands)
            .signal_semaphores(&signal)
            .push_next(&mut keyed);
        self.device.queue_submit(queue, &[submit], f.fence)?;
        self.sender.publish(channel, VULKAN);
        Ok(Some(f.ready))
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            for f in &self.frames {
                self.device.destroy_fence(f.fence, None);
                self.device.destroy_semaphore(f.ready, None)
            }
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
