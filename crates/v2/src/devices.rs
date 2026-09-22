//! Paired devices (`~/deckboard/devices.json`) and one-time pairing codes
//! (docs/protocol-v2.md §3). One token per device: revoking a device is
//! removing its entry; a leaked token never widens beyond one tablet.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Alphabet for pairing codes: Crockford-style base32 without the
/// confusable 0/O/1/I.
pub const PAIR_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ234567";
pub const PAIR_CODE_LEN: usize = 8;
pub const PAIR_CODE_TTL: Duration = Duration::from_secs(300);

pub fn unix_now() -> u64 {
    crate::unix_millis() / 1000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    pub token: String,
    pub created: u64,
    pub last_seen: u64,
}

/// Device registry backed by a JSON file. Loads eagerly; every mutation
/// persists atomically (write temp + rename) so a crash never truncates
/// the file.
pub struct DeviceStore {
    path: PathBuf,
    entries: std::sync::Mutex<Vec<DeviceEntry>>,
    /// `touch`-only saves are debounced to one per minute: hello arrives on
    /// every (re)connect and `last_seen` is metadata, not data.
    last_touch_save: std::sync::Mutex<Option<std::time::Instant>>,
}

impl DeviceStore {
    /// Missing file = no paired devices yet; unreadable file = refuse to
    /// start rather than silently wiping the registry.
    pub fn load(path: PathBuf) -> std::io::Result<DeviceStore> {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(DeviceStore {
            path,
            entries: std::sync::Mutex::new(entries),
            last_touch_save: std::sync::Mutex::new(None),
        })
    }

    /// Token -> device, or `None` for unknown tokens. Comparison runs on
    /// sha-256 digests so the match leaks nothing about the token itself.
    pub fn verify(&self, token: &str) -> Option<DeviceEntry> {
        let wanted = Sha256::digest(token);
        self.entries
            .lock()
            .expect("device store poisoned")
            .iter()
            .find(|d| Sha256::digest(d.token.as_bytes()) == wanted)
            .cloned()
    }

    pub fn touch(&self, id: &str) -> Option<DeviceEntry> {
        self.update(id, None)
    }

    /// hello.name may rename a paired device; persisted with the touch.
    /// Returns the updated entry so the caller's `welcome` reflects it.
    pub fn rename(&self, id: &str, name: &str) -> Option<DeviceEntry> {
        self.update(id, Some(name.to_string()))
    }

    fn update(&self, id: &str, name: Option<String>) -> Option<DeviceEntry> {
        let is_rename = name.is_some();
        let updated = {
            let mut entries = self.entries.lock().expect("device store poisoned");
            let entry = entries.iter_mut().find(|d| d.id == id)?;
            entry.last_seen = unix_now();
            if let Some(name) = name {
                entry.name = name;
            }
            entry.clone()
        };
        if is_rename {
            self.save();
            return Some(updated);
        }
        // Debounced last_seen persistence: at most one file write a minute.
        let mut last_save = self.last_touch_save.lock().expect("device store poisoned");
        let due = last_save.is_none_or(|at| at.elapsed() >= Duration::from_secs(60));
        if due {
            *last_save = Some(std::time::Instant::now());
            drop(last_save);
            self.save();
        }
        Some(updated)
    }

    /// Creates a device with a fresh random id + token (pairing step 4).
    pub fn create(&self, name: &str) -> DeviceEntry {
        let entry = DeviceEntry {
            id: hex_encode(&random_bytes(16)),
            name: name.to_string(),
            token: hex_encode(&random_bytes(32)),
            created: unix_now(),
            last_seen: unix_now(),
        };
        self.entries
            .lock()
            .expect("device store poisoned")
            .push(entry.clone());
        self.save();
        entry
    }

    pub fn revoke(&self, id: &str) -> bool {
        let mut entries = self.entries.lock().expect("device store poisoned");
        let before = entries.len();
        entries.retain(|d| d.id != id);
        let removed = entries.len() != before;
        drop(entries);
        if removed {
            self.save();
        }
        removed
    }

    pub fn list(&self) -> Vec<DeviceEntry> {
        self.entries.lock().expect("device store poisoned").clone()
    }

    fn save(&self) {
        let entries = self.entries.lock().expect("device store poisoned");
        let tmp = self.path.with_extension("json.tmp");
        let write = std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&*entries).unwrap_or_default(),
        )
        .and_then(|_| std::fs::rename(&tmp, &self.path));
        if let Err(e) = write {
            tracing::warn!(path = %self.path.display(), error = %e, "cannot persist devices.json");
        }
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|_| rand::thread_rng().gen()).collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PairError {
    /// Unknown code.
    Invalid,
    /// Known code but older than `PAIR_CODE_TTL`.
    Expired,
}

/// In-memory pool of live one-time codes. Codes burn on use, valid or not:
/// a second connection attempt with the same code always fails.
#[derive(Default)]
pub struct Pairing {
    codes: std::sync::Mutex<HashMap<String, Instant>>,
}

impl Pairing {
    pub fn new() -> Pairing {
        Pairing::default()
    }

    pub fn new_code(&self) -> String {
        let mut rng = rand::thread_rng();
        let code: String = (0..PAIR_CODE_LEN)
            .map(|_| PAIR_ALPHABET[rng.gen_range(0..PAIR_ALPHABET.len())] as char)
            .collect();
        // Purge expired codes so the map cannot grow without bound.
        self.codes
            .lock()
            .expect("pairing poisoned")
            .retain(|_, created| created.elapsed() < PAIR_CODE_TTL);
        self.codes
            .lock()
            .expect("pairing poisoned")
            .insert(code.clone(), Instant::now());
        code
    }

    pub fn peek(&self, code: &str) -> Result<(), PairError> {
        let codes = self.codes.lock().expect("pairing poisoned");
        match codes.get(code) {
            None => Err(PairError::Invalid),
            Some(created) if created.elapsed() >= PAIR_CODE_TTL => Err(PairError::Expired),
            Some(_) => Ok(()),
        }
    }

    pub fn consume(&self, code: &str) -> Result<(), PairError> {
        // Lookup and remove under one lock: with separate scopes two
        // concurrent consumes could both pass the check and both mint
        // devices off a single one-time code.
        let mut codes = self.codes.lock().expect("pairing poisoned");
        match codes.get(code) {
            None => Err(PairError::Invalid),
            Some(created) if created.elapsed() >= PAIR_CODE_TTL => Err(PairError::Expired),
            Some(_) => {
                codes.remove(code);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_codes_are_one_time_and_expiring() {
        let pairing = Pairing::new();
        let code = pairing.new_code();
        assert_eq!(code.len(), PAIR_CODE_LEN);
        assert_eq!(pairing.peek(&code), Ok(()));
        assert_eq!(pairing.consume(&code), Ok(()));
        assert_eq!(pairing.consume(&code), Err(PairError::Invalid));
        assert_eq!(pairing.consume("NOPE2345"), Err(PairError::Invalid));
    }

    #[test]
    fn devices_persist_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let store = DeviceStore::load(path.clone()).unwrap();
        assert!(store.verify("nope").is_none());
        let device = store.create("Tablet salon");
        assert_eq!(device.name, "Tablet salon");
        assert_eq!(store.verify(&device.token).unwrap().id, device.id);

        // A second store instance over the same file sees the device.
        let reopened = DeviceStore::load(path).unwrap();
        assert_eq!(reopened.verify(&device.token).unwrap().id, device.id);
        assert!(reopened.revoke(&device.id));
        assert!(reopened.verify(&device.token).is_none());
    }
}
