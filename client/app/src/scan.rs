use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;

use crate::{core::Core, psych::Kind};

const MAX_DEPTH: usize = 8;
const SKIP: &[&str] = &[
    "windows", "$recycle.bin", "system volume information", "programdata", "node_modules", ".git",
    "target", ".cargo", ".rustup", "temp", "tmp", "cache", "caches", "microsoft", "packages",
    "windowsapps", "winsxs", "driverstore", ".vscode", ".npm", ".nuget", "site-packages",
    "__pycache__", "recovery", "perflogs", "msocache", "config.msi",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Engine {
    Funkin,
    Codename,
    Psych,
    Nmv,
    Kade,
    Gd,
}

impl Engine {
    fn key(self) -> &'static str {
        match self {
            Engine::Funkin => "funkin",
            Engine::Codename => "codename",
            Engine::Psych => "psych",
            Engine::Nmv => "nmv",
            Engine::Kade => "kade",
            Engine::Gd => "gd",
        }
    }
}

#[derive(Serialize)]
pub struct Found {
    pub engine: &'static str,
    pub folder: String,
    pub name: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct ScanResult {
    pub found: Vec<Found>,
    pub other_engines: usize,
}

fn contains(data: &[u8], marker: &[u8]) -> bool {
    memchr::memmem::find(data, marker).is_some()
}

pub fn classify(dir: &Path) -> Option<Engine> {
    if crate::gd::is_game_dir(dir) {
        return Some(Engine::Gd);
    }
    let exes = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        n.ends_with(".exe") && !n.contains("crash") && !n.starts_with("unins")
    });
    let mut psych = false;
    let mut kade = false;
    for exe in exes {
        let Ok(data) = std::fs::read(&exe) else {
            continue;
        };
        if contains(&data, b"funkin.backend.system.Main") {
            return Some(Engine::Codename);
        }
        if contains(&data, b"funkin.play.PlayState") {
            return Some(Engine::Funkin);
        }
        if contains(&data, b"funkin.states.PlayState") {
            return Some(Engine::Nmv);
        }
        psych |= [&b"FunkinLua"[..], b"psychEngineVersion", b"PsychEngine"].iter().any(|m| contains(&data, m));
        kade |= crate::kade::is_kade_exe(&data);
    }
    if psych {
        Some(Engine::Psych)
    } else {
        kade.then_some(Engine::Kade)
    }
}

fn game_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = ('C'..='Z')
        .map(|d| PathBuf::from(format!("{d}:\\")))
        .filter(|p| p.is_dir())
        .map(|p| (p, 0))
        .collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let entries: Vec<_> = entries.flatten().collect();
        if entries.iter().any(|e| {
            e.file_name().eq_ignore_ascii_case("lime.ndll") || e.file_name().eq_ignore_ascii_case("GeometryDash.exe")
        }) {
            out.push(dir);
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        for e in entries {
            let Ok(ft) = e.file_type() else {
                continue;
            };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if SKIP.contains(&name.as_str()) || name.starts_with('.') && depth > 0 {
                continue;
            }
            stack.push((e.path(), depth + 1));
        }
    }
    out.sort();
    out
}

fn connected(core: &Arc<Core>, engine: Engine) -> Vec<String> {
    let base = core.data_dir();
    match engine {
        Engine::Funkin => crate::funkin::list_folders(&base),
        Engine::Codename => crate::fnf::list_folders(&base),
        Engine::Psych => crate::psych::list_folders(&base, Kind::Psych),
        Engine::Nmv => crate::psych::list_folders(&base, Kind::Nmv),
        Engine::Kade => crate::kade::list_folders(&base),
        Engine::Gd => crate::gd::list_folders(&base),
    }
}

fn connect(core: &Arc<Core>, engine: Engine, folder: &str) -> Result<(), String> {
    let base = core.data_dir();
    match engine {
        Engine::Funkin => crate::funkin::add_folder(core, folder.to_string()).map(|_| ()),
        Engine::Codename => crate::fnf::add_folder(&base, folder.to_string()).map(|_| ()),
        Engine::Psych => crate::psych::add_folder(core, Kind::Psych, folder.to_string()).map(|_| ()),
        Engine::Nmv => crate::psych::add_folder(core, Kind::Nmv, folder.to_string()).map(|_| ()),
        Engine::Kade => crate::kade::add_folder(core, folder.to_string()).map(|_| ()),
        Engine::Gd => crate::gd::add_folder(core, folder.to_string()).map(|_| ()),
    }
}

pub async fn scan_and_connect(core: Arc<Core>) -> Result<ScanResult, String> {
    let found = tokio::task::spawn_blocking(|| {
        game_dirs()
            .into_iter()
            .map(|d| {
                let e = classify(&d);
                (d, e)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut other = 0;
    for (dir, engine) in found {
        let Some(engine) = engine else {
            other += 1;
            continue;
        };
        let folder = dir.to_string_lossy().into_owned();
        let name = crate::modname::folder_name(&folder);
        let already = connected(&core, engine).iter().any(|f| {
            f.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(folder.trim_end_matches(['\\', '/']))
        });
        let (status, error) = if already {
            ("gia'", None)
        } else {
            match connect(&core, engine, &folder) {
                Ok(()) => ("nuova", None),
                Err(e) => ("errore", Some(e)),
            }
        };
        tracing::info!("ricerca mod: {folder} ({}) -> {status}", engine.key());
        out.push(Found { engine: engine.key(), folder, name, status, error });
    }
    Ok(ScanResult { found: out, other_engines: other })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_are_told_apart_by_their_classes() {
        let dir = tempfile::tempdir().unwrap();
        let game = |name: &str, content: &[u8]| {
            let d = dir.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("Game.exe"), content).unwrap();
            d
        };
        assert_eq!(classify(&game("cne", b"..funkin.backend.system.Main..Psych Engine..")), Some(Engine::Codename));
        assert_eq!(classify(&game("base", b"..funkin.play.PlayState..")), Some(Engine::Funkin));
        assert_eq!(classify(&game("nmv", b"..funkin.states.PlayState..FunkinLua..")), Some(Engine::Nmv));
        assert_eq!(classify(&game("psych", b"..states.PlayState..FunkinLua..")), Some(Engine::Psych));
        assert_eq!(classify(&game("kade", b"..KadeEngineData..")), None);
    }
}

