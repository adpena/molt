use super::*;

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_count_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("count")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_key_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("key")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_manifest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("manifest.json")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_partition_manifest_sidecar_path(
    stdlib_path: &Path,
) -> std::path::PathBuf {
    stdlib_path.with_extension("partition.json")
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_object_digest_sidecar_path(stdlib_path: &Path) -> std::path::PathBuf {
    stdlib_path.with_extension("sha256")
}

#[cfg(feature = "native-backend")]
pub(crate) fn sha256_file_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = hasher.finalize();
    let bytes: &[u8] = digest.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_publish_lock_path(stdlib_path: &Path) -> PathBuf {
    stdlib_path.with_file_name(format!(
        "{}.publish.lock",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared")
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_cache_temp_publish_path(stdlib_path: &Path, label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    stdlib_path.with_file_name(format!(
        ".{}.{}.{}.{}.tmp",
        stdlib_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stdlib_shared"),
        std::process::id(),
        stamp,
        label,
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn atomic_replace_file(temp_path: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if final_path.exists() {
        let _ = std::fs::remove_file(final_path);
    }
    std::fs::rename(temp_path, final_path)
}

#[cfg(feature = "native-backend")]
pub(crate) fn sync_published_file(path: &Path) -> io::Result<()> {
    File::options()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

#[cfg(feature = "native-backend")]
pub(crate) fn write_atomic_text_file(path: &Path, contents: &str) -> io::Result<()> {
    ensure_output_parent_dir(path.to_str().unwrap_or_default())?;
    let temp_path = stdlib_cache_temp_publish_path(path, "text");
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    sync_published_file(path)?;
    Ok(())
}

#[cfg(all(feature = "native-backend", unix))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let lock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if lock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if unlock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", windows))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let mut overlapped = OVERLAPPED::default();
    let lock_rc = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if lock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    if unlock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(all(feature = "native-backend", not(any(unix, windows))))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    _stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    body()
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_key_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "native-backend")]
pub(crate) fn remove_shared_stdlib_cache_artifacts(stdlib_path: &Path) {
    let _ = std::fs::remove_file(stdlib_path);
    let _ = std::fs::remove_file(stdlib_cache_count_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_key_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(stdlib_path));
    let _ = std::fs::remove_file(stdlib_cache_object_digest_sidecar_path(stdlib_path));
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_cache_matches(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    let Some(expected_key) = expected_key.filter(|key| !key.is_empty()) else {
        return false;
    };
    let Some(expected_manifest) = expected_manifest.filter(|manifest| !manifest.is_empty()) else {
        return false;
    };
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return false;
    }
    let Ok(actual_object_digest) = sha256_file_hex(stdlib_path) else {
        return false;
    };
    let Ok(cached_object_digest) =
        std::fs::read_to_string(stdlib_cache_object_digest_sidecar_path(stdlib_path))
    else {
        return false;
    };
    if cached_object_digest.trim() != actual_object_digest {
        return false;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
    {
        return cached_partition_manifest.as_deref() == Some(expected_partition_manifest);
    }
    cached_partition_manifest.is_some()
}

#[cfg(feature = "native-backend")]
pub(crate) fn write_shared_stdlib_cache_sidecars(
    stdlib_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
    write_atomic_text_file(&count_path, &stdlib_count.to_string())?;

    let key_path = stdlib_cache_key_sidecar_path(stdlib_path);
    if let Some(cache_key) = cache_key.filter(|key| !key.is_empty()) {
        write_atomic_text_file(&key_path, cache_key)?;
    } else {
        match std::fs::remove_file(&key_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    let manifest_path = stdlib_cache_manifest_sidecar_path(stdlib_path);
    if let Some(cache_manifest) = cache_manifest.filter(|manifest| !manifest.is_empty()) {
        write_atomic_text_file(&manifest_path, cache_manifest)?;
    } else {
        match std::fs::remove_file(&manifest_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    write_atomic_text_file(
        &stdlib_cache_partition_manifest_sidecar_path(stdlib_path),
        partition_manifest,
    )?;
    let object_digest = sha256_file_hex(stdlib_path)?;
    write_atomic_text_file(
        &stdlib_cache_object_digest_sidecar_path(stdlib_path),
        &object_digest,
    )?;
    Ok(())
}

#[cfg(feature = "native-backend")]
pub(crate) fn publish_shared_stdlib_cache_object(
    stdlib_path: &Path,
    temp_object_path: &Path,
    stdlib_count: usize,
    cache_key: Option<&str>,
    cache_manifest: Option<&str>,
    partition_manifest: &str,
) -> io::Result<()> {
    let result = with_shared_stdlib_cache_publish_lock(stdlib_path, || {
        if let Err(err) = atomic_replace_file(temp_object_path, stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = sync_published_file(stdlib_path) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        if let Err(err) = write_shared_stdlib_cache_sidecars(
            stdlib_path,
            stdlib_count,
            cache_key,
            cache_manifest,
            partition_manifest,
        ) {
            remove_shared_stdlib_cache_artifacts(stdlib_path);
            return Err(err);
        }
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temp_object_path);
    }
    result
}
