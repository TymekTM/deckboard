//! Extension package sources: `.asar` archives or plain directories.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::asar::AsarArchive;

#[derive(Error, Debug)]
pub enum SourceError {
    #[error("asar error: {0}")]
    Asar(#[from] crate::asar::AsarError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Where an extension package lives. Everything is normalized to a plain
/// directory before the JS host sees it: asar packages are extracted to a
/// temp dir once per open (see [`PackageSource::open`]).
pub struct PackageSource {
    pub package: String,
    pub root: PathBuf,
    keep: SourceKeep,
}

/// Extraction artifacts that must outlive every reader of a package's
/// `root`: the archive handle and the private temp dir an asar was
/// extracted into. Dropping the guard deletes that dir.
pub struct SourceKeep {
    _asar: Option<AsarArchive>,
    _tmp: Option<tempfile::TempDir>,
}

impl SourceKeep {
    fn nothing() -> SourceKeep {
        SourceKeep {
            _asar: None,
            _tmp: None,
        }
    }
}

impl PackageSource {
    /// `path` is either a directory or an `.asar` file. The returned root
    /// is always a real directory on disk; for archives it stays valid
    /// only while the [`SourceKeep`] from [`into_root`][Self::into_root]
    /// is alive.
    ///
    /// Archives extract into a fresh, randomly named temp dir per open:
    /// the original app's one shared predictable
    /// `%TEMP%\pulpit-extensions\<pkg>` target let any local process
    /// pre-plant or overwrite package code before it ran. The trade: a
    /// package writing state into its own extracted tree loses it across
    /// app restarts (it already only survived until the next reboot).
    pub fn open(path: &Path, package: String) -> Result<PackageSource, SourceError> {
        if path.is_dir() {
            Ok(PackageSource {
                package,
                root: path.to_path_buf(),
                keep: SourceKeep::nothing(),
            })
        } else {
            let arch = AsarArchive::open(path)?;
            let tmp = tempfile::Builder::new().prefix(EXT_TEMP_PREFIX).tempdir()?;
            extract_asar(&arch, tmp.path())?;
            Ok(PackageSource {
                package,
                root: tmp.path().to_path_buf(),
                keep: SourceKeep {
                    _asar: Some(arch),
                    _tmp: Some(tmp),
                },
            })
        }
    }

    /// Split into the extraction root and the guard that must outlive
    /// every reader of that root. Directory packages own nothing; asar
    /// packages own their temp dir (dropping the guard deletes it).
    pub fn into_root(self) -> (PathBuf, SourceKeep) {
        (self.root, self.keep)
    }

    pub fn entry_candidates(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let pkg_json = self.root.join("package.json");
        if let Ok(text) = fs::read_to_string(&pkg_json) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(main) = v.get("main").and_then(|m| m.as_str()) {
                    out.push(self.root.join(main));
                }
            }
        }
        out.push(self.root.join("index.js"));
        out
    }
}

/// Temp extraction prefix for asar packages; the directory itself is
/// randomly named by `tempfile` (see [`PackageSource::open`]).
const EXT_TEMP_PREFIX: &str = "pulpit-ext-";

fn extract_asar(arch: &AsarArchive, target: &Path) -> Result<(), SourceError> {
    fs::create_dir_all(target)?;
    // paths come from the archive tree; walk them via the header
    for rel in arch.file_list() {
        let dest = target.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = arch.read(&rel)?;
        fs::write(&dest, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny asar with the same layout the electron tool produces
    /// (16-byte pickle header + json tree + contents), one `index.js`.
    fn build_test_asar() -> Vec<u8> {
        let json = r#"{"files":{"index.js":{"size":11,"offset":"0"}}}"#;
        let json_bytes = json.as_bytes();
        let json_len = json_bytes.len();
        let header_size = 8 + json_len + (4 - (json_len % 4)) % 4;
        let mut out = Vec::new();
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(header_size as u32).to_le_bytes());
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(json_len as u32).to_le_bytes());
        out.extend_from_slice(json_bytes);
        out.extend(std::iter::repeat_n(0, (4 - (json_len % 4)) % 4));
        out.extend_from_slice(b"hello world");
        out
    }

    #[test]
    fn asar_extraction_is_private_per_open_and_disposable() {
        let dir = tempfile::tempdir().unwrap();
        let asar_path = dir.path().join("pkg.asar");
        std::fs::write(&asar_path, build_test_asar()).unwrap();

        let source = PackageSource::open(&asar_path, "pkg".to_string()).unwrap();
        // not the predictable shared path another process could pre-plant
        assert!(
            !source
                .root
                .starts_with(std::env::temp_dir().join("pulpit-extensions")),
            "extraction must not use a predictable directory"
        );
        // contents extracted correctly
        assert_eq!(
            std::fs::read_to_string(source.root.join("index.js")).unwrap(),
            "hello world"
        );

        // a second open gets its own directory
        let second = PackageSource::open(&asar_path, "pkg".to_string()).unwrap();
        assert_ne!(source.root, second.root, "each open extracts privately");
        drop(second);

        // dropping the keep-guard deletes the extraction
        let (root, keep) = source.into_root();
        drop(keep);
        assert!(!root.exists(), "dropping the guard must clean up");
    }

    #[test]
    fn directory_packages_keep_their_real_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/index.js"), "x").unwrap();

        let (root, _keep) =
            PackageSource::open(dir.path().join("pkg").as_path(), "pkg".to_string())
                .unwrap()
                .into_root();
        assert_eq!(root, dir.path().join("pkg"));
    }
}
