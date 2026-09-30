use anyhow::{Context, Result};
use relay_hook_gpu::Sender;
use relay_hook_protocol::{ipc::Channel, DXGI};
use windows::{
    core::Interface,
    Win32::Graphics::{Direct3D10::*, Direct3D11::*, Dxgi::*},
};
struct Slot {
    texture: ID3D10Texture2D,
    import: ID3D11Texture2D,
    query10: ID3D10Query,
    query11: ID3D11Query,
    stage: u8,
    serial: u64,
}
pub struct Capture {
    device: ID3D10Device,
    context: ID3D11DeviceContext,
    slots: Vec<Slot>,
    serial: u64,
}
impl Capture {
    pub unsafe fn new(
        device: ID3D10Device,
        desc: D3D10_TEXTURE2D_DESC,
        luid: u64,
    ) -> Result<(Self, Sender)> {
        let (d11, context) = relay_hook_gpu::device(luid)?;
        let sender = Sender::new(&d11, desc.Width, desc.Height, desc.Format)?;
        let mut td = desc;
        td.SampleDesc.Count = 1;
        td.SampleDesc.Quality = 0;
        td.Usage = D3D10_USAGE_DEFAULT;
        td.CPUAccessFlags = 0;
        td.BindFlags = (D3D10_BIND_RENDER_TARGET.0 | D3D10_BIND_SHADER_RESOURCE.0) as u32;
        td.MiscFlags = D3D10_RESOURCE_MISC_SHARED.0 as u32;
        let mut slots = Vec::new();
        for _ in 0..3 {
            let texture = device
                .CreateTexture2D(&td, None)
                .context("texture D3D10 condivisa")?;
            let resource: IDXGIResource = texture.cast()?;
            let mut import = None;
            d11.OpenSharedResource(resource.GetSharedHandle()?, &mut import)?;
            let mut query10 = None;
            device.CreateQuery(
                &D3D10_QUERY_DESC {
                    Query: D3D10_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query10),
            )?;
            let query10 = query10.context("query10")?;
            let mut query11 = None;
            d11.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query11),
            )?;
            slots.push(Slot {
                texture,
                import: import.context("import D3D10")?,
                query10,
                query11: query11.context("query11")?,
                stage: 0,
                serial: 0,
            });
        }
        Ok((
            Self {
                device,
                context,
                slots,
                serial: 0,
            },
            sender,
        ))
    }
    pub unsafe fn publish(
        &mut self,
        source: &ID3D10Texture2D,
        desc: D3D10_TEXTURE2D_DESC,
        sender: &Sender,
        channel: &Channel,
    ) -> Result<()> {
        let mut order = [0, 1, 2];
        order.sort_by_key(|i| self.slots[*i].serial);
        for index in order {
            let slot = &mut self.slots[index];
            let mut done = 0u32;
            if slot.stage == 2 {
                let status = (self.context.vtable().GetData)(
                    self.context.as_raw(),
                    slot.query11.as_raw(),
                    (&mut done as *mut u32).cast(),
                    4,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                );
                if status.0 == 0 && done != 0 {
                    slot.stage = 0
                }
            } else if slot.stage == 1 {
                let status = (slot.query10.vtable().base__.GetData)(
                    slot.query10.as_raw(),
                    (&mut done as *mut u32).cast(),
                    4,
                    D3D10_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                );
                if status.0 != 0 || done == 0 {
                    break;
                }
                sender.copy(&self.context, &slot.import, channel, DXGI)?;
                self.context.End(&slot.query11);
                self.context.Flush();
                slot.stage = 2;
            }
        }
        if let Some(slot) = self.slots.iter_mut().find(|s| s.stage == 0) {
            if desc.SampleDesc.Count > 1 {
                self.device.ResolveSubresource(
                    &slot.texture,
                    0,
                    source,
                    0,
                    relay_hook_gpu::wire_format(desc.Format)?.0,
                )
            } else {
                self.device.CopyResource(&slot.texture, source)
            }
            slot.query10.End();
            self.device.Flush();
            self.serial += 1;
            slot.serial = self.serial;
            slot.stage = 1;
        }
        Ok(())
    }
}
