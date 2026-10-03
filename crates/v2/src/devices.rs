//! Paired devices (`~/pulpitApp/devices.json`) and one-time pairing codes
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

/// Wrong pairing codes within one TTL window that burn every
/// outstanding code. Codes are 8 chars from a 31-char alphabet - short
/// enough that unlimited free guesses would eventually hit, so after
/// this many misses the operator mints a fresh one instead.
const MAX_FAILED_PAIR_ATTEMPTS: usize = 5;

/// Marker + sha-256 of a device token, as stored in `devices.json`
/// (`sha256:<hex>`). Only digests are persisted: the file is as
/// sensitive as the tokens themselves otherwise, and every verify
/// already compares digests. The prefix keeps a stored digest
/// distinguishable from a legacy plaintext token (both are hex-shaped)
/// so migration is one-way and idempotent.
const DIGEST_PREFIX: &str = "sha256:";

fn token_digest(token: &str) -> String {
    format!("{DIGEST_PREFIX}{:x}", Sha256::digest(token.as_bytes()))
}

fn is_digest(stored: &str) -> bool {
    let Some(hex) = stored.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

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
    /// start rather than silently wiping the registry. Legacy files with
    /// plaintext tokens migrate to digests on load (one-way, persisted).
    pub fn load(path: PathBuf) -> std::io::Result<DeviceStore> {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        let store = DeviceStore {
            path,
            entries: std::sync::Mutex::new(entries),
            last_touch_save: std::sync::Mutex::new(None),
        };
        store.migrate_plaintext_tokens();
        Ok(store)
    }

    /// Replaces any legacy plaintext token with its digest and persists.
    /// `sha256:`-prefixed values pass through untouched, so this is safe
    /// to run on every load.
    fn migrate_plaintext_tokens(&self) {
        let mut entries = self.entries.lock().expect("device store poisoned");
        let mut migrated = 0;
        for entry in entries.iter_mut() {
            if !is_digest(&entry.token) {
                entry.token = token_digest(&entry.token);
                migrated += 1;
            }
        }
        if migrated > 0 {
            tracing::info!(
                migrated,
                "devices.json: plaintext tokens migrated to digests"
            );
            drop(entries);
            self.save();
        }
    }

    /// Token -> device, or `None` for unknown tokens. Comparison runs on
    /// sha-256 digests so the match leaks nothing about the token itself.
    pub fn verify(&self, token: &str) -> Option<DeviceEntry> {
        let wanted = token_digest(token);
        self.entries
            .lock()
            .expect("device store poisoned")
            .iter()
            .find(|d| d.token == wanted)
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
    /// Only the token's digest is stored; the plaintext travels exactly
    /// once, in this return value, on its way to the new device's
    /// `welcome`.
    pub fn create(&self, name: &str) -> (DeviceEntry, String) {
        let plaintext = hex_encode(&random_bytes(32));
        let entry = DeviceEntry {
            id: hex_encode(&random_bytes(16)),
            name: name.to_string(),
            token: token_digest(&plaintext),
            created: unix_now(),
            last_seen: unix_now(),
        };
        self.entries
            .lock()
            .expect("device store poisoned")
            .push(entry.clone());
        self.save();
        (entry, plaintext)
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
        let bytes = {
            let entries = self.entries.lock().expect("device store poisoned");
            serde_json::to_vec_pretty(&*entries).unwrap_or_default()
        };
        if let Err(e) = pulpit_db::write_atomic(&self.path, &bytes) {
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

/// The desktop's fresh-pairing approval decision: the sanitized device
/// name in, trust out. Blocking by contract - see [`Pairing::ask_trust`].
type TrustGate = dyn Fn(&str) -> bool + Send + Sync;

/// In-memory pool of live one-time codes. Codes burn on use, valid or not:
/// a second connection attempt with the same code always fails. A burst
/// of wrong codes (a guessing client) burns every outstanding code.
///
/// Fresh pairings also pass an operator gate ([`Pairing::set_trust_gate`]):
/// minting a device needs the desktop's trust decision (audit B2 step 6).
#[derive(Default)]
pub struct Pairing {
    codes: std::sync::Mutex<HashMap<String, Instant>>,
    /// Wrong-code attempts inside the TTL window (the failed-attempt
    /// budget's sliding window).
    failures: std::sync::Mutex<Vec<Instant>>,
    /// How long a code stays valid. `PAIR_CODE_TTL` in production;
    /// tests shrink it so expiry is observable without waiting.
    ttl: Duration,
    /// The fresh-pairing approval gate; `None` (headless builds, tests)
    /// keeps the pre-UI auto-accept behavior, with its warning log.
    trust: std::sync::Mutex<Option<std::sync::Arc<TrustGate>>>,
}

impl Pairing {
    pub fn new() -> Pairing {
        Pairing {
            ttl: PAIR_CODE_TTL,
            ..Pairing::default()
        }
    }

    /// A pool whose codes expire after `ttl` - the test seam for the
    /// expiry paths (production always uses [`Pairing::new`]).
    pub fn with_ttl(ttl: Duration) -> Pairing {
        Pairing {
            ttl,
            ..Pairing::default()
        }
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
            .retain(|_, created| created.elapsed() < self.ttl);
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
            Some(created) if created.elapsed() >= self.ttl => Err(PairError::Expired),
            Some(_) => Ok(()),
        }
    }

    pub fn consume(&self, code: &str) -> Result<(), PairError> {
        // Lookup and remove under one lock: with separate scopes two
        // concurrent consumes could both pass the check and both mint
        // devices off a single one-time code.
        let mut codes = self.codes.lock().expect("pairing poisoned");
        let outcome = match codes.get(code) {
            None => Err(PairError::Invalid),
            Some(created) if created.elapsed() >= self.ttl => Err(PairError::Expired),
            Some(_) => Ok(()),
        };
        match outcome {
            Ok(()) => {
                codes.remove(code);
                Ok(())
            }
            Err(PairError::Invalid) => {
                // A wrong code is a guess at an 8-char secret: budget
                // them, and past the budget invalidate everything
                // outstanding (the operator re-mints).
                drop(codes);
                if self.record_failed_attempt() {
                    self.codes.lock().expect("pairing poisoned").clear();
                    tracing::warn!(
                        attempts = MAX_FAILED_PAIR_ATTEMPTS,
                        "too many wrong pairing codes - outstanding codes invalidated"
                    );
                }
                Err(PairError::Invalid)
            }
            Err(PairError::Expired) => Err(PairError::Expired),
        }
    }

    /// Records one wrong-code attempt; `true` when the budget is spent.
    fn record_failed_attempt(&self) -> bool {
        let mut failures = self.failures.lock().expect("pairing poisoned");
        failures.push(Instant::now());
        failures.retain(|at| at.elapsed() < self.ttl);
        failures.len() >= MAX_FAILED_PAIR_ATTEMPTS
    }

    /// Installs the operator-approval gate for fresh pairings (the
    /// desktop does this at startup; headless builds keep the
    /// auto-accept default). Receives the sanitized device name.
    pub fn set_trust_gate<F>(&self, gate: F)
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        *self.trust.lock().expect("pairing poisoned") = Some(std::sync::Arc::new(gate));
    }

    /// Consults the trust gate about a fresh pairing. Blocking on
    /// purpose: the session calls it from its blocking pool and bounds
    /// the wait by the pairing-code TTL - a gate that never answers
    /// (nobody at the desktop) denies once the code would have expired
    /// anyway. `false` rejects the hello.
    pub fn ask_trust(&self, name: &str) -> bool {
        let gate = self.trust.lock().expect("pairing poisoned").clone();
        match gate {
            Some(gate) => gate(name),
            None => {
                tracing::warn!(name = %name, "pairing auto-accepted (no trust gate installed)");
                true
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
    fn concurrent_consumers_burn_one_code_exactly_once() {
        let pairing = std::sync::Arc::new(Pairing::new());
        let code = pairing.new_code();
        let winners = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..16 {
                let pairing = pairing.clone();
                let code = code.clone();
                let winners = &winners;
                s.spawn(move || {
                    if pairing.consume(&code).is_ok() {
                        winners.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(
            winners.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "exactly one consumer may mint off a one-time code"
        );
    }

    #[test]
    fn pairing_gate_decides_fresh_devices() {
        let pairing = Pairing::new();
        // no gate installed: the headless default auto-accepts (the
        // pre-UI behavior, with its warning log)
        assert!(pairing.ask_trust("Tablet salon"));

        // an installed gate decides: this one approves the salon
        // tablet and denies the stranger
        let (ask_tx, ask_rx) = std::sync::mpsc::channel::<String>();
        pairing.set_trust_gate(move |name| {
            let _ = ask_tx.send(name.to_string());
            name == "Tablet salon"
        });
        assert!(pairing.ask_trust("Tablet salon"));
        assert!(!pairing.ask_trust("Stranger"));
        assert_eq!(ask_rx.recv().unwrap(), "Tablet salon");
        assert_eq!(ask_rx.recv().unwrap(), "Stranger");
    }

    #[test]
    fn wrong_codes_burn_every_outstanding_code() {
        let pairing = Pairing::new();
        let a = pairing.new_code();
        let b = pairing.new_code();
        // one under the budget: outstanding codes survive
        for _ in 0..MAX_FAILED_PAIR_ATTEMPTS - 1 {
            assert_eq!(pairing.consume("WRONGCOD"), Err(PairError::Invalid));
        }
        assert_eq!(pairing.peek(&a), Ok(()));
        // the budget's last attempt invalidates everything outstanding
        assert_eq!(pairing.consume("WRONGCOD"), Err(PairError::Invalid));
        assert_eq!(pairing.peek(&a), Err(PairError::Invalid));
        assert_eq!(pairing.peek(&b), Err(PairError::Invalid));
        // minting still works; the operator relays a fresh code
        let c = pairing.new_code();
        assert_eq!(pairing.peek(&c), Ok(()));
        // and a valid consume still succeeds after the burn
        assert_eq!(pairing.consume(&c), Ok(()));
    }

    #[test]
    fn devices_persist_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let store = DeviceStore::load(path.clone()).unwrap();
        assert!(store.verify("nope").is_none());
        let (device, plaintext) = store.create("Tablet salon");
        assert_eq!(device.name, "Tablet salon");
        // only the digest is stored; the plaintext verifies against it
        assert!(is_digest(&device.token));
        assert_eq!(store.verify(&plaintext).unwrap().id, device.id);

        // The file holds the digest, never the plaintext token.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("sha256:"));
        assert!(!on_disk.contains(&plaintext));

        // A second store instance over the same file sees the device.
        let reopened = DeviceStore::load(path).unwrap();
        assert_eq!(reopened.verify(&plaintext).unwrap().id, device.id);
        assert!(reopened.revoke(&device.id));
        assert!(reopened.verify(&plaintext).is_none());
    }

    #[test]
    fn legacy_plaintext_tokens_migrate_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        // an obvious fake token, plaintext like the pre-digest format
        let legacy_token = "f4k3-legacy-plaintext-token-000000000000";
        let file = format!(
            r#"[{{"id":"aa","name":"Old tablet","token":"{legacy_token}","created":1,"last_seen":2}}]"#
        );
        std::fs::write(&path, file).unwrap();

        let store = DeviceStore::load(path.clone()).unwrap();
        // the legacy token still verifies after migration
        assert_eq!(store.verify(legacy_token).unwrap().id, "aa");
        assert!(is_digest(&store.list()[0].token));

        // migration is persisted and idempotent across reloads
        let reopened = DeviceStore::load(path.clone()).unwrap();
        assert_eq!(reopened.verify(legacy_token).unwrap().id, "aa");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains(legacy_token));
        assert!(on_disk.contains("sha256:"));
    }
}
