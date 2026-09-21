//! Minimal Electron ASAR archive reader (read-only).
//!
//! Format: a Pickle-wrapped JSON header describing the file tree, followed
//! by concatenated file contents. We keep the whole archive in memory -
//! extension packages are a few MB.

use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AsarError {
    #[error("not a valid asar archive: {0}")]
    Invalid(&'static str),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file not found in archive: {0}")]
    NotFound(String),
}

#[derive(Debug)]
pub struct AsarArchive {
    bytes: Vec<u8>,
    content_offset: usize,
    /// relative path (lowercased? no - exact, without leading slash) -> (offset, size)
    files: HashMap<String, (u64, u64)>,
}

impl AsarArchive {
    pub fn open(path: &Path) -> Result<AsarArchive, AsarError> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<AsarArchive, AsarError> {
        if bytes.len() < 16 {
            return Err(AsarError::Invalid("too small"));
        }
        let u32_at = |off: usize| -> Result<u32, AsarError> {
            Ok(u32::from_le_bytes(
                bytes.get(off..off + 4).ok_or(AsarError::Invalid("truncated"))?.try_into().unwrap(),
            ))
        };
        if u32_at(0)? != 4 {
            return Err(AsarError::Invalid("bad pickle magic"));
        }
        let header_size = u32_at(4)? as usize;
        let json_len = u32_at(12)? as usize;
        let json_start = 16;
        if json_start + json_len > bytes.len() {
            return Err(AsarError::Invalid("header out of bounds"));
        }
        let json = String::from_utf8_lossy(&bytes[json_start..json_start + json_len]);
        let header: serde_json::Value = serde_json::from_str(&json)
            .map_err(|_| AsarError::Invalid("header json"))?;
        let content_offset = 8usize.checked_add(header_size)
            .ok_or(AsarError::Invalid("header out of bounds"))?;

        let mut files = HashMap::new();
        collect_files(header.get("files"), String::new(), &mut files)?;

        Ok(AsarArchive { bytes, content_offset, files })
    }

    /// Read one file from the archive by relative path (`index.js`,
    /// `node_modules/pkg/index.js`, ...).
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, AsarError> {
        let rel = rel.trim_start_matches(['/', '\\']).replace('\\', "/");
        let (offset, size) = self
            .files
            .get(&rel)
            .copied()
            .ok_or_else(|| AsarError::NotFound(rel.clone()))?;
        // Offset/size come from the (attacker-controllable) header: range
        // math must be overflow-checked, or a wrapped `end` slips past the
        // bounds check and the slice panics.
        let Some((start, end)) = content_range(self.content_offset, offset, size, self.bytes.len())
        else {
            return Err(AsarError::Invalid("content out of bounds"));
        };
        Ok(self.bytes[start..end].to_vec())
    }

    pub fn exists(&self, rel: &str) -> bool {
        let rel = rel.trim_start_matches(['/', '\\']).replace('\\', "/");
        self.files.contains_key(&rel)
    }

    /// All file paths in the archive (sorted for deterministic extraction).
    pub fn file_list(&self) -> Vec<String> {
        let mut list: Vec<String> = self.files.keys().cloned().collect();
        list.sort();
        list
    }
}

/// Checked `bytes[base+offset .. base+offset+size]` range, `None` when the
/// numbers overflow or reach past the buffer.
fn content_range(base: usize, offset: u64, size: u64, len: usize) -> Option<(usize, usize)> {
    let start = usize::try_from(offset).ok()?.checked_add(base)?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    (end <= len).then_some((start, end))
}

/// Header entry names become real paths on extraction; a `..` (or drive
/// letter, separator, control char) in a name would write outside the
/// extraction target, so such archives are rejected outright.
fn is_unsafe_component(name: &str) -> bool {
    name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.bytes().any(|b| b < 0x20)
}

fn collect_files(
    node: Option<&serde_json::Value>,
    prefix: String,
    out: &mut HashMap<String, (u64, u64)>,
) -> Result<(), AsarError> {
    let Some(node) = node.and_then(|v| v.as_object()) else {
        return Ok(());
    };
    for (name, val) in node {
        if is_unsafe_component(name) {
            return Err(AsarError::Invalid("unsafe entry name in header"));
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if val.get("files").is_some() {
            // descend into the child's "files" map, not the child itself
            collect_files(val.get("files"), path, out)?;
        } else if let (Some(offset), Some(size)) = (
            val.get("offset").and_then(|v| v.as_str()),
            val.get("size").and_then(|v| v.as_u64()),
        ) {
            let offset = offset.parse::<u64>().unwrap_or(0);
            out.insert(path, (offset, size));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny asar in memory using the same layout the electron tool
    /// produces: 16-byte pickle header + json tree + contents.
    fn build_test_asar() -> Vec<u8> {
        let json = r#"{"files":{"index.js":{"size":11,"offset":"0"},"lib":{"files":{"a.js":{"size":5,"offset":"11"}}}}}"#;
        let json_bytes = json.as_bytes();
        let json_len = json_bytes.len();
        // real asar: pickle payload = 4 (size field) + 4 (json len) + json + pad
        let header_size = 8 + json_len + pad(json_len);
        let mut out = Vec::new();
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(header_size as u32).to_le_bytes());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(json_len as u32).to_le_bytes());
        out.extend_from_slice(json_bytes);
        out.extend(std::iter::repeat(0).take(pad(json_len)));
        out.extend_from_slice(b"hello world");
        out.extend_from_slice(b"chunk");
        return out;

        fn pad(n: usize) -> usize {
            (4 - (n % 4)) % 4
        }
    }

    #[test]
    fn reads_files_from_archive() {
        let arch = AsarArchive::from_bytes(build_test_asar()).unwrap();
        assert_eq!(arch.read("index.js").unwrap(), b"hello world");
        assert_eq!(arch.read("lib/a.js").unwrap(), b"chunk");
        assert!(arch.exists("lib/a.js"));
        assert!(!arch.exists("nope.js"));
        assert!(matches!(
            arch.read("nope.js"),
            Err(AsarError::NotFound(_))
        ));
    }

    fn build_asar_with_json(json: &str) -> Vec<u8> {
        let json_bytes = json.as_bytes();
        let json_len = json_bytes.len();
        let header_size = 8 + json_len + (4 - (json_len % 4)) % 4;
        let mut out = Vec::new();
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(header_size as u32).to_le_bytes());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(json_len as u32).to_le_bytes());
        out.extend_from_slice(json_bytes);
        out.extend(std::iter::repeat(0).take((4 - (json_len % 4)) % 4));
        out.extend_from_slice(b"content-bytes");
        out
    }

    #[test]
    fn traversal_names_are_rejected_at_open() {
        // A crafted header whose entry names escape the extraction target
        // (`../Startup/x.js`, absolute paths, drive letters) must be
        // rejected when the archive is parsed, not written to disk.
        for evil in [
            r#"{"files":{"../evil.js":{"size":5,"offset":"0"}}}"#,
            r#"{"files":{"..\\evil.js":{"size":5,"offset":"0"}}}"#,
            r#"{"files":{"C:\\evil.js":{"size":5,"offset":"0"}}}"#,
            r#"{"files":{"a/../../evil.js":{"size":5,"offset":"0"}}}"#,
            r#"{"files":{"nested":{"files":{"..\\up.js":{"size":5,"offset":"0"}}}}}"#,
        ] {
            let err = AsarArchive::from_bytes(build_asar_with_json(evil)).unwrap_err();
            assert!(matches!(err, AsarError::Invalid(_)), "{evil}: {err:?}");
        }
    }

    #[test]
    fn overflowing_offsets_error_instead_of_panicking() {
        let json = r#"{"files":{"index.js":{"size":18446744073709551615,"offset":"0"}}}"#;
        let arch = AsarArchive::from_bytes(build_asar_with_json(json)).unwrap();
        assert!(matches!(arch.read("index.js"), Err(AsarError::Invalid(_))));

        let json = r#"{"files":{"index.js":{"size":4,"offset":"18446744073709551615"}}}"#;
        let arch = AsarArchive::from_bytes(build_asar_with_json(json)).unwrap();
        assert!(matches!(arch.read("index.js"), Err(AsarError::Invalid(_))));
    }
}
