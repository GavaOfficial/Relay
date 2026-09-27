use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hmac::{Hmac, Mac};
use relay_common::storage::{PLAIN_CHUNK, TAG_LEN};
use sha2::Sha256;

pub struct BlobCipher(Aes256Gcm);

impl BlobCipher {
    pub fn new(master: &[u8; 32], blob: &str) -> Self {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master).expect("chiave HMAC");
        mac.update(blob.as_bytes());
        let key = mac.finalize().into_bytes();
        Self(Aes256Gcm::new_from_slice(&key).expect("chiave AES"))
    }

    fn nonce(index: u64) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[..8].copy_from_slice(&index.to_le_bytes());
        n
    }

    pub fn seal(&self, index: u64, plain: &[u8]) -> Vec<u8> {
        self.0
            .encrypt(Nonce::from_slice(&Self::nonce(index)), plain)
            .expect("cifratura")
    }

    pub fn open(&self, index: u64, cipher: &[u8]) -> Option<Vec<u8>> {
        self.0.decrypt(Nonce::from_slice(&Self::nonce(index)), cipher).ok()
    }
}

pub fn cipher_offset(index: u64) -> u64 {
    index * (PLAIN_CHUNK + TAG_LEN)
}

pub fn cipher_chunk_len(size: u64, index: u64) -> u64 {
    let start = index * PLAIN_CHUNK;
    size.saturating_sub(start).min(PLAIN_CHUNK) + TAG_LEN
}

pub fn chunks(size: u64) -> u64 {
    size.div_ceil(PLAIN_CHUNK).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_round_trip_and_are_bound_to_blob_and_position() {
        let master = [7u8; 32];
        let a = BlobCipher::new(&master, "blob-a");
        let sealed = a.seal(3, b"ciao");
        assert_eq!(sealed.len(), 4 + TAG_LEN as usize);
        assert_eq!(a.open(3, &sealed).as_deref(), Some(&b"ciao"[..]));
        assert!(a.open(4, &sealed).is_none(), "pezzo spostato");
        assert!(BlobCipher::new(&master, "blob-b").open(3, &sealed).is_none(), "altro blob");
        assert!(BlobCipher::new(&[8u8; 32], "blob-a").open(3, &sealed).is_none(), "altra chiave");
    }

    #[test]
    fn chunk_layout() {
        let size = PLAIN_CHUNK * 2 + 10;
        assert_eq!(chunks(size), 3);
        assert_eq!(cipher_chunk_len(size, 0), PLAIN_CHUNK + TAG_LEN);
        assert_eq!(cipher_chunk_len(size, 2), 10 + TAG_LEN);
        assert_eq!(cipher_offset(2), 2 * (PLAIN_CHUNK + TAG_LEN));
        assert_eq!(chunks(0), 1);
    }
}
