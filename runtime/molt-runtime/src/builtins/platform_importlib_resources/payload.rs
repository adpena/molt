use super::*;

pub(super) struct ImportlibResourcesPathPayload {
    pub(super) basename: String,
    pub(super) exists: bool,
    pub(super) is_file: bool,
    pub(super) is_dir: bool,
    pub(super) entries: Vec<String>,
    pub(super) has_init_py: bool,
    pub(super) is_archive_member: bool,
}
pub(super) struct ImportlibResourcesPackagePayload {
    pub(super) roots: Vec<String>,
    pub(super) is_namespace: bool,
    pub(super) has_regular_package: bool,
    pub(super) init_file: Option<String>,
}
pub(super) struct ImportlibResourcesFilesPayload {
    pub(super) package_name: String,
    pub(super) roots: Vec<String>,
    pub(super) is_namespace: bool,
    pub(super) reader_bits: Option<u64>,
    pub(super) files_traversable_bits: Option<u64>,
}
#[cfg(feature = "stdlib_archive")]
pub(super) fn zip_archive_resources_path_payload(
    archive_path: &str,
    inner_path: &str,
    basename: String,
) -> ImportlibResourcesPathPayload {
    let mut exists = false;
    let mut is_file = false;
    let mut is_dir = false;
    let mut entries: BTreeSet<String> = BTreeSet::new();
    let mut has_init_py = false;

    let normalized_inner = inner_path.replace('\\', "/").trim_matches('/').to_string();
    if normalized_inner.is_empty() {
        exists = true;
        is_dir = true;
    }

    let mut archive = match zip_archive_open(archive_path) {
        Ok(archive) => archive,
        Err(_) => {
            return ImportlibResourcesPathPayload {
                basename,
                exists: false,
                is_file: false,
                is_dir: false,
                entries: Vec::new(),
                has_init_py: false,
                is_archive_member: true,
            };
        }
    };

    let prefix = if normalized_inner.is_empty() {
        String::new()
    } else {
        format!("{normalized_inner}/")
    };

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

        if !normalized_inner.is_empty() {
            if name == normalized_inner {
                exists = true;
                if is_dir_entry {
                    is_dir = true;
                } else {
                    is_file = true;
                }
                continue;
            }
            if !name.starts_with(&prefix) {
                continue;
            }
            exists = true;
            is_dir = true;
            let rel = &name[prefix.len()..];
            if rel.is_empty() {
                continue;
            }
            if let Some(child) = rel.split('/').next()
                && !child.is_empty()
            {
                entries.insert(child.to_string());
            }
            continue;
        }

        if let Some(child) = name.split('/').next()
            && !child.is_empty()
        {
            entries.insert(child.to_string());
        }
    }

    let entries_vec: Vec<String> = entries.into_iter().collect();
    if is_dir {
        has_init_py = entries_vec.iter().any(|entry| entry == "__init__.py");
    }
    if is_file {
        has_init_py = false;
    }

    ImportlibResourcesPathPayload {
        basename,
        exists,
        is_file,
        is_dir,
        entries: entries_vec,
        has_init_py,
        is_archive_member: true,
    }
}
#[cfg(not(feature = "stdlib_archive"))]
pub(super) fn zip_archive_resources_path_payload(
    _archive_path: &str,
    _inner_path: &str,
    basename: String,
) -> ImportlibResourcesPathPayload {
    ImportlibResourcesPathPayload {
        basename,
        exists: false,
        is_file: false,
        is_dir: false,
        entries: Vec::new(),
        has_init_py: false,
        is_archive_member: true,
    }
}
pub(super) fn importlib_resources_path_payload(path: &str) -> ImportlibResourcesPathPayload {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep);
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = std::fs::metadata(&archive_path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        if archive_exists {
            return zip_archive_resources_path_payload(&archive_path, &inner_path, basename);
        }
    }
    let mut entries: Vec<String> = Vec::new();
    let mut has_init_py = false;
    let mut exists = false;
    let mut is_file = false;
    let mut is_dir = false;
    if let Ok(metadata) = std::fs::metadata(path) {
        exists = true;
        is_file = metadata.is_file();
        is_dir = metadata.is_dir();
    }
    if is_dir && let Ok(read_dir) = std::fs::read_dir(path) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "__init__.py" {
                has_init_py = true;
            }
            entries.push(name);
        }
        entries.sort();
    }
    ImportlibResourcesPathPayload {
        basename,
        exists,
        is_file,
        is_dir,
        entries,
        has_init_py,
        is_archive_member: false,
    }
}
pub(super) fn importlib_resources_package_payload(
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> ImportlibResourcesPackagePayload {
    let resolved = importlib_search_paths(search_paths, module_file.clone());
    let resolution = importlib_find_in_path(package, &resolved, false);
    let mut roots: Vec<String> = Vec::new();
    let mut has_regular_package = false;
    let mut init_file: Option<String> = None;
    if let Some(spec) = resolution {
        if spec.is_package
            && matches!(
                spec.loader_kind.as_str(),
                "source" | "zip_source" | "bytecode"
            )
        {
            has_regular_package = true;
            init_file = spec.origin.clone();
            if let Some(locations) = spec.submodule_search_locations {
                for location in locations {
                    append_unique_path(&mut roots, &location);
                }
            } else {
                let sep = bootstrap_path_sep();
                if let Some(origin) = spec.origin.as_deref() {
                    let dir = path_dirname_text(origin, sep);
                    if !dir.is_empty() {
                        append_unique_path(&mut roots, &dir);
                    }
                }
            }
        } else if spec.is_package
            && spec.loader_kind == "namespace"
            && let Some(locations) = spec.submodule_search_locations
        {
            for location in locations {
                append_unique_path(&mut roots, &location);
            }
        }
    }
    let namespace_roots = importlib_namespace_paths(package, search_paths, module_file);
    for root in &namespace_roots {
        append_unique_path(&mut roots, root);
    }
    let is_namespace = !has_regular_package && !namespace_roots.is_empty();
    ImportlibResourcesPackagePayload {
        roots,
        is_namespace,
        has_regular_package,
        init_file,
    }
}
pub(super) fn importlib_resources_open_resource_bytes_from_package_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    resource: &str,
) -> Result<Vec<u8>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    importlib_validate_resource_name_text(_py, resource)?;
    if let Some(candidate) = importlib_resources_first_file_candidate(&payload.roots, resource) {
        return importlib_read_file_bytes(_py, &candidate);
    }
    Err(raise_exception::<_>(_py, "FileNotFoundError", resource))
}
pub(super) fn importlib_resources_join_parts_path(parts: &[String]) -> String {
    let sep = bootstrap_path_sep();
    let mut out = String::new();
    for part in parts {
        out = path_join_text(out, part, sep);
    }
    out
}
pub(super) fn importlib_resources_name_parts(name: &str) -> Vec<String> {
    let normalized = name.replace('\\', "/");
    normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}
pub(super) fn importlib_resources_candidate_path(root: &str, resource: &str) -> String {
    if resource.is_empty() {
        root.to_string()
    } else {
        let sep = bootstrap_path_sep();
        path_join_text(root.to_string(), resource, sep)
    }
}
pub(super) fn importlib_resources_open_resource_bytes_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Vec<u8>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    let mut first_dir: Option<String> = None;
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return importlib_read_file_bytes(_py, &candidate);
        }
        if payload.is_dir && first_dir.is_none() {
            first_dir = Some(candidate);
        }
    }
    if let Some(path) = first_dir {
        return Err(raise_exception::<_>(_py, "IsADirectoryError", &path));
    }
    let not_found = if resource.is_empty() {
        package
    } else {
        resource.as_str()
    };
    Err(raise_exception::<_>(_py, "FileNotFoundError", not_found))
}
pub(super) fn importlib_resources_is_resource_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<bool, u64> {
    if path_parts.is_empty() {
        return Ok(false);
    }
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn importlib_resources_contents_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Vec<String>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    let mut entries: BTreeSet<String> = BTreeSet::new();
    let mut has_init_py = false;
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if !payload.is_dir {
            continue;
        }
        has_init_py = has_init_py || payload.has_init_py;
        for entry in payload.entries {
            entries.insert(entry);
        }
    }
    if has_init_py {
        entries.insert(String::from("__pycache__"));
    }
    Ok(entries.into_iter().collect())
}
pub(super) fn importlib_resources_resource_path_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Option<String>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.exists && !payload.is_archive_member {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}
pub(super) fn importlib_resources_required_package_payload(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> Result<ImportlibResourcesPackagePayload, u64> {
    let payload = importlib_resources_package_payload(package, search_paths, module_file);
    if payload.roots.is_empty() {
        return Err(raise_exception::<_>(_py, "ModuleNotFoundError", package));
    }
    Ok(payload)
}
pub(super) fn importlib_resources_first_file_candidate(
    roots: &[String],
    resource: &str,
) -> Option<String> {
    let sep = bootstrap_path_sep();
    for root in roots {
        let candidate = path_join_text(root.clone(), resource, sep);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return Some(candidate);
        }
    }
    None
}
pub(super) fn importlib_resources_files_payload(
    _py: &PyToken<'_>,
    module_bits: u64,
    fallback_bits: u64,
    search_paths: &[String],
    module_file: Option<String>,
) -> Result<ImportlibResourcesFilesPayload, u64> {
    let package_name = importlib_resources_module_name_from_bits(_py, module_bits, fallback_bits)?;
    let package_payload =
        importlib_resources_package_payload(&package_name, search_paths, module_file);
    let reader_bits = importlib_resources_loader_reader_from_bits(_py, module_bits, &package_name)?;
    let mut roots = package_payload.roots;
    let mut is_namespace = package_payload.is_namespace;
    let mut files_traversable_bits: Option<u64> = None;
    if let Some(reader_bits_value) = reader_bits {
        files_traversable_bits = importlib_reader_files_traversable_bits(_py, reader_bits_value)?;
        let reader_roots = importlib_resources_reader_roots_impl(_py, reader_bits_value)?;
        if !reader_roots.is_empty() {
            roots = reader_roots;
            is_namespace = package_payload.is_namespace && roots.len() > 1;
        }
    }
    Ok(ImportlibResourcesFilesPayload {
        package_name,
        roots,
        is_namespace,
        reader_bits,
        files_traversable_bits,
    })
}
pub(super) fn importlib_resources_first_fs_file_candidate(
    roots: &[String],
    resource: &str,
) -> Option<String> {
    let sep = bootstrap_path_sep();
    for root in roots {
        let candidate = path_join_text(root.clone(), resource, sep);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file && !payload.is_archive_member {
            return Some(candidate);
        }
    }
    None
}
pub(super) fn importlib_validate_resource_name_text(
    _py: &PyToken<'_>,
    resource: &str,
) -> Result<(), u64> {
    if resource.is_empty() || resource == "." || resource == ".." {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            &format!(
                "'{}' must be only a file name",
                resource.replace('\'', "\\'")
            ),
        ));
    }
    if resource.contains('/') || resource.contains('\\') {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            &format!(
                "'{}' must be only a file name",
                resource.replace('\'', "\\'")
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "stdlib_archive")]
    use std::io::Write;

    fn bootstrap_module_file() -> String {
        if super::super::super::sys_platform_str().starts_with("win") {
            "C:\\repo\\src\\molt\\stdlib\\sys.py".to_string()
        } else {
            "/repo/src/molt/stdlib/sys.py".to_string()
        }
    }

    #[test]
    fn importlib_resources_path_payload_reports_entries_and_init_marker() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        std::fs::write(tmp.join("__init__.py"), "x = 1\n").expect("write __init__.py");
        std::fs::write(tmp.join("data.txt"), "payload\n").expect("write data.txt");

        let payload = importlib_resources_path_payload(&tmp.to_string_lossy());
        assert!(payload.exists);
        assert!(payload.is_dir);
        assert!(!payload.is_file);
        assert!(payload.has_init_py);
        assert!(!payload.is_archive_member);
        assert!(payload.entries.iter().any(|entry| entry == "__init__.py"));
        assert!(payload.entries.iter().any(|entry| entry == "data.txt"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[cfg(feature = "stdlib_archive")]
    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_resources_zip_payload_reports_entries_and_init_marker() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_zip_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("resources.zip");
        let file = std::fs::File::create(&archive).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("pkg/__init__.py", options)
            .expect("start __init__.py");
        writer
            .write_all(b"x = 1\n")
            .expect("write __init__.py in zip");
        writer
            .start_file("pkg/data.txt", options)
            .expect("start data.txt");
        writer
            .write_all(b"payload\n")
            .expect("write data.txt in zip");
        writer.finish().expect("finish zip archive");

        let archive_text = archive.to_string_lossy().into_owned();
        let package_root = format!("{archive_text}/pkg");
        let package_payload = importlib_resources_path_payload(&package_root);
        assert!(package_payload.exists);
        assert!(package_payload.is_dir);
        assert!(!package_payload.is_file);
        assert!(package_payload.has_init_py);
        assert!(package_payload.is_archive_member);
        assert!(
            package_payload
                .entries
                .iter()
                .any(|entry| entry == "__init__.py")
        );
        assert!(
            package_payload
                .entries
                .iter()
                .any(|entry| entry == "data.txt")
        );

        let file_payload = importlib_resources_path_payload(&format!("{package_root}/data.txt"));
        assert!(file_payload.exists);
        assert!(file_payload.is_file);
        assert!(!file_payload.is_dir);
        assert!(!file_payload.has_init_py);
        assert!(file_payload.is_archive_member);

        let package_meta = importlib_resources_package_payload(
            "pkg",
            std::slice::from_ref(&archive_text),
            Some(bootstrap_module_file()),
        );
        assert!(package_meta.has_regular_package);
        assert!(
            package_meta
                .roots
                .iter()
                .any(|entry| entry == &package_root)
        );
        assert!(
            package_meta
                .init_file
                .as_deref()
                .is_some_and(|entry| entry.ends_with("resources.zip/pkg/__init__.py"))
        );

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[cfg(feature = "stdlib_archive")]
    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_resources_whl_payload_reports_archive_member_flag() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_whl_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("resources.whl");
        let file = std::fs::File::create(&archive).expect("create whl file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("pkg/data.txt", options)
            .expect("start data.txt");
        writer
            .write_all(b"payload\n")
            .expect("write data.txt in whl");
        writer.finish().expect("finish whl archive");

        let archive_text = archive.to_string_lossy().into_owned();
        let file_payload =
            importlib_resources_path_payload(&format!("{archive_text}/pkg/data.txt"));
        assert!(file_payload.exists);
        assert!(file_payload.is_file);
        assert!(file_payload.is_archive_member);

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }
}
