use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(test)]
use crate::format_exception_with_traceback;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::runtime_flags::{self, RuntimeFlag};
use molt_obj_model::MoltObject;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::{
    attr_name_bits_from_bytes, clear_attribute_error_if_pending, module_attr_lookup,
};
use crate::builtins::classes::builtin_classes;
use crate::builtins::exceptions::{
    ExceptionValue, molt_exception_last_pending, raise_import_error,
};
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    HashContext, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    alloc_dict_with_pairs, alloc_list, alloc_module_obj, alloc_string, alloc_tuple, call_callable0,
    call_callable1, call_callable2, class_mro_vec, clear_exception, dec_ref_bits,
    dict_del_in_place, dict_get_in_place, dict_set_in_place, exception_pending, format_obj_str,
    frame_stack_active_globals_bits, has_capability, inc_ref_bits, init_atomic_bits,
    intern_static_name, is_missing_bits, is_truthy, missing_bits, module_dict_bits,
    module_name_bits, molt_call_bind, molt_callargs_expand_kwstar, molt_callargs_expand_star,
    molt_callargs_new, molt_callargs_push_pos, molt_exception_kind, molt_getattr_builtin,
    molt_int_from_obj, molt_is_callable, obj_from_bits, object_type_id, ptr_from_bits,
    raise_exception, runtime_state, set_add_in_place, string_bytes, string_len,
    string_obj_to_owned, type_name, type_of_bits,
};

mod execution;
mod import_star;
mod runpy;
mod type_attributes;
pub(crate) use type_attributes::raise_module_attribute_miss;
mod type_protocol;
pub(crate) use type_protocol::publish_module_methods;

pub(crate) use execution::{ExecutionMetadata, execute_compiled_module};
use execution::{copy_dict_entries, execution_sys_path_entries, module_dict_ptr};
pub use runpy::{molt_runpy_run_module, molt_runpy_run_path};
pub use type_attributes::*;
pub use type_protocol::*;

/// Original defining globals for an admitted initializer/private body.
pub(crate) fn module_body_owns_globals(py: &PyToken<'_>, name: &str, globals: u64) -> bool {
    execution::execution_owns_globals(py, name, globals)
        || crate::builtins::module_table::module_execution_owns_initializing_globals(
            py, name, globals,
        )
}

fn trace_module_cache() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_MODULE_CACHE)
}

fn trace_name_error() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_NAME_ERROR)
}

/// Diagnostic level shared by `MOLT_TRACE_MODULE_ATTRS` and
/// `MOLT_TRACE_MODULE_GLOBALS`: `1` traces a filtered set of names, `all` or
/// `verbose` traces every operation. Each flag is read once per process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TraceMode {
    Off,
    Filtered,
    Verbose,
}

fn trace_mode(raw: Option<&str>) -> TraceMode {
    match raw {
        Some("all") | Some("verbose") => TraceMode::Verbose,
        Some("1") => TraceMode::Filtered,
        _ => TraceMode::Off,
    }
}

fn trace_module_attrs_mode() -> TraceMode {
    static MODE: OnceLock<TraceMode> = OnceLock::new();
    *MODE.get_or_init(|| trace_mode(std::env::var("MOLT_TRACE_MODULE_ATTRS").ok().as_deref()))
}

/// Whether `mode` traces an access to `attr_name` on `module_name`. Verbose
/// traces every access; the filtered mode names `_sys` and importlib modules.
fn module_attr_trace_selected(mode: TraceMode, module_name: &str, attr_name: &str) -> bool {
    match mode {
        TraceMode::Off => false,
        TraceMode::Verbose => true,
        TraceMode::Filtered => attr_name == "_sys" || module_name.contains("importlib"),
    }
}

/// The module and attribute names when `MOLT_TRACE_MODULE_ATTRS` traces this
/// get or set.
unsafe fn traced_module_attr(module_ptr: *mut u8, attr_bits: u64) -> Option<(String, String)> {
    let mode = trace_module_attrs_mode();
    if mode == TraceMode::Off {
        return None;
    }
    let module_name = string_obj_to_owned(obj_from_bits(unsafe { module_name_bits(module_ptr) }))
        .unwrap_or_else(|| "<module>".to_string());
    let attr_name =
        string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_else(|| "<attr>".to_string());
    module_attr_trace_selected(mode, &module_name, &attr_name).then_some((module_name, attr_name))
}

fn trace_get_attr() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_GET_ATTR)
}

fn debug_module_get_attr() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("MOLT_DEBUG_MODULE_GET_ATTR").as_deref() == Ok("1"))
}

fn trace_sys_module() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_SYS_MODULE)
}

fn trace_op_silent() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_OP_SILENT)
}

#[cfg(unix)]
fn trace_op_sigtrap_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_OP_SIGTRAP)
}

fn trace_module_globals_mode() -> TraceMode {
    static MODE: OnceLock<TraceMode> = OnceLock::new();
    *MODE.get_or_init(|| trace_mode(std::env::var("MOLT_TRACE_MODULE_GLOBALS").ok().as_deref()))
}

fn trace_bad_module_name() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BAD_MODULE_NAME)
}

fn trace_bad_module_name_backtrace() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BAD_MODULE_NAME_BT)
}

fn trace_bad_module_name_arg(_py: &PyToken<'_>, where_: &str, bits: u64) {
    if !trace_bad_module_name() {
        return;
    }
    let obj = obj_from_bits(bits);
    let type_name = type_name(_py, obj);
    let rendered = format_obj_str(_py, obj);
    if let Some((file, line, func, _, _)) = crate::builtins::frames::frame_stack_top_info(_py) {
        eprintln!(
            "molt bad module name where={} type={} value={} frame={} file={} line={}",
            where_, type_name, rendered, func, file, line
        );
    } else {
        eprintln!(
            "molt bad module name where={} type={} value={} frame=<none>",
            where_, type_name, rendered
        );
    }
    if trace_bad_module_name_backtrace() {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("{bt}");
    }
}

const MODULES_OBJECT_SLOT_COUNT: usize = 14;

pub(crate) struct ModulesRuntimeState {
    /// Serializes only user-visible process-state transitions made by
    /// runpy (`sys.argv`, `sys.path`, and temporary `sys.modules` aliases).
    /// Ordinary fresh execution is coordinated per ModuleId by ModuleTable.
    sys_transition_lock: Mutex<()>,
    copyreg_dispatch_table_bits: AtomicU64,
    copyreg_extension_registry_bits: AtomicU64,
    copyreg_inverted_registry_bits: AtomicU64,
    copyreg_extension_cache_bits: AtomicU64,
    copyreg_constructor_registry_bits: AtomicU64,
    runpy_import_dunder_name: AtomicU64,
    module_path_name: AtomicU64,
    module_name_name: AtomicU64,
    module_file_name: AtomicU64,
    module_package_name: AtomicU64,
    module_cached_name: AtomicU64,
    module_spec_name: AtomicU64,
    module_doc_name: AtomicU64,
    module_loader_name: AtomicU64,
}

impl ModulesRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            sys_transition_lock: Mutex::new(()),
            copyreg_dispatch_table_bits: AtomicU64::new(0),
            copyreg_extension_registry_bits: AtomicU64::new(0),
            copyreg_inverted_registry_bits: AtomicU64::new(0),
            copyreg_extension_cache_bits: AtomicU64::new(0),
            copyreg_constructor_registry_bits: AtomicU64::new(0),
            runpy_import_dunder_name: AtomicU64::new(0),
            module_path_name: AtomicU64::new(0),
            module_name_name: AtomicU64::new(0),
            module_file_name: AtomicU64::new(0),
            module_package_name: AtomicU64::new(0),
            module_cached_name: AtomicU64::new(0),
            module_spec_name: AtomicU64::new(0),
            module_doc_name: AtomicU64::new(0),
            module_loader_name: AtomicU64::new(0),
        }
    }

    fn object_slots(&self) -> [&AtomicU64; MODULES_OBJECT_SLOT_COUNT] {
        [
            &self.copyreg_dispatch_table_bits,
            &self.copyreg_extension_registry_bits,
            &self.copyreg_inverted_registry_bits,
            &self.copyreg_extension_cache_bits,
            &self.copyreg_constructor_registry_bits,
            &self.runpy_import_dunder_name,
            &self.module_path_name,
            &self.module_name_name,
            &self.module_file_name,
            &self.module_package_name,
            &self.module_cached_name,
            &self.module_spec_name,
            &self.module_doc_name,
            &self.module_loader_name,
        ]
    }
}

fn modules_state(_py: &PyToken<'_>) -> &'static ModulesRuntimeState {
    &runtime_state(_py).modules
}

pub(crate) fn modules_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let slots = state.modules.object_slots();
    crate::state::cache::clear_atomic_slots(_py, &slots)
}

static TRACE_LAST_OP: AtomicU64 = AtomicU64::new(0);
#[cfg(unix)]
static TRACE_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
unsafe extern "C" fn trace_sigtrap_handler(sig: i32) {
    unsafe {
        let op = TRACE_LAST_OP.load(Ordering::Relaxed);
        let mut buf = [0u8; 64];
        let prefix = b"molt trace last op=";
        let mut idx = 0usize;
        buf[..prefix.len()].copy_from_slice(prefix);
        idx += prefix.len();
        let mut value = op;
        let mut digits = [0u8; 20];
        let mut len = 0usize;
        if value == 0 {
            digits[0] = b'0';
            len = 1;
        } else {
            while value > 0 {
                digits[len] = b'0' + (value % 10) as u8;
                value /= 10;
                len += 1;
            }
        }
        for i in 0..len {
            buf[idx + i] = digits[len - 1 - i];
        }
        idx += len;
        buf[idx] = b'\n';
        idx += 1;
        let _ = libc::write(2, buf.as_ptr() as *const _, idx);
        libc::_exit(128 + sig);
    }
}

#[cfg(unix)]
fn ensure_sigtrap_handler() {
    if trace_op_sigtrap_enabled() && !TRACE_SIGTRAP_INSTALLED.swap(true, Ordering::Relaxed) {
        unsafe {
            libc::signal(libc::SIGTRAP, trace_sigtrap_handler as *const () as usize);
        }
    }
}

#[cfg(not(unix))]
fn ensure_sigtrap_handler() {}

unsafe fn sys_populate_argv_executable(_py: &PyToken<'_>, sys_ptr: *mut u8) -> Result<(), ()> {
    unsafe {
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                raise_exception::<()>(_py, "SystemError", "sys module has no dictionary storage");
                return Err(());
            }
        };
        let argv_key_ptr = alloc_string(_py, b"argv");
        let exec_key_ptr = alloc_string(_py, b"executable");
        if argv_key_ptr.is_null() || exec_key_ptr.is_null() {
            return Err(());
        }
        let argv_key_bits = MoltObject::from_ptr(argv_key_ptr).bits();
        let exec_key_bits = MoltObject::from_ptr(exec_key_ptr).bits();

        let projection = crate::object::ops_sys::with_process_argv(_py, |args| {
            let exec_val = crate::builtins::platform::startup_executable(_py)
                .unwrap_or_else(|| args.first().cloned().unwrap_or_default());
            let mut elems = Vec::with_capacity(args.len());
            for arg in args {
                let ptr = alloc_string(_py, arg);
                if ptr.is_null() {
                    for bits in elems {
                        dec_ref_bits(_py, bits);
                    }
                    return None;
                }
                elems.push(MoltObject::from_ptr(ptr).bits());
            }
            Some((exec_val, elems))
        });
        let Some((exec_val, elems)) = projection else {
            dec_ref_bits(_py, argv_key_bits);
            dec_ref_bits(_py, exec_key_bits);
            return Err(());
        };

        let argv_list_ptr = alloc_list(_py, &elems);
        if argv_list_ptr.is_null() {
            for bits in elems {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, argv_key_bits);
            dec_ref_bits(_py, exec_key_bits);
            return Err(());
        }
        let argv_list_bits = MoltObject::from_ptr(argv_list_ptr).bits();
        for bits in elems {
            dec_ref_bits(_py, bits);
        }
        if !dict_set_in_place(_py, dict_ptr, argv_key_bits, argv_list_bits) {
            dec_ref_bits(_py, argv_list_bits);
            dec_ref_bits(_py, argv_key_bits);
            dec_ref_bits(_py, exec_key_bits);
            return Err(());
        }

        let exec_val_ptr = alloc_string(_py, &exec_val);
        if exec_val_ptr.is_null() {
            dec_ref_bits(_py, argv_list_bits);
            dec_ref_bits(_py, argv_key_bits);
            dec_ref_bits(_py, exec_key_bits);
            return Err(());
        }
        let exec_val_bits = MoltObject::from_ptr(exec_val_ptr).bits();
        let stored = dict_set_in_place(_py, dict_ptr, exec_key_bits, exec_val_bits);

        dec_ref_bits(_py, argv_list_bits);
        dec_ref_bits(_py, exec_val_bits);
        dec_ref_bits(_py, argv_key_bits);
        dec_ref_bits(_py, exec_key_bits);
        if stored { Ok(()) } else { Err(()) }
    }
}

/// Reconcile an already-published `sys` module with the process arguments
/// installed by the native entrypoint.  Runtime bootstrap can publish `sys`
/// before `wmain`/`main` calls `molt_set_argv`; leaving the first empty
/// snapshot in place makes `sys.argv` depend on module/link initialization
/// order.  The process argument store and Python-visible attributes therefore
/// move as one transaction regardless of when `sys` was materialized.
pub(crate) unsafe fn refresh_sys_argv_executable(_py: &PyToken<'_>) -> Result<(), ()> {
    unsafe {
        let Some(sys_bits) = interpreter_sys_module(_py) else {
            return Ok(());
        };
        inc_ref_bits(_py, sys_bits);

        let result = obj_from_bits(sys_bits)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == TYPE_ID_MODULE)
            .map_or(Ok(()), |sys_ptr| sys_populate_argv_executable(_py, sys_ptr));
        dec_ref_bits(_py, sys_bits);
        result
    }
}

unsafe fn sys_populate_stdio(_py: &PyToken<'_>, sys_ptr: *mut u8) -> Result<(), ()> {
    unsafe {
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                raise_exception::<()>(_py, "SystemError", "sys module has no dictionary storage");
                return Err(());
            }
        };

        let mut keys: Vec<u64> = Vec::with_capacity(6);
        let stdin_key_bits = {
            let ptr = alloc_string(_py, b"stdin");
            if ptr.is_null() {
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };
        let stdout_key_bits = {
            let ptr = alloc_string(_py, b"stdout");
            if ptr.is_null() {
                for bits in keys {
                    dec_ref_bits(_py, bits);
                }
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };
        let stderr_key_bits = {
            let ptr = alloc_string(_py, b"stderr");
            if ptr.is_null() {
                for bits in keys {
                    dec_ref_bits(_py, bits);
                }
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };
        let dunder_stdin_bits = {
            let ptr = alloc_string(_py, b"__stdin__");
            if ptr.is_null() {
                for bits in keys {
                    dec_ref_bits(_py, bits);
                }
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };
        let dunder_stdout_bits = {
            let ptr = alloc_string(_py, b"__stdout__");
            if ptr.is_null() {
                for bits in keys {
                    dec_ref_bits(_py, bits);
                }
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };
        let dunder_stderr_bits = {
            let ptr = alloc_string(_py, b"__stderr__");
            if ptr.is_null() {
                for bits in keys {
                    dec_ref_bits(_py, bits);
                }
                return Err(());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            keys.push(bits);
            bits
        };

        let stdin_bits = molt_sys_stdin();
        if obj_from_bits(stdin_bits).is_none() {
            for bits in keys {
                dec_ref_bits(_py, bits);
            }
            return Err(());
        }
        let stdout_bits = molt_sys_stdout();
        if obj_from_bits(stdout_bits).is_none() {
            dec_ref_bits(_py, stdin_bits);
            for bits in keys {
                dec_ref_bits(_py, bits);
            }
            return Err(());
        }
        let stderr_bits = molt_sys_stderr();
        if obj_from_bits(stderr_bits).is_none() {
            dec_ref_bits(_py, stdin_bits);
            dec_ref_bits(_py, stdout_bits);
            for bits in keys {
                dec_ref_bits(_py, bits);
            }
            return Err(());
        }

        let stored = dict_set_in_place(_py, dict_ptr, stdin_key_bits, stdin_bits)
            && dict_set_in_place(_py, dict_ptr, dunder_stdin_bits, stdin_bits)
            && dict_set_in_place(_py, dict_ptr, stdout_key_bits, stdout_bits)
            && dict_set_in_place(_py, dict_ptr, dunder_stdout_bits, stdout_bits)
            && dict_set_in_place(_py, dict_ptr, stderr_key_bits, stderr_bits)
            && dict_set_in_place(_py, dict_ptr, dunder_stderr_bits, stderr_bits);

        dec_ref_bits(_py, stdin_bits);
        dec_ref_bits(_py, stdout_bits);
        dec_ref_bits(_py, stderr_bits);
        for bits in keys {
            dec_ref_bits(_py, bits);
        }
        if stored { Ok(()) } else { Err(()) }
    }
}

unsafe fn sys_set_owned_attr(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key: &str,
    value_bits: u64,
) -> Result<(), ()> {
    if obj_from_bits(value_bits).is_none() {
        return Err(());
    }
    let result = unsafe { dict_set_str_key_bits(_py, dict_ptr, key, value_bits) };
    dec_ref_bits(_py, value_bits);
    result.map_err(|_| ())
}

pub(crate) unsafe fn sys_populate_version_metadata(
    _py: &PyToken<'_>,
    sys_ptr: *mut u8,
) -> Result<(), ()> {
    unsafe {
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                raise_exception::<()>(_py, "SystemError", "sys module has no dictionary storage");
                return Err(());
            }
        };

        let platform = crate::molt_sys_platform();
        let windows = string_obj_to_owned(obj_from_bits(platform))
            .is_some_and(|value| value.starts_with("win"));
        sys_set_owned_attr(_py, dict_ptr, "platform", platform)?;
        sys_set_owned_attr(_py, dict_ptr, "version", crate::molt_sys_version())?;
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "version_info",
            crate::molt_sys_version_info(),
        )?;
        sys_set_owned_attr(_py, dict_ptr, "hexversion", crate::molt_sys_hexversion())?;
        sys_set_owned_attr(_py, dict_ptr, "api_version", crate::molt_sys_api_version())?;
        if !windows {
            sys_set_owned_attr(_py, dict_ptr, "abiflags", crate::molt_sys_abiflags())?;
        }
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "implementation",
            crate::molt_sys_implementation_payload(),
        )?;
        Ok(())
    }
}

unsafe fn sys_populate_bootstrap_metadata(_py: &PyToken<'_>, sys_ptr: *mut u8) -> Result<(), ()> {
    unsafe {
        sys_populate_version_metadata(_py, sys_ptr)?;
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                raise_exception::<()>(_py, "SystemError", "sys module has no dictionary storage");
                return Err(());
            }
        };

        sys_set_owned_attr(_py, dict_ptr, "maxsize", crate::molt_sys_maxsize())?;
        sys_set_owned_attr(_py, dict_ptr, "maxunicode", crate::molt_sys_maxunicode())?;
        sys_set_owned_attr(_py, dict_ptr, "byteorder", crate::molt_sys_byteorder())?;
        sys_set_owned_attr(_py, dict_ptr, "prefix", crate::molt_sys_prefix())?;
        sys_set_owned_attr(_py, dict_ptr, "exec_prefix", crate::molt_sys_exec_prefix())?;
        sys_set_owned_attr(_py, dict_ptr, "base_prefix", crate::molt_sys_base_prefix())?;
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "base_exec_prefix",
            crate::molt_sys_base_exec_prefix(),
        )?;
        sys_set_owned_attr(_py, dict_ptr, "platlibdir", crate::molt_sys_platlibdir())?;
        sys_set_owned_attr(_py, dict_ptr, "path", crate::molt_sys_path())?;
        sys_set_owned_attr(_py, dict_ptr, "orig_argv", crate::molt_sys_orig_argv())?;
        sys_set_owned_attr(_py, dict_ptr, "copyright", crate::molt_sys_copyright())?;
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "stdlib_module_names",
            crate::molt_sys_stdlib_module_names(),
        )?;
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "builtin_module_names",
            crate::molt_sys_builtin_module_names(),
        )?;

        let builtin_finder = crate::builtins::types::compiled_loader_builtin_class(_py);
        if builtin_finder == 0 || exception_pending(_py) {
            return Err(());
        }
        let meta_path_ptr = alloc_list(_py, &[builtin_finder]);
        if meta_path_ptr.is_null() {
            return Err(());
        }
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "meta_path",
            MoltObject::from_ptr(meta_path_ptr).bits(),
        )?;

        let path_hooks_ptr = alloc_list(_py, &[]);
        if path_hooks_ptr.is_null() {
            return Err(());
        }
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "path_hooks",
            MoltObject::from_ptr(path_hooks_ptr).bits(),
        )?;

        let path_importer_cache_ptr = alloc_dict_with_pairs(_py, &[]);
        if path_importer_cache_ptr.is_null() {
            return Err(());
        }
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "path_importer_cache",
            MoltObject::from_ptr(path_importer_cache_ptr).bits(),
        )?;

        crate::builtins::platform::importlib_seed_default_generation(
            _py,
            MoltObject::from_ptr(meta_path_ptr).bits(),
            MoltObject::from_ptr(path_hooks_ptr).bits(),
        );

        Ok(())
    }
}

/// Read one namespace item without overloading an object value as a miss/error
/// sentinel. Exact dictionaries keep their borrowed fast path; subclasses and
/// custom mappings retain Python's observable `__getitem__` dispatch.
fn lookup_namespace_item(
    _py: &PyToken<'_>,
    namespace_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, ()> {
    if let Some(ptr) = obj_from_bits(namespace_bits).as_ptr()
        && unsafe { crate::object_is_exact_builtin_dict(_py, ptr) }
    {
        let value = unsafe { dict_get_in_place(_py, ptr, name_bits) };
        let value = match value {
            Ok(value) => value,
            Err(()) => {
                return Err(());
            }
        };
        if let Some(value) = value {
            inc_ref_bits(_py, value);
        }
        return Ok(value);
    }

    let value = crate::molt_index(namespace_bits, name_bits);
    if !exception_pending(_py) {
        return Ok(Some(value));
    }
    dec_ref_bits(_py, value);
    let exception = molt_exception_last_pending();
    let absent =
        crate::builtins::exceptions::exception_matches_builtin_name(_py, exception, "KeyError");
    dec_ref_bits(_py, exception);
    if !absent {
        return Err(());
    }
    clear_exception(_py);
    Ok(None)
}

pub(crate) fn lookup_builtin_global(
    _py: &PyToken<'_>,
    name_bits: u64,
    builtins_bits: u64,
) -> Result<Option<u64>, ()> {
    if builtins_bits == 0 {
        return Ok(None);
    }
    lookup_namespace_item(_py, builtins_bits, name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_new(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            trace_bad_module_name_arg(_py, "module_new_ptr", name_bits);
            return raise_exception::<_>(_py, "TypeError", "module name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                trace_bad_module_name_arg(_py, "module_new_type", name_bits);
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        }
        let _name = match string_obj_to_owned(name_obj) {
            Some(val) => val,
            None => {
                trace_bad_module_name_arg(_py, "module_new_utf8", name_bits);
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        };
        if let Some(bits) = execution::module_new_target(_py, &_name) {
            return bits;
        }
        let ptr = alloc_module_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        crate::intrinsics::install_into_builtins(_py, ptr);
        if exception_pending(_py) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// One owned observation of the canonical public dictionary, independent of
/// explicit loader/runpy execution contexts.
pub(crate) enum PublicModuleCache {
    Missing,
    Present(u64),
}

pub(crate) fn public_module_cache_lookup(
    py: &PyToken<'_>,
    name: &str,
) -> Result<PublicModuleCache, u64> {
    Ok(
        match crate::builtins::module_table::module_lookup(py, name)? {
            Some(bits) => PublicModuleCache::Present(bits),
            None => PublicModuleCache::Missing,
        },
    )
}

fn module_publication_admits(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { matches!(object_type_id(ptr), TYPE_ID_MODULE | TYPE_ID_DICT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            trace_bad_module_name_arg(_py, "module_cache_get_ptr", name_bits);
            return raise_exception::<_>(_py, "TypeError", "module name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                trace_bad_module_name_arg(_py, "module_cache_get_type", name_bits);
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        }
        let name_bytes =
            unsafe { std::slice::from_raw_parts(string_bytes(name_ptr), string_len(name_ptr)) };
        let name_owned;
        let name = if let Ok(val) = std::str::from_utf8(name_bytes) {
            val
        } else {
            name_owned = match string_obj_to_owned(name_obj) {
                Some(val) => val,
                None => {
                    trace_bad_module_name_arg(_py, "module_cache_get_utf8", name_bits);
                    return raise_exception::<_>(_py, "TypeError", "module name must be str");
                }
            };
            name_owned.as_str()
        };
        // Before and after sys bootstrap, the same dictionary owns deletion,
        // replacement and arbitrary public values.
        match public_module_cache_lookup(_py, name) {
            Ok(PublicModuleCache::Present(bits)) => bits,
            Ok(PublicModuleCache::Missing) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_import(name_bits: u64) -> u64 {
    molt_module_import_inner(name_bits)
}

/// Missing includes the provider's diagnostic name without becoming an
/// execution exception. Imported transfers one owner, even for a None result.
#[derive(Debug)]
pub(crate) enum ModuleImportOutcome {
    Imported(u64),
    Missing { diagnostic_name: String },
}

fn registered_module_import(
    py: &PyToken<'_>,
    id: u32,
    observed: PublicModuleCache,
) -> Result<ModuleImportOutcome, u64> {
    let bits = crate::builtins::module_table::module_ensure_with_cache(py, id, Some(observed));
    if exception_pending(py) {
        dec_ref_bits(py, bits);
        return Err(MoltObject::none().bits());
    }
    // ensure owns completed result and publication effects. Never
    // recanonicalize after callbacks replace/delete the public entry.
    Ok(ModuleImportOutcome::Imported(bits))
}

fn molt_module_import_inner(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match module_import_attempt(name_bits) {
            Ok(ModuleImportOutcome::Imported(bits)) => bits,
            Ok(ModuleImportOutcome::Missing { diagnostic_name }) => {
                let Some(name) = attr_name_bits_from_bytes(py, diagnostic_name.as_bytes()) else {
                    return MoltObject::none().bits();
                };
                let error = crate::builtins::exceptions::raise_module_not_found(
                    py,
                    format!("No module named '{diagnostic_name}'").as_bytes(),
                    name,
                );
                dec_ref_bits(py, name);
                error
            }
            Err(error) => error,
        }
    })
}

/// Public dynamic/fromlist/isolate imports use exactly the same cache and
/// ordered finder transaction as importlib. Missing remains a typed outcome so
/// a fromlist probe can distinguish absence from a loader's actual exception.
pub(crate) fn module_import_attempt(name_bits: u64) -> Result<ModuleImportOutcome, u64> {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return Err(raise_exception::<_>(
                py,
                "TypeError",
                "module name must be str",
            ));
        };
        let Some(dictionary) = crate::builtins::module_table::module_dictionary(py) else {
            return Err(MoltObject::none().bits());
        };
        let pointer = ptr_from_bits(dictionary);
        crate::builtins::platform::importlib_import_resolved_attempt(
            py,
            &name,
            name_bits,
            pointer,
            crate::builtins::platform::ImportlibImportMode::Builtin,
        )
    })
}

/// Explicit runtime-support bootstrap. A catalog body is an internal execution
/// capability, never permission to bypass finders for an ordinary Python import.
pub(crate) fn catalog_import_attempt(name_bits: u64) -> Result<ModuleImportOutcome, u64> {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return Err(raise_exception::<_>(
                py,
                "TypeError",
                "module name must be str",
            ));
        };
        let observed = public_module_cache_lookup(py, &name)?;
        if let PublicModuleCache::Present(bits) = observed {
            if let Some(id) = crate::builtins::module_table::module_id_of(&name) {
                return registered_module_import(py, id, PublicModuleCache::Present(bits));
            }
            let Some(dictionary) = crate::builtins::module_table::module_dictionary(py) else {
                dec_ref_bits(py, bits);
                return Err(MoltObject::none().bits());
            };
            return match crate::builtins::module_table::resolve_cached_module(
                py,
                &name,
                Some(name_bits),
                dictionary,
                bits,
                crate::builtins::module_table::ModuleImportPolicy::CImport,
            )? {
                crate::builtins::module_table::CachedModuleOutcome::Return(bits) => {
                    Ok(ModuleImportOutcome::Imported(bits))
                }
                crate::builtins::module_table::CachedModuleOutcome::Reload => {
                    let Some(_depth) = crate::builtins::module_table::enter_import_recursion(py)
                    else {
                        return Err(MoltObject::none().bits());
                    };
                    catalog_import_attempt(name_bits)
                }
                crate::builtins::module_table::CachedModuleOutcome::Missing => {
                    unreachable!("ordinary import may reload")
                }
            };
        }
        if let Some(id) = crate::builtins::module_table::module_id_of(&name) {
            return registered_module_import(py, id, observed);
        }
        Ok(ModuleImportOutcome::Missing {
            diagnostic_name: name,
        })
    })
}

unsafe fn dict_set_str_key_bits(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key: &str,
    value_bits: u64,
) -> Result<(), u64> {
    unsafe {
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let completed = dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
        dec_ref_bits(_py, key_bits);
        if completed {
            Ok(())
        } else {
            Err(MoltObject::none().bits())
        }
    }
}

fn copyreg_dict_slot_bits(_py: &PyToken<'_>, slot: &AtomicU64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn copyreg_set_slot_bits(_py: &PyToken<'_>, slot: &AtomicU64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::object::builders::alloc_set_with_entries(_py, &[]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn copyreg_dispatch_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &modules_state(_py).copyreg_dispatch_table_bits);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_extension_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &modules_state(_py).copyreg_extension_registry_bits);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_inverted_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &modules_state(_py).copyreg_inverted_registry_bits);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_extension_cache_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &modules_state(_py).copyreg_extension_cache_bits);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_constructor_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_set_slot_bits(_py, &modules_state(_py).copyreg_constructor_registry_bits);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_SET {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_extension_key_bits(_py: &PyToken<'_>, module_bits: u64, name_bits: u64) -> Option<u64> {
    let key_ptr = alloc_tuple(_py, &[module_bits, name_bits]);
    if key_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(key_ptr).bits())
}

fn copyreg_add_extension_code_int(_py: &PyToken<'_>, code_bits: u64) -> Result<u64, u64> {
    let int_code_bits = molt_int_from_obj(code_bits, MoltObject::none().bits(), 0);
    let mut owner = obj_from_bits(int_code_bits)
        .as_ptr()
        .map(crate::PtrDropGuard::new);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(code) = crate::builtins::numbers::integer_storage_i64(_py, int_code_bits) else {
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Err(raise_exception::<_>(_py, "ValueError", "code out of range"));
    };
    if !(1..=0x7fff_ffff).contains(&code) {
        return Err(raise_exception::<_>(_py, "ValueError", "code out of range"));
    }
    if let Some(owner) = &mut owner {
        owner.release();
    }
    Ok(int_code_bits)
}

fn copyreg_add_constructor(_py: &PyToken<'_>, func_bits: u64) -> Result<(), u64> {
    let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(func_bits)));
    if !callable_ok {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "constructors must be callable",
        ));
    }
    let Some(set_ptr) = copyreg_constructor_registry_ptr(_py) else {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "copyreg constructor registry unavailable",
        ));
    };
    if !unsafe { set_add_in_place(_py, set_ptr, func_bits, HashContext::SetElement) } {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn copyreg_attr_optional(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if clear_attribute_error_if_pending(_py) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, value_bits) {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn copyreg_attr_required(_py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<u64, u64> {
    match copyreg_attr_optional(_py, obj_bits, name)? {
        Some(bits) => Ok(bits),
        None => {
            let name_text = std::str::from_utf8(name).unwrap_or("attribute");
            let msg = format!("copyreg: missing required attribute {name_text}");
            Err(raise_exception::<_>(_py, "AttributeError", &msg))
        }
    }
}

fn copyreg_class_name(_py: &PyToken<'_>, cls_bits: u64) -> String {
    if let Ok(Some(name_bits)) = copyreg_attr_optional(_py, cls_bits, b"__name__") {
        let name = string_obj_to_owned(obj_from_bits(name_bits))
            .unwrap_or_else(|| type_name(_py, obj_from_bits(cls_bits)).to_string());
        dec_ref_bits(_py, name_bits);
        return name;
    }
    type_name(_py, obj_from_bits(cls_bits)).to_string()
}

fn copyreg_slots_truthy(_py: &PyToken<'_>, obj_bits: u64) -> Result<bool, u64> {
    if let Some(slots_bits) = copyreg_attr_optional(_py, obj_bits, b"__slots__")? {
        let truthy = is_truthy(_py, obj_from_bits(slots_bits));
        dec_ref_bits(_py, slots_bits);
        return Ok(truthy);
    }
    Ok(false)
}

fn copyreg_reconstructor_bits(_py: &PyToken<'_>) -> Result<u64, u64> {
    let module_ptr = alloc_string(_py, b"copyreg");
    if module_ptr.is_null() {
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();
    let imported_bits = crate::molt_module_import(module_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let name_ptr = alloc_string(_py, b"_reconstructor");
    if name_ptr.is_null() {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let value_bits = crate::molt_object_getattribute(imported_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_bootstrap() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_state = modules_state(_py);
        let dispatch_bits = copyreg_dict_slot_bits(_py, &module_state.copyreg_dispatch_table_bits);
        let extension_bits =
            copyreg_dict_slot_bits(_py, &module_state.copyreg_extension_registry_bits);
        let inverted_bits =
            copyreg_dict_slot_bits(_py, &module_state.copyreg_inverted_registry_bits);
        let cache_bits = copyreg_dict_slot_bits(_py, &module_state.copyreg_extension_cache_bits);
        let constructor_bits =
            copyreg_set_slot_bits(_py, &module_state.copyreg_constructor_registry_bits);
        if obj_from_bits(dispatch_bits).is_none()
            || obj_from_bits(extension_bits).is_none()
            || obj_from_bits(inverted_bits).is_none()
            || obj_from_bits(cache_bits).is_none()
            || obj_from_bits(constructor_bits).is_none()
        {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        let state_ptr = alloc_tuple(
            _py,
            &[
                dispatch_bits,
                extension_bits,
                inverted_bits,
                cache_bits,
                constructor_bits,
            ],
        );
        if state_ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(state_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_pickle(
    cls_bits: u64,
    reducer_bits: u64,
    constructor_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(reducer_bits)));
        if !callable_ok {
            return raise_exception::<_>(_py, "TypeError", "reduction functions must be callable");
        }
        let Some(dispatch_ptr) = copyreg_dispatch_ptr(_py) else {
            return raise_exception::<_>(_py, "RuntimeError", "copyreg dispatch table unavailable");
        };
        let completed = unsafe { dict_set_in_place(_py, dispatch_ptr, cls_bits, reducer_bits) };
        if !completed {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(constructor_bits).is_none()
            && let Err(err_bits) = copyreg_add_constructor(_py, constructor_bits)
        {
            return err_bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_constructor(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err_bits) = copyreg_add_constructor(_py, func_bits) {
            return err_bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_newobj(cls_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let new_bits = match copyreg_attr_required(_py, cls_bits, b"__new__") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let builder_bits = molt_callargs_new(0, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, new_bits);
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        let _ = unsafe { molt_callargs_push_pos(builder_bits, cls_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return MoltObject::none().bits();
        }
        let _ = unsafe { molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return MoltObject::none().bits();
        }
        let out_bits = molt_call_bind(new_bits, builder_bits);
        dec_ref_bits(_py, new_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_newobj_ex(cls_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let new_bits = match copyreg_attr_required(_py, cls_bits, b"__new__") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let builder_bits = molt_callargs_new(0, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, new_bits);
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        let _ = unsafe { molt_callargs_push_pos(builder_bits, cls_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return MoltObject::none().bits();
        }
        let _ = unsafe { molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return MoltObject::none().bits();
        }
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return MoltObject::none().bits();
        }
        let out_bits = molt_call_bind(new_bits, builder_bits);
        dec_ref_bits(_py, new_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_reconstructor(
    cls_bits: u64,
    base_bits: u64,
    state_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let object_bits = builtins.object;
        let obj_bits = if base_bits == object_bits {
            let new_bits = match copyreg_attr_required(_py, object_bits, b"__new__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            let builder_bits = molt_callargs_new(0, 0);
            if builder_bits == 0 {
                dec_ref_bits(_py, new_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
            let _ = unsafe { molt_callargs_push_pos(builder_bits, cls_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return MoltObject::none().bits();
            }
            let out_bits = molt_call_bind(new_bits, builder_bits);
            dec_ref_bits(_py, new_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            out_bits
        } else {
            let new_bits = match copyreg_attr_required(_py, base_bits, b"__new__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            let builder_bits = molt_callargs_new(0, 0);
            if builder_bits == 0 {
                dec_ref_bits(_py, new_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
            let _ = unsafe { molt_callargs_push_pos(builder_bits, cls_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return MoltObject::none().bits();
            }
            let _ = unsafe { molt_callargs_push_pos(builder_bits, state_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return MoltObject::none().bits();
            }
            let out_bits = molt_call_bind(new_bits, builder_bits);
            dec_ref_bits(_py, new_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            out_bits
        };
        if base_bits != object_bits {
            let base_init_bits = match copyreg_attr_optional(_py, base_bits, b"__init__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            let object_init_bits = match copyreg_attr_optional(_py, object_bits, b"__init__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            match (base_init_bits, object_init_bits) {
                (Some(base_bits), Some(object_bits)) => {
                    let needs = base_bits != object_bits;
                    dec_ref_bits(_py, object_bits);
                    if !needs {
                        dec_ref_bits(_py, base_bits);
                        return obj_bits;
                    }
                    let out = unsafe { call_callable2(_py, base_bits, obj_bits, state_bits) };
                    dec_ref_bits(_py, base_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, out);
                }
                (Some(base_bits), None) => {
                    let out = unsafe { call_callable2(_py, base_bits, obj_bits, state_bits) };
                    dec_ref_bits(_py, base_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, out);
                }
                (None, Some(object_bits)) => {
                    dec_ref_bits(_py, object_bits);
                }
                (None, None) => {}
            };
        }
        obj_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_reduce_ex(self_bits: u64, proto_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let proto_int_bits = molt_int_from_obj(proto_bits, MoltObject::none().bits(), 0);
        let proto_owner = obj_from_bits(proto_int_bits)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(proto) = crate::builtins::numbers::integer_storage_i64(_py, proto_int_bits) else {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(_py, "TypeError", "proto must be int");
        };
        drop(proto_owner);
        if proto >= 2 {
            return raise_exception::<_>(_py, "AssertionError", "");
        }

        let builtins = builtin_classes(_py);
        let cls_bits = type_of_bits(_py, self_bits);
        let mut base_bits = builtins.object;
        let mut found_base = false;
        let mro = class_mro_vec(cls_bits);
        for candidate_bits in mro {
            let flags_bits = match copyreg_attr_optional(_py, candidate_bits, b"__flags__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            if let Some(flags_bits) = flags_bits {
                let input_owner = obj_from_bits(flags_bits)
                    .as_ptr()
                    .map(crate::PtrDropGuard::new);
                let flags_int_bits = molt_int_from_obj(flags_bits, MoltObject::none().bits(), 0);
                let flags_owner = obj_from_bits(flags_int_bits)
                    .as_ptr()
                    .map(crate::PtrDropGuard::new);
                drop(input_owner);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let Some(flags) =
                    crate::builtins::numbers::integer_storage_i64(_py, flags_int_bits)
                else {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "__flags__ must be int");
                };
                drop(flags_owner);
                if (flags & 0x200) == 0 {
                    base_bits = candidate_bits;
                    found_base = true;
                    break;
                }
            }
            let new_bits = match copyreg_attr_optional(_py, candidate_bits, b"__new__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            if let Some(new_bits) = new_bits {
                let new_type_bits = type_of_bits(_py, new_bits);
                if builtins.is_builtin_callable_class(new_type_bits) {
                    let self_obj_bits = match copyreg_attr_optional(_py, new_bits, b"__self__") {
                        Ok(bits) => bits,
                        Err(err_bits) => return err_bits,
                    };
                    if let Some(self_obj_bits) = self_obj_bits {
                        let matches = self_obj_bits == candidate_bits;
                        dec_ref_bits(_py, self_obj_bits);
                        if matches {
                            dec_ref_bits(_py, new_bits);
                            base_bits = candidate_bits;
                            found_base = true;
                            break;
                        }
                    }
                }
                dec_ref_bits(_py, new_bits);
            }
        }
        if !found_base {
            base_bits = builtins.object;
        }

        let state_bits = if base_bits == builtins.object {
            MoltObject::none().bits()
        } else {
            if base_bits == cls_bits {
                let class_name = copyreg_class_name(_py, cls_bits);
                let msg = format!("cannot pickle '{class_name}' object");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let out_bits = unsafe { call_callable1(_py, base_bits, self_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            out_bits
        };

        let args_ptr = alloc_tuple(_py, &[cls_bits, base_bits, state_bits]);
        if args_ptr.is_null() {
            if !obj_from_bits(state_bits).is_none() {
                dec_ref_bits(_py, state_bits);
            }
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        if !obj_from_bits(state_bits).is_none() {
            dec_ref_bits(_py, state_bits);
        }

        let mut dict_bits = MoltObject::none().bits();
        let mut include_state = false;
        let getstate_bits = match copyreg_attr_optional(_py, self_bits, b"__getstate__") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        if let Some(getstate_bits) = getstate_bits {
            let slots_truthy = match copyreg_slots_truthy(_py, self_bits) {
                Ok(value) => value,
                Err(err_bits) => return err_bits,
            };
            if slots_truthy {
                let type_getstate = match copyreg_attr_optional(_py, cls_bits, b"__getstate__") {
                    Ok(bits) => bits,
                    Err(err_bits) => return err_bits,
                };
                let object_getstate =
                    match copyreg_attr_optional(_py, builtins.object, b"__getstate__") {
                        Ok(bits) => bits,
                        Err(err_bits) => return err_bits,
                    };
                if let (Some(type_bits), Some(object_bits)) = (type_getstate, object_getstate) {
                    let matches = type_bits == object_bits;
                    dec_ref_bits(_py, type_bits);
                    dec_ref_bits(_py, object_bits);
                    if matches {
                        dec_ref_bits(_py, getstate_bits);
                        dec_ref_bits(_py, args_bits);
                        let msg = "a class that defines __slots__ without defining __getstate__ cannot be pickled";
                        return raise_exception::<_>(_py, "TypeError", msg);
                    }
                } else {
                    if let Some(type_bits) = type_getstate {
                        dec_ref_bits(_py, type_bits);
                    }
                    if let Some(object_bits) = object_getstate {
                        dec_ref_bits(_py, object_bits);
                    }
                }
            }
            let out_bits = unsafe { call_callable0(_py, getstate_bits) };
            dec_ref_bits(_py, getstate_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, args_bits);
                return MoltObject::none().bits();
            }
            dict_bits = out_bits;
            include_state = is_truthy(_py, obj_from_bits(dict_bits));
        } else {
            let slots_truthy = match copyreg_slots_truthy(_py, self_bits) {
                Ok(value) => value,
                Err(err_bits) => return err_bits,
            };
            if slots_truthy {
                let class_name = copyreg_class_name(_py, cls_bits);
                let msg = format!(
                    "cannot pickle '{class_name}' object: a class that defines __slots__ without defining __getstate__ cannot be pickled with protocol {proto}"
                );
                dec_ref_bits(_py, args_bits);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let state_dict_bits = match copyreg_attr_optional(_py, self_bits, b"__dict__") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
            if let Some(state_dict_bits) = state_dict_bits {
                dict_bits = state_dict_bits;
                include_state = is_truthy(_py, obj_from_bits(dict_bits));
            }
        }

        let reconstructor_bits = match copyreg_reconstructor_bits(_py) {
            Ok(bits) => bits,
            Err(err_bits) => {
                if include_state && !obj_from_bits(dict_bits).is_none() {
                    dec_ref_bits(_py, dict_bits);
                }
                dec_ref_bits(_py, args_bits);
                return err_bits;
            }
        };

        let out_ptr = if include_state && !obj_from_bits(dict_bits).is_none() {
            alloc_tuple(_py, &[reconstructor_bits, args_bits, dict_bits])
        } else {
            alloc_tuple(_py, &[reconstructor_bits, args_bits])
        };

        if !obj_from_bits(dict_bits).is_none() {
            dec_ref_bits(_py, dict_bits);
        }
        dec_ref_bits(_py, reconstructor_bits);
        dec_ref_bits(_py, args_bits);

        if out_ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

/// Preserve Python's lookup/compare/truth sequence. Do not preload the other
/// registry: this comparison can mutate it. The dict result stays retained only
/// through its rich operator, then that owned result reaches the truth boundary.
fn copyreg_entry_compare(
    py: &PyToken<'_>,
    dictionary: *mut u8,
    key: u64,
    expected: u64,
    unequal: bool,
) -> Result<bool, ()> {
    use crate::object::ops_compare::{
        CompareBoolOutcome, CompareValueOutcome, comparison_value_to_bool,
    };
    let value = unsafe { dict_get_in_place(py, dictionary, key) }?
        .unwrap_or_else(|| MoltObject::none().bits());
    inc_ref_bits(py, value);
    let compared = if unequal {
        crate::molt_ne(value, expected)
    } else {
        crate::molt_eq(value, expected)
    };
    dec_ref_bits(py, value);
    if exception_pending(py) {
        dec_ref_bits(py, compared);
        return Err(());
    }
    match comparison_value_to_bool(py, CompareValueOutcome::Value(compared)) {
        CompareBoolOutcome::True => Ok(true),
        CompareBoolOutcome::False => Ok(false),
        CompareBoolOutcome::Error | CompareBoolOutcome::NotComparable => Err(()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_add_extension(
    module_bits: u64,
    name_bits: u64,
    code_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(extension_ptr) = copyreg_extension_registry_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension registry unavailable",
            );
        };
        let Some(inverted_ptr) = copyreg_inverted_registry_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension registry unavailable",
            );
        };
        let code_key_bits = match copyreg_add_extension_code_int(_py, code_bits) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let Some(key_bits) = copyreg_extension_key_bits(_py, module_bits, name_bits) else {
            dec_ref_bits(_py, code_key_bits);
            return MoltObject::none().bits();
        };
        let result = (|| {
            let code_matches =
                match copyreg_entry_compare(_py, extension_ptr, key_bits, code_key_bits, false) {
                    Ok(matches) => matches,
                    Err(()) => return MoltObject::none().bits(),
                };
            if code_matches {
                match copyreg_entry_compare(_py, inverted_ptr, code_key_bits, key_bits, false) {
                    Ok(true) | Err(()) => return MoltObject::none().bits(),
                    Ok(false) => {}
                }
            }
            // These are separate Python containment and indexing operations.
            // Re-read after hashing/equality instead of retaining a stale result.
            let existing = unsafe { dict_get_in_place(_py, extension_ptr, key_bits) };
            let existing = match existing {
                Ok(value) => value,
                Err(()) => {
                    return MoltObject::none().bits();
                }
            };
            if existing.is_some() {
                let found = crate::molt_getitem_method(
                    MoltObject::from_ptr(extension_ptr).bits(),
                    key_bits,
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, found);
                    return MoltObject::none().bits();
                }
                let key_text =
                    crate::object::ops_format::format_obj_str_bytes(_py, obj_from_bits(key_bits));
                if exception_pending(_py) {
                    dec_ref_bits(_py, found);
                    return MoltObject::none().bits();
                }
                let code_text =
                    crate::object::ops_format::format_obj_str_bytes(_py, obj_from_bits(found));
                dec_ref_bits(_py, found);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return crate::builtins::exceptions::raise_exception_bytes::<_>(
                    _py,
                    "ValueError",
                    &[
                        b"key ".as_slice(),
                        &key_text,
                        b" is already registered with code ",
                        &code_text,
                    ]
                    .concat(),
                );
            }
            let existing = unsafe { dict_get_in_place(_py, inverted_ptr, code_key_bits) };
            let existing = match existing {
                Ok(value) => value,
                Err(()) => {
                    return MoltObject::none().bits();
                }
            };
            if existing.is_some() {
                let found = crate::molt_getitem_method(
                    MoltObject::from_ptr(inverted_ptr).bits(),
                    code_key_bits,
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, found);
                    return MoltObject::none().bits();
                }
                let code_text = crate::object::ops_format::format_obj_str_bytes(
                    _py,
                    obj_from_bits(code_key_bits),
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, found);
                    return MoltObject::none().bits();
                }
                let key_text =
                    crate::object::ops_format::format_obj_str_bytes(_py, obj_from_bits(found));
                dec_ref_bits(_py, found);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return crate::builtins::exceptions::raise_exception_bytes::<_>(
                    _py,
                    "ValueError",
                    &[
                        b"code ".as_slice(),
                        &code_text,
                        b" is already in use for key ",
                        &key_text,
                    ]
                    .concat(),
                );
            }
            unsafe {
                if !dict_set_in_place(_py, extension_ptr, key_bits, code_key_bits) {
                    return MoltObject::none().bits();
                }
                // This is the last operation; its failure remains selected for
                // the intrinsic's boxed-None return boundary.
                let _ = dict_set_in_place(_py, inverted_ptr, code_key_bits, key_bits);
            }
            MoltObject::none().bits()
        })();
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, code_key_bits);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_remove_extension(
    module_bits: u64,
    name_bits: u64,
    code_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(extension_ptr) = copyreg_extension_registry_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension registry unavailable",
            );
        };
        let Some(inverted_ptr) = copyreg_inverted_registry_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension registry unavailable",
            );
        };
        let Some(cache_ptr) = copyreg_extension_cache_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension cache unavailable",
            );
        };
        let Some(key_bits) = copyreg_extension_key_bits(_py, module_bits, name_bits) else {
            return MoltObject::none().bits();
        };
        let result = (|| {
            let mismatch =
                match copyreg_entry_compare(_py, extension_ptr, key_bits, code_bits, true) {
                    Ok(mismatch) => mismatch,
                    Err(()) => return MoltObject::none().bits(),
                };
            let mismatch = mismatch
                || match copyreg_entry_compare(_py, inverted_ptr, code_bits, key_bits, true) {
                    Ok(mismatch) => mismatch,
                    Err(()) => return MoltObject::none().bits(),
                };
            if mismatch {
                let key_text =
                    crate::object::ops_format::format_obj_str_bytes(_py, obj_from_bits(key_bits));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let code_text =
                    crate::object::ops_format::format_obj_str_bytes(_py, obj_from_bits(code_bits));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return crate::builtins::exceptions::raise_exception_bytes::<_>(
                    _py,
                    "ValueError",
                    &[
                        b"key ".as_slice(),
                        &key_text,
                        b" is not registered with code ",
                        &code_text,
                    ]
                    .concat(),
                );
            }
            for (dictionary, key) in [(extension_ptr, key_bits), (inverted_ptr, code_bits)] {
                let deleted =
                    crate::molt_delitem_method(MoltObject::from_ptr(dictionary).bits(), key);
                dec_ref_bits(_py, deleted);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            let cached = unsafe { dict_get_in_place(_py, cache_ptr, code_bits) };
            let cached = match cached {
                Ok(value) => value,
                Err(()) => {
                    return MoltObject::none().bits();
                }
            };
            if cached.is_some() {
                let deleted =
                    crate::molt_delitem_method(MoltObject::from_ptr(cache_ptr).bits(), code_bits);
                dec_ref_bits(_py, deleted);
            }
            MoltObject::none().bits()
        })();
        dec_ref_bits(_py, key_bits);
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_clear_extension_cache() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(cache_ptr) = copyreg_extension_cache_ptr(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "copyreg extension cache unavailable",
            );
        };
        unsafe {
            crate::dict_clear_in_place(_py, cache_ptr);
        }
        MoltObject::none().bits()
    })
}

/// Borrow the interpreter's sys namespace; imports remain a separate view.
pub(crate) fn interpreter_sys_module(py: &PyToken<'_>) -> Option<u64> {
    runtime_state(py).interpreter_sys.module(py)
}

/// Bootstrap through the existing canonical initializer, never adopt the
/// object returned by a same-named public import replacement.
pub(crate) fn ensure_interpreter_sys_module(py: &PyToken<'_>) -> Option<u64> {
    if let Some(bits) = interpreter_sys_module(py) {
        return Some(bits);
    }
    if !runtime_state(py).interpreter_sys.allows_bootstrap(py)
        || crate::builtins::module_table::module_id_of("sys").is_none()
    {
        return None;
    }
    let name = attr_name_bits_from_bytes(py, b"sys")?;
    let imported = catalog_import_attempt(name);
    dec_ref_bits(py, name);
    match imported {
        Ok(ModuleImportOutcome::Imported(bits)) => {
            dec_ref_bits(py, bits);
            interpreter_sys_module(py)
        }
        Ok(ModuleImportOutcome::Missing { .. }) | Err(_) => None,
    }
}

pub(crate) fn sys_modules_dict_bits(py: &PyToken<'_>, sys_bits: u64) -> Option<u64> {
    let sys_ptr = obj_from_bits(sys_bits).as_ptr()?;
    if unsafe { object_type_id(sys_ptr) } != TYPE_ID_MODULE {
        return None;
    }
    let bits = crate::builtins::module_table::module_dictionary(py)?;
    let name = intern_static_name(py, &runtime_state(py).interned.modules_name, b"modules");
    if name == 0 {
        return None;
    }
    let visible =
        unsafe { crate::object::accessors::instance_attribute_lookup(py, sys_ptr, name, None) }
            .ok()?;
    if let Some(visible) = visible {
        dec_ref_bits(py, visible);
    } else if module_set_attr(py, sys_bits, name, bits).is_err() {
        return None;
    }
    inc_ref_bits(py, bits);
    Some(bits)
}

fn sys_modules_set_canonical_name<'a, 'py>(
    py: &'a PyToken<'py>,
    modules_ptr: *mut u8,
    name: &str,
    module_bits: u64,
) -> Result<crate::object::ops::DetachedDictReferences<'a, 'py>, u64> {
    let Some(key) = attr_name_bits_from_bytes(py, name.as_bytes()) else {
        return Err(MoltObject::none().bits());
    };
    let retired =
        unsafe { crate::object::ops::dict_set_deferred(py, modules_ptr, key, module_bits) };
    dec_ref_bits(py, key);
    retired.map_err(|()| MoltObject::none().bits())
}

/// Publish a borrowed module without transferring either argument's ownership.
/// Every successful path returns None; typed initializer admission owns once-only execution.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    module_cache_publish(
        name_bits,
        module_bits,
        ModuleCachePublication::CompilerNamespace,
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ModuleCachePublication {
    CompilerNamespace,
    // Every admitted extension result, including multi-phase execution,
    // owns its exact publication. Earlier values cannot substitute its identity.
    Extension,
}

pub(crate) fn module_cache_publish(
    name_bits: u64,
    module_bits: u64,
    publication: ModuleCachePublication,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        if !module_publication_admits(module_bits) {
            return raise_exception::<_>(_py, "TypeError", "import returned non-module payload");
        }
        if publication == ModuleCachePublication::CompilerNamespace
            && crate::builtins::module_table::executing_private_body(_py, &name)
        {
            return match execution::on_module_publish(_py, &name, module_bits) {
                Ok(()) => MoltObject::none().bits(),
                Err(error) => error,
            };
        }
        let initializing_builtins = name == "builtins"
            && crate::builtins::module_table::module_initialization_awaits_publication(_py, &name);
        let is_sys = name == "sys";
        let initializing_sys = is_sys
            && crate::builtins::module_table::module_initialization_awaits_publication(_py, &name);
        // A new initializer namespace needs bootstrap facts; a public cache
        // rewrite or republished retained namespace must preserve user changes.
        let initialize_sys_namespace =
            initializing_sys && interpreter_sys_module(_py) != Some(module_bits);
        if initializing_sys
            && !obj_from_bits(module_bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MODULE })
        {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "canonical sys initializer must publish a module",
            );
        }
        let trace_cache = trace_module_cache();
        let builtins_dict = if initializing_builtins {
            obj_from_bits(module_bits)
                .as_ptr()
                .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_MODULE)
                .and_then(|ptr| obj_from_bits(unsafe { module_dict_bits(ptr) }).as_ptr())
        } else {
            None
        };
        // Bindings made before publication are not the runtime's (HF-79).
        let prior_guard_bits =
            builtins_dict.map_or(0, crate::builtins::binding_guards::namespace_guard_bits);
        // Seed only the exact namespace being published by the canonical
        // initializer. Constructing a same-named ModuleType, replacing the
        // visible cache, or re-publishing a live module grants no privilege.
        if crate::builtins::module_table::module_initialization_awaits_publication(_py, &name)
            && !crate::intrinsics::registry::publish_python_native_namespace(
                _py,
                &name,
                module_bits,
            )
        {
            return MoltObject::none().bits();
        }
        if let Some(dict) = builtins_dict {
            // The namespace holds the runtime's builtins now. Seal it before
            // aliases or the Python facade can run any code (HF-79).
            crate::builtins::binding_guards::seal_builtins_namespace(_py, dict, prior_guard_bits);
        }
        if trace_cache {
            eprintln!("module cache set: {name} bits=0x{module_bits:x}");
        }
        // Private publication returned above; imports update the sole dictionary.
        let retired;
        {
            let Some(dictionary) = crate::builtins::module_table::module_dictionary(_py) else {
                return MoltObject::none().bits();
            };
            retired = match sys_modules_set_canonical_name(
                _py,
                ptr_from_bits(dictionary),
                &name,
                module_bits,
            ) {
                Ok(displaced) => displaced,
                Err(error) => return error,
            };
        }
        let previous_table = if publication == ModuleCachePublication::Extension {
            crate::builtins::module_table::publish_extension_result(_py, &name, module_bits)
        } else {
            crate::builtins::module_table::publish_initializing_namespace(_py, &name, module_bits);
            0
        };
        let _previous_table_owner = obj_from_bits(previous_table)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        if initializing_sys {
            let Some(dictionary) = sys_modules_dict_bits(_py, module_bits) else {
                return MoltObject::none().bits();
            };
            dec_ref_bits(_py, dictionary);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        // Displaced dictionary edges remain pinned until namespace/transaction
        // metadata is coherent; their finalizers cannot observe partial publication.
        let _retired = retired;
        // A provider may recursively import builtins. Its dictionary entry,
        // interpreter role and temporary initializer namespace are visible.
        if initializing_builtins
            && !crate::intrinsics::registry::publish_python_builtin_aliases(_py, module_bits)
        {
            // Preserve the original error for the initializer transaction's
            // normal unwind; no alias getter or separate provider lane retries.
            return MoltObject::none().bits();
        }
        if initialize_sys_namespace {
            let sys_obj = obj_from_bits(module_bits);
            if let Some(sys_ptr) = sys_obj.as_ptr() {
                unsafe {
                    if sys_populate_argv_executable(_py, sys_ptr).is_err() {
                        return MoltObject::none().bits();
                    }
                    if trace_sys_module() && exception_pending(_py) {
                        let exc_bits = molt_exception_last_pending();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string());
                        eprintln!("sys module pending after argv/executable: {kind}");
                        dec_ref_bits(_py, exc_bits);
                    }
                    if sys_populate_stdio(_py, sys_ptr).is_err() {
                        return MoltObject::none().bits();
                    }
                    if sys_populate_bootstrap_metadata(_py, sys_ptr).is_err() {
                        return MoltObject::none().bits();
                    }
                    if trace_sys_module() && exception_pending(_py) {
                        let exc_bits = molt_exception_last_pending();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string());
                        eprintln!("sys module pending after stdio: {kind}");
                        dec_ref_bits(_py, exc_bits);
                    }
                }
            }
        }
        if is_sys && trace_sys_module() {
            let sys_obj = obj_from_bits(module_bits);
            if let Some(sys_ptr) = sys_obj.as_ptr() {
                unsafe {
                    let exe_ptr = alloc_string(_py, b"executable");
                    if exe_ptr.is_null() {
                        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(
                        );
                    }
                    let exe_name = ExceptionValue::adopt(_py, MoltObject::from_ptr(exe_ptr).bits());
                    let argv_ptr = alloc_string(_py, b"argv");
                    if argv_ptr.is_null() {
                        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(
                        );
                    }
                    let argv_name =
                        ExceptionValue::adopt(_py, MoltObject::from_ptr(argv_ptr).bits());
                    let exe_val = match module_attr_lookup(_py, sys_ptr, exe_name.bits()) {
                        Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)),
                        Err(()) => return MoltObject::none().bits(),
                    };
                    let argv_val = match module_attr_lookup(_py, sys_ptr, argv_name.bits()) {
                        Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)),
                        Err(()) => return MoltObject::none().bits(),
                    };
                    drop(exe_name);
                    drop(argv_name);
                    let exe_desc = exe_val
                        .map(|value| {
                            let obj = obj_from_bits(value.bits());
                            string_obj_to_owned(obj)
                                .unwrap_or_else(|| type_name(_py, obj).to_string())
                        })
                        .unwrap_or_else(|| "<missing>".to_string());
                    let argv_desc = argv_val
                        .map(|value| type_name(_py, obj_from_bits(value.bits())).to_string())
                        .unwrap_or_else(|| "<missing>".to_string());
                    eprintln!("sys module set: executable={exe_desc} argv_type={argv_desc}");
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// import_add_module already published this identity in the canonical dict.
/// Reconcile only transient initialization custody before callbacks can run.
pub(crate) fn reconcile_extension_publication(py: &PyToken<'_>, name: &str, bits: u64) {
    let previous = crate::builtins::module_table::publish_extension_result(py, name, bits);
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_del(name_bits: u64) -> u64 {
    module_cache_remove(name_bits, None)
}

/// Detach dictionary and transient initializer owners before finalizers. An owned
/// extension rollback supplies its identity so partial publication is removed
/// without deleting a different public replacement or nested import.
pub(crate) fn module_cache_remove(name_bits: u64, expected: Option<u64>) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(py, "TypeError", "module name must be str");
        };
        if expected.is_none() && crate::builtins::module_table::executing_private_body(py, &name) {
            return MoltObject::none().bits();
        }
        use crate::builtins::exceptions::take_raised;
        let saved = exception_pending(py).then(|| take_raised(py));
        let cleanup_name = if expected.is_none() && saved.is_some() {
            crate::builtins::module_table::ordinary_failure_name(py, &name)
        } else {
            Ok(None)
        };
        let modules = crate::builtins::module_table::existing_module_dictionary(py);
        // Removal detaches dictionary edges but defers their finalizers. No
        // reference is released until dictionary and transaction custody agree.
        // A failed live spec.name read is the cleanup error; it must not cause
        // deletion using an obsolete catalog key instead.
        let public = match cleanup_name.as_ref() {
            Ok(key) => modules.and_then(|bits| unsafe {
                let ptr = ptr_from_bits(bits);
                let key = key.as_ref().map_or(name_bits, |key| key.bits());
                crate::object::ops::dict_del_if_value_deferred(py, ptr, key, expected)
                    .unwrap_or_default()
            }),
            Err(_) => None,
        };
        let table = crate::builtins::module_table::detach_cache_publication(py, &name, expected);
        // There are no name-based mutations after this release boundary.
        drop(public);
        if table != 0 {
            dec_ref_bits(py, table);
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| drop(cleanup_name));
        crate::builtins::exceptions::finish_exception_cleanup(
            py,
            saved,
            if expected.is_some() {
                crate::builtins::exceptions::ExceptionCleanupPolicy::Preserve
            } else {
                crate::builtins::exceptions::ExceptionCleanupPolicy::IgnoreKeyError
            },
        );
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_debug_trace(
    func_ptr_bits: u64,
    func_len_bits: u64,
    op_idx_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        TRACE_LAST_OP.store(op_idx_bits, Ordering::Relaxed);
        ensure_sigtrap_handler();
        let Some(ptr) = crate::provenance::abi::const_ptr::<u8>(func_ptr_bits) else {
            return MoltObject::none().bits();
        };
        let Some(bytes) = (unsafe { crate::provenance::abi::slice(ptr, func_len_bits) }) else {
            return MoltObject::none().bits();
        };
        if !trace_op_silent() {
            if let Ok(name) = std::str::from_utf8(bytes) {
                eprintln!("trace {name} op={op_idx_bits}");
            } else {
                eprintln!("trace <invalid> op={op_idx_bits}");
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug_attr = debug_module_get_attr();
        if trace_get_attr() || trace_module_attrs_mode() == TraceMode::Verbose {
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            eprintln!(
                "module_get_attr: mod=0x{:x} attr=0x{:x} name={}",
                module_bits, attr_bits, attr_name
            );
        }
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            // When the native backend continues past a RAISE (exception pending
            // but no control-flow exit), the next MODULE_GET_ATTR receives None.
            // Propagate the already-pending exception instead of overwriting it
            // with a confusing TypeError about "expects module".
            if module_obj.is_none() || exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            let msg = format!(
                "module attribute access expects module, got non-pointer (bits=0x{:x}) for attr '{}'",
                module_bits, attr_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                    .unwrap_or_else(|| "<attr>".to_string());
                let type_id = object_type_id(module_ptr);
                if debug_attr {
                    eprintln!(
                        "molt module_get_attr non-module (bits=0x{:x}, type_id={}) for attr={}",
                        module_bits, type_id, attr_name
                    );
                }
                let msg = format!(
                    "module attribute access expects module, got type_id={} (bits=0x{:x}) for attr '{}'",
                    type_id, module_bits, attr_name
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let _dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let found =
                match crate::builtins::attributes::attr_lookup_ptr(_py, module_ptr, attr_bits) {
                    Ok(value) => value,
                    Err(()) => return MoltObject::none().bits(),
                };
            if let Some(val) = found {
                if let Some((module_name, attr_name)) = traced_module_attr(module_ptr, attr_bits) {
                    eprintln!("molt module attr get module={module_name} attr={attr_name}");
                }
                return val;
            }
            if debug_attr {
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                    .unwrap_or_default();
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                    .unwrap_or_else(|| "<attr>".to_string());
                let mut present = false;
                let order = crate::dict_live_entries(_dict_ptr);
                let entries = crate::dict_len(_dict_ptr);
                for row in order {
                    if let Some(key_name) = string_obj_to_owned(obj_from_bits(row.key))
                        && key_name == attr_name
                    {
                        present = true;
                        break;
                    }
                }
                let pending = exception_pending(_py);
                eprintln!(
                    "molt module_get_attr missing module={} attr={} present_in_dict={} dict_entries={} pending={}",
                    module_name, attr_name, present, entries, pending
                );
            }
            raise_module_attribute_miss(_py, module_ptr, attr_bits)
        }
    })
}

/// Circular-import recovery consumes the public cache exactly, including a
/// cached None. Unlike IMPORT_NAME, IMPORT_FROM does not reject that value.
unsafe fn import_from_sys_modules_lookup(
    py: &PyToken<'_>,
    module_name: u64,
    attr: u64,
) -> Result<Option<u64>, ()> {
    unsafe {
        let Some(sys_bits) = interpreter_sys_module(py) else {
            return Ok(None);
        };
        let modules_bits = sys_modules_dict_bits(py, sys_bits).ok_or(())?;
        let modules_ptr = ptr_from_bits(modules_bits);
        let _modules_owner = crate::PtrDropGuard::new(modules_ptr);
        let Some(name_ptr) = obj_from_bits(module_name).as_ptr() else {
            return Ok(None);
        };
        let Some(attr_ptr) = obj_from_bits(attr).as_ptr() else {
            return Ok(None);
        };
        let mut name =
            std::slice::from_raw_parts(string_bytes(name_ptr), string_len(name_ptr)).to_vec();
        name.push(b'.');
        name.extend_from_slice(std::slice::from_raw_parts(
            string_bytes(attr_ptr),
            string_len(attr_ptr),
        ));
        let key_ptr = alloc_string(py, &name);
        if key_ptr.is_null() {
            return Err(());
        }
        let key = ExceptionValue::adopt(py, MoltObject::from_ptr(key_ptr).bits());
        let found = dict_get_in_place(py, modules_ptr, key.bits())?;
        if let Some(bits) = found {
            inc_ref_bits(py, bits);
        }
        Ok(found)
    }
}

/// PyModule_GetFilenameObject reads the physical module dictionary, bypassing
/// descriptors and __getattr__. A missing/non-string filename is no origin.
unsafe fn module_file_origin(py: &PyToken<'_>, module_bits: u64) -> Result<Option<u64>, ()> {
    unsafe {
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            return Ok(None);
        };
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return Ok(None);
        }
        let Some(dict_ptr) = obj_from_bits(module_dict_bits(module_ptr)).as_ptr() else {
            return Ok(None);
        };
        let key = intern_static_name(py, &modules_state(py).module_file_name, b"__file__");
        if key == 0 {
            return Err(());
        }
        let Some(bits) = dict_get_in_place(py, dict_ptr, key)? else {
            return Ok(None);
        };
        if !import_from_is_string(bits) {
            return Ok(None);
        }
        inc_ref_bits(py, bits);
        Ok(Some(bits))
    }
}

fn import_from_is_string(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
}

/// The existing importlib optional-attribute owner preserves descriptor and
/// PEP 562 failures; only AttributeError means absence. Keep owned results
/// across subsequent metadata callbacks and error-preserving cleanup.
fn import_from_attr<'a, 'py>(
    py: &'a PyToken<'py>,
    object: u64,
    name: &[u8],
) -> Result<Option<ExceptionValue<'a, 'py>>, u64> {
    let key = attr_name_bits_from_bytes(py, name)
        .map(|bits| ExceptionValue::adopt(py, bits))
        .ok_or_else(|| MoltObject::none().bits())?;
    crate::builtins::platform::getattr_optional_bits(py, object, key.bits())
        .map(|value| value.map(|bits| ExceptionValue::adopt(py, bits)))
}

fn import_from_error(py: &PyToken<'_>, module: u64, attr: u64, name: Option<u64>) -> u64 {
    let modern = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
    let none = MoltObject::none().bits();
    let mut origin = None;
    let mut initializing = false;
    if modern {
        let spec = match import_from_attr(py, module, b"__spec__") {
            Ok(value) => value,
            Err(result) => return result,
        };
        // An absent spec differs from a present None: only a present spec
        // takes the file fallback and circular-initialization path in 3.13+.
        if let Some(spec) = spec {
            let has_location = match import_from_attr(py, spec.bits(), b"has_location") {
                Ok(value) => value,
                Err(result) => return result,
            };
            let located = match has_location.as_ref() {
                Some(value) => {
                    match crate::object::ops::truth_value(py, obj_from_bits(value.bits())) {
                        Ok(value) => value,
                        Err(()) => return none,
                    }
                }
                None => false,
            };
            if located {
                origin = match import_from_attr(py, spec.bits(), b"origin") {
                    Ok(value) => value.filter(|value| import_from_is_string(value.bits())),
                    Err(result) => return result,
                };
            }
            if origin.is_none() {
                origin = match unsafe { module_file_origin(py, module) } {
                    Ok(value) => value.map(|bits| ExceptionValue::adopt(py, bits)),
                    Err(()) => return none,
                };
            }
            initializing = match crate::builtins::platform::spec_is_initializing(
                py,
                spec.bits(),
                crate::builtins::module_table::ModuleImportPolicy::CImport,
            ) {
                Ok(value) => value,
                Err(result) => return result,
            };
        }
    } else {
        origin = match unsafe { module_file_origin(py, module) } {
            Ok(value) => value.map(|bits| ExceptionValue::adopt(py, bits)),
            Err(()) => {
                clear_exception(py);
                None
            }
        };
        if origin.is_none() {
            // 3.12 clears PyModule_GetFilenameObject failures and does not
            // consult __spec__ when the filename is absent.
            clear_exception(py);
        } else {
            initializing = import_from_attr(py, module, b"__spec__")
                .and_then(|spec| match spec {
                    Some(spec) => crate::builtins::platform::spec_is_initializing(
                        py,
                        spec.bits(),
                        crate::builtins::module_table::ModuleImportPolicy::CImport,
                    ),
                    None => Ok(false),
                })
                .unwrap_or_else(|_| {
                    // 3.12 _PyModuleSpec_IsInitializing explicitly clears
                    // callback failures; 3.13+ propagates them.
                    clear_exception(py);
                    false
                });
        }
    }
    let attr_repr = ExceptionValue::adopt(py, crate::molt_repr_from_obj(attr));
    if exception_pending(py) {
        return none;
    }
    let name_repr = name.map(|bits| ExceptionValue::adopt(py, crate::molt_repr_from_obj(bits)));
    if exception_pending(py) {
        return none;
    }
    // Keep Python's lossless string storage, including surrogate characters.
    let mut message = b"cannot import name ".to_vec();
    unsafe {
        let ptr = ptr_from_bits(attr_repr.bits());
        message.extend_from_slice(std::slice::from_raw_parts(
            string_bytes(ptr),
            string_len(ptr),
        ));
    };
    if initializing {
        message.extend_from_slice(b" from partially initialized module ");
    } else {
        message.extend_from_slice(b" from ");
    }
    if let Some(name_repr) = name_repr {
        unsafe {
            let ptr = ptr_from_bits(name_repr.bits());
            message.extend_from_slice(std::slice::from_raw_parts(
                string_bytes(ptr),
                string_len(ptr),
            ));
        };
    } else {
        message.extend_from_slice(b"'<unknown module name>'");
    }
    if initializing {
        message.extend_from_slice(b" (most likely due to a circular import)");
    }
    if let Some(origin) = origin.as_ref() {
        let rendered =
            crate::object::ops_format::format_obj_str_bytes(py, obj_from_bits(origin.bits()));
        if exception_pending(py) {
            return none;
        }
        message.extend_from_slice(b" (");
        message.extend_from_slice(&rendered);
        message.push(b')');
    } else if !initializing {
        message.extend_from_slice(b" (unknown location)");
    }
    raise_import_error(
        py,
        &message,
        name.unwrap_or(none),
        origin.as_ref().map_or(none, ExceptionValue::bits),
        attr,
    )
}

/// Publish a freshly loaded child through Python's attribute protocol once.
/// Module state is already committed: a rejected publication must not roll back
/// sys.modules, and cached import/reload must not repeat this callback.
pub(crate) fn publish_import_child(
    py: &PyToken<'_>,
    parent_bits: u64,
    parent_name: &str,
    child_name: &str,
    child_bits: u64,
) -> Result<(), u64> {
    // The setter may remove either module from sys.modules or replace its
    // class. Keep the exact receiver and child alive throughout the callback.
    inc_ref_bits(py, parent_bits);
    inc_ref_bits(py, child_bits);
    let result = (|| {
        let Some(name_bits) = attr_name_bits_from_bytes(py, child_name.as_bytes()) else {
            return Err(MoltObject::none().bits());
        };
        let result = crate::molt_set_attr_name(parent_bits, name_bits, child_bits);
        dec_ref_bits(py, name_bits);
        crate::call::discard_owned_call_result(py, result);
        if !exception_pending(py) {
            return Ok(());
        }
        if !clear_attribute_error_if_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let message =
            format!("Cannot set an attribute on '{parent_name}' for child module '{child_name}'");
        if crate::builtins::warnings_ext::emit_runtime_warning(py, &message, "ImportWarning") {
            Ok(())
        } else {
            Err(MoltObject::none().bits())
        }
    })();
    dec_ref_bits(py, child_bits);
    dec_ref_bits(py, parent_bits);
    result
}

/// Prepare the child side effect for `from package import child` without
/// deciding the final binding value.
///
/// CPython's fromlist handling lets an existing package attribute win. Only
/// when the attribute is absent does it import `package.child`, bind that
/// module onto the parent, and leave the later IMPORT_FROM read to choose the
/// final value.
pub(crate) fn prepare_from_import_child(
    _py: &PyToken<'_>,
    module_bits: u64,
    attr_bits: u64,
    child_name_bits: u64,
) -> Result<(), u64> {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        if module_obj.is_none() || exception_pending(_py) {
            return Ok(());
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "from-import expects module",
        ));
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            if exception_pending(_py) {
                return Ok(());
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "from-import expects module",
            ));
        }
        let found =
            crate::builtins::attr::attr_lookup_ptr_allow_missing(_py, module_ptr, attr_bits)
                .map_err(|()| MoltObject::none().bits())?;
        if let Some(existing_bits) = found {
            dec_ref_bits(_py, existing_bits);
            return Ok(());
        }
    }

    let ModuleImportOutcome::Imported(imported_bits) = module_import_attempt(child_name_bits)?
    else {
        return Ok(());
    };
    // The successful fresh-load transaction already published the child.
    // A cache hit deliberately does not restore a deleted parent attribute.
    dec_ref_bits(_py, imported_bits);
    Ok(())
}

/// `from MODULE import name` attribute binding.
///
/// Mirrors CPython's `IMPORT_FROM` opcode (`import_from` in ceval): it performs
/// the module attribute lookup and, on a *missing* attribute, applies the
/// import-specific recovery+failure semantics that distinguish it from a plain
/// `module.attr` access ([`molt_module_get_attr`]):
///
///   1. `getattr(module, name)` — including PEP 562 module `__getattr__`.
///   2. On `AttributeError` (a clean miss, or `__getattr__` raising
///      `AttributeError`): retry as `sys.modules["{module}.{name}"]`, which
///      recovers a circularly-imported submodule not yet bound as an attribute.
///   3. On miss, raise `ImportError("cannot import name '{name}' from
///      '{module}' ({origin})")`.
///
/// A non-`AttributeError` raised by the lookup (e.g. a module `__getattr__`
/// raising `ValueError`) propagates unchanged, exactly as CPython does.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_import_from(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let _module = ExceptionValue::pin(py, module_bits);
        let _attr = ExceptionValue::pin(py, attr_bits);
        // IMPORT_FROM also accepts a non-module result from an overridden
        // __import__; the ordinary attribute protocol owns that lookup.
        if let Some(ptr) = obj_from_bits(module_bits).as_ptr() {
            // Retain the existing pointer lookup fast path: a clean missing
            // attribute does not allocate a throwaway AttributeError.
            match unsafe {
                crate::builtins::attr::attr_lookup_ptr_allow_missing(py, ptr, attr_bits)
            } {
                Ok(Some(value)) => return value,
                Ok(None) => {}
                Err(()) => return MoltObject::none().bits(),
            }
        } else {
            match crate::builtins::platform::getattr_optional_bits(py, module_bits, attr_bits) {
                Ok(Some(value)) => return value,
                Ok(None) => {}
                Err(result) => return result,
            }
        }
        let name = match import_from_attr(py, module_bits, b"__name__") {
            Ok(value) => value.filter(|value| import_from_is_string(value.bits())),
            Err(result) => {
                if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                    return result;
                }
                clear_exception(py);
                None
            }
        };
        if let Some(name) = name.as_ref() {
            match unsafe { import_from_sys_modules_lookup(py, name.bits(), attr_bits) } {
                Ok(Some(value)) => return value,
                Ok(None) => {}
                Err(()) => return MoltObject::none().bits(),
            }
        }
        import_from_error(
            py,
            module_bits,
            attr_bits,
            name.as_ref().map(ExceptionValue::bits),
        )
    })
}

/// LOAD_NAME/LOAD_CLASSDEREF mapping probe. Only KeyError means absence;
/// __getitem__ callbacks and every other exception retain normal semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_namespace_get(
    namespace_bits: u64,
    name_bits: u64,
    missing_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match lookup_namespace_item(_py, namespace_bits, name_bits) {
            Ok(Some(value)) => value,
            Ok(None) => {
                inc_ref_bits(_py, missing_bits);
                missing_bits
            }
            Err(()) => MoltObject::none().bits(),
        }
    })
}

/// DELETE_NAME translates any mapping-deletion failure into NameError. This
/// intentionally differs from LOAD_NAME's KeyError-only fallback (CPython3.12+).
#[unsafe(no_mangle)]
pub extern "C" fn molt_namespace_del(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "namespace name must be a string");
        };
        let deleted = if let Some(ptr) = obj_from_bits(namespace_bits).as_ptr()
            && unsafe { crate::object_is_exact_builtin_dict(_py, ptr) }
        {
            unsafe { dict_del_in_place(_py, ptr, name_bits) }.unwrap_or(false)
        } else {
            match unsafe {
                crate::builtins::attr::lookup_special_method(_py, namespace_bits, b"__delitem__")
            } {
                Ok(Some(method)) => {
                    let result = unsafe { call_callable1(_py, method, name_bits) };
                    let succeeded = !exception_pending(_py);
                    crate::call::discard_owned_call_result(_py, result);
                    dec_ref_bits(_py, method);
                    succeeded
                }
                Ok(None) | Err(()) => false,
            }
        };
        if deleted {
            // DELETE_NAME has no result owner. The generic subscript deletion
            // ABI returns its borrowed receiver; exposing that through an
            // intrinsic result would manufacture an owner and release it twice.
            return MoltObject::none().bits();
        }
        clear_exception(_py);
        raise_exception::<u64>(_py, "NameError", &format!("name '{name}' is not defined"))
    })
}

/// Select suggestions from the same dictionaries that own LOAD_GLOBAL hits and
/// misses. Module metadata and a later builtins cache cannot change this search.
fn global_name_suggestion(
    py: &PyToken<'_>,
    dictionary: u64,
    name: &str,
) -> Result<Option<String>, ()> {
    let Some(ptr) = crate::builtins::frames::globals_namespace_storage_ptr(py, dictionary)? else {
        return Ok(None);
    };
    unsafe {
        let length = crate::dict_len(ptr);
        use crate::builtins::diagnostic_suggestions::{MAX_CANDIDATE_ITEMS, calculate_suggestion};
        if length >= MAX_CANDIDATE_ITEMS {
            return Ok(None);
        }
        let mut candidates = Vec::with_capacity(length);
        for row in crate::dict_live_entries(ptr) {
            let Some(key) = obj_from_bits(row.key).as_ptr() else {
                return Ok(None);
            };
            if object_type_id(key) != TYPE_ID_STRING {
                return Ok(None);
            }
            let bytes = std::slice::from_raw_parts(string_bytes(key), string_len(key));
            let Ok(name) = std::str::from_utf8(bytes) else {
                return Ok(None);
            };
            candidates.push(name);
        }
        Ok(calculate_suggestion(name, &candidates).map(str::to_owned))
    }
}

/// Who owns a globals namespace, named only when a trace or an error prints it.
#[derive(Clone, Copy)]
pub(crate) enum GlobalsOwner {
    Named(&'static str),
    Module(*mut u8),
}

impl GlobalsOwner {
    fn label(self) -> String {
        match self {
            GlobalsOwner::Named(label) => label.to_string(),
            GlobalsOwner::Module(module_ptr) => {
                string_obj_to_owned(obj_from_bits(unsafe { module_name_bits(module_ptr) }))
                    .unwrap_or_else(|| "<module>".to_string())
            }
        }
    }
}

/// A global's name as text, for traces and errors only. Every global read in
/// a function reaches the lookup, so the hit path compares and hashes the
/// interned key and allocates nothing (HF-79).
fn global_name_text(name_bits: u64) -> String {
    string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string())
}

/// Whether ``MOLT_TRACE_MODULE_GLOBALS`` traces this name; the filtered mode
/// names two sys flag globals.
fn traces_module_global(name_bits: u64) -> bool {
    match trace_module_globals_mode() {
        TraceMode::Off => false,
        TraceMode::Verbose => true,
        TraceMode::Filtered => matches!(
            global_name_text(name_bits).as_str(),
            "_SYS_FLAGS_SEQUENCE_FIELDS" | "_FlagsTuple"
        ),
    }
}

/// The single lookup path for Python activations and explicit runtime module
/// dictionaries. The caller selects namespace custody; this primitive never
/// replaces a captured builtins dictionary after a miss.
fn lookup_global_namespace(
    py: &PyToken<'_>,
    module_bits: u64,
    owner: GlobalsOwner,
    globals_bits: u64,
    builtins_bits: u64,
    name_bits: u64,
) -> u64 {
    let trace_globals = traces_module_global(name_bits);
    match lookup_namespace_item(py, globals_bits, name_bits) {
        Ok(Some(value)) => {
            if trace_globals {
                eprintln!(
                    "molt module_get_global hit module={} name={} module_bits=0x{:x} dict_bits=0x{:x} val_type={}",
                    owner.label(),
                    global_name_text(name_bits),
                    module_bits,
                    globals_bits,
                    type_name(py, obj_from_bits(value)),
                );
            }
            return value;
        }
        Ok(None) => {
            if trace_globals {
                eprintln!(
                    "molt module_get_global miss module={} name={} module_bits=0x{:x} dict_bits=0x{:x}",
                    owner.label(),
                    global_name_text(name_bits),
                    module_bits,
                    globals_bits,
                );
            }
        }
        Err(()) => return MoltObject::none().bits(),
    }
    match lookup_builtin_global(py, name_bits, builtins_bits) {
        Ok(Some(value)) => return value,
        Ok(None) => {}
        Err(()) => return MoltObject::none().bits(),
    }
    let name = global_name_text(name_bits);
    if name == "exec" || name == "eval" {
        let message = format!(
            "MOLT_COMPAT_ERROR: {name}() is unsupported in compiled Molt binaries; \
dynamic code execution is outside the verified subset. \
Use static modules or pre-generated code paths instead."
        );
        return raise_exception::<_>(py, "RuntimeError", &message);
    }
    if trace_name_error() {
        eprintln!(
            "molt name error module={} name={} pending={}",
            owner.label(),
            name,
            exception_pending(py),
        );
    }
    let suggestion = match global_name_suggestion(py, globals_bits, &name) {
        Ok(Some(suggestion)) => Some(suggestion),
        Ok(None) => match global_name_suggestion(py, builtins_bits, &name) {
            Ok(suggestion) => suggestion,
            Err(()) => return MoltObject::none().bits(),
        },
        Err(()) => return MoltObject::none().bits(),
    };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let message = match suggestion {
        Some(similar) => format!("name '{name}' is not defined. Did you mean: '{similar}'?"),
        None => format!("name '{name}' is not defined"),
    };
    raise_exception::<_>(py, "NameError", &message)
}

/// LOAD_GLOBAL uses the executing frame, including FunctionType aliases whose
/// lexical module operand differs from their activation namespace. With no
/// Python frame, runtime callers select the explicit module dictionary.
/// Attribute access and MODULE_GET_NAME remain module-only APIs.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_globals = traces_module_global(name_bits);
        if trace_globals {
            eprintln!(
                "molt module_get_global enter name={} module_bits=0x{:x} pending={}",
                global_name_text(name_bits),
                module_bits,
                exception_pending(_py),
            );
        }
        let active_globals = frame_stack_active_globals_bits();
        if active_globals != 0 {
            let globals = match crate::builtins::frames::globals_namespace_storage_bits(
                _py,
                active_globals,
            ) {
                Ok(storage) => storage,
                Err(()) => return MoltObject::none().bits(),
            };
            if globals.is_some() {
                return lookup_global_namespace(
                    _py,
                    module_bits,
                    GlobalsOwner::Named("<frame>"),
                    active_globals,
                    crate::builtins::frames::frame_stack_active_builtins_bits(),
                    name_bits,
                );
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(_py, "SystemError", "active globals is not a dictionary");
        }
        let module = obj_from_bits(module_bits);
        let Some(module_ptr) = module.as_ptr() else {
            // Preserve the exception-edge ABI: None/uninitialized operands do
            // not overwrite the exception the caller is about to propagate.
            if module.is_none() || exception_pending(_py) {
                if trace_globals {
                    eprintln!(
                        "molt module_get_global early_none name={} module_bits=0x{:x} module_is_none={} pending={}",
                        global_name_text(name_bits),
                        module_bits,
                        module.is_none(),
                        exception_pending(_py),
                    );
                }
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!(
                    "module get_global expects module, got non-pointer (bits=0x{:x}) for name '{}'",
                    module_bits,
                    global_name_text(name_bits),
                ),
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if exception_pending(_py) {
                    if trace_globals {
                        eprintln!(
                            "molt module_get_global early_type name={} module_bits=0x{:x} type_id={} pending={}",
                            global_name_text(name_bits),
                            module_bits,
                            object_type_id(module_ptr),
                            exception_pending(_py),
                        );
                    }
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!(
                        "module get_global expects module, got type_id={} (bits=0x{:x}) for name '{}'",
                        object_type_id(module_ptr),
                        module_bits,
                        global_name_text(name_bits),
                    ),
                );
            }
            let globals = module_dict_bits(module_ptr);
            if obj_from_bits(globals)
                .as_ptr()
                .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_DICT)
            {
                return raise_exception::<_>(_py, "TypeError", "module dict missing");
            }
            let builtins =
                match crate::builtins::frames::frame_effective_builtins_bits(_py, globals) {
                    Ok(bits) => bits,
                    Err(()) => return MoltObject::none().bits(),
                };
            lookup_global_namespace(
                _py,
                module_bits,
                GlobalsOwner::Module(module_ptr),
                globals,
                builtins,
                name_bits,
            )
        }
    })
}

fn module_del_global_impl(
    _py: &PyToken<'_>,
    module_bits: u64,
    name_bits: u64,
    missing_ok: bool,
) -> u64 {
    let trace = trace_name_error();
    unsafe {
        let active_globals = frame_stack_active_globals_bits();
        let dict_ptr = if active_globals != 0 {
            match crate::builtins::frames::globals_namespace_storage_ptr(_py, active_globals) {
                Ok(Some(ptr)) => ptr,
                Err(()) => return MoltObject::none().bits(),
                Ok(None) => {
                    return raise_exception::<_>(
                        _py,
                        "SystemError",
                        "active globals is not a dictionary",
                    );
                }
            }
        } else {
            match module_dict_ptr(_py, module_bits) {
                Ok(ptr) => ptr,
                Err(bits) => return bits,
            }
        };
        match dict_del_in_place(_py, dict_ptr, name_bits) {
            Ok(true) | Err(()) => return MoltObject::none().bits(),
            Ok(false) => {}
        }
        if missing_ok {
            return MoltObject::none().bits();
        }
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        if trace {
            let pending = exception_pending(_py);
            eprintln!(
                "molt name error(del) globals=0x{:x} name={} pending={}",
                MoltObject::from_ptr(dict_ptr).bits(),
                name,
                pending
            );
        }
        let msg = format!("name '{name}' is not defined");
        raise_exception::<_>(_py, "NameError", &msg)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        module_del_global_impl(_py, module_bits, name_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_del_global_if_present(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        module_del_global_impl(_py, module_bits, name_bits, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_name(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Keep wasm import parity; module __name__ is stored in the module dict.
        molt_module_get_attr(module_bits, attr_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = module_set_attr(py, module_bits, attr_bits, val_bits);
        MoltObject::none().bits()
    })
}

pub(crate) fn module_set_attr(
    _py: &PyToken<'_>,
    module_bits: u64,
    attr_bits: u64,
    val_bits: u64,
) -> Result<(), ()> {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        raise_exception::<()>(_py, "TypeError", "module attribute set expects module");
        return Err(());
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            raise_exception::<()>(_py, "TypeError", "module attribute set expects module");
            return Err(());
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                raise_exception::<()>(_py, "TypeError", "module dict missing");
                return Err(());
            }
        };
        if let Some((module_name, attr_name)) = traced_module_attr(module_ptr, attr_bits) {
            eprintln!("molt module attr set module={module_name} attr={attr_name}");
        }
        let annotations_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotations_name,
            b"__annotations__",
        );
        if crate::object::ops_compare::string_storage_equal(attr_bits, annotations_bits) {
            if !dict_set_in_place(_py, dict_ptr, attr_bits, val_bits) {
                return Err(());
            }
            if pep649_enabled(_py) {
                let annotate_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                let none_bits = MoltObject::none().bits();
                if !dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits) {
                    return Err(());
                }
            }
            return Ok(());
        }
        let annotate_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotate_name,
            b"__annotate__",
        );
        if crate::object::ops_compare::string_storage_equal(attr_bits, annotate_bits)
            && pep649_enabled(_py)
        {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let callable_ok = crate::builtins::callable::is_callable_impl(_py, val_bits);
                if !callable_ok {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "__annotate__ must be callable or None",
                    );
                    return Err(());
                }
            }
            if !dict_set_in_place(_py, dict_ptr, attr_bits, val_bits) {
                return Err(());
            }
            if !val_obj.is_none() {
                dict_del_in_place(_py, dict_ptr, annotations_bits)?;
            }
            return Ok(());
        }
        let (effective_val_bits, owned) =
            match execution::module_metadata_store(_py, module_bits, attr_bits) {
                Ok(execution::MetadataStore::Body) => (val_bits, None),
                Ok(execution::MetadataStore::Value { bits, owned }) => {
                    (bits, owned.then_some(bits))
                }
                Ok(execution::MetadataStore::Absent) => return Ok(()),
                Err(_) => return Err(()),
            };
        let stored = dict_set_in_place(_py, dict_ptr, attr_bits, effective_val_bits)
            && execution::after_module_metadata_set(
                _py,
                module_bits,
                attr_bits,
                effective_val_bits,
            )
            .is_ok();
        if let Some(bits) = owned {
            dec_ref_bits(_py, bits);
        }
        if !stored {
            return Err(());
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_import_star(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { import_star::import_star(_py, src_bits, dst_bits) })
}

#[cfg(test)]
#[path = "namespace_delete_tests.rs"]
mod namespace_delete_tests;

#[cfg(test)]
#[path = "namespace_lookup_tests.rs"]
mod namespace_lookup_tests;

#[cfg(test)]
#[path = "modules/tests.rs"]
mod tests;
