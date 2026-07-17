mod contract_pipeline;
mod daemon_cache;
mod daemon_env;
mod daemon_request_io;
mod native_batch;
mod native_objects;
mod output_paths;
mod shared_stdlib;

use super::run_luau_tir_module_pipeline;
#[cfg(feature = "rust-backend")]
use super::rust_source_for_ir;
use super::validate_fact_graph_cli_contract;
use super::{
    BACKEND_DAEMON_PROTOCOL_VERSION, BackendOutputKind, DEFAULT_BACKEND_BATCH_OP_BUDGET,
    DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE, DaemonCache, DaemonJobRequest,
    DaemonRequest, GIB, MIB, NativeApplicationObjectOptions, RequestBoundedRead,
    batch_external_function_names, compile_native_application_object_to_path, compile_single_job,
    compile_stdlib_cache_object, default_backend_max_rss_gb_from_physical_mem_bytes,
    default_backend_output_path, default_daemon_cache_bytes_from_physical_mem_bytes,
    ensure_output_parent_dir, is_user_owned_symbol, merge_relocatable_objects,
    partition_functions_for_batches, preserve_native_batch_worker_failure_artifacts,
    prune_and_partition_native_stdlib, read_bounded_request_bytes, read_json_artifact,
    read_stdlib_cache_key, read_stdlib_cache_manifest, relocatable_linker_binary,
    remove_native_batch_temp_dir, resolve_backend_output_path, resolved_batch_op_budget_limit,
    resolved_batch_size_limit, shared_stdlib_cache_matches, shared_stdlib_partition_closure_issue,
    shared_stdlib_partition_manifest, stdlib_cache_count_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path, validate_shared_stdlib_partition,
    with_shared_stdlib_cache_publish_lock, write_bytes_atomically, write_cached_output,
    write_json_artifact, write_shared_stdlib_cache_sidecars,
};
#[cfg(unix)]
use super::{DaemonResponse, daemon_response_payload, read_daemon_request_bytes};
use super::{NativeBatchModuleMetadata, NativeBatchObjectJob};
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use std::io::{self, Cursor, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());
const SHARED_STDLIB_CACHE_ENV_KEYS: &[&str] = &[
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_CACHE_MANIFEST",
];

struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    snapshot: Vec<(&'static str, Option<String>)>,
}

impl TestEnvGuard {
    fn capture(keys: &'static [&'static str]) -> Self {
        let lock = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        Self {
            _lock: lock,
            snapshot,
        }
    }

    fn clear(keys: &'static [&'static str]) -> Self {
        let guard = Self::capture(keys);
        for (key, _) in &guard.snapshot {
            unsafe { std::env::remove_var(key) };
        }
        guard
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.snapshot {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

fn write_failing_relocatable_linker(tmp_dir: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    let linker = tmp_dir.join("fail-linker.cmd");
    #[cfg(not(windows))]
    let linker = tmp_dir.join("fail-linker.sh");

    #[cfg(windows)]
    std::fs::write(
        &linker,
        b"@echo off\r\necho forced relocatable link failure 1>&2\r\nexit /b 1\r\n",
    )
    .expect("write failing linker script");

    #[cfg(not(windows))]
    {
        std::fs::write(
            &linker,
            b"#!/bin/sh\necho forced relocatable link failure >&2\nexit 1\n",
        )
        .expect("write failing linker script");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&linker)
            .expect("stat failing linker script")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&linker, permissions)
            .expect("make failing linker script executable");
    }

    linker
}
