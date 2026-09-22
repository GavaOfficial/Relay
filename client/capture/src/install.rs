use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const OBS_VERSION: &str = "32.2.2";
pub const OBS_URL: &str = "https://github.com/obsproject/obs-studio/releases/download/32.2.2/OBS-Studio-32.2.2-Windows-x64.zip";

const CHUNK: u64 = 2 * 1024 * 1024;
const MANIFEST: &str = "installed.json";

pub const HOST_EXE: &str = "relay-capture.exe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub id: &'static str,
    pub label: &'static str,
}

pub const ENGINE: Group = Group {
    id: "engine",
    label: "Motore di cattura",
};
pub const ENCODERS: Group = Group {
    id: "encoders",
    label: "Codifica video e audio",
};
pub const GAMES: Group = Group {
    id: "games",
    label: "Aggancio ai giochi",
};
pub const AUDIO: Group = Group {
    id: "audio",
    label: "Audio del gioco",
};

pub const GROUPS: [Group; 4] = [ENGINE, ENCODERS, GAMES, AUDIO];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub dest: String,
    pub group: Group,
}

const ROOT_ENGINE: &[&str] = &[
    "obs.dll",
    "w32-pthreads.dll",
    "libobs-d3d11.dll",
    "libobs-winrt.dll",
    "obs-ffmpeg-mux.exe",
];
const ROOT_ENCODERS: &[&str] = &[
    "avcodec-62.dll",
    "avdevice-62.dll",
    "avfilter-11.dll",
    "avformat-62.dll",
    "avutil-60.dll",
    "swresample-6.dll",
    "swscale-9.dll",
    "libx264-164.dll",
    "libcurl.dll",
    "librist.dll",
    "srt.dll",
    "zlib.dll",
    "obs-nvenc-test.exe",
    "obs-amf-test.exe",
    "obs-qsv-test.exe",
];
const PLUGINS_ENCODERS: &[&str] = &["obs-ffmpeg", "obs-nvenc", "obs-x264", "obs-qsv11"];

pub fn obs_select(name: &str) -> Option<Selected> {
    if name.ends_with('/') || name.ends_with(".pdb") {
        return None;
    }
    if let Some(f) = name.strip_prefix("bin/64bit/") {
        if ROOT_ENGINE.contains(&f) {
            return Some(Selected {
                dest: f.into(),
                group: ENGINE,
            });
        }
        if ROOT_ENCODERS.contains(&f) {
            return Some(Selected {
                dest: f.into(),
                group: ENCODERS,
            });
        }
        return None;
    }
    if let Some(f) = name.strip_prefix("obs-plugins/64bit/") {
        let plugin = f.strip_suffix(".dll")?;
        let group = match plugin {
            "win-capture" => GAMES,
            "win-wasapi" => AUDIO,
            p if PLUGINS_ENCODERS.contains(&p) => ENCODERS,
            _ => return None,
        };
        return Some(Selected {
            dest: name.into(),
            group,
        });
    }
    if let Some(rest) = name.strip_prefix("data/libobs/") {
        return (!rest.is_empty()).then(|| Selected {
            dest: name.into(),
            group: ENGINE,
        });
    }
    if let Some(rest) = name.strip_prefix("data/obs-plugins/") {
        let (plugin, file) = rest.split_once('/')?;
        let group = match plugin {
            "win-capture" => GAMES,
            "win-wasapi" => AUDIO,
            p if PLUGINS_ENCODERS.contains(&p) => ENCODERS,
            _ => return None,
        };
        if file.starts_with("locale/") && file != "locale/en-US.ini" {
            return None;
        }
        return Some(Selected {
            dest: name.into(),
            group,
        });
    }
    None
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GroupProgress {
    pub id: &'static str,
    pub label: &'static str,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: f32,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InstallProgress {
    pub percent: f32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub groups: Vec<GroupProgress>,
    pub current: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: String,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    size: u64,
}

pub fn runtime_dir(base: &Path) -> PathBuf {
    base.join("obs")
}

pub fn host_exe(dir: &Path) -> PathBuf {
    dir.join(HOST_EXE)
}

/// Cartella da cui avviare `relay-capture.exe` (deve esistere ed essere passata come `current_dir`
/// del processo): l'aggancio ai giochi cerca alcuni suoi file relativi a questa posizione.
pub fn spawn_dir(dir: &Path) -> PathBuf {
    dir.join("bin").join("64bit")
}

pub fn is_ready(dir: &Path) -> bool {
    let Ok(text) = fs::read_to_string(dir.join(MANIFEST)) else {
        return false;
    };
    let Ok(m) = serde_json::from_str::<Manifest>(&text) else {
        return false;
    };
    m.version == OBS_VERSION
        && !m.files.is_empty()
        && m.files.iter().all(|f| {
            fs::metadata(dir.join(&f.path))
                .map(|md| md.len() == f.size)
                .unwrap_or(false)
        })
}

struct RangeReader {
    http: reqwest::blocking::Client,
    url: String,
    size: u64,
    pos: u64,
    cache_start: u64,
    cache: Vec<u8>,
    plan_len: u64,
    cancel: Arc<AtomicBool>,
}

impl RangeReader {
    fn open(url: &str, cancel: Arc<AtomicBool>) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()?;
        let resp = http
            .get(url)
            .header("Range", "bytes=0-0")
            .send()
            .context("connessione al sito di OBS")?;
        if !resp.status().is_success() {
            bail!("il sito di OBS ha risposto {}", resp.status());
        }
        let size = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| anyhow!("il sito di OBS non supporta i download parziali"))?;
        Ok(Self {
            http,
            url: url.to_string(),
            size,
            pos: 0,
            cache_start: 0,
            cache: Vec::new(),
            plan_len: CHUNK,
            cancel,
        })
    }

    fn expect(&mut self, n: u64) {
        self.plan_len = n;
    }

    fn fetch(&mut self, start: u64) -> io::Result<()> {
        let want = self.plan_len.clamp(32 * 1024, CHUNK);
        let end = (start + want).min(self.size) - 1;
        let mut last = None;
        for attempt in 0..5u64 {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("installazione annullata"));
            }
            let res = self
                .http
                .get(&self.url)
                .header("Range", format!("bytes={start}-{end}"))
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.bytes());
            match res {
                Ok(b) if b.len() as u64 == end - start + 1 => {
                    self.cache_start = start;
                    self.cache = b.to_vec();
                    return Ok(());
                }
                Ok(b) => last = Some(format!("risposta incompleta ({} byte)", b.len())),
                Err(e) => last = Some(e.to_string()),
            }
            std::thread::sleep(Duration::from_millis(400 * (attempt + 1)));
        }
        Err(io::Error::other(
            last.unwrap_or_else(|| "download non riuscito".into()),
        ))
    }
}

impl Read for RangeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let in_cache =
            self.pos >= self.cache_start && self.pos < self.cache_start + self.cache.len() as u64;
        if !in_cache {
            self.fetch(self.pos)?;
        }
        let off = (self.pos - self.cache_start) as usize;
        let n = buf.len().min(self.cache.len() - off);
        buf[..n].copy_from_slice(&self.cache[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(o) => o as i128,
            SeekFrom::Current(o) => self.pos as i128 + o as i128,
            SeekFrom::End(o) => self.size as i128 + o as i128,
        };
        if p < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "posizione negativa",
            ));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

#[derive(Clone)]
pub struct Canceller(Arc<AtomicBool>);

impl Canceller {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Default for Canceller {
    fn default() -> Self {
        Self::new()
    }
}

struct PlanItem {
    sel: Selected,
    comp: u64,
    size: u64,
    crc: u32,
    method: usize,
    offset: u64,
}

struct CdEntry {
    name: String,
    crc: u32,
    comp: u64,
    size: u64,
    method: usize,
    offset: u64,
}

fn u16_at(b: &[u8], i: usize) -> usize {
    u16::from_le_bytes([b[i], b[i + 1]]) as usize
}

fn u32_at(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(a)
}

fn read_central_directory(r: &mut RangeReader) -> Result<Vec<CdEntry>> {
    let tail_len = r.size.min(70_000);
    r.seek(SeekFrom::Start(r.size - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    r.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| anyhow!("archivio di OBS non valido: manca l'indice"))?;
    let (count, cd_size, cd_off) = (
        u16_at(&tail, eocd + 10),
        u32_at(&tail, eocd + 12),
        u32_at(&tail, eocd + 16),
    );
    if count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
        bail!("archivio di OBS in formato zip64: non supportato");
    }
    r.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size as usize];
    r.read_exact(&mut cd)?;

    let mut out = Vec::with_capacity(count);
    let mut i = 0usize;
    while i + 46 <= cd.len() && cd[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] {
        let (name_len, extra_len, comment_len) = (
            u16_at(&cd, i + 28),
            u16_at(&cd, i + 30),
            u16_at(&cd, i + 32),
        );
        let crc = u32_at(&cd, i + 16) as u32;
        let method = u16_at(&cd, i + 10);
        let (mut comp, mut size, mut offset) = (
            u32_at(&cd, i + 20),
            u32_at(&cd, i + 24),
            u32_at(&cd, i + 42),
        );
        let name = String::from_utf8_lossy(&cd[i + 46..i + 46 + name_len]).into_owned();
        if comp == 0xFFFF_FFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
            let extra = &cd[i + 46 + name_len..i + 46 + name_len + extra_len];
            let mut k = 0;
            while k + 4 <= extra.len() {
                let (id, len) = (u16_at(extra, k), u16_at(extra, k + 2));
                if id == 1 {
                    let mut f = k + 4;
                    if size == 0xFFFF_FFFF {
                        size = u64_at(extra, f);
                        f += 8;
                    }
                    if comp == 0xFFFF_FFFF {
                        comp = u64_at(extra, f);
                        f += 8;
                    }
                    if offset == 0xFFFF_FFFF {
                        offset = u64_at(extra, f);
                    }
                }
                k += 4 + len;
            }
        }
        out.push(CdEntry {
            name,
            crc,
            comp,
            size,
            method,
            offset,
        });
        i += 46 + name_len + extra_len + comment_len;
    }
    if out.len() != count {
        bail!(
            "indice dell'archivio di OBS incompleto ({} voci su {count})",
            out.len()
        );
    }
    Ok(out)
}

pub fn install(
    dir: &Path,
    cancel: &Canceller,
    progress: &mut dyn FnMut(&InstallProgress),
) -> Result<()> {
    install_from(OBS_URL, OBS_VERSION, dir, &obs_select, cancel, progress)
}

pub fn install_from(
    url: &str,
    version: &str,
    dir: &Path,
    select: &dyn Fn(&str) -> Option<Selected>,
    cancel: &Canceller,
    progress: &mut dyn FnMut(&InstallProgress),
) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("non riesco a creare {}", dir.display()))?;
    let _ = fs::remove_file(dir.join(MANIFEST));

    let reader = RangeReader::open(url, cancel.0.clone())?;
    let mut reader = reader;
    let entries = read_central_directory(&mut reader)?;

    let mut plan: Vec<PlanItem> = Vec::new();
    for e in &entries {
        if let Some(sel) = select(&e.name) {
            plan.push(PlanItem {
                sel,
                comp: e.comp.max(1),
                size: e.size,
                crc: e.crc,
                method: e.method,
                offset: e.offset,
            });
        }
    }
    if plan.is_empty() {
        bail!("nell'archivio di OBS non ho trovato i file che servono");
    }
    plan.sort_by_key(|p| p.offset);

    let total: u64 = plan.iter().map(|p| p.comp).sum();
    let mut group_total = std::collections::HashMap::new();
    for p in &plan {
        *group_total.entry(p.sel.group.id).or_insert(0u64) += p.comp;
    }
    let mut group_done: std::collections::HashMap<&'static str, u64> =
        GROUPS.iter().map(|g| (g.id, 0)).collect();
    let mut done = 0u64;

    let snapshot = |done: u64,
                    group_done: &std::collections::HashMap<&'static str, u64>,
                    current: &str| InstallProgress {
        percent: (done as f32 / total as f32 * 100.0).min(100.0),
        bytes_done: done,
        bytes_total: total,
        current: current.to_string(),
        groups: GROUPS
            .iter()
            .filter(|g| group_total.contains_key(g.id))
            .map(|g| {
                let t = group_total[g.id];
                let d = group_done[g.id];
                GroupProgress {
                    id: g.id,
                    label: g.label,
                    bytes_done: d,
                    bytes_total: t,
                    percent: (d as f32 / t as f32 * 100.0).min(100.0),
                    done: d >= t,
                }
            })
            .collect(),
    };
    progress(&snapshot(0, &group_done, ""));

    let mut files = Vec::with_capacity(plan.len());
    let mut last_report = std::time::Instant::now();
    for item in &plan {
        if cancel.0.load(Ordering::Relaxed) {
            bail!("installazione annullata");
        }
        let (sel, comp, size) = (&item.sel, item.comp, item.size);
        let target = dir.join(&sel.dest);
        if already_ok(&target, size, item.crc) {
            *group_done.get_mut(sel.group.id).unwrap() += comp;
            done += comp;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let part = target.with_extension("part");
            let mut out = fs::File::create(&part)
                .with_context(|| format!("non riesco a scrivere {}", part.display()))?;
            let mut hasher = crc32fast::Hasher::new();
            let mut written = 0u64;
            let mut counted = 0u64;
            let res = (|| -> Result<()> {
                reader.seek(SeekFrom::Start(item.offset))?;
                reader.expect(30 + 1024 + comp);
                let mut head = [0u8; 30];
                reader.read_exact(&mut head)?;
                if head[..4] != [0x50, 0x4b, 0x03, 0x04] {
                    bail!("intestazione del file {} non valida", sel.dest);
                }
                let skip = u16_at(&head, 26) + u16_at(&head, 28);
                reader.seek(SeekFrom::Start(item.offset + 30 + skip as u64))?;
                reader.expect(comp);
                let limited = (&mut reader).take(comp);
                let mut src: Box<dyn Read> = match item.method {
                    0 => Box::new(limited),
                    8 => Box::new(flate2::read::DeflateDecoder::new(io::BufReader::new(
                        limited,
                    ))),
                    m => bail!("metodo di compressione {m} non supportato"),
                };
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = src
                        .read(&mut buf)
                        .with_context(|| format!("scaricando {}", sel.dest))?;
                    if n == 0 {
                        break;
                    }
                    io::Write::write_all(&mut out, &buf[..n])?;
                    hasher.update(&buf[..n]);
                    written += n as u64;
                    if last_report.elapsed() >= Duration::from_millis(120) {
                        let frac = if size == 0 {
                            1.0
                        } else {
                            written as f64 / size as f64
                        };
                        let cur = ((comp as f64 * frac) as u64).min(comp);
                        *group_done.get_mut(sel.group.id).unwrap() += cur - counted;
                        done += cur - counted;
                        counted = cur;
                        progress(&snapshot(done, &group_done, &sel.dest));
                        last_report = std::time::Instant::now();
                    }
                }
                Ok(())
            })();
            drop(out);
            if let Err(e) = res {
                let _ = fs::remove_file(&part);
                return Err(e);
            }
            if written != size || hasher.finalize() != item.crc {
                let _ = fs::remove_file(&part);
                bail!("il file {} scaricato e' danneggiato", sel.dest);
            }
            let _ = fs::remove_file(&target);
            fs::rename(&part, &target)
                .with_context(|| format!("non riesco a completare {}", sel.dest))?;
            *group_done.get_mut(sel.group.id).unwrap() += comp - counted;
            done += comp - counted;
        }
        files.push(ManifestFile {
            path: sel.dest.clone(),
            size,
        });
        progress(&snapshot(done.min(total), &group_done, &sel.dest));
    }

    // il modulo di aggancio ai giochi cerca i suoi file due cartelle sopra la cartella di lavoro
    // (come se fossimo dentro "bin/64bit" di un'installazione vera di OBS): questa cartella
    // vuota serve solo da punto di partenza quando si avvia relay-capture.exe
    fs::create_dir_all(dir.join("bin").join("64bit"))?;
    let manifest = Manifest {
        version: version.to_string(),
        files,
    };
    fs::write(dir.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)?;
    progress(&snapshot(total, &group_done, ""));
    Ok(())
}

fn already_ok(target: &Path, size: u64, crc: u32) -> bool {
    match fs::metadata(target) {
        Ok(m) if m.len() == size => {}
        _ => return false,
    }
    let Ok(mut f) = fs::File::open(target) else {
        return false;
    };
    let mut h = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    h.finalize() == crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_what_the_recorder_needs() {
        let d = |n: &str| obs_select(n).map(|s| (s.dest, s.group.id));
        assert_eq!(d("bin/64bit/obs.dll"), Some(("obs.dll".into(), "engine")));
        assert_eq!(
            d("bin/64bit/avcodec-62.dll"),
            Some(("avcodec-62.dll".into(), "encoders"))
        );
        assert_eq!(d("bin/64bit/obs64.exe"), None);
        assert_eq!(d("bin/64bit/Qt6Core.dll"), None);
        assert_eq!(d("bin/64bit/obs.pdb"), None);
        assert_eq!(
            d("obs-plugins/64bit/win-capture.dll"),
            Some(("obs-plugins/64bit/win-capture.dll".into(), "games"))
        );
        assert_eq!(
            d("obs-plugins/64bit/win-wasapi.dll"),
            Some(("obs-plugins/64bit/win-wasapi.dll".into(), "audio"))
        );
        assert_eq!(
            d("obs-plugins/64bit/obs-nvenc.dll"),
            Some(("obs-plugins/64bit/obs-nvenc.dll".into(), "encoders"))
        );
        assert_eq!(d("obs-plugins/64bit/obs-browser.dll"), None);
        assert_eq!(d("obs-plugins/64bit/obs-outputs.dll"), None);
        assert_eq!(
            d("data/libobs/default.effect"),
            Some(("data/libobs/default.effect".into(), "engine"))
        );
        assert_eq!(
            d("data/obs-plugins/win-capture/graphics-hook64.dll"),
            Some((
                "data/obs-plugins/win-capture/graphics-hook64.dll".into(),
                "games"
            ))
        );
        assert_eq!(d("data/obs-plugins/win-capture/locale/it-IT.ini"), None);
        assert_eq!(
            d("data/obs-plugins/win-capture/locale/en-US.ini"),
            Some((
                "data/obs-plugins/win-capture/locale/en-US.ini".into(),
                "games"
            ))
        );
        assert_eq!(d("data/obs-plugins/obs-browser/x.txt"), None);
        assert_eq!(d("data/obs-studio/locale/it-IT.ini"), None);
        assert_eq!(d("bin/64bit/"), None);
    }

    #[test]
    fn not_ready_without_manifest() {
        let d = std::env::temp_dir().join(format!("relay-obs-empty-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        assert!(!is_ready(&d));
        let _ = fs::remove_dir_all(&d);
    }
}
