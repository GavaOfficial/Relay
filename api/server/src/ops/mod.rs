pub mod routes;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use relay_common::ops::{InputFile, Job, JobKind, Poll};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::storage::hash_key;

const LEASE: Duration = Duration::from_secs(5 * 60);
const ONLINE: Duration = Duration::from_secs(90);
const MAX_BACKOFF_SECS: u64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsNode {
    pub id: String,
    pub name: String,
    key_sha256: String,
    pub created_at: u64,
    #[serde(default)]
    pub parallel: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    nodes: Vec<OpsNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queued {
    pub job: Job,
    pub dir: String,
    pub created_at: u64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub assigned: Option<String>,
    #[serde(default)]
    pub lease_until: u64,
    #[serde(default)]
    pub assigned_at: u64,
    #[serde(default)]
    pub not_before: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Queue {
    #[serde(default)]
    jobs: Vec<Queued>,
}

#[derive(Debug, Clone, Default)]
pub struct Live {
    pub last_seen: Option<Instant>,
    pub poll: Poll,
    pub jobs: BTreeMap<Uuid, (String, u8)>,
}

pub enum Sink {
    Local {
        tmp: PathBuf,
        file: tokio::fs::File,
    },
    Stored {
        node: String,
        link: Arc<crate::storage::tunnel::NodeLink>,
        blob: String,
        hasher: Sha256,
    },
}

pub struct BigUpload {
    pub next: u64,
    pub chunk: u64,
    pub plain: u64,
    pub sink: Sink,
}

pub struct Ops {
    dir: PathBuf,
    data_dir: PathBuf,
    config: Mutex<Config>,
    queue: Mutex<Queue>,
    pub live: Mutex<HashMap<String, Live>>,
    pub wake: Notify,
    pub bigs: tokio::sync::Mutex<HashMap<(Uuid, String), BigUpload>>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let tmp = path.with_extension("json.tmp");
    let ok = serde_json::to_vec_pretty(value)
        .ok()
        .map(|b| std::fs::write(&tmp, b).and_then(|_| std::fs::rename(&tmp, path)));
    if !matches!(ok, Some(Ok(()))) {
        tracing::error!("operazioni: non riesco a salvare {}", path.display());
    }
}

fn same_work(a: &JobKind, b: &JobKind) -> bool {
    match (a, b) {
        (
            JobKind::Match {
                match_id: m1,
                player: p1,
            },
            JobKind::Match {
                match_id: m2,
                player: p2,
            },
        ) => m1 == m2 && p1 == p2,
        (JobKind::Clip { clip: c1, .. }, JobKind::Clip { clip: c2, .. }) => c1 == c2,
        _ => false,
    }
}

pub fn label(kind: &JobKind) -> String {
    match kind {
        JobKind::Match { player, .. } => format!("partita, video di {player}"),
        JobKind::Clip { engine, .. } => format!("clip {engine}"),
    }
}

impl Ops {
    pub fn open(data_dir: &Path) -> std::io::Result<Arc<Self>> {
        let dir = data_dir.join("ops");
        std::fs::create_dir_all(&dir)?;
        let mut queue: Queue = read_json(&dir.join("queue.json"));
        for q in queue.jobs.iter_mut() {
            q.assigned = None;
        }
        Ok(Arc::new(Self {
            config: Mutex::new(read_json(&dir.join("config.json"))),
            queue: Mutex::new(queue),
            dir,
            data_dir: data_dir.to_path_buf(),
            live: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            bigs: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }

    fn save_config(&self) {
        write_json(&self.dir.join("config.json"), &*self.config.lock().unwrap());
    }

    fn save_queue(&self) {
        write_json(&self.dir.join("queue.json"), &*self.queue.lock().unwrap());
    }

    pub fn enabled(&self) -> bool {
        !self.config.lock().unwrap().nodes.is_empty()
    }

    pub fn nodes(&self) -> Vec<OpsNode> {
        self.config.lock().unwrap().nodes.clone()
    }

    pub fn add_node(&self, name: &str) -> (OpsNode, String) {
        let mut secret = [0u8; 32];
        let _ = getrandom::getrandom(&mut secret);
        let key = format!("rok_{}", hex::encode(secret));
        let node = OpsNode {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            key_sha256: hash_key(&key),
            created_at: now_secs(),
            parallel: None,
        };
        self.config.lock().unwrap().nodes.push(node.clone());
        self.save_config();
        (node, key)
    }

    pub fn rename_node(&self, id: &str, name: &str) -> bool {
        let mut c = self.config.lock().unwrap();
        let Some(n) = c.nodes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        n.name = name.to_string();
        drop(c);
        self.save_config();
        true
    }

    pub fn set_parallel(&self, id: &str, parallel: Option<u32>) -> bool {
        let mut c = self.config.lock().unwrap();
        let Some(n) = c.nodes.iter_mut().find(|n| n.id == id) else {
            return false;
        };
        n.parallel = parallel.map(|p| p.clamp(1, 16));
        drop(c);
        self.save_config();
        self.wake.notify_waiters();
        true
    }

    pub fn limit(&self, id: &str) -> u32 {
        let fixed = self
            .config
            .lock()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.parallel);
        let live = self
            .live
            .lock()
            .unwrap()
            .get(id)
            .map(|l| (l.poll.threads, l.poll.slots))
            .unwrap_or((1, 1));
        let want = fixed.unwrap_or_else(|| relay_common::ops::auto_parallel(live.0));
        want.min(live.1.max(1))
    }

    pub fn rotate_key(&self, id: &str) -> Option<String> {
        let mut secret = [0u8; 32];
        let _ = getrandom::getrandom(&mut secret);
        let key = format!("rok_{}", hex::encode(secret));
        let mut c = self.config.lock().unwrap();
        c.nodes.iter_mut().find(|n| n.id == id)?.key_sha256 = hash_key(&key);
        drop(c);
        self.save_config();
        Some(key)
    }

    pub fn remove_node(&self, id: &str) -> bool {
        let mut c = self.config.lock().unwrap();
        let before = c.nodes.len();
        c.nodes.retain(|n| n.id != id);
        let removed = c.nodes.len() != before;
        drop(c);
        if removed {
            self.save_config();
            self.live.lock().unwrap().remove(id);
            let mut q = self.queue.lock().unwrap();
            for j in q
                .jobs
                .iter_mut()
                .filter(|j| j.assigned.as_deref() == Some(id))
            {
                j.assigned = None;
            }
            drop(q);
            self.save_queue();
            self.wake.notify_waiters();
        }
        removed
    }

    pub fn node_for_key(&self, key: &str) -> Option<String> {
        let h = hash_key(key);
        self.config
            .lock()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.key_sha256 == h)
            .map(|n| n.id.clone())
    }

    pub fn online(&self, id: &str) -> bool {
        self.live
            .lock()
            .unwrap()
            .get(id)
            .and_then(|l| l.last_seen)
            .is_some_and(|t| t.elapsed() < ONLINE)
    }

    pub fn rel(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.data_dir).ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.data_dir.clone(), |p, c| p.join(c))
    }

    pub fn enqueue(&self, kind: JobKind, dir: &Path, inputs: Vec<InputFile>) -> bool {
        let Some(dir) = self.rel(dir) else {
            return false;
        };
        let mut q = self.queue.lock().unwrap();
        if let Some(existing) = q.jobs.iter_mut().find(|j| same_work(&j.job.kind, &kind)) {
            if existing.assigned.is_none() {
                existing.job.kind = kind;
                existing.job.inputs = inputs;
                existing.dir = dir;
            }
            drop(q);
            self.save_queue();
            return false;
        }
        q.jobs.push(Queued {
            job: Job {
                id: Uuid::new_v4(),
                kind,
                inputs,
                threads: 0,
            },
            dir,
            created_at: now_secs(),
            attempts: 0,
            last_error: None,
            assigned: None,
            lease_until: 0,
            assigned_at: 0,
            not_before: 0,
        });
        drop(q);
        self.save_queue();
        self.wake.notify_waiters();
        true
    }

    pub fn jobs(&self) -> Vec<Queued> {
        self.queue.lock().unwrap().jobs.clone()
    }

    pub fn is_queued(&self, kind: &JobKind) -> bool {
        self.queue
            .lock()
            .unwrap()
            .jobs
            .iter()
            .any(|j| same_work(&j.job.kind, kind))
    }

    pub fn see(&self, node: &str, poll: Option<Poll>) {
        let mut live = self.live.lock().unwrap();
        let l = live.entry(node.to_string()).or_default();
        l.last_seen = Some(Instant::now());
        if let Some(p) = poll {
            l.poll = p;
        }
    }

    pub fn take(&self, node: &str, running: Option<&[Uuid]>) -> Option<Job> {
        let now = now_secs();
        let limit = self.limit(node);
        let threads = self
            .live
            .lock()
            .unwrap()
            .get(node)
            .map(|l| l.poll.threads)
            .unwrap_or(1)
            .max(1);
        let mut q = self.queue.lock().unwrap();
        let mut changed = false;
        for j in q.jobs.iter_mut() {
            let lost = running.is_some_and(|r| {
                j.assigned.as_deref() == Some(node)
                    && !r.contains(&j.job.id)
                    && j.assigned_at + 60 < now
            });
            if j.assigned.is_some() && (j.lease_until < now || lost) {
                tracing::warn!(
                    "operazioni: {} interrotto, torna in coda",
                    label(&j.job.kind)
                );
                j.assigned = None;
                changed = true;
            }
        }
        let busy = q
            .jobs
            .iter()
            .filter(|j| j.assigned.as_deref() == Some(node))
            .count() as u32;
        let job = if busy < limit {
            q.jobs
                .iter_mut()
                .find(|j| j.assigned.is_none() && j.not_before <= now)
                .map(|j| {
                    j.assigned = Some(node.to_string());
                    j.assigned_at = now;
                    j.lease_until = now + LEASE.as_secs();
                    j.job.threads = (threads / limit).max(1);
                    j.job.clone()
                })
        } else {
            None
        };
        drop(q);
        if changed || job.is_some() {
            self.save_queue();
        }
        let job = job?;
        self.live
            .lock()
            .unwrap()
            .entry(node.to_string())
            .or_default()
            .jobs
            .insert(job.id, ("download".into(), 0));
        Some(job)
    }

    pub fn leased(&self, node: &str, id: Uuid) -> Option<Queued> {
        let now = now_secs();
        let mut q = self.queue.lock().unwrap();
        let j = q
            .jobs
            .iter_mut()
            .find(|j| j.job.id == id && j.assigned.as_deref() == Some(node))?;
        j.lease_until = now + LEASE.as_secs();
        Some(j.clone())
    }

    pub fn progress(&self, node: &str, id: Uuid, stage: &str, pct: u8) {
        let mut live = self.live.lock().unwrap();
        let l = live.entry(node.to_string()).or_default();
        l.last_seen = Some(Instant::now());
        l.jobs.insert(id, (stage.to_string(), pct.min(100)));
    }

    pub fn failed(&self, node: &str, id: Uuid, error: &str) {
        let now = now_secs();
        let mut q = self.queue.lock().unwrap();
        if let Some(j) = q
            .jobs
            .iter_mut()
            .find(|j| j.job.id == id && j.assigned.as_deref() == Some(node))
        {
            j.attempts += 1;
            j.last_error = Some(error.chars().take(300).collect());
            j.assigned = None;
            j.not_before = now + (60 * 2u64.pow(j.attempts.min(5))).min(MAX_BACKOFF_SECS);
            tracing::warn!(
                "operazioni: {} non riuscito (tentativo {}): {error}",
                label(&j.job.kind),
                j.attempts
            );
        }
        drop(q);
        self.save_queue();
        self.idle(node, id);
        self.wake.notify_waiters();
    }

    pub fn done(&self, node: &str, id: Uuid) {
        self.queue.lock().unwrap().jobs.retain(|j| j.job.id != id);
        self.save_queue();
        self.idle(node, id);
        self.wake.notify_waiters();
    }

    fn idle(&self, node: &str, id: Uuid) {
        if let Some(l) = self.live.lock().unwrap().get_mut(node) {
            l.jobs.remove(&id);
        }
    }

    pub fn retry_now(&self) {
        let mut q = self.queue.lock().unwrap();
        for j in q.jobs.iter_mut() {
            j.not_before = 0;
        }
        drop(q);
        self.save_queue();
        self.wake.notify_waiters();
    }

    pub fn forget(&self, kind: &JobKind) {
        self.queue
            .lock()
            .unwrap()
            .jobs
            .retain(|j| !same_work(&j.job.kind, kind));
        self.save_queue();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(p: &str) -> JobKind {
        JobKind::Match {
            match_id: Uuid::nil(),
            player: p.into(),
        }
    }

    #[test]
    fn jobs_are_deduplicated_leased_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let ops = Ops::open(dir.path()).unwrap();
        assert!(!ops.enabled());
        let (node, key) = ops.add_node("casa");
        assert!(ops.enabled());
        assert_eq!(ops.node_for_key(&key).as_deref(), Some(node.id.as_str()));
        let d = dir.path().join("matches/x/players/a");
        assert!(ops.enqueue(kind("a"), &d, vec![]));
        assert!(
            !ops.enqueue(kind("a"), &d, vec![]),
            "stesso lavoro una volta sola"
        );
        ops.see(
            &node.id,
            Some(Poll {
                threads: 12,
                slots: 4,
                ..Default::default()
            }),
        );
        assert_eq!(ops.limit(&node.id), 2, "12 thread: due lavori insieme");
        assert!(ops.enqueue(kind("b"), &d, vec![]));
        assert!(ops.enqueue(kind("c"), &d, vec![]));
        let job = ops.take(&node.id, Some(&[])).unwrap();
        assert_eq!(job.threads, 6);
        let second = ops.take(&node.id, Some(&[job.id])).unwrap();
        assert_ne!(job.id, second.id);
        assert!(
            ops.take(&node.id, Some(&[job.id, second.id])).is_none(),
            "limite raggiunto"
        );
        ops.failed(&node.id, job.id, "errore");
        ops.done(&node.id, second.id);
        let third = ops.take(&node.id, Some(&[])).unwrap();
        assert_ne!(
            third.id, job.id,
            "quello fallito aspetta prima di riprovare"
        );
        ops.done(&node.id, third.id);
        assert!(ops.take(&node.id, Some(&[])).is_none());
        ops.retry_now();
        let again = ops.take(&node.id, Some(&[])).unwrap();
        assert_eq!(again.id, job.id);
        ops.done(&node.id, again.id);
        assert!(ops.jobs().is_empty());
        assert!(ops.set_parallel(&node.id, Some(1)));
        assert_eq!(ops.limit(&node.id), 1);
        let reopened = Ops::open(dir.path()).unwrap();
        assert!(reopened.enabled());
    }
}
