pub const VERSION: u32 = 3;
pub const GPU_ONLY: u32 = 1;
pub const CPU_ONLY: u32 = 2;
pub const MAGIC: u32 = 0x524c5948;
pub const CAPACITY: usize = 3840 * 2160 * 4;
pub const BGRA8: u32 = 1;
pub const RGBA8: u32 = 2;
pub const BGRX8: u32 = 3;
pub const OPENGL: u32 = 1;
pub const DXGI: u32 = 2;
pub const D3D9: u32 = 3;
pub const VULKAN: u32 = 4;

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Header {
    pub magic: u32,
    pub version: u32,
    pub header_bytes: u32,
    pub capacity: u32,
    pub owner_pid: u32,
    pub target_pid: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub api: u32,
    pub flipped: u32,
    pub texture_handle: u64,
    pub frames: u64,
    pub payload_bytes: u32,
    pub reserved: u32,
    pub target_hwnd: u64,
    pub adapter_luid: u64,
    pub texture_epoch: u64,
    pub fps: u32,
    pub padding: u32,
}

impl Header {
    pub fn gpu_valid(&self) -> bool {
        self.texture_handle != 0
            && self.texture_epoch != 0
            && matches!(self.format, BGRA8 | RGBA8 | BGRX8)
            && self.width > 0
            && self.height > 0
            && self.width <= 8192
            && self.height <= 8192
            && self.flipped == 0
            && self.payload_bytes == 0
    }
    pub fn valid(&self, pid: u32) -> bool {
        self.magic == MAGIC
            && self.version == VERSION
            && self.header_bytes as usize == std::mem::size_of::<Self>()
            && (self.capacity == 0 || self.capacity as usize == CAPACITY)
            && self.target_pid == pid
    }

    pub fn cpu_bytes(&self) -> Option<usize> {
        if self.width == 0
            || self.height == 0
            || self.width > 3840
            || self.height > 2160
            || self.format != BGRA8
            || self.flipped > 1
            || self.texture_handle != 0
        {
            return None;
        }
        let row = self.width.checked_mul(4)?;
        if self.stride != row {
            return None;
        }
        let bytes = (row as usize).checked_mul(self.height as usize)?;
        (bytes <= self.capacity as usize
            && bytes <= CAPACITY
            && bytes == self.payload_bytes as usize)
            .then_some(bytes)
    }
}

pub fn name(pid: u32, suffix: &str) -> String {
    format!("Local\\RelayHook_{pid}{suffix}")
}

pub fn blocked_name(name: &str) -> bool {
    let n = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    [
        "easyanticheat",
        "battleye",
        "bedaisy",
        "beclient",
        "beservice",
        "vgc",
        "vgk",
        "vanguard",
        "faceit",
        "equ8",
        "xigncode",
        "xhunter",
        "nprotect",
        "gameguard",
        "eac_",
    ]
    .iter()
    .any(|s| n.contains(s))
        || [
            "valorant-win64-shipping.exe",
            "fortniteclient-win64-shipping.exe",
            "cod.exe",
            "cod22-cod.exe",
            "r5apex.exe",
            "destiny2.exe",
            "rainbowsix.exe",
            "rainbowsix_vulkan.exe",
            "pubg.exe",
            "tslgame.exe",
        ]
        .contains(&n.as_str())
}

#[cfg(windows)]
pub mod ipc;
#[cfg(windows)]
pub mod policy;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_layout_and_bounds() {
        assert_eq!(std::mem::size_of::<Header>(), 104);
        assert_eq!(std::mem::offset_of!(Header, frames), 56);
        let mut h = Header {
            width: 1920,
            height: 1080,
            stride: 7680,
            format: BGRA8,
            payload_bytes: 1920 * 1080 * 4,
            capacity: CAPACITY as u32,
            ..Default::default()
        };
        assert_eq!(h.cpu_bytes(), Some(1920 * 1080 * 4));
        h.height = u32::MAX;
        assert_eq!(h.cpu_bytes(), None);
        h.height = 1080;
        h.stride += 4;
        assert_eq!(h.cpu_bytes(), None);
    }
    #[test]
    fn gpu_formats_do_not_change_cpu_layout() {
        let mut h = Header {
            width: 1280,
            height: 720,
            texture_handle: 42,
            texture_epoch: 1,
            ..Default::default()
        };
        for format in [BGRA8, RGBA8, BGRX8] {
            h.format = format;
            assert!(h.gpu_valid());
            assert!(h.cpu_bytes().is_none());
        }
        h.format = 999;
        assert!(!h.gpu_valid());
        h.format = BGRA8;
        h.payload_bytes = 4;
        assert!(!h.gpu_valid());
    }
    #[test]
    fn anticheat_paths_and_case() {
        assert!(blocked_name(r"C:\Games\EasyAntiCheat_EOS.dll"));
        assert!(blocked_name("VALORANT-Win64-Shipping.exe"));
        assert!(blocked_name("BEClient_x64.dll"));
        assert!(!blocked_name("Funkin.exe"));
    }
}
