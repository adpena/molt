use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use molt_obj_model::MoltObject;
use std::path::PathBuf;
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::{
    attr_name_bits_from_bytes, clear_attribute_error_if_pending, module_attr_lookup,
};
use crate::builtins::classes::builtin_classes;
use crate::builtins::exceptions::{
    exception_kind_bits, exception_message_is_lazy, exception_msg_bits, molt_exception_last_pending,
};
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    HashContext, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET,
    TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_bytes, alloc_dict_with_pairs, alloc_list,
    alloc_module_obj, alloc_string, alloc_tuple, call_callable0, call_callable1, call_callable2,
    call_function_obj_vec, class_mro_vec, class_name_for_error, clear_exception, dec_ref_bits,
    dict_del_in_place, dict_get_in_place, dict_order, dict_set_in_place, exception_pending,
    format_exception_with_traceback, format_obj_str, frame_stack_active_globals_bits,
    has_capability, inc_ref_bits, init_atomic_bits, int_bits_from_i64, intern_static_name,
    is_missing_bits, is_truthy, missing_bits, module_dict_bits, module_name_bits, molt_call_bind,
    molt_callargs_expand_kwstar, molt_callargs_expand_star, molt_callargs_new,
    molt_callargs_push_pos, molt_exception_kind, molt_exception_last, molt_getattr_builtin,
    molt_int_from_obj, molt_is_callable, molt_iter, molt_iter_next, obj_eq, obj_from_bits,
    object_type_id, raise_exception, runtime_state, seq_vec, seq_vec_ref, set_add_in_place,
    string_bytes, string_len, string_obj_to_owned, to_i64, type_name, type_of_bits,
};
use unicode_ident::{is_xid_continue, is_xid_start};

// `molt_isolate_import` is APP-OWNED (emitted by the compiler into the user
// module / provided by the embedding host). On native targets it resolves at
// static link; on wasm32 the shared-runtime cdylib must declare it as an
// `env` import (the molt_call_indirect* pattern in lib.rs) — a plain extern
// is an undefined symbol at rust-lld time and breaks the wasm runtime build.
#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" {
    fn molt_isolate_import(name_bits: u64) -> u64;
}
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn molt_isolate_import(name_bits: u64) -> u64;
}

mod runpy;

pub(crate) use runpy::runpy_exec_restricted_source;
pub use runpy::{
    molt_importlib_exec_restricted_source, molt_runpy_run_module, molt_runpy_run_path,
};

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

fn pending_import_exception_kind_and_message(_py: &PyToken<'_>) -> Option<(String, String)> {
    if !exception_pending(_py) {
        return None;
    }
    let exc_bits = molt_exception_last_pending();
    let out = (|| {
        let exc_ptr = obj_from_bits(exc_bits).as_ptr()?;
        unsafe {
            if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
                return None;
            }
            let kind_bits = exception_kind_bits(exc_ptr);
            let msg_bits = exception_msg_bits(exc_ptr);
            if exception_message_is_lazy(msg_bits) {
                return None;
            }
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))?;
            let message = string_obj_to_owned(obj_from_bits(msg_bits))?;
            Some((kind, message))
        }
    })();
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    out
}

fn normalize_pending_import_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let mut kind = "RuntimeError".to_string();
    let mut message = "module import failed".to_string();
    if let Some((pending_kind, pending_message)) = pending_import_exception_kind_and_message(_py) {
        kind = pending_kind;
        message = pending_message;
    }
    if message.starts_with("No module named ") {
        kind = "ModuleNotFoundError".to_string();
    }
    clear_exception(_py);
    let _ = raise_exception::<u64>(_py, &kind, &message);
}

fn clear_pending_missing_import_exception_for(_py: &PyToken<'_>, expected_name: &str) -> bool {
    if !exception_pending(_py) {
        return false;
    }
    let mut clear = false;
    if let Some((kind, message)) = pending_import_exception_kind_and_message(_py) {
        clear = (kind == "ImportError" || kind == "ModuleNotFoundError")
            && message == format!("No module named '{expected_name}'");
    }
    if clear {
        clear_exception(_py);
    }
    clear
}

#[inline]
fn module_bits_are_module_like(bits: u64) -> bool {
    if obj_from_bits(bits).is_none() {
        return false;
    }
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    let ty = unsafe { object_type_id(ptr) };
    ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT
}

const MODULES_OBJECT_SLOT_COUNT: usize = 15;

pub(crate) struct ModulesRuntimeState {
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
    sys_argv_name: AtomicU64,
}

impl ModulesRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
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
            sys_argv_name: AtomicU64::new(0),
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
            &self.sys_argv_name,
        ]
    }
}

fn modules_state(_py: &PyToken<'_>) -> &'static ModulesRuntimeState {
    &runtime_state(_py).modules
}

pub(crate) fn modules_clear_runtime_state(_py: &PyToken<'_>, state: &crate::state::RuntimeState) {
    crate::gil_assert();
    let slots = state.modules.object_slots();
    crate::state::cache::clear_atomic_slots(_py, &slots);
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

        let args = runtime_state(_py).argv.lock().unwrap();
        let exec_val = std::env::var("MOLT_SYS_EXECUTABLE")
            .ok()
            .filter(|v| !v.is_empty())
            .map(String::into_bytes)
            .unwrap_or_else(|| args.first().cloned().unwrap_or_default());
        let mut elems = Vec::with_capacity(args.len());
        for arg in args.iter() {
            let ptr = alloc_string(_py, arg);
            if ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, argv_key_bits);
                dec_ref_bits(_py, exec_key_bits);
                return Err(());
            }
            elems.push(MoltObject::from_ptr(ptr).bits());
        }
        drop(args);

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

        sys_set_owned_attr(_py, dict_ptr, "platform", crate::molt_sys_platform())?;
        sys_set_owned_attr(_py, dict_ptr, "version", crate::molt_sys_version())?;
        sys_set_owned_attr(
            _py,
            dict_ptr,
            "version_info",
            crate::molt_sys_version_info(),
        )?;
        sys_set_owned_attr(_py, dict_ptr, "hexversion", crate::molt_sys_hexversion())?;
        sys_set_owned_attr(_py, dict_ptr, "api_version", crate::molt_sys_api_version())?;
        sys_set_owned_attr(_py, dict_ptr, "abiflags", crate::molt_sys_abiflags())?;
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

#[unsafe(no_mangle)]
fn simple_edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m.abs_diff(n) > 2 {
        return 3;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
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
        let ptr = alloc_module_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        crate::intrinsics::install_into_builtins(_py, ptr);
        MoltObject::from_ptr(ptr).bits()
    })
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

fn molt_module_import_inner(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => {
                trace_bad_module_name_arg(_py, "module_import", name_bits);
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        };
        if let Some(missing_name) =
            crate::builtins::platform::known_absent_module_missing_name(_py, &name)
        {
            let msg = format!("No module named '{missing_name}'");
            return raise_exception::<_>(_py, "ModuleNotFoundError", &msg);
        }
        let trace_import_stage = std::env::var("MOLT_TRACE_IMPORT_STAGE").as_deref() == Ok("1");
        let trace_stage = |stage: &str| {
            if !trace_import_stage {
                return;
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last_pending();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<exc>".to_string());
                eprintln!("import stage {stage} name={name} pending={kind}");
                dec_ref_bits(_py, exc_bits);
            } else {
                eprintln!("import stage {stage} name={name} pending=<none>");
            }
        };
        let name_key_bits = {
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        };
        let result_bits = 'result: {
            // Prefer canonical handles already present in sys.modules before
            // isolate import, so alias-backed module names (for example
            // `os.path`) resolve even when the runtime importer would reject
            // them as non-package dotted paths.
            let sys_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("sys").copied()
            };
            if let Some(sys_bits) = sys_bits
                && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
            {
                let from_sys_bits = unsafe { dict_get_in_place(_py, modules_ptr, name_key_bits) };
                if exception_pending(_py) {
                    break 'result MoltObject::none().bits();
                }
                if let Some(bits) = from_sys_bits
                    && let Some(ptr) = obj_from_bits(bits).as_ptr()
                {
                    let ty = unsafe { object_type_id(ptr) };
                    if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                        // Keep runtime module cache aligned with sys.modules alias hits
                        // so frontend MODULE_CACHE_GET-based import lowering observes
                        // the same module identity as importlib/builtins paths.
                        let cache = crate::builtins::exceptions::internals::module_cache(_py);
                        let mut guard = cache.lock().unwrap();
                        if let Some(old) = guard.insert(name.clone(), bits) {
                            dec_ref_bits(_py, old);
                        }
                        inc_ref_bits(_py, bits);
                        inc_ref_bits(_py, bits);
                        break 'result bits;
                    }
                }
            }

            trace_stage("before_isolate_import");
            let module_bits = unsafe { molt_isolate_import(name_key_bits) };
            trace_stage("after_isolate_import");

            if exception_pending(_py) {
                normalize_pending_import_exception(_py);
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                break 'result MoltObject::none().bits();
            }
            let mut canonical_bits: Option<u64> = None;
            let sys_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("sys").copied()
            };
            if let Some(sys_bits) = sys_bits
                && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
            {
                let from_sys_bits = unsafe { dict_get_in_place(_py, modules_ptr, name_key_bits) };
                if exception_pending(_py) {
                    break 'result MoltObject::none().bits();
                }
                if let Some(bits) = from_sys_bits
                    && let Some(ptr) = obj_from_bits(bits).as_ptr()
                {
                    let ty = unsafe { object_type_id(ptr) };
                    if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                        canonical_bits = Some(bits);
                    }
                }
            }
            if canonical_bits.is_none() {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                if let Some(bits) = guard.get(&name)
                    && let Some(ptr) = obj_from_bits(*bits).as_ptr()
                {
                    let ty = unsafe { object_type_id(ptr) };
                    if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                        canonical_bits = Some(*bits);
                    }
                }
            }
            if let Some(bits) = canonical_bits {
                let sys_bits = {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    guard.get("sys").copied()
                };
                if let Some(sys_bits) = sys_bits
                    && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
                {
                    unsafe {
                        dict_set_in_place(_py, modules_ptr, name_key_bits, bits);
                    }
                    trace_stage("after_sys_modules_set_canonical");
                    if exception_pending(_py) {
                        if bits != module_bits && !obj_from_bits(module_bits).is_none() {
                            dec_ref_bits(_py, module_bits);
                        }
                        break 'result MoltObject::none().bits();
                    }
                }
                if bits != module_bits {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    inc_ref_bits(_py, bits);
                }
                break 'result bits;
            }
            let module_obj = obj_from_bits(module_bits);
            if !module_obj.is_none() {
                let is_valid_module = if let Some(ptr) = module_obj.as_ptr() {
                    let ty = unsafe { object_type_id(ptr) };
                    ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT
                } else {
                    false
                };
                if !is_valid_module {
                    // Isolate import should only yield module-like objects. If we
                    // get a scalar/status payload instead, treat this as a missing
                    // module for `import` semantics instead of surfacing an
                    // internal payload type to user code.
                    dec_ref_bits(_py, module_bits);
                    let msg = format!("No module named '{name}'");
                    break 'result raise_exception::<_>(_py, "ModuleNotFoundError", &msg);
                }

                // Keep sys.modules synchronized with successful runtime imports so
                // importlib.reload()/sys.modules round-trips remain consistent.
                let sys_bits = {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    guard.get("sys").copied()
                };
                if let Some(sys_bits) = sys_bits
                    && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
                {
                    unsafe {
                        dict_set_in_place(_py, modules_ptr, name_key_bits, module_bits);
                    }
                    trace_stage("after_sys_modules_set_module_bits");
                    if exception_pending(_py) {
                        dec_ref_bits(_py, module_bits);
                        break 'result MoltObject::none().bits();
                    }
                }
            }
            if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
                let msg = format!("No module named '{name}'");
                break 'result raise_exception::<_>(_py, "ModuleNotFoundError", &msg);
            }
            module_bits
        };
        dec_ref_bits(_py, name_key_bits);
        if module_bits_are_module_like(result_bits)
            && exception_pending(_py)
            && !clear_pending_missing_import_exception_for(_py, &name)
        {
            return MoltObject::none().bits();
        }
        result_bits
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
                if new_type_bits == builtins.builtin_function_or_method {
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        };
        let existing_bits = unsafe { dict_get_in_place(_py, extension_ptr, key_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, code_key_bits);
            return MoltObject::none().bits();
        }
        let existing_key_bits = unsafe { dict_get_in_place(_py, inverted_ptr, code_key_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, code_key_bits);
            return MoltObject::none().bits();
        }
        if let Some(found_bits) = existing_bits {
            if let Some(found_key_bits) = existing_key_bits
                && obj_eq(_py, obj_from_bits(found_bits), obj_from_bits(code_key_bits))
                && obj_eq(_py, obj_from_bits(found_key_bits), obj_from_bits(key_bits))
            {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, code_key_bits);
                return MoltObject::none().bits();
            }
            let key_text = crate::format_obj_str(_py, obj_from_bits(key_bits));
            let code_text = crate::format_obj_str(_py, obj_from_bits(found_bits));
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, code_key_bits);
            let msg = format!("key {key_text} is already registered with code {code_text}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if let Some(found_key_bits) = existing_key_bits {
            let code_text = crate::format_obj_str(_py, obj_from_bits(code_key_bits));
            let key_text = crate::format_obj_str(_py, obj_from_bits(found_key_bits));
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, code_key_bits);
            let msg = format!("code {code_text} is already in use for key {key_text}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        unsafe {
            dict_set_in_place(_py, extension_ptr, key_bits, code_key_bits);
            dict_set_in_place(_py, inverted_ptr, code_key_bits, key_bits);
        }
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, code_key_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
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
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        };
        let existing_bits = unsafe { dict_get_in_place(_py, extension_ptr, key_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        let existing_key_bits = unsafe { dict_get_in_place(_py, inverted_ptr, code_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        let registered = match (existing_bits, existing_key_bits) {
            (Some(found_code_bits), Some(found_key_bits)) => {
                obj_eq(
                    _py,
                    obj_from_bits(found_code_bits),
                    obj_from_bits(code_bits),
                ) && obj_eq(_py, obj_from_bits(found_key_bits), obj_from_bits(key_bits))
            }
            _ => false,
        };
        if !registered {
            let key_text = crate::format_obj_str(_py, obj_from_bits(key_bits));
            let code_text = crate::format_obj_str(_py, obj_from_bits(code_bits));
            dec_ref_bits(_py, key_bits);
            let msg = format!("key {key_text} is not registered with code {code_text}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        unsafe {
            dict_del_in_place(_py, extension_ptr, key_bits);
            dict_del_in_place(_py, inverted_ptr, code_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        let cached_bits = unsafe { dict_get_in_place(_py, cache_ptr, code_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        if cached_bits.is_some() {
            unsafe {
                dict_del_in_place(_py, cache_ptr, code_bits);
            }
        }
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
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

pub(crate) fn sys_modules_dict_bits(_py: &PyToken<'_>, sys_bits: u64) -> Option<u64> {
    let sys_obj = obj_from_bits(sys_bits);
    let sys_ptr = sys_obj.as_ptr()?;
    unsafe {
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return None;
        }
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return None,
        };
        let modules_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.modules_name, b"modules");
        if obj_from_bits(modules_name_bits).is_none() {
            return None;
        }
        let mut modules_bits = dict_get_in_place(_py, dict_ptr, modules_name_bits);
        if modules_bits.is_none() {
            let new_ptr = alloc_dict_with_pairs(_py, &[]);
            if new_ptr.is_null() {
                return None;
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            dict_set_in_place(_py, dict_ptr, modules_name_bits, new_bits);
            modules_bits = Some(new_bits);
            dec_ref_bits(_py, new_bits);
        }
        let modules_bits = modules_bits?;
        match obj_from_bits(modules_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "sys.modules must be dict"),
        };
        inc_ref_bits(_py, modules_bits);
        Some(modules_bits)
    }
}

pub(crate) fn sys_modules_dict_ptr(_py: &PyToken<'_>, sys_bits: u64) -> Option<*mut u8> {
    let modules_bits = sys_modules_dict_bits(_py, sys_bits)?;
    unsafe {
        let modules_ptr = match obj_from_bits(modules_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                dec_ref_bits(_py, modules_bits);
                return None;
            }
        };
        dec_ref_bits(_py, modules_bits);
        Some(modules_ptr)
    }
}

fn sys_modules_set_canonical_name(
    _py: &PyToken<'_>,
    modules_ptr: *mut u8,
    name: &str,
    module_bits: u64,
) -> Result<(), u64> {
    let key_ptr = alloc_string(_py, name.as_bytes());
    if key_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(_py, modules_ptr, key_bits, module_bits);
    }
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let is_sys = name == "sys";
        let trace_cache = trace_module_cache();
        if trace_cache {
            eprintln!("module cache set: {name} bits=0x{module_bits:x}");
        }
        let (sys_bits, cached_modules) = {
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
            if let Some(&existing) = guard.get(&name)
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
                let sys_bits_out = guard.get("sys").copied();
                return if let Some(sys_bits) = sys_bits_out
                    && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
                {
                    if let Err(err) =
                        sys_modules_set_canonical_name(_py, modules_ptr, &name, existing)
                    {
                        return err;
                    }
                    existing
                } else {
                    existing
                };
            }
            if let Some(old) = guard.insert(name.clone(), module_bits) {
                dec_ref_bits(_py, old);
            }
            inc_ref_bits(_py, module_bits);
            if is_sys {
                let entries = guard
                    .iter()
                    .map(|(key, &bits)| (key.clone(), bits))
                    .collect::<Vec<_>>();
                (Some(module_bits), Some(entries))
            } else {
                (guard.get("sys").copied(), None)
            }
        };
        if let Some(sys_bits) = sys_bits
            && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
        {
            if let Some(entries) = cached_modules {
                for (key, bits) in entries {
                    let key_ptr = alloc_string(_py, key.as_bytes());
                    if key_ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    unsafe {
                        dict_set_in_place(_py, modules_ptr, key_bits, bits);
                    }
                    dec_ref_bits(_py, key_bits);
                }
            } else {
                if let Err(err) =
                    sys_modules_set_canonical_name(_py, modules_ptr, &name, module_bits)
                {
                    return err;
                }
            }
        }
        if is_sys {
            let sys_obj = obj_from_bits(module_bits);
            if let Some(sys_ptr) = sys_obj.as_ptr() {
                unsafe {
                    if sys_populate_argv_executable(_py, sys_ptr).is_err() {
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
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if sys_populate_bootstrap_metadata(_py, sys_ptr).is_err() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_del(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let trace_import_failure = matches!(
            std::env::var("MOLT_TRACE_IMPORT_FAILURE").ok().as_deref(),
            Some("1")
        );
        if trace_import_failure {
            if exception_pending(_py) {
                let exc_bits = molt_exception_last_pending();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<exc>".to_string());
                let detail = obj_from_bits(exc_bits)
                    .as_ptr()
                    .map(|ptr| format_exception_with_traceback(_py, ptr))
                    .unwrap_or_else(|| "<no traceback>".to_string());
                eprintln!("module init failed: {kind} while importing {name}: {detail}");
                dec_ref_bits(_py, exc_bits);
            } else {
                eprintln!("module cache cleared without pending exception: {name}");
            }
        }
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let mut guard = cache.lock().unwrap();
            if let Some(bits) = guard.remove(&name) {
                dec_ref_bits(_py, bits);
            }
            if trace_module_cache() {
                eprintln!("module cache del: {name}");
            }
            guard.get("sys").copied()
        };
        if let Some(sys_bits) = sys_bits {
            let sys_obj = obj_from_bits(sys_bits);
            let Some(sys_ptr) = sys_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            let saved_exc_bits = if exception_pending(_py) {
                let bits = molt_exception_last_pending();
                clear_exception(_py);
                Some(bits)
            } else {
                None
            };
            unsafe {
                if object_type_id(sys_ptr) != TYPE_ID_MODULE {
                    if let Some(saved_bits) = saved_exc_bits {
                        let _ = crate::molt_exception_set_last(saved_bits);
                        dec_ref_bits(_py, saved_bits);
                    }
                    return MoltObject::none().bits();
                }
                let dict_bits = module_dict_bits(sys_ptr);
                let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
                    Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                    _ => {
                        if let Some(saved_bits) = saved_exc_bits {
                            let _ = crate::molt_exception_set_last(saved_bits);
                            dec_ref_bits(_py, saved_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                let modules_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.modules_name, b"modules");
                if obj_from_bits(modules_name_bits).is_none() {
                    if let Some(saved_bits) = saved_exc_bits {
                        let _ = crate::molt_exception_set_last(saved_bits);
                        dec_ref_bits(_py, saved_bits);
                    }
                    return MoltObject::none().bits();
                }
                let Some(modules_bits) = dict_get_in_place(_py, dict_ptr, modules_name_bits) else {
                    if let Some(saved_bits) = saved_exc_bits {
                        if exception_pending(_py) {
                            clear_exception(_py);
                        }
                        let _ = crate::molt_exception_set_last(saved_bits);
                        dec_ref_bits(_py, saved_bits);
                    }
                    return MoltObject::none().bits();
                };
                let modules_ptr = match obj_from_bits(modules_bits).as_ptr() {
                    Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                    _ => {
                        if let Some(saved_bits) = saved_exc_bits {
                            if exception_pending(_py) {
                                clear_exception(_py);
                            }
                            let _ = crate::molt_exception_set_last(saved_bits);
                            dec_ref_bits(_py, saved_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                dict_del_in_place(_py, modules_ptr, name_bits);
            }
            if let Some(saved_bits) = saved_exc_bits {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                let _ = crate::molt_exception_set_last(saved_bits);
                dec_ref_bits(_py, saved_bits);
            }
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
        let ptr = func_ptr_bits as usize as *const u8;
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let len = func_len_bits as usize;
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
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
            if let Some(val) = module_attr_lookup(_py, module_ptr, attr_bits) {
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
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get("sys").copied()
        }?;
        let modules_ptr = sys_modules_dict_ptr(_py, sys_bits)?;
        let key_ptr = alloc_string(_py, name.as_bytes());
        if key_ptr.is_null() {
            raise_exception::<u64>(_py, "MemoryError", "out of memory");
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let found = dict_get_in_place(_py, modules_ptr, key_bits);
        dec_ref_bits(_py, key_bits);
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
        if let Some(existing_bits) = module_attr_lookup(_py, module_ptr, attr_bits) {
            if !obj_from_bits(existing_bits).is_none() {
                dec_ref_bits(_py, existing_bits);
            }
            return Ok(());
        }
        clear_attribute_error_if_pending(_py);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }

    let Some(child_name) = string_obj_to_owned(obj_from_bits(child_name_bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "from-import child name must be str",
        ));
    };
    let imported_bits = molt_module_import(child_name_bits);
    if exception_pending(_py) {
        if clear_pending_missing_import_exception_for(_py, &child_name) {
            return Ok(());
        }
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(imported_bits).is_none() {
        return Ok(());
    }
    let out_bits = molt_module_set_attr(module_bits, attr_bits, imported_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
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
            if let Some(val) = module_attr_lookup(_py, module_ptr, attr_bits) {
                return val;
            }
            // module_attr_lookup returned None: a clean miss, or the lookup
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace = trace_name_error();
        let trace_globals_mode = trace_module_globals_mode();
        let trace_globals = trace_globals_mode != TraceModuleGlobalsMode::Off;
        let trace_globals_all = trace_globals_mode == TraceModuleGlobalsMode::Verbose;
        let trace_name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        let trace_name_match = trace_globals_all
            || trace_name == "_SYS_FLAGS_SEQUENCE_FIELDS"
            || trace_name == "_FlagsTuple";
        let active_globals_bits = frame_stack_active_globals_bits();
        if active_globals_bits != 0
            && !obj_from_bits(active_globals_bits).is_none()
            && let Some(active_globals_ptr) = obj_from_bits(active_globals_bits).as_ptr()
            && unsafe { object_type_id(active_globals_ptr) } == TYPE_ID_DICT
        {
            unsafe {
                if let Some(val) = dict_get_in_place(_py, active_globals_ptr, name_bits) {
                    inc_ref_bits(_py, val);
                    return val;
                }
            }
            let builtins_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("builtins").copied()
            };
            if let Some(builtins_bits) = builtins_bits {
                let builtins_ptr = match obj_from_bits(builtins_bits).as_ptr() {
                    Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_MODULE => ptr,
                    _ => std::ptr::null_mut(),
                };
                if !builtins_ptr.is_null() {
                    let builtins_dict_bits = unsafe { module_dict_bits(builtins_ptr) };
                    let builtins_dict_ptr = match obj_from_bits(builtins_dict_bits).as_ptr() {
                        Some(ptr) if unsafe { object_type_id(ptr) } == TYPE_ID_DICT => ptr,
                        _ => std::ptr::null_mut(),
                    };
                    if !builtins_dict_ptr.is_null()
                        && let Some(val) =
                            unsafe { dict_get_in_place(_py, builtins_dict_ptr, name_bits) }
                    {
                        inc_ref_bits(_py, val);
                        return val;
                    }
                }
            }
            if trace_name == "exec" || trace_name == "eval" {
                let msg = format!(
                    "MOLT_COMPAT_ERROR: {trace_name}() is unsupported in compiled Molt binaries; \
dynamic code execution is outside the verified subset. \
Use static modules or pre-generated code paths instead."
                );
                return raise_exception::<_>(_py, "RuntimeError", &msg);
            }
            let msg = format!("name '{trace_name}' is not defined");
            return raise_exception::<_>(_py, "NameError", &msg);
        }
        if trace_globals && trace_name_match {
            eprintln!(
                "molt module_get_global enter name={} module_bits=0x{:x} pending={}",
                trace_name,
                module_bits,
                exception_pending(_py),
            );
        }
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            // On exception handler paths, SSA variables may resolve to
            // None (default for undefined Cranelift Variables).  Return
            // None silently — the exception handler will check
            // exception_last and re-raise if needed.
            if module_obj.is_none() || exception_pending(_py) {
                if trace_globals && trace_name_match {
                    eprintln!(
                        "molt module_get_global early_none name={} module_bits=0x{:x} module_is_none={} pending={}",
                        trace_name,
                        module_bits,
                        module_obj.is_none(),
                        exception_pending(_py),
                    );
                }
                return MoltObject::none().bits();
            }
            let msg = format!(
                "module get_global expects module, got non-pointer (bits=0x{:x}) for name '{}'",
                module_bits, trace_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if exception_pending(_py) {
                    if trace_globals && trace_name_match {
                        eprintln!(
                            "molt module_get_global early_type name={} module_bits=0x{:x} type_id={} pending={}",
                            trace_name,
                            module_bits,
                            object_type_id(module_ptr),
                            exception_pending(_py),
                        );
                    }
                    return MoltObject::none().bits();
                }
                let type_id = object_type_id(module_ptr);
                let msg = format!(
                    "module get_global expects module, got type_id={} (bits=0x{:x}) for name '{}'",
                    type_id, module_bits, trace_name
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if let Some(val) = dict_get_in_place(_py, dict_ptr, name_bits) {
                if trace_globals && trace_name_match {
                    let module_name =
                        string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                            .unwrap_or_else(|| "<module>".to_string());
                    eprintln!(
                        "molt module_get_global hit module={} name={} module_bits=0x{:x} dict_bits=0x{:x} val_type={}",
                        module_name,
                        trace_name,
                        module_bits,
                        dict_bits,
                        type_name(_py, obj_from_bits(val)),
                    );
                }
                inc_ref_bits(_py, val);
                return val;
            }
            if trace_globals && trace_name_match {
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                    .unwrap_or_else(|| "<module>".to_string());
                eprintln!(
                    "molt module_get_global miss module={} name={} module_bits=0x{:x} dict_bits=0x{:x}",
                    module_name, trace_name, module_bits, dict_bits
                );
            }
            // Mirror CPython LOAD_GLOBAL: fall back to the builtins module dict.
            let builtins_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("builtins").copied()
            };
            if let Some(builtins_bits) = builtins_bits {
                let builtins_ptr = match obj_from_bits(builtins_bits).as_ptr() {
                    Some(ptr) if object_type_id(ptr) == TYPE_ID_MODULE => ptr,
                    _ => std::ptr::null_mut(),
                };
                if !builtins_ptr.is_null() {
                    let builtins_dict_bits = module_dict_bits(builtins_ptr);
                    let builtins_dict_ptr = match obj_from_bits(builtins_dict_bits).as_ptr() {
                        Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                        _ => std::ptr::null_mut(),
                    };
                    if !builtins_dict_ptr.is_null()
                        && let Some(val) = dict_get_in_place(_py, builtins_dict_ptr, name_bits)
                    {
                        inc_ref_bits(_py, val);
                        return val;
                    }
                }
            }
            if trace_name == "exec" || trace_name == "eval" {
                let msg = format!(
                    "MOLT_COMPAT_ERROR: {trace_name}() is unsupported in compiled Molt binaries; \
dynamic code execution is outside the verified subset. \
Use static modules or pre-generated code paths instead."
                );
                return raise_exception::<_>(_py, "RuntimeError", &msg);
            }
            if trace {
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                    .unwrap_or_else(|| "<module>".to_string());
                let pending = exception_pending(_py);
                eprintln!(
                    "molt name error module={} name={} pending={}",
                    module_name, trace_name, pending
                );
            }
            // CPython 3.12+: suggest similar names in NameError.
            let dict_bits = module_dict_bits(module_ptr);
            let suggestion: Option<String> = if let Some(dict_ptr) =
                obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let order = crate::builtins::containers::dict_order(dict_ptr);
                let mut best: Option<(String, usize)> = None;
                let mut i = 0;
                while i + 1 < order.len() {
                    if let Some(key_ptr) = obj_from_bits(order[i]).as_ptr()
                        && object_type_id(key_ptr) == TYPE_ID_STRING
                    {
                        let len = string_len(key_ptr);
                        let bytes = std::slice::from_raw_parts(string_bytes(key_ptr), len);
                        if let Ok(cand) = std::str::from_utf8(bytes) {
                            let d = simple_edit_distance(&trace_name, cand);
                            let t = if trace_name.len() <= 2 { 1 } else { 2 };
                            if d > 0 && d <= t && (best.is_none() || d < best.as_ref().unwrap().1) {
                                best = Some((cand.to_string(), d));
                            }
                        }
                    }
                    i += 2;
                }
                best.map(|(n, _)| n)
            } else {
                None
            };
            // Also search builtins dict for suggestions (CPython does this).
            let suggestion = suggestion.or_else(|| {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().ok()?;
                let builtins_bits = guard.get("builtins").copied()?;
                drop(guard);
                let builtins_ptr = obj_from_bits(builtins_bits).as_ptr()?;
                let bdict_bits = module_dict_bits(builtins_ptr);
                let bdict_ptr = obj_from_bits(bdict_bits).as_ptr()?;
                if object_type_id(bdict_ptr) != TYPE_ID_DICT {
                    return None;
                }
                let order = crate::builtins::containers::dict_order(bdict_ptr);
                let mut best: Option<(String, usize)> = None;
                let mut i = 0;
                while i + 1 < order.len() {
                    if let Some(key_ptr) = obj_from_bits(order[i]).as_ptr()
                        && object_type_id(key_ptr) == TYPE_ID_STRING
                    {
                        let len = string_len(key_ptr);
                        let bytes = std::slice::from_raw_parts(string_bytes(key_ptr), len);
                        if let Ok(cand) = std::str::from_utf8(bytes) {
                            let d = simple_edit_distance(&trace_name, cand);
                            let t = if trace_name.len() <= 2 { 1 } else { 2 };
                            if d > 0 && d <= t && (best.is_none() || d < best.as_ref().unwrap().1) {
                                best = Some((cand.to_string(), d));
                            }
                        }
                    }
                    i += 2;
                }
                best.map(|(n, _)| n)
            });
            let msg = if let Some(similar) = suggestion {
                format!("name '{trace_name}' is not defined. Did you mean: '{similar}'?")
            } else {
                format!("name '{trace_name}' is not defined")
            };
            raise_exception::<_>(_py, "NameError", &msg)
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
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        return raise_exception::<_>(_py, "TypeError", "module attribute access expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
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
            let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                .unwrap_or_else(|| "<module>".to_string());
            let pending = exception_pending(_py);
            eprintln!(
                "molt name error(del) module={} name={} pending={}",
                module_name, name, pending
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
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotations_bits),
            ) {
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
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(annotate_bits))
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
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_import_star(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let src_obj = obj_from_bits(src_bits);
        let Some(src_ptr) = src_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module import expects module");
        };
        let dst_obj = obj_from_bits(dst_bits);
        let Some(dst_ptr) = dst_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module import expects module");
        };
        unsafe {
            if object_type_id(src_ptr) != TYPE_ID_MODULE
                || object_type_id(dst_ptr) != TYPE_ID_MODULE
            {
                return raise_exception::<_>(_py, "TypeError", "module import expects module");
            }
            let src_dict_bits = module_dict_bits(src_ptr);
            let dst_dict_bits = module_dict_bits(dst_ptr);
            let src_dict_obj = obj_from_bits(src_dict_bits);
            let dst_dict_obj = obj_from_bits(dst_dict_bits);
            let src_dict_ptr = match src_dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let dst_dict_ptr = match dst_dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let module_name =
                string_obj_to_owned(obj_from_bits(module_name_bits(src_ptr))).unwrap_or_default();
            let all_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.all_name, b"__all__");
            if let Some(all_bits) = dict_get_in_place(_py, src_dict_ptr, all_name_bits) {
                let iter_bits = molt_iter(all_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        return MoltObject::none().bits();
                    }
                    let done_bits = elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let name_bits = elems[0];
                    let name_obj = obj_from_bits(name_bits);
                    if let Some(name_ptr) = name_obj.as_ptr() {
                        if object_type_id(name_ptr) != TYPE_ID_STRING {
                            let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                            let msg = format!(
                                "Item in {module_name}.__all__ must be str, not {type_name}"
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    } else {
                        let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                        let msg =
                            format!("Item in {module_name}.__all__ must be str, not {type_name}");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let Some(val_bits) = dict_get_in_place(_py, src_dict_ptr, name_bits) else {
                        let name =
                            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                        let msg = format!("module '{module_name}' has no attribute '{name}'");
                        return raise_exception::<_>(_py, "AttributeError", &msg);
                    };
                    dict_set_in_place(_py, dst_dict_ptr, name_bits, val_bits);
                }
                return MoltObject::none().bits();
            }

            let order = dict_order(src_dict_ptr);
            for idx in (0..order.len()).step_by(2) {
                let name_bits = order[idx];
                let name_obj = obj_from_bits(name_bits);
                let Some(name_ptr) = name_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    continue;
                }
                let name_len = string_len(name_ptr);
                if name_len > 0 {
                    let name_bytes = std::slice::from_raw_parts(string_bytes(name_ptr), name_len);
                    if name_bytes[0] == b'_' {
                        continue;
                    }
                }
                let val_bits = order[idx + 1];
                dict_set_in_place(_py, dst_dict_ptr, name_bits, val_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

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

    #[test]
    fn modules_runtime_state_is_owned_and_clearable() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    fn sys_module_cache_set_does_not_leave_pending_exception() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(_py, name_bits);
            let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits = molt_module_cache_set(cache_restore.name_bits(), module_bits);
            // Contract: a successful registration leaves no pending exception.
            // The return value is either:
            //   - None (fresh insert succeeded), or
            //   - the previously-cached `existing` module bits (first-init-wins
            //     skip path; only when a different module was already cached
            //     under the same name).
            // The "no pending exception" invariant is the public success
            // criterion; the return value is an internal handoff that callers
            // dec_ref unconditionally.
            assert!(
                !exception_pending(_py),
                "sys module registration must not leave a pending exception"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, module_bits);
        });
    }

    #[test]
    fn from_import_child_missing_clear_preserves_unrelated_pending_failure_in_handler() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            crate::builtins::exceptions::exception_stack_push();
            let raised_bits = raise_exception::<u64>(
                _py,
                "ModuleNotFoundError",
                "No module named 'definitely_missing_dependency'",
            );
            assert!(obj_from_bits(raised_bits).is_none());
            assert!(exception_pending(_py));

            assert!(
                !clear_pending_missing_import_exception_for(_py, "pkg.child"),
                "a failed child body import must not be treated as absent child module"
            );
            assert!(
                exception_pending(_py),
                "unrelated import failure must remain pending for the caller's handler"
            );

            let (_kind, message) =
                pending_import_exception_kind_and_message(_py).expect("pending import exception");
            assert_eq!(message, "No module named 'definitely_missing_dependency'");
            clear_exception(_py);
            crate::builtins::exceptions::exception_stack_pop(_py);
        });
    }

    #[test]
    fn prepare_from_import_child_preserves_existing_package_attribute() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let name_ptr = alloc_string(_py, b"sys");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let cache_restore = ModuleCacheRestore::new(_py, name_bits);
                let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
                assert!(!module_ptr.is_null());
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let result_bits = molt_module_cache_set(cache_restore.name_bits(), module_bits);
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
                    "abiflags",
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
                dec_ref_bits(_py, platform_key_bits);

                dec_ref_bits(_py, result_bits);
                dec_ref_bits(_py, module_bits);
            }
        });
    }
}
