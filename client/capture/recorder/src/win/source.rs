use super::{capture, device::Gpu, hook, window::Handle};
use anyhow::Result;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

pub enum Source {
    Wgc(capture::Source),
    Hook(Box<hook::Source>),
}
impl Source {
    pub fn window(gpu: &Gpu, window: Handle) -> Result<Self> {
        Ok(Self::Wgc(capture::Source::window(gpu, window)?))
    }
    pub fn monitor(gpu: &Gpu, monitor: isize) -> Result<Self> {
        Ok(Self::Wgc(capture::Source::monitor(gpu, monitor)?))
    }
    pub fn hook(gpu: &Gpu, pid: u32, window: Handle, fps: u32) -> Result<Self> {
        Ok(Self::Hook(Box::new(hook::Source::start_at(
            gpu, pid, window, fps,
        )?)))
    }
    pub fn border(&self) -> bool {
        match self {
            Self::Wgc(s) => s.border,
            Self::Hook(_) => false,
        }
    }
    pub fn closed(&self) -> bool {
        match self {
            Self::Wgc(s) => s.closed(),
            Self::Hook(s) => s.closed(),
        }
    }
    pub fn frames(&self) -> u64 {
        match self {
            Self::Wgc(s) => s.frames(),
            Self::Hook(s) => s.frames(),
        }
    }
    pub fn last_frame(&self) -> Option<Instant> {
        match self {
            Self::Wgc(s) => s.last_frame(),
            Self::Hook(s) => s.last_frame(),
        }
    }
    pub fn frame(&self) -> Option<(ID3D11Texture2D, (u32, u32))> {
        match self {
            Self::Wgc(s) => s.frame(),
            Self::Hook(s) => s.frame(),
        }
    }
}
