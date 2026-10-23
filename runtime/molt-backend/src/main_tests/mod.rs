mod contract_pipeline;
#[cfg(molt_backend_daemon)]
mod daemon_cache;
#[cfg(molt_backend_daemon)]
mod daemon_env;
#[cfg(all(molt_backend_daemon, any(unix, windows)))]
mod daemon_environment_isolation;
#[cfg(molt_backend_daemon)]
mod daemon_request_io;
#[cfg(feature = "native-backend")]
mod native_batch;
#[cfg(feature = "native-backend")]
mod native_objects;
mod output_paths;
mod request_limits;
#[cfg(feature = "native-backend")]
mod shared_stdlib;

#[cfg(feature = "wasm-backend")]
use super::WasmTargetOptions;
use super::run_luau_tir_module_pipeline;
#[cfg(feature = "rust-backend")]
use super::rust_source_for_ir;
#[cfg(molt_backend_daemon)]
use super::{
    BACKEND_DAEMON_PROTOCOL_VERSION, DaemonCache, DaemonIrSource, DaemonJobRequest,
    DaemonJobResponse, DaemonJobTarget, DaemonLru, DaemonRequest, DaemonResponse, MIB,
    compile_single_job, daemon_response_payload,
    default_daemon_cache_bytes_from_physical_mem_bytes, read_daemon_request_bytes,
    write_cached_output,
};
use super::{
    BackendTarget, RequestBoundedRead, ensure_output_parent_dir, read_bounded_request_bytes,
};
#[cfg(feature = "native-backend")]
use super::{
    DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE, DEFAULT_STDLIB_BATCH_SIZE,
    NativeApplicationArtifactOptions, NativeBatchObjectJob,
    append_referenced_external_declarations, batch_external_function_names,
    compile_native_application_artifact_to_path, external_function_declarations,
    is_user_owned_symbol, partition_functions_for_batches, plan_stdlib_cache_archive,
    preserve_native_batch_worker_failure_artifacts, prune_and_partition_native_stdlib,
    read_json_artifact, read_stdlib_cache_key, read_stdlib_cache_manifest,
    remove_native_batch_temp_dir, resolved_batch_op_budget_limit, resolved_batch_size_limit,
    shared_stdlib_cache_matches, shared_stdlib_partition_closure_issue,
    shared_stdlib_partition_manifest, stdlib_cache_count_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path, validate_shared_stdlib_partition,
    with_shared_stdlib_cache_publish_lock, write_shared_stdlib_cache_sidecars,
};
#[cfg(feature = "native-backend")]
use super::{NativeArtifactKind, NativeTargetOptions};
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use std::io::{self, Cursor, Read};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Publish fixture IR inside the caller's existing artifact root and lifetime.
#[cfg(molt_backend_daemon)]
fn write_daemon_ir_fixture(path: &std::path::Path, ir: &impl serde::Serialize) -> String {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create MessagePack IR fixture");
    rmp_serde::encode::write_named(&mut file, ir).expect("encode MessagePack IR fixture");
    path.to_str().expect("Unicode fixture path").to_owned()
}

/// Every variable a daemon request carries, for tests that isolate them.
#[cfg(molt_backend_daemon)]
static DAEMON_REQUEST_ENV_KEYS: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| {
        molt_ir::backend_environment::daemon_request_env_keys()
            .map(molt_ir::backend_environment::BackendEnv::name)
            .collect()
    });

struct TestEnvGuard {
    // Fields drop in declaration order: restore while the mutex is still held.
    _snapshot: TestEnvSnapshot,
    _lock: std::sync::MutexGuard<'static, ()>,
}

struct TestEnvSnapshot(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl TestEnvSnapshot {
    fn capture(keys: &'static [&'static str]) -> Self {
        Self(
            keys.iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect(),
        )
    }
}

impl Drop for TestEnvSnapshot {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

impl TestEnvGuard {
    fn capture(keys: &'static [&'static str]) -> Self {
        let lock = ENV_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = TestEnvSnapshot::capture(keys);
        Self {
            _lock: lock,
            _snapshot: snapshot,
        }
    }

    #[cfg(any(molt_backend_daemon, feature = "native-backend"))]
    fn clear(keys: &'static [&'static str]) -> Self {
        let guard = Self::capture(keys);
        for key in keys {
            unsafe { std::env::remove_var(key) };
        }
        guard
    }
}

#[test]
#[cfg(any(unix, windows))]
fn environment_fixture_restores_absent_empty_unicode_and_non_unicode_values() {
    const KEY: &str = "MOLT_TEST_ENV_SNAPSHOT_CUSTODY";
    let _guard = TestEnvGuard::capture(&[KEY]);
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800])
    };
    for value in [
        None,
        Some("".into()),
        Some("lambda-\u{03bb}".into()),
        Some(invalid),
    ] {
        match &value {
            Some(value) => unsafe { std::env::set_var(KEY, value) },
            None => unsafe { std::env::remove_var(KEY) },
        }
        {
            let _snapshot = TestEnvSnapshot::capture(&[KEY]);
            unsafe { std::env::set_var(KEY, "changed") };
        }
        assert_eq!(std::env::var_os(KEY), value);
    }
}

#[cfg(feature = "native-backend")]
fn assert_native_archive_members(bytes: &[u8], expected: usize) {
    use object::Object;
    let archive = object::read::archive::ArchiveFile::parse(bytes).expect("parse native archive");
    let mut count = 0;
    for member in archive.members() {
        let member = member.expect("read archive member");
        let object = object::File::parse(member.data(bytes).expect("read member bytes"))
            .expect("parse ordinary member object");
        assert_eq!(object.kind(), object::ObjectKind::Relocatable);
        count += 1;
    }
    assert_eq!(count, expected);
}
