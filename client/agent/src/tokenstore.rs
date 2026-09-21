use std::sync::Mutex;

use anyhow::{Context, Result};

pub trait TokenStore: Send + Sync {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, token: &str) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

pub struct KeyringStore {
    service: String,
    user: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self::with_service("relay-agent")
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.into(),
            user: "default".into(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &self.user).context("accesso al keyring non riuscito")
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for KeyringStore {
    fn get(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("lettura dal keyring non riuscita"),
        }
    }

    fn set(&self, token: &str) -> Result<()> {
        self.entry()?
            .set_password(token)
            .context("scrittura nel keyring non riuscita")
    }

    fn clear(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("cancellazione dal keyring non riuscita"),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryStore {
    fn get(&self) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set(&self, token: &str) -> Result<()> {
        *self.0.lock().unwrap() = Some(token.into());
        Ok(())
    }
    fn clear(&self) -> Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_roundtrip() {
        let s = MemoryStore::new();
        assert_eq!(s.get().unwrap(), None);
        s.set("a").unwrap();
        assert_eq!(s.get().unwrap().as_deref(), Some("a"));
        s.clear().unwrap();
        assert_eq!(s.get().unwrap(), None);
    }

    #[test]
    #[ignore]
    fn keyring_reale_600_caratteri() {
        let s = KeyringStore::with_service("relay-agent-test");
        let v: String = (0..600).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        s.clear().unwrap();
        s.set(&v).unwrap();
        assert_eq!(s.get().unwrap().as_deref(), Some(v.as_str()));
        s.clear().unwrap();
        assert_eq!(s.get().unwrap(), None);
    }
}
