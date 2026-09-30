use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const TUNNEL_PATH: &str = "/api/storage/tunnel";
pub const PROTOCOL_VERSION: u32 = 1;
pub const PLAIN_CHUNK: u64 = 1 << 20;
pub const TAG_LEN: u64 = 16;
pub const DATA_FRAME: usize = 256 * 1024;

pub const KIND_HELLO: u8 = 1;
pub const KIND_REQUEST: u8 = 2;
pub const KIND_RESPONSE: u8 = 3;
pub const KIND_DATA: u8 = 4;
pub const KIND_END: u8 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Put {
        blob: String,
        offset: u64,
    },
    Commit {
        blob: String,
        size: u64,
        sha256: String,
    },
    Get {
        blob: String,
        offset: u64,
        len: u64,
    },
    Delete {
        blob: String,
    },
    Abort {
        blob: String,
    },
    Stat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Stats(NodeStats),
    Error { message: String },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeStats {
    pub total: u64,
    pub free: u64,
    pub used: u64,
    pub blobs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub version: String,
    pub protocol: u32,
    pub conn: u32,
    pub stats: NodeStats,
}

pub fn valid_blob(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub fn encode<H: Serialize>(kind: u8, id: u64, header: &H, payload: &[u8]) -> Vec<u8> {
    let head = serde_json::to_vec(header).unwrap_or_else(|_| b"null".to_vec());
    let mut out = Vec::with_capacity(13 + head.len() + payload.len());
    out.push(kind);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(payload);
    out
}

pub struct Frame<'a> {
    pub kind: u8,
    pub id: u64,
    pub header: &'a [u8],
    pub payload: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let kind = *bytes.first()?;
        let id = u64::from_le_bytes(bytes.get(1..9)?.try_into().ok()?);
        let len = u32::from_le_bytes(bytes.get(9..13)?.try_into().ok()?) as usize;
        let header = bytes.get(13..13 + len)?;
        let payload = bytes.get(13 + len..)?;
        Some(Self {
            kind,
            id,
            header,
            payload,
        })
    }

    pub fn header<H: DeserializeOwned>(&self) -> Option<H> {
        serde_json::from_slice(self.header).ok()
    }
}

pub fn cipher_len(plain: u64) -> u64 {
    plain + plain.div_ceil(PLAIN_CHUNK).max(1) * TAG_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let bytes = encode(
            KIND_REQUEST,
            42,
            &Request::Get {
                blob: "x".into(),
                offset: 5,
                len: 9,
            },
            b"abc",
        );
        let f = Frame::parse(&bytes).unwrap();
        assert_eq!((f.kind, f.id, f.payload), (KIND_REQUEST, 42, &b"abc"[..]));
        assert_eq!(
            f.header::<Request>(),
            Some(Request::Get {
                blob: "x".into(),
                offset: 5,
                len: 9
            })
        );
        assert!(Frame::parse(&bytes[..10]).is_none());
    }

    #[test]
    fn responses_are_tagged() {
        let s = serde_json::to_string(&Response::Error {
            message: "no".into(),
        })
        .unwrap();
        assert_eq!(s, r#"{"status":"error","message":"no"}"#);
    }

    #[test]
    fn blob_ids_are_uuids() {
        assert!(valid_blob("0f8f2a1e-6f3c-4f8e-9a51-2a7b3c4d5e6f"));
        assert!(!valid_blob("../../etc/passwd"));
    }

    #[test]
    fn cipher_length_counts_one_tag_per_chunk() {
        assert_eq!(cipher_len(0), 16);
        assert_eq!(cipher_len(10), 26);
        assert_eq!(cipher_len(PLAIN_CHUNK), PLAIN_CHUNK + 16);
        assert_eq!(cipher_len(PLAIN_CHUNK + 1), PLAIN_CHUNK + 1 + 32);
    }
}
