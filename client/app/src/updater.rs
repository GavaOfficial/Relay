use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PUBKEY_HEX: &str = "ef865f3fe0a28dc8f16a717f96bdd3b5abdead62e2e43e77a6e25da26754ef86";

pub const MAX_APP_BYTES: u64 = 250 * 1024 * 1024;

pub const MAX_FFMPEG_BYTES: u64 = 400 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub signature: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub download_url: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['.', '-'])
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (a, b) = (parts(current), parts(candidate));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return y > x;
        }
    }
    false
}

pub const KIND_APP: &str = "relay-app";
pub const KIND_FFMPEG: &str = "relay-ffmpeg";

pub fn signed_message(kind: &str, version: &str, sha256: &str) -> String {
    format!("{kind}|{version}|{}", sha256.to_ascii_lowercase())
}

pub fn verify_signature(
    pubkey_hex: &str,
    kind: &str,
    version: &str,
    sha256: &str,
    signature_hex: &str,
) -> bool {
    let (Ok(pk), Ok(sig)) = (hex::decode(pubkey_hex), hex::decode(signature_hex)) else {
        return false;
    };
    let (Ok(pk), Ok(sig)) = (
        <[u8; 32]>::try_from(pk.as_slice()),
        <[u8; 64]>::try_from(sig.as_slice()),
    ) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&pk) else {
        return false;
    };
    key.verify(
        signed_message(kind, version, sha256).as_bytes(),
        &Signature::from_bytes(&sig),
    )
    .is_ok()
}

pub async fn fetch_latest(
    http: &reqwest::Client,
    server: &str,
    api_path: &str,
) -> Result<Option<Release>, String> {
    let url = format!("{}{api_path}", server.trim_end_matches('/'));
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("server non raggiungibile: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("il server ha risposto {}", resp.status()));
    }
    resp.json::<Release>()
        .await
        .map(Some)
        .map_err(|e| format!("risposta non valida: {e}"))
}

#[allow(clippy::too_many_arguments)]
pub async fn download(
    http: &reqwest::Client,
    server: &str,
    api_path: &str,
    kind: &str,
    rel: &Release,
    max_bytes: u64,
    dest: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    if !verify_signature(PUBKEY_HEX, kind, &rel.version, &rel.sha256, &rel.signature) {
        return Err("firma del file non valida".into());
    }
    if rel.size == 0 || rel.size > max_bytes {
        return Err("dimensione del file non plausibile".into());
    }
    let url = format!("{}{api_path}", server.trim_end_matches('/'));
    let mut resp = http
        .get(url)
        .timeout(std::time::Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|e| format!("download non riuscito: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("il server ha risposto {}", resp.status()));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let result = async {
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("download interrotto: {e}"))?
        {
            total += chunk.len() as u64;
            if total > max_bytes {
                return Err("file troppo grande".to_string());
            }
            hasher.update(&chunk);
            std::io::Write::write_all(&mut file, &chunk).map_err(|e| e.to_string())?;
            progress(total, rel.size);
        }
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if total != rel.size || hex::encode(hasher.finalize()) != rel.sha256.to_ascii_lowercase() {
        let _ = std::fs::remove_file(&part);
        return Err("il file scaricato non corrisponde a quello firmato".into());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

fn old_path(current: &Path) -> PathBuf {
    current.with_extension("old.exe")
}

pub fn cleanup_old() {
    if let Ok(cur) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&cur));
    }
}

pub fn install(new_exe: &Path) -> Result<(), String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = old_path(&cur);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&cur, &old)
        .map_err(|e| format!("non posso sostituire il programma qui ({e}): scaricalo dal sito"))?;
    if let Err(e) = std::fs::copy(new_exe, &cur) {
        let _ = std::fs::rename(&old, &cur);
        return Err(format!("installazione non riuscita: {e}"));
    }
    relaunch(&cur);
    Ok(())
}

fn relaunch(exe: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let _ = std::process::Command::new("cmd")
            .raw_arg(format!(
                "/C ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"",
                exe.display()
            ))
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = std::process::Command::new(exe).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.2.0", "0.2.1"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.2.0", "v1.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.3.0", "0.2.9"));
    }

    fn keypair() -> (SigningKey, String) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = hex::encode(sk.verifying_key().to_bytes());
        (sk, pk)
    }

    #[test]
    fn only_a_matching_signature_is_accepted() {
        let (sk, pk) = keypair();
        let sig = hex::encode(
            sk.sign(signed_message(KIND_APP, "0.3.0", "ABCD").as_bytes())
                .to_bytes(),
        );
        assert!(verify_signature(&pk, KIND_APP, "0.3.0", "abcd", &sig));

        assert!(!verify_signature(&pk, KIND_FFMPEG, "0.3.0", "abcd", &sig));

        assert!(!verify_signature(&pk, KIND_APP, "0.4.0", "abcd", &sig));
        assert!(!verify_signature(&pk, KIND_APP, "0.3.0", "abce", &sig));
        assert!(!verify_signature(
            &pk,
            KIND_APP,
            "0.3.0",
            "abcd",
            &sig[..sig.len() - 2]
        ));
        let other = hex::encode(
            SigningKey::from_bytes(&[9u8; 32])
                .verifying_key()
                .to_bytes(),
        );
        assert!(!verify_signature(&other, KIND_APP, "0.3.0", "abcd", &sig));
        assert!(!verify_signature(&pk, KIND_APP, "0.3.0", "abcd", ""));
    }

    #[test]
    fn the_embedded_key_is_a_valid_public_key() {
        let b = hex::decode(PUBKEY_HEX).unwrap();
        assert!(VerifyingKey::from_bytes(&<[u8; 32]>::try_from(b.as_slice()).unwrap()).is_ok());
    }
}
