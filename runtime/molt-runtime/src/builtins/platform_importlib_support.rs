use super::*;

pub(super) struct SourceLoaderResolution {
    pub(super) is_package: bool,
    pub(super) module_package: String,
    pub(super) package_root: Option<String>,
}

pub(super) struct ImportlibSourceExecPayload {
    pub(super) source: Vec<u8>,
    pub(super) is_package: bool,
    pub(super) module_package: String,
    pub(super) package_root: Option<String>,
}

pub(super) struct ImportlibZipSourceExecPayload {
    pub(super) source: Vec<u8>,
    pub(super) is_package: bool,
    pub(super) module_package: String,
    pub(super) package_root: Option<String>,
    pub(super) origin: String,
}

pub(super) struct ImportlibPathResolution {
    pub(super) origin: Option<String>,
    pub(super) is_package: bool,
    pub(super) submodule_search_locations: Option<Vec<String>>,
    pub(super) cached: Option<String>,
    pub(super) has_location: bool,
    pub(super) loader_kind: String,
    pub(super) zip_archive: Option<String>,
    pub(super) zip_inner_path: Option<String>,
}

pub(super) struct ImportlibFindSpecPayload {
    pub(super) origin: Option<String>,
    pub(super) is_package: bool,
    pub(super) submodule_search_locations: Option<Vec<String>>,
    pub(super) cached: Option<String>,
    pub(super) is_builtin: bool,
    pub(super) has_location: bool,
    pub(super) loader_kind: String,
    pub(super) zip_archive: Option<String>,
    pub(super) zip_inner_path: Option<String>,
    pub(super) meta_path_count: i64,
    pub(super) path_hooks_count: i64,
}

pub(super) struct ImportlibParentSearchPathsPayload {
    pub(super) has_parent: bool,
    pub(super) parent_name: Option<String>,
    pub(super) search_paths: Vec<String>,
    pub(super) needs_parent_spec: bool,
    pub(super) package_context: bool,
}

pub(super) struct ImportlibRuntimeStateViewBits {
    pub(super) modules_bits: u64,
    pub(super) meta_path_bits: u64,
    pub(super) path_hooks_bits: u64,
    pub(super) path_importer_cache_bits: u64,
}

pub(super) struct ImportlibSpecFromFileLocationPayload {
    pub(super) path: String,
    pub(super) is_package: bool,
    pub(super) package_root: Option<String>,
}

pub(super) struct ImportlibBootstrapPayload {
    pub(super) resolved_search_paths: Vec<String>,
    pub(super) pythonpath_entries: Vec<String>,
    pub(super) module_roots_entries: Vec<String>,
    pub(super) venv_site_packages_entries: Vec<String>,
    pub(super) pwd: String,
    pub(super) include_cwd: bool,
    pub(super) stdlib_root: Option<String>,
}

pub(super) struct ImportlibMetadataPayload {
    pub(super) path: String,
    pub(super) name: String,
    pub(super) version: String,
    pub(super) metadata: Vec<(String, String)>,
    pub(super) entry_points: Vec<(String, String, String)>,
    pub(super) requires_dist: Vec<String>,
    pub(super) provides_extra: Vec<String>,
    pub(super) requires_python: Option<String>,
}

pub(super) struct ImportlibMetadataRecordEntry {
    pub(super) path: String,
    pub(super) hash: Option<String>,
    pub(super) size: Option<String>,
}

pub(super) fn bootstrap_path_sep() -> char {
    if sys_platform_str().starts_with("win") {
        '\\'
    } else {
        '/'
    }
}

pub(super) fn path_is_absolute_text(path: &str, sep: char) -> bool {
    if path.starts_with(sep) {
        return true;
    }
    if sep == '\\' {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        if path.starts_with("\\\\") {
            return true;
        }
    }
    false
}

pub(super) fn bootstrap_resolve_path_entry(path: &str, pwd: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path_is_absolute_text(path, sep) || pwd.is_empty() {
        return path_normpath_text(path, sep);
    }
    path_normpath_text(&path_join_text(pwd.to_string(), path, sep), sep)
}

pub(super) fn source_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let is_init = path_basename_text(path, sep) == "__init__.py";
    let is_package = spec_is_package || is_init;
    let module_package = if is_package {
        module_name.to_string()
    } else {
        module_name
            .rsplit_once('.')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_default()
    };
    let package_root = if is_package {
        Some(path_dirname_text(path, sep))
    } else {
        None
    };
    SourceLoaderResolution {
        is_package,
        module_package,
        package_root,
    }
}

pub(super) fn extension_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep).to_ascii_lowercase();
    let is_init = basename.starts_with("__init__.");
    let is_package = spec_is_package || is_init;
    let module_package = if is_package {
        module_name.to_string()
    } else {
        module_name
            .rsplit_once('.')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_default()
    };
    let package_root = if is_package {
        Some(path_dirname_text(path, sep))
    } else {
        None
    };
    SourceLoaderResolution {
        is_package,
        module_package,
        package_root,
    }
}

pub(super) fn sourceless_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep).to_ascii_lowercase();
    let dirname = path_dirname_text(path, sep);
    let dirname_base = path_basename_text(&dirname, sep).to_ascii_lowercase();
    let is_cache_init = dirname_base == "__pycache__"
        && basename.starts_with("__init__.")
        && basename.ends_with(".pyc");
    let is_init = basename == "__init__.pyc" || is_cache_init;
    let is_package = spec_is_package || is_init;
    let module_package = if is_package {
        module_name.to_string()
    } else {
        module_name
            .rsplit_once('.')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_default()
    };
    let package_root = if is_package {
        if is_cache_init {
            Some(path_dirname_text(&dirname, sep))
        } else {
            Some(path_dirname_text(path, sep))
        }
    } else {
        None
    };
    SourceLoaderResolution {
        is_package,
        module_package,
        package_root,
    }
}

pub(super) fn importlib_source_exec_payload(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> Result<ImportlibSourceExecPayload, std::io::Error> {
    let source_bytes = std::fs::read(path)?;
    let source =
        match crate::object::ops::decode_bytes_text("utf-8", "surrogateescape", &source_bytes) {
            Ok((text, _encoding)) => text,
            Err(_) => String::from_utf8_lossy(&source_bytes)
                .into_owned()
                .into_bytes(),
        };
    let resolution = source_loader_resolution(module_name, path, spec_is_package);
    Ok(ImportlibSourceExecPayload {
        source,
        is_package: resolution.is_package,
        module_package: resolution.module_package,
        package_root: resolution.package_root,
    })
}

pub(super) fn split_zip_archive_path(path: &str) -> Option<(String, String)> {
    const ARCHIVE_SUFFIXES: [&str; 3] = [".zip", ".whl", ".egg"];
    if path.is_empty() {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    let mut best_idx: Option<usize> = None;
    let mut best_suffix_len: usize = 0;
    for suffix in ARCHIVE_SUFFIXES {
        let Some(idx) = lower.rfind(suffix) else {
            continue;
        };
        let archive_end = idx + suffix.len();
        if archive_end < path.len() {
            let tail = path.as_bytes()[archive_end];
            if tail != b'/' && tail != b'\\' {
                continue;
            }
        }
        if best_idx.is_none_or(|current| idx > current) {
            best_idx = Some(idx);
            best_suffix_len = suffix.len();
        }
    }
    let idx = best_idx?;
    let archive_end = idx + best_suffix_len;
    let archive = path[..archive_end].to_string();
    let remainder = path[archive_end..]
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    Some((archive, remainder))
}

pub(super) fn zip_entry_join(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}/{rel}")
    }
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_open(
    path: &str,
) -> Result<zip::ZipArchive<std::fs::File>, std::io::Error> {
    let file = std::fs::File::open(path)?;
    zip::ZipArchive::new(file)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_entry_exists(path: &str, entry: &str) -> bool {
    let Ok(mut archive) = zip_archive_open(path) else {
        return false;
    };

    archive.by_name(entry).is_ok()
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_has_prefix(path: &str, prefix: &str) -> bool {
    let Ok(mut archive) = zip_archive_open(path) else {
        return false;
    };
    let mut normalized = prefix.replace('\\', "/");
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            continue;
        };
        let name = file.name();
        if name.starts_with(&normalized) {
            return true;
        }
    }
    false
}

#[derive(Default)]
#[cfg(feature = "stdlib_archive")]
pub(super) struct ZipArchiveIndex {
    pub(super) entries: HashSet<String>,
    pub(super) prefixes: HashSet<String>,
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_build_index(path: &str) -> Option<ZipArchiveIndex> {
    let mut archive = zip_archive_open(path).ok()?;
    let mut entries: HashSet<String> = HashSet::new();
    let mut prefixes: HashSet<String> = HashSet::new();
    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            continue;
        };
        let mut name = file.name().replace('\\', "/");
        if name.is_empty() {
            continue;
        }
        let is_dir_entry = name.ends_with('/');
        name = name.trim_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        entries.insert(name.clone());
        if is_dir_entry {
            prefixes.insert(name.clone());
        }
        let mut cursor = name.as_str();
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            prefixes.insert(parent.to_string());
            cursor = parent;
        }
    }
    Some(ZipArchiveIndex { entries, prefixes })
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_index_cached<'a>(
    cache: &'a mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
) -> Option<&'a ZipArchiveIndex> {
    cache
        .entry(path.to_string())
        .or_insert_with(|| zip_archive_build_index(path))
        .as_ref()
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_entry_exists_cached(
    cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
    entry: &str,
) -> bool {
    let normalized = entry.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        return false;
    }
    zip_archive_index_cached(cache, path)
        .map(|index| index.entries.contains(&normalized))
        .unwrap_or(false)
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_has_prefix_cached(
    cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
    prefix: &str,
) -> bool {
    let normalized = prefix.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        return false;
    }
    zip_archive_index_cached(cache, path)
        .map(|index| index.prefixes.contains(&normalized))
        .unwrap_or(false)
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_read_entry(path: &str, entry: &str) -> Result<Vec<u8>, std::io::Error> {
    let mut archive = zip_archive_open(path)?;
    let mut file = archive
        .by_name(entry)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::NotFound, err.to_string()))?;
    let mut bytes: Vec<u8> = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(feature = "stdlib_archive")]
pub(super) fn importlib_zip_source_exec_payload(
    module_name: &str,
    archive_path: &str,
    inner_path: &str,
    spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, std::io::Error> {
    let source_bytes = zip_archive_read_entry(archive_path, inner_path)?;
    let source =
        match crate::object::ops::decode_bytes_text("utf-8", "surrogateescape", &source_bytes) {
            Ok((text, _encoding)) => text,
            Err(_) => String::from_utf8_lossy(&source_bytes)
                .into_owned()
                .into_bytes(),
        };
    let origin = format!("{archive_path}/{inner_path}");
    let resolution = source_loader_resolution(module_name, &origin, spec_is_package);
    Ok(ImportlibZipSourceExecPayload {
        source,
        is_package: resolution.is_package,
        module_package: resolution.module_package,
        package_root: resolution.package_root,
        origin,
    })
}

// --- Stubs when stdlib_archive is disabled ---

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_entry_exists(_path: &str, _entry: &str) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_has_prefix(_path: &str, _prefix: &str) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
#[derive(Default)]
pub(super) struct ZipArchiveIndex;

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_index_cached<'a>(
    cache: &'a mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
) -> Option<&'a ZipArchiveIndex> {
    cache.entry(path.to_string()).or_insert(None).as_ref()
}

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_entry_exists_cached(
    _cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    _path: &str,
    _entry: &str,
) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_has_prefix_cached(
    _cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    _path: &str,
    _prefix: &str,
) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_read_entry(_path: &str, _entry: &str) -> Result<Vec<u8>, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "zip archive support requires the stdlib_archive feature",
    ))
}

#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn importlib_zip_source_exec_payload(
    _module_name: &str,
    _archive_path: &str,
    _inner_path: &str,
    _spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "zip archive support requires the stdlib_archive feature",
    ))
}

pub(super) fn importlib_cache_from_source(path: &str) -> String {
    let sep = bootstrap_path_sep();
    let base = path_basename_text(path, sep);
    if base.ends_with(".py") {
        let cache_dir = path_join_text(path_dirname_text(path, sep), "__pycache__", sep);
        return path_join_text(cache_dir, &format!("{base}c"), sep);
    }
    format!("{path}c")
}

pub(super) fn importlib_is_extension_filename(name: &str, module_name: &str) -> bool {
    if !name.starts_with(module_name) {
        return false;
    }
    let remainder = &name[module_name.len()..];
    if !remainder.starts_with('.') {
        return false;
    }
    if matches!(remainder, ".so" | ".pyd" | ".dll" | ".dylib") {
        return true;
    }
    (remainder.starts_with(".cpython-") && remainder.ends_with(".so"))
        || (remainder.starts_with(".abi") && remainder.ends_with(".so"))
        || (remainder.starts_with(".cp") && remainder.ends_with(".pyd"))
}

pub(super) fn importlib_find_extension_module(base_dir: &str, module_name: &str) -> Option<String> {
    let dir = if base_dir.is_empty() { "." } else { base_dir };
    let read_dir = std::fs::read_dir(dir).ok()?;
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !importlib_is_extension_filename(&file_name, module_name) {
            continue;
        }
        let file_type = entry.file_type().ok()?;
        if !file_type.is_file() {
            continue;
        }
        return Some(entry.path().to_string_lossy().into_owned());
    }
    None
}

pub(super) fn importlib_find_in_path(
    fullname: &str,
    search_paths: &[String],
    package_context: bool,
) -> Option<ImportlibPathResolution> {
    let sep = bootstrap_path_sep();
    let parts_all: Vec<&str> = fullname.split('.').collect();
    let parts: Vec<&str> = if package_context && parts_all.len() > 1 {
        vec![parts_all[parts_all.len() - 1]]
    } else {
        parts_all
    };
    if parts.is_empty() {
        return None;
    }
    let mut current_paths = search_paths.to_vec();
    let mut zip_index_cache: HashMap<String, Option<ZipArchiveIndex>> = HashMap::new();
    for (idx, part) in parts.iter().enumerate() {
        let is_last = idx + 1 == parts.len();
        let mut found_pkg = false;
        let mut next_paths: Vec<String> = Vec::new();
        let mut namespace_paths: Vec<String> = Vec::new();
        let mut namespace_seen: HashSet<String> = HashSet::new();
        for base in &current_paths {
            if let Some((zip_archive, zip_prefix)) = split_zip_archive_path(base)
                && zip_archive_index_cached(&mut zip_index_cache, &zip_archive).is_some()
            {
                let pkg_rel = zip_entry_join(&zip_prefix, part);
                let init_entry = format!("{pkg_rel}/__init__.py");
                if zip_archive_entry_exists_cached(&mut zip_index_cache, &zip_archive, &init_entry)
                {
                    if is_last {
                        return Some(ImportlibPathResolution {
                            origin: Some(format!("{zip_archive}/{init_entry}")),
                            is_package: true,
                            submodule_search_locations: Some(vec![format!(
                                "{zip_archive}/{pkg_rel}"
                            )]),
                            cached: None,
                            has_location: true,
                            loader_kind: "zip_source".to_string(),
                            zip_archive: Some(zip_archive),
                            zip_inner_path: Some(init_entry),
                        });
                    }
                    next_paths = vec![format!("{zip_archive}/{pkg_rel}")];
                    found_pkg = true;
                    break;
                }
                if zip_archive_has_prefix_cached(&mut zip_index_cache, &zip_archive, &pkg_rel) {
                    append_unique_path_hashed(
                        &mut namespace_paths,
                        &mut namespace_seen,
                        &format!("{zip_archive}/{pkg_rel}"),
                    );
                }
                if is_last {
                    let mod_entry = zip_entry_join(&zip_prefix, &format!("{part}.py"));
                    if zip_archive_entry_exists_cached(
                        &mut zip_index_cache,
                        &zip_archive,
                        &mod_entry,
                    ) {
                        return Some(ImportlibPathResolution {
                            origin: Some(format!("{zip_archive}/{mod_entry}")),
                            is_package: false,
                            submodule_search_locations: None,
                            cached: None,
                            has_location: true,
                            loader_kind: "zip_source".to_string(),
                            zip_archive: Some(zip_archive),
                            zip_inner_path: Some(mod_entry),
                        });
                    }
                }
                continue;
            }
            let root = if base.is_empty() {
                ".".to_string()
            } else {
                base.clone()
            };
            let pkg_dir = path_join_text(root.clone(), part, sep);
            let init_file = path_join_text(pkg_dir.clone(), "__init__.py", sep);
            if std::fs::metadata(&init_file)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                if is_last {
                    return Some(ImportlibPathResolution {
                        origin: Some(init_file.clone()),
                        is_package: true,
                        submodule_search_locations: Some(vec![pkg_dir]),
                        cached: Some(importlib_cache_from_source(&init_file)),
                        has_location: true,
                        loader_kind: "source".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                next_paths = vec![pkg_dir];
                found_pkg = true;
                break;
            }
            let init_pyc = path_join_text(pkg_dir.clone(), "__init__.pyc", sep);
            if std::fs::metadata(&init_pyc)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                if is_last {
                    return Some(ImportlibPathResolution {
                        origin: Some(init_pyc),
                        is_package: true,
                        submodule_search_locations: Some(vec![pkg_dir]),
                        cached: None,
                        has_location: true,
                        loader_kind: "bytecode".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                next_paths = vec![pkg_dir];
                found_pkg = true;
                break;
            }
            if std::fs::metadata(&pkg_dir)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
            {
                append_unique_path_hashed(&mut namespace_paths, &mut namespace_seen, &pkg_dir);
            }
            if is_last {
                let mod_file = path_join_text(root.clone(), &format!("{part}.py"), sep);
                if std::fs::metadata(&mod_file)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Some(ImportlibPathResolution {
                        origin: Some(mod_file.clone()),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: Some(importlib_cache_from_source(&mod_file)),
                        has_location: true,
                        loader_kind: "source".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                if let Some(ext_file) = importlib_find_extension_module(base, part) {
                    return Some(ImportlibPathResolution {
                        origin: Some(ext_file),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: None,
                        has_location: true,
                        loader_kind: "extension".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                let bytecode_file = path_join_text(root, &format!("{part}.pyc"), sep);
                if std::fs::metadata(&bytecode_file)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Some(ImportlibPathResolution {
                        origin: Some(bytecode_file),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: None,
                        has_location: true,
                        loader_kind: "bytecode".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
            }
        }
        if found_pkg {
            current_paths = next_paths;
            continue;
        }
        if !namespace_paths.is_empty() {
            if is_last {
                return Some(ImportlibPathResolution {
                    origin: None,
                    is_package: true,
                    submodule_search_locations: Some(namespace_paths),
                    cached: None,
                    has_location: false,
                    loader_kind: "namespace".to_string(),
                    zip_archive: None,
                    zip_inner_path: None,
                });
            }
            current_paths = namespace_paths;
            continue;
        }
        return None;
    }
    None
}

pub(super) fn importlib_search_paths(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<String> {
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(module_file);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in search_paths {
        append_unique_path_hashed(&mut out, &mut seen, entry);
    }
    if let Some(stdlib_root) = state.stdlib_root.as_deref() {
        append_unique_path_hashed(&mut out, &mut seen, stdlib_root);
    }
    for root in &state.module_roots_entries {
        append_unique_path_hashed(&mut out, &mut seen, root);
    }
    for base in search_paths {
        let root = if base.is_empty() { "." } else { base.as_str() };
        let candidate =
            path_join_text(path_join_text(root.to_string(), "molt", sep), "stdlib", sep);
        append_unique_path_hashed(&mut out, &mut seen, &candidate);
    }
    out
}

pub(super) fn importlib_module_root_package_context_paths(
    fullname: &str,
    module_file: Option<String>,
) -> Vec<String> {
    let parts: Vec<&str> = fullname.split('.').collect();
    if parts.len() <= 1 {
        return Vec::new();
    }
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(module_file);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for root in state.module_roots_entries {
        let mut package_path = root;
        for part in &parts[..parts.len() - 1] {
            package_path = path_join_text(package_path, part, sep);
        }
        append_unique_path_hashed(&mut out, &mut seen, &package_path);
    }
    out
}

pub(super) fn importlib_find_spec_search_paths(
    fullname: &str,
    search_paths: &[String],
    module_file: Option<String>,
    package_context: bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if package_context {
        for entry in importlib_module_root_package_context_paths(fullname, module_file.clone()) {
            append_unique_path_hashed(&mut out, &mut seen, &entry);
        }
    }
    for entry in importlib_search_paths(search_paths, module_file) {
        append_unique_path_hashed(&mut out, &mut seen, &entry);
    }
    out
}

pub(super) fn importlib_bootstrap_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> ImportlibBootstrapPayload {
    let state = sys_bootstrap_state_from_module_file(module_file.clone());
    let resolved_search_paths = importlib_search_paths(search_paths, module_file);
    ImportlibBootstrapPayload {
        resolved_search_paths,
        pythonpath_entries: state.pythonpath_entries,
        module_roots_entries: state.module_roots_entries,
        venv_site_packages_entries: state.venv_site_packages_entries,
        pwd: state.pwd,
        include_cwd: state.include_cwd,
        stdlib_root: state.stdlib_root,
    }
}

pub(super) fn importlib_namespace_paths(
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<String> {
    if package.is_empty() {
        return Vec::new();
    }
    let sep = bootstrap_path_sep();
    let mut resolved = importlib_search_paths(search_paths, module_file.clone());
    if !resolved.iter().any(|entry| entry.is_empty()) {
        resolved.push(String::new());
    }
    let mut resolved_seen: HashSet<String> = resolved.iter().cloned().collect();
    let state = sys_bootstrap_state_from_module_file(module_file);
    if !state.pwd.is_empty() {
        append_unique_path_hashed(&mut resolved, &mut resolved_seen, &state.pwd);
    }
    let mut matches: Vec<String> = Vec::new();
    let mut matches_seen: HashSet<String> = HashSet::new();
    let mut zip_index_cache: HashMap<String, Option<ZipArchiveIndex>> = HashMap::new();
    let parts: Vec<&str> = package.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return matches;
    }
    for base in resolved {
        if let Some((archive_path, zip_prefix)) = split_zip_archive_path(&base)
            && zip_archive_index_cached(&mut zip_index_cache, &archive_path).is_some()
        {
            let mut rel = zip_prefix;
            for part in &parts {
                rel = zip_entry_join(&rel, part);
            }
            if zip_archive_has_prefix_cached(&mut zip_index_cache, &archive_path, &rel) {
                append_unique_path_hashed(
                    &mut matches,
                    &mut matches_seen,
                    &format!("{archive_path}/{rel}"),
                );
            }
            continue;
        }
        let mut path = if base.is_empty() {
            ".".to_string()
        } else {
            base
        };
        for part in &parts {
            path = path_join_text(path, part, sep);
        }
        if std::fs::metadata(&path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            append_unique_path_hashed(&mut matches, &mut matches_seen, &path);
        }
    }
    matches
}

pub(super) fn importlib_metadata_dist_paths(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<String> {
    let resolved = importlib_search_paths(search_paths, module_file);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for base in resolved {
        if base.is_empty() {
            continue;
        }
        let read_dir = match std::fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".dist-info") && !name.ends_with(".egg-info") {
                continue;
            }
            let path = entry.path();
            let path_text = path.to_string_lossy().into_owned();
            append_unique_path_hashed(&mut out, &mut seen, &path_text);
        }
    }
    out
}

pub(super) fn importlib_metadata_entry_points_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<(String, String, String)> {
    importlib_metadata_entry_points_select_payload(search_paths, module_file, None, None)
}

pub(super) fn importlib_metadata_distributions_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<ImportlibMetadataPayload> {
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    let mut out: Vec<ImportlibMetadataPayload> = Vec::with_capacity(dist_paths.len());
    for path in dist_paths {
        out.push(importlib_metadata_payload(&path));
    }
    out
}

pub(super) fn importlib_metadata_entry_points_select_payload(
    search_paths: &[String],
    module_file: Option<String>,
    group: Option<&str>,
    name: Option<&str>,
) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        for (entry_name, entry_value, entry_group) in payload.entry_points {
            if let Some(expected_group) = group
                && entry_group != expected_group
            {
                continue;
            }
            if let Some(expected_name) = name
                && entry_name != expected_name
            {
                continue;
            }
            out.push((entry_name, entry_value, entry_group));
        }
    }
    out
}

pub(super) fn importlib_metadata_entry_points_filter_payload(
    search_paths: &[String],
    module_file: Option<String>,
    group: Option<&str>,
    name: Option<&str>,
    value: Option<&str>,
) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        for (entry_name, entry_value, entry_group) in payload.entry_points {
            if let Some(expected_group) = group
                && entry_group != expected_group
            {
                continue;
            }
            if let Some(expected_name) = name
                && entry_name != expected_name
            {
                continue;
            }
            if let Some(expected_value) = value
                && entry_value != expected_value
            {
                continue;
            }
            out.push((entry_name, entry_value, entry_group));
        }
    }
    out
}

pub(super) fn importlib_metadata_normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
            continue;
        }
        for lowered in ch.to_lowercase() {
            out.push(lowered);
        }
        prev_sep = false;
    }
    out
}

pub(super) fn importlib_enforce_extension_spec_boundary(
    _py: &PyToken<'_>,
    module_name: &str,
    resolution: &ImportlibPathResolution,
) -> Result<(), u64> {
    if resolution.loader_kind != "extension" {
        return Ok(());
    }
    let Some(origin) = resolution.origin.as_deref() else {
        return Err(raise_exception::<u64>(
            _py,
            "ImportError",
            "extension module path must point to a file",
        ));
    };
    match importlib_path_is_file(_py, origin) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<u64>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    importlib_require_extension_metadata(_py, module_name, origin)
}

pub(super) fn importlib_find_spec_payload(
    _py: &PyToken<'_>,
    fullname: &str,
    search_paths: &[String],
    module_file: Option<String>,
    meta_path_count: i64,
    path_hooks_count: i64,
    package_context: bool,
) -> Result<Option<ImportlibFindSpecPayload>, u64> {
    let resolved =
        importlib_find_spec_search_paths(fullname, search_paths, module_file, package_context);
    let Some(resolution) = importlib_find_in_path(fullname, &resolved, package_context) else {
        return Ok(None);
    };
    importlib_enforce_extension_spec_boundary(_py, fullname, &resolution)?;
    Ok(Some(ImportlibFindSpecPayload {
        origin: resolution.origin,
        is_package: resolution.is_package,
        submodule_search_locations: resolution.submodule_search_locations,
        cached: resolution.cached,
        is_builtin: false,
        has_location: resolution.has_location,
        loader_kind: resolution.loader_kind,
        zip_archive: resolution.zip_archive,
        zip_inner_path: resolution.zip_inner_path,
        meta_path_count,
        path_hooks_count,
    }))
}

pub(super) fn importlib_metadata_parse_headers(text: &str) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = Vec::new();
    let mut current_idx: Option<usize> = None;
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            current_idx = None;
            continue;
        }
        let bytes = raw_line.as_bytes();
        if !bytes.is_empty() && (bytes[0] == b' ' || bytes[0] == b'\t') {
            if let Some(idx) = current_idx {
                mapping[idx].1.push('\n');
                mapping[idx].1.push_str(raw_line.trim());
            }
            continue;
        }
        if let Some((key, value)) = raw_line.split_once(':') {
            mapping.push((key.trim().to_string(), value.trim().to_string()));
            current_idx = Some(mapping.len() - 1);
        }
    }
    mapping
}

pub(super) fn importlib_metadata_header_values(
    headers: &[(String, String)],
    key: &str,
) -> Vec<String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            if k.eq_ignore_ascii_case(key) {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn importlib_metadata_first_nonempty(
    headers: &[(String, String)],
    key: &str,
) -> Option<String> {
    importlib_metadata_header_values(headers, key)
        .into_iter()
        .find(|value| !value.is_empty())
}

pub(super) fn importlib_metadata_parse_entry_points(text: &str) -> Vec<(String, String, String)> {
    let mut group: Option<String> = None;
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if stripped.starts_with('[') && stripped.ends_with(']') {
            group = Some(stripped[1..stripped.len() - 1].trim().to_string());
            continue;
        }
        let Some(current_group) = group.as_ref() else {
            continue;
        };
        let Some((name, value)) = stripped.split_once('=') else {
            continue;
        };
        out.push((
            name.trim().to_string(),
            value.trim().to_string(),
            current_group.clone(),
        ));
    }
    out
}

pub(super) fn importlib_metadata_parse_csv_row(row: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = row.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek().is_some_and(|next| *next == '"') {
                    current.push('"');
                    let _ = chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            ',' => {
                out.push(current);
                current = String::new();
            }
            '"' => in_quotes = true,
            _ => current.push(ch),
        }
    }
    out.push(current);
    out
}

pub(super) fn importlib_metadata_record_payload(path: &str) -> Vec<ImportlibMetadataRecordEntry> {
    let sep = bootstrap_path_sep();
    let record_path = path_join_text(path.to_string(), "RECORD", sep);
    let record_text = match std::fs::read(&record_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<ImportlibMetadataRecordEntry> = Vec::new();
    for raw_line in record_text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let fields = importlib_metadata_parse_csv_row(line);
        let Some(path_field) = fields.first() else {
            continue;
        };
        let rel_path = path_field.trim();
        if rel_path.is_empty() {
            continue;
        }
        let hash = fields
            .get(1)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let size = fields
            .get(2)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        out.push(ImportlibMetadataRecordEntry {
            path: rel_path.to_string(),
            hash,
            size,
        });
    }
    out
}

pub(super) fn importlib_metadata_packages_distributions_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<(String, Vec<String>)> {
    let mut mapping: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let sep = bootstrap_path_sep();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        let dist_name = payload.name.trim().to_string();
        if dist_name.is_empty() {
            continue;
        }
        let top_level_path = path_join_text(path, "top_level.txt", sep);
        let top_level_text = match std::fs::read(&top_level_path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => continue,
        };
        for line in top_level_text.lines() {
            let package = line.trim();
            if package.is_empty() {
                continue;
            }
            mapping
                .entry(package.to_string())
                .or_default()
                .insert(dist_name.clone());
        }
    }
    mapping
        .into_iter()
        .map(|(package, providers)| (package, providers.into_iter().collect()))
        .collect()
}

pub(super) fn importlib_metadata_payload(path: &str) -> ImportlibMetadataPayload {
    let sep = bootstrap_path_sep();
    let base = path_basename_text(path, sep);
    let fallback_name = base
        .split_once('-')
        .map(|(name, _)| name)
        .unwrap_or(base.as_str())
        .to_string();

    let metadata_path = path_join_text(path.to_string(), "METADATA", sep);
    let pkg_info_path = path_join_text(path.to_string(), "PKG-INFO", sep);
    let metadata_text = std::fs::read(&metadata_path)
        .or_else(|_| std::fs::read(&pkg_info_path))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let metadata_pairs = metadata_text
        .as_deref()
        .map(importlib_metadata_parse_headers)
        .unwrap_or_default();
    let name = importlib_metadata_first_nonempty(&metadata_pairs, "Name").unwrap_or(fallback_name);
    let version = importlib_metadata_first_nonempty(&metadata_pairs, "Version").unwrap_or_default();
    let requires_python = importlib_metadata_first_nonempty(&metadata_pairs, "Requires-Python");
    let requires_dist = importlib_metadata_header_values(&metadata_pairs, "Requires-Dist");
    let provides_extra = importlib_metadata_header_values(&metadata_pairs, "Provides-Extra");

    let entry_points_path = path_join_text(path.to_string(), "entry_points.txt", sep);
    let entry_points = std::fs::read(&entry_points_path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .as_deref()
        .map(importlib_metadata_parse_entry_points)
        .unwrap_or_default();

    ImportlibMetadataPayload {
        path: path.to_string(),
        name,
        version,
        metadata: metadata_pairs,
        entry_points,
        requires_dist,
        provides_extra,
        requires_python,
    }
}

pub(super) fn importlib_spec_from_file_location_payload(
    path: &str,
) -> ImportlibSpecFromFileLocationPayload {
    let sep = bootstrap_path_sep();
    let is_package = path_basename_text(path, sep) == "__init__.py";
    let package_root = if is_package {
        Some(path_dirname_text(path, sep))
    } else {
        None
    };
    ImportlibSpecFromFileLocationPayload {
        path: path.to_string(),
        is_package,
        package_root,
    }
}

pub(super) fn importlib_normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn importlib_is_archive_member_path(path: &str) -> bool {
    importlib_normalize_path_text(path).contains(".zip/")
}

pub(super) fn importlib_package_root_from_origin(path: &str) -> Option<String> {
    let normalized = importlib_normalize_path_text(path);
    if normalized.ends_with("/__init__.py") || normalized.ends_with("/__init__.pyc") {
        return normalized
            .rsplit_once('/')
            .map(|(root, _)| root.to_string());
    }
    None
}

pub(super) fn importlib_iter_next_value_bits(
    _py: &PyToken<'_>,
    iter_bits: u64,
) -> Result<Option<u64>, u64> {
    let pair_bits = molt_iter_next(iter_bits);
    let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return Err(MoltObject::none().bits());
        }
    }
    let pair = unsafe { seq_vec_ref(pair_ptr) };
    if pair.len() < 2 {
        return Err(MoltObject::none().bits());
    }
    if is_truthy(_py, obj_from_bits(pair[1])) {
        return Ok(None);
    }
    inc_ref_bits(_py, pair[0]);
    Ok(Some(pair[0]))
}

pub(super) fn importlib_best_effort_str(_py: &PyToken<'_>, value_bits: u64) -> String {
    let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, value_bits) };
    if exception_pending(_py) {
        clear_exception(_py);
        return String::from("<object>");
    }
    let out =
        string_obj_to_owned(obj_from_bits(text_bits)).unwrap_or_else(|| String::from("<object>"));
    if !obj_from_bits(text_bits).is_none() {
        dec_ref_bits(_py, text_bits);
    }
    out
}

pub(super) fn importlib_exception_name_from_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
) -> Option<String> {
    let name_attr = intern_runtime_static_name(_py, b"__name__");
    let missing = missing_bits(_py);
    let name_bits = molt_getattr_builtin(class_bits, name_attr, missing);
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if is_missing_bits(_py, name_bits) {
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        return None;
    }
    let name = string_obj_to_owned(obj_from_bits(name_bits));
    if !obj_from_bits(name_bits).is_none() {
        dec_ref_bits(_py, name_bits);
    }
    name
}

pub(super) fn raise_importlib_io_error(_py: &PyToken<'_>, err: std::io::Error) -> u64 {
    let message = err.to_string();
    match err.kind() {
        std::io::ErrorKind::NotFound => {
            raise_exception::<u64>(_py, "FileNotFoundError", message.as_str())
        }
        std::io::ErrorKind::PermissionDenied => {
            raise_exception::<u64>(_py, "PermissionError", message.as_str())
        }
        std::io::ErrorKind::IsADirectory => {
            raise_exception::<u64>(_py, "IsADirectoryError", message.as_str())
        }
        _ => raise_exception::<u64>(_py, "OSError", message.as_str()),
    }
}

pub(super) fn importlib_read_file_bytes(_py: &PyToken<'_>, path: &str) -> Result<Vec<u8>, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = match std::fs::metadata(&archive_path) {
            Ok(metadata) => metadata.is_file(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(raise_importlib_io_error(_py, err)),
        };
        if archive_exists {
            if inner_path.is_empty() {
                return Err(raise_importlib_io_error(
                    _py,
                    std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        "zip archive root is a directory",
                    ),
                ));
            }
            return match zip_archive_read_entry(&archive_path, &inner_path) {
                Ok(bytes) => Ok(bytes),
                Err(err) => {
                    if zip_archive_has_prefix(&archive_path, &inner_path) {
                        Err(raise_importlib_io_error(
                            _py,
                            std::io::Error::new(std::io::ErrorKind::IsADirectory, err.to_string()),
                        ))
                    } else {
                        Err(raise_importlib_io_error(_py, err))
                    }
                }
            };
        }
    }
    std::fs::read(path).map_err(|err| raise_importlib_io_error(_py, err))
}

pub(super) fn importlib_path_is_file(_py: &PyToken<'_>, path: &str) -> Result<bool, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = match std::fs::metadata(&archive_path) {
            Ok(metadata) => metadata.is_file(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(raise_importlib_io_error(_py, err)),
        };
        if archive_exists {
            if inner_path.is_empty() {
                return Ok(false);
            }
            if zip_archive_entry_exists(&archive_path, &inner_path) {
                return Ok(true);
            }
            return Ok(false);
        }
    }
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(raise_importlib_io_error(_py, err)),
    }
}

pub(super) struct LoadedExtensionManifest {
    pub(super) source: String,
    pub(super) manifest: JsonValue,
    pub(super) wheel_path: Option<String>,
}

pub(super) fn importlib_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

pub(super) fn importlib_sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    importlib_hex_lower(digest.as_ref())
}

pub(super) fn importlib_sha256_file(path: &str) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    Ok(importlib_hex_lower(digest.as_ref()))
}

pub(super) fn importlib_metadata_timestamp_nanos(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

pub(super) fn importlib_metadata_fingerprint(path: &str) -> Result<String, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    let kind = if metadata.is_file() {
        "f"
    } else if metadata.is_dir() {
        "d"
    } else {
        "o"
    };
    Ok(format!(
        "{kind}:{}:{}",
        metadata.len(),
        importlib_metadata_timestamp_nanos(&metadata)
    ))
}

pub(super) fn importlib_cache_fingerprint_for_path(path: &str) -> Result<String, std::io::Error> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_fp = importlib_metadata_fingerprint(&archive_path)?;
        return Ok(format!("zip:{archive_fp}:{inner_path}"));
    }
    importlib_metadata_fingerprint(path).map(|fp| format!("file:{fp}"))
}

pub(super) fn importlib_manifest_cache_fingerprint(
    loaded: &LoadedExtensionManifest,
) -> Result<String, std::io::Error> {
    if let Some(wheel_path) = loaded.wheel_path.as_deref() {
        let wheel_fp = importlib_metadata_fingerprint(wheel_path)?;
        return Ok(format!("wheel:{wheel_fp}:{}", loaded.source));
    }
    let sidecar_fp = importlib_metadata_fingerprint(loaded.source.as_str())?;
    Ok(format!("sidecar:{sidecar_fp}:{}", loaded.source))
}

pub(super) fn importlib_sha256_path(_py: &PyToken<'_>, path: &str) -> Result<String, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let bytes = zip_archive_read_entry(&archive_path, &inner_path)
            .map_err(|err| raise_importlib_io_error(_py, err))?;
        return Ok(importlib_sha256_hex(&bytes));
    }
    importlib_sha256_file(path).map_err(|err| raise_importlib_io_error(_py, err))
}

pub(super) fn importlib_normalize_path_separators(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn importlib_extension_path_matches_manifest(
    path: &str,
    manifest_extension: &str,
) -> bool {
    let path_norm = importlib_normalize_path_separators(path);
    let expected_norm = importlib_normalize_path_separators(manifest_extension)
        .trim_matches('/')
        .to_string();
    if expected_norm.is_empty() {
        return false;
    }
    if path_norm == expected_norm || path_norm.ends_with(&format!("/{expected_norm}")) {
        return true;
    }
    let path_name = Path::new(path_norm.as_str())
        .file_name()
        .and_then(|value| value.to_str());
    let expected_name = Path::new(expected_norm.as_str())
        .file_name()
        .and_then(|value| value.to_str());
    path_name.is_some() && path_name == expected_name
}

pub(super) fn importlib_find_extension_manifest_sidecar(
    path: &str,
) -> Result<Option<String>, std::io::Error> {
    let mut current = Path::new(path).parent();
    for _ in 0..6 {
        let Some(dir) = current else {
            break;
        };
        let candidate = dir.join("extension_manifest.json");
        match std::fs::metadata(&candidate) {
            Ok(meta) => {
                if meta.is_file() {
                    return Ok(Some(candidate.to_string_lossy().into_owned()));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        current = dir.parent();
    }
    Ok(None)
}

pub(super) fn importlib_load_extension_manifest_for_path(
    _py: &PyToken<'_>,
    path: &str,
) -> Result<LoadedExtensionManifest, u64> {
    if let Some((archive_path, _inner_path)) = split_zip_archive_path(path) {
        let manifest_bytes = match zip_archive_read_entry(&archive_path, "extension_manifest.json")
        {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "extension metadata missing for {path:?}: unable to read extension_manifest.json ({err})"
                    ),
                ));
            }
        };
        let manifest = match serde_json::from_slice::<JsonValue>(&manifest_bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "invalid extension metadata in {archive_path}/extension_manifest.json: {err}"
                    ),
                ));
            }
        };
        return Ok(LoadedExtensionManifest {
            source: format!("{archive_path}/extension_manifest.json"),
            manifest,
            wheel_path: Some(archive_path),
        });
    }

    let Some(manifest_path) = importlib_find_extension_manifest_sidecar(path)
        .map_err(|err| raise_importlib_io_error(_py, err))?
    else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata missing for {path:?}: extension_manifest.json not found near extension path"
            ),
        ));
    };
    let manifest_bytes =
        std::fs::read(&manifest_path).map_err(|err| raise_importlib_io_error(_py, err))?;
    let manifest = serde_json::from_slice::<JsonValue>(&manifest_bytes).map_err(|err| {
        raise_exception::<u64>(
            _py,
            "ImportError",
            &format!("invalid extension metadata in {manifest_path}: {err}"),
        )
    })?;
    Ok(LoadedExtensionManifest {
        source: manifest_path,
        manifest,
        wheel_path: None,
    })
}

pub(super) fn importlib_manifest_required_string<'a>(
    _py: &PyToken<'_>,
    manifest: &'a serde_json::Map<String, JsonValue>,
    field: &str,
    manifest_source: &str,
) -> Result<&'a str, u64> {
    let value = manifest
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {manifest_source}: missing or invalid field {field:?}"
                ),
            )
        })?;
    Ok(value)
}

pub(super) fn importlib_validate_extension_metadata(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    extension_sha256: &str,
    loaded: &LoadedExtensionManifest,
) -> Result<(), u64> {
    let Some(manifest) = loaded.manifest.as_object() else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "invalid extension metadata {}: payload must be a JSON object",
                loaded.source
            ),
        ));
    };

    let manifest_module =
        importlib_manifest_required_string(_py, manifest, "module", loaded.source.as_str())?;
    if manifest_module != module_name {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata module mismatch in {}: manifest has {:?}, loader requested {:?}",
                loaded.source, manifest_module, module_name
            ),
        ));
    }

    let abi_version = importlib_manifest_required_string(
        _py,
        manifest,
        "molt_c_api_version",
        loaded.source.as_str(),
    )?;
    let manifest_abi_major = abi_version
        .split('.')
        .next()
        .and_then(|segment| segment.parse::<u32>().ok())
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension ABI version in {}: {:?}",
                    loaded.source, abi_version
                ),
            )
        })?;
    let runtime_abi_major = crate::c_api::MOLT_C_API_VERSION;
    if manifest_abi_major != runtime_abi_major {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension ABI mismatch for {:?}: runtime requires {}, manifest declares {} (source: {})",
                module_name, runtime_abi_major, manifest_abi_major, loaded.source
            ),
        ));
    }

    let abi_tag =
        importlib_manifest_required_string(_py, manifest, "abi_tag", loaded.source.as_str())?;
    let expected_abi_tag = format!("molt_abi{manifest_abi_major}");
    if abi_tag != expected_abi_tag {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension ABI tag mismatch in {}: expected {expected_abi_tag}, found {abi_tag}",
                loaded.source
            ),
        ));
    }

    let _target_triple =
        importlib_manifest_required_string(_py, manifest, "target_triple", loaded.source.as_str())?;
    let _platform_tag =
        importlib_manifest_required_string(_py, manifest, "platform_tag", loaded.source.as_str())?;

    let manifest_extension =
        importlib_manifest_required_string(_py, manifest, "extension", loaded.source.as_str())?;
    if !importlib_extension_path_matches_manifest(path, manifest_extension) {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata path mismatch in {}: manifest expects {:?}, loader path is {:?}",
                loaded.source, manifest_extension, path
            ),
        ));
    }

    let expected_extension_sha = importlib_manifest_required_string(
        _py,
        manifest,
        "extension_sha256",
        loaded.source.as_str(),
    )?
    .to_ascii_lowercase();
    if expected_extension_sha != extension_sha256 {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension checksum mismatch for {:?} at {:?} using metadata {}: expected {}, got {}",
                module_name, path, loaded.source, expected_extension_sha, extension_sha256
            ),
        ));
    }

    if let Some(wheel_path) = loaded.wheel_path.as_deref() {
        let expected_wheel_sha = importlib_manifest_required_string(
            _py,
            manifest,
            "wheel_sha256",
            loaded.source.as_str(),
        )?
        .to_ascii_lowercase();
        let actual_wheel_sha =
            importlib_sha256_file(wheel_path).map_err(|err| raise_importlib_io_error(_py, err))?;
        if expected_wheel_sha != actual_wheel_sha {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "wheel checksum mismatch for extension metadata {}: expected {}, got {}",
                    loaded.source, expected_wheel_sha, actual_wheel_sha
                ),
            ));
        }

        if let Some(wheel_name) = manifest
            .get("wheel")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let archive_name = Path::new(wheel_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if archive_name != wheel_name {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "wheel name mismatch in {}: manifest has {:?}, archive is {:?}",
                        loaded.source, wheel_name, archive_name
                    ),
                ));
            }
        }
    }

    let capabilities = manifest
        .get("capabilities")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capabilities must be an array",
                    loaded.source
                ),
            )
        })?;
    if capabilities.is_empty() {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "invalid extension metadata {}: capabilities list must not be empty",
                loaded.source
            ),
        ));
    }
    let mut missing_caps: BTreeSet<String> = BTreeSet::new();
    for cap_value in capabilities {
        let Some(cap_raw) = cap_value.as_str() else {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capabilities must contain only strings",
                    loaded.source
                ),
            ));
        };
        let cap = cap_raw.trim();
        if cap.is_empty() {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capability entries must be non-empty",
                    loaded.source
                ),
            ));
        }
        let allowed = has_capability(_py, cap);
        // Audit each individual capability check for extension loading.
        audit_emit(AuditEvent::new(
            "module.extension.cap_check",
            "module.extension",
            AuditArgs::Custom(cap.to_string()),
            if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {cap} capability for extension {module_name:?}"),
                }
            },
            module_path!().to_string(),
        ));
        if !allowed {
            missing_caps.insert(cap.to_string());
        }
    }
    if !missing_caps.is_empty() {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            &format!(
                "missing extension capabilities for {:?}: {}",
                module_name,
                missing_caps.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }

    Ok(())
}

pub(super) fn importlib_require_extension_metadata(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
) -> Result<(), u64> {
    let cache_key = format!("{module_name}\u{1f}{path}");
    let loaded = importlib_load_extension_manifest_for_path(_py, path)?;
    let path_fingerprint = importlib_cache_fingerprint_for_path(path)
        .map_err(|err| raise_importlib_io_error(_py, err))?;
    let manifest_fingerprint = importlib_manifest_cache_fingerprint(&loaded)
        .map_err(|err| raise_importlib_io_error(_py, err))?;
    let cache_value = format!("{path_fingerprint}\u{1f}{manifest_fingerprint}");
    {
        let cache = extension_metadata_ok_cache();
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard
            .get(&cache_key)
            .is_some_and(|value| value == &cache_value)
        {
            EXTENSION_METADATA_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    EXTENSION_METADATA_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);

    let extension_sha256 = importlib_sha256_path(_py, path)?;
    importlib_validate_extension_metadata(_py, module_name, path, &extension_sha256, &loaded)?;

    let cache = extension_metadata_ok_cache();
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(cache_key, cache_value);
    Ok(())
}

pub(super) fn importlib_path_has_extension_suffix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".so")
        || lower.ends_with(".pyd")
        || lower.ends_with(".dll")
        || lower.ends_with(".dylib")
}

pub(super) fn importlib_path_looks_like_extension(path: &str) -> bool {
    if let Some((_archive_path, inner_path)) = split_zip_archive_path(path) {
        return importlib_path_has_extension_suffix(&inner_path);
    }
    importlib_path_has_extension_suffix(path)
}

pub(super) fn importlib_spec_attr_string(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
) -> Result<Option<String>, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    let Some(attr_bits) = getattr_optional_bits(_py, target_bits, attr_name)? else {
        return Ok(None);
    };
    let value = string_obj_to_owned(obj_from_bits(attr_bits));
    if !obj_from_bits(attr_bits).is_none() {
        dec_ref_bits(_py, attr_bits);
    }
    Ok(value)
}

pub(super) fn importlib_extension_spec_target(
    _py: &PyToken<'_>,
    expected_module_name: &str,
    spec_bits: u64,
) -> Result<Option<(String, String)>, u64> {
    if obj_from_bits(spec_bits).is_none() {
        return Ok(None);
    }

    let module_name = importlib_spec_attr_string(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"name"),
        b"name",
    )?
    .unwrap_or_else(|| expected_module_name.to_string());
    let origin = importlib_spec_attr_string(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"origin"),
        b"origin",
    )?;
    let mut has_extension_loader = false;

    let loader_name = intern_runtime_static_name(_py, b"loader");
    if let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
        && !obj_from_bits(loader_bits).is_none()
    {
        let loader_type = type_name(_py, obj_from_bits(loader_bits));
        has_extension_loader = loader_type.contains("ExtensionFileLoader");
        dec_ref_bits(_py, loader_bits);
    }

    let Some(path) = origin else {
        if has_extension_loader {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        return Ok(None);
    };
    if !has_extension_loader && !importlib_path_looks_like_extension(&path) {
        return Ok(None);
    }
    Ok(Some((module_name, path)))
}

pub(super) fn importlib_enforce_extension_spec_object_boundary(
    _py: &PyToken<'_>,
    expected_module_name: &str,
    spec_bits: u64,
) -> Result<(), u64> {
    let Some((module_name, path)) =
        importlib_extension_spec_target(_py, expected_module_name, spec_bits)?
    else {
        return Ok(());
    };

    if split_zip_archive_path(&path).is_none() {
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                ));
            }
            Err(bits) => return Err(bits),
        }
    }
    importlib_require_extension_metadata(_py, &module_name, &path)
}
