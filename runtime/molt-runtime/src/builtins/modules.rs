use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(test)]
use crate::format_exception_with_traceback;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
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
use crate::builtins::exceptions::molt_exception_last_pending;
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    HashContext, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE, alloc_dict_with_pairs, alloc_list, alloc_module_obj, alloc_string, alloc_tuple,
    call_callable0, call_callable1, call_callable2, class_mro_vec, class_name_for_error,
    clear_exception, dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order,
    dict_set_in_place, exception_pending, format_obj_str, frame_stack_active_globals_bits,
    has_capability, inc_ref_bits, init_atomic_bits, intern_static_name, is_missing_bits, is_truthy,
    missing_bits, module_dict_bits, module_name_bits, molt_call_bind, molt_callargs_expand_kwstar,
    molt_callargs_expand_star, molt_callargs_new, molt_callargs_push_pos, molt_exception_kind,
    molt_exception_last, molt_getattr_builtin, molt_int_from_obj, molt_is_callable, molt_iter,
    molt_iter_next, obj_from_bits, object_type_id, ptr_from_bits, raise_exception, runtime_state,
    set_add_in_place, string_bytes, string_len, string_obj_to_owned, to_i64, type_name,
    type_of_bits,
};

mod execution;
mod import_star;
mod runpy;
mod type_attributes;
mod type_protocol;
pub(crate) use type_protocol::publish_module_methods;

pub(crate) use execution::{ExecutionMetadata, execute_compiled_module};
use execution::{copy_dict_entries, execution_sys_path_entries, module_dict_ptr};
pub use runpy::{molt_runpy_run_module, molt_runpy_run_path};
pub use type_attributes::*;
pub use type_protocol::*;

fn trace_module_cache() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_MODULE_CACHE").ok().as_deref(),
            Some("1")
        )
    })
}

fn trace_name_error() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_NAME_ERROR").ok().as_deref(),
            Some("1")
        )
    })
}

fn trace_module_attrs() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_MODULE_ATTRS").ok().as_deref(),
            Some("1")
        )
    })
}

fn trace_module_attrs_verbose() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_MODULE_ATTRS").ok().as_deref(),
            Some("all" | "verbose")
        )
    })
}

fn trace_sys_module() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_SYS_MODULE").ok().as_deref(),
            Some("1")
        )
    })
}

fn trace_op_silent() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_OP_SILENT").ok().as_deref(),
            Some("1")
        )
    })
}

#[cfg(unix)]
fn trace_op_sigtrap_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_OP_SIGTRAP").ok().as_deref(),
            Some("1")
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TraceModuleGlobalsMode {
    Off,
    Filtered,
    Verbose,
}

fn trace_module_globals_mode_raw(raw: Option<&str>) -> TraceModuleGlobalsMode {
    match raw {
        Some("all") | Some("verbose") => TraceModuleGlobalsMode::Verbose,
        Some("1") => TraceModuleGlobalsMode::Filtered,
        _ => TraceModuleGlobalsMode::Off,
    }
}

fn trace_module_globals_mode() -> TraceModuleGlobalsMode {
    static TRACE: OnceLock<TraceModuleGlobalsMode> = OnceLock::new();
    *TRACE.get_or_init(|| {
        trace_module_globals_mode_raw(std::env::var("MOLT_TRACE_MODULE_GLOBALS").ok().as_deref())
    })
}

fn trace_bad_module_name_arg(_py: &PyToken<'_>, where_: &str, bits: u64) {
    if !matches!(
        std::env::var("MOLT_TRACE_BAD_MODULE_NAME").ok().as_deref(),
        Some("1")
    ) {
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
    if matches!(
        std::env::var("MOLT_TRACE_BAD_MODULE_NAME_BT")
            .ok()
            .as_deref(),
        Some("1")
    ) {
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
            _ => return Err(()),
        };
        let argv_key_ptr = alloc_string(_py, b"argv");
        let exec_key_ptr = alloc_string(_py, b"executable");
        if argv_key_ptr.is_null() || exec_key_ptr.is_null() {
            return Err(());
        }
        let argv_key_bits = MoltObject::from_ptr(argv_key_ptr).bits();
        let exec_key_bits = MoltObject::from_ptr(exec_key_ptr).bits();

        let projection = crate::object::ops_sys::with_process_argv(_py, |args| {
            let exec_val = std::env::var("MOLT_SYS_EXECUTABLE")
                .ok()
                .filter(|v| !v.is_empty())
                .map(String::into_bytes)
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
        dict_set_in_place(_py, dict_ptr, argv_key_bits, argv_list_bits);

        let exec_val_ptr = alloc_string(_py, &exec_val);
        if exec_val_ptr.is_null() {
            dec_ref_bits(_py, argv_list_bits);
            dec_ref_bits(_py, argv_key_bits);
            dec_ref_bits(_py, exec_key_bits);
            return Err(());
        }
        let exec_val_bits = MoltObject::from_ptr(exec_val_ptr).bits();
        dict_set_in_place(_py, dict_ptr, exec_key_bits, exec_val_bits);

        dec_ref_bits(_py, argv_list_bits);
        dec_ref_bits(_py, exec_val_bits);
        dec_ref_bits(_py, argv_key_bits);
        dec_ref_bits(_py, exec_key_bits);
        Ok(())
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
            _ => return Err(()),
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

        dict_set_in_place(_py, dict_ptr, stdin_key_bits, stdin_bits);
        dict_set_in_place(_py, dict_ptr, dunder_stdin_bits, stdin_bits);
        dict_set_in_place(_py, dict_ptr, stdout_key_bits, stdout_bits);
        dict_set_in_place(_py, dict_ptr, dunder_stdout_bits, stdout_bits);
        dict_set_in_place(_py, dict_ptr, stderr_key_bits, stderr_bits);
        dict_set_in_place(_py, dict_ptr, dunder_stderr_bits, stderr_bits);

        dec_ref_bits(_py, stdin_bits);
        dec_ref_bits(_py, stdout_bits);
        dec_ref_bits(_py, stderr_bits);
        for bits in keys {
            dec_ref_bits(_py, bits);
        }
        Ok(())
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
            _ => return Err(()),
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
            _ => return Err(()),
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

        let meta_path_ptr = alloc_list(_py, &[]);
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
    if exception_pending(_py) {
        return Err(());
    }
    if let Some(ptr) = obj_from_bits(namespace_bits).as_ptr()
        && unsafe { crate::object_is_exact_builtin_dict(_py, ptr) }
    {
        let value = unsafe { dict_get_in_place(_py, ptr, name_bits) };
        if exception_pending(_py) {
            return Err(());
        }
        if let Some(value) = value {
            inc_ref_bits(_py, value);
        }
        return Ok(value);
    }

    let value = crate::molt_index(namespace_bits, name_bits);
    if !exception_pending(_py) {
        return Ok(Some(value));
    }
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
    if exception_pending(_py) {
        return Err(());
    }
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

/// One owned projection of the public cache. Unavailable means bootstrap
/// before sys exists, or an explicit runpy/loader execution suppression scope.
pub(crate) enum PublicModuleCache {
    Unavailable,
    Missing,
    Present(u64),
}

pub(crate) fn public_module_cache_lookup(
    py: &PyToken<'_>,
    name: &str,
) -> Result<PublicModuleCache, u64> {
    if execution::python_import_publication_policy(py, name)
        != execution::PythonImportPublication::Normal
    {
        return Ok(PublicModuleCache::Unavailable);
    }
    let Some(sys) = interpreter_sys_module(py) else {
        return Ok(PublicModuleCache::Unavailable);
    };
    let modules = sys_modules_dict_bits(py, sys);

    if exception_pending(py) {
        if let Some(bits) = modules {
            dec_ref_bits(py, bits);
        }
        return Err(MoltObject::none().bits());
    }
    let Some(modules) = modules else {
        return Err(raise_exception::<_>(
            py,
            "RuntimeError",
            "canonical sys.modules is unavailable",
        ));
    };
    let key = attr_name_bits_from_bytes(py, name.as_bytes());
    let Some(key) = key else {
        dec_ref_bits(py, modules);
        return Err(MoltObject::none().bits());
    };
    let borrowed = unsafe { dict_get_in_place(py, ptr_from_bits(modules), key) };
    let value = if exception_pending(py) {
        None
    } else {
        borrowed.inspect(|bits| inc_ref_bits(py, *bits))
    };
    dec_ref_bits(py, key);
    dec_ref_bits(py, modules);
    if exception_pending(py) {
        if let Some(bits) = value {
            dec_ref_bits(py, bits);
        }
        return Err(MoltObject::none().bits());
    }
    Ok(match value {
        Some(bits) => PublicModuleCache::Present(bits),
        None => PublicModuleCache::Missing,
    })
}

fn private_module_cache_admits(bits: u64) -> bool {
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
        // Once sys exists, deletion/replacement is public import state,
        // including arbitrary values. Never revive a private shadow entry.
        match public_module_cache_lookup(_py, name) {
            Ok(PublicModuleCache::Present(bits)) => return bits,
            Ok(PublicModuleCache::Missing) => return MoltObject::none().bits(),
            Ok(PublicModuleCache::Unavailable) => {}
            Err(error) => return error,
        }
        let trace = trace_module_cache();
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        if let Some(bits) = guard.get(name) {
            inc_ref_bits(_py, *bits);
            if trace {
                eprintln!("module cache hit: {name}");
            }
            return *bits;
        }
        if trace {
            eprintln!("module cache miss: {name}");
        }
        MoltObject::none().bits()
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
            Ok(ModuleImportOutcome::Missing { diagnostic_name }) => raise_exception::<_>(
                py,
                "ModuleNotFoundError",
                &format!("No module named '{diagnostic_name}'"),
            ),
            Err(error) => error,
        }
    })
}

/// Resolve the admitted compiled/bootstrap lane. Public values are never copied
/// into private module storage; the spec route owns resolution after a miss.
pub(crate) fn module_import_attempt(name_bits: u64) -> Result<ModuleImportOutcome, u64> {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            trace_bad_module_name_arg(py, "module_import", name_bits);
            return Err(raise_exception::<_>(
                py,
                "TypeError",
                "module name must be str",
            ));
        };
        let private_available = match public_module_cache_lookup(py, &name)? {
            PublicModuleCache::Present(bits) => {
                if let Some(id) = crate::builtins::module_table::module_id_of(&name) {
                    return registered_module_import(py, id, PublicModuleCache::Present(bits));
                }
                if obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                    return Err(raise_exception::<_>(
                        py,
                        "ModuleNotFoundError",
                        &format!("import of {name} halted; None in sys.modules"),
                    ));
                }
                return Ok(ModuleImportOutcome::Imported(bits));
            }
            PublicModuleCache::Missing => false,
            PublicModuleCache::Unavailable => true,
        };
        if execution::python_import_publication_policy(py, &name)
            == execution::PythonImportPublication::Normal
            && let Some(absence) = crate::builtins::platform::known_import_absence(py, &name)
        {
            match absence {
                crate::builtins::platform::KnownImportAbsence::Provider(diagnostic_name) => {
                    return Ok(ModuleImportOutcome::Missing { diagnostic_name });
                }
                crate::builtins::platform::KnownImportAbsence::Dependency(missing) => {
                    return Err(raise_exception::<_>(
                        py,
                        "ModuleNotFoundError",
                        &format!("No module named '{missing}'"),
                    ));
                }
            }
        }
        if let Some(id) = crate::builtins::module_table::module_id_of(&name) {
            return registered_module_import(
                py,
                id,
                if private_available {
                    PublicModuleCache::Unavailable
                } else {
                    PublicModuleCache::Missing
                },
            );
        }
        if private_available {
            let bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(py);
                let guard = cache.lock().unwrap();
                guard
                    .get(&name)
                    .copied()
                    .inspect(|bits| inc_ref_bits(py, *bits))
            };
            if let Some(bits) = bits {
                if !private_module_cache_admits(bits) {
                    dec_ref_bits(py, bits);
                    return Err(raise_exception::<_>(
                        py,
                        "TypeError",
                        "import returned non-module payload",
                    ));
                }
                return Ok(ModuleImportOutcome::Imported(bits));
            }
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
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
        dec_ref_bits(_py, key_bits);
        Ok(())
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
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(code) = to_i64(obj_from_bits(int_code_bits)) else {
        dec_ref_bits(_py, int_code_bits);
        return Err(raise_exception::<_>(_py, "ValueError", "code out of range"));
    };
    if !(1..=0x7fff_ffff).contains(&code) {
        dec_ref_bits(_py, int_code_bits);
        return Err(raise_exception::<_>(_py, "ValueError", "code out of range"));
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
    unsafe {
        set_add_in_place(_py, set_ptr, func_bits, HashContext::SetElement);
    }
    if exception_pending(_py) {
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
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
        unsafe {
            dict_set_in_place(_py, dispatch_ptr, cls_bits, reducer_bits);
        }
        if exception_pending(_py) {
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(proto) = to_i64(obj_from_bits(proto_int_bits)) else {
            dec_ref_bits(_py, proto_int_bits);
            return raise_exception::<_>(_py, "TypeError", "proto must be int");
        };
        dec_ref_bits(_py, proto_int_bits);
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
                let flags_int_bits = molt_int_from_obj(flags_bits, MoltObject::none().bits(), 0);
                dec_ref_bits(_py, flags_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let Some(flags) = to_i64(obj_from_bits(flags_int_bits)) else {
                    dec_ref_bits(_py, flags_int_bits);
                    return raise_exception::<_>(_py, "TypeError", "__flags__ must be int");
                };
                dec_ref_bits(_py, flags_int_bits);
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
    let value = unsafe { dict_get_in_place(py, dictionary, key) }
        .unwrap_or_else(|| MoltObject::none().bits());
    if exception_pending(py) {
        return Err(());
    }
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
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
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
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
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
                dict_set_in_place(_py, extension_ptr, key_bits, code_key_bits);
                if !exception_pending(_py) {
                    dict_set_in_place(_py, inverted_ptr, code_key_bits, key_bits);
                }
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
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
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
    let imported = molt_module_import(name);
    dec_ref_bits(py, name);
    dec_ref_bits(py, imported);
    if exception_pending(py) {
        None
    } else {
        interpreter_sys_module(py)
    }
}

pub(crate) fn sys_modules_dict_bits(py: &PyToken<'_>, sys_bits: u64) -> Option<u64> {
    let sys_ptr = obj_from_bits(sys_bits).as_ptr()?;
    unsafe {
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return None;
        }
        let name = intern_static_name(py, &runtime_state(py).interned.modules_name, b"modules");
        if obj_from_bits(name).is_none() {
            return None;
        }
        let modules = crate::object::accessors::instance_attribute_lookup(py, sys_ptr, name, None);
        if exception_pending(py) {
            return None;
        }
        let modules = match modules {
            Some(bits) => bits,
            None => {
                let ptr = alloc_dict_with_pairs(py, &[]);
                if ptr.is_null() {
                    return None;
                }
                let bits = MoltObject::from_ptr(ptr).bits();
                molt_module_set_attr(sys_bits, name, bits);
                if exception_pending(py) {
                    dec_ref_bits(py, bits);
                    return None;
                }
                bits
            }
        };
        if !obj_from_bits(modules)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT)
        {
            dec_ref_bits(py, modules);
            return raise_exception::<_>(py, "TypeError", "sys.modules must be dict");
        }
        Some(modules)
    }
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
/// Every successful path returns None, including first-init-wins publication.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    module_cache_publish(
        name_bits,
        module_bits,
        ModuleCachePublication::FirstInitialization,
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ModuleCachePublication {
    FirstInitialization,
    // Every admitted extension result, including multi-phase execution,
    // owns its exact publication. Stale private entries cannot substitute it.
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
        if !private_module_cache_admits(module_bits) {
            return raise_exception::<_>(_py, "TypeError", "import returned non-module payload");
        }
        let initializing_builtins = name == "builtins"
            && crate::builtins::module_table::module_initialization_awaits_publication(_py, &name);
        let is_sys = name == "sys";
        let initializing_sys = is_sys
            && crate::builtins::module_table::module_initialization_awaits_publication(_py, &name);
        let bootstrap_sys =
            initializing_sys && runtime_state(_py).interpreter_sys.allows_bootstrap(_py);
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
        if let Err(bits) = execution::on_module_publish(_py, &name, module_bits) {
            return bits;
        }
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
        let sys_modules_policy = execution::python_import_publication_policy(_py, &name);
        let suppress_sys_modules = sys_modules_policy != execution::PythonImportPublication::Normal;
        if trace_cache {
            eprintln!(
                "module cache set: {name} bits=0x{module_bits:x} sys_modules_policy={sys_modules_policy:?}"
            );
        }
        let mut retired_public = Vec::new();
        let (cached_modules, previous) = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let mut guard = cache.lock().unwrap();
            // First-init-wins: if a module is already cached under this name
            // with a valid (non-None, non-zero) module object, do NOT overwrite
            // it.  WASM linked binaries can include duplicate init sequences for
            // the same module (e.g., `abc` pulled in from both `os` and
            // `typing`).  Overwriting destroys class identity — objects created
            // during the first init hold references to the original type objects,
            // but code that fetches the class via MODULE_GET_ATTR on the
            // overwritten module gets a new, incompatible type object.  This
            // causes `super(type, obj)` failures and isinstance mismatches.
            if publication == ModuleCachePublication::FirstInitialization
                && !bootstrap_sys
                && let Some(&existing) = guard.get(&name)
                && existing != 0
                && !obj_from_bits(existing).is_none()
                && existing != module_bits
            {
                if trace_cache {
                    eprintln!(
                        "module cache set: {name} SKIPPED (already cached as 0x{existing:x})"
                    );
                }
                // Still need to sync sys.modules, but use the EXISTING bits.
                // Do NOT dec_ref module_bits — the caller still holds a local
                // reference and will populate the orphan module (harmlessly).
                // The WASM function's epilogue releases its locals normally.
                inc_ref_bits(_py, existing);
                drop(guard);
                let _existing_owner = obj_from_bits(existing)
                    .as_ptr()
                    .map(crate::PtrDropGuard::new);

                // Import bedrock: mirror the effective publication into the
                // ModuleTable slot while its ensure transaction is open
                // (publish-before-exec, invariant I6).
                crate::builtins::module_table::publish_from_cache_set(_py, &name, existing);
                let modules_bits = if suppress_sys_modules {
                    None
                } else {
                    interpreter_sys_module(_py).and_then(|bits| sys_modules_dict_bits(_py, bits))
                };
                let _modules_owner = modules_bits
                    .and_then(|bits| obj_from_bits(bits).as_ptr())
                    .map(crate::PtrDropGuard::new);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if let Some(modules_bits) = modules_bits {
                    let modules_ptr = ptr_from_bits(modules_bits);
                    match sys_modules_set_canonical_name(_py, modules_ptr, &name, existing) {
                        Ok(retired) => retired_public.push(retired),
                        Err(error) => return error,
                    }
                }
                return MoltObject::none().bits();
            }
            // Acquire the cache's new owner before releasing its old one: the
            // borrowed input may be the same object held solely by this entry.
            inc_ref_bits(_py, module_bits);
            let previous = guard.insert(name.clone(), module_bits);
            if bootstrap_sys {
                let entries = guard
                    .iter()
                    .map(|(key, &bits)| {
                        inc_ref_bits(_py, bits);
                        let owner = obj_from_bits(bits).as_ptr().map(crate::PtrDropGuard::new);
                        (key.clone(), bits, owner)
                    })
                    .collect::<Vec<_>>();
                (Some(entries), previous)
            } else {
                (None, previous)
            }
        };

        let _previous_owner = previous
            .and_then(|bits| obj_from_bits(bits).as_ptr())
            .map(crate::PtrDropGuard::new);
        // Import bedrock: mirror the publication into the ModuleTable slot
        // while its ensure transaction is open (publish-before-exec, I6).
        let previous_table = if publication == ModuleCachePublication::Extension {
            crate::builtins::module_table::publish_extension_result(_py, &name, module_bits)
        } else {
            crate::builtins::module_table::publish_from_cache_set(_py, &name, module_bits);
            0
        };
        let _previous_table_owner = obj_from_bits(previous_table)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        let modules_bits = if suppress_sys_modules {
            None
        } else {
            interpreter_sys_module(_py).and_then(|bits| sys_modules_dict_bits(_py, bits))
        };
        let _modules_owner = modules_bits
            .and_then(|bits| obj_from_bits(bits).as_ptr())
            .map(crate::PtrDropGuard::new);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(modules_bits) = modules_bits {
            let modules_ptr = ptr_from_bits(modules_bits);
            if let Some(entries) = cached_modules {
                for (key, bits, _owner) in entries {
                    match sys_modules_set_canonical_name(_py, modules_ptr, &key, bits) {
                        Ok(retired) => retired_public.push(retired),
                        Err(error) => return error,
                    }
                }
            } else {
                match sys_modules_set_canonical_name(_py, modules_ptr, &name, module_bits) {
                    Ok(retired) => retired_public.push(retired),
                    Err(error) => return error,
                }
            }
        }
        // A provider may recursively import builtins. Its base namespace,
        // private cache, table slot, and public cache are all visible now.
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
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if std::env::var("MOLT_TRACE_SYS_MODULE").as_deref() == Ok("1")
                        && exception_pending(_py)
                    {
                        let exc_bits = molt_exception_last_pending();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string());
                        eprintln!("sys module pending after argv/executable: {kind}");
                        dec_ref_bits(_py, exc_bits);
                    }
                    if sys_populate_stdio(_py, sys_ptr).is_err() {
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if sys_populate_bootstrap_metadata(_py, sys_ptr).is_err() {
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if std::env::var("MOLT_TRACE_SYS_MODULE").as_deref() == Ok("1")
                        && exception_pending(_py)
                    {
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
                    let argv_ptr = alloc_string(_py, b"argv");
                    if exe_ptr.is_null() || argv_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    let exe_bits = MoltObject::from_ptr(exe_ptr).bits();
                    let argv_bits = MoltObject::from_ptr(argv_ptr).bits();
                    let exe_val = module_attr_lookup(_py, sys_ptr, exe_bits);
                    let argv_val = module_attr_lookup(_py, sys_ptr, argv_bits);
                    dec_ref_bits(_py, exe_bits);
                    dec_ref_bits(_py, argv_bits);
                    let exe_desc = exe_val
                        .map(|bits| {
                            let obj = obj_from_bits(bits);
                            let desc = string_obj_to_owned(obj)
                                .unwrap_or_else(|| type_name(_py, obj).to_string());
                            dec_ref_bits(_py, bits);
                            desc
                        })
                        .unwrap_or_else(|| "<missing>".to_string());
                    let argv_desc = argv_val
                        .map(|bits| {
                            let obj = obj_from_bits(bits);
                            let desc = type_name(_py, obj).to_string();
                            dec_ref_bits(_py, bits);
                            desc
                        })
                        .unwrap_or_else(|| "<missing>".to_string());
                    eprintln!("sys module set: executable={exe_desc} argv_type={argv_desc}");
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// Legacy import_add_module has already chosen and published this public
/// identity. Retire any different private owner without adopting a public
/// observation into that cache, and use the same table projection transition
/// as other trusted extension publications before callbacks can run.
pub(crate) fn reconcile_extension_publication(py: &PyToken<'_>, name: &str, bits: u64) {
    let private = {
        let cache = crate::builtins::exceptions::internals::module_cache(py);
        let mut guard = cache.lock().unwrap();
        if guard.get(name).is_some_and(|&old| old != bits) {
            guard.remove(name)
        } else {
            None
        }
    };
    let table = crate::builtins::module_table::publish_extension_result(py, name, bits);
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        for old in private.into_iter().chain(std::iter::once(table)) {
            if old != 0 {
                dec_ref_bits(py, old);
            }
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_del(name_bits: u64) -> u64 {
    module_cache_remove(name_bits, None)
}

/// Detach public/private/table owners before running any finalizer. An owned
/// extension rollback supplies its identity so partial publication is removed
/// without deleting a different public replacement or nested import.
pub(crate) fn module_cache_remove(name_bits: u64, expected: Option<u64>) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(py, "TypeError", "module name must be str");
        };
        let saved = if exception_pending(py) {
            let error = molt_exception_last_pending();
            clear_exception(py);
            Some(error)
        } else {
            None
        };
        let sys = interpreter_sys_module(py);

        let modules = if execution::python_import_publication_policy(py, &name)
            == execution::PythonImportPublication::Normal
        {
            sys.and_then(|bits| sys_modules_dict_bits(py, bits))
        } else {
            None
        };
        // Removal detaches dictionary edges but defers their finalizers. No
        // reference is released until public, private and table custody agree.
        let public = modules.and_then(|bits| unsafe {
            let ptr = ptr_from_bits(bits);
            if expected.is_some_and(|own| dict_get_in_place(py, ptr, name_bits) != Some(own)) {
                return None;
            }
            crate::object::ops::dict_del_deferred(py, ptr, name_bits)
        });
        let private = {
            let cache = crate::builtins::exceptions::internals::module_cache(py);
            let mut guard = cache.lock().unwrap();
            if expected.is_none_or(|own| guard.get(&name) == Some(&own)) {
                guard.remove(&name)
            } else {
                None
            }
        };
        let table = crate::builtins::module_table::detach_cache_publication(py, &name, expected);
        // There are no name-based mutations after this release boundary.
        drop(public);
        for bits in [private, Some(table), modules].into_iter().flatten() {
            if bits != 0 {
                dec_ref_bits(py, bits);
            }
        }
        if let Some(error) = saved {
            clear_exception(py);
            crate::molt_exception_set_last(error);
            dec_ref_bits(py, error);
        }
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
        let debug_attr = std::env::var("MOLT_DEBUG_MODULE_GET_ATTR").as_deref() == Ok("1");
        let trace_attrs = trace_module_attrs();
        let trace_attrs_verbose = trace_module_attrs_verbose();
        if std::env::var("MOLT_TRACE_GET_ATTR").is_ok() || trace_attrs_verbose {
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
            if let Some(val) =
                crate::builtins::attributes::attr_lookup_ptr(_py, module_ptr, attr_bits)
            {
                if trace_attrs || trace_attrs_verbose {
                    let module_name =
                        string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                            .unwrap_or_else(|| "<module>".to_string());
                    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                        .unwrap_or_else(|| "<attr>".to_string());
                    if trace_attrs_verbose
                        || attr_name == "_sys"
                        || module_name.contains("importlib")
                    {
                        eprintln!(
                            "molt module attr get module={} attr={}",
                            module_name, attr_name
                        );
                    }
                }
                return val;
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                .unwrap_or_default();
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if debug_attr {
                let mut present = false;
                let order = dict_order(_dict_ptr);
                let entries = order.len() / 2;
                for pair in order.chunks_exact(2) {
                    if let Some(key_name) = string_obj_to_owned(obj_from_bits(pair[0]))
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
            let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
            raise_exception::<_>(_py, "AttributeError", &msg)
        }
    })
}

/// Look up `name` in `sys.modules`, returning a fresh (inc-ref'd) reference to
/// the cached module on hit. Used by [`molt_module_import_from`] for CPython's
/// circular-import recovery path. Returns `None` on a clean miss; if building
/// the lookup key fails it leaves a `MemoryError` pending (the caller observes
/// it via `exception_pending`).
unsafe fn import_from_sys_modules_lookup(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    unsafe {
        let sys_bits = interpreter_sys_module(_py)?;
        let modules_bits = sys_modules_dict_bits(_py, sys_bits)?;
        let modules_ptr = ptr_from_bits(modules_bits);
        let _modules_owner = crate::PtrDropGuard::new(modules_ptr);

        let key_ptr = alloc_string(_py, name.as_bytes());
        if key_ptr.is_null() {
            raise_exception::<u64>(_py, "MemoryError", "out of memory");
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let found = dict_get_in_place(_py, modules_ptr, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        let bits = found?;
        if obj_from_bits(bits).is_none() {
            return None;
        }
        inc_ref_bits(_py, bits);
        Some(bits)
    }
}

/// Best-effort module file origin for an `ImportError` message, mirroring the
/// `(origin)` suffix CPython's `import_from` derives from a module's file
/// origin. Returns `None` — rendered as `"unknown location"` — for modules with
/// no file origin (builtins, frozen, synthetic).
unsafe fn module_file_origin(_py: &PyToken<'_>, module_ptr: *mut u8) -> Option<String> {
    unsafe {
        let dict_bits = module_dict_bits(module_ptr);
        let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let file_key = intern_static_name(_py, &modules_state(_py).module_file_name, b"__file__");
        let file_bits = dict_get_in_place(_py, dict_ptr, file_key)?;
        string_obj_to_owned(obj_from_bits(file_bits))
    }
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
    let full_name = format!("{parent_name}.{child_name}");
    if execution::python_import_publication_policy(py, &full_name)
        == execution::PythonImportPublication::Suppress
    {
        return Ok(());
    }
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
        if let Some(existing_bits) =
            crate::builtins::attributes::attr_lookup_ptr(_py, module_ptr, attr_bits)
        {
            dec_ref_bits(_py, existing_bits);
            return Ok(());
        }
        clear_attribute_error_if_pending(_py);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
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
    crate::with_gil_entry_nopanic!(_py, {
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            // Mirror molt_module_get_attr: a None/pending module operand on an
            // exception-handler continuation path propagates the pending state
            // rather than overwriting it.
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
                let msg = format!(
                    "module attribute access expects module, got type_id={} (bits=0x{:x}) for attr '{}'",
                    type_id, module_bits, attr_name
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            // Step 1: module-aware attribute lookup (resolves PEP 562
            // module-level `__getattr__` identically to molt_module_get_attr).
            if let Some(val) =
                crate::builtins::attributes::attr_lookup_ptr(_py, module_ptr, attr_bits)
            {
                return val;
            }
            // Attribute lookup returned None: a clean miss, or the lookup
            // raised. CPython's IMPORT_FROM converts an `AttributeError` into
            // the submodule-fallback + `ImportError`, but lets any other
            // exception propagate. `clear_attribute_error_if_pending` clears a
            // pending AttributeError (cases: clean miss / AttributeError →
            // nothing left pending → fall through); a still-pending exception
            // afterward is a non-AttributeError that must propagate.
            clear_attribute_error_if_pending(_py);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                .unwrap_or_default();
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            // Step 2: circular-import recovery via sys.modules["{module}.{name}"].
            let full_name = format!("{module_name}.{attr_name}");
            if let Some(submodule_bits) = import_from_sys_modules_lookup(_py, &full_name) {
                return submodule_bits;
            }
            if exception_pending(_py) {
                // Building the sys.modules lookup key failed — propagate.
                return MoltObject::none().bits();
            }
            // Step 3: raise ImportError with CPython's origin suffix.
            let msg = match module_file_origin(_py, module_ptr) {
                Some(path) => {
                    format!("cannot import name '{attr_name}' from '{module_name}' ({path})")
                }
                None => format!(
                    "cannot import name '{attr_name}' from '{module_name}' (unknown location)"
                ),
            };
            raise_exception::<_>(_py, "ImportError", &msg)
        }
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
            unsafe { dict_del_in_place(_py, ptr, name_bits) }
        } else if let Some(method) = unsafe {
            crate::builtins::attr::lookup_special_method(_py, namespace_bits, b"__delitem__")
        } {
            let result = unsafe { call_callable1(_py, method, name_bits) };
            crate::call::discard_owned_call_result(_py, result);
            dec_ref_bits(_py, method);
            !exception_pending(_py)
        } else {
            false
        };
        if deleted && !exception_pending(_py) {
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
fn global_name_suggestion(py: &PyToken<'_>, dictionary: u64, name: &str) -> Option<String> {
    let ptr = crate::builtins::frames::globals_namespace_storage_ptr(py, dictionary)?;
    unsafe {
        let order = crate::builtins::containers::dict_order(ptr);
        use crate::builtins::diagnostic_suggestions::{MAX_CANDIDATE_ITEMS, calculate_suggestion};
        if order.len() / 2 >= MAX_CANDIDATE_ITEMS {
            return None;
        }
        let mut candidates = Vec::with_capacity(order.len() / 2);
        for pair in order.chunks_exact(2) {
            let key = obj_from_bits(pair[0]).as_ptr()?;
            if object_type_id(key) != TYPE_ID_STRING {
                return None;
            }
            let bytes = std::slice::from_raw_parts(string_bytes(key), string_len(key));
            candidates.push(std::str::from_utf8(bytes).ok()?);
        }
        calculate_suggestion(name, &candidates).map(str::to_owned)
    }
}

/// The single lookup path for Python activations and explicit runtime module
/// dictionaries. The caller selects namespace custody; this primitive never
/// replaces a captured builtins dictionary after a miss.
fn lookup_global_namespace(
    py: &PyToken<'_>,
    module_bits: u64,
    module_label: &str,
    globals_bits: u64,
    builtins_bits: u64,
    name_bits: u64,
    name: &str,
) -> u64 {
    let trace_mode = trace_module_globals_mode();
    let trace_globals = trace_mode != TraceModuleGlobalsMode::Off
        && (trace_mode == TraceModuleGlobalsMode::Verbose
            || name == "_SYS_FLAGS_SEQUENCE_FIELDS"
            || name == "_FlagsTuple");
    match lookup_namespace_item(py, globals_bits, name_bits) {
        Ok(Some(value)) => {
            if trace_globals {
                eprintln!(
                    "molt module_get_global hit module={} name={} module_bits=0x{:x} dict_bits=0x{:x} val_type={}",
                    module_label,
                    name,
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
                    module_label, name, module_bits, globals_bits,
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
            module_label,
            name,
            exception_pending(py),
        );
    }
    let suggestion = global_name_suggestion(py, globals_bits, name)
        .or_else(|| global_name_suggestion(py, builtins_bits, name));
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
        let trace_mode = trace_module_globals_mode();
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        let trace_globals = trace_mode != TraceModuleGlobalsMode::Off
            && (trace_mode == TraceModuleGlobalsMode::Verbose
                || name == "_SYS_FLAGS_SEQUENCE_FIELDS"
                || name == "_FlagsTuple");
        if trace_globals {
            eprintln!(
                "molt module_get_global enter name={} module_bits=0x{:x} pending={}",
                name,
                module_bits,
                exception_pending(_py),
            );
        }
        let active_globals = frame_stack_active_globals_bits();
        if active_globals != 0 {
            if crate::builtins::frames::globals_namespace_storage_bits(_py, active_globals)
                .is_some()
            {
                return lookup_global_namespace(
                    _py,
                    module_bits,
                    "<frame>",
                    active_globals,
                    crate::builtins::frames::frame_stack_active_builtins_bits(),
                    name_bits,
                    &name,
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
                        name,
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
                    module_bits, name,
                ),
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if exception_pending(_py) {
                    if trace_globals {
                        eprintln!(
                            "molt module_get_global early_type name={} module_bits=0x{:x} type_id={} pending={}",
                            name,
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
                        name,
                    ),
                );
            }
            let globals = module_dict_bits(module_ptr);
            if !obj_from_bits(globals)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT)
            {
                return raise_exception::<_>(_py, "TypeError", "module dict missing");
            }
            let module_label = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                .unwrap_or_else(|| "<module>".to_string());
            let builtins = crate::builtins::frames::frame_effective_builtins_bits(_py, globals);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            lookup_global_namespace(
                _py,
                module_bits,
                &module_label,
                globals,
                builtins,
                name_bits,
                &name,
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
                Some(ptr) => ptr,
                None if exception_pending(_py) => return MoltObject::none().bits(),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "SystemError",
                        "active globals is not a dictionary",
                    );
                }
            }
        } else {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            match module_dict_ptr(_py, module_bits) {
                Ok(ptr) => ptr,
                Err(bits) => return bits,
            }
        };
        if dict_del_in_place(_py, dict_ptr, name_bits) {
            return MoltObject::none().bits();
        }
        if exception_pending(_py) || missing_ok {
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
    if std::env::var("MOLT_TRACE_SET_ATTR").as_deref() == Ok("1") {
        eprintln!(
            "module_set_attr: mod=0x{:x} attr=0x{:x} val=0x{:x}",
            module_bits, attr_bits, val_bits
        );
    }
    crate::with_gil_entry_nopanic!(_py, {
        let trace_attrs = trace_module_attrs();
        let trace_attrs_verbose = trace_module_attrs_verbose();
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(_py, "TypeError", "module attribute set expects module");
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute set expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if trace_attrs {
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                    .unwrap_or_else(|| "<module>".to_string());
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                    .unwrap_or_else(|| "<attr>".to_string());
                if trace_attrs_verbose || attr_name == "_sys" || module_name.contains("importlib") {
                    eprintln!(
                        "molt module attr set module={} attr={}",
                        module_name, attr_name
                    );
                }
            }
            let annotations_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotations_name,
                b"__annotations__",
            );
            if crate::object::ops_compare::string_storage_equal(attr_bits, annotations_bits) {
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                if pep649_enabled(_py) {
                    let annotate_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotate_name,
                        b"__annotate__",
                    );
                    let none_bits = MoltObject::none().bits();
                    dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                }
                return MoltObject::none().bits();
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
                    let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                    if !callable_ok {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotate__ must be callable or None",
                        );
                    }
                }
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                if !val_obj.is_none() {
                    dict_del_in_place(_py, dict_ptr, annotations_bits);
                }
                return MoltObject::none().bits();
            }
            let override_value =
                match execution::module_metadata_override_bits(_py, module_bits, attr_bits) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
            let effective_val_bits = override_value.map_or(val_bits, |(bits, _)| bits);
            dict_set_in_place(_py, dict_ptr, attr_bits, effective_val_bits);
            if exception_pending(_py) {
                if let Some((bits, true)) = override_value {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            if let Err(bits) = execution::after_module_metadata_set(
                _py,
                module_bits,
                attr_bits,
                effective_val_bits,
            ) {
                if let Some((owned, true)) = override_value {
                    dec_ref_bits(_py, owned);
                }
                return bits;
            }
            if let Some((bits, true)) = override_value {
                dec_ref_bits(_py, bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_import_star(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        import_star::import_star(_py, src_bits, dst_bits)
    })
}

#[cfg(test)]
#[path = "namespace_delete_tests.rs"]
mod namespace_delete_tests;

#[cfg(test)]
#[path = "namespace_lookup_tests.rs"]
mod namespace_lookup_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn namespace_del_normalizes_non_key_mapping_failures() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let key = MoltObject::from_ptr(alloc_string(_py, b"absent")).bits();
            // A TypeError from the receiver is normalized, not just KeyError.
            let _ = molt_namespace_del(MoltObject::none().bits(), key);
            assert!(exception_pending(_py));
            let error = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                error,
                "NameError"
            ));
            clear_exception(_py);
            dec_ref_bits(_py, error);
            dec_ref_bits(_py, key);
        });
    }

    #[test]
    fn namespace_get_preserves_values_absence_and_non_key_errors() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let key = MoltObject::from_ptr(alloc_string(_py, b"bound")).bits();
            let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
            let ns = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, value])).bits();
            let missing = crate::missing_bits(_py);
            let loaded = molt_namespace_get(ns, key, missing);
            assert_eq!(loaded, value);
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, loaded);

            let absent = MoltObject::from_ptr(alloc_string(_py, b"absent")).bits();
            assert_eq!(molt_namespace_get(ns, absent, missing), missing);
            assert!(!exception_pending(_py));

            // Only a KeyError from the namespace means fall through. TypeError
            // from a non-mapping (or a bad hash) must remain pending.
            let _ = molt_namespace_get(MoltObject::none().bits(), key, missing);
            assert!(exception_pending(_py));
            let error = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                error,
                "TypeError"
            ));
            let _ = molt_namespace_get(ns, key, missing);
            let retained = molt_exception_last_pending();
            assert_eq!(retained, error, "a pending exception must not be consumed");
            clear_exception(_py);
            dec_ref_bits(_py, retained);
            dec_ref_bits(_py, error);
            dec_ref_bits(_py, absent);
            dec_ref_bits(_py, ns);
            dec_ref_bits(_py, value);
            dec_ref_bits(_py, key);
        });
    }

    struct ModuleCacheRestore {
        name_bits: u64,
        previous_bits: u64,
    }

    impl ModuleCacheRestore {
        fn new(_py: &PyToken<'_>, name_bits: u64) -> Self {
            let previous_bits = molt_module_cache_get(name_bits);
            let _ = crate::molt_exception_clear();
            let _ = molt_module_cache_del(name_bits);
            let _ = crate::molt_exception_clear();
            Self {
                name_bits,
                previous_bits,
            }
        }

        fn name_bits(&self) -> u64 {
            self.name_bits
        }
    }

    impl Drop for ModuleCacheRestore {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(_py, {
                let _ = crate::molt_exception_clear();
                let _ = molt_module_cache_del(self.name_bits);
                let _ = crate::molt_exception_clear();
                if !obj_from_bits(self.previous_bits).is_none() {
                    let restore_bits = molt_module_cache_set(self.name_bits, self.previous_bits);
                    if !obj_from_bits(restore_bits).is_none() {
                        dec_ref_bits(_py, restore_bits);
                    }
                    let _ = crate::molt_exception_clear();
                    dec_ref_bits(_py, self.previous_bits);
                }
                dec_ref_bits(_py, self.name_bits);
            });
        }
    }

    fn assert_pending_exception_class(_py: &PyToken<'_>, expected: &str) {
        assert!(exception_pending(_py));
        let exc_bits = molt_exception_last_pending();
        assert!(!obj_from_bits(exc_bits).is_none());
        assert!(
            crate::builtins::exceptions::exception_matches_builtin_name(_py, exc_bits, expected),
            "expected pending exception to be {expected}"
        );
        dec_ref_bits(_py, exc_bits);
        let _ = crate::molt_exception_clear();
        assert!(!exception_pending(_py));
    }

    #[test]
    fn modules_runtime_state_is_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            let dispatch_bits =
                copyreg_dict_slot_bits(_py, &state.modules.copyreg_dispatch_table_bits);
            assert_ne!(dispatch_bits, 0);
            assert_ne!(
                state
                    .modules
                    .copyreg_dispatch_table_bits
                    .load(Ordering::Acquire),
                0
            );
            let constructors_bits =
                copyreg_set_slot_bits(_py, &state.modules.copyreg_constructor_registry_bits);
            assert_ne!(constructors_bits, 0);
            assert_ne!(
                state
                    .modules
                    .copyreg_constructor_registry_bits
                    .load(Ordering::Acquire),
                0
            );
            let import_name =
                intern_static_name(_py, &state.modules.runpy_import_dunder_name, b"__import__");
            assert_ne!(import_name, 0);
            assert_ne!(
                state
                    .modules
                    .runpy_import_dunder_name
                    .load(Ordering::Acquire),
                0
            );

            modules_clear_runtime_state(_py, state);

            for slot in state.modules.object_slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }

    #[test]
    fn raw_module_allocation_seeds_public_name_metadata() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let name_ptr = alloc_string(_py, b"synthetic_runtime_module");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let module_ptr = alloc_module_obj(_py, name_bits);
                assert!(!module_ptr.is_null());
                dec_ref_bits(_py, name_bits);
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let name_key_ptr = alloc_string(_py, b"__name__");
                assert!(!name_key_ptr.is_null());
                let name_key_bits = MoltObject::from_ptr(name_key_ptr).bits();
                let found_name_bits =
                    module_attr_lookup(_py, module_ptr, name_key_bits).expect("module __name__");
                let found_name = string_obj_to_owned(obj_from_bits(found_name_bits));
                assert_eq!(found_name.as_deref(), Some("synthetic_runtime_module"));
                dec_ref_bits(_py, found_name_bits);
                dec_ref_bits(_py, name_key_bits);

                let missing_key_ptr = alloc_string(_py, b"tolist");
                assert!(!missing_key_ptr.is_null());
                let missing_key_bits = MoltObject::from_ptr(missing_key_ptr).bits();
                let has_bits = crate::molt_has_attr_name(module_bits, missing_key_bits);
                assert_eq!(has_bits, MoltObject::from_bool(false).bits());
                assert!(
                    !exception_pending(_py),
                    "hasattr(module, missing) must clear AttributeError"
                );
                dec_ref_bits(_py, missing_key_bits);
                dec_ref_bits(_py, module_bits);
            }
        });
    }

    #[test]
    fn generated_native_providers_own_self_and_public_aliases_own_lookup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let builtins =
                    crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
                let io = crate::test_support::NativeProviderTestNamespace::new(py, "_io");
                let builtin_dict = obj_from_bits(module_dict_bits(
                    obj_from_bits(builtins.bits()).as_ptr().unwrap(),
                ))
                .as_ptr()
                .unwrap();
                let io_dict =
                    obj_from_bits(module_dict_bits(obj_from_bits(io.bits()).as_ptr().unwrap()))
                        .as_ptr()
                        .unwrap();
                let open_name = attr_name_bits_from_bytes(py, b"open").unwrap();
                let self_name = attr_name_bits_from_bytes(py, b"__self__").unwrap();
                assert!(dict_get_in_place(py, builtin_dict, open_name).is_none());
                let open = dict_get_in_place(py, io_dict, open_name).unwrap();
                let owner = crate::molt_get_attr_name(open, self_name);
                assert_eq!(owner, io.bits());
                dec_ref_bits(py, owner);
                // Model the runtime initializer's exact alias publication;
                // this unit fixture deliberately has no generated module table.
                dict_set_in_place(py, builtin_dict, open_name, open);
                let alias = crate::builtins::functions::lookup_builtin_name(py, "open").unwrap();
                assert_eq!(alias, open);
                dec_ref_bits(py, alias);
                dict_set_in_place(py, builtin_dict, open_name, MoltObject::from_int(37).bits());
                assert_eq!(
                    crate::builtins::functions::lookup_builtin_name(py, "open"),
                    Some(MoltObject::from_int(37).bits())
                );
                crate::dict_del_in_place(py, builtin_dict, open_name);
                assert_eq!(
                    crate::builtins::functions::lookup_builtin_name(py, "open"),
                    None
                );
                assert_eq!(dict_get_in_place(py, io_dict, open_name), Some(open));
                // A compiled named materializer cannot use its raw target to
                // refill a deleted known public builtin.
                let absent = crate::molt_func_new_builtin_named(
                    open_name,
                    fn_addr!(crate::molt_open_builtin),
                    0,
                    8,
                );
                assert!(exception_pending(py));
                assert_pending_exception_class(py, "NameError");
                dec_ref_bits(py, absent);
                for bits in [open_name, self_name] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn module_get_global_uses_captured_builtins_for_hits_misses_and_suggestions() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let string = |value: &[u8]| MoltObject::from_ptr(alloc_string(py, value)).bits();
            let builtins_name = string(b"builtins");
            let cache_restore = ModuleCacheRestore::new(py, builtins_name);
            let cached_module = alloc_module_obj(py, cache_restore.name_bits());
            let cached_bits = MoltObject::from_ptr(cached_module).bits();
            molt_module_cache_set(cache_restore.name_bits(), cached_bits);
            let cached_dict = unsafe { module_dict_bits(cached_module) };
            let captured_ptr = alloc_dict_with_pairs(py, &[]);
            let captured = MoltObject::from_ptr(captured_ptr).bits();
            let globals_ptr = alloc_dict_with_pairs(py, &[]);
            let globals = MoltObject::from_ptr(globals_ptr).bits();
            let answer = string(b"answer");
            let len = string(b"len");
            let custom = string(b"custom_token");
            let runtime_only = string(b"runtime_only");
            let builtins_key = string(b"__builtins__");
            let module_name = string(b"unrelated_module");
            let module_ptr = alloc_module_obj(py, module_name);
            let module = MoltObject::from_ptr(module_ptr).bits();
            unsafe {
                dict_set_in_place(py, globals_ptr, answer, MoltObject::from_int(42).bits());
                dict_set_in_place(py, captured_ptr, len, MoltObject::from_int(11).bits());
                dict_set_in_place(py, captured_ptr, custom, MoltObject::from_int(1).bits());
                let cached_ptr = obj_from_bits(cached_dict).as_ptr().unwrap();
                dict_set_in_place(py, cached_ptr, len, MoltObject::from_int(33).bits());
                dict_set_in_place(py, cached_ptr, runtime_only, MoltObject::from_int(1).bits());
                let unrelated = obj_from_bits(module_dict_bits(module_ptr))
                    .as_ptr()
                    .unwrap();
                dict_set_in_place(py, unrelated, answer, MoltObject::from_int(99).bits());
                // A replaced __builtins__ entry must not reinterpret this activation.
                dict_set_in_place(py, globals_ptr, builtins_key, cached_dict);
            }
            inc_ref_bits(py, globals);
            inc_ref_bits(py, captured);
            crate::builtins::frames::frame_stack_push_owned(py, 0, globals, captured, 0);
            assert_eq!(
                molt_module_get_global(module, answer),
                MoltObject::from_int(42).bits()
            );
            assert_eq!(
                molt_module_get_global(module, len),
                MoltObject::from_int(11).bits()
            );
            // Explicit module attribute access is not Python LOAD_GLOBAL.
            assert_eq!(
                molt_module_get_attr(module, answer),
                MoltObject::from_int(99).bits()
            );
            assert!(unsafe { dict_del_in_place(py, captured_ptr, len) });
            assert!(obj_from_bits(molt_module_get_global(module, len)).is_none());
            assert_pending_exception_class(py, "NameError");
            assert!(obj_from_bits(molt_module_get_global(module, runtime_only)).is_none());
            assert_pending_exception_class(py, "NameError");

            for (typo, expected) in [
                (b"custom_toke".as_slice(), Some("custom_token")),
                (b"runtime_onl".as_slice(), None),
            ] {
                let typo = string(typo);
                let missing = molt_module_get_global(module, typo);
                assert!(obj_from_bits(missing).is_none());
                let exception = molt_exception_last_pending();
                let ptr = obj_from_bits(exception).as_ptr().unwrap();
                let message = format_exception_with_traceback(py, ptr);
                match expected {
                    Some(candidate) => {
                        assert!(message.contains(&format!("Did you mean: '{candidate}'?")))
                    }
                    None => assert!(!message.contains("Did you mean:")),
                }
                dec_ref_bits(py, exception);
                assert_pending_exception_class(py, "NameError");
                dec_ref_bits(py, typo);
            }
            assert!(unsafe { dict_del_in_place(py, captured_ptr, custom) });
            assert!(obj_from_bits(molt_module_get_global(module, len)).is_none());
            assert_pending_exception_class(py, "NameError");
            crate::builtins::frames::frame_stack_pop(py);
            assert_eq!(
                molt_module_get_global(module, answer),
                MoltObject::from_int(99).bits()
            );
            assert_eq!(
                molt_module_get_global(module, len),
                MoltObject::from_int(33).bits()
            );
            for bits in [
                module,
                module_name,
                globals,
                captured,
                cached_bits,
                answer,
                len,
                custom,
                runtime_only,
                builtins_key,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn module_get_global_captured_unavailable_builtins_does_not_switch_to_cache() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let builtins_name = MoltObject::from_ptr(alloc_string(py, b"builtins")).bits();
            let cache_restore = ModuleCacheRestore::new(py, builtins_name);
            let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            inc_ref_bits(py, globals);
            crate::builtins::frames::frame_stack_push_owned(py, 0, globals, 0, 0);
            let module_ptr = alloc_module_obj(py, cache_restore.name_bits());
            let module = MoltObject::from_ptr(module_ptr).bits();
            molt_module_cache_set(cache_restore.name_bits(), module);
            assert!(!exception_pending(py));
            assert_eq!(
                crate::builtins::frames::frame_stack_active_builtins(),
                Some(0)
            );
            assert_eq!(
                crate::builtins::frames::frame_effective_builtins_bits(py, globals),
                0
            );
            for spelling in ["len", "list", "ValueError", "molt_len"] {
                let name = MoltObject::from_ptr(alloc_string(py, spelling.as_bytes())).bits();
                unsafe {
                    let dictionary = obj_from_bits(module_dict_bits(module_ptr))
                        .as_ptr()
                        .unwrap();
                    dict_set_in_place(py, dictionary, name, MoltObject::from_int(73).bits());
                }
                let found = molt_module_get_global(MoltObject::none().bits(), name);
                assert!(obj_from_bits(found).is_none());
                assert!(exception_pending(py));
                let exception = molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "NameError"
                ));
                clear_exception(py);
                dec_ref_bits(py, exception);
                dec_ref_bits(py, name);
            }
            assert_eq!(
                crate::builtins::functions::lookup_builtin_name(py, "len"),
                None
            );
            assert!(!exception_pending(py));
            unsafe {
                assert!(molt_cpython_abi::api::eval::PyEval_GetBuiltins().is_null());
                assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
                molt_cpython_abi::api::errors::PyErr_Clear();
            }
            clear_exception(py);
            crate::builtins::frames::frame_stack_pop(py);
            assert_eq!(crate::builtins::frames::frame_stack_active_builtins(), None);
            assert_eq!(
                crate::builtins::frames::frame_effective_builtins_bits(py, globals),
                unsafe { module_dict_bits(module_ptr) }
            );
            assert_eq!(
                crate::builtins::functions::lookup_builtin_name(py, "len"),
                Some(MoltObject::from_int(73).bits())
            );
            unsafe {
                let view = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
                assert!(!view.is_null());
                assert_eq!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(view)
                        .unwrap()
                        .bits(),
                    module_dict_bits(module_ptr)
                );
            }
            for bits in [globals, module] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn module_get_global_respects_present_builtins_dict_miss() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtins_name_ptr = alloc_string(_py, b"builtins");
            assert!(!builtins_name_ptr.is_null());
            let builtins_name_bits = MoltObject::from_ptr(builtins_name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(_py, builtins_name_bits);

            let builtins_module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
            assert!(!builtins_module_ptr.is_null());
            let builtins_module_bits = MoltObject::from_ptr(builtins_module_ptr).bits();
            let set_result = molt_module_cache_set(cache_restore.name_bits(), builtins_module_bits);
            assert!(obj_from_bits(set_result).is_none());
            assert!(!exception_pending(_py));

            let module_name_ptr = alloc_string(_py, b"empty_builtins_lookup_module");
            assert!(!module_name_ptr.is_null());
            let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
            let module_ptr = alloc_module_obj(_py, module_name_bits);
            dec_ref_bits(_py, module_name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let builtins_dict_bits = unsafe { module_dict_bits(builtins_module_ptr) };
            let builtins_dict_ptr = obj_from_bits(builtins_dict_bits)
                .as_ptr()
                .expect("builtins module dictionary");
            assert_eq!(unsafe { object_type_id(builtins_dict_ptr) }, TYPE_ID_DICT);

            for (index, builtin_name) in ["len", "globals", "locals", "vars", "__import__"]
                .into_iter()
                .enumerate()
            {
                let name_ptr = alloc_string(_py, builtin_name.as_bytes());
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let published_bits = MoltObject::from_int(index as i64 + 1).bits();
                unsafe {
                    dict_set_in_place(_py, builtins_dict_ptr, name_bits, published_bits);
                }
                assert!(!exception_pending(_py));

                let loaded_bits = molt_module_get_global(module_bits, name_bits);
                assert_eq!(
                    loaded_bits, published_bits,
                    "published builtins.{builtin_name} must win over runtime synthesis"
                );
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, loaded_bits);

                assert!(unsafe {
                    crate::object::ops::dict_del_in_place(_py, builtins_dict_ptr, name_bits)
                });
                assert!(!exception_pending(_py));
                let missing_bits = molt_module_get_global(module_bits, name_bits);
                assert!(
                    obj_from_bits(missing_bits).is_none(),
                    "deleting builtins.{builtin_name} from a published dictionary must be authoritative"
                );
                assert_pending_exception_class(_py, "NameError");
                dec_ref_bits(_py, name_bits);
            }

            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, builtins_module_bits);
        });
    }

    #[test]
    fn module_cache_publication_preserves_duplicate_and_aliased_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let refcount = |bits| {
                let ptr = obj_from_bits(bits).as_ptr().expect("module object");
                unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
            };
            // Exercise both bootstrap publication without sys.modules and the
            // ordinary mirrored publication path.
            for mirror_sys_modules in [false, true] {
                let sys_name = MoltObject::from_ptr(alloc_string(py, b"sys")).bits();
                let sys_restore = ModuleCacheRestore::new(py, sys_name);
                let sys_bits = if mirror_sys_modules {
                    let bits = molt_module_new(sys_restore.name_bits());
                    assert!(!obj_from_bits(bits).is_none());
                    assert!(
                        obj_from_bits(
                            crate::builtins::module_table::publish_interpreter_sys_for_test(
                                py, bits
                            )
                        )
                        .is_none()
                    );
                    bits
                } else {
                    MoltObject::none().bits()
                };
                let name =
                    MoltObject::from_ptr(alloc_string(py, b"_molt_publication_ownership")).bits();
                let cache_restore = ModuleCacheRestore::new(py, name);
                // This proves cache ownership, including a sole-owner
                // borrowed replacement. molt_module_new can additionally
                // anchor the first module as the runtime intrinsic registry.
                let first_ptr = alloc_module_obj(py, name);
                let duplicate_ptr = alloc_module_obj(py, name);
                assert!(!first_ptr.is_null());
                assert!(!duplicate_ptr.is_null());
                let first = MoltObject::from_ptr(first_ptr).bits();
                let duplicate = MoltObject::from_ptr(duplicate_ptr).bits();
                assert_ne!(first, duplicate);
                assert_eq!(refcount(first), 1, "fixture has exactly one caller owner");
                assert_eq!(
                    refcount(duplicate),
                    1,
                    "fixture has exactly one caller owner"
                );
                assert!(obj_from_bits(molt_module_cache_set(name, first)).is_none());
                assert!(!exception_pending(py));
                let first_owners = refcount(first);
                let duplicate_owners = refcount(duplicate);

                for _ in 0..3 {
                    let result = molt_module_cache_set(name, duplicate);
                    assert!(
                        obj_from_bits(result).is_none(),
                        "duplicate publication must not return a borrowed cache owner"
                    );
                    // Model an unbound owned-result sink, as used by WASM.
                    dec_ref_bits(py, result);
                    assert!(!exception_pending(py));
                    assert_eq!(refcount(first), first_owners);
                    assert_eq!(refcount(duplicate), duplicate_owners);
                    let cached = molt_module_cache_get(name);
                    assert_eq!(cached, first, "first initialization keeps its identity");
                    dec_ref_bits(py, cached);
                    if mirror_sys_modules {
                        let modules =
                            sys_modules_dict_bits(py, sys_bits).expect("published sys.modules");
                        let dict = obj_from_bits(modules).as_ptr().unwrap();
                        let _modules_owner = crate::PtrDropGuard::new(dict);
                        assert_eq!(unsafe { dict_get_in_place(py, dict, name) }, Some(first));
                    }
                }

                dec_ref_bits(py, duplicate);
                dec_ref_bits(py, first);
                let cache_owners = refcount(first);
                if !mirror_sys_modules {
                    assert_eq!(cache_owners, 1, "the cache is now the sole owner");
                }
                // The argument is borrowed from the live cache entry. A
                // release-before-retain replacement would destroy it here.
                for _ in 0..3 {
                    let result = molt_module_cache_set(name, first);
                    assert!(obj_from_bits(result).is_none());
                    dec_ref_bits(py, result);
                    assert!(!exception_pending(py));
                    assert_eq!(refcount(first), cache_owners);
                }
                drop(cache_restore);
                dec_ref_bits(py, sys_bits);
                drop(sys_restore);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn sys_module_cache_set_does_not_leave_pending_exception() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(_py, name_bits);
            let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits = molt_module_cache_set(cache_restore.name_bits(), module_bits);
            // Publication is a mutator, never a borrowed module handoff.
            assert!(obj_from_bits(result_bits).is_none());
            assert!(
                !exception_pending(_py),
                "sys module registration must not leave a pending exception"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, module_bits);
        });
    }

    #[test]
    fn import_outcome_public_values_never_poison_or_revive_private_cache() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sys_name = attr_name_bits_from_bytes(py, b"sys").unwrap();
            let _sys_restore = ModuleCacheRestore::new(py, sys_name);
            let text = "_molt_public_cache_outcome_probe";
            let name = attr_name_bits_from_bytes(py, text.as_bytes()).unwrap();
            let _name_restore = ModuleCacheRestore::new(py, name);
            let bootstrap = molt_module_new(name);
            molt_module_cache_set(name, bootstrap);
            let sys = molt_module_new(sys_name);
            crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
            let modules_bits = sys_modules_dict_bits(py, sys).unwrap();
            let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
            let _modules_owner = crate::PtrDropGuard::new(modules);
            let module = molt_module_new(name);
            let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            for value in [MoltObject::from_int(42).bits(), module, dictionary] {
                unsafe { dict_set_in_place(py, modules, name, value) };
                let cached = molt_module_cache_get(name);
                assert_eq!(cached, value);
                dec_ref_bits(py, cached);
                let imported = match module_import_attempt(name).unwrap() {
                    ModuleImportOutcome::Imported(bits) => bits,
                    ModuleImportOutcome::Missing { .. } => panic!("visible cache hit lost"),
                };
                assert_eq!(imported, value);
                dec_ref_bits(py, imported);
                {
                    let cache = crate::builtins::exceptions::internals::module_cache(py);
                    assert_eq!(cache.lock().unwrap().get(text).copied(), Some(bootstrap));
                }
                assert!(unsafe { dict_del_in_place(py, modules, name) });
                assert!(obj_from_bits(molt_module_cache_get(name)).is_none());
                assert!(matches!(module_import_attempt(name),
                    Ok(ModuleImportOutcome::Missing { diagnostic_name }) if diagnostic_name == text));
                assert!(obj_from_bits(molt_module_import(name)).is_none());
                assert_pending_exception_class(py, "ModuleNotFoundError");
            }
            for bits in [module, dictionary, bootstrap, sys] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn import_outcome_provider_parent_is_missing_and_public_cache_has_precedence() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            let previous = crate::object::ops_sys::runtime_target_python_info(state);
            let mut target = previous.clone();
            target.minor = 13;
            *state.sys_version_info.lock().unwrap() = Some(target);
            let sys_name = attr_name_bits_from_bytes(py, b"sys").unwrap();
            let _sys_restore = ModuleCacheRestore::new(py, sys_name);
            let sys = molt_module_new(sys_name);
            crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
            let modules_bits = sys_modules_dict_bits(py, sys).unwrap();
            let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
            let _modules_owner = crate::PtrDropGuard::new(modules);
            let name = attr_name_bits_from_bytes(py, b"msilib.schema").unwrap();
            assert!(matches!(module_import_attempt(name),
                Ok(ModuleImportOutcome::Missing { diagnostic_name }) if diagnostic_name == "msilib"));
            assert!(!exception_pending(py));
            let replacement = MoltObject::from_int(42).bits();
            unsafe { dict_set_in_place(py, modules, name, replacement) };
            assert!(matches!(module_import_attempt(name),
                Ok(ModuleImportOutcome::Imported(bits)) if bits == replacement));
            assert!(unsafe { dict_del_in_place(py, modules, name) });
            #[cfg(not(target_os = "windows"))]
            {
                let dependency =
                    attr_name_bits_from_bytes(py, b"multiprocessing.popen_spawn_win32").unwrap();
                assert!(module_import_attempt(dependency).is_err());
                assert_pending_exception_class(py, "ModuleNotFoundError");
                dec_ref_bits(py, dependency);
            }
            dec_ref_bits(py, name);
            dec_ref_bits(py, sys);
            *state.sys_version_info.lock().unwrap() = Some(previous);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn import_attempt_preserves_pending_failure_even_when_its_text_names_the_target() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name = attr_name_bits_from_bytes(_py, b"pkg.child").unwrap();
            crate::builtins::exceptions::exception_stack_push();
            let raised =
                raise_exception::<u64>(_py, "ModuleNotFoundError", "No module named 'pkg.child'");
            dec_ref_bits(_py, raised);
            let original = molt_exception_last_pending();
            assert!(module_import_attempt(name).is_err());
            let observed = molt_exception_last_pending();
            assert_eq!(observed, original);
            assert!(exception_pending(_py));
            dec_ref_bits(_py, observed);
            dec_ref_bits(_py, original);
            clear_exception(_py);
            crate::builtins::exceptions::exception_stack_pop(_py);
            dec_ref_bits(_py, name);
        });
    }

    #[test]
    fn import_attempt_rejects_invalid_cache_hits_without_reporting_a_miss() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let sys_name = attr_name_bits_from_bytes(_py, b"sys").unwrap();
            let _sys_restore = ModuleCacheRestore::new(_py, sys_name);
            let name_text = "_molt_invalid_import_outcome_cache";
            let name = attr_name_bits_from_bytes(_py, name_text.as_bytes()).unwrap();
            let _restore = ModuleCacheRestore::new(_py, name);
            let string = MoltObject::from_ptr(alloc_string(_py, b"not a module")).bits();
            let refcount = || unsafe {
                (*crate::object::header_from_obj_ptr(obj_from_bits(string).as_ptr().unwrap()))
                    .ref_count_snapshot()
            };
            for payload in [MoltObject::none().bits(), string] {
                inc_ref_bits(_py, payload);
                {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    assert!(
                        cache
                            .lock()
                            .unwrap()
                            .insert(name_text.to_owned(), payload)
                            .is_none()
                    );
                }
                let owners = refcount();
                assert!(module_import_attempt(name).is_err());
                assert_pending_exception_class(_py, "TypeError");
                assert_eq!(
                    refcount(),
                    owners,
                    "failed admission must release its dispatch owner"
                );
                let cached = {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let mut guard = cache.lock().unwrap();
                    guard.remove(name_text).unwrap()
                };
                dec_ref_bits(_py, cached);
            }
            dec_ref_bits(_py, string);
        });
    }

    #[test]
    fn prepare_from_import_child_preserves_existing_package_attribute() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let module_name_ptr = alloc_string(_py, b"pkg");
                assert!(!module_name_ptr.is_null());
                let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
                let module_ptr = alloc_module_obj(_py, module_name_bits);
                assert!(!module_ptr.is_null());
                dec_ref_bits(_py, module_name_bits);
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let existing_ptr = alloc_string(_py, b"class-export");
                assert!(!existing_ptr.is_null());
                let existing_bits = MoltObject::from_ptr(existing_ptr).bits();
                let module_dict = module_dict_bits(module_ptr);
                let module_dict_ptr = obj_from_bits(module_dict)
                    .as_ptr()
                    .expect("module dict pointer");
                assert_eq!(object_type_id(module_dict_ptr), TYPE_ID_DICT);
                dict_set_str_key_bits(_py, module_dict_ptr, "Tensor", existing_bits)
                    .expect("set exported Tensor");

                let attr_ptr = alloc_string(_py, b"Tensor");
                assert!(!attr_ptr.is_null());
                let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
                let child_ptr = alloc_string(_py, b"pkg.Tensor");
                assert!(!child_ptr.is_null());
                let child_bits = MoltObject::from_ptr(child_ptr).bits();

                let result = prepare_from_import_child(_py, module_bits, attr_bits, child_bits);
                assert!(
                    !exception_pending(_py),
                    "existing package attr prepare path must not leave an exception"
                );
                assert!(result.is_ok());

                let found_bits =
                    dict_get_in_place(_py, module_dict_ptr, attr_bits).expect("Tensor attr");
                assert_eq!(found_bits, existing_bits);

                dec_ref_bits(_py, child_bits);
                dec_ref_bits(_py, attr_bits);
                dec_ref_bits(_py, existing_bits);
                dec_ref_bits(_py, module_bits);
            }
        });
    }

    #[test]
    fn sys_module_cache_set_populates_bootstrap_metadata() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let name_ptr = alloc_string(_py, b"sys");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let cache_restore = ModuleCacheRestore::new(_py, name_bits);
                let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
                assert!(!module_ptr.is_null());
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let result_bits = crate::builtins::module_table::publish_interpreter_sys_for_test(
                    _py,
                    module_bits,
                );
                assert!(
                    !exception_pending(_py),
                    "sys module registration must not leave a pending exception"
                );

                let dict_bits = module_dict_bits(module_ptr);
                let dict_ptr = obj_from_bits(dict_bits)
                    .as_ptr()
                    .expect("sys module dict pointer");
                assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);

                for key in [
                    "platform",
                    "version",
                    "version_info",
                    "hexversion",
                    "api_version",
                    "implementation",
                    "maxsize",
                    "maxunicode",
                    "byteorder",
                    "prefix",
                    "exec_prefix",
                    "base_prefix",
                    "base_exec_prefix",
                    "platlibdir",
                    "path",
                    "orig_argv",
                    "copyright",
                    "stdlib_module_names",
                    "builtin_module_names",
                    "meta_path",
                    "path_hooks",
                    "path_importer_cache",
                ] {
                    let key_ptr = alloc_string(_py, key.as_bytes());
                    assert!(!key_ptr.is_null());
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    let value_bits = dict_get_in_place(_py, dict_ptr, key_bits)
                        .unwrap_or_else(|| panic!("missing sys.{key}"));
                    assert!(
                        !obj_from_bits(value_bits).is_none(),
                        "sys.{key} must not be None"
                    );
                    dec_ref_bits(_py, key_bits);
                }

                let platform_key_ptr = alloc_string(_py, b"platform");
                assert!(!platform_key_ptr.is_null());
                let platform_key_bits = MoltObject::from_ptr(platform_key_ptr).bits();
                let platform_bits =
                    dict_get_in_place(_py, dict_ptr, platform_key_bits).expect("sys.platform");
                let platform_text = string_obj_to_owned(obj_from_bits(platform_bits));
                assert!(
                    platform_text
                        .as_ref()
                        .is_some_and(|value| !value.is_empty()),
                    "sys.platform must be a non-empty string"
                );
                let abiflags_key = crate::attr_name_bits_from_bytes(_py, b"abiflags").unwrap();
                let abiflags = dict_get_in_place(_py, dict_ptr, abiflags_key);
                assert_eq!(
                    abiflags.is_some(),
                    !platform_text.as_ref().unwrap().starts_with("win"),
                    "native bootstrap owns platform-specific abiflags presence"
                );
                if let Some(bits) = abiflags {
                    assert!(string_obj_to_owned(obj_from_bits(bits)).is_some());
                }
                dec_ref_bits(_py, abiflags_key);
                dec_ref_bits(_py, platform_key_bits);

                dec_ref_bits(_py, result_bits);
                dec_ref_bits(_py, module_bits);
            }
        });
    }
}
