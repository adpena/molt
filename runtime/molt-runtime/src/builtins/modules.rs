use crate::PyToken;
use molt_obj_model::MoltObject;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use libc;

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::module_attr_lookup;
use crate::builtins::io::{molt_sys_stderr, molt_sys_stdin, molt_sys_stdout};
use crate::{
    alloc_dict_with_pairs, alloc_list, alloc_module_obj, alloc_string, class_name_for_error,
    dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order, dict_set_in_place,
    exception_pending, format_exception_with_traceback, inc_ref_bits, intern_static_name,
    is_truthy, module_dict_bits, module_name_bits, molt_exception_kind, molt_exception_last,
    molt_is_callable, molt_iter, molt_iter_next, obj_eq, obj_from_bits, object_type_id,
    raise_exception, runtime_state, seq_vec_ref, string_bytes, string_len, string_obj_to_owned,
    type_name, type_of_bits, TYPE_ID_DICT, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE,
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
        .unwrap_or_else(|| args.first().cloned().unwrap_or_default());
    let mut elems = Vec::with_capacity(args.len());
    for arg in args.iter() {
        let ptr = alloc_string(_py, arg.as_bytes());
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

    let exec_val_ptr = alloc_string(_py, exec_val.as_bytes());
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
        if name == "builtins" {
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
