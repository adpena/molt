use crate::PyToken;
use molt_obj_model::MoltObject;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::module_attr_lookup;
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    alloc_dict_with_pairs, alloc_list, alloc_module_obj, alloc_string, alloc_tuple,
    class_name_for_error, dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order,
    dict_set_in_place, exception_pending, format_exception_with_traceback, has_capability,
    inc_ref_bits, init_atomic_bits, int_bits_from_i64, intern_static_name, is_truthy,
    module_dict_bits, module_name_bits, molt_exception_kind, molt_exception_last,
    molt_int_from_obj, molt_is_callable, molt_iter, molt_iter_next, obj_eq, obj_from_bits,
    object_type_id, raise_exception, runtime_state, seq_vec_ref, set_add_in_place, string_bytes,
    string_len, string_obj_to_owned, to_i64, type_name, type_of_bits, TYPE_ID_DICT, TYPE_ID_MODULE,
    TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE,
};
use unicode_ident::{is_xid_continue, is_xid_start};

#[cfg(not(target_arch = "wasm32"))]
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

static TRACE_LAST_OP: AtomicU64 = AtomicU64::new(0);
static TRACE_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);
static COPYREG_DISPATCH_TABLE_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_EXTENSION_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_INVERTED_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_EXTENSION_CACHE_BITS: AtomicU64 = AtomicU64::new(0);
static COPYREG_CONSTRUCTOR_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);

#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" fn trace_sigtrap_handler(sig: i32) {
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

#[cfg(not(target_arch = "wasm32"))]
fn ensure_sigtrap_handler() {
    if trace_op_sigtrap_enabled() && !TRACE_SIGTRAP_INSTALLED.swap(true, Ordering::Relaxed) {
        unsafe {
            libc::signal(libc::SIGTRAP, trace_sigtrap_handler as usize);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn ensure_sigtrap_handler() {}

unsafe fn sys_populate_argv_executable(_py: &PyToken<'_>, sys_ptr: *mut u8) -> Result<(), ()> {
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

unsafe fn sys_populate_stdio(_py: &PyToken<'_>, sys_ptr: *mut u8) -> Result<(), ()> {
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

#[no_mangle]
pub extern "C" fn molt_module_new(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        }
        let name = match string_obj_to_owned(name_obj) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let ptr = alloc_module_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let dict_bits = module_dict_bits(ptr);
            let dict_obj = obj_from_bits(dict_bits);
            if let Some(dict_ptr) = dict_obj.as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let key_ptr = alloc_string(_py, b"__name__");
                    if !key_ptr.is_null() {
                        let key_bits = MoltObject::from_ptr(key_ptr).bits();
                        dict_set_in_place(_py, dict_ptr, key_bits, name_bits);
                        dec_ref_bits(_py, key_bits);
                    }
                }
            }
        }
        if name == "builtins" || name == "_intrinsics" {
            crate::intrinsics::install_into_builtins(_py, ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let trace = trace_module_cache();
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        if let Some(bits) = guard.get(&name) {
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

#[no_mangle]
pub extern "C" fn molt_module_import(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        #[cfg(not(target_arch = "wasm32"))]
        let module_bits = unsafe { molt_isolate_import(name_bits) };
        #[cfg(target_arch = "wasm32")]
        let module_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            if let Some(bits) = guard.get(&name) {
                inc_ref_bits(_py, *bits);
                *bits
            } else {
                MoltObject::none().bits()
            }
        };
        if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
            let msg = format!("No module named '{name}'");
            return raise_exception::<_>(_py, "ImportError", &msg);
        }
        module_bits
    })
}

unsafe fn dict_copy_entries(_py: &PyToken<'_>, src_ptr: *mut u8, dst_ptr: *mut u8) {
    let source_order = dict_order(src_ptr);
    for idx in (0..source_order.len()).step_by(2) {
        let key_bits = source_order[idx];
        let val_bits = source_order[idx + 1];
        dict_set_in_place(_py, dst_ptr, key_bits, val_bits);
    }
}

unsafe fn dict_set_str_key_bits(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key: &str,
    value_bits: u64,
) -> Result<(), u64> {
    let key_ptr = alloc_string(_py, key.as_bytes());
    if key_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    dec_ref_bits(_py, key_bits);
    Ok(())
}

fn runpy_package_name(run_name: &str) -> String {
    run_name
        .rsplit_once('.')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default()
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

#[derive(Debug, Clone, PartialEq)]
enum RestrictedLiteral {
    NoneValue,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
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

fn parse_restricted_literal(text: &str) -> Option<RestrictedLiteral> {
    match text {
        "None" => return Some(RestrictedLiteral::NoneValue),
        "True" => return Some(RestrictedLiteral::Bool(true)),
        "False" => return Some(RestrictedLiteral::Bool(false)),
        _ => {}
    }
    if let Some(rest) = text.strip_prefix('+') {
        if is_ascii_digits(rest) {
            return rest.parse::<i64>().ok().map(RestrictedLiteral::Int);
        }
    }
    if let Some(rest) = text.strip_prefix('-') {
        if is_ascii_digits(rest) {
            return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
        }
    }
    if is_ascii_digits(text) {
        return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if text.contains('.') || text.contains('e') || text.contains('E') {
        if let Ok(value) = text.parse::<f64>() {
            return Some(RestrictedLiteral::Float(value));
        }
    }
    parse_restricted_string_literal(text).map(RestrictedLiteral::Str)
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
    }
}

unsafe fn runpy_exec_restricted_source(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    source: &str,
    filename: &str,
) -> Result<(), u64> {
    let lines: Vec<&str> = source.lines().collect();
    let mut idx = 0usize;
    let mut saw_stmt = false;
    while idx < lines.len() {
        let raw = lines[idx];
        idx += 1;
        let stripped = raw.trim();
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
        if stripped == "pass" {
            continue;
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
        let value = parse_restricted_literal(right.trim()).ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "NotImplementedError",
                &format!("unsupported assignment in {filename}"),
            )
        })?;
        let value_bits = restricted_literal_to_bits(_py, value)?;
        dict_set_str_key_bits(_py, namespace_ptr, target, value_bits)?;
        dec_ref_bits(_py, value_bits);
    }
    Ok(())
}

#[no_mangle]
pub extern "C" fn molt_runpy_run_module(
    mod_name_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mod_name = match string_obj_to_owned(obj_from_bits(mod_name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "mod_name must be str"),
        };
        let target_name = {
            let run_name_obj = obj_from_bits(run_name_bits);
            if run_name_obj.is_none() {
                mod_name.clone()
            } else {
                match string_obj_to_owned(run_name_obj) {
                    Some(val) => val,
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
        #[cfg(not(target_arch = "wasm32"))]
        let module_bits = unsafe { molt_isolate_import(mod_name_bits) };
        #[cfg(target_arch = "wasm32")]
        let module_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            if let Some(bits) = guard.get(&mod_name) {
                inc_ref_bits(_py, *bits);
                *bits
            } else {
                MoltObject::none().bits()
            }
        };
        if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
            let msg = format!("No module named '{mod_name}'");
            return raise_exception::<_>(_py, "ImportError", &msg);
        }
        let module_ptr = match obj_from_bits(module_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_MODULE } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "module import expects module"),
        };
        let module_dict_ptr = unsafe {
            match obj_from_bits(module_dict_bits(module_ptr)).as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            }
        };
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        unsafe {
            let source_order = dict_order(module_dict_ptr);
            for idx in (0..source_order.len()).step_by(2) {
                let key_bits = source_order[idx];
                let val_bits = source_order[idx + 1];
                dict_set_in_place(_py, out_ptr, key_bits, val_bits);
            }
            if let Some(init_ptr) = init_dict_ptr {
                let init_order = dict_order(init_ptr);
                for idx in (0..init_order.len()).step_by(2) {
                    let key_bits = init_order[idx];
                    let val_bits = init_order[idx + 1];
                    dict_set_in_place(_py, out_ptr, key_bits, val_bits);
                }
            }
        }
        let name_key_ptr = alloc_string(_py, b"__name__");
        if name_key_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let target_name_ptr = alloc_string(_py, target_name.as_bytes());
        if target_name_ptr.is_null() {
            let name_key_bits = MoltObject::from_ptr(name_key_ptr).bits();
            dec_ref_bits(_py, name_key_bits);
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let name_key_bits = MoltObject::from_ptr(name_key_ptr).bits();
        let target_name_bits = MoltObject::from_ptr(target_name_ptr).bits();
        unsafe {
            dict_set_in_place(_py, out_ptr, name_key_bits, target_name_bits);
        }
        dec_ref_bits(_py, name_key_bits);
        dec_ref_bits(_py, target_name_bits);
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_runpy_run_path(
    path_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
                        )
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
        let source_bytes = match std::fs::read(&path) {
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
        unsafe {
            if let Some(init_ptr) = init_dict_ptr {
                dict_copy_entries(_py, init_ptr, out_ptr);
            }

            let run_name_ptr = alloc_string(_py, run_name.as_bytes());
            if run_name_ptr.is_null() {
                dec_ref_bits(_py, out_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let run_name_value_bits = MoltObject::from_ptr(run_name_ptr).bits();
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__name__", run_name_value_bits) {
                dec_ref_bits(_py, run_name_value_bits);
                dec_ref_bits(_py, out_bits);
                return err;
            }
            dec_ref_bits(_py, run_name_value_bits);

            let path_ptr = alloc_string(_py, path.as_bytes());
            if path_ptr.is_null() {
                dec_ref_bits(_py, out_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let path_value_bits = MoltObject::from_ptr(path_ptr).bits();
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__file__", path_value_bits) {
                dec_ref_bits(_py, path_value_bits);
                dec_ref_bits(_py, out_bits);
                return err;
            }
            dec_ref_bits(_py, path_value_bits);

            let package = runpy_package_name(&run_name);
            let package_ptr = alloc_string(_py, package.as_bytes());
            if package_ptr.is_null() {
                dec_ref_bits(_py, out_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let package_bits = MoltObject::from_ptr(package_ptr).bits();
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__package__", package_bits) {
                dec_ref_bits(_py, package_bits);
                dec_ref_bits(_py, out_bits);
                return err;
            }
            dec_ref_bits(_py, package_bits);

            let none_bits = MoltObject::none().bits();
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__cached__", none_bits) {
                dec_ref_bits(_py, out_bits);
                return err;
            }
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__spec__", none_bits) {
                dec_ref_bits(_py, out_bits);
                return err;
            }
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__doc__", none_bits) {
                dec_ref_bits(_py, out_bits);
                return err;
            }
            if let Err(err) = dict_set_str_key_bits(_py, out_ptr, "__loader__", none_bits) {
                dec_ref_bits(_py, out_bits);
                return err;
            }

            // TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P1, status:partial): replace restricted runpy source execution with full code-object execution once eval/exec runtime lowering is available.
            if let Err(err) = runpy_exec_restricted_source(_py, out_ptr, &source, &path) {
                dec_ref_bits(_py, out_bits);
                return err;
            }
        }
        out_bits
    })
}

#[no_mangle]
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
        // TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P1, status:partial): replace restricted module source execution with full code-object execution once eval/exec runtime lowering is available.
        unsafe {
            if let Err(err) = runpy_exec_restricted_source(_py, namespace_ptr, &source, &filename) {
                return err;
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
    }

    #[test]
    fn identifier_parser_matches_basic_python_rules() {
        assert!(is_identifier_text("_value"));
        assert!(is_identifier_text("alpha9"));
        assert!(is_identifier_text("Δx"));
        assert!(!is_identifier_text("9abc"));
        assert!(!is_identifier_text("a-b"));
    }

    #[test]
    fn runpy_package_name_uses_parent_module() {
        assert_eq!(runpy_package_name("pkg.tool"), "pkg");
        assert_eq!(runpy_package_name("single"), "");
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

#[no_mangle]
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

#[no_mangle]
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
        if !obj_from_bits(constructor_bits).is_none() {
            if let Err(err_bits) = copyreg_add_constructor(_py, constructor_bits) {
                return err_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_copyreg_constructor(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(err_bits) = copyreg_add_constructor(_py, func_bits) {
            return err_bits;
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
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
            if let Some(found_key_bits) = existing_key_bits {
                if obj_eq(_py, obj_from_bits(found_bits), obj_from_bits(code_key_bits))
                    && obj_eq(_py, obj_from_bits(found_key_bits), obj_from_bits(key_bits))
                {
                    dec_ref_bits(_py, key_bits);
                    dec_ref_bits(_py, code_key_bits);
                    return MoltObject::none().bits();
                }
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
        if let Some(sys_bits) = sys_bits {
            if let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits) {
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
        }
        if is_sys {
            let sys_obj = obj_from_bits(module_bits);
            if let Some(sys_ptr) = sys_obj.as_ptr() {
                unsafe {
                    if sys_populate_argv_executable(_py, sys_ptr).is_err() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    if sys_populate_stdio(_py, sys_ptr).is_err() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let debug_attr = std::env::var("MOLT_DEBUG_MODULE_GET_ATTR").as_deref() == Ok("1");
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            if debug_attr {
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                    .unwrap_or_else(|| "<attr>".to_string());
                eprintln!("molt module_get_attr invalid module for attr={}", attr_name);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                if debug_attr {
                    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                        .unwrap_or_else(|| "<attr>".to_string());
                    eprintln!("molt module_get_attr non-module for attr={}", attr_name);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute access expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let _dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if let Some(val) = module_attr_lookup(_py, module_ptr, attr_bits) {
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
                    if let Some(key_name) = string_obj_to_owned(obj_from_bits(pair[0])) {
                        if key_name == attr_name {
                            present = true;
                            break;
                        }
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

#[no_mangle]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = trace_name_error();
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
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
            if let Some(val) = dict_get_in_place(_py, dict_ptr, name_bits) {
                inc_ref_bits(_py, val);
                return val;
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
                    if !builtins_dict_ptr.is_null() {
                        if let Some(val) = dict_get_in_place(_py, builtins_dict_ptr, name_bits) {
                            inc_ref_bits(_py, val);
                            return val;
                        }
                    }
                }
            }
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<name>".to_string());
            if trace {
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                    .unwrap_or_else(|| "<module>".to_string());
                let pending = exception_pending(_py);
                eprintln!(
                    "molt name error module={} name={} pending={}",
                    module_name, name, pending
                );
            }
            let msg = format!("name '{name}' is not defined");
            raise_exception::<_>(_py, "NameError", &msg)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = trace_name_error();
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
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

#[no_mangle]
pub extern "C" fn molt_module_get_name(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Keep wasm import parity; module __name__ is stored in the module dict.
        molt_module_get_attr(module_bits, attr_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace_attrs = trace_module_attrs();
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module attribute set expects module");
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
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
                if attr_name == "_sys" || module_name.contains("importlib") {
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

#[no_mangle]
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
