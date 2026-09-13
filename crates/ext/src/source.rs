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
/// temp dir once at startup (the original desktop app did exactly the
/// same - `os.tmpdir()/deckboard-extensions`).
pub struct PackageSource {
    pub package: String,
    pub root: PathBuf,
    _keep_asar: Option<AsarArchive>,
}

impl PackageSource {
    /// `path` is either a directory or an `.asar` file. The returned root
    /// is always a real directory on disk.
    pub fn open(path: &Path, package: String) -> Result<PackageSource, SourceError> {
        if path.is_dir() {
            Ok(PackageSource { package, root: path.to_path_buf(), _keep_asar: None })
        } else {
            let arch = AsarArchive::open(path)?;
            let target = temp_ext_dir(&package);
            extract_asar(&arch, &target)?;
            Ok(PackageSource { package, root: target, _keep_asar: Some(arch) })
        }
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

/// Temp extraction root, mirroring the original app's layout.
pub fn temp_ext_root() -> PathBuf {
    std::env::temp_dir().join("deckboard-extensions")
}

fn temp_ext_dir(package: &str) -> PathBuf {
    temp_ext_root().join(package)
}

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
