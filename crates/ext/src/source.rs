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
/// directory before the JS host sees it: asar packages are extracted once
/// per source version (see [`PackageSource::open`]).
pub struct PackageSource {
    pub package: String,
    pub root: PathBuf,
    keep: SourceKeep,
}

/// Extraction artifacts that must outlive every reader of a package's
/// `root`: the archive handle and the private temp dir a fallback
/// extraction was unpacked into. Dropping the guard deletes that dir.
/// Cached extractions (the normal path) keep nothing: their directory is
/// persistent by design.
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
    /// is always a real directory on disk.
    ///
    /// Asar packages are extracted into a persistent per-version cache
    /// under `<cache>/pulpit/ext-extract/` keyed by the archive's
    /// size+mtime signature, so the 6-7k-file extraction a big package
    /// needs happens once per source version instead of once per app
    /// session (a cold first `run-command` press paid ~9 s for this on a
    /// 12 MB package; the press itself only needs ~150 ms of JS work).
    ///
    /// Security: the original app extracted to one shared predictable
    /// `%TEMP%\pulpit-extensions\<pkg>` path, letting any local process
    /// pre-plant package code before it ran. The cache instead lives in
    /// the user's own profile (`dirs::cache_dir()`), which only the user
    /// (or an admin) can write - the same powers needed to replace the
    /// `.asar` in `~/deckboard/extensions` itself, so the trust boundary
    /// does not move (extensions are trusted user-installed code, ADR-012).
    /// A crash can never leave a half-populated cache entry visible: the
    /// extraction is staged under a dot-prefixed temp name, stamped, and
    /// renamed into place in one step; entries without a matching stamp
    /// are never used.
    ///
    /// When the cache is unusable (unwritable location, cross-device
    /// rename, ...) the old behavior applies: extraction into a private,
    /// randomly named temp dir that is deleted when the guard drops.
    pub fn open(path: &Path, package: String) -> Result<PackageSource, SourceError> {
        Self::open_with_cache_root(path, package, default_cache_root().as_deref())
    }

    /// [`open`][Self::open] with an explicit extraction-cache root.
    /// `None` disables caching (private temp-dir extraction only); tests
    /// inject a temp dir here instead of touching the real cache.
    pub fn open_with_cache_root(
        path: &Path,
        package: String,
        cache_root: Option<&Path>,
    ) -> Result<PackageSource, SourceError> {
        if path.is_dir() {
            return Ok(PackageSource {
                package,
                root: path.to_path_buf(),
                keep: SourceKeep::nothing(),
            });
        }
        let signature = extraction_signature(path);
        if let Some((root, sig)) = cache_root.zip(signature) {
            if let Some(dir) = cached_extraction(root, &package, sig) {
                tracing::debug!(package = %package, dir = %dir.display(), "asar extraction reused from cache");
                return Ok(PackageSource {
                    package,
                    root: dir,
                    keep: SourceKeep::nothing(),
                });
            }
            let arch = AsarArchive::open(path)?;
            if let Ok(dir) = extract_into_cache(&arch, root, &package, sig) {
                tracing::info!(package = %package, dir = %dir.display(), "asar extracted into persistent cache");
                return Ok(PackageSource {
                    package,
                    root: dir,
                    keep: SourceKeep::nothing(),
                });
            }
            // fall through to the private temp-dir extraction
            let tmp = tempfile::Builder::new().prefix(EXT_TEMP_PREFIX).tempdir()?;
            extract_asar(&arch, tmp.path())?;
            return Ok(PackageSource {
                package,
                root: tmp.path().to_path_buf(),
                keep: SourceKeep {
                    _asar: Some(arch),
                    _tmp: Some(tmp),
                },
            });
        }
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

    /// Split into the extraction root and the guard that must outlive
    /// every reader of that root. Directory packages own nothing; cached
    /// extractions own nothing; fallback temp-dir extractions own their
    /// temp dir (dropping the guard deletes it).
    pub fn into_root(self) -> (PathBuf, SourceKeep) {
        (self.root, self.keep)
    }
}

/// Temp extraction prefix for the uncached fallback; the directory itself
/// is randomly named by `tempfile` (see [`PackageSource::open`]).
const EXT_TEMP_PREFIX: &str = "pulpit-ext-";

/// Marker file inside a cached extraction. Written into the staging
/// directory *before* it is renamed into its final name, so a visible
/// entry always carries a complete extraction; the content is the
/// signature hex, so a mismatch (wrong version, corruption) invalidates.
const STAMP_FILE: &str = ".pulpit-extract-stamp";

/// Cache root shared with the extension metadata cache:
/// `<dirs::cache_dir()>/pulpit/ext-extract`.
fn default_cache_root() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("pulpit").join("ext-extract"))
}

/// Extraction cache key: a u64 mixing the archive's size and mtime. Two
/// versions of a package collide only on a 64-bit hash collision of
/// (size, mtime); a collision serves one stale package version, the same
/// failure a broken mtime-based invalidation would produce.
fn extraction_signature(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    Some(splitmix(
        meta.len().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ mtime,
    ))
}

/// splitmix64 finalizer: avalanche the mixed inputs over the whole u64.
fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Package names come from file names in the extensions directory; make
/// them a safe single path component so they cannot escape the cache root.
fn safe_dir_name(package: &str) -> String {
    let cleaned: String = package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches(['.', '-']).is_empty() {
        "pkg".to_string()
    } else {
        cleaned
    }
}

fn cache_dir_for(root: &Path, package: &str, sig: u64) -> PathBuf {
    root.join(format!("{}-{:016x}", safe_dir_name(package), sig))
}

/// A cache entry is usable only when its stamp matches the signature.
fn cached_extraction(root: &Path, package: &str, sig: u64) -> Option<PathBuf> {
    let dir = cache_dir_for(root, package, sig);
    let expected = format!("{sig:016x}");
    let stamp = fs::read_to_string(dir.join(STAMP_FILE)).ok()?;
    (stamp.trim() == expected).then_some(dir)
}

/// Extract `arch` into the persistent cache under `root`, keyed by
/// `package` + `sig`. Best-effort: any error propagates so the caller can
/// fall back to a private temp dir.
fn extract_into_cache(
    arch: &AsarArchive,
    root: &Path,
    package: &str,
    sig: u64,
) -> Result<PathBuf, SourceError> {
    fs::create_dir_all(root)?;
    let final_dir = cache_dir_for(root, package, sig);
    cleanup_stale_entries(root, package, sig);
    // stage inside the cache root (same volume => atomic rename) under a
    // tempfile-unique name, so concurrent extractions never collide
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{}-", safe_dir_name(package)))
        .tempdir_in(root)?;
    let staging_path = staging.path().to_path_buf();
    extract_asar(arch, &staging_path)?;
    fs::write(staging_path.join(STAMP_FILE), format!("{sig:016x}"))?;
    // hand the tree out of TempDir's auto-delete before renaming it away
    let staging_path = staging.keep();
    match fs::rename(&staging_path, &final_dir) {
        Ok(()) => Ok(final_dir),
        Err(_) => {
            // lost a race with a concurrent extractor, or leftovers: if
            // the winner's entry validates, reuse it
            let _ = fs::remove_dir_all(&staging_path);
            cached_extraction(root, package, sig).ok_or_else(|| {
                SourceError::Io(std::io::Error::other("cache promotion lost the race"))
            })
        }
    }
}

/// Remove complete cache entries of the same package with a different
/// signature (older/newer versions), so the cache holds only the current
/// version per package. Entries without a valid stamp are never touched:
/// they are either in-progress stagings of another process or foreign.
fn cleanup_stale_entries(root: &Path, package: &str, sig: u64) {
    let prefix = format!("{}-", safe_dir_name(package));
    let Ok(list) = fs::read_dir(root) else {
        return;
    };
    for entry in list.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || name.len() != prefix.len() + 16 {
            continue; // staging dirs are dot-prefixed, foreign names skipped
        }
        let entry_sig = &name[prefix.len()..];
        if entry_sig.eq_ignore_ascii_case(&format!("{sig:016x}")) {
            continue;
        }
        let stamp = fs::read_to_string(entry.path().join(STAMP_FILE)).unwrap_or_default();
        // only complete (stamped) entries are provably stale
        if stamp.trim().eq_ignore_ascii_case(entry_sig) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
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

    /// Asar with two entries of different content than `build_test_asar`,
    /// so the size signature differs and forces a fresh extraction.
    fn build_test_asar_v2() -> Vec<u8> {
        let json = r#"{"files":{"index.js":{"size":7,"offset":"0"}}}"#;
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
        out.extend_from_slice(b"version");
        out
    }

    fn distinct_mtime(path: &std::path::Path) {
        // bump mtime by a second so even identical sizes re-signature
        let t = fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(1);
        let f = fs::File::options().write(true).open(path).unwrap();
        let _ = f.set_modified(t);
    }

    #[test]
    fn asar_extraction_is_cached_and_reused_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let asar_path = dir.path().join("pkg.asar");
        std::fs::write(&asar_path, build_test_asar()).unwrap();

        let source =
            PackageSource::open_with_cache_root(&asar_path, "pkg".into(), Some(cache.path()))
                .unwrap();
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
        let root = source.root.clone();
        drop(source);

        // a marker written into the cached tree survives a re-open: the
        // second open reuses the directory instead of re-extracting
        std::fs::write(root.join("marker.txt"), "kept").unwrap();
        let second =
            PackageSource::open_with_cache_root(&asar_path, "pkg".into(), Some(cache.path()))
                .unwrap();
        assert_eq!(second.root, root, "cache hit must reuse the same root");
        assert_eq!(
            std::fs::read_to_string(second.root.join("marker.txt")).unwrap(),
            "kept",
            "a fresh extraction would not contain the marker"
        );
    }

    #[test]
    fn signature_change_extracts_fresh_and_cleans_the_stale_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let asar_path = dir.path().join("pkg.asar");
        std::fs::write(&asar_path, build_test_asar()).unwrap();

        let first =
            PackageSource::open_with_cache_root(&asar_path, "pkg".into(), Some(cache.path()))
                .unwrap();
        let old_root = first.root.clone();
        drop(first);

        std::fs::write(&asar_path, build_test_asar_v2()).unwrap();
        distinct_mtime(&asar_path);

        let second =
            PackageSource::open_with_cache_root(&asar_path, "pkg".into(), Some(cache.path()))
                .unwrap();
        assert_ne!(second.root, old_root, "new version needs a new entry");
        assert_eq!(
            std::fs::read_to_string(second.root.join("index.js")).unwrap(),
            "version"
        );
        assert!(
            !old_root.exists(),
            "the stale version's entry must be cleaned up"
        );
    }

    #[test]
    fn unusable_cache_falls_back_to_a_private_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        // a *file* where the cache root should be: create_dir_all fails
        let cache_target = dir.path().join("not-a-dir");
        std::fs::write(&cache_target, b"x").unwrap();
        let asar_path = dir.path().join("pkg.asar");
        std::fs::write(&asar_path, build_test_asar()).unwrap();

        let source = PackageSource::open_with_cache_root(
            &asar_path,
            "pkg".into(),
            Some(dir.path().join("not-a-dir").join("ext-extract").as_path()),
        )
        .unwrap();
        assert!(
            !source.root.starts_with(&cache_target),
            "fallback must not write through the unusable cache root"
        );
        assert_eq!(
            std::fs::read_to_string(source.root.join("index.js")).unwrap(),
            "hello world"
        );

        // the fallback extraction is private and disposable
        let (root, keep) = source.into_root();
        drop(keep);
        assert!(!root.exists(), "dropping the guard must clean up");
    }

    #[test]
    fn hostile_package_names_cannot_escape_the_cache_root() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let asar_path = dir.path().join("pkg.asar");
        std::fs::write(&asar_path, build_test_asar()).unwrap();

        let source = PackageSource::open_with_cache_root(
            &asar_path,
            "..\\..\\evil".into(),
            Some(cache.path()),
        )
        .unwrap();
        assert!(
            source.root.starts_with(cache.path()),
            "sanitized name must stay inside the cache root, got {}",
            source.root.display()
        );
    }

    #[test]
    fn directory_packages_keep_their_real_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/index.js"), "x").unwrap();

        let (root, _keep) = PackageSource::open_with_cache_root(
            dir.path().join("pkg").as_path(),
            "pkg".to_string(),
            None,
        )
        .unwrap()
        .into_root();
        assert_eq!(root, dir.path().join("pkg"));
    }
}
