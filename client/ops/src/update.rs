use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const PUBKEY_HEX: &str = "ef865f3fe0a28dc8f16a717f96bdd3b5abdead62e2e43e77a6e25da26754ef86";
const KIND: &str = "relay-ops";

#[derive(Deserialize)]
struct Release {
    version: String,
    sha256: String,
    #[serde(default)]
    signature: String,
}

fn parse(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.trim().parse().unwrap_or(0))
        .collect()
}

pub fn newer(latest: &str, current: &str) -> bool {
    parse(latest) > parse(current)
}

pub fn signature_ok(pubkey_hex: &str, version: &str, sha256: &str, signature_hex: &str) -> bool {
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
    let msg = format!("{KIND}|{version}|{}", sha256.to_ascii_lowercase());
    key.verify(msg.as_bytes(), &Signature::from_bytes(&sig))
        .is_ok()
}

async fn check(server: &str) -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let rel: Release = client
        .get(format!("{server}/api/app/ops/latest"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if !newer(&rel.version, crate::VERSION) {
        return Ok(false);
    }
    if !signature_ok(PUBKEY_HEX, &rel.version, &rel.sha256, &rel.signature) {
        return Err(format!(
            "la versione {} non ha una firma valida: la ignoro",
            rel.version
        ));
    }
    let bytes = client
        .get(format!("{server}/api/app/ops/download"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    if !hex::encode(Sha256::digest(&bytes)).eq_ignore_ascii_case(&rel.sha256) {
        return Err("file scaricato diverso da quello firmato".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("new");
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::rename(&tmp, &exe)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("aggiornato alla versione {}: riavvio", rel.version);
    Ok(true)
}

pub async fn run(server: String) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        match check(&server).await {
            Ok(true) => std::process::exit(0),
            Ok(false) => {}
            Err(e) => tracing::warn!("aggiornamento: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.3.47", "0.3.46"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.3.46", "0.3.46"));
        assert!(!newer("0.3.5", "0.3.46"));
    }

    #[test]
    fn a_bad_signature_is_rejected() {
        assert!(!signature_ok(PUBKEY_HEX, "0.3.47", "abcd", ""));
        assert!(!signature_ok(
            PUBKEY_HEX,
            "0.3.47",
            "abcd",
            &"00".repeat(64)
        ));
    }
}
