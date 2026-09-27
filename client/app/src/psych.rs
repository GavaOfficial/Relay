use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    core::Core,
    funkin::{asset_slug, read_json, upload_changed, write_if_changed, Built, MAX_ASSET_BYTES},
};

const SCRIPT: &str = include_str!("psych_script.lua.txt");
const SCRIPT_NAME: &str = "relay-integration.lua";
const NMV_SCRIPT: &str = include_str!("nmv_script.hx.txt");
const NMV_SCRIPT_NAME: &str = "relay-integration.hx";
const CHECK_EVERY: Duration = Duration::from_secs(60);
const NOT_MODS: &[&str] = &[
    "characters", "custom_events", "custom_notetypes", "data", "songs", "music", "sounds", "shaders",
    "videos", "images", "stages", "weeks", "fonts", "scripts", "achievements", "backgrounds",
];

fn folders_path(base: &Path, kind: Kind) -> PathBuf {
    base.join(match kind {
        Kind::Psych => "psych-folders.json",
        Kind::Nmv => "nmv-folders.json",
    })
}

pub fn list_folders(base: &Path, kind: Kind) -> Vec<String> {
    std::fs::read_to_string(folders_path(base, kind))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_folders(base: &Path, kind: Kind, folders: &[String]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(folders).map_err(|e| e.to_string())?;
    std::fs::write(folders_path(base, kind), json).map_err(|e| e.to_string())
}

fn exes(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            n.ends_with(".exe") && !n.contains("crash") && !n.starts_with("unins")
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Psych,
    Nmv,
}

impl Kind {
    pub fn engine(self) -> &'static str {
        match self {
            Kind::Psych => "psych",
            Kind::Nmv => "nmv",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Psych => "Psych Engine",
            Kind::Nmv => "Nightmare Vision",
        }
    }
}

fn contains(data: &[u8], marker: &[u8]) -> bool {
    memchr::memmem::find(data, marker).is_some()
}

fn detect(dir: &Path) -> Option<Kind> {
    for exe in exes(dir) {
        let Ok(data) = std::fs::read(&exe) else {
            continue;
        };
        if contains(&data, b"funkin.states.PlayState") {
            return Some(Kind::Nmv);
        }
        if [&b"FunkinLua"[..], b"psychEngineVersion", b"PsychEngine"].iter().any(|m| contains(&data, m)) {
            return Some(Kind::Psych);
        }
    }
    None
}

fn kind_of(dir: &Path) -> Option<Kind> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, Option<Kind>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(k) = cache.lock().unwrap().get(dir) {
        return *k;
    }
    let k = detect(dir);
    cache.lock().unwrap().insert(dir.to_path_buf(), k);
    k
}

fn mods_dir(game_dir: &Path, kind: Kind) -> PathBuf {
    let content = game_dir.join("content");
    let uses_mods = game_dir.join("mods").is_dir() || game_dir.join("modsList.txt").is_file();
    if kind == Kind::Nmv && (content.is_dir() || !uses_mods) {
        content
    } else {
        game_dir.join("mods")
    }
}

fn global_scripts(dir: &Path) -> bool {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(dir) {
        return *v;
    }
    let exes = exes(dir);
    let v = exes.is_empty() || exes.iter().any(|e| std::fs::read(e).is_ok_and(|d| contains(&d, b"scripts/")));
    cache.lock().unwrap().insert(dir.to_path_buf(), v);
    v
}

const SONG_BEGIN: &str = "-- RELAY-INTEGRATION-BEGIN";
const SONG_END: &str = "-- RELAY-INTEGRATION-END";
const SONG_COPY: &str = "-- RELAY-COPY: copia dello script della canzone con in fondo quello di Relay";

fn song_ids(game_dir: &Path) -> Vec<String> {
    let mut ids = std::collections::BTreeSet::new();
    for root in [game_dir.join("assets"), game_dir.join("mods")] {
        for f in std::fs::read_dir(root.join("data").join("songData")).into_iter().flatten().flatten() {
            if f.path().is_dir() {
                ids.insert(f.file_name().to_string_lossy().to_lowercase());
            }
        }
        for f in std::fs::read_dir(root.join("data")).into_iter().flatten().flatten() {
            let id = f.file_name().to_string_lossy().to_lowercase();
            let chart = std::fs::read_dir(f.path()).into_iter().flatten().flatten().any(|c| {
                let n = c.file_name().to_string_lossy().to_lowercase();
                n.ends_with(".json") && n.starts_with(&id)
            });
            if f.path().is_dir() && chart && id != "songdata" {
                ids.insert(id);
            }
        }
    }
    ids.into_iter().collect()
}

fn strip_block(text: &str) -> String {
    match (text.find(SONG_BEGIN), text.find(SONG_END)) {
        (Some(a), Some(b)) if b > a => format!("{}{}", &text[..a], &text[b + SONG_END.len()..]).trim_end().to_string(),
        _ => text.to_string(),
    }
}

fn install_song_scripts(game_dir: &Path) -> std::io::Result<()> {
    for id in song_ids(game_dir) {
        let target = game_dir.join("mods").join("data").join("songData").join(&id).join("script.lua");
        let original = std::fs::read_to_string(game_dir.join("assets").join("data").join("songData").join(&id).join("script.lua")).ok();
        let base = match std::fs::read_to_string(&target) {
            Ok(t) if t.starts_with(SONG_COPY) => format!("{SONG_COPY}
{}", original.clone().unwrap_or_default()),
            Ok(t) => strip_block(&t),
            Err(_) => format!("{SONG_COPY}
{}", original.unwrap_or_default()),
        };
        let text = format!("{}

{SONG_BEGIN}
{SCRIPT}
{SONG_END}
", base.trim_end());
        write_if_changed(&target, &text)?;
    }
    Ok(())
}

fn uninstall_song_scripts(game_dir: &Path) {
    let root = game_dir.join("mods").join("data").join("songData");
    for dir in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let path = dir.path().join("script.lua");
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if text.starts_with(SONG_COPY) {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(dir.path());
        } else if text.contains(SONG_BEGIN) {
            let _ = std::fs::write(&path, strip_block(&text) + "
");
        }
    }
}

fn install(game_dir: &Path, kind: Kind) -> Result<(), String> {
    let mods = mods_dir(game_dir, kind);
    if !mods.is_dir() && !game_dir.join("modsList.txt").is_file() {
        return Err("Questa mod non carica script esterni (manca la cartella delle mod): Relay non puo' collegarla.".into());
    }
    let (name, script) = match kind {
        Kind::Psych => (SCRIPT_NAME, SCRIPT),
        Kind::Nmv => (NMV_SCRIPT_NAME, NMV_SCRIPT),
    };
    let result = if kind == Kind::Psych && !global_scripts(game_dir) {
        install_song_scripts(game_dir)
    } else {
        write_if_changed(&mods.join("scripts").join(name), script)
    };
    result
        .and_then(|_| std::fs::create_dir_all(game_dir.join("relay")))
        .map_err(|e| format!("non riesco a installare lo script di Relay: {e}"))
}

fn uninstall(game_dir: &Path) {
    for dir in ["mods", "content"] {
        for name in [SCRIPT_NAME, NMV_SCRIPT_NAME] {
            let _ = std::fs::remove_file(game_dir.join(dir).join("scripts").join(name));
        }
    }
    uninstall_song_scripts(game_dir);
    let _ = std::fs::remove_dir_all(game_dir.join("relay"));
}

pub fn add_folder(core: &Arc<Core>, kind: Kind, folder: String) -> Result<Vec<String>, String> {
    let folder = folder.trim().to_string();
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Err("La cartella scelta non esiste.".into());
    }
    match kind_of(&dir) {
        Some(k) if k == kind => {}
        Some(other) => {
            return Err(format!(
                "Questa mod usa {}: collegala dal connettore {}.",
                other.label(),
                other.label()
            ))
        }
        None => {
            return Err(format!(
                "Non sembra una mod fatta con {}: scegli la cartella con il suo .exe.",
                kind.label()
            ))
        }
    }
    install(&dir, kind)?;
    let base = core.data_dir();
    let mut folders = list_folders(&base, kind);
    if !folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
        folders.push(folder.clone());
    }
    save_folders(&base, kind, &folders)?;
    crate::funkin::start_tail(core, &folder);
    let core = core.clone();
    tauri::async_runtime::spawn(async move { sync_catalogs(&core, Path::new(&folder), kind).await });
    Ok(folders)
}

pub fn remove_folder(base: &Path, kind: Kind, folder: &str) -> Result<Vec<String>, String> {
    uninstall(Path::new(folder));
    let mut folders = list_folders(base, kind);
    folders.retain(|f| f != folder);
    save_folders(base, kind, &folders)?;
    Ok(folders)
}

use crate::modname::folder_name;

pub fn mod_identity(game_dir: &Path, kind: Kind, mod_folder: &str) -> (String, Vec<String>) {
    let all = sources(game_dir, kind);
    let src = all
        .iter()
        .skip(1)
        .find(|s| !mod_folder.is_empty() && s.folder.eq_ignore_ascii_case(mod_folder))
        .unwrap_or(&all[0]);
    (src.name.clone(), src.aliases.clone())
}

pub fn song_path(name: &str) -> String {
    name.trim()
        .replace(' ', "-")
        .chars()
        .filter_map(|c| match c {
            '~' | '&' | '\\' | ';' | ':' | '<' | '>' | '#' => Some('-'),
            '.' | ',' | '\'' | '"' | '%' | '?' | '!' => None,
            c => Some(c),
        })
        .collect::<String>()
        .to_lowercase()
}

fn lenient_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }
    let mut out = String::with_capacity(text.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in text.chars() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                '\n' => {
                    out.push_str("\\n");
                    continue;
                }
                '\r' | '\t' => continue,
                _ => {}
            }
        } else if c == '"' {
            in_str = true;
        }
        out.push(c);
    }
    serde_json::from_str(&out).ok()
}

struct Source {
    name: String,
    aliases: Vec<String>,
    folder: String,
    title: String,
    roots: Vec<PathBuf>,
    weeks: Vec<PathBuf>,
    pack: Option<PathBuf>,
    credits: Vec<PathBuf>,
}

fn enabled_mods(game_dir: &Path) -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(game_dir.join("modsList.txt")).ok()?;
    Some(
        text.lines()
            .filter_map(|l| {
                let (name, on) = l.trim().rsplit_once('|')?;
                (on.trim() == "1").then(|| name.to_lowercase())
            })
            .collect(),
    )
}

fn sources(game_dir: &Path, kind: Kind) -> Vec<Source> {
    let mods = mods_dir(game_dir, kind);
    let assets = game_dir.join("assets");
    let shared = assets.join("shared");
    let game_folder = folder_name(&game_dir.to_string_lossy());
    let game = crate::modname::game_name(game_dir);
    let mut out = vec![Source {
        aliases: crate::modname::aliases(&game, &[&game_folder]),
        name: game,
        title: crate::modname::strip_version(&game_folder),
        folder: game_folder,
        roots: vec![mods.clone(), shared.clone(), assets.clone()],
        weeks: vec![
            shared.join("weeks"),
            assets.join("weeks"),
            shared.join("data").join("weeks"),
            assets.join("data").join("weeks"),
            mods.join("weeks"),
        ],
        pack: Some(mods.join("pack.json")),
        credits: vec![mods.join("data").join("credits.txt")],
    }];
    let enabled = enabled_mods(game_dir);
    let mut inner: Vec<PathBuf> = std::fs::read_dir(&mods)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            !NOT_MODS.contains(&n.as_str()) && enabled.as_ref().is_none_or(|on| on.contains(&n))
        })
        .map(|e| e.path())
        .collect();
    inner.sort();
    for dir in inner {
        let folder = folder_name(&dir.to_string_lossy());
        let title = std::fs::read_to_string(dir.join("pack.json"))
            .ok()
            .and_then(|t| lenient_json(&t))
            .and_then(|p| p["name"].as_str().map(|n| n.trim().to_string()))
            .filter(|n| !n.is_empty() && n != "Name")
            .unwrap_or_else(|| folder.clone());
        let name = crate::modname::strip_version(&title);
        out.push(Source {
            aliases: crate::modname::aliases(&name, &[&folder]),
            title: name.clone(),
            name,
            folder,
            roots: vec![dir.clone(), mods.clone(), shared.clone(), assets.clone()],
            weeks: vec![dir.join("weeks"), dir.join("data").join("weeks")],
            pack: Some(dir.join("pack.json")),
            credits: vec![dir.join("data").join("credits.txt")],
        });
    }
    out
}

fn find(roots: &[PathBuf], rel: &Path) -> Option<PathBuf> {
    roots.iter().map(|r| r.join(rel)).find(|p| p.is_file())
}

fn week_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    let order: Vec<String> = std::fs::read_to_string(dir.join("weekList.txt"))
        .map(|t| t.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    files.sort_by_key(|p| {
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let pos = order.iter().position(|o| *o == stem).unwrap_or(usize::MAX);
        (pos, crate::funkin::natural_key(&stem))
    });
    files
}

fn chart_info(roots: &[PathBuf], id: &str) -> (Option<f64>, Vec<String>) {
    let bpm_of = |p: &PathBuf| {
        let v = read_json(p)?;
        v["song"]["bpm"].as_f64().or(v["bpm"].as_f64())
    };
    let jsons = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        v.sort_by_key(|p| p.file_name().map(|n| n.len()));
        v
    };
    if let Some(dir) = roots.iter().map(|r| r.join("songs").join(id).join("data")).find(|p| p.is_dir()) {
        let charts: Vec<PathBuf> = jsons(&dir)
            .into_iter()
            .filter(|p| !p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("events")))
            .collect();
        let diffs = charts
            .iter()
            .filter_map(|p| {
                let s = p.file_stem()?.to_string_lossy().into_owned();
                let mut c = s.chars();
                Some(c.next()?.to_uppercase().collect::<String>() + c.as_str())
            })
            .collect();
        return (charts.iter().find_map(bpm_of), diffs);
    }
    let Some(dir) = roots.iter().map(|r| r.join("data").join(id)).find(|p| p.is_dir()) else {
        return (None, Vec::new());
    };
    let charts: Vec<PathBuf> = jsons(&dir)
        .into_iter()
        .filter(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase().starts_with(id))
        .collect();
    (charts.iter().find_map(bpm_of), Vec::new())
}

fn logo_frame(roots: &[PathBuf]) -> Option<PathBuf> {
    let png = find(roots, &Path::new("images").join("logoBumpin.png"))?;
    let xml = std::fs::read_to_string(png.with_extension("xml")).ok()?;
    let tag = xml.split("<SubTexture").nth(1)?;
    let attr = |name: &str| -> Option<u32> {
        let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
        tag[start..].split('"').next()?.trim().parse().ok()
    };
    let (x, y, w, h) = (attr("x")?, attr("y")?, attr("width")?, attr("height")?);
    let img = image::open(&png).ok()?;
    if w == 0 || h == 0 || x + w > img.width() || y + h > img.height() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&png, &mut hasher);
    let out = std::env::temp_dir()
        .join("relay-assets")
        .join(format!("logo-{:016x}.png", std::hash::Hasher::finish(&hasher)));
    std::fs::create_dir_all(out.parent()?).ok()?;
    img.crop_imm(x, y, w, h).save(&out).ok()?;
    Some(out)
}

fn color_hex(v: &Value) -> Option<String> {
    let a = v.as_array()?;
    let c = |i: usize| a.get(i).and_then(|x| x.as_f64()).map(|x| x.clamp(0.0, 255.0) as u8);
    Some(format!("#{:02x}{:02x}{:02x}", c(0)?, c(1)?, c(2)?))
}

fn read_credits(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split("::").map(str::trim).collect();
            if parts.len() < 3 || parts[0].is_empty() {
                return None;
            }
            let url = parts.get(3).filter(|u| u.starts_with("http")).copied();
            let role = Some(parts[2]).filter(|r| !r.is_empty());
            Some(json!({ "name": parts[0], "role": role, "url": url }))
        })
        .collect()
}

fn build_catalog(src: &Source) -> Option<Built> {
    let mut assets: Vec<(String, PathBuf)> = Vec::new();
    let add_asset = |name: String, path: PathBuf, assets: &mut Vec<(String, PathBuf)>| {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_ASSET_BYTES) {
            if !assets.iter().any(|(n, _)| *n == name) {
                assets.push((name.clone(), path));
            }
            Some(name)
        } else {
            None
        }
    };
    let mut albums = Vec::new();
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();
    let mut seen_weeks = HashSet::new();
    let mut sections: Vec<String> = Vec::new();
    for dir in &src.weeks {
        for file in week_files(dir) {
            let stem = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if !seen_weeks.insert(stem.to_lowercase()) {
                continue;
            }
            let Some(week) = std::fs::read_to_string(&file).ok().and_then(|t| lenient_json(&t)) else {
                continue;
            };
            if week["hideFreeplay"].as_bool() == Some(true) {
                continue;
            }
            let songs = week["songs"].as_array().cloned().unwrap_or_default();
            if songs.is_empty() {
                continue;
            }
            let week_difficulties: Option<Vec<String>> = week["difficulties"]
                .as_str()
                .map(|d| d.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                .filter(|d: &Vec<String>| !d.is_empty());
            let section = week["section"].as_str().map(str::to_string);
            if let Some(s) = &section {
                if !sections.iter().any(|x: &String| x == s) {
                    sections.push(s.clone());
                }
            }
            let art = find(&src.roots, &Path::new("images").join("storymenu").join(format!("{stem}.png")))
                .and_then(|p| add_asset(format!("week-{}.png", asset_slug(&stem)), p, &mut assets));
            let story = week["storyName"].as_str().unwrap_or("").trim();
            let week_name = week["weekName"].as_str().unwrap_or("").trim();
            let name =[story, week_name].into_iter().find(|n| !n.is_empty()).unwrap_or(&stem).to_string();
            albums.push(json!({
                "id": stem,
                "name": name,
                "artists": if !story.is_empty() && !week_name.is_empty() && story != week_name { vec![week_name.to_string()] } else { vec![] },
                "art": art,
            }));
            for song in songs {
                let Some(title) = song[0].as_str() else {
                    continue;
                };
                let id = song_path(title);
                if id.is_empty() || !seen.insert(id.clone()) {
                    continue;
                }
                let icon = song[1].as_str().and_then(|icon| {
                    let dir = Path::new("images").join("icons");
                    find(&src.roots, &dir.join(format!("icon-{icon}.png")))
                        .or_else(|| find(&src.roots, &dir.join(format!("{icon}.png"))))
                        .and_then(|p| add_asset(format!("hi-{}.png", asset_slug(icon)), p, &mut assets))
                });
                let (bpm, chart_diffs) = chart_info(&src.roots, &id);
                let difficulties = week_difficulties
                    .clone()
                    .or(Some(chart_diffs).filter(|d| !d.is_empty()))
                    .unwrap_or_else(|| vec!["Easy".into(), "Normal".into(), "Hard".into()]);
                let portrait = song[3].as_str().and_then(|p| {
                    let path = Path::new("images").join("menu").join("freeplay").join("portraits").join(format!("{p}.png"));
                    find(&src.roots, &path).and_then(|f| add_asset(format!("portrait-{}.png", asset_slug(p)), f, &mut assets))
                });
                let composers = song[4].as_str().map(str::trim).filter(|c| !c.is_empty());
                let mut extra = serde_json::Map::new();
                if let Some(p) = portrait {
                    extra.insert("portrait".into(), json!(p));
                }
                if let Some(c) = composers {
                    extra.insert("composers".into(), json!(c));
                }
                if let Some(t) = song[3].as_f64() {
                    extra.insert("threat".into(), json!(t.clamp(0.0, 100.0)));
                }
                if let Some(s) = &section {
                    extra.insert("section".into(), json!(s));
                }
                tracks.push(json!({
                    "id": id,
                    "name": title,
                    "album": stem,
                    "bpm": bpm,
                    "difficulties": difficulties,
                    "icon": icon,
                    "color": color_hex(&song[2]),
                    "extra": if extra.is_empty() { Value::Null } else { Value::Object(extra) },
                }));
            }
        }
    }
    let pack = src.pack.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| lenient_json(&t));
    let icon = src.pack.as_ref().map(|p| p.with_file_name("pack.png"));
    let logo = logo_frame(&src.roots).and_then(|p| add_asset("logo.png".into(), p, &mut assets));
    let has_icon = icon
        .clone()
        .and_then(|p| add_asset("icon.png".into(), p, &mut assets))
        .or_else(|| {
            let (_, path) = assets.iter().find(|(n, _)| n == "logo.png")?.clone();
            add_asset("icon.png".into(), path, &mut assets)
        })
        .is_some();
    if tracks.is_empty() && pack.is_none() {
        return None;
    }
    let sections: Vec<Value> = sections
        .iter()
        .map(|id| {
            let data = [Path::new("data").join("weeks").join("freeplay").join(format!("{id}.json"))]
                .iter()
                .find_map(|rel| find(&src.roots, rel))
                .and_then(|p| read_json(&p))
                .unwrap_or(Value::Null);
            let icon = find(&src.roots, &Path::new("images").join("menu").join("freeplay").join("sections").join(format!("{id}.png")))
                .and_then(|p| add_asset(format!("section-{}.png", asset_slug(id)), p, &mut assets));
            json!({ "id": id, "title": data["title"].as_str().unwrap_or(id), "index": data["index"].as_i64(), "icon": icon })
        })
        .collect();
    let mut catalog_extra = serde_json::Map::new();
    if let Some(l) = logo {
        catalog_extra.insert("logo".into(), json!(l));
    }
    if !sections.is_empty() {
        catalog_extra.insert("sections".into(), json!(sections));
    }
    let pack = pack.unwrap_or(Value::Null);
    let contributors: Vec<Value> = src.credits.iter().flat_map(|p| read_credits(p)).collect();
    let catalog = json!({
        "title": pack["name"].as_str().map(str::trim).filter(|n| !n.is_empty() && *n != "Name").unwrap_or(&src.title),
        "description": pack["description"].as_str().map(str::trim).filter(|d| !d.is_empty()),
        "version": pack["version"].as_str(),
        "contributors": contributors,
        "has_icon": has_icon,
        "albums": albums,
        "tracks": tracks,
        "extra": if catalog_extra.is_empty() { Value::Null } else { Value::Object(catalog_extra) },
    });
    Some(Built { catalog, assets, aliases: src.aliases.clone() })
}

async fn sync_catalogs(core: &Arc<Core>, game_dir: &Path, kind: Kind) {
    let dir = game_dir.to_path_buf();
    let Ok(builts) = tokio::task::spawn_blocking(move || {
        sources(&dir, kind)
            .iter()
            .filter_map(|s| Some((s.name.clone(), build_catalog(s)?)))
            .collect::<Vec<_>>()
    })
    .await
    else {
        return;
    };
    upload_changed(core, kind.engine(), game_dir, builts).await;
}

pub fn spawn(core: Arc<Core>) {
    for kind in [Kind::Psych, Kind::Nmv] {
        for folder in list_folders(&core.data_dir(), kind) {
            crate::funkin::start_tail(&core, &folder);
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        loop {
            let all: Vec<(Kind, String)> = [Kind::Psych, Kind::Nmv]
                .into_iter()
                .flat_map(|k| list_folders(&core.data_dir(), k).into_iter().map(move |f| (k, f)))
                .collect();
            for (kind, folder) in all {
                let dir = PathBuf::from(&folder);
                if let Err(e) = install(&dir, kind) {
                    tracing::warn!("script di Relay non aggiornato in {folder}: {e}");
                    continue;
                }
                let events = dir.join("relay").join("events.jsonl");
                if std::fs::metadata(&events).is_ok_and(|m| m.len() > 1024 * 1024) {
                    let _ = std::fs::write(&events, "");
                }
                sync_catalogs(&core, &dir, kind).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn old_psych_gets_the_script_at_the_end_of_each_song_script() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        write(g.join("Mario.exe"), "..FunkinLua..psychEngineVersion..");
        write(g.join("mods/readme.txt"), "");
        write(g.join("assets/data/alone/alone-hard.json"), "{}");
        write(g.join("assets/data/songData/alone/script.lua"), "function onEndSong() return Function_Stop end
");
        write(g.join("assets/data/unbeatable/unbeatable.json"), "{}");
        assert!(!global_scripts(g));
        install(g, Kind::Psych).unwrap();
        let alone = std::fs::read_to_string(g.join("mods/data/songData/alone/script.lua")).unwrap();
        assert!(alone.starts_with(SONG_COPY));
        assert!(alone.contains("function onEndSong() return Function_Stop end"), "lo script della canzone resta");
        assert!(alone.contains(SONG_BEGIN) && alone.contains("relayPrev"));
        assert!(g.join("mods/data/songData/unbeatable/script.lua").exists(), "anche le canzoni senza script");
        install(g, Kind::Psych).unwrap();
        let again = std::fs::read_to_string(g.join("mods/data/songData/alone/script.lua")).unwrap();
        assert_eq!(again.matches(SONG_BEGIN).count(), 1, "reinstallare non duplica");
        uninstall(g);
        assert!(!g.join("mods/data/songData/alone/script.lua").exists());
        assert!(g.join("assets/data/songData/alone/script.lua").exists());
    }

    #[test]
    fn a_mod_override_keeps_its_own_code() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        write(g.join("Old.exe"), "psychEngineVersion");
        write(g.join("mods/data/songData/x/script.lua"), "print('mia')
");
        write(g.join("assets/data/x/x.json"), "{}");
        install(g, Kind::Psych).unwrap();
        let t = std::fs::read_to_string(g.join("mods/data/songData/x/script.lua")).unwrap();
        assert!(t.starts_with("print('mia')") && t.contains(SONG_BEGIN));
        uninstall(g);
        assert_eq!(std::fs::read_to_string(g.join("mods/data/songData/x/script.lua")).unwrap(), "print('mia')
");
    }

    #[test]
    fn song_folders_follow_psych_rules() {
        assert_eq!(song_path("Pizza-Time"), "pizza-time");
        assert_eq!(song_path("Dad Battle"), "dad-battle");
        assert_eq!(song_path("You're Mine!"), "youre-mine");
        assert_eq!(song_path("M.I.L.F"), "milf");
    }

    #[test]
    fn mods_inside_a_bin_folder_take_the_parent_name() {
        assert_eq!(folder_name(r"E:\FNF-MODS\CatNap V2\bin"), "CatNap V2");
        assert_eq!(folder_name(r"E:\FNF-MODS\YeahMan\"), "YeahMan");
    }

    #[test]
    fn pack_json_with_real_newlines_is_read() {
        let v = lenient_json("{\n \"name\": \"Crafted\",\n \"description\": \"riga 1\nriga 2\"\n}").unwrap();
        assert_eq!(v["description"], "riga 1\nriga 2");
    }

    #[test]
    fn the_catalog_has_weeks_songs_icons_and_credits() {
        let game = tempfile::tempdir().unwrap();
        let g = game.path();
        write(
            g.join("mods/weeks/weekman.json"),
            r#"{"songs":[["Yuh","fuegoyeah",[255,105,0]],["Pizza Time","dad",[0,0,0]]],
                "storyName":"Yeah Man","weekName":"Week 1","difficulties":"Hard"}"#,
        );
        write(g.join("mods/weeks/secret.json"), r#"{"songs":[["Hidden","dad",[0,0,0]]],"hideFreeplay":true}"#);
        write(g.join("mods/images/icons/icon-fuegoyeah.png"), "png");
        write(g.join("assets/shared/images/icons/icon-dad.png"), "png");
        write(g.join("mods/images/storymenu/weekman.png"), "png");
        write(g.join("mods/data/yuh/yuh-hard.json"), r#"{"song":{"bpm":150}}"#);
        write(g.join("mods/data/credits.txt"), "Team\nFuego::fuego::Musicista::https://x.com/f::FF0000\n");
        write(g.join("mods/Inner Mod/pack.json"), "{\"name\": \"Inner\", \"description\": \"a\nb\"}");
        write(g.join("mods/Off Mod/pack.json"), "{\"name\": \"Off\"}");
        write(g.join("modsList.txt"), "Inner Mod|1\nOff Mod|0\n");

        let all = sources(g, Kind::Psych);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names[1..], ["Inner"], "nome da pack.json; le mod disattivate restano fuori");
        assert_eq!(all[1].aliases, ["Inner Mod"], "la cartella resta come vecchio nome");
        assert_eq!(mod_identity(g, Kind::Psych, "Inner Mod").0, "Inner");

        let top = build_catalog(&all[0]).unwrap();
        let c = &top.catalog;
        assert_eq!(c["albums"].as_array().unwrap().len(), 1, "la settimana nascosta resta fuori");
        assert_eq!(c["albums"][0]["name"], "Yeah Man");
        assert_eq!(c["albums"][0]["art"], "week-weekman.png");
        assert_eq!(c["tracks"][0]["id"], "yuh");
        assert_eq!(c["tracks"][0]["bpm"], 150.0);
        assert_eq!(c["tracks"][0]["color"], "#ff6900");
        assert_eq!(c["tracks"][0]["icon"], "hi-fuegoyeah.png");
        assert_eq!(c["tracks"][1]["id"], "pizza-time");
        assert_eq!(c["tracks"][1]["icon"], "hi-dad.png", "icona presa dagli asset del gioco");
        assert_eq!(c["tracks"][0]["difficulties"], json!(["Hard"]));
        assert_eq!(c["contributors"][0]["name"], "Fuego");
        assert_eq!(c["contributors"][0]["role"], "Musicista");

        let inner = build_catalog(&all[1]).unwrap();
        assert_eq!(inner.catalog["title"], "Inner");
        assert_eq!(inner.catalog["description"], "a\nb");
    }
}
