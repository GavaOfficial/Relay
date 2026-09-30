use std::path::{Path, PathBuf};

pub const LAYER_FILE: &str = "relay-vk-layer.json";

pub fn manifest_path(runtime: &Path, arch: &str) -> PathBuf {
    runtime.join("hooks").join(arch).join(LAYER_FILE)
}

#[cfg(windows)]
mod registry {
    use std::path::Path;
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
        },
    };

    pub const KEY_64: &str = r"Software\Khronos\Vulkan\ImplicitLayers";
    pub const KEY_32: &str = r"Software\WOW6432Node\Khronos\Vulkan\ImplicitLayers";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open(path: &str) -> Result<Key, String> {
        let mut key: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("registro di Windows: errore {status} su {path}"));
        }
        Ok(Key(key))
    }

    fn value_names(key: &Key) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut buf = vec![0u16; 32768];
            let mut len = buf.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        names
    }

    pub fn apply(path: &str, keep: Option<&Path>) -> Result<(), String> {
        let key = open(path)?;
        let keep = keep.map(|p| p.to_string_lossy().into_owned());
        for name in value_names(&key) {
            if name.ends_with(super::LAYER_FILE) && keep.as_deref() != Some(name.as_str()) {
                unsafe {
                    RegDeleteValueW(key.0, wide(&name).as_ptr());
                }
            }
        }
        if let Some(manifest) = keep {
            let enabled = 0u32.to_le_bytes();
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    wide(&manifest).as_ptr(),
                    0,
                    REG_DWORD,
                    enabled.as_ptr(),
                    enabled.len() as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(format!("registro di Windows: errore {status} nel layer"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn values(path: &str) -> Vec<String> {
        open(path).map(|k| value_names(&k)).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn delete_tree(path: &str) {
        use windows_sys::Win32::System::Registry::RegDeleteTreeW;
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr());
        }
    }
}

#[cfg(windows)]
pub fn register(runtime: &Path) -> Result<(), String> {
    registry::apply(registry::KEY_64, Some(&manifest_path(runtime, "x64")))?;
    let x86 = manifest_path(runtime, "x86");
    registry::apply(registry::KEY_32, x86.is_file().then_some(x86.as_path()))
}

#[cfg(windows)]
pub fn unregister() -> Result<(), String> {
    registry::apply(registry::KEY_64, None)?;
    registry::apply(registry::KEY_32, None)
}

#[cfg(not(windows))]
pub fn register(_runtime: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn unregister() -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_layer_is_added_replaced_and_removed() {
        let root = format!(r"Software\RelayTest\vk-{}", std::process::id());
        let key = format!(r"{root}\ImplicitLayers");
        let old = Path::new(r"C:\vecchio\hooks\x64\relay-vk-layer.json");
        let new = Path::new(r"C:\nuovo\hooks\x64\relay-vk-layer.json");
        registry::apply(&key, Some(old)).unwrap();
        assert_eq!(
            registry::values(&key),
            vec![old.to_string_lossy().to_string()]
        );
        registry::apply(&key, Some(new)).unwrap();
        assert_eq!(
            registry::values(&key),
            vec![new.to_string_lossy().to_string()]
        );
        registry::apply(&key, Some(new)).unwrap();
        assert_eq!(registry::values(&key).len(), 1);
        registry::apply(&key, None).unwrap();
        assert!(registry::values(&key).is_empty());
        registry::delete_tree(&root);
    }

    #[test]
    fn manifests_live_next_to_their_dll() {
        let runtime = Path::new(r"C:\Relay\relay-capture");
        assert_eq!(
            manifest_path(runtime, "x86"),
            Path::new(r"C:\Relay\relay-capture\hooks\x86\relay-vk-layer.json")
        );
    }
}
