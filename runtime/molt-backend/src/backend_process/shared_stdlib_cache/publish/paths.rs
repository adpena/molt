use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use molt_ir::backend_environment::{self as backend_env, BackendEnv};

pub(crate) fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("count")
}

pub(super) fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("key")
}

pub(super) fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("manifest.json")
}

pub(crate) fn stdlib_cache_partition_manifest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("partition.json")
}

pub(super) fn stdlib_cache_origin_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("origin.json")
}

pub(super) fn stdlib_cache_archive_digest_sidecar_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_extension("sha256")
}

/// Python derives these projections from this archive generation. They share
/// its publication lock and are invalidated on replacement or generation abort.
pub(super) fn stdlib_cache_derived_sidecar_paths(stdlib_path: &Path) -> [PathBuf; 2] {
    [
        stdlib_path.with_extension("symbol-contract.json"),
        stdlib_path.with_extension("symbols.json"),
    ]
}

fn exact_cache_key(path: &Path) -> io::Result<&str> {
    let key = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cache entry key must be a lowercase SHA-256",
        ));
    }
    Ok(key)
}

/// Cross-language path contract owned on the CLI side by molt.default_paths.
/// Explicit standalone archives keep their selected directory; only the
/// canonical shared archive layout projects to the shared cache namespace.
pub(crate) fn canonical_stdlib_cache_root(archive: &Path) -> io::Result<Option<PathBuf>> {
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    if archive.file_name() == Some(OsStr::new("stdlib.a"))
        && parent.parent().and_then(Path::file_name) == Some(OsStr::new(SHARED_STDLIB_DIRECTORY))
    {
        exact_cache_key(parent)?;
        return Ok(Some(
            parent
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
        ));
    }
    Ok(None)
}

pub(super) fn stdlib_archive_cache_root(archive: &Path) -> io::Result<PathBuf> {
    Ok(canonical_stdlib_cache_root(archive)?.unwrap_or_else(|| {
        archive
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }))
}

/// Resolve existing ancestors (including symlinks) without creating an output.
/// Missing suffix components remain in the resolved parent's namespace.
pub(crate) fn resolved_archive_path(path: &Path) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => {
                resolved.push(other.as_os_str());
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = std::fs::canonicalize(&resolved)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(resolved)
}

pub(crate) fn validate_required_stdlib_output(
    output: &Path,
    source: Option<&Path>,
) -> io::Result<()> {
    let output = resolved_archive_path(output)?;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "required build-owned stdlib output must be outside cache entries or inside canonical cache/home, and distinct from its source",
        )
    };
    let adjacent = output.with_file_name(format!(
        "{}.publish.lock",
        output.file_name().unwrap_or_default().to_string_lossy()
    ));
    if stdlib_cache_publish_lock_path(&output)? != adjacent {
        return Err(invalid());
    }
    let source = source.map(resolved_archive_path).transpose()?;
    if source.as_ref() == Some(&output) {
        return Err(invalid());
    }
    let cache_root = source
        .as_deref()
        .map(canonical_stdlib_cache_root)
        .transpose()?
        .flatten()
        .or_else(|| {
            // MOLT_CACHE roots every Molt cache: process configuration (the
            // `cache` catalog group), never request data.
            backend_env::var_os(BackendEnv::MOLT_CACHE)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        });
    if let Some(root) = cache_root {
        let root = resolved_archive_path(&root)?;
        // Canonical home is durable across maintenance environments; a selected
        // MOLT_HOME/MOLT_BIN exception alone is not future output custody.
        let home = resolved_archive_path(&root.join("home"))?;
        if output.starts_with(&root) && !output.starts_with(&home) {
            return Err(invalid());
        }
    }
    Ok(())
}

/// The producer stripe for a canonical shared stdlib cache entry
/// (`<root>/shared-stdlib/<key>/stdlib.a`). Builds that need one entry take it
/// before they compile, so one compiles and the rest hydrate its archive.
/// Another path has no cross-build identity and no producer. Like the
/// publication lock, it stripes by the key's first byte: at most 256 lock
/// files, and concurrent misses of two different entries share a stripe one
/// time in 256.
pub(super) fn stdlib_cache_producer_lock_path(path: &Path) -> io::Result<Option<PathBuf>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let namespace = parent.parent();
    let shared = path.file_name() == Some(OsStr::new("stdlib.a"))
        && namespace.and_then(Path::file_name) == Some(OsStr::new(SHARED_STDLIB_DIRECTORY));
    if !shared {
        return Ok(None);
    }
    let key = exact_cache_key(parent)?;
    let cache_root = namespace
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."));
    Ok(Some(cache_root.join("locks").join(format!(
        "shared-stdlib-producer.{}.lock",
        &key[..2]
    ))))
}

/// Cross-language namespace contract owned on the CLI side by
/// molt.default_paths (`*_CACHE_DIRECTORY`): program-keyed archives, archives
/// keyed by their ordered constituents, and the constituents themselves.
pub(super) const SHARED_STDLIB_DIRECTORY: &str = "shared-stdlib";
pub(super) const NATIVE_STDLIB_ARCHIVE_DIRECTORY: &str = "native-stdlib-archives-v1";
pub(super) const NATIVE_STDLIB_OBJECT_DIRECTORY: &str = "native-stdlib-objects-v1";

/// A retained cache entry family. Each owns one namespace directory, one leaf
/// name and one stripe of persistent lock files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheEntryFamily {
    /// Archives keyed by the program (the CLI's lookup key).
    SharedStdlib,
    /// Archives keyed by their ordered constituent inputs (content).
    NativeStdlibArchive,
    /// Constituent object envelopes keyed by their complete job input.
    NativeStdlibObject,
}

impl CacheEntryFamily {
    const ALL: [Self; 3] = [
        Self::SharedStdlib,
        Self::NativeStdlibArchive,
        Self::NativeStdlibObject,
    ];

    fn namespace(self) -> &'static str {
        match self {
            Self::SharedStdlib => SHARED_STDLIB_DIRECTORY,
            Self::NativeStdlibArchive => NATIVE_STDLIB_ARCHIVE_DIRECTORY,
            Self::NativeStdlibObject => NATIVE_STDLIB_OBJECT_DIRECTORY,
        }
    }

    fn leaf(self) -> &'static str {
        match self {
            Self::SharedStdlib | Self::NativeStdlibArchive => "stdlib.a",
            Self::NativeStdlibObject => "object.entry",
        }
    }

    fn lock_stem(self) -> &'static str {
        match self {
            Self::SharedStdlib => "shared-stdlib",
            Self::NativeStdlibArchive => "native-stdlib-archive",
            Self::NativeStdlibObject => "native-stdlib-object",
        }
    }

    /// The family of a cache entry leaf, or `None` for any other path.
    fn of(path: &Path) -> Option<Self> {
        let leaf = path.file_name()?;
        let namespace = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)?;
        Self::ALL
            .into_iter()
            .find(|family| leaf == family.leaf() && namespace == family.namespace())
    }
}

/// Whether `path` names an archive inside a retained cache entry, whose
/// directory recency the Python eviction owner reads.
pub(super) fn is_stdlib_archive_cache_entry(path: &Path) -> bool {
    matches!(
        CacheEntryFamily::of(path),
        Some(CacheEntryFamily::SharedStdlib | CacheEntryFamily::NativeStdlibArchive)
    )
}

/// The content-addressed archive generation for `key` under `cache_root`.
pub(super) fn native_stdlib_archive_entry_path(cache_root: &Path, key: &str) -> PathBuf {
    let family = CacheEntryFamily::NativeStdlibArchive;
    cache_root
        .join(family.namespace())
        .join(key)
        .join(family.leaf())
}

pub(super) fn stdlib_cache_publish_lock_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let namespace = parent.parent();
    if let Some(family) = CacheEntryFamily::of(path) {
        let key = exact_cache_key(parent)?;
        let cache_root = namespace
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."));
        return Ok(cache_root.join("locks").join(format!(
            "{}.{}.lock",
            family.lock_stem(),
            &key[..2]
        )));
    }
    let mut name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("stdlib_shared"))
        .to_os_string();
    name.push(".publish.lock");
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_archive_refuses_every_leaf_inside_cache_entry() {
        let source = PathBuf::from("cache/shared-stdlib")
            .join("aa".repeat(32))
            .join("stdlib.a");
        for prefix in ["aa", "bb"] {
            for leaf in ["stdlib.a", "nested/owned.a", "nested/../owned.a"] {
                let output = PathBuf::from("cache/shared-stdlib")
                    .join(prefix.repeat(32))
                    .join(leaf);
                assert_eq!(
                    validate_required_stdlib_output(&output, Some(&source))
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::InvalidInput
                );
            }
        }
        validate_required_stdlib_output(Path::new("build/owned.a"), Some(&source)).unwrap();
        let root = Path::new("cache");
        validate_required_stdlib_output(&root.join("home/build/app/stdlib.a"), Some(&source))
            .unwrap();
        for relative in [
            "wasm_link/schema/key/owned.a",
            "module_lowering/entry.json",
            "unclassified/owned.a",
        ] {
            assert_eq!(
                validate_required_stdlib_output(&root.join(relative), Some(&source))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn archive_and_constituent_paths_share_durable_control_contract() {
        let cache = Path::new("cache");
        let key = "ab".repeat(32);
        let archive = cache.join("shared-stdlib").join(&key).join("stdlib.a");
        let object = cache
            .join("native-stdlib-objects-v1")
            .join(&key)
            .join("object.entry");
        assert_eq!(stdlib_archive_cache_root(&archive).unwrap(), cache);
        assert_eq!(
            stdlib_cache_publish_lock_path(&archive).unwrap(),
            cache.join("locks/shared-stdlib.ab.lock")
        );
        assert_eq!(
            stdlib_cache_publish_lock_path(&object).unwrap(),
            cache.join("locks/native-stdlib-object.ab.lock")
        );
        let content = native_stdlib_archive_entry_path(cache, &key);
        assert_eq!(
            content,
            cache
                .join("native-stdlib-archives-v1")
                .join(&key)
                .join("stdlib.a")
        );
        assert_eq!(
            stdlib_cache_publish_lock_path(&content).unwrap(),
            cache.join("locks/native-stdlib-archive.ab.lock")
        );
        assert!(is_stdlib_archive_cache_entry(&archive));
        assert!(is_stdlib_archive_cache_entry(&content));
        assert!(!is_stdlib_archive_cache_entry(&object));
        assert!(!is_stdlib_archive_cache_entry(Path::new("build/stdlib.a")));
        assert_eq!(
            stdlib_cache_publish_lock_path(Path::new("explicit/stdlib.a")).unwrap(),
            Path::new("explicit/stdlib.a.publish.lock")
        );
        assert!(
            stdlib_cache_publish_lock_path(Path::new("cache/shared-stdlib/invalid/stdlib.a"))
                .is_err()
        );
    }
}
