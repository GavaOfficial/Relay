use std::{
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use relay_common::storage::{valid_blob, NodeStats};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub struct Store {
    root: PathBuf,
    used: AtomicU64,
    blobs: AtomicU64,
}

fn check(blob: &str) -> Result<(), String> {
    if valid_blob(blob) {
        Ok(())
    } else {
        Err("id del blob non valido".into())
    }
}

impl Store {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root.join("blobs"))?;
        std::fs::create_dir_all(root.join("tmp"))?;
        let (mut used, mut blobs) = (0, 0);
        for dir in std::fs::read_dir(root.join("blobs"))?.flatten() {
            for f in std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(m) = f.metadata() {
                    used += m.len();
                    blobs += 1;
                }
            }
        }
        for f in std::fs::read_dir(root.join("tmp"))?.flatten() {
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e.as_secs() > 86_400);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            used: AtomicU64::new(used),
            blobs: AtomicU64::new(blobs),
        })
    }

    fn blob_path(&self, blob: &str) -> PathBuf {
        self.root.join("blobs").join(&blob[..2]).join(blob)
    }

    fn part_path(&self, blob: &str) -> PathBuf {
        self.root.join("tmp").join(format!("{blob}.part"))
    }

    pub fn stats(&self) -> NodeStats {
        NodeStats {
            total: fs2::total_space(&self.root).unwrap_or(0),
            free: fs2::available_space(&self.root).unwrap_or(0),
            used: self.used.load(Ordering::Relaxed),
            blobs: self.blobs.load(Ordering::Relaxed),
        }
    }

    pub async fn put(&self, blob: &str, offset: u64, data: &[u8]) -> Result<(), String> {
        check(blob)?;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.part_path(blob))
            .await
            .map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| e.to_string())?;
        f.write_all(data).await.map_err(|e| e.to_string())?;
        f.flush().await.map_err(|e| e.to_string())
    }

    pub async fn commit(&self, blob: &str, size: u64, sha256: &str) -> Result<(), String> {
        check(blob)?;
        let part = self.part_path(blob);
        let len = tokio::fs::metadata(&part)
            .await
            .map_err(|_| "file non ricevuto".to_string())?
            .len();
        if len != size {
            return Err(format!("dimensione sbagliata: {len} invece di {size}"));
        }
        let p = part.clone();
        let got = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
            let mut h = Sha256::new();
            let mut f = std::fs::OpenOptions::new().read(true).write(true).open(p)?;
            std::io::copy(&mut f, &mut h)?;
            f.sync_all()?;
            Ok(hex::encode(h.finalize()))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(sha256) {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("hash diverso: file danneggiato durante il trasferimento".into());
        }
        let dst = self.blob_path(blob);
        tokio::fs::create_dir_all(dst.parent().unwrap())
            .await
            .map_err(|e| e.to_string())?;
        let replaced = tokio::fs::metadata(&dst).await.map(|m| m.len()).ok();
        tokio::fs::rename(&part, &dst)
            .await
            .map_err(|e| e.to_string())?;
        match replaced {
            Some(old) => {
                self.used.fetch_sub(old, Ordering::Relaxed);
            }
            None => {
                self.blobs.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.used.fetch_add(size, Ordering::Relaxed);
        Ok(())
    }

    pub async fn open_range(
        &self,
        blob: &str,
        offset: u64,
        len: u64,
    ) -> Result<tokio::io::Take<tokio::fs::File>, String> {
        check(blob)?;
        let mut f = tokio::fs::File::open(self.blob_path(blob))
            .await
            .map_err(|_| "blob inesistente".to_string())?;
        let size = f.metadata().await.map_err(|e| e.to_string())?.len();
        if offset.checked_add(len).is_none_or(|end| end > size) {
            return Err("richiesta oltre la fine del file".into());
        }
        f.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| e.to_string())?;
        Ok(f.take(len))
    }

    pub async fn delete(&self, blob: &str) -> Result<(), String> {
        check(blob)?;
        let p = self.blob_path(blob);
        if let Ok(m) = tokio::fs::metadata(&p).await {
            tokio::fs::remove_file(&p)
                .await
                .map_err(|e| e.to_string())?;
            self.used.fetch_sub(m.len(), Ordering::Relaxed);
            self.blobs.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }

    pub async fn abort(&self, blob: &str) -> Result<(), String> {
        check(blob)?;
        let _ = tokio::fs::remove_file(self.part_path(blob)).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "0f8f2a1e-6f3c-4f8e-9a51-2a7b3c4d5e6f";

    #[tokio::test]
    async fn pieces_in_any_order_then_verified_commit() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        s.put(B, 5, b"world").await.unwrap();
        s.put(B, 0, b"hello").await.unwrap();
        let sha = hex::encode(Sha256::digest(b"helloworld"));
        assert!(s.commit(B, 10, "00").await.is_err(), "hash sbagliato");
        s.put(B, 0, b"helloworld").await.unwrap();
        s.commit(B, 10, &sha).await.unwrap();
        assert_eq!(s.stats().blobs, 1);
        assert_eq!(s.stats().used, 10);
        let mut buf = String::new();
        s.open_range(B, 2, 6)
            .await
            .unwrap()
            .read_to_string(&mut buf)
            .await
            .unwrap();
        assert_eq!(buf, "llowor");
        assert!(s.open_range(B, 8, 5).await.is_err());
        s.delete(B).await.unwrap();
        assert_eq!(s.stats().blobs, 0);
        assert!(s.put("../x", 0, b"").await.is_err());
    }
}
