//! The host is the only native precompile producer and consumer.
//!
//! A fixed header and native payload share one atomic publication. Digests bind
//! source and payload, not author authenticity: native artifacts must still be
//! trusted. Wasmtime independently rejects incompatible engine/target versions.

use super::*;
use molt_artifact_publish::AtomicFilePublication;
use molt_wasm_host::{sha256_digest_hex, sha256_hex_parts};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

const MAGIC: &[u8; 8] = b"MOLTAOT\x01";
const HEADER_SIZE: usize = 88;
#[derive(Clone, Copy)]
pub(super) enum ModuleRole {
    Main,
    Runtime,
}

impl ModuleRole {
    fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Runtime => "runtime",
        }
    }
    fn override_env(self) -> &'static str {
        match self {
            Self::Main => "MOLT_WASM_PRECOMPILED_PATH",
            Self::Runtime => "MOLT_WASM_PRECOMPILED_RUNTIME_PATH",
        }
    }
}

fn header(source: &ModuleSource, payload: &[u8]) -> [u8; HEADER_SIZE] {
    let mut bytes = [0; HEADER_SIZE];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..16].copy_from_slice(&(source.bytes().len() as u64).to_le_bytes());
    bytes[16..48].copy_from_slice(source.sha256());
    bytes[48..56].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes[56..88].copy_from_slice(&Sha256::digest(payload));
    bytes
}

fn validated_payload<'a>(container: &'a [u8], source: &ModuleSource) -> Result<&'a [u8]> {
    if container.len() < HEADER_SIZE || &container[..8] != MAGIC {
        bail!(
            "unsupported Molt native precompile container; regenerate with molt-wasm-host --precompile"
        );
    }
    let source_size = u64::from_le_bytes(container[8..16].try_into().unwrap());
    if source_size != source.bytes().len() as u64 {
        bail!("precompiled source size mismatch");
    }
    if container[16..48] != source.sha256()[..] {
        bail!("precompiled source SHA-256 mismatch");
    }
    let payload_size = u64::from_le_bytes(container[48..56].try_into().unwrap());
    let payload = &container[HEADER_SIZE..];
    if payload_size == 0 || payload_size != payload.len() as u64 {
        bail!("precompiled payload size mismatch");
    }
    if container[56..88] != Sha256::digest(payload)[..] {
        bail!("precompiled payload SHA-256 mismatch");
    }
    Ok(payload)
}

fn destination(source: &ModuleSource, role: ModuleRole) -> Result<(PathBuf, bool)> {
    let override_env = role.override_env();
    match env::var_os(override_env) {
        Some(value) if value.is_empty() => bail!("{override_env} must not be empty"),
        Some(value) => Ok((
            std::path::absolute(value).context("resolve precompiled destination")?,
            true,
        )),
        None => Ok((source.path().with_extension("molt.cwasm"), false)),
    }
}

pub(super) fn load_or_compile_module(
    engine: &Engine,
    source: &ModuleSource,
    role: ModuleRole,
) -> Result<Module> {
    let label = role.label();
    let enabled = match env::var_os("MOLT_WASM_PRECOMPILED") {
        None => false,
        Some(value) if value == "0" => false,
        Some(value) if value == "1" => true,
        Some(_) => bail!("MOLT_WASM_PRECOMPILED must be 0 or 1"),
    };
    if enabled {
        let (path, explicit) = destination(source, role)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect {label} precompiled {}", path.display()));
            }
            Ok(_) => {
                let started = Instant::now();
                let artifact = ModuleSource::read(path, "precompiled", None)?;
                let payload = validated_payload(artifact.bytes(), source).with_context(|| {
                    format!("validate {label} precompiled {}", artifact.path().display())
                })?;
                // This is native executable code from the explicitly enabled,
                // trusted artifact store. Digests detect corruption and stale
                // source, but do not authorize attacker-provided native code.
                log::debug!(
                    "precompiled {label} admission bytes={} elapsed={:?}",
                    artifact.bytes().len(),
                    started.elapsed()
                );
                let started = Instant::now();
                let module =
                    unsafe { Module::deserialize(engine, payload) }.with_context(|| {
                        format!(
                            "deserialize {label} precompiled {}",
                            artifact.path().display()
                        )
                    })?;
                log::debug!(
                    "precompiled {label} deserialize elapsed={:?}",
                    started.elapsed()
                );
                return Ok(module);
            }
        }
    }
    let started = Instant::now();
    let module = Module::new(engine, source.bytes())
        .with_context(|| format!("compile {label} {}", source.path().display()))?;
    log::debug!("compiled {label} module in {:?}", started.elapsed());
    Ok(module)
}

#[derive(Serialize)]
pub(super) struct PrecompileReceipt {
    version: u32,
    kind: &'static str,
    artifacts: BTreeMap<&'static str, ArtifactReceipt>,
}

#[derive(Serialize)]
struct ArtifactReceipt {
    source: PathBuf,
    path: PathBuf,
    source_sha256: String,
    sha256: String,
    size: u64,
}

struct PreparedArtifact {
    publication: AtomicFilePublication,
    receipt: ArtifactReceipt,
}

impl PreparedArtifact {
    fn prepare(engine: &Engine, source: &ModuleSource, path: PathBuf) -> Result<Self> {
        // Compile directly to serialized bytes: loading a Module here would
        // register executable mappings and then copy them solely to discard it.
        // The engine API accepts the same binary/WAT source without executing
        // imports, entrypoints or core starts, or populating a duplicate JIT cache.
        let started = Instant::now();
        log::debug!(
            "precompile codegen source_bytes={} path={}",
            source.bytes().len(),
            source.path().display()
        );
        let payload = engine
            .precompile_module(source.bytes())
            .with_context(|| format!("precompile {}", source.path().display()))?;
        log::debug!(
            "precompile codegen payload_bytes={} elapsed={:?}",
            payload.len(),
            started.elapsed()
        );
        let started = Instant::now();
        let header = header(source, &payload);
        let receipt = ArtifactReceipt {
            source: source.path().to_path_buf(),
            path: path.clone(),
            source_sha256: sha256_digest_hex(source.sha256()),
            sha256: sha256_hex_parts([header.as_slice(), payload.as_slice()]),
            size: (HEADER_SIZE as u64)
                .checked_add(payload.len() as u64)
                .context("native container size overflow")?,
        };
        log::debug!("precompile identity elapsed={:?}", started.elapsed());
        let started = Instant::now();
        let mut publication = AtomicFilePublication::new(&path)
            .with_context(|| format!("prepare native container {}", path.display()))?;
        let result = publication
            .writer()
            .write_all(&header)
            .and_then(|()| publication.writer().write_all(&payload));
        if let Err(error) = result {
            return Err(publication.abort(error).into());
        }
        log::debug!("precompile staging elapsed={:?}", started.elapsed());
        // The payload is now buffered in the private file, not held alongside
        // every split member's serialized bytes or duplicated into a container.
        Ok(Self {
            publication,
            receipt,
        })
    }
}

struct PathIdentity {
    normalized: PathBuf,
    leaf: Option<same_file::Handle>,
    parent: same_file::Handle,
    folded_name: String,
}

impl PathIdentity {
    fn read(path: &Path) -> Result<Self> {
        let normalized = normalized_destination(path)?;
        let leaf = match same_file::Handle::from_path(path) {
            Ok(handle) => Some(handle),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("identify {}", path.display()));
            }
        };
        let parent = same_file::Handle::from_path(normalized.parent().unwrap())
            .context("identify precompiled parent directory")?;
        let folded_name = normalized
            .file_name()
            .unwrap()
            .to_str()
            .context("precompile receipt requires a UTF-8 file name")?
            .nfd()
            .case_fold()
            .collect();
        Ok(Self {
            normalized,
            leaf,
            parent,
            folded_name,
        })
    }

    fn collides(&self, other: &Self) -> bool {
        self.normalized == other.normalized
            || matches!((&self.leaf, &other.leaf), (Some(left), Some(right)) if left == right)
            // Existing aliases are decided by the actual filesystem, including
            // symlinks, short names and volume-specific case behavior. For two
            // absent outputs no leaf identity exists yet; require portable,
            // case-distinct names within the same physical parent directory.
            || (self.leaf.is_none() && other.leaf.is_none()
                && self.parent == other.parent && self.folded_name == other.folded_name)
    }
}

fn normalized_destination(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .context("precompiled destination has no file name")?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical_parent = fs::canonicalize(parent).with_context(|| {
        format!(
            "precompiled destination parent must exist: {}",
            parent.display()
        )
    })?;
    let normalized = canonical_parent.join(filename);
    #[cfg(windows)]
    {
        // Reject stream selectors and trailing-dot/space aliases. Actual leaf
        // identity, not lexical lowercase, owns filesystem alias comparisons.
        let name = filename
            .to_str()
            .context("precompiled destination name is not UTF-8")?;
        if name.contains(':') || name.ends_with(['.', ' ']) {
            bail!("precompiled destination has an ambiguous Windows file name");
        }
        Ok(normalized)
    }
    #[cfg(not(windows))]
    Ok(normalized)
}

fn validate_destinations(
    sources: &[&ModuleSource],
    destinations: &[PathBuf],
    manifest: Option<&Path>,
) -> Result<()> {
    let mut protected = sources
        .iter()
        .map(|source| PathIdentity::read(source.path()))
        .collect::<Result<Vec<_>>>()?;
    if let Some(manifest) = manifest {
        protected.push(PathIdentity::read(manifest)?);
    }
    let mut selected = Vec::with_capacity(destinations.len());
    for path in destinations {
        // Never replace a symlink, directory, device or other non-regular leaf.
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => bail!(
                "precompiled destination is not a regular file: {}",
                path.display()
            ),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect precompiled destination"),
        }
        let identity = PathIdentity::read(path)?;
        if protected
            .iter()
            .chain(selected.iter())
            .any(|other| identity.collides(other))
        {
            bail!(
                "precompiled destination collides with a source, manifest, or another output: {}",
                path.display()
            );
        }
        selected.push(identity);
    }
    Ok(())
}

pub(super) fn precompile_execution(
    engine: &Engine,
    execution: &ResolvedExecution,
) -> Result<PrecompileReceipt> {
    let (main, runtime, manifest) = match execution {
        ResolvedExecution::MoltApplication(modules) => (
            &modules.main,
            modules.runtime.as_ref(),
            Some(modules.manifest_path.as_path()),
        ),
        ResolvedExecution::WasiCommand { module } => (module, None, None),
    };
    let mut members = vec![(
        ModuleRole::Main,
        main,
        destination(main, ModuleRole::Main)?.0,
    )];
    if let Some(runtime) = runtime {
        members.push((
            ModuleRole::Runtime,
            runtime,
            destination(runtime, ModuleRole::Runtime)?.0,
        ));
    }
    validate_destinations(
        &members
            .iter()
            .map(|(_, source, _)| *source)
            .collect::<Vec<_>>(),
        &members
            .iter()
            .map(|(_, _, path)| path.clone())
            .collect::<Vec<_>>(),
        manifest,
    )?;
    let mut prepared = Vec::with_capacity(members.len());
    for (label, source, path) in members {
        prepared.push((label, PreparedArtifact::prepare(engine, source, path)?));
    }
    // All members must compile and stage before any public path changes. Each
    // container is independently source-bound and crash-atomic. Cross-file
    // publication is not a transaction: on failure, report earlier commits and
    // never pretend a directory-sync error left its destination unchanged.
    let mut artifacts = BTreeMap::new();
    for (label, prepared) in prepared {
        let label = label.label();
        let started = Instant::now();
        prepared.publication.commit().with_context(|| {
            let committed = artifacts
                .values()
                .map(|receipt: &ArtifactReceipt| receipt.path.display().to_string())
                .collect::<Vec<_>>();
            format!(
                "publish {label} native container; previously committed containers: {committed:?}"
            )
        })?;
        log::debug!(
            "precompile {label} publication elapsed={:?}",
            started.elapsed()
        );
        artifacts.insert(label, prepared.receipt);
    }
    Ok(PrecompileReceipt {
        version: 1,
        kind: "molt-wasm-precompile",
        artifacts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "molt-native-container-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn source(&self, name: &str, bytes: &[u8]) -> ModuleSource {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            ModuleSource::read(path, "fixture", None).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove owned native-container fixture");
        }
    }

    #[test]
    fn container_rejects_corruption_truncation_trailing_bytes_and_stale_source() {
        let fixture = Fixture::new();
        let source = fixture.source("source.wasm", b"source");
        let other = fixture.source("other.wasm", b"other!");
        let longer = fixture.source("longer.wasm", b"longer source");
        let payload = b"native";
        let mut bytes = header(&source, payload).to_vec();
        bytes.extend_from_slice(payload);
        assert_eq!(validated_payload(&bytes, &source).unwrap(), payload);
        for index in [0, 7, 8, 16, 48, 56, HEADER_SIZE] {
            let mut corrupt = bytes.clone();
            corrupt[index] ^= 1;
            assert!(
                validated_payload(&corrupt, &source).is_err(),
                "byte {index}"
            );
        }
        for len in 0..bytes.len() {
            assert!(
                validated_payload(&bytes[..len], &source).is_err(),
                "length {len}"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(validated_payload(&trailing, &source).is_err());
        assert!(
            validated_payload(&bytes, &other)
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
        assert!(validated_payload(&bytes, &longer).is_err());
        assert!(validated_payload(b"raw wasmtime bytes", &source).is_err());
        assert!(validated_payload(&header(&source, b""), &source).is_err());
    }

    #[test]
    fn staged_container_uses_admitted_source_and_does_not_run_core_start() {
        let fixture = Fixture::new();
        let source = fixture.source(
            "trap.wat",
            b"(module (func $start unreachable) (start $start))",
        );
        let engine = build_engine().unwrap();
        let path = fixture.0.join("trap.molt.cwasm");
        let prepared = PreparedArtifact::prepare(&engine, &source, path.clone()).unwrap();
        assert!(!path.exists(), "preparation must not publish");
        fs::write(source.path(), b"replacement source").unwrap();
        prepared.publication.commit().unwrap();
        let container = fs::read(&path).unwrap();
        assert_eq!(sha256_hex(&container), prepared.receipt.sha256);
        assert_eq!(container.len() as u64, prepared.receipt.size);
        let payload = validated_payload(&container, &source).unwrap();
        let replacement =
            ModuleSource::read(source.path().to_path_buf(), "replacement", None).unwrap();
        assert!(validated_payload(&container, &replacement).is_err());
        // Trusted bytes generated immediately above, never arbitrary input.
        let module = unsafe { Module::deserialize(&engine, payload) }.unwrap();
        let mut store = Store::new(&engine, ());
        assert!(
            Instance::new(&mut store, &module, &[]).is_err(),
            "core start was not executed by producer"
        );
        assert_eq!(
            fs::read_dir(&fixture.0).unwrap().count(),
            2,
            "no sidecars or temporaries"
        );
    }

    #[test]
    fn split_preparation_failure_preserves_both_existing_containers() {
        let fixture = Fixture::new();
        let main = fixture.source("app.wat", b"(module)");
        let runtime = fixture.source("runtime.wat", b"invalid module");
        for name in ["app.molt.cwasm", "runtime.molt.cwasm"] {
            fs::write(fixture.0.join(name), b"retained generation").unwrap();
        }
        let execution = ResolvedExecution::MoltApplication(ResolvedExecutionModules {
            manifest_path: fixture.0.join("manifest.json"),
            main,
            runtime: Some(runtime),
            linked: false,
        });
        let error = precompile_execution(&build_engine().unwrap(), &execution)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("runtime.wat"));
        for name in ["app.molt.cwasm", "runtime.molt.cwasm"] {
            assert_eq!(
                fs::read(fixture.0.join(name)).unwrap(),
                b"retained generation"
            );
        }
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 4);
    }

    #[test]
    fn split_producer_publishes_both_members_and_receipt_matches_exact_bytes() {
        let fixture = Fixture::new();
        let main = fixture.source(
            "app.wat",
            b"(module (func (export \"answer\") (result i32) i32.const 42))",
        );
        let runtime = fixture.source("runtime.wat", b"(module)");
        let execution = ResolvedExecution::MoltApplication(ResolvedExecutionModules {
            manifest_path: fixture.0.join("manifest.json"),
            main,
            runtime: Some(runtime),
            linked: false,
        });
        let engine = build_engine().unwrap();
        let receipt = precompile_execution(&engine, &execution).unwrap();
        assert_eq!(receipt.artifacts.len(), 2);
        for artifact in receipt.artifacts.values() {
            let bytes = fs::read(&artifact.path).unwrap();
            let source =
                ModuleSource::read(artifact.source.clone(), "published source", None).unwrap();
            assert_eq!(artifact.sha256, sha256_hex(&bytes));
            assert_eq!(artifact.source_sha256, sha256_hex(source.bytes()));
            assert_eq!(artifact.size, bytes.len() as u64);
            let payload = validated_payload(&bytes, &source).unwrap();
            // Trusted bytes from this operation.
            unsafe { Module::deserialize(&engine, payload) }.unwrap();
        }
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 4);
    }

    #[test]
    fn output_collisions_and_nonregular_destinations_fail_before_publication() {
        let fixture = Fixture::new();
        let source = fixture.source("source.wasm", b"(module)");
        let manifest = fixture.0.join("manifest.json");
        let output = fixture.0.join("output.molt.cwasm");
        for paths in [
            vec![source.path().to_path_buf()],
            vec![manifest.clone()],
            vec![output.clone(), output.clone()],
            vec![fixture.0.clone()],
        ] {
            assert!(validate_destinations(&[&source], &paths, Some(&manifest)).is_err());
        }
        assert!(validate_destinations(&[&source], &[output], Some(&manifest)).is_ok());
        for names in [
            ["New.molt.cwasm", "new.molt.cwasm"],
            ["é.molt.cwasm", "e\u{301}.molt.cwasm"],
        ] {
            assert!(
                validate_destinations(
                    &[&source],
                    &names.map(|name| fixture.0.join(name)),
                    Some(&manifest)
                )
                .is_err()
            );
        }
        let alias = fixture.0.join("hardlink-source.wasm");
        fs::hard_link(source.path(), &alias).unwrap();
        assert!(validate_destinations(&[&source], &[alias], Some(&manifest)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn admitted_source_and_manifest_symlink_targets_are_protected() {
        let fixture = Fixture::new();
        let source = fixture.source("actual.wasm", b"(module)");
        let link = fixture.0.join("source.wasm");
        std::os::unix::fs::symlink(source.path(), &link).unwrap();
        let admitted = ModuleSource::read(link, "symlink source", None).unwrap();
        assert!(validate_destinations(&[&admitted], &[source.path().to_path_buf()], None).is_err());
        let manifest_target = fixture.0.join("actual.json");
        let manifest_link = fixture.0.join("manifest.json");
        fs::write(&manifest_target, b"{}").unwrap();
        std::os::unix::fs::symlink(&manifest_target, &manifest_link).unwrap();
        assert!(
            validate_destinations(&[&source], &[manifest_target], Some(&manifest_link)).is_err()
        );
    }
}
