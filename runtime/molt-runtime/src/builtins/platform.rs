use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::Value as JsonValue;

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_capability_decision, audit_emit};
use crate::builtins::exceptions::molt_exception_last_pending;
use crate::builtins::modules::{ExecutionMetadata, execute_compiled_module, sys_modules_dict_bits};
use crate::object::ops_sys::runtime_target_minor;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::*;
pub(crate) use molt_runtime_platform::env_support::{
    locale_encoding, os_name_str, process_env_set, process_env_snapshot, set_locale,
    sys_platform_str,
};
pub(crate) use molt_runtime_platform::importlib_support::{
    append_unique_path, append_unique_path_hashed, split_nonempty_paths,
};
use molt_runtime_platform::path_text::{
    path_basename_text, path_dirname_text, path_join_many_text, path_join_text, path_normpath_text,
};
use molt_runtime_platform::uuid_support::{
    uuid_node, uuid_v1_bytes, uuid_v3_bytes, uuid_v4_bytes, uuid_v5_bytes,
};

// --- Platform constants ---

// I/O readiness events shared by the networking I/O driver and `select`.
#[cfg(any(molt_net, feature = "stdlib_select"))]
pub(crate) const IO_EVENT_READ: u32 = 1;
#[cfg(any(molt_net, feature = "stdlib_select"))]
pub(crate) const IO_EVENT_WRITE: u32 = 1 << 1;
#[cfg(any(molt_net, feature = "stdlib_select"))]
pub(crate) const IO_EVENT_ERROR: u32 = 1 << 2;

// --- errno/socket/env helpers ---

const PLATFORM_OBJECT_SLOT_COUNT: usize = 4;

pub(crate) struct PlatformRuntimeState {
    encoding: OnceLock<molt_runtime_platform::env_support::StartupEncoding>,
    startup_environment: OnceLock<StartupEnvironment>,
    // 0: uninitialized; true: initializing; None: terminal retirement.
    environment_mapping: AtomicU64,
    errno_constants_cache: AtomicU64,
    socket_constants_cache: AtomicU64,
    os_name_cache: AtomicU64,
    sys_platform_cache: AtomicU64,
    import_lock: crate::concurrency::MoltRLock,
    import_defaults: Mutex<DefaultFinderGeneration>,
}

/// Only these exact initial list generations belong to interpreter bootstrap.
/// Strong owners prevent address reuse; user mutation invalidates their epochs.
#[derive(Clone, Copy)]
struct PendingDefaultFinders {
    meta: u64,
    meta_epoch: u64,
    hooks: u64,
    hooks_epoch: u64,
}

enum DefaultFinderGeneration {
    Uninitialized,
    Pending(PendingDefaultFinders),
    Loading(PendingDefaultFinders),
    Complete,
    Cancelled,
    Failed,
    Retired,
}

/// Scoped participation in the same physical reentrant lock exposed by _imp.
struct ImportLockGuard<'a> {
    lock: Option<&'a crate::concurrency::MoltRLock>,
}
impl ImportLockGuard<'_> {
    fn acquire(py: &PyToken<'_>) -> Result<Self, u64> {
        importlib_acquire_import_lock(py)?;
        Ok(Self {
            lock: Some(&platform_state(py).import_lock),
        })
    }
    fn finish(mut self, py: &PyToken<'_>) -> Result<(), u64> {
        if self.lock.take().unwrap().release() {
            return Ok(());
        }
        use crate::builtins::exceptions::{
            ExceptionCleanupPolicy, finish_exception_cleanup, take_raised,
        };
        let saved = exception_pending(py).then(|| take_raised(py));
        raise_exception::<()>(py, "RuntimeError", "not holding the import lock");
        finish_exception_cleanup(py, saved, ExceptionCleanupPolicy::Chain);
        Err(MoltObject::none().bits())
    }
}
impl Drop for ImportLockGuard<'_> {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
    }
}

#[cfg(test)]
pub(crate) struct ImportDefaultsTestSnapshot(DefaultFinderGeneration);
#[cfg(test)]
impl ImportDefaultsTestSnapshot {
    pub(crate) fn detach(py: &PyToken<'_>) -> Self {
        let mut defaults = platform_state(py).import_defaults.lock().unwrap();
        Self(std::mem::replace(
            &mut *defaults,
            DefaultFinderGeneration::Uninitialized,
        ))
    }
    pub(crate) fn restore(self, py: &PyToken<'_>) {
        let fresh = {
            let mut defaults = platform_state(py).import_defaults.lock().unwrap();
            std::mem::replace(&mut *defaults, self.0)
        };
        release_default_owners(py, pending_default_owners(fresh));
    }
}

impl PlatformRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            encoding: OnceLock::new(),
            startup_environment: OnceLock::new(),
            environment_mapping: AtomicU64::new(0),
            errno_constants_cache: AtomicU64::new(0),
            socket_constants_cache: AtomicU64::new(0),
            os_name_cache: AtomicU64::new(0),
            sys_platform_cache: AtomicU64::new(0),
            import_lock: crate::concurrency::MoltRLock::new(),
            import_defaults: Mutex::new(DefaultFinderGeneration::Uninitialized),
        }
    }

    fn object_slots(&self) -> [&AtomicU64; PLATFORM_OBJECT_SLOT_COUNT] {
        [
            &self.errno_constants_cache,
            &self.socket_constants_cache,
            &self.os_name_cache,
            &self.sys_platform_cache,
        ]
    }
}

fn platform_state(_py: &PyToken<'_>) -> &'static PlatformRuntimeState {
    &runtime_state(_py).platform
}

pub(crate) fn startup_encoding<'py>(
    _py: &'py PyToken<'_>,
) -> &'py molt_runtime_platform::env_support::StartupEncoding {
    platform_state(_py)
        .encoding
        .get()
        .expect("startup encoding precedes Python execution")
}

pub(crate) fn initialize_startup_encoding(state: &crate::state::RuntimeState) {
    let mut policy =
        molt_runtime_platform::env_support::startup_encoding().unwrap_or_else(|error| {
            eprintln!("Fatal Python error: startup encoding: {error}");
            std::process::exit(1);
        });
    // Configuration selects actual encodings. Resolve their codec identity
    // once before publication, never independently in sys or path consumers.
    for label in [&mut policy.filesystem_encoding, &mut policy.stdio_encoding] {
        let Some(codec) = molt_stdlib_text::codec_registry::normalize_encoding(label) else {
            eprintln!("Fatal Python error: startup encoding: unknown encoding: {label}");
            std::process::exit(1);
        };
        if codec.text_class().is_none() {
            eprintln!("Fatal Python error: startup encoding: {label} is not a text encoding");
            std::process::exit(1);
        }
    }
    // CPython canonicalizes the filesystem codec name; stdio retains the
    // selected spelling for TextIOWrapper.encoding.
    policy.filesystem_encoding =
        molt_stdlib_text::codec_registry::normalize_encoding(&policy.filesystem_encoding)
            .expect("validated filesystem codec")
            .name()
            .to_owned();
    assert!(
        state.platform.encoding.set(policy).is_ok(),
        "startup encoding published twice"
    );
}

#[derive(Clone)]
struct StartupEnvironment {
    module_roots: String,
    pwd: String,
    entry_is_spawn: bool,
    mp_spawn_enabled: bool,
    executable: Option<Vec<u8>>,
}

pub(crate) fn initialize_startup_environment(state: &crate::state::RuntimeState) {
    // Selected bootstrap controls are immutable startup facts, not a second
    // environment mapping. No full process snapshot is needed at startup.
    let startup = StartupEnvironment {
        module_roots: std::env::var("MOLT_MODULE_ROOTS").unwrap_or_default(),
        pwd: resolve_bootstrap_pwd(&std::env::var("PWD").unwrap_or_default()),
        entry_is_spawn: std::env::var("MOLT_ENTRY_MODULE").as_deref()
            == Ok("multiprocessing.spawn"),
        mp_spawn_enabled: runtime_flags::enabled(RuntimeFlag::MOLT_MP_SPAWN),
        executable: std::env::var("MOLT_SYS_EXECUTABLE")
            .ok()
            .filter(|value| !value.is_empty())
            .map(String::into_bytes),
    };
    assert!(state.platform.startup_environment.set(startup).is_ok());
}

pub(crate) fn inherit_startup_state(
    child: &crate::state::RuntimeState,
    primary: &crate::state::RuntimeState,
) {
    assert!(
        child
            .platform
            .encoding
            .set(
                primary
                    .platform
                    .encoding
                    .get()
                    .expect("primary startup encoding")
                    .clone()
            )
            .is_ok()
    );
    assert!(
        child
            .platform
            .startup_environment
            .set(
                primary
                    .platform
                    .startup_environment
                    .get()
                    .expect("primary startup controls")
                    .clone()
            )
            .is_ok()
    );
    // Each interpreter owns a separate, still-uninitialized Python mapping.
}

pub(crate) fn startup_executable(py: &PyToken<'_>) -> Option<Vec<u8>> {
    platform_state(py)
        .startup_environment
        .get()
        .expect("startup controls")
        .executable
        .clone()
}

pub(crate) fn startup_sys_path(py: &PyToken<'_>) -> Vec<String> {
    sys_bootstrap_state_from_module_file(py, None).path
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_startup_entry_is_spawn() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MoltObject::from_bool(
            platform_state(py)
                .startup_environment
                .get()
                .expect("startup controls")
                .entry_is_spawn,
        )
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_startup_mp_spawn_enabled() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MoltObject::from_bool(
            platform_state(py)
                .startup_environment
                .get()
                .expect("startup controls")
                .mp_spawn_enabled,
        )
        .bits()
    })
}

fn init_platform_cached_owned_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    init: impl FnOnce() -> u64,
) -> u64 {
    let bits = init_atomic_bits(_py, slot, init);
    inc_ref_bits(_py, bits);
    bits
}

pub(crate) fn platform_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let slots = state.platform.object_slots();
    // Publish terminal retirement before releasing Python objects. Finalizer
    // reentry must never recreate the mapping or mutate the process environment.
    let previous = state
        .platform
        .environment_mapping
        .swap(MoltObject::none().bits(), Ordering::AcqRel);
    let rooted = previous != 0
        && previous != MoltObject::none().bits()
        && previous != MoltObject::from_bool(true).bits();
    let defaults = importlib_take_default_generation(state);
    let default_roots = defaults.is_some();
    // Both terminal dispositions are visible before either cohort can run a
    // Python finalizer. No bootstrap/mapping state can be recreated from it.
    release_default_owners(_py, defaults);
    if rooted {
        dec_ref_bits(_py, previous);
    }
    crate::state::cache::clear_atomic_slots(_py, &slots) || rooted || default_roots
}

static EXTENSION_METADATA_OK_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
static SOURCE_EXTENSION_LIBRARIES: OnceLock<Mutex<Vec<libloading::Library>>> = OnceLock::new();
static EXTENSION_METADATA_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_METADATA_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

fn extension_metadata_ok_cache() -> &'static Mutex<BTreeMap<String, String>> {
    EXTENSION_METADATA_OK_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn extension_metadata_cache_stats() -> (u64, u64) {
    (
        EXTENSION_METADATA_CACHE_HITS.load(Ordering::Relaxed),
        EXTENSION_METADATA_CACHE_MISSES.load(Ordering::Relaxed),
    )
}

fn bootstrap_stdlib_root_from_module_file(module_file: &str, path_sep: char) -> Option<String> {
    if module_file.is_empty() {
        return None;
    }
    let module_dir = path_dirname_text(module_file, path_sep);
    if module_dir.is_empty() {
        return None;
    }
    let mut current = module_dir.clone();
    loop {
        if path_basename_text(&current, path_sep) == "stdlib" {
            let parent = path_dirname_text(&current, path_sep);
            if path_basename_text(&parent, path_sep) == "molt" {
                return Some(current);
            }
        }
        let parent = path_dirname_text(&current, path_sep);
        if parent.is_empty() || parent == current {
            break;
        }
        current = parent;
    }
    Some(module_dir)
}

fn resolve_bootstrap_pwd(raw_pwd: &str) -> String {
    if !raw_pwd.is_empty() {
        return raw_pwd.to_string();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(cwd) = std::env::current_dir() {
            let text = cwd.to_string_lossy().into_owned();
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

struct SysBootstrapState {
    path: Vec<String>,
    stdlib_root: Option<String>,
    pythonpath_entries: Vec<String>,
    module_roots_entries: Vec<String>,
    venv_site_packages_entries: Vec<String>,
    pwd: String,
    include_cwd: bool,
}

fn sys_bootstrap_state_from_module_file(
    py: &PyToken<'_>,
    module_file: Option<String>,
) -> SysBootstrapState {
    let startup = platform_state(py)
        .startup_environment
        .get()
        .expect("startup controls");
    let module_roots_raw = &startup.module_roots;
    let windows_paths = sys_platform_str().starts_with("win");
    let sep = if windows_paths { ';' } else { ':' };
    let path_sep = if windows_paths { '\\' } else { '/' };
    let pwd = startup.pwd.clone();
    let pythonpath_entries: Vec<String> = Vec::new();
    let mut paths: Vec<String> = pythonpath_entries.clone();
    let mut paths_seen: HashSet<String> = pythonpath_entries.iter().cloned().collect();

    let stdlib_root = module_file.and_then(|path| {
        bootstrap_stdlib_root_from_module_file(&path, path_sep)
            .map(|root| bootstrap_resolve_path_entry(&root, &pwd, path_sep))
    });
    if let Some(root) = &stdlib_root {
        append_unique_path_hashed(&mut paths, &mut paths_seen, root);
    }

    let mut module_roots_entries: Vec<String> = Vec::new();
    let mut module_roots_seen: HashSet<String> = HashSet::new();
    for entry in split_nonempty_paths(module_roots_raw, sep) {
        let resolved = bootstrap_resolve_path_entry(&entry, &pwd, path_sep);
        append_unique_path_hashed(&mut module_roots_entries, &mut module_roots_seen, &resolved);
        append_unique_path_hashed(&mut paths, &mut paths_seen, &resolved);
    }

    let venv_site_packages_entries: Vec<String> = Vec::new();

    // The working directory joins sys.path only at the full capability tier;
    // the default tier runs untrusted, as CI does.
    let include_cwd = is_trusted(py);
    if include_cwd && !paths.iter().any(|entry| entry.is_empty()) {
        paths.insert(0, String::new());
    }

    SysBootstrapState {
        path: paths,
        stdlib_root,
        pythonpath_entries,
        module_roots_entries,
        venv_site_packages_entries,
        pwd,
        include_cwd,
    }
}

fn pending_exception_matches_any(_py: &PyToken<'_>, kinds: &[&str]) -> bool {
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last_pending();
    let matches = kinds.iter().any(|kind| {
        crate::builtins::exceptions::exception_matches_builtin_name(_py, exc_bits, kind)
    });
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    matches
}

pub(super) fn clear_pending_if_kind(_py: &PyToken<'_>, kinds: &[&str]) -> bool {
    if pending_exception_matches_any(_py, kinds) {
        clear_exception(_py);
        return true;
    }
    false
}

fn call_callable_positional(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args: &[u64],
) -> Result<u64, u64> {
    // Use direct call entrypoints for common arities to preserve callable
    // binding semantics and returned objects exactly.
    match args.len() {
        0 => return Ok(unsafe { call_callable0(_py, callable_bits) }),
        1 => return Ok(unsafe { call_callable1(_py, callable_bits, args[0]) }),
        2 => return Ok(unsafe { call_callable2(_py, callable_bits, args[0], args[1]) }),
        3 => return Ok(unsafe { call_callable3(_py, callable_bits, args[0], args[1], args[2]) }),
        4 => {}
        _ => {}
    }
    let builder_bits = molt_callargs_new(args.len() as u64, 0);
    if builder_bits == 0 {
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    for &arg in args {
        let _ = unsafe { molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(molt_call_bind(callable_bits, builder_bits))
}

fn importlib_extension_exec_unavailable(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    kind: &str,
) -> u64 {
    raise_exception::<u64>(
        _py,
        "ImportError",
        &format!(
            "{kind} module {module_name:?} at {path:?} has no compiler-emitted body in this binary"
        ),
    )
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_libraries() -> &'static Mutex<Vec<libloading::Library>> {
    SOURCE_EXTENSION_LIBRARIES.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_merge_module_dict(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_bits: u64,
) -> Result<(), String> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        return Err("PyInit returned an invalid module handle".to_string());
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err("PyInit returned a non-module object".to_string());
    }
    let module_dict_bits = unsafe { crate::object::layout::module_dict_bits(module_ptr) };
    let Some(dict_ptr) = obj_from_bits(module_dict_bits).as_ptr() else {
        return Err("PyInit module has no dictionary".to_string());
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err("PyInit module dictionary is not a dict".to_string());
    }
    unsafe {
        crate::object::ops_dict::dict_update_apply(
            _py,
            MoltObject::from_ptr(namespace_ptr).bits(),
            crate::object::ops_dict::dict_update_set_in_place,
            module_dict_bits,
        );
    }
    if exception_pending(_py) {
        clear_exception(_py);
        return Err("module dictionary merge raised".to_string());
    }
    Ok(())
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_install_sys_module(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<(), String> {
    let modules_bits = match importlib_runtime_modules_bits(_py) {
        Ok(bits) => bits,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err("failed to access sys.modules".to_string());
        }
    };
    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        return Err("sys.modules is not a dict".to_string());
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        return Err("sys.modules is not a dict".to_string());
    }
    let name_bits = match alloc_str_bits(_py, module_name) {
        Ok(bits) => bits,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err("failed to allocate sys.modules key".to_string());
        }
    };
    let set_result = importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits);
    if !obj_from_bits(name_bits).is_none() {
        dec_ref_bits(_py, name_bits);
    }
    if set_result.is_err() {
        if exception_pending(_py) {
            clear_exception(_py);
        }
        return Err("failed to install source extension in sys.modules".to_string());
    }
    Ok(())
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_loader_dlopen(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_name: &str,
    path: &str,
    init_symbol: &str,
) -> Result<(), String> {
    type InitFn = unsafe extern "C" fn() -> *mut u8;
    let library = unsafe { libloading::Library::new(Path::new(path)) }
        .map_err(|err| format!("dlopen failed: {err}"))?;
    let module_ptr = {
        let init_fn: libloading::Symbol<'_, InitFn> = unsafe {
            library
                .get(init_symbol.as_bytes())
                .map_err(|err| format!("missing init symbol {init_symbol}: {err}"))?
        };
        unsafe { init_fn() }
    };
    if exception_pending(_py) {
        clear_exception(_py);
        return Err(format!("{init_symbol} raised during initialization"));
    }
    if module_ptr.is_null() {
        return Err(format!("{init_symbol} returned NULL"));
    }
    let module_bits = module_ptr as usize as u64;
    source_extension_merge_module_dict(_py, namespace_ptr, module_bits)?;
    source_extension_install_sys_module(_py, module_name, module_bits)?;
    let mut guard = source_extension_libraries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.push(library);
    Ok(())
}

fn importlib_exec_compiled_module(
    _py: &PyToken<'_>,
    target_module_bits: u64,
    module_name: &str,
) -> Result<(), u64> {
    let module_bits = match execute_compiled_module(
        _py,
        module_name,
        module_name,
        None,
        false,
        ExecutionMetadata::LoaderNamespace {
            module_bits: target_module_bits,
        },
    ) {
        Ok(bits) => bits,
        Err(err) => return Err(err.into_import_error(_py, module_name)),
    };
    let result = if module_bits == target_module_bits {
        Ok(())
    } else {
        Err(raise_exception::<_>(
            _py,
            "SystemError",
            "compiled loader execution did not preserve module identity",
        ))
    };
    dec_ref_bits(_py, module_bits);
    result
}

/// Execute a compiler-admitted body without translating a catalog miss into
/// an ImportError.  Extension loading is the sole caller with a real dynamic
/// fallback; every admitted-body exception is propagated unchanged.
fn importlib_try_exec_compiled_module(
    _py: &PyToken<'_>,
    target_module_bits: u64,
    module_name: &str,
) -> Result<bool, u64> {
    let module_bits = match execute_compiled_module(
        _py,
        module_name,
        module_name,
        None,
        false,
        ExecutionMetadata::LoaderNamespace {
            module_bits: target_module_bits,
        },
    ) {
        Ok(bits) => bits,
        Err(err) if err.is_missing_compiled_body() => {
            err.discard_missing_compiled_body(_py);
            return Ok(false);
        }
        Err(err) => return Err(err.into_bits()),
    };
    let result = if module_bits == target_module_bits {
        Ok(true)
    } else {
        Err(raise_exception::<_>(
            _py,
            "SystemError",
            "compiled loader execution did not preserve module identity",
        ))
    };
    dec_ref_bits(_py, module_bits);
    result
}

fn linecache_loader_get_source_impl(
    _py: &PyToken<'_>,
    loader_bits: u64,
    module_name: &str,
) -> Result<Option<String>, u64> {
    let get_source_name = intern_runtime_static_name(_py, b"get_source");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, loader_bits, get_source_name)?
    else {
        return Ok(None);
    };
    let module_name_bits = alloc_str_bits(_py, module_name)?;
    let value_bits = unsafe { call_callable1(_py, call_bits, module_name_bits) };
    dec_ref_bits(_py, module_name_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        if clear_pending_if_kind(_py, &["ImportError", "OSError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "loader.get_source() must return str or None",
        ));
    };
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(Some(text))
}

fn importlib_reader_lookup_callable(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(target_bits, name_bits, missing);
    if exception_pending(_py) {
        if clear_pending_if_kind(_py, &["AttributeError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, attr_bits) {
        return Ok(None);
    }
    let callable_bits = molt_is_callable(attr_bits);
    let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
    if !obj_from_bits(callable_bits).is_none() {
        dec_ref_bits(_py, callable_bits);
    }
    if !is_callable {
        if !obj_from_bits(attr_bits).is_none() {
            dec_ref_bits(_py, attr_bits);
        }
        return Ok(None);
    }
    Ok(Some(attr_bits))
}

pub(crate) fn getattr_optional_bits(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    // The caller already owns the execution lease and token. Use the same
    // attribute authority as the public entrypoint without two nested entries.
    let result = crate::builtins::attributes::get_attr_name(_py, target_bits, name_bits);
    if exception_pending(_py) {
        if let Ok(bits) = result {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, bits));
        }
        if clear_pending_if_kind(_py, &["AttributeError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    let attr_bits = result.map_err(|()| MoltObject::none().bits())?;
    if is_missing_bits(_py, attr_bits) {
        return Ok(None);
    }
    Ok(Some(attr_bits))
}

fn importlib_module_spec_is_package_bits(_py: &PyToken<'_>, module_bits: u64) -> Result<bool, u64> {
    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? else {
        return Ok(false);
    };
    let submodule_search_locations_name = intern_static_name(
        _py,
        runtime_static_name_slot(_py, b"submodule_search_locations"),
        b"submodule_search_locations",
    );
    let locations_bits = getattr_optional_bits(_py, spec_bits, submodule_search_locations_name)?;
    if !obj_from_bits(spec_bits).is_none() {
        dec_ref_bits(_py, spec_bits);
    }
    let Some(locations_bits) = locations_bits else {
        return Ok(false);
    };
    let is_package = !obj_from_bits(locations_bits).is_none();
    if !obj_from_bits(locations_bits).is_none() {
        dec_ref_bits(_py, locations_bits);
    }
    Ok(is_package)
}

fn traceback_exception_suppress_context_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
) -> Result<bool, u64> {
    if obj_from_bits(value_bits).is_none() {
        return Ok(false);
    }
    let suppress_context_name = intern_runtime_static_name(_py, b"__suppress_context__");
    let Some(suppress_bits) = getattr_optional_bits(_py, value_bits, suppress_context_name)? else {
        return Ok(false);
    };
    let out = is_truthy(_py, obj_from_bits(suppress_bits));
    if !obj_from_bits(suppress_bits).is_none() {
        dec_ref_bits(_py, suppress_bits);
    }
    Ok(out)
}

#[cfg(test)]
fn bootstrap_resolve_abspath(py: &PyToken<'_>, path: &str, module_file: Option<String>) -> String {
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(py, module_file);
    let joined = if path_is_absolute_text(path, sep) || state.pwd.is_empty() {
        path.to_string()
    } else {
        path_join_text(state.pwd, path, sep)
    };
    path_normpath_text(&joined, sep)
}

fn module_file_from_bits(_py: &PyToken<'_>, module_file_bits: u64) -> Result<Option<String>, u64> {
    if obj_from_bits(module_file_bits).is_none() {
        return Ok(None);
    }
    match string_obj_to_owned(obj_from_bits(module_file_bits)) {
        Some(value) => Ok(Some(value)),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module file must be str or None",
        )),
    }
}

fn alloc_string_list_bits(_py: &PyToken<'_>, values: &[String]) -> Option<u64> {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        bits_vec.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, bits_vec.as_slice());
    for bits in bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn alloc_string_pairs_dict_bits(_py: &PyToken<'_>, values: &[(String, String)]) -> Option<u64> {
    let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
    let mut owned: Vec<u64> = Vec::with_capacity(values.len() * 2);
    for (key, value) in values {
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        pairs.push(key_bits);
        pairs.push(value_bits);
        owned.push(key_bits);
        owned.push(value_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    for bits in owned {
        dec_ref_bits(_py, bits);
    }
    if dict_ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(dict_ptr).bits())
    }
}

fn alloc_string_triplets_list_bits(
    _py: &PyToken<'_>,
    values: &[(String, String, String)],
) -> Option<u64> {
    let mut tuple_bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for (name, value, group) in values {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let group_ptr = alloc_string(_py, group.as_bytes());
        if group_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, value_bits);
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let group_bits = MoltObject::from_ptr(group_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[name_bits, value_bits, group_bits]);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, value_bits);
        dec_ref_bits(_py, group_bits);
        if tuple_ptr.is_null() {
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        tuple_bits_vec.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, tuple_bits_vec.as_slice());
    for bits in tuple_bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn alloc_str_bits(_py: &PyToken<'_>, value: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn importlib_loader_resolution_payload_bits(
    _py: &PyToken<'_>,
    resolution: &SourceLoaderResolution,
) -> Result<u64, u64> {
    let module_package_bits = alloc_str_bits(_py, &resolution.module_package)?;
    let package_root_bits = match resolution.package_root.as_deref() {
        Some(root) => match alloc_str_bits(_py, root) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, module_package_bits);
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    let is_package_bits = MoltObject::from_bool(resolution.is_package).bits();

    let keys_and_values: [(&[u8], u64); 3] = [
        (b"is_package", is_package_bits),
        (b"module_package", module_package_bits),
        (b"package_root", package_root_bits),
    ];
    let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
    let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
    for (key, value_bits) in keys_and_values {
        let key_ptr = alloc_string(_py, key);
        if key_ptr.is_null() {
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        pairs.push(key_bits);
        pairs.push(value_bits);
        owned.push(key_bits);
        owned.push(value_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    for bits in owned {
        dec_ref_bits(_py, bits);
    }
    if dict_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
}

fn string_arg_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(value),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be str"),
        )),
    }
}

fn bytes_arg_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be bytes-like"),
        ));
    };
    let Some(slice) = (unsafe { bytes_like_slice(ptr) }) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be bytes-like"),
        ));
    };
    Ok(slice.to_vec())
}

fn optional_string_arg_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<Option<String>, u64> {
    if obj_from_bits(bits).is_none() {
        return Ok(None);
    }
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(Some(value)),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be str or None"),
        )),
    }
}

const REMOVED_STDLIB_MODULES_313: [&str; 20] = [
    "aifc",
    "audioop",
    "cgi",
    "cgitb",
    "chunk",
    "crypt",
    "imghdr",
    "mailcap",
    "msilib",
    "nis",
    "nntplib",
    "ossaudiodev",
    "pipes",
    "sndhdr",
    "spwd",
    "sunau",
    "telnetlib",
    "tkinter.tix",
    "uu",
    "xdrlib",
];

fn removed_stdlib_313_missing_name(resolved: &str) -> Option<&'static str> {
    REMOVED_STDLIB_MODULES_313.iter().copied().find(|&module| {
        resolved == module
            || resolved
                .strip_prefix(module)
                .is_some_and(|tail| tail.starts_with('.'))
    })
}

/// Provider absence permits another resolver; a declared execution dependency
/// failure does not. The rule table assigns provenance, never error text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KnownImportAbsence {
    Provider(String),
    Dependency(&'static str),
}

pub(crate) fn known_import_absence(
    _py: &PyToken<'_>,
    resolved: &str,
) -> Option<KnownImportAbsence> {
    let target_minor = runtime_target_minor(_py);
    if target_minor >= 13
        && let Some(missing_name) = removed_stdlib_313_missing_name(resolved)
    {
        return Some(KnownImportAbsence::Provider(missing_name.to_string()));
    }
    match resolved {
        "asyncio.graph" if target_minor < 14 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "json.__main__" if target_minor < 14 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "_android_support" if !cfg!(target_os = "android") => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "_remote_debugging" if target_minor < 13 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "_interpchannels" if target_minor < 13 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "_opcode_metadata" if target_minor < 14 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "importlib.metadata.diagnose" if target_minor < 13 => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "importlib.resources._functional" => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "encodings._win_cp_codecs" if !cfg!(target_os = "windows") => {
            Some(KnownImportAbsence::Provider(resolved.to_string()))
        }
        "multiprocessing.popen_spawn_win32" if !cfg!(target_os = "windows") => {
            Some(KnownImportAbsence::Dependency("msvcrt"))
        }
        _ => None,
    }
}

/// Materialize the structured error only after a selected provider owns body
/// execution. Earlier finders and external same-name sources remain independent.
pub(crate) fn raise_known_import_absence(py: &PyToken<'_>, absence: KnownImportAbsence) -> u64 {
    let name = match absence {
        KnownImportAbsence::Provider(name) => name,
        KnownImportAbsence::Dependency(name) => name.to_owned(),
    };
    let Some(key) = attr_name_bits_from_bytes(py, name.as_bytes()) else {
        return MoltObject::none().bits();
    };
    let error = crate::builtins::exceptions::raise_module_not_found(
        py,
        format!("No module named '{name}'").as_bytes(),
        key,
    );
    dec_ref_bits(py, key);
    error
}

fn importlib_modules_runtime_error(_py: &PyToken<'_>) -> u64 {
    raise_exception::<_>(
        _py,
        "RuntimeError",
        "invalid importlib runtime state payload: modules",
    )
}

struct ImportlibSystemModule<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}

impl ImportlibSystemModule<'_, '_> {
    #[inline]
    fn bits(&self) -> u64 {
        self.bits
    }
}

impl Drop for ImportlibSystemModule<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

/// Resolve one owned reference to the interpreter's canonical `sys` module.
/// Every importlib consumer uses this authority so cached lookup, bootstrap
/// import, validation, and release semantics cannot drift by call site.
fn importlib_system_module<'a, 'py>(
    _py: &'a PyToken<'py>,
) -> Result<ImportlibSystemModule<'a, 'py>, u64> {
    let Some(sys_bits) = crate::builtins::modules::ensure_interpreter_sys_module(_py) else {
        return Err(if exception_pending(_py) {
            MoltObject::none().bits()
        } else {
            importlib_modules_runtime_error(_py)
        });
    };
    inc_ref_bits(_py, sys_bits);

    Ok(ImportlibSystemModule {
        py: _py,
        bits: sys_bits,
    })
}

pub(crate) fn importlib_runtime_modules_bits(_py: &PyToken<'_>) -> Result<u64, u64> {
    let sys = importlib_system_module(_py)?;
    let Some(modules_bits) = sys_modules_dict_bits(_py, sys.bits()) else {
        return Err(importlib_modules_runtime_error(_py));
    };
    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        return Err(importlib_modules_runtime_error(_py));
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        return Err(importlib_modules_runtime_error(_py));
    }
    Ok(modules_bits)
}

fn importlib_dict_get_string_key_bits(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Result<Option<u64>, u64> {
    let value_bits = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    let value_bits = match value_bits {
        Ok(value) => value,
        Err(()) => {
            return Err(MoltObject::none().bits());
        }
    };
    Ok(value_bits)
}

fn importlib_dict_set_string_key(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    value_bits: u64,
) -> Result<(), u64> {
    let completed = unsafe { dict_set_in_place(_py, dict_ptr, key_bits, value_bits) };
    if !completed {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

#[cfg(test)]
fn pending_exception_kind_and_message(_py: &PyToken<'_>) -> Option<(String, String)> {
    if !exception_pending(_py) {
        return None;
    }
    let exception =
        crate::builtins::exceptions::ExceptionValue::adopt(_py, molt_exception_last_pending());
    let exc_ptr = maybe_ptr_from_bits(exception.bits())?;
    let kind = crate::builtins::exceptions::exception_diagnostic_name(exc_ptr);
    let mut message = String::new();
    if !crate::builtins::exceptions::with_saved_raised_exception(_py, || {
        message = format_obj_str(_py, obj_from_bits(exception.bits()));
        !exception_pending(_py)
    }) {
        return None;
    }
    Some((kind, message))
}

fn importlib_required_callable(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
    owner: &str,
) -> Result<u64, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    match importlib_reader_lookup_callable(_py, target_bits, attr_name)? {
        Some(bits) => Ok(bits),
        None => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{owner}.{} is unavailable", String::from_utf8_lossy(name)),
        )),
    }
}

fn importlib_required_attribute(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
    owner: &str,
) -> Result<u64, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    match getattr_optional_bits(_py, target_bits, attr_name)? {
        Some(bits) => Ok(bits),
        None => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{owner}.{} is unavailable", String::from_utf8_lossy(name)),
        )),
    }
}

fn importlib_set_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
    value_bits: u64,
) -> Result<(), u64> {
    let attr_name = intern_static_name(_py, slot, name);
    let result_bits = crate::molt_set_attr_name(target_bits, attr_name, value_bits);
    if !obj_from_bits(result_bits).is_none() {
        dec_ref_bits(_py, result_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

/// Metadata factories consume either a retained facade module or a function's
/// actual defining globals. A public sys.modules spelling is never rediscovered.
fn importlib_machinery_member(
    py: &PyToken<'_>,
    namespace: u64,
    slot: &AtomicU64,
    name: &'static [u8],
) -> Result<u64, u64> {
    if let Some(dictionary) = obj_from_bits(namespace).as_ptr()
        && unsafe { object_type_id(dictionary) == TYPE_ID_DICT }
    {
        let key = intern_static_name(py, slot, name);
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let value = unsafe { dict_get_in_place(py, dictionary, key) }
            .map_err(|()| MoltObject::none().bits())?;
        if let Some(value) = value {
            inc_ref_bits(py, value);
            return Ok(value);
        }
        return Err(raise_exception::<_>(
            py,
            "RuntimeError",
            &format!(
                "importlib.machinery.{} is unavailable",
                String::from_utf8_lossy(name)
            ),
        ));
    }
    importlib_required_attribute(py, namespace, slot, name, "importlib.machinery")
}

fn importlib_machinery_loader_instance(
    _py: &PyToken<'_>,
    machinery_bits: u64,
    class_slot: &AtomicU64,
    class_name: &'static [u8],
    args: &[u64],
) -> Result<u64, u64> {
    let loader_cls_bits = importlib_machinery_member(_py, machinery_bits, class_slot, class_name)?;
    let loader_bits = call_callable_positional(_py, loader_cls_bits, args)?;
    dec_ref_bits(_py, loader_cls_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(loader_bits)
}

fn importlib_find_spec_object_bits(
    _py: &PyToken<'_>,
    fullname: &str,
    payload: &ImportlibPathResolution,
    machinery_bits: u64,
) -> Result<u64, u64> {
    let fullname_bits = alloc_str_bits(_py, fullname)?;
    let origin_bits = match payload.origin.as_deref() {
        Some(origin) => match alloc_str_bits(_py, origin) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    let loader_bits = match payload.loader_kind.as_str() {
        "source" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"SourceFileLoader"),
            b"SourceFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "extension" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"ExtensionFileLoader"),
            b"ExtensionFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "bytecode" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"SourcelessFileLoader"),
            b"SourcelessFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "zip_source" => {
            let Some(zip_archive) = payload.zip_archive.as_deref() else {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib find-spec payload: zip source archive missing",
                ));
            };
            let Some(zip_inner_path) = payload.zip_inner_path.as_deref() else {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib find-spec payload: zip source inner path missing",
                ));
            };
            let zip_archive_bits = match alloc_str_bits(_py, zip_archive) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            };
            let zip_inner_path_bits = match alloc_str_bits(_py, zip_inner_path) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(zip_archive_bits).is_none() {
                        dec_ref_bits(_py, zip_archive_bits);
                    }
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            };
            let out = importlib_machinery_loader_instance(
                _py,
                machinery_bits,
                runtime_static_name_slot(_py, b"_ZipSourceLoader"),
                b"_ZipSourceLoader",
                &[fullname_bits, zip_archive_bits, zip_inner_path_bits],
            );
            if !obj_from_bits(zip_archive_bits).is_none() {
                dec_ref_bits(_py, zip_archive_bits);
            }
            if !obj_from_bits(zip_inner_path_bits).is_none() {
                dec_ref_bits(_py, zip_inner_path_bits);
            }
            match out {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            }
        }
        "namespace" => MoltObject::none().bits(),
        kind => {
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                &format!("unsupported importlib loader kind: {kind}"),
            ));
        }
    };

    let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
    let module_spec_cls_bits = match importlib_machinery_member(
        _py,
        machinery_bits,
        runtime_static_name_slot(_py, b"ModuleSpec"),
        b"ModuleSpec",
    ) {
        Ok(bits) => bits,
        Err(err) => {
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    };
    let spec_bits = match call_callable_positional(
        _py,
        module_spec_cls_bits,
        &[fullname_bits, loader_bits, origin_bits, is_package_bits],
    ) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, module_spec_cls_bits);
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    };
    dec_ref_bits(_py, module_spec_cls_bits);
    if exception_pending(_py) {
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(MoltObject::none().bits());
    }

    if let Some(locations) = payload.submodule_search_locations.as_ref() {
        let Some(locations_bits) = alloc_string_list_bits(_py, locations) else {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        };
        let out = importlib_set_attr(
            _py,
            spec_bits,
            runtime_static_name_slot(_py, b"submodule_search_locations"),
            b"submodule_search_locations",
            locations_bits,
        );
        if !obj_from_bits(locations_bits).is_none() {
            dec_ref_bits(_py, locations_bits);
        }
        if let Err(err) = out {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    }

    let cached_bits = match payload.cached.as_deref() {
        Some(cached) => match alloc_str_bits(_py, cached) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(loader_bits).is_none() {
                    dec_ref_bits(_py, loader_bits);
                }
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    if let Err(err) = importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"cached"),
        b"cached",
        cached_bits,
    ) {
        if !obj_from_bits(cached_bits).is_none() {
            dec_ref_bits(_py, cached_bits);
        }
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(err);
    }
    if !obj_from_bits(cached_bits).is_none() {
        dec_ref_bits(_py, cached_bits);
    }

    let has_location_bits = MoltObject::from_bool(payload.has_location).bits();
    if let Err(err) = importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"has_location"),
        b"has_location",
        has_location_bits,
    ) {
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(err);
    }

    if !obj_from_bits(fullname_bits).is_none() {
        dec_ref_bits(_py, fullname_bits);
    }
    if !obj_from_bits(origin_bits).is_none() {
        dec_ref_bits(_py, origin_bits);
    }
    if !obj_from_bits(loader_bits).is_none() {
        dec_ref_bits(_py, loader_bits);
    }

    Ok(spec_bits)
}

fn importlib_single_item_list_bits(_py: &PyToken<'_>, value_bits: u64) -> Result<u64, u64> {
    let list_ptr = alloc_list(_py, &[value_bits]);
    if list_ptr.is_null() {
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

fn importlib_require_package_root_bits(
    _py: &PyToken<'_>,
    package_root_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(package_root_bits).is_none()
        || string_obj_to_owned(obj_from_bits(package_root_bits)).is_none()
    {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid importlib package root for package module",
        ));
    }
    Ok(())
}

fn importlib_module_support_bits(
    _py: &PyToken<'_>,
    modules_ptr: *mut u8,
    module_name: &'static str,
) -> Result<u64, u64> {
    let key_bits = alloc_str_bits(_py, module_name)?;
    let cached_bits = unsafe { dict_get_in_place(_py, modules_ptr, key_bits) };
    let cached_bits = match cached_bits {
        Ok(value) => value,
        Err(()) => {
            dec_ref_bits(_py, key_bits);
            return Err(MoltObject::none().bits());
        }
    };
    if let Some(cached_bits) = cached_bits {
        dec_ref_bits(_py, key_bits);
        if obj_from_bits(cached_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!("import of {module_name} halted; None in sys.modules"),
            ));
        }
        inc_ref_bits(_py, cached_bits);
        return Ok(cached_bits);
    }

    // Support loading is a typed internal catalog capability. It must not
    // recursively ask its own public finder implementation to bootstrap itself.
    let imported = crate::builtins::modules::catalog_import_attempt(key_bits);
    dec_ref_bits(_py, key_bits);
    match imported? {
        crate::builtins::modules::ModuleImportOutcome::Imported(bits)
            if !obj_from_bits(bits).is_none() =>
        {
            Ok(bits)
        }
        crate::builtins::modules::ModuleImportOutcome::Imported(_)
        | crate::builtins::modules::ModuleImportOutcome::Missing { .. } => {
            Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                &format!("runtime support module unavailable: {module_name}"),
            ))
        }
    }
}

#[derive(Clone, Copy)]
enum ImportlibColdParentPolicy {
    Membership,
    NonNoneAndInitializingParent,
}

fn importlib_import_via_spec(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
    import_mode: ImportlibImportMode,
) -> Result<Option<u64>, u64> {
    let _execution =
        match crate::builtins::module_table::ModuleExecutionGuard::acquire(_py, resolved) {
            Ok(guard) => guard,
            Err(failure) => return Err(failure.into_exception(_py, resolved)),
        };
    if let Some(existing_bits) =
        importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)?
    {
        if obj_from_bits(existing_bits).is_none() {
            return Err(crate::builtins::exceptions::raise_module_not_found(
                _py,
                format!("import of {resolved} halted; None in sys.modules").as_bytes(),
                resolved_bits,
            ));
        }
        inc_ref_bits(_py, existing_bits);
        return Ok(Some(existing_bits));
    }

    use crate::builtins::exceptions::ExceptionValue;
    // The admitted target authority owns version selection. Pinned 3.12 uses
    // membership; pinned 3.13/3.14 add a parent wait and non-None rechecks.
    let cold_parent_policy = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
        ImportlibColdParentPolicy::NonNoneAndInitializingParent
    } else {
        ImportlibColdParentPolicy::Membership
    };
    let (parent_owner, _parent_spec_owner, parent_path) = if let Some((parent_name, _)) = resolved
        .rsplit_once('.')
        .filter(|(parent, _)| !parent.is_empty())
    {
        let parent_key = ExceptionValue::adopt(_py, alloc_str_bits(_py, parent_name)?);
        // Reuse the caller's actual parent-import protocol. _gcd_import returns
        // the leaf; import.c passes the saved builtin __import__, including its
        // false-fromlist dotted-root return/reimport before discarding it.
        let import_parent = || -> Result<(), u64> {
            // Missing package prefixes recurse through this same cold owner.
            // Charge the existing execution-stack limit before descending.
            let Some(_depth) = crate::state::recursion::RecursionGuard::enter(_py) else {
                return Err(MoltObject::none().bits());
            };
            let return_plan = match import_mode {
                ImportlibImportMode::Python => ImportlibTransactionReturn::Leaf,
                ImportlibImportMode::Builtin => ImportlibTransactionReturn::Dunder {
                    requested_name: parent_name,
                    level: 0,
                    fromlist_bits: MoltObject::none().bits(),
                },
            };
            let imported = ExceptionValue::adopt(
                _py,
                importlib_import_resolved_transaction(_py, parent_name, modules_ptr, return_plan)?,
            );
            drop(imported);
            Ok(())
        };
        // Parent admission belongs to this locked cold miss. A cached leaf
        // never observes its parents, and a present None parent is not absent.
        if importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key.bits())?.is_none() {
            import_parent()?;
        }
        // Parent execution and return-owner retirement may publish the child.
        // 3.12 returns even None; 3.13/3.14 continue loading when it is None.
        if let Some(existing_bits) =
            importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)?
            && (matches!(cold_parent_policy, ImportlibColdParentPolicy::Membership)
                || !obj_from_bits(existing_bits).is_none())
        {
            inc_ref_bits(_py, existing_bits);
            return Ok(Some(existing_bits));
        }
        let parent = importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key.bits())?
            .ok_or_else(|| {
                crate::builtins::exceptions::raise_key_error_with_key::<u64>(_py, parent_key.bits())
            })?;
        let parent = ExceptionValue::pin(_py, parent);
        let key = intern_runtime_static_name(_py, b"__path__");
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let bits = match crate::builtins::attributes::get_attr_name(_py, parent.bits(), key) {
            Ok(bits) => bits,
            Err(()) => {
                use crate::builtins::exceptions::{
                    ExceptionCleanupPolicy, finish_exception_cleanup, take_raised,
                };
                let error = ExceptionValue::adopt(_py, molt_exception_last_pending());
                if !crate::builtins::exceptions::exception_matches_builtin_name(
                    _py,
                    error.bits(),
                    "AttributeError",
                ) {
                    return Err(MoltObject::none().bits());
                }
                let saved = Some(take_raised(_py));
                let child_repr =
                    crate::object::ops_format::format_obj_bytes(_py, obj_from_bits(resolved_bits));
                if !exception_pending(_py) {
                    let parent_repr = crate::object::ops_format::format_obj_bytes(
                        _py,
                        obj_from_bits(parent_key.bits()),
                    );
                    if !exception_pending(_py) {
                        let message = [
                            b"No module named ".as_slice(),
                            &child_repr,
                            b"; ",
                            &parent_repr,
                            b" is not a package",
                        ]
                        .concat();
                        crate::builtins::exceptions::raise_module_not_found(
                            _py,
                            &message,
                            resolved_bits,
                        );
                        let missing = ExceptionValue::adopt(_py, molt_exception_last_pending());
                        if crate::builtins::exceptions::exception_matches_builtin_name(
                            _py,
                            missing.bits(),
                            "ModuleNotFoundError",
                        ) {
                            let _ = crate::builtins::exceptions::exception_replace_suppress_context(
                                _py,
                                missing.bits(),
                                true,
                            );
                        }
                    }
                }
                finish_exception_cleanup(_py, saved, ExceptionCleanupPolicy::Chain);
                return Err(MoltObject::none().bits());
            }
        };
        let path = ExceptionValue::adopt(_py, bits);
        let spec_key = intern_runtime_static_name(_py, b"__spec__");
        if spec_key == 0 {
            return Err(MoltObject::none().bits());
        }
        // This is a required read after __path__, not the optional module
        // cache probe. Retain this exact spec through finder/loader callbacks.
        let parent_spec = ExceptionValue::adopt(
            _py,
            crate::builtins::attributes::get_attr_name(_py, parent.bits(), spec_key)
                .map_err(|()| MoltObject::none().bits())?,
        );
        if matches!(
            cold_parent_policy,
            ImportlibColdParentPolicy::NonNoneAndInitializingParent
        ) {
            if spec_is_initializing(
                _py,
                parent_spec.bits(),
                crate::builtins::module_table::ModuleImportPolicy::Python,
            )? {
                import_parent()?;
            }
            // Path/spec/flag callbacks can publish the child even when the
            // parent is not initializing. This second check is unconditional.
            if let Some(existing_bits) =
                importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)?
                && !obj_from_bits(existing_bits).is_none()
            {
                inc_ref_bits(_py, existing_bits);
                return Ok(Some(existing_bits));
            }
        }
        (Some(parent), Some(parent_spec), Some(path))
    } else {
        (None, None, None)
    };
    let spec_bits = importlib_find_spec_impl(
        _py,
        resolved_bits,
        parent_path
            .as_ref()
            .map_or(MoltObject::none().bits(), ExceptionValue::bits),
        MoltObject::none().bits(),
    )?;
    let _spec_owner = ExceptionValue::adopt(_py, spec_bits);
    if obj_from_bits(spec_bits).is_none() {
        return Ok(None);
    }
    importlib_enforce_extension_spec_object_boundary(_py, resolved, spec_bits)?;

    let result = importlib_spec_execution_transaction(
        _py,
        resolved,
        resolved_bits,
        spec_bits,
        modules_ptr,
        ImportlibSpecExecutionOptions {
            target: ImportlibSpecTarget::Fresh,
            allow_load_module_fallback: true,
        },
    );
    let out_bits = result?;
    // This branch executed a fresh spec. Cache hits and explicit loader/reload
    // transactions never reach its parent-publication operation.
    if let Err(error) =
        importlib_bind_submodule_on_parent(_py, resolved, out_bits, modules_ptr, parent_owner)
    {
        dec_ref_bits(_py, out_bits);
        return Err(error);
    }
    Ok(Some(out_bits))
}

#[derive(Clone, Copy)]
enum ImportlibSpecTarget {
    Fresh,
    Current,
    Exact(u64),
}

#[derive(Clone, Copy)]
struct ImportlibSpecExecutionOptions {
    target: ImportlibSpecTarget,
    allow_load_module_fallback: bool,
}

fn initialization_probe_result(
    py: &PyToken<'_>,
    policy: crate::builtins::module_table::ModuleImportPolicy,
    result: Result<bool, u64>,
) -> Result<bool, u64> {
    if result.is_err()
        && policy != crate::builtins::module_table::ModuleImportPolicy::Python
        && !crate::object::ops_sys::runtime_target_at_least(py, 3, 13)
    {
        clear_exception(py);
        Ok(false)
    } else {
        result
    }
}

/// One owned public spec-flag protocol, shared with ModuleType diagnostics.
pub(crate) fn spec_is_initializing(
    py: &PyToken<'_>,
    spec: u64,
    policy: crate::builtins::module_table::ModuleImportPolicy,
) -> Result<bool, u64> {
    let result = (|| {
        let key = intern_runtime_static_name(py, b"_initializing");
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let Some(value) = getattr_optional_bits(py, spec, key)? else {
            return Ok(false);
        };
        let value = crate::builtins::exceptions::ExceptionValue::adopt(py, value);
        crate::object::ops::truth_value(py, obj_from_bits(value.bits()))
            .map_err(|()| MoltObject::none().bits())
    })();
    initialization_probe_result(py, policy, result)
}

pub(crate) fn public_module_is_initializing(
    py: &PyToken<'_>,
    module: u64,
    policy: crate::builtins::module_table::ModuleImportPolicy,
) -> Result<bool, u64> {
    let result = (|| {
        let key = intern_runtime_static_name(py, b"__spec__");
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let Some(spec) = getattr_optional_bits(py, module, key)? else {
            return Ok(false);
        };
        let spec = crate::builtins::exceptions::ExceptionValue::adopt(py, spec);
        spec_is_initializing(py, spec.bits(), policy)
    })();
    initialization_probe_result(py, policy, result)
}

thread_local! {
    // Only the spec transaction may add its exact module to the runtime cache.
    // A direct Loader.exec_module call owns neither sys.modules nor cache
    // publication, even if the caller supplied a valid physical C module.
    static IMPORTLIB_SPEC_EXEC_TARGET: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

struct ImportlibSpecExecTargetGuard(Option<u64>);

impl ImportlibSpecExecTargetGuard {
    fn enter(module_bits: u64) -> Self {
        let previous = IMPORTLIB_SPEC_EXEC_TARGET.with(|target| target.replace(Some(module_bits)));
        Self(previous)
    }
}

impl Drop for ImportlibSpecExecTargetGuard {
    fn drop(&mut self) {
        IMPORTLIB_SPEC_EXEC_TARGET.with(|target| target.set(self.0));
    }
}

#[cfg(any(test, all(feature = "cext_loader", not(target_arch = "wasm32"))))]
fn importlib_spec_exec_owns(module_bits: u64) -> bool {
    IMPORTLIB_SPEC_EXEC_TARGET.with(|target| target.get() == Some(module_bits))
}

fn importlib_spec_execution_failure(py: &PyToken<'_>, dictionary: *mut u8, spec: u64) {
    let saved = exception_pending(py).then(|| crate::builtins::exceptions::take_raised(py));
    if let Ok(name) = crate::builtins::module_table::spec_name(py, spec) {
        unsafe {
            let _ = dict_del_in_place(py, dictionary, name.bits());
        }
    }
    crate::builtins::exceptions::finish_exception_cleanup(
        py,
        saved,
        crate::builtins::exceptions::ExceptionCleanupPolicy::IgnoreKeyError,
    );
}

fn importlib_warn_legacy_loader(
    py: &PyToken<'_>,
    loader: crate::builtins::exceptions::ExceptionValue<'_, '_>,
) -> Result<(), u64> {
    use crate::builtins::exceptions::ExceptionValue;
    let key = crate::attr_name_bits_from_bytes(py, b"__qualname__")
        .ok_or_else(|| MoltObject::none().bits())?;
    let key = ExceptionValue::adopt(py, key);
    let label = match getattr_optional_bits(py, loader.bits(), key.bits())? {
        Some(label) => label,
        None => {
            let class = ExceptionValue::pin(py, type_of_bits(py, loader.bits()));
            crate::builtins::attributes::get_attr_name(py, class.bits(), key.bits())
                .map_err(|()| MoltObject::none().bits())?
        }
    };
    let label = ExceptionValue::adopt(py, label);
    drop(loader);
    let mut message =
        crate::object::ops_format::format_obj_str_bytes(py, obj_from_bits(label.bits()));
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    message.extend_from_slice(b".exec_module() not found; falling back to load_module()");
    let pointer = alloc_string(py, &message);
    if pointer.is_null() {
        return Err(MoltObject::none().bits());
    }
    let message = ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits());
    let category = crate::builtins::exceptions::exception_type_bits_from_name(py, "ImportWarning");
    if category == 0
        || exception_pending(py)
        || !crate::builtins::warnings_ext::emit_warning_object(py, message.bits(), category, 1)
    {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn importlib_spec_execution_transaction(
    py: &PyToken<'_>,
    module_name: &str,
    name_bits: u64,
    spec_bits: u64,
    modules_ptr: *mut u8,
    options: ImportlibSpecExecutionOptions,
) -> Result<u64, u64> {
    use crate::builtins::exceptions::{ExceptionCleanupPolicy, ExceptionValue};
    use crate::builtins::module_table::{
        PublicationName, complete_publication, finish_spec_initializing, set_spec_initializing,
        spec_name,
    };
    let _execution =
        match crate::builtins::module_table::ModuleExecutionGuard::acquire(py, module_name) {
            Ok(guard) => guard,
            Err(failure) => return Err(failure.into_exception(py, module_name)),
        };
    let existing = match options.target {
        ImportlibSpecTarget::Fresh => None,
        ImportlibSpecTarget::Current => {
            importlib_dict_get_string_key_bits(py, modules_ptr, name_bits)?
        }
        ImportlibSpecTarget::Exact(original) => {
            let observed = importlib_dict_get_string_key_bits(py, modules_ptr, name_bits)?;
            if observed != Some(original) {
                let repr =
                    crate::object::ops_format::format_obj_bytes(py, obj_from_bits(name_bits));
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
                let message = [b"module ".as_slice(), &repr, b" not in sys.modules"].concat();
                return Err(crate::builtins::exceptions::raise_import_error(
                    py,
                    &message,
                    name_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                ));
            }
            observed
        }
    };
    let mut existing_owner = existing.map(|bits| ExceptionValue::pin(py, bits));
    let spec = ExceptionValue::pin(py, spec_bits);
    let dictionary = MoltObject::from_ptr(modules_ptr).bits();
    let loader_name = intern_runtime_static_name(py, b"loader");
    let exec_name = intern_runtime_static_name(py, b"exec_module");
    if loader_name == 0 || exec_name == 0 {
        return Err(MoltObject::none().bits());
    }
    let read_loader = || -> Result<ExceptionValue<'_, '_>, u64> {
        let bits = crate::builtins::attributes::get_attr_name(py, spec.bits(), loader_name)
            .map_err(|()| MoltObject::none().bits())?;
        Ok(ExceptionValue::adopt(py, bits))
    };
    if existing.is_none() {
        // _load_unlocked's legacy-loader selection precedes module creation;
        // _exec performs all of its loader observations inside its finally.
        let namespace = {
            let loader = read_loader()?;
            obj_from_bits(loader.bits()).is_none()
        };
        let missing_exec = !namespace && {
            let loader = read_loader()?;
            getattr_optional_bits(py, loader.bits(), exec_name)?
                .map(|bits| ExceptionValue::adopt(py, bits))
                .is_none()
        };
        if missing_exec {
            if !options.allow_load_module_fallback {
                return Err(raise_exception::<_>(py, "ImportError", ""));
            }
            importlib_warn_legacy_loader(py, read_loader()?)?;
            let loader = read_loader()?;
            let load_name = intern_runtime_static_name(py, b"load_module");
            if load_name == 0 {
                return Err(MoltObject::none().bits());
            }
            let load = ExceptionValue::adopt(
                py,
                crate::builtins::attributes::get_attr_name(py, loader.bits(), load_name)
                    .map_err(|()| MoltObject::none().bits())?,
            );
            drop(loader);
            let name = spec_name(py, spec.bits())?;
            let loaded = unsafe { call_callable1(py, load.bits(), name.bits()) };
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, loaded));
            if exception_pending(py) {
                let saved = Some(crate::builtins::exceptions::take_raised(py));
                // Legacy load_module does not delete on failure. A surviving
                // current spec-name entry is only reordered.
                if let Ok(name) = spec_name(py, spec.bits()) {
                    let current = unsafe { dict_get_in_place(py, modules_ptr, name.bits()) };
                    if matches!(current, Ok(Some(_)))
                        && let Ok(bits) =
                            complete_publication(py, dictionary, PublicationName::Spec(spec.bits()))
                    {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(py, bits)
                        });
                    }
                }
                crate::builtins::exceptions::finish_exception_cleanup(
                    py,
                    saved,
                    ExceptionCleanupPolicy::Chain,
                );
                return Err(MoltObject::none().bits());
            }
            return complete_publication(py, dictionary, PublicationName::Spec(spec.bits()));
        }
    }
    let ordinary_owner = if existing.is_none() {
        crate::builtins::module_table::RegisteredSpecImport::begin(py, module_name)?
    } else {
        None
    };
    let module = match existing {
        Some(_) => existing_owner.take().expect("pinned existing target"),
        None => {
            let bits = importlib_ffi::importlib_module_from_spec_impl(py, spec.bits());
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            ExceptionValue::adopt(py, bits)
        }
    };
    let using_existing = existing.is_some();
    let ordinary = !using_existing;
    if ordinary {
        // CPython assigns True outside the try/finally: if this setter fails,
        // neither publication nor the False setter is admitted.
        set_spec_initializing(py, spec.bits(), true);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if let Some(owner) = ordinary_owner.as_ref() {
            owner.admit(module_name, module.bits(), spec.bits());
        }
    }
    let mut entered_body = false;
    let result = (|| {
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if ordinary {
            let name = spec_name(py, spec.bits())?;
            importlib_dict_set_string_key(py, modules_ptr, name.bits(), module.bits())?;
        }
        entered_body = true;
        let namespace = {
            let loader = read_loader()?;
            obj_from_bits(loader.bits()).is_none()
        };
        if namespace {
            let locations_name = intern_runtime_static_name(py, b"submodule_search_locations");
            if locations_name == 0 {
                return Err(MoltObject::none().bits());
            }
            let locations =
                crate::builtins::attributes::get_attr_name(py, spec.bits(), locations_name)
                    .map_err(|()| MoltObject::none().bits())?;
            let locations = ExceptionValue::adopt(py, locations);
            if obj_from_bits(locations.bits()).is_none() {
                let name = spec_name(py, spec.bits())?;
                return Err(crate::builtins::exceptions::raise_import_error(
                    py,
                    b"missing loader",
                    name.bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                ));
            }
            if using_existing {
                importlib_ffi::importlib_init_module_attrs_impl(
                    py,
                    spec.bits(),
                    module.bits(),
                    true,
                )?;
            }
        } else {
            if using_existing {
                importlib_ffi::importlib_init_module_attrs_impl(
                    py,
                    spec.bits(),
                    module.bits(),
                    true,
                )?;
            }
            // _exec's hasattr and eventual invocation are two observable
            // loader/descriptor reads. A present non-callable is still called.
            let present = if using_existing {
                let loader = read_loader()?;
                getattr_optional_bits(py, loader.bits(), exec_name)?
                    .map(|bits| ExceptionValue::adopt(py, bits))
                    .is_some()
            } else {
                true
            };
            if !present && options.allow_load_module_fallback {
                importlib_warn_legacy_loader(py, read_loader()?)?;
                let loader = read_loader()?;
                let key = intern_runtime_static_name(py, b"load_module");
                if key == 0 {
                    return Err(MoltObject::none().bits());
                }
                let load = ExceptionValue::adopt(
                    py,
                    crate::builtins::attributes::get_attr_name(py, loader.bits(), key)
                        .map_err(|()| MoltObject::none().bits())?,
                );
                drop(loader);
                let loaded = unsafe { call_callable1(py, load.bits(), name_bits) };
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, loaded));
            } else {
                let loader = read_loader()?;
                let executor = ExceptionValue::adopt(
                    py,
                    crate::builtins::attributes::get_attr_name(py, loader.bits(), exec_name)
                        .map_err(|()| MoltObject::none().bits())?,
                );
                drop(loader);
                let out = {
                    let _target = ImportlibSpecExecTargetGuard::enter(module.bits());
                    unsafe { call_callable1(py, executor.bits(), module.bits()) }
                };
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, out));
            }
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
        }
        Ok(())
    })();
    if result.is_err() && ordinary && entered_body {
        importlib_spec_execution_failure(py, modules_ptr, spec.bits());
    }
    let result = if using_existing {
        // _exec's finally reorders even after failure, without deleting the
        // existing module or changing its spec's initializing state.
        let saved = exception_pending(py).then(|| crate::builtins::exceptions::take_raised(py));
        let completed = complete_publication(py, dictionary, PublicationName::Spec(spec.bits()));
        crate::builtins::exceptions::finish_exception_cleanup(
            py,
            saved,
            ExceptionCleanupPolicy::Chain,
        );
        completed
    } else if result.is_ok() {
        result
            .and_then(|()| complete_publication(py, dictionary, PublicationName::Spec(spec.bits())))
    } else {
        Err(MoltObject::none().bits())
    };
    if let Some(owner) = ordinary_owner {
        owner.finish();
    } else if ordinary {
        finish_spec_initializing(py, spec.bits());
    }
    // The original spec and namespace stay pinned through every finally and
    // completion callback. Their retirement cannot replace the selected error.
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        drop(module);
        drop(spec);
    });
    let _ = module_name;
    if exception_pending(py) {
        if let Ok(bits) = result {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
        }
        Err(MoltObject::none().bits())
    } else {
        result
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_load_module_from_spec(
    loader_bits: u64,
    fullname_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "fullname") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if obj_from_bits(loader_bits).is_none() {
            return raise_exception::<_>(_py, "ImportError", "");
        }
        let modules_bits = match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let out = (|| -> Result<u64, u64> {
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                return Err(importlib_modules_runtime_error(_py));
            };
            importlib_spec_execution_transaction(
                _py,
                &fullname,
                fullname_bits,
                spec_bits,
                modules_ptr,
                ImportlibSpecExecutionOptions {
                    target: ImportlibSpecTarget::Current,
                    allow_load_module_fallback: false,
                },
            )
        })();
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

fn importlib_bind_submodule_on_parent(
    _py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
    modules_ptr: *mut u8,
    previous_parent: Option<crate::builtins::exceptions::ExceptionValue<'_, '_>>,
) -> Result<(), u64> {
    let Some((parent_name, child_name)) = resolved.rsplit_once('.') else {
        return Ok(());
    };
    if parent_name.is_empty() {
        return Ok(());
    }

    use crate::builtins::exceptions::ExceptionValue;
    let parent_key = ExceptionValue::adopt(_py, alloc_str_bits(_py, parent_name)?);
    let parent_bits = importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key.bits())?
        .ok_or_else(|| {
            crate::builtins::exceptions::raise_key_error_with_key::<u64>(_py, parent_key.bits())
        })?;
    let parent_owner = ExceptionValue::pin(_py, parent_bits);
    // Python's parent_module reassignment acquires the fresh cache owner before
    // retiring the earlier observation; its finalizer may mutate the cache.
    drop(previous_parent);
    crate::builtins::modules::publish_import_child(
        _py,
        parent_owner.bits(),
        parent_name,
        child_name,
        module_bits,
    )
}

fn string_sequence_arg_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<Vec<String>, u64> {
    let Some(mut iterator) = crate::object::iterable::OwnedIterator::new(_py, bits) else {
        return Err(MoltObject::none().bits());
    };
    let mut out: Vec<String> = Vec::new();
    while let Some(bits) = iterator.next().map_err(|_| MoltObject::none().bits())? {
        let value = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), bits)
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(value.bits())) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{name} entries must be str"),
            ));
        };
        out.push(text);
    }
    Ok(out)
}

#[path = "platform_importlib_support.rs"]
mod importlib_support;
use importlib_support::*;

#[path = "platform_importlib_resources/mod.rs"]
mod importlib_resources;
pub use importlib_resources::*;

#[path = "platform_importlib_ffi.rs"]
mod importlib_ffi;
pub use importlib_ffi::*;

#[path = "platform_env_ffi.rs"]
mod env_ffi;
pub use env_ffi::*;

#[cfg(test)]
#[path = "platform_tests.rs"]
mod tests;
