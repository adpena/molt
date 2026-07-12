use std::path::Path;

use molt_backend::SimpleIR;

use super::super::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, read_stdlib_cache_partition_manifest,
    remove_shared_stdlib_cache_artifacts, shared_stdlib_cache_matches,
    stdlib_cache_count_sidecar_path,
};
use super::request::NativeStdlibCachePrepare;

pub(crate) enum ExistingStdlibCache {
    Reused,
    MissingOrIneligible,
    Invalidated,
}

impl ExistingStdlibCache {
    pub(crate) fn reused(&self) -> bool {
        matches!(self, Self::Reused)
    }
}

pub(crate) fn try_reuse_existing_stdlib_cache(
    ir: &mut SimpleIR,
    stdlib_path: &Path,
    request: &NativeStdlibCachePrepare<'_>,
    current_partition_manifest: &str,
    user_remaining: &mut Vec<molt_backend::FunctionIR>,
    stdlib_funcs: &mut Vec<molt_backend::FunctionIR>,
) -> ExistingStdlibCache {
    if !request.have_entry_module || !stdlib_path.exists() {
        return ExistingStdlibCache::MissingOrIneligible;
    }

    let current_stdlib_count = stdlib_funcs.len();
    let count_path = stdlib_cache_count_sidecar_path(stdlib_path);
    let cached_count: usize = std::fs::read_to_string(&count_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if shared_stdlib_cache_matches(
        stdlib_path,
        request.expected_cache_key,
        request.expected_cache_manifest,
        Some(current_partition_manifest),
    ) {
        let mut retained = std::mem::take(user_remaining);
        let mut extern_count = 0usize;
        for mut func in std::mem::take(stdlib_funcs) {
            molt_backend::externalize_function_with_signature(&mut func);
            extern_count += 1;
            retained.push(func);
        }
        let user_count = retained.len().saturating_sub(extern_count);
        ir.functions = retained;
        eprintln!(
            "{}: incremental -- compiling {user_count} user functions \
             ({extern_count} stdlib extern from {})",
            request.log_prefix,
            stdlib_path.display()
        );
        return ExistingStdlibCache::Reused;
    }

    let cached_key = read_stdlib_cache_key(stdlib_path);
    let cached_manifest = read_stdlib_cache_manifest(stdlib_path);
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path);
    eprintln!(
        "{}: stdlib cache contract mismatch \
         (cached key {}, expected key {}; cached manifest {}, expected manifest present {}; \
         cached partition manifest present {}, expected partition manifest present true; \
         cached {} functions, need {}) -- rebuilding",
        request.log_prefix,
        cached_key.as_deref().unwrap_or("<missing>"),
        request.expected_cache_key.unwrap_or("<missing>"),
        cached_manifest.as_deref().unwrap_or("<missing>"),
        request.expected_cache_manifest.is_some(),
        cached_partition_manifest.is_some(),
        cached_count,
        current_stdlib_count,
    );
    remove_shared_stdlib_cache_artifacts(stdlib_path);
    ExistingStdlibCache::Invalidated
}
