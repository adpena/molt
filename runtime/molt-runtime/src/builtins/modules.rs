use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
use molt_obj_model::MoltObject;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::{
    attr_name_bits_from_bytes, clear_attribute_error_if_pending, module_attr_lookup,
};
use crate::builtins::classes::builtin_classes;
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE,
    alloc_bytes, alloc_dict_with_pairs, alloc_list, alloc_module_obj, alloc_string, alloc_tuple,
    call_callable0, call_callable1, call_callable2, call_function_obj_vec, class_mro_vec,
    class_name_for_error, clear_exception, dec_ref_bits, dict_del_in_place, dict_get_in_place,
    dict_order, dict_set_in_place, exception_pending, format_exception_with_traceback,
    format_obj_str, has_capability, inc_ref_bits, init_atomic_bits, int_bits_from_i64,
    intern_static_name, is_missing_bits, is_truthy, missing_bits, module_dict_bits,
    module_name_bits, molt_call_bind, molt_callargs_expand_kwstar, molt_callargs_expand_star,
    molt_callargs_new, molt_callargs_push_pos, molt_exception_kind, molt_exception_last,
    molt_getattr_builtin, molt_int_from_obj, molt_is_callable, molt_iter, molt_iter_next, obj_eq,
    obj_from_bits, object_type_id, raise_exception, runtime_state, seq_vec, seq_vec_ref,
    set_add_in_place, string_bytes, string_len, string_obj_to_owned, to_i64, type_name,
    type_of_bits,
};
use unicode_ident::{is_xid_continue, is_xid_start};

unsafe extern "C" {
    fn molt_isolate_import(name_bits: u64) -> u64;
}

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
    if let Some((file, line, func, _, _)) = crate::builtins::exceptions::frame_stack_top_info(_py)
    {
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

fn normalize_pending_import_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last();
    let mut kind = "RuntimeError".to_string();
    let mut message = "module import failed".to_string();
    if !obj_from_bits(exc_bits).is_none() {
        let kind_bits = molt_exception_kind(exc_bits);
        if let Some(name) = string_obj_to_owned(obj_from_bits(kind_bits)) {
            kind = name;
        }
        message = format_obj_str(_py, obj_from_bits(exc_bits));
        if message.starts_with("No module named ") {
            kind = "ModuleNotFoundError".to_string();
        }
        dec_ref_bits(_py, exc_bits);
    }
    clear_exception(_py);
    let _ = raise_exception::<u64>(_py, &kind, &message);
}

fn clear_pending_missing_import_exception(_py: &PyToken<'_>) -> bool {
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last();
    let mut clear = false;
    if !obj_from_bits(exc_bits).is_none() {
        let kind_bits = molt_exception_kind(exc_bits);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_default();
        let message = format_obj_str(_py, obj_from_bits(exc_bits));
        clear = (kind == "ImportError" || kind == "ModuleNotFoundError")
            && message.starts_with("No module named ");
        dec_ref_bits(_py, exc_bits);
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

static TRACE_LAST_OP: AtomicU64 = AtomicU64::new(0);
static TRACE_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);
static COPYREG_DISPATCH_TABLE_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_EXTENSION_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_INVERTED_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_EXTENSION_CACHE_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_CONSTRUCTOR_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);
static RUNPY_IMPORT_DUNDER_NAME: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn ensure_sigtrap_handler() {
    if trace_op_sigtrap_enabled() && !TRACE_SIGTRAP_INSTALLED.swap(true, Ordering::Relaxed) {
        unsafe {
            libc::signal(libc::SIGTRAP, trace_sigtrap_handler as *const () as usize);
        }
    }
}

#[cfg(target_arch = "wasm32")]
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
    crate::with_gil_entry!(_py, {
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
        unsafe {
            let dict_bits = module_dict_bits(ptr);
            let dict_obj = obj_from_bits(dict_bits);
            if let Some(dict_ptr) = dict_obj.as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let key_ptr = alloc_string(_py, b"__name__");
                if !key_ptr.is_null() {
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    dict_set_in_place(_py, dict_ptr, key_bits, name_bits);
                    dec_ref_bits(_py, key_bits);
                }
            }
        }
        crate::intrinsics::install_into_builtins(_py, ptr);
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    // Suspend nursery during import — objects created during module init
    // (type objects, class dicts, etc.) may be stored in persistent containers
    // (sys.modules, module.__dict__) that outlive the nursery reset.
    crate::object::nursery_suspend();
    let result = molt_module_import_inner(name_bits);
    crate::object::nursery_resume();
    result
}

fn molt_module_import_inner(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => {
                trace_bad_module_name_arg(_py, "module_import", name_bits);
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        };
        let trace_import_stage = std::env::var("MOLT_TRACE_IMPORT_STAGE").as_deref() == Ok("1");
        let trace_stage = |stage: &str| {
            if !trace_import_stage {
                return;
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<exc>".to_string());
                eprintln!("import stage {stage} name={name} pending={kind}");
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
                    break 'result raise_exception::<_>(_py, "ImportError", &msg);
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
                break 'result raise_exception::<_>(_py, "ImportError", &msg);
            }
            module_bits
        };
        dec_ref_bits(_py, name_key_bits);
        if module_bits_are_module_like(result_bits) && exception_pending(_py) {
            if !clear_pending_missing_import_exception(_py) {
                return MoltObject::none().bits();
            }
        }
        result_bits
    })
}

unsafe fn dict_copy_entries(_py: &PyToken<'_>, src_ptr: *mut u8, dst_ptr: *mut u8) {
    unsafe {
        let source_order = dict_order(src_ptr);
        for idx in (0..source_order.len()).step_by(2) {
            let key_bits = source_order[idx];
            let val_bits = source_order[idx + 1];
            dict_set_in_place(_py, dst_ptr, key_bits, val_bits);
        }
    }
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

unsafe fn runpy_import_module_bits(_py: &PyToken<'_>, name: &str) -> Result<u64, u64> {
    unsafe {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_bits = molt_isolate_import(name_bits);
            let name_text =
                string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| name.to_string());
            let mut canonical_bits: Option<u64> = None;
            if !exception_pending(_py) {
                // Prefer canonical module handles from sys.modules when isolate-import
                // returns non-module payloads (status sentinels, accidental scalar values, etc).
                let sys_bits = {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    guard.get("sys").copied()
                };
                if let Some(sys_bits) = sys_bits
                    && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
                {
                    let module_key_ptr = alloc_string(_py, name_text.as_bytes());
                    if module_key_ptr.is_null() {
                        dec_ref_bits(_py, name_bits);
                        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                    }
                    let module_key_bits = MoltObject::from_ptr(module_key_ptr).bits();
                    let from_sys_bits = dict_get_in_place(_py, modules_ptr, module_key_bits);
                    dec_ref_bits(_py, module_key_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, name_bits);
                        return Err(MoltObject::none().bits());
                    }
                    if let Some(bits) = from_sys_bits
                        && let Some(ptr) = obj_from_bits(bits).as_ptr()
                    {
                        let ty = object_type_id(ptr);
                        if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                            canonical_bits = Some(bits);
                        }
                    }
                }
                if canonical_bits.is_none() {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    if let Some(bits) = guard.get(name)
                        && let Some(ptr) = obj_from_bits(*bits).as_ptr()
                    {
                        let ty = object_type_id(ptr);
                        if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                            canonical_bits = Some(*bits);
                        }
                    }
                }
            }
            dec_ref_bits(_py, name_bits);

            if let Some(bits) = canonical_bits {
                if bits != module_bits {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    inc_ref_bits(_py, bits);
                }
                return Ok(bits);
            }

            if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
                if let Some(bits) = runpy_import_intrinsic_module(_py, &name_text)? {
                    return Ok(bits);
                }
                match runpy_import_via_builtins(_py, &name_text) {
                    Ok(Some(bits)) => return Ok(bits),
                    Ok(None) => {}
                    Err(err) => return Err(err),
                }
            }

            if let Some(ptr) = obj_from_bits(module_bits).as_ptr() {
                let ty = object_type_id(ptr);
                if ty != TYPE_ID_MODULE && ty != TYPE_ID_DICT {
                    let type_name = type_name(_py, obj_from_bits(module_bits));
                    dec_ref_bits(_py, module_bits);
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("runpy import returned non-module payload: {type_name}"),
                    ));
                }
            }
            Ok(module_bits)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            if let Some(bits) = guard.get(name) {
                inc_ref_bits(_py, *bits);
                Ok(*bits)
            } else {
                Ok(MoltObject::none().bits())
            }
        }
    }
}

unsafe fn runpy_import_intrinsic_module(_py: &PyToken<'_>, name: &str) -> Result<Option<u64>, u64> {
    unsafe {
        if name != "errno" {
            return Ok(None);
        }
        let name_ptr = alloc_string(_py, b"errno");
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_bits = molt_module_new(name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        };
        let module_dict_bits = module_dict_bits(module_ptr);
        let Some(module_dict_ptr) = obj_from_bits(module_dict_bits).as_ptr() else {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno module dict unavailable",
            ));
        };
        if object_type_id(module_dict_ptr) != TYPE_ID_DICT {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno module dict unavailable",
            ));
        }
        let payload_bits = crate::molt_errno_constants();
        if exception_pending(_py) {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        let Some(payload_ptr) = obj_from_bits(payload_bits).as_ptr() else {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        if object_type_id(payload_ptr) != TYPE_ID_TUPLE {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        let payload_items = seq_vec_ref(payload_ptr);
        if payload_items.len() != 2 {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        let constants_bits = payload_items[0];
        let errorcode_bits = payload_items[1];
        let Some(constants_ptr) = obj_from_bits(constants_bits).as_ptr() else {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        let Some(errorcode_ptr) = obj_from_bits(errorcode_bits).as_ptr() else {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        if object_type_id(constants_ptr) != TYPE_ID_DICT
            || object_type_id(errorcode_ptr) != TYPE_ID_DICT
        {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        dict_copy_entries(_py, constants_ptr, module_dict_ptr);
        if exception_pending(_py) {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        dict_set_str_key_bits(_py, module_dict_ptr, "errorcode", errorcode_bits)?;
        let cache_result = molt_module_cache_set(name_bits, module_bits);
        if obj_from_bits(cache_result).is_none() && exception_pending(_py) {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        dec_ref_bits(_py, payload_bits);
        dec_ref_bits(_py, name_bits);
        Ok(Some(module_bits))
    }
}

unsafe fn runpy_import_via_builtins(_py: &PyToken<'_>, name: &str) -> Result<Option<u64>, u64> {
    unsafe {
        let builtins_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get("builtins").copied()
        };
        let Some(builtins_bits) = builtins_bits else {
            return Ok(None);
        };
        let missing = missing_bits(_py);
        let import_name_bits = intern_static_name(_py, &RUNPY_IMPORT_DUNDER_NAME, b"__import__");
        let import_bits = molt_getattr_builtin(builtins_bits, import_name_bits, missing);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, import_bits) || obj_from_bits(import_bits).is_none() {
            return Ok(None);
        }
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(import_bits)));
        if !callable {
            dec_ref_bits(_py, import_bits);
            return Ok(None);
        }
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let none_bits = MoltObject::none().bits();
        // Non-empty fromlist keeps __import__ aligned with importlib for dotted
        // names (for example pkg.__main__), instead of returning only the
        // top-level package object.
        let fromlist_marker_ptr = alloc_string(_py, b"_molt_runpy");
        if fromlist_marker_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let fromlist_marker_bits = MoltObject::from_ptr(fromlist_marker_ptr).bits();
        let fromlist_ptr = alloc_tuple(_py, &[fromlist_marker_bits]);
        if fromlist_ptr.is_null() {
            dec_ref_bits(_py, fromlist_marker_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        dec_ref_bits(_py, fromlist_marker_bits);
        let fromlist_bits = MoltObject::from_ptr(fromlist_ptr).bits();
        let zero_bits = int_bits_from_i64(_py, 0);
        let imported_bits = call_function_obj_vec(
            _py,
            import_bits,
            &[name_bits, none_bits, none_bits, fromlist_bits, zero_bits],
        );
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, fromlist_bits);
        dec_ref_bits(_py, import_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(imported_bits).is_none() {
            return Ok(None);
        }
        if let Some(ptr) = obj_from_bits(imported_bits).as_ptr() {
            let ty = object_type_id(ptr);
            if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                return Ok(Some(imported_bits));
            }
        }
        dec_ref_bits(_py, imported_bits);
        Ok(None)
    }
}

unsafe fn runpy_module_dict_ptr(_py: &PyToken<'_>, module_bits: u64) -> Result<*mut u8, u64> {
    unsafe {
        let module_ptr = match obj_from_bits(module_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_MODULE => ptr,
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => return Ok(ptr),
            _ => {
                let got = type_name(_py, obj_from_bits(module_bits));
                let msg = format!("module import expects module or dict, got {got}");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        match obj_from_bits(module_dict_bits(module_ptr)).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => Ok(ptr),
            _ => Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module dict missing",
            )),
        }
    }
}

unsafe fn runpy_module_is_package(_py: &PyToken<'_>, module_dict_ptr: *mut u8) -> bool {
    unsafe {
        static MODULE_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        let path_name = intern_static_name(_py, &MODULE_PATH_NAME, b"__path__");
        if let Some(bits) = dict_get_in_place(_py, module_dict_ptr, path_name) {
            return !obj_from_bits(bits).is_none();
        }
        false
    }
}

unsafe fn runpy_apply_module_metadata(
    _py: &PyToken<'_>,
    module_dict_ptr: *mut u8,
    out_ptr: *mut u8,
    target_name: &str,
) -> Result<(), u64> {
    unsafe {
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_CACHED_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_DOC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);

        let target_name_ptr = alloc_string(_py, target_name.as_bytes());
        if target_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let target_name_bits = MoltObject::from_ptr(target_name_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__name__", target_name_bits)?;
        dec_ref_bits(_py, target_name_bits);

        let specials: [(&str, &[u8], &AtomicU64); 6] = [
            ("__file__", b"__file__", &MODULE_FILE_NAME),
            ("__package__", b"__package__", &MODULE_PACKAGE_NAME),
            ("__cached__", b"__cached__", &MODULE_CACHED_NAME),
            ("__spec__", b"__spec__", &MODULE_SPEC_NAME),
            ("__doc__", b"__doc__", &MODULE_DOC_NAME),
            ("__loader__", b"__loader__", &MODULE_LOADER_NAME),
        ];
        for (public_name, interned, slot) in specials {
            let key_bits = intern_static_name(_py, slot, interned);
            let value_bits = dict_get_in_place(_py, module_dict_ptr, key_bits)
                .unwrap_or_else(|| MoltObject::none().bits());
            dict_set_str_key_bits(_py, out_ptr, public_name, value_bits)?;
        }

        // Keep __name__ lookup warm for metadata reads in repeated runs.
        let _ = intern_static_name(_py, &MODULE_NAME_NAME, b"__name__");
        Ok(())
    }
}

fn runpy_package_name(run_name: &str) -> String {
    run_name
        .rsplit_once('.')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default()
}

unsafe fn runpy_sys_path_entries(_py: &PyToken<'_>) -> Vec<String> {
    unsafe {
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get("sys").copied()
        };
        let Some(sys_bits) = sys_bits else {
            return Vec::new();
        };
        let Some(sys_ptr) = obj_from_bits(sys_bits).as_ptr() else {
            return Vec::new();
        };
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return Vec::new();
        }
        let path_key_ptr = alloc_string(_py, b"path");
        if path_key_ptr.is_null() {
            return Vec::new();
        }
        let path_key_bits = MoltObject::from_ptr(path_key_ptr).bits();
        let path_bits = module_attr_lookup(_py, sys_ptr, path_key_bits);
        dec_ref_bits(_py, path_key_bits);
        let Some(path_bits) = path_bits else {
            return Vec::new();
        };
        let Some(path_ptr) = obj_from_bits(path_bits).as_ptr() else {
            dec_ref_bits(_py, path_bits);
            return Vec::new();
        };
        let entries = seq_vec_ref(path_ptr)
            .iter()
            .filter_map(|&bits| string_obj_to_owned(obj_from_bits(bits)))
            .collect::<Vec<_>>();
        dec_ref_bits(_py, path_bits);
        entries
    }
}

fn runpy_normalize_candidate(path: PathBuf) -> String {
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// ── VFS-aware filesystem helpers (Plan B v0.1) ──────────────────────
// These check the VFS mount table first and fall back to the real
// filesystem, so `import mymodule` finds `/bundle/mymodule.py` when
// `/bundle` is in `sys.path`.

/// Returns `true` if `path` is a file in the VFS or on disk.
fn vfs_is_file(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(&path_str)
    {
        return match backend.stat(&rel) {
            Ok(st) => !st.is_dir,
            Err(_) => false,
        };
    }
    path.is_file()
}

/// Read a file to `String`, trying VFS first then the real filesystem.
#[allow(dead_code)]
fn vfs_read_to_string(path: &std::path::Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(&path_str)
    {
        return match backend.open_read(&rel) {
            Ok(bytes) => String::from_utf8(bytes).ok(),
            Err(_) => None,
        };
    }
    std::fs::read_to_string(path).ok()
}

/// Read a file to raw bytes, trying VFS first then the real filesystem.
fn vfs_read(path: &str) -> std::io::Result<Vec<u8>> {
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(path)
    {
        return backend.open_read(&rel).map_err(|e| {
            let kind = match e {
                crate::vfs::VfsError::NotFound => std::io::ErrorKind::NotFound,
                crate::vfs::VfsError::PermissionDenied
                | crate::vfs::VfsError::ReadOnly
                | crate::vfs::VfsError::CapabilityDenied(_) => std::io::ErrorKind::PermissionDenied,
                crate::vfs::VfsError::IsDirectory => std::io::ErrorKind::IsADirectory,
                _ => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, e.to_string())
        });
    }
    std::fs::read(path)
}

fn runpy_resolve_module_source(
    mod_name: &str,
    sys_path: &[String],
) -> Option<(String, String, String)> {
    let parts = mod_name.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    for base in sys_path {
        let mut cur = if base.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(base)
        };
        let mut matched = true;
        for (idx, part) in parts.iter().enumerate() {
            let last = idx + 1 == parts.len();
            if last {
                let file_path = cur.join(format!("{part}.py"));
                if vfs_is_file(&file_path) {
                    let package_name = runpy_package_name(mod_name);
                    return Some((
                        runpy_normalize_candidate(file_path),
                        mod_name.to_string(),
                        package_name,
                    ));
                }
                let pkg_dir = cur.join(part);
                let init_path = pkg_dir.join("__init__.py");
                if vfs_is_file(&init_path) {
                    let main_path = pkg_dir.join("__main__.py");
                    if vfs_is_file(&main_path) {
                        return Some((
                            runpy_normalize_candidate(main_path),
                            format!("{mod_name}.__main__"),
                            mod_name.to_string(),
                        ));
                    }
                }
                matched = false;
            } else {
                cur.push(part);
                if !vfs_is_file(&cur.join("__init__.py")) {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
    }
    None
}

unsafe fn runpy_make_spec_obj(_py: &PyToken<'_>, import_name: &str) -> Result<u64, u64> {
    unsafe {
        let name_ptr = alloc_string(_py, import_name.as_bytes());
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let spec_ptr = alloc_module_obj(_py, name_bits);
        if spec_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let spec_bits = MoltObject::from_ptr(spec_ptr).bits();
        let dict_ptr = match obj_from_bits(module_dict_bits(spec_ptr)).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                dec_ref_bits(_py, spec_bits);
                dec_ref_bits(_py, name_bits);
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "spec dict missing",
                ));
            }
        };
        dict_set_str_key_bits(_py, dict_ptr, "name", name_bits)?;
        dec_ref_bits(_py, name_bits);
        Ok(spec_bits)
    }
}

unsafe fn runpy_apply_source_metadata(
    _py: &PyToken<'_>,
    out_ptr: *mut u8,
    target_name: &str,
    import_name: &str,
    source_path: &str,
    package_name: &str,
) -> Result<(), u64> {
    unsafe {
        let run_name_ptr = alloc_string(_py, target_name.as_bytes());
        if run_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let run_name_bits = MoltObject::from_ptr(run_name_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__name__", run_name_bits)?;
        dec_ref_bits(_py, run_name_bits);

        let path_ptr = alloc_string(_py, source_path.as_bytes());
        if path_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let path_bits = MoltObject::from_ptr(path_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__file__", path_bits)?;
        dec_ref_bits(_py, path_bits);

        let pkg_ptr = alloc_string(_py, package_name.as_bytes());
        if pkg_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let pkg_bits = MoltObject::from_ptr(pkg_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__package__", pkg_bits)?;
        dec_ref_bits(_py, pkg_bits);

        let none_bits = MoltObject::none().bits();
        dict_set_str_key_bits(_py, out_ptr, "__cached__", none_bits)?;
        dict_set_str_key_bits(_py, out_ptr, "__doc__", none_bits)?;
        dict_set_str_key_bits(_py, out_ptr, "__loader__", none_bits)?;

        let spec_bits = runpy_make_spec_obj(_py, import_name)?;
        dict_set_str_key_bits(_py, out_ptr, "__spec__", spec_bits)?;
        dec_ref_bits(_py, spec_bits);
        Ok(())
    }
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.as_bytes().iter().all(u8::is_ascii_digit)
}

fn is_identifier_text(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !is_xid_start(first) {
        return false;
    }
    chars.all(|ch| ch == '_' || is_xid_continue(ch))
}

fn is_dotted_identifier_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('.')
            .all(|segment| !segment.is_empty() && is_identifier_text(segment))
}

fn strip_inline_comment_text(text: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '#' => return text[..idx].trim_end(),
            _ => {}
        }
    }
    text.trim_end()
}

fn parse_restricted_import_item(part: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = part.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((left, right)) = trimmed.rsplit_once(" as ") {
        let module_name = left.trim();
        let alias = right.trim();
        if !is_dotted_identifier_text(module_name) || !is_identifier_text(alias) {
            return None;
        }
        return Some((module_name, Some(alias)));
    }
    if !is_dotted_identifier_text(trimmed) {
        return None;
    }
    Some((trimmed, None))
}

unsafe fn runpy_restricted_import_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    spec: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        for part in spec.split(',') {
            let Some((module_name, alias)) = parse_restricted_import_item(part) else {
                return Err(raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    &format!("unsupported import statement in {filename}"),
                ));
            };
            let imported_bits = runpy_import_module_bits(_py, module_name)?;
            if obj_from_bits(imported_bits).is_none() {
                if !exception_pending(_py) {
                    let message = format!("No module named '{module_name}'");
                    return Err(raise_exception::<_>(_py, "ImportError", &message));
                }
                return Err(MoltObject::none().bits());
            }

            let mut bind_bits = imported_bits;
            let bind_name = if let Some(alias_name) = alias {
                alias_name
            } else if let Some((head, _)) = module_name.split_once('.') {
                let top_bits = runpy_import_module_bits(_py, head)?;
                if !obj_from_bits(top_bits).is_none() {
                    bind_bits = top_bits;
                    dec_ref_bits(_py, imported_bits);
                }
                head
            } else {
                module_name
            };
            dict_set_str_key_bits(_py, namespace_ptr, bind_name, bind_bits)?;
            if !obj_from_bits(bind_bits).is_none() {
                dec_ref_bits(_py, bind_bits);
            }
        }
        Ok(())
    }
}

unsafe fn runpy_restricted_from_import_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    spec: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        let Some((module_name_raw, targets_raw)) = spec.split_once(" import ") else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported from-import statement in {filename}"),
            ));
        };
        let module_name = module_name_raw.trim();
        if !is_dotted_identifier_text(module_name) {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported from-import statement in {filename}"),
            ));
        }

        let module_bits = runpy_import_module_bits(_py, module_name)?;
        if obj_from_bits(module_bits).is_none() {
            if !exception_pending(_py) {
                let message = format!("No module named '{module_name}'");
                return Err(raise_exception::<_>(_py, "ImportError", &message));
            }
            return Err(MoltObject::none().bits());
        }

        for target in targets_raw.split(',') {
            let trimmed = target.trim();
            if trimmed.is_empty() {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    &format!("unsupported from-import statement in {filename}"),
                ));
            }
            if trimmed == "*" {
                if let Err(err) = runpy_restricted_import_star_into_namespace(
                    _py,
                    namespace_ptr,
                    module_bits,
                    module_name,
                ) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(err);
                }
                continue;
            }
            let (name, alias) = if let Some((left, right)) = trimmed.rsplit_once(" as ") {
                let left = left.trim();
                let right = right.trim();
                if !is_identifier_text(left) || !is_identifier_text(right) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported from-import statement in {filename}"),
                    ));
                }
                (left, right)
            } else {
                if !is_identifier_text(trimmed) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported from-import statement in {filename}"),
                    ));
                }
                (trimmed, trimmed)
            };

            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let missing = missing_bits(_py);
            let value_bits = molt_getattr_builtin(module_bits, name_bits, missing);
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if is_missing_bits(_py, value_bits) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                let message = format!("cannot import name '{name}' from '{module_name}'");
                return Err(raise_exception::<_>(_py, "ImportError", &message));
            }

            dict_set_str_key_bits(_py, namespace_ptr, alias, value_bits)?;
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
        }

        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        Ok(())
    }
}

unsafe fn runpy_restricted_import_star_into_namespace(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_bits: u64,
    module_name: &str,
) -> Result<(), u64> {
    unsafe {
        let import_star_error = || {
            let message = format!("cannot import name '*' from '{module_name}'");
            raise_exception::<u64>(_py, "ImportError", &message)
        };
        let mut module_obj_ptr = obj_from_bits(module_bits).as_ptr();
        let mut module_ty = module_obj_ptr.map(|ptr| object_type_id(ptr));
        if !matches!(module_ty, Some(TYPE_ID_MODULE | TYPE_ID_DICT)) {
            let sys_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("sys").copied()
            };
            if let Some(sys_bits) = sys_bits
                && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
            {
                let key_ptr = alloc_string(_py, module_name.as_bytes());
                if key_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let from_sys_bits = dict_get_in_place(_py, modules_ptr, key_bits);
                dec_ref_bits(_py, key_bits);
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                if let Some(bits) = from_sys_bits {
                    module_obj_ptr = obj_from_bits(bits).as_ptr();
                    module_ty = module_obj_ptr.map(|ptr| object_type_id(ptr));
                }
            }
        }
        let Some(module_obj_ptr) = module_obj_ptr else {
            return Err(import_star_error());
        };
        let module_ty = module_ty.unwrap_or_else(|| object_type_id(module_obj_ptr));
        let module_dict_ptr = if module_ty == TYPE_ID_MODULE {
            let module_dict_bits = module_dict_bits(module_obj_ptr);
            match obj_from_bits(module_dict_bits).as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => {
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "module dict missing",
                    ));
                }
            }
        } else if module_ty == TYPE_ID_DICT {
            module_obj_ptr
        } else {
            return Err(import_star_error());
        };

        let all_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.all_name, b"__all__");
        if let Some(all_bits) = dict_get_in_place(_py, module_dict_ptr, all_name_bits) {
            let iter_bits = molt_iter(all_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
                    return Err(MoltObject::none().bits());
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return Err(MoltObject::none().bits());
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return Err(MoltObject::none().bits());
                }
                if is_truthy(_py, obj_from_bits(elems[1])) {
                    break;
                }
                let name_bits = elems[0];
                match obj_from_bits(name_bits).as_ptr() {
                    Some(name_ptr) if object_type_id(name_ptr) == TYPE_ID_STRING => {}
                    _ => {
                        let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                        let message =
                            format!("Item in {module_name}.__all__ must be str, not {type_name}");
                        return Err(raise_exception::<_>(_py, "TypeError", &message));
                    }
                }
                let Some(value_bits) = dict_get_in_place(_py, module_dict_ptr, name_bits) else {
                    let name = string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                    let message = format!("module '{module_name}' has no attribute '{name}'");
                    return Err(raise_exception::<_>(_py, "AttributeError", &message));
                };
                dict_set_in_place(_py, namespace_ptr, name_bits, value_bits);
            }
            return Ok(());
        }

        let order = dict_order(module_dict_ptr);
        for idx in (0..order.len()).step_by(2) {
            let name_bits = order[idx];
            let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() else {
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
            let value_bits = order[idx + 1];
            dict_set_in_place(_py, namespace_ptr, name_bits, value_bits);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedLiteral {
    NoneValue,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<RestrictedLiteral>),
    Tuple(Vec<RestrictedLiteral>),
    Dict(Vec<(RestrictedLiteral, RestrictedLiteral)>),
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedReferenceIndex {
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedReferenceStep {
    Attr(String),
    Index(RestrictedReferenceIndex),
}

#[derive(Debug, Clone, PartialEq)]
struct RestrictedReferenceExpr {
    base: String,
    steps: Vec<RestrictedReferenceStep>,
}

fn split_identifier_prefix(text: &str) -> Option<(&str, &str)> {
    let mut chars = text.char_indices();
    let (first_idx, first) = chars.next()?;
    debug_assert_eq!(first_idx, 0);
    if first != '_' && !is_xid_start(first) {
        return None;
    }

    let mut end = text.len();
    for (idx, ch) in chars {
        if ch == '_' || is_xid_continue(ch) {
            continue;
        }
        end = idx;
        break;
    }
    Some((&text[..end], &text[end..]))
}

fn split_restricted_reference_index(text: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ']' => {
                let next = idx + ch.len_utf8();
                return Some((&text[..idx], &text[next..]));
            }
            _ => {}
        }
    }
    None
}

fn parse_restricted_string_literal(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    if !text.ends_with(quote) || text.chars().count() < 2 {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut iter = inner.chars();
    while let Some(ch) = iter.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match iter.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\'') if quote == '\'' => out.push('\''),
            Some('"') if quote == '"' => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    Some(out)
}

fn split_top_level_parts(text: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut round_depth = 0i32;
    let mut square_depth = 0i32;
    let mut curly_depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => round_depth += 1,
            ')' => {
                round_depth -= 1;
                if round_depth < 0 {
                    return None;
                }
            }
            '[' => square_depth += 1,
            ']' => {
                square_depth -= 1;
                if square_depth < 0 {
                    return None;
                }
            }
            '{' => curly_depth += 1,
            '}' => {
                curly_depth -= 1;
                if curly_depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        if ch == delimiter && round_depth == 0 && square_depth == 0 && curly_depth == 0 {
            parts.push(&text[start..idx]);
            start = idx + ch.len_utf8();
        }
    }
    if quote.is_some() || round_depth != 0 || square_depth != 0 || curly_depth != 0 {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}

fn split_top_level_once(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut round_depth = 0i32;
    let mut square_depth = 0i32;
    let mut curly_depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => round_depth += 1,
            ')' => {
                round_depth -= 1;
                if round_depth < 0 {
                    return None;
                }
            }
            '[' => square_depth += 1,
            ']' => {
                square_depth -= 1;
                if square_depth < 0 {
                    return None;
                }
            }
            '{' => curly_depth += 1,
            '}' => {
                curly_depth -= 1;
                if curly_depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        if ch == delimiter && round_depth == 0 && square_depth == 0 && curly_depth == 0 {
            let split_at = idx + ch.len_utf8();
            return Some((&text[..idx], &text[split_at..]));
        }
    }
    None
}

fn parse_restricted_bytes_literal(text: &str) -> Option<Vec<u8>> {
    let rest = text.strip_prefix('b').or_else(|| text.strip_prefix('B'))?;
    parse_restricted_string_literal(rest).map(|value| value.into_bytes())
}

fn parse_restricted_reference_index(text: &str) -> Option<RestrictedReferenceIndex> {
    match parse_restricted_literal(text.trim())? {
        RestrictedLiteral::Int(value) => Some(RestrictedReferenceIndex::Int(value)),
        RestrictedLiteral::Str(value) => Some(RestrictedReferenceIndex::Str(value)),
        _ => None,
    }
}

fn parse_restricted_reference_expr(text: &str) -> Option<RestrictedReferenceExpr> {
    let mut rest = text.trim();
    let (base, tail) = split_identifier_prefix(rest)?;
    let mut steps: Vec<RestrictedReferenceStep> = Vec::new();
    rest = tail;

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after_dot) = rest.strip_prefix('.') {
            let after_dot = after_dot.trim_start();
            let (attr, tail) = split_identifier_prefix(after_dot)?;
            if !is_identifier_text(attr) {
                return None;
            }
            steps.push(RestrictedReferenceStep::Attr(attr.to_string()));
            rest = tail;
            continue;
        }
        if let Some(after_open) = rest.strip_prefix('[') {
            let (inner, tail) = split_restricted_reference_index(after_open)?;
            let index = parse_restricted_reference_index(inner)?;
            steps.push(RestrictedReferenceStep::Index(index));
            rest = tail;
            continue;
        }
        return None;
    }

    Some(RestrictedReferenceExpr {
        base: base.to_string(),
        steps,
    })
}

fn parse_restricted_literal(text: &str) -> Option<RestrictedLiteral> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match text {
        "None" => return Some(RestrictedLiteral::NoneValue),
        "True" => return Some(RestrictedLiteral::Bool(true)),
        "False" => return Some(RestrictedLiteral::Bool(false)),
        _ => {}
    }
    if let Some(rest) = text.strip_prefix('+')
        && is_ascii_digits(rest)
    {
        return rest.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if let Some(rest) = text.strip_prefix('-')
        && is_ascii_digits(rest)
    {
        return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if is_ascii_digits(text) {
        return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if (text.contains('.') || text.contains('e') || text.contains('E'))
        && let Ok(value) = text.parse::<f64>()
    {
        return Some(RestrictedLiteral::Float(value));
    }
    if let Some(value) = parse_restricted_bytes_literal(text) {
        return Some(RestrictedLiteral::Bytes(value));
    }
    if text.starts_with('[') && text.ends_with(']') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::List(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let mut values: Vec<RestrictedLiteral> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            values.push(parse_restricted_literal(part)?);
        }
        return Some(RestrictedLiteral::List(values));
    }
    if text.starts_with('(') && text.ends_with(')') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::Tuple(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let has_top_level_comma = parts.len() > 1;
        let mut values: Vec<RestrictedLiteral> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            values.push(parse_restricted_literal(part)?);
        }
        if !has_top_level_comma && values.len() == 1 {
            return values.into_iter().next();
        }
        return Some(RestrictedLiteral::Tuple(values));
    }
    if text.starts_with('{') && text.ends_with('}') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::Dict(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let mut entries: Vec<(RestrictedLiteral, RestrictedLiteral)> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (left, right) = split_top_level_once(part, ':')?;
            let key = parse_restricted_literal(left.trim())?;
            let value = parse_restricted_literal(right.trim())?;
            entries.push((key, value));
        }
        return Some(RestrictedLiteral::Dict(entries));
    }
    parse_restricted_string_literal(text).map(RestrictedLiteral::Str)
}

fn restricted_literal_truthy(value: &RestrictedLiteral) -> bool {
    match value {
        RestrictedLiteral::NoneValue => false,
        RestrictedLiteral::Bool(flag) => *flag,
        RestrictedLiteral::Int(v) => *v != 0,
        RestrictedLiteral::Float(v) => *v != 0.0,
        RestrictedLiteral::Str(v) => !v.is_empty(),
        RestrictedLiteral::Bytes(v) => !v.is_empty(),
        RestrictedLiteral::List(v) => !v.is_empty(),
        RestrictedLiteral::Tuple(v) => !v.is_empty(),
        RestrictedLiteral::Dict(v) => !v.is_empty(),
    }
}

fn restricted_literal_to_bits(_py: &PyToken<'_>, value: RestrictedLiteral) -> Result<u64, u64> {
    match value {
        RestrictedLiteral::NoneValue => Ok(MoltObject::none().bits()),
        RestrictedLiteral::Bool(flag) => Ok(MoltObject::from_bool(flag).bits()),
        RestrictedLiteral::Int(value) => Ok(int_bits_from_i64(_py, value)),
        RestrictedLiteral::Float(value) => Ok(MoltObject::from_float(value).bits()),
        RestrictedLiteral::Str(value) => {
            let ptr = alloc_string(_py, value.as_bytes());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Bytes(value) => {
            let ptr = alloc_bytes(_py, value.as_slice());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::List(values) => {
            let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
            for item in values {
                bits_vec.push(restricted_literal_to_bits(_py, item)?);
            }
            let ptr = alloc_list(_py, bits_vec.as_slice());
            for bits in bits_vec {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Tuple(values) => {
            let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
            for item in values {
                bits_vec.push(restricted_literal_to_bits(_py, item)?);
            }
            let ptr = alloc_tuple(_py, bits_vec.as_slice());
            for bits in bits_vec {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Dict(entries) => {
            let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            for (key, value) in entries {
                pairs.push(restricted_literal_to_bits(_py, key)?);
                pairs.push(restricted_literal_to_bits(_py, value)?);
            }
            let ptr = alloc_dict_with_pairs(_py, &pairs);
            for bits in pairs {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
    }
}

unsafe fn runpy_restricted_namespace_lookup_bits(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    name: &str,
) -> Result<u64, u64> {
    unsafe {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_bits = dict_get_in_place(_py, namespace_ptr, name_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(value_bits) = value_bits else {
            let msg = format!("name '{name}' is not defined");
            return Err(raise_exception::<_>(_py, "NameError", &msg));
        };
        inc_ref_bits(_py, value_bits);
        Ok(value_bits)
    }
}

unsafe fn runpy_restricted_reference_attr_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    attr: &str,
) -> Result<u64, u64> {
    let attr_ptr = alloc_string(_py, attr.as_bytes());
    if attr_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
    let missing = missing_bits(_py);
    let out_bits = molt_getattr_builtin(value_bits, attr_bits, missing);
    dec_ref_bits(_py, attr_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, out_bits) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        let type_name = class_name_for_error(type_of_bits(_py, value_bits));
        let msg = format!("'{type_name}' object has no attribute '{attr}'");
        return Err(raise_exception::<_>(_py, "AttributeError", &msg));
    }
    Ok(out_bits)
}

unsafe fn runpy_restricted_reference_index_key_bits(
    _py: &PyToken<'_>,
    index: &RestrictedReferenceIndex,
) -> Result<u64, u64> {
    match index {
        RestrictedReferenceIndex::Int(value) => Ok(int_bits_from_i64(_py, *value)),
        RestrictedReferenceIndex::Str(value) => {
            let ptr = alloc_string(_py, value.as_bytes());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
    }
}

unsafe fn runpy_restricted_reference_subscript_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    index: &RestrictedReferenceIndex,
) -> Result<u64, u64> {
    unsafe {
        let key_bits = runpy_restricted_reference_index_key_bits(_py, index)?;
        let out_bits = crate::molt_index(value_bits, key_bits);
        if !obj_from_bits(key_bits).is_none() {
            dec_ref_bits(_py, key_bits);
        }
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        Ok(out_bits)
    }
}

unsafe fn runpy_eval_restricted_reference_expr(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    expr: &RestrictedReferenceExpr,
) -> Result<u64, u64> {
    unsafe {
        let mut current_bits =
            runpy_restricted_namespace_lookup_bits(_py, namespace_ptr, &expr.base)?;
        for step in &expr.steps {
            let next_bits = match step {
                RestrictedReferenceStep::Attr(attr) => {
                    runpy_restricted_reference_attr_bits(_py, current_bits, attr)
                }
                RestrictedReferenceStep::Index(index) => {
                    runpy_restricted_reference_subscript_bits(_py, current_bits, index)
                }
            };
            let next_bits = match next_bits {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(current_bits).is_none() {
                        dec_ref_bits(_py, current_bits);
                    }
                    return Err(err);
                }
            };
            if !obj_from_bits(current_bits).is_none() {
                dec_ref_bits(_py, current_bits);
            }
            current_bits = next_bits;
        }
        Ok(current_bits)
    }
}

unsafe fn runpy_exec_restricted_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    stripped: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        if stripped == "pass" {
            return Ok(());
        }
        if let Some(rest) = stripped.strip_prefix("import ") {
            runpy_restricted_import_stmt(_py, namespace_ptr, rest.trim(), filename)?;
            return Ok(());
        }
        if let Some(rest) = stripped.strip_prefix("from ") {
            runpy_restricted_from_import_stmt(_py, namespace_ptr, rest.trim(), filename)?;
            return Ok(());
        }
        if !stripped.contains('=') || stripped.contains("==") || stripped.contains("!=") {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported module statement in {filename}"),
            ));
        }
        let Some((left, right)) = stripped.split_once('=') else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported module statement in {filename}"),
            ));
        };
        let target = left.trim();
        if !is_identifier_text(target) {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported assignment target in {filename}"),
            ));
        }
        let rhs = right.trim();
        let value_bits = if let Some(value) = parse_restricted_literal(rhs) {
            restricted_literal_to_bits(_py, value)?
        } else if let Some(reference) = parse_restricted_reference_expr(rhs) {
            runpy_eval_restricted_reference_expr(_py, namespace_ptr, &reference)?
        } else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported assignment in {filename}"),
            ));
        };
        dict_set_str_key_bits(_py, namespace_ptr, target, value_bits)?;
        dec_ref_bits(_py, value_bits);
        Ok(())
    }
}

pub(crate) unsafe fn runpy_exec_restricted_source(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    source: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        let lines: Vec<&str> = source.lines().collect();
        let mut idx = 0usize;
        let mut saw_stmt = false;
        while idx < lines.len() {
            let raw = lines[idx];
            idx += 1;
            let stripped = strip_inline_comment_text(raw.trim());
            if stripped.is_empty() || stripped.starts_with('#') {
                continue;
            }
            if !saw_stmt && (stripped.starts_with("\"\"\"") || stripped.starts_with("'''")) {
                let quote = &stripped[..3];
                let doc = if stripped.ends_with(quote) && stripped.len() > 6 {
                    stripped[3..stripped.len() - 3].to_string()
                } else {
                    let mut doc_lines: Vec<String> = vec![stripped[3..].to_string()];
                    while idx < lines.len() {
                        let chunk = lines[idx];
                        idx += 1;
                        if let Some(end) = chunk.find(quote) {
                            doc_lines.push(chunk[..end].to_string());
                            break;
                        }
                        doc_lines.push(chunk.to_string());
                    }
                    doc_lines.join("\n")
                };
                let doc_ptr = alloc_string(_py, doc.as_bytes());
                if doc_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let doc_bits = MoltObject::from_ptr(doc_ptr).bits();
                dict_set_str_key_bits(_py, namespace_ptr, "__doc__", doc_bits)?;
                dec_ref_bits(_py, doc_bits);
                saw_stmt = true;
                continue;
            }

            saw_stmt = true;
            if let Some(cond_raw) = stripped
                .strip_prefix("if ")
                .and_then(|rest| rest.strip_suffix(':'))
            {
                let condition = parse_restricted_literal(cond_raw.trim()).ok_or_else(|| {
                    raise_exception::<u64>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported module statement in {filename}"),
                    )
                })?;
                let cond_true = restricted_literal_truthy(&condition);
                let current_indent = raw
                    .chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .count();
                let mut saw_indented_stmt = false;
                while idx < lines.len() {
                    let block_raw = lines[idx];
                    let block_indent = block_raw
                        .chars()
                        .take_while(|ch| *ch == ' ' || *ch == '\t')
                        .count();
                    let block_trimmed = strip_inline_comment_text(block_raw.trim());
                    if !block_trimmed.is_empty() && block_indent <= current_indent {
                        break;
                    }
                    idx += 1;
                    if block_trimmed.is_empty() || block_trimmed.starts_with('#') {
                        continue;
                    }
                    if block_indent <= current_indent {
                        continue;
                    }
                    saw_indented_stmt = true;
                    if !cond_true {
                        continue;
                    }
                    runpy_exec_restricted_stmt(_py, namespace_ptr, block_trimmed, filename)?;
                }
                if !saw_indented_stmt {
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported module statement in {filename}"),
                    ));
                }
                continue;
            }
            runpy_exec_restricted_stmt(_py, namespace_ptr, stripped, filename)?;
        }
        Ok(())
    }
}

struct RunpySysModulesSwapState {
    modules_ptr: *mut u8,
    key_bits: u64,
    previous_bits: Option<u64>,
}

struct RunpyArgv0SwapState {
    previous_arg0_bits: u64,
}

fn runpy_sys_module_bits(_py: &PyToken<'_>) -> Option<u64> {
    let cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = cache.lock().unwrap();
    guard.get("sys").copied()
}

unsafe fn runpy_begin_sys_modules_swap(
    _py: &PyToken<'_>,
    run_name: &str,
    module_bits: u64,
) -> Result<Option<RunpySysModulesSwapState>, u64> {
    unsafe {
        let Some(sys_bits) = runpy_sys_module_bits(_py) else {
            return Ok(None);
        };
        let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits) else {
            return Ok(None);
        };
        let run_name_ptr = alloc_string(_py, run_name.as_bytes());
        if run_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let run_name_bits = MoltObject::from_ptr(run_name_ptr).bits();
        let previous_bits = dict_get_in_place(_py, modules_ptr, run_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, run_name_bits);
            return Err(MoltObject::none().bits());
        }
        if let Some(bits) = previous_bits {
            inc_ref_bits(_py, bits);
        }
        dict_set_in_place(_py, modules_ptr, run_name_bits, module_bits);
        if exception_pending(_py) {
            if let Some(bits) = previous_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, run_name_bits);
            return Err(MoltObject::none().bits());
        }
        Ok(Some(RunpySysModulesSwapState {
            modules_ptr,
            key_bits: run_name_bits,
            previous_bits,
        }))
    }
}

unsafe fn runpy_restore_sys_modules_swap(
    _py: &PyToken<'_>,
    state: &mut Option<RunpySysModulesSwapState>,
) {
    unsafe {
        let Some(state) = state.take() else {
            return;
        };
        let saved_exc_bits = if exception_pending(_py) {
            let bits = molt_exception_last();
            clear_exception(_py);
            Some(bits)
        } else {
            None
        };
        if let Some(bits) = state.previous_bits {
            dict_set_in_place(_py, state.modules_ptr, state.key_bits, bits);
            dec_ref_bits(_py, bits);
        } else {
            dict_del_in_place(_py, state.modules_ptr, state.key_bits);
        }
        dec_ref_bits(_py, state.key_bits);
        if let Some(saved_bits) = saved_exc_bits {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            if !obj_from_bits(saved_bits).is_none() {
                let _ = crate::molt_exception_set_last(saved_bits);
            }
            dec_ref_bits(_py, saved_bits);
        }
    }
}

unsafe fn runpy_sys_argv_list_bits(_py: &PyToken<'_>) -> Result<Option<u64>, u64> {
    unsafe {
        static SYS_ARGV_NAME: AtomicU64 = AtomicU64::new(0);
        let Some(sys_bits) = runpy_sys_module_bits(_py) else {
            return Ok(None);
        };
        let Some(sys_ptr) = obj_from_bits(sys_bits).as_ptr() else {
            return Ok(None);
        };
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return Ok(None);
        }
        let argv_name_bits = intern_static_name(_py, &SYS_ARGV_NAME, b"argv");
        if obj_from_bits(argv_name_bits).is_none() {
            return if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(None)
            };
        }
        let Some(argv_bits) = module_attr_lookup(_py, sys_ptr, argv_name_bits) else {
            if exception_pending(_py) {
                if clear_attribute_error_if_pending(_py) {
                    return Ok(None);
                }
                return Err(MoltObject::none().bits());
            }
            return Ok(None);
        };
        let is_list = obj_from_bits(argv_bits)
            .as_ptr()
            .map(|ptr| object_type_id(ptr) == TYPE_ID_LIST)
            .unwrap_or(false);
        if !is_list {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        }
        Ok(Some(argv_bits))
    }
}

unsafe fn runpy_begin_sys_argv0_swap(
    _py: &PyToken<'_>,
    argv0_text: &str,
) -> Result<Option<RunpyArgv0SwapState>, u64> {
    unsafe {
        let Some(argv_bits) = runpy_sys_argv_list_bits(_py)? else {
            return Ok(None);
        };
        let Some(argv_ptr) = obj_from_bits(argv_bits).as_ptr() else {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        };
        let argv_vec = seq_vec(argv_ptr);
        if argv_vec.is_empty() {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        }
        let argv0_ptr = alloc_string(_py, argv0_text.as_bytes());
        if argv0_ptr.is_null() {
            dec_ref_bits(_py, argv_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let argv0_bits = MoltObject::from_ptr(argv0_ptr).bits();
        let previous_arg0_bits = argv_vec[0];
        inc_ref_bits(_py, previous_arg0_bits);
        if previous_arg0_bits != argv0_bits {
            inc_ref_bits(_py, argv0_bits);
            argv_vec[0] = argv0_bits;
            dec_ref_bits(_py, previous_arg0_bits);
        }
        dec_ref_bits(_py, argv0_bits);
        dec_ref_bits(_py, argv_bits);
        Ok(Some(RunpyArgv0SwapState { previous_arg0_bits }))
    }
}

unsafe fn runpy_restore_sys_argv0_swap(_py: &PyToken<'_>, state: &mut Option<RunpyArgv0SwapState>) {
    unsafe {
        let Some(state) = state.take() else {
            return;
        };
        let saved_exc_bits = if exception_pending(_py) {
            let bits = molt_exception_last();
            clear_exception(_py);
            Some(bits)
        } else {
            None
        };
        match runpy_sys_argv_list_bits(_py) {
            Ok(Some(argv_bits)) => {
                if let Some(argv_ptr) = obj_from_bits(argv_bits).as_ptr()
                    && object_type_id(argv_ptr) == TYPE_ID_LIST
                {
                    let argv_vec = seq_vec(argv_ptr);
                    if !argv_vec.is_empty() {
                        let current_bits = argv_vec[0];
                        if current_bits != state.previous_arg0_bits {
                            inc_ref_bits(_py, state.previous_arg0_bits);
                            argv_vec[0] = state.previous_arg0_bits;
                            dec_ref_bits(_py, current_bits);
                        }
                    }
                }
                dec_ref_bits(_py, argv_bits);
            }
            Ok(None) => {}
            Err(_) => {}
        }
        dec_ref_bits(_py, state.previous_arg0_bits);
        if let Some(saved_bits) = saved_exc_bits {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            if !obj_from_bits(saved_bits).is_none() {
                let _ = crate::molt_exception_set_last(saved_bits);
            }
            dec_ref_bits(_py, saved_bits);
        }
    }
}

unsafe fn runpy_run_module_from_resolved_source(
    _py: &PyToken<'_>,
    source_path: &str,
    import_name: &str,
    package_name: &str,
    requested_run_name: Option<&str>,
    init_dict_ptr: Option<*mut u8>,
    alter_sys: bool,
) -> u64 {
    let source_bytes = match vfs_read(source_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let message = err.to_string();
            return match err.kind() {
                std::io::ErrorKind::NotFound => {
                    raise_exception::<_>(_py, "FileNotFoundError", &message)
                }
                std::io::ErrorKind::PermissionDenied => {
                    raise_exception::<_>(_py, "PermissionError", &message)
                }
                std::io::ErrorKind::IsADirectory => {
                    raise_exception::<_>(_py, "IsADirectoryError", &message)
                }
                _ => raise_exception::<_>(_py, "OSError", &message),
            };
        }
    };
    let source = String::from_utf8_lossy(&source_bytes).into_owned();
    let target_name = requested_run_name.unwrap_or(import_name);
    let out_ptr = alloc_dict_with_pairs(_py, &[]);
    if out_ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    let out_bits = MoltObject::from_ptr(out_ptr).bits();
    let mut alter_sys_modules_state: Option<RunpySysModulesSwapState> = None;
    let mut alter_sys_argv0_state: Option<RunpyArgv0SwapState> = None;
    let exec_result = unsafe {
        (|| -> Result<(), u64> {
            if let Some(init_ptr) = init_dict_ptr {
                dict_copy_entries(_py, init_ptr, out_ptr);
            }
            runpy_apply_source_metadata(
                _py,
                out_ptr,
                target_name,
                import_name,
                source_path,
                package_name,
            )?;
            if alter_sys {
                alter_sys_modules_state = runpy_begin_sys_modules_swap(_py, target_name, out_bits)?;
                alter_sys_argv0_state = runpy_begin_sys_argv0_swap(_py, source_path)?;
            }
            runpy_exec_restricted_source(_py, out_ptr, &source, source_path)?;
            Ok(())
        })()
    };
    unsafe {
        runpy_restore_sys_argv0_swap(_py, &mut alter_sys_argv0_state);
        runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
    }
    if let Err(err) = exec_result {
        dec_ref_bits(_py, out_bits);
        return err;
    }
    if exception_pending(_py) {
        dec_ref_bits(_py, out_bits);
        return MoltObject::none().bits();
    }
    out_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_module(
    mod_name_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
    alter_sys_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mod_name = match string_obj_to_owned(obj_from_bits(mod_name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "mod_name must be str"),
        };
        let requested_run_name = {
            let run_name_obj = obj_from_bits(run_name_bits);
            if run_name_obj.is_none() {
                None
            } else {
                match string_obj_to_owned(run_name_obj) {
                    Some(val) => Some(val),
                    None => {
                        return raise_exception::<_>(_py, "TypeError", "run_name must be str");
                    }
                }
            }
        };
        let init_dict_ptr = {
            let init_obj = obj_from_bits(init_globals_bits);
            if init_obj.is_none() {
                None
            } else {
                match init_obj.as_ptr() {
                    Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(ptr),
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "init_globals must be dict or None",
                        );
                    }
                }
            }
        };
        let alter_sys = is_truthy(_py, obj_from_bits(alter_sys_bits));
        if alter_sys {
            let sys_path = unsafe { runpy_sys_path_entries(_py) };
            if let Some((source_path, import_name, package_name)) =
                runpy_resolve_module_source(&mod_name, &sys_path)
            {
                return unsafe {
                    runpy_run_module_from_resolved_source(
                        _py,
                        &source_path,
                        &import_name,
                        &package_name,
                        requested_run_name.as_deref(),
                        init_dict_ptr,
                        true,
                    )
                };
            }
        }
        let mut import_name = mod_name.clone();
        let mut module_bits = match unsafe { runpy_import_module_bits(_py, &import_name) } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
            let msg = format!("No module named '{mod_name}'");
            return raise_exception::<_>(_py, "ImportError", &msg);
        }
        if obj_from_bits(module_bits).is_none() {
            return MoltObject::none().bits();
        }
        let payload_is_module_or_dict = obj_from_bits(module_bits)
            .as_ptr()
            .map(|ptr| {
                let ty = unsafe { object_type_id(ptr) };
                ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT
            })
            .unwrap_or(false);
        if !payload_is_module_or_dict {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            let sys_path = unsafe { runpy_sys_path_entries(_py) };
            let Some((source_path, import_name, package_name)) =
                runpy_resolve_module_source(&mod_name, &sys_path)
            else {
                let msg = format!("No module named '{mod_name}'");
                return raise_exception::<_>(_py, "ImportError", &msg);
            };
            return unsafe {
                runpy_run_module_from_resolved_source(
                    _py,
                    &source_path,
                    &import_name,
                    &package_name,
                    requested_run_name.as_deref(),
                    init_dict_ptr,
                    alter_sys,
                )
            };
        }
        let mut module_dict_ptr = match unsafe { runpy_module_dict_ptr(_py, module_bits) } {
            Ok(ptr) => ptr,
            Err(bits) => {
                dec_ref_bits(_py, module_bits);
                return bits;
            }
        };
        if unsafe { runpy_module_is_package(_py, module_dict_ptr) } {
            let package_bits = module_bits;
            import_name = format!("{mod_name}.__main__");
            module_bits = match unsafe { runpy_import_module_bits(_py, &import_name) } {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, package_bits);
                    return bits;
                }
            };
            dec_ref_bits(_py, package_bits);
            if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
                let message = format!(
                    "No module named '{import_name}'; '{mod_name}' is a package and cannot be directly executed"
                );
                return raise_exception::<_>(_py, "ImportError", &message);
            }
            if obj_from_bits(module_bits).is_none() {
                return MoltObject::none().bits();
            }
            module_dict_ptr = match unsafe { runpy_module_dict_ptr(_py, module_bits) } {
                Ok(ptr) => ptr,
                Err(bits) => {
                    dec_ref_bits(_py, module_bits);
                    return bits;
                }
            };
        }
        let target_name = requested_run_name
            .clone()
            .unwrap_or_else(|| import_name.clone());
        let mut alter_sys_modules_state: Option<RunpySysModulesSwapState> = None;
        if alter_sys {
            alter_sys_modules_state =
                match unsafe { runpy_begin_sys_modules_swap(_py, &target_name, module_bits) } {
                    Ok(state) => state,
                    Err(err) => {
                        dec_ref_bits(_py, module_bits);
                        return err;
                    }
                };
        }
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            unsafe {
                runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
            }
            dec_ref_bits(_py, module_bits);
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let out_bits = MoltObject::from_ptr(out_ptr).bits();
        let metadata_result: Result<(), u64> = unsafe {
            dict_copy_entries(_py, module_dict_ptr, out_ptr);
            if let Some(init_ptr) = init_dict_ptr {
                dict_copy_entries(_py, init_ptr, out_ptr);
            }
            runpy_apply_module_metadata(_py, module_dict_ptr, out_ptr, &target_name)
        };
        unsafe {
            runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
        }
        if let Err(err) = metadata_result {
            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, module_bits);
            return err;
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, module_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_path(
    path_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("runpy.run_path", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "path must be str"),
        };
        let run_name = {
            let run_name_obj = obj_from_bits(run_name_bits);
            if run_name_obj.is_none() {
                "<run_path>".to_string()
            } else {
                match string_obj_to_owned(run_name_obj) {
                    Some(value) => value,
                    None => return raise_exception::<_>(_py, "TypeError", "run_name must be str"),
                }
            }
        };
        let init_dict_ptr = {
            let init_obj = obj_from_bits(init_globals_bits);
            if init_obj.is_none() {
                None
            } else {
                match init_obj.as_ptr() {
                    Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(ptr),
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "init_globals must be dict or None",
                        );
                    }
                }
            }
        };
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return raise_exception::<_>(_py, "FileNotFoundError", &path),
            Err(err) => {
                let message = err.to_string();
                return match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &message)
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &message)
                    }
                    std::io::ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, "IsADirectoryError", &message)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &message),
                };
            }
        }
        let source_bytes = match vfs_read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let message = err.to_string();
                return match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &message)
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &message)
                    }
                    std::io::ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, "IsADirectoryError", &message)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &message),
                };
            }
        };
        let source = String::from_utf8_lossy(&source_bytes).into_owned();
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let out_bits = MoltObject::from_ptr(out_ptr).bits();
        let mut argv0_swap_state = match unsafe { runpy_begin_sys_argv0_swap(_py, &path) } {
            Ok(state) => state,
            Err(err) => {
                dec_ref_bits(_py, out_bits);
                return err;
            }
        };
        let exec_result = unsafe {
            (|| -> Result<(), u64> {
                if let Some(init_ptr) = init_dict_ptr {
                    dict_copy_entries(_py, init_ptr, out_ptr);
                }

                let run_name_ptr = alloc_string(_py, run_name.as_bytes());
                if run_name_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let run_name_value_bits = MoltObject::from_ptr(run_name_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__name__", run_name_value_bits)?;
                dec_ref_bits(_py, run_name_value_bits);

                let path_ptr = alloc_string(_py, path.as_bytes());
                if path_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let path_value_bits = MoltObject::from_ptr(path_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__file__", path_value_bits)?;
                dec_ref_bits(_py, path_value_bits);

                let package = runpy_package_name(&run_name);
                let package_ptr = alloc_string(_py, package.as_bytes());
                if package_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let package_bits = MoltObject::from_ptr(package_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__package__", package_bits)?;
                dec_ref_bits(_py, package_bits);

                let none_bits = MoltObject::none().bits();
                dict_set_str_key_bits(_py, out_ptr, "__cached__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__spec__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__doc__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__loader__", none_bits)?;

                // NOTE(dynamic-exec-policy): Keep runpy on restricted source execution for
                // compiled binaries. Full code-object execution is intentionally deferred
                // until an explicit capability-gated design, perf evidence, and user
                // approval are in place.
                runpy_exec_restricted_source(_py, out_ptr, &source, &path)?;
                Ok(())
            })()
        };
        unsafe {
            runpy_restore_sys_argv0_swap(_py, &mut argv0_swap_state);
        }
        if let Err(err) = exec_result {
            dec_ref_bits(_py, out_bits);
            return err;
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, out_bits);
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_restricted_source(
    namespace_bits: u64,
    source_bits: u64,
    filename_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "source must be str"),
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "filename must be str"),
        };
        // NOTE(dynamic-exec-policy): Keep importlib source execution in the
        // restricted intrinsic lane for compiled binaries. Do not widen to
        // unrestricted code-object execution without an approved capability gate
        // and measured perf impact.
        unsafe {
            if let Err(err) = runpy_exec_restricted_source(_py, namespace_ptr, &source, &filename) {
                return err;
            }
        }
        MoltObject::none().bits()
    })
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
    let bits = copyreg_dict_slot_bits(_py, &COPYREG_DISPATCH_TABLE_BITS);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_extension_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &COPYREG_EXTENSION_REGISTRY_BITS);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_inverted_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &COPYREG_INVERTED_REGISTRY_BITS);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_extension_cache_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_dict_slot_bits(_py, &COPYREG_EXTENSION_CACHE_BITS);
    let ptr = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(ptr)
}

fn copyreg_constructor_registry_ptr(_py: &PyToken<'_>) -> Option<*mut u8> {
    let bits = copyreg_set_slot_bits(_py, &COPYREG_CONSTRUCTOR_REGISTRY_BITS);
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
        set_add_in_place(_py, set_ptr, func_bits);
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
    crate::with_gil_entry!(_py, {
        let dispatch_bits = copyreg_dict_slot_bits(_py, &COPYREG_DISPATCH_TABLE_BITS);
        let extension_bits = copyreg_dict_slot_bits(_py, &COPYREG_EXTENSION_REGISTRY_BITS);
        let inverted_bits = copyreg_dict_slot_bits(_py, &COPYREG_INVERTED_REGISTRY_BITS);
        let cache_bits = copyreg_dict_slot_bits(_py, &COPYREG_EXTENSION_CACHE_BITS);
        let constructor_bits = copyreg_set_slot_bits(_py, &COPYREG_CONSTRUCTOR_REGISTRY_BITS);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        if let Err(err_bits) = copyreg_add_constructor(_py, func_bits) {
            return err_bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_copyreg_newobj(cls_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

fn sys_modules_dict_ptr(_py: &PyToken<'_>, sys_bits: u64) -> Option<*mut u8> {
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
        let modules_ptr = match obj_from_bits(modules_bits?).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "sys.modules must be dict"),
        };
        Some(modules_ptr)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            if let Some(&existing) = guard.get(&name) {
                if existing != 0
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
                        unsafe {
                            dict_set_in_place(_py, modules_ptr, name_bits, existing);
                        }
                        existing
                    } else {
                        existing
                    };
                }
            }
            if let Some(old) = guard.insert(name, module_bits) {
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
                unsafe {
                    dict_set_in_place(_py, modules_ptr, name_bits, module_bits);
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
                        let exc_bits = molt_exception_last();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string());
                        eprintln!("sys module pending after argv/executable: {kind}");
                    }
                    if sys_populate_stdio(_py, sys_ptr).is_err() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if std::env::var("MOLT_TRACE_SYS_MODULE").as_deref() == Ok("1")
                        && exception_pending(_py)
                    {
                        let exc_bits = molt_exception_last();
                        let kind_bits = molt_exception_kind(exc_bits);
                        let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string());
                        eprintln!("sys module pending after stdio: {kind}");
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
    crate::with_gil_entry!(_py, {
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
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<exc>".to_string());
                let detail = obj_from_bits(exc_bits)
                    .as_ptr()
                    .map(|ptr| format_exception_with_traceback(_py, ptr))
                    .unwrap_or_else(|| "<no traceback>".to_string());
                eprintln!("module init failed: {kind} while importing {name}: {detail}");
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
            unsafe {
                if object_type_id(sys_ptr) != TYPE_ID_MODULE {
                    return MoltObject::none().bits();
                }
                let dict_bits = module_dict_bits(sys_ptr);
                let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
                    Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                    _ => return MoltObject::none().bits(),
                };
                let modules_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.modules_name, b"modules");
                if obj_from_bits(modules_name_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let Some(modules_bits) = dict_get_in_place(_py, dict_ptr, modules_name_bits) else {
                    return MoltObject::none().bits();
                };
                let modules_ptr = match obj_from_bits(modules_bits).as_ptr() {
                    Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                    _ => return MoltObject::none().bits(),
                };
                dict_del_in_place(_py, modules_ptr, name_bits);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = trace_name_error();
        let trace_globals_mode = trace_module_globals_mode();
        let trace_globals = trace_globals_mode != TraceModuleGlobalsMode::Off;
        let trace_globals_all = trace_globals_mode == TraceModuleGlobalsMode::Verbose;
        let trace_name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<name>".to_string());
        let trace_name_match = trace_globals_all
            || trace_name == "_SYS_FLAGS_SEQUENCE_FIELDS"
            || trace_name == "_FlagsTuple";
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = trace_name_error();
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
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
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<name>".to_string());
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
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_name(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
                if pep649_enabled() {
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
                && pep649_enabled()
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
    crate::with_gil_entry!(_py, {
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

    #[test]
    fn restricted_literal_parser_supports_core_values() {
        assert_eq!(
            parse_restricted_literal("None"),
            Some(RestrictedLiteral::NoneValue)
        );
        assert_eq!(
            parse_restricted_literal("True"),
            Some(RestrictedLiteral::Bool(true))
        );
        assert_eq!(
            parse_restricted_literal("-12"),
            Some(RestrictedLiteral::Int(-12))
        );
        assert_eq!(
            parse_restricted_literal("1.25"),
            Some(RestrictedLiteral::Float(1.25))
        );
        assert_eq!(
            parse_restricted_literal("'hello\\nworld'"),
            Some(RestrictedLiteral::Str("hello\nworld".to_string()))
        );
        assert_eq!(
            parse_restricted_literal("b'abc'"),
            Some(RestrictedLiteral::Bytes(b"abc".to_vec()))
        );
        assert_eq!(
            parse_restricted_literal("[1, 2, 3]"),
            Some(RestrictedLiteral::List(vec![
                RestrictedLiteral::Int(1),
                RestrictedLiteral::Int(2),
                RestrictedLiteral::Int(3),
            ]))
        );
        assert_eq!(
            parse_restricted_literal("(1, 'x')"),
            Some(RestrictedLiteral::Tuple(vec![
                RestrictedLiteral::Int(1),
                RestrictedLiteral::Str("x".to_string()),
            ]))
        );
        assert_eq!(
            parse_restricted_literal("{'a': 1, 'b': [2, 3]}"),
            Some(RestrictedLiteral::Dict(vec![
                (
                    RestrictedLiteral::Str("a".to_string()),
                    RestrictedLiteral::Int(1),
                ),
                (
                    RestrictedLiteral::Str("b".to_string()),
                    RestrictedLiteral::List(vec![
                        RestrictedLiteral::Int(2),
                        RestrictedLiteral::Int(3),
                    ]),
                ),
            ]))
        );
    }

    #[test]
    fn identifier_parser_matches_basic_python_rules() {
        assert!(is_identifier_text("_value"));
        assert!(is_identifier_text("alpha9"));
        assert!(is_identifier_text("Δx"));
        assert!(!is_identifier_text("9abc"));
        assert!(!is_identifier_text("a-b"));
        assert!(is_dotted_identifier_text("pkg.mod"));
        assert!(!is_dotted_identifier_text("pkg..mod"));
    }

    #[test]
    fn strip_inline_comment_preserves_strings() {
        assert_eq!(strip_inline_comment_text("x = 1 # tail"), "x = 1");
        assert_eq!(strip_inline_comment_text("x = 'a#b'  # tail"), "x = 'a#b'");
        assert_eq!(
            strip_inline_comment_text("x = \"a#b\\\"c\" # tail"),
            "x = \"a#b\\\"c\""
        );
    }

    #[test]
    fn restricted_reference_parser_accepts_attr_and_literal_indices() {
        assert_eq!(
            parse_restricted_reference_expr("sys.argv[0]"),
            Some(RestrictedReferenceExpr {
                base: "sys".to_string(),
                steps: vec![
                    RestrictedReferenceStep::Attr("argv".to_string()),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Int(0)),
                ],
            })
        );
        assert_eq!(
            parse_restricted_reference_expr("cfg.paths['root'][1]"),
            Some(RestrictedReferenceExpr {
                base: "cfg".to_string(),
                steps: vec![
                    RestrictedReferenceStep::Attr("paths".to_string()),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Str(
                        "root".to_string()
                    )),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Int(1)),
                ],
            })
        );
        assert_eq!(
            parse_restricted_reference_expr("value"),
            Some(RestrictedReferenceExpr {
                base: "value".to_string(),
                steps: vec![],
            })
        );
    }

    #[test]
    fn restricted_reference_parser_rejects_dynamic_rhs_forms() {
        assert_eq!(parse_restricted_reference_expr("a[b]"), None);
        assert_eq!(parse_restricted_reference_expr("items[1:3]"), None);
        assert_eq!(parse_restricted_reference_expr("loader()"), None);
        assert_eq!(parse_restricted_reference_expr("sys.argv[0]()"), None);
        assert_eq!(parse_restricted_reference_expr("a + b"), None);
        assert_eq!(parse_restricted_reference_expr("sys.argv[True]"), None);
    }

    #[test]
    fn restricted_reference_eval_supports_module_attr_then_subscript() {
        crate::with_gil_entry!(_py, {
            unsafe {
                let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!namespace_ptr.is_null());
                let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();

                let name_ptr = alloc_string(_py, b"sys");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let module_ptr = alloc_module_obj(_py, name_bits);
                assert!(!module_ptr.is_null());
                dec_ref_bits(_py, name_bits);
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let argv0_ptr = alloc_string(_py, b"/tmp/runpy_test.py");
                assert!(!argv0_ptr.is_null());
                let argv0_bits = MoltObject::from_ptr(argv0_ptr).bits();
                let argv_ptr = alloc_list(_py, &[argv0_bits]);
                assert!(!argv_ptr.is_null());
                dec_ref_bits(_py, argv0_bits);
                let argv_bits = MoltObject::from_ptr(argv_ptr).bits();

                let module_dict = module_dict_bits(module_ptr);
                let module_dict_ptr = obj_from_bits(module_dict)
                    .as_ptr()
                    .expect("module dict ptr");
                assert_eq!(object_type_id(module_dict_ptr), TYPE_ID_DICT);
                dict_set_str_key_bits(_py, module_dict_ptr, "argv", argv_bits).expect("set argv");
                dec_ref_bits(_py, argv_bits);

                dict_set_str_key_bits(_py, namespace_ptr, "sys", module_bits).expect("set sys");
                dec_ref_bits(_py, module_bits);

                let expr = parse_restricted_reference_expr("sys.argv[0]").expect("reference parse");
                let value_bits =
                    runpy_eval_restricted_reference_expr(_py, namespace_ptr, &expr).expect("eval");
                let value_text = string_obj_to_owned(obj_from_bits(value_bits));
                assert_eq!(value_text.as_deref(), Some("/tmp/runpy_test.py"));
                dec_ref_bits(_py, value_bits);

                dec_ref_bits(_py, namespace_bits);
            }
        });
    }

    #[test]
    fn runpy_package_name_uses_parent_module() {
        assert_eq!(runpy_package_name("pkg.tool"), "pkg");
        assert_eq!(runpy_package_name("single"), "");
    }

    #[test]
    fn sys_module_cache_set_does_not_leave_pending_exception() {
        crate::with_gil_entry!(_py, {
            let name_ptr = alloc_string(_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_ptr = alloc_module_obj(_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits = molt_module_cache_set(name_bits, module_bits);
            assert!(
                !exception_pending(_py),
                "sys module registration must not leave a pending exception"
            );
            assert!(
                !obj_from_bits(result_bits).is_none(),
                "sys module cache set should return a non-None success value"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
        });
    }
}
