//! Content-addressed asset store (docs/protocol-v2.md §7):
//! `~/deckboard/assets/<sha256-hex>.<ext>`. Imports are idempotent by
//! hash; legacy DB data URLs convert into the same store when boards are
//! built. Served from `GET /assets/<hash>?token=...` with immutable cache
//! headers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use sha2::{Digest, Sha256};

pub struct AssetStore {
    dir: PathBuf,
    /// hash -> file extension, rebuilt from the directory at open.
    exts: Mutex<HashMap<String, String>>,
    /// data-URL fingerprint -> hash. Board rebuilds re-import the same URL
    /// strings on every generation bump; the fingerprint key (not the URL
    /// itself) keeps multi-MB base64 strings out of memory.
    url_hashes: Mutex<HashMap<[u8; 16], String>>,
}

impl AssetStore {
    pub fn open(dir: PathBuf) -> std::io::Result<AssetStore> {
        std::fs::create_dir_all(&dir)?;
        let mut exts = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some((hash, ext)) = split_stem(entry.file_name().to_string_lossy().as_ref())
                {
                    exts.insert(hash.to_string(), ext.to_string());
                }
            }
        }
        Ok(AssetStore {
            dir,
            exts: Mutex::new(exts),
            url_hashes: Mutex::new(HashMap::new()),
        })
    }

    /// Stores bytes under their sha-256 and returns the hex hash. Existing
    /// files with the same hash are left untouched.
    pub fn import_bytes(&self, bytes: &[u8], ext: &str) -> std::io::Result<String> {
        let ext = normalize_ext(ext);
        let hash = hex::encode(Sha256::digest(bytes));
        let path = self.dir.join(format!("{hash}.{ext}"));
        if !path.exists() {
            std::fs::write(&path, bytes)?;
        }
        self.exts
            .lock()
            .expect("asset store poisoned")
            .insert(hash.clone(), ext);
        Ok(hash)
    }

    /// `data:image/png;base64,....` -> store entry, hash returned. Returns
    /// `None` for anything that is not a base64 data URL with a mime type.
    /// Repeated URLs (board rebuilds) hit the fingerprint cache and skip
    /// the base64 decode + sha-256.
    pub fn import_data_url(&self, url: &str) -> Option<String> {
        let fingerprint = url_fingerprint(url);
        if let Some(hash) = self
            .url_hashes
            .lock()
            .expect("asset store poisoned")
            .get(&fingerprint)
        {
            return Some(hash.clone());
        }
        let rest = url.strip_prefix("data:")?;
        let (head, payload) = rest.split_once(',')?;
        let mime = head.strip_suffix(";base64")?;
        let ext = mime_to_ext(mime)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?;
        let hash = self.import_bytes(&bytes, ext).ok()?;
        let mut urls = self.url_hashes.lock().expect("asset store poisoned");
        // distinct images per install are far below this; the clear is a
        // safety valve so a pathological input cannot grow the map forever
        if urls.len() >= 512 {
            urls.clear();
        }
        urls.insert(fingerprint, hash.clone());
        Some(hash)
    }

    pub fn get(&self, hash: &str) -> Option<Vec<u8>> {
        let ext = self
            .exts
            .lock()
            .expect("asset store poisoned")
            .get(hash)
            .cloned()?;
        std::fs::read(self.dir.join(format!("{hash}.{ext}"))).ok()
    }

    pub fn content_type(&self, hash: &str) -> Option<&'static str> {
        let ext = self
            .exts
            .lock()
            .expect("asset store poisoned")
            .get(hash)
            .cloned()?;
        Some(content_type(&ext))
    }
}

/// sha-256 hex of stored assets; the route only serves these.
pub fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 128-bit fingerprint of a data URL: two independently seeded SipHash
/// passes, used as the memo key so the multi-MB URL strings themselves
/// never live in the cache. Deterministic within a process run, which is
/// all the in-memory cache needs.
fn url_fingerprint(url: &str) -> [u8; 16] {
    use std::hash::{Hash, Hasher};
    fn seeded(seed: u64, url: &str) -> [u8; 8] {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut h);
        url.hash(&mut h);
        h.finish().to_be_bytes()
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&seeded(1, url));
    out[8..].copy_from_slice(&seeded(2, url));
    out
}

fn split_stem(file_name: &str) -> Option<(&str, &str)> {
    let (hash, ext) = file_name.rsplit_once('.')?;
    is_valid_hash(hash).then_some((hash, ext))
}

fn normalize_ext(ext: &str) -> String {
    let cleaned: String = ext
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if cleaned.is_empty() {
        "bin".to_string()
    } else {
        cleaned
    }
}

fn mime_to_ext(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "audio/ogg" => Some("ogg"), // kept in sync with content_type()
        _ => None,
    }
}

pub fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_is_idempotent_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path().to_path_buf()).unwrap();
        let hash = store.import_bytes(b"hello", "png").unwrap();
        assert!(is_valid_hash(&hash));
        let again = store.import_bytes(b"hello", "png").unwrap();
        assert_eq!(hash, again);
        assert_eq!(store.get(&hash).unwrap(), b"hello");
        assert_eq!(store.content_type(&hash), Some("image/png"));
        assert!(store.get(&"0".repeat(64)).is_none());
        assert!(store.get("not-a-hash").is_none());
    }

    #[test]
    fn data_urls_convert() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path().to_path_buf()).unwrap();
        // 1x1 PNG from the base64 of "png-bytes"
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"png-bytes")
        );
        let hash = store.import_data_url(&url).unwrap();
        assert_eq!(store.get(&hash).unwrap(), b"png-bytes");
        assert_eq!(store.content_type(&hash), Some("image/png"));
        // jpeg alias maps to jpg, same bytes same hash
        let url2 = format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"png-bytes")
        );
        assert_eq!(store.import_data_url(&url2).unwrap(), hash);
        assert!(store.import_data_url("data:image/png,notbase64").is_none());
        assert!(store.import_data_url("https://x/y.png").is_none());
    }

    #[test]
    fn repeated_data_urls_hit_the_fingerprint_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = AssetStore::open(dir.path().to_path_buf()).unwrap();
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"cache-me")
        );
        let hash = store.import_data_url(&url).unwrap();
        // a garbage data URL cannot collide into the cached entry: the
        // cache only ever holds successfully imported URLs
        let garbage = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"other")
        );
        let _ = store.import_data_url(&garbage).unwrap();
        assert_eq!(store.url_hashes.lock().unwrap().len(), 2);
        // same URL again: same hash, no new cache entry, no decode work
        assert_eq!(store.import_data_url(&url).unwrap(), hash);
        assert_eq!(store.url_hashes.lock().unwrap().len(), 2);
        // a URL that fails to import (bad mime) must not poison the cache
        assert!(store.import_data_url("data:image/tiff;base64,Zm9v").is_none());
        assert!(!store
            .url_hashes
            .lock()
            .unwrap()
            .contains_key(&url_fingerprint("data:image/tiff;base64,Zm9v")));
    }

    #[test]
    fn store_rebuilds_index_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = AssetStore::open(dir.path().to_path_buf()).unwrap();
            store.import_bytes(b"persist", "webp").unwrap();
        }
        let reopened = AssetStore::open(dir.path().to_path_buf()).unwrap();
        let hash = hex::encode(Sha256::digest(b"persist"));
        assert_eq!(reopened.get(&hash).unwrap(), b"persist");
        assert_eq!(reopened.content_type(&hash), Some("image/webp"));
    }
}
