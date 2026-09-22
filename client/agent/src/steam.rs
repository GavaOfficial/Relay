use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SteamGame {
    pub app_id: Option<u32>,
    pub name: String,
    pub cover_url: Option<String>,
}

#[derive(Deserialize)]
struct CatalogGame {
    #[serde(rename = "steamAppID")]
    steam_app_id: Option<String>,
    #[serde(rename = "external")]
    name: String,
    thumb: Option<String>,
}

pub async fn search_catalog(query: &str) -> anyhow::Result<Vec<SteamGame>> {
    let query = query.trim().to_lowercase();
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let games = reqwest::Client::new()
        .get("https://www.cheapshark.com/api/1.0/games")
        .header(reqwest::header::USER_AGENT, "Relay/0.3 (https://github.com/GavaOfficial/Relay)")
        .query(&[("title", query.as_str()), ("limit", "30")])
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<CatalogGame>>()
        .await?;
    Ok(games
        .into_iter()
        .map(|game| SteamGame {
            app_id: game.steam_app_id.and_then(|id| id.parse().ok()),
            name: game.name,
            cover_url: game.thumb.map(|url| url.replace("http://", "https://")),
        })
        .collect())
}

fn vdf_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with(&needle) {
            continue;
        }
        let rest = &line[needle.len()..];
        let mut parts = rest.splitn(3, '"');
        parts.next();
        if let Some(value) = parts.next() {
            return Some(value.to_string());
        }
    }
    None
}

fn library_paths(steam_path: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam_path.to_path_buf()];
    if let Ok(text) =
        std::fs::read_to_string(steam_path.join("steamapps").join("libraryfolders.vdf"))
    {
        for line in text.lines() {
            let line = line.trim();
            if !line.starts_with("\"path\"") {
                continue;
            }
            if let Some(v) = vdf_value(line, "path") {
                out.push(PathBuf::from(v.replace("\\\\", "\\")));
            }
        }
    }
    out
}

fn app_name(steam_path: &Path, app_id: u32) -> Option<String> {
    for lib in library_paths(steam_path) {
        let manifest = lib
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"));
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            if let Some(name) = vdf_value(&text, "name") {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(windows)]
mod registry {
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
        REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open(subkey: &str) -> Option<HKEY> {
        let mut key: HKEY = std::ptr::null_mut();
        let ok = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(subkey).as_ptr(),
                0,
                KEY_READ,
                &mut key,
            )
        };
        (ok == ERROR_SUCCESS).then_some(key)
    }

    pub fn read_dword(subkey: &str, value: &str) -> Option<u32> {
        let key = open(subkey)?;
        let mut data: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let mut kind: u32 = 0;
        let ok = unsafe {
            RegQueryValueExW(
                key,
                wide(value).as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut len,
            )
        };
        unsafe { RegCloseKey(key) };
        (ok == ERROR_SUCCESS && kind == REG_DWORD).then_some(data)
    }

    pub fn read_string(subkey: &str, value: &str) -> Option<PathBuf> {
        let key = open(subkey)?;
        let mut buf = vec![0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let mut kind: u32 = 0;
        let ok = unsafe {
            RegQueryValueExW(
                key,
                wide(value).as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            )
        };
        unsafe { RegCloseKey(key) };
        if ok != ERROR_SUCCESS || kind != REG_SZ {
            return None;
        }
        let chars = (len as usize / 2).min(buf.len());
        let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
    }
}

#[cfg(windows)]
pub fn detect_current_game() -> Option<(u32, String)> {
    let app_id = registry::read_dword(r"Software\Valve\Steam\ActiveProcess", "RunningAppID")?;
    if app_id == 0 {
        return None;
    }
    let steam_path = registry::read_string(r"Software\Valve\Steam", "SteamPath")?;
    let name = app_name(&steam_path, app_id).unwrap_or_else(|| format!("App Steam {app_id}"));
    Some((app_id, name))
}

#[cfg(not(windows))]
pub fn detect_current_game() -> Option<(u32, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_quoted_value_from_a_vdf_line() {
        let text = "\t\t\"name\"\t\t\"Backrooms\"\n\t\t\"appid\"\t\t\"123\"";
        assert_eq!(vdf_value(text, "name"), Some("Backrooms".into()));
        assert_eq!(vdf_value(text, "appid"), Some("123".into()));
        assert_eq!(vdf_value(text, "missing"), None);
    }

    #[test]
    fn app_name_reads_from_any_library_folder() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path().join("Steam");
        let lib2 = dir.path().join("Games");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib2.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps").join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                lib2.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        std::fs::write(
            lib2.join("steamapps").join("appmanifest_570.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n}\n",
        )
        .unwrap();

        assert_eq!(app_name(&steam, 570), Some("Dota 2".into()));
        assert_eq!(app_name(&steam, 9999), None);
    }
}
