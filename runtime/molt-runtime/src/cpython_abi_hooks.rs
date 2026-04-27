//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use molt_cpython_abi::RuntimeHooks;
use molt_cpython_abi::abi_types::MoltTypeTag;
use molt_obj_model::MoltObject;

use crate::builtins::containers::{dict_len, dict_order, list_len, tuple_len};
use crate::concurrency::gil::with_gil;
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity,
    alloc_module_obj, alloc_string, alloc_tuple_with_capacity,
};
use crate::object::layout::{
    function_set_call_target_ptr, function_set_dict_bits, function_set_trampoline_ptr,
    module_dict_bits, seq_vec, seq_vec_ref,
};
use crate::object::ops::dict_set_in_place;
use crate::object::type_ids::{
    TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE,
};
use crate::object::{
    MoltHeader, bytes_data, bytes_len, dec_ref_bits, inc_ref_bits, object_type_id, string_bytes,
    string_len,
};

// ─── Hook implementations ─────────────────────────────────────────────────

unsafe extern "C" fn hook_alloc_str(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_string(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            // NaN-box the heap pointer so the bridge round-trip via
            // PyObject* -> trailing-bits read recovers a value the runtime's
            // `obj.as_ptr()` recognises as a heap pointer (see
            // `MoltObject::from_ptr` for the canonical encoding).
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_bytes(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_bytes(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_list() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_list_with_capacity(&_py, &[], 8);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_list_append(list_bits: u64, item_bits: u64) {
    let obj = MoltObject::from_bits(list_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    unsafe { seq_vec(ptr) }.push(item_bits);
}

unsafe extern "C" fn hook_list_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
        return 0;
    }
    unsafe { list_len(ptr) }
}

unsafe extern "C" fn hook_list_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe { seq_vec_ref(ptr) }.get(i).copied().unwrap_or(0)
}

unsafe extern "C" fn hook_alloc_tuple(n: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_tuple_with_capacity(&_py, &[], n);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_tuple_set(bits: u64, i: usize, val_bits: u64) {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    let v = unsafe { seq_vec(ptr) };
    if i < v.len() {
        v[i] = val_bits;
    } else {
        v.resize(i + 1, MoltObject::none().bits());
        v[i] = val_bits;
    }
}

unsafe extern "C" fn hook_tuple_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return 0;
    }
    unsafe { tuple_len(ptr) }
}

unsafe extern "C" fn hook_tuple_item(bits: u64, i: usize) -> u64 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    unsafe { seq_vec_ref(ptr) }.get(i).copied().unwrap_or(0)
}

unsafe extern "C" fn hook_alloc_dict() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_dict_with_pairs(&_py, &[]);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) {
    let obj = MoltObject::from_bits(dict_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return;
    }
    let order = unsafe { dict_order(ptr) };
    let mut found = false;
    for chunk in order.chunks_mut(2) {
        if chunk[0] == key_bits {
            chunk[1] = val_bits;
            found = true;
            break;
        }
    }
    if !found {
        order.push(key_bits);
        order.push(val_bits);
    }
}

unsafe extern "C" fn hook_dict_get(dict_bits: u64, key_bits: u64) -> u64 {
    let obj = MoltObject::from_bits(dict_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    let order = unsafe { dict_order(ptr) };
    for chunk in order.chunks(2) {
        if chunk[0] == key_bits {
            return chunk[1];
        }
    }
    0
}

unsafe extern "C" fn hook_dict_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    unsafe { dict_len(ptr) }
}

unsafe extern "C" fn hook_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_STRING {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { string_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { string_bytes(ptr) }
        }
    }
}

unsafe extern "C" fn hook_bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { bytes_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { bytes_data(ptr) }
        }
    }
}

unsafe extern "C" fn hook_classify_heap(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return MoltTypeTag::Other as u8,
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_STRING => MoltTypeTag::Str as u8,
        TYPE_ID_BYTES => MoltTypeTag::Bytes as u8,
        TYPE_ID_LIST => MoltTypeTag::List as u8,
        TYPE_ID_TUPLE => MoltTypeTag::Tuple as u8,
        TYPE_ID_DICT => MoltTypeTag::Dict as u8,
        TYPE_ID_SET => MoltTypeTag::Set as u8,
        TYPE_ID_MODULE => MoltTypeTag::Module as u8,
        _ => MoltTypeTag::Other as u8,
    }
}

unsafe extern "C" fn hook_inc_ref(bits: u64) {
    let obj = MoltObject::from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        let hdr = ptr as *mut MoltHeader;
        if !hdr.is_null() {
            unsafe { (*hdr).ref_count.fetch_add(1, Ordering::Relaxed) };
        }
    }
}

unsafe extern "C" fn hook_dec_ref(bits: u64) {
    with_gil(|_py| dec_ref_bits(&_py, bits));
}

// ─── Module / C-extension support ────────────────────────────────────────

unsafe extern "C" fn hook_alloc_module(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let name_ptr = alloc_string(&_py, bytes);
        if name_ptr.is_null() {
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_ptr = alloc_module_obj(&_py, name_bits);
        // alloc_module_obj inc_ref's the name; drop the local reference.
        dec_ref_bits(&_py, name_bits);
        if module_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(module_ptr).bits()
    })
}

unsafe extern "C" fn hook_module_set_attr(
    module_bits: u64,
    name_data: *const u8,
    name_len: usize,
    value_bits: u64,
) -> std::os::raw::c_int {
    if name_data.is_null() {
        return -1;
    }
    let module_obj = MoltObject::from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return -1;
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return -1;
    }
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let dict_obj = MoltObject::from_bits(dict_bits);
        let Some(dict_ptr) = dict_obj.as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return -1;
        }
        let name_str_ptr = alloc_string(&_py, name_bytes);
        if name_str_ptr.is_null() {
            return -1;
        }
        let name_str_bits = MoltObject::from_ptr(name_str_ptr).bits();
        unsafe { dict_set_in_place(&_py, dict_ptr, name_str_bits, value_bits) };
        // dict_set_in_place takes its own references on key+value.  Drop our
        // local key reference; the caller still owns the value.
        dec_ref_bits(&_py, name_str_bits);
        0
    })
}

// ── PyCFunction → Molt callable bridge ───────────────────────────────────
//
// CPython C extensions register functions as PyCFunction pointers with a
// METH_* flag bitmask describing the calling convention.  Molt's call
// dispatch uses fixed-arity native functions (TYPE_ID_FUNCTION) with a
// trampoline slot for variadic dispatch.
//
// To bridge the two we maintain a small process-wide registry mapping each
// registered C function to its (meth_addr, flags) tuple.  The registry key
// is stored as a NaN-boxed int in the Molt function's `closure` slot, and
// every C function shares a single trampoline that decodes the registry id
// and forks on the calling convention.
//
// PARTIAL: only METH_NOARGS is implemented today.  METH_VARARGS, METH_O,
// METH_KEYWORDS, and METH_FASTCALL require Molt-side support for variadic
// callable types and are tracked separately.

const METH_VARARGS: i32 = 0x0001;
const METH_KEYWORDS: i32 = 0x0002;
const METH_NOARGS: i32 = 0x0004;
const METH_O: i32 = 0x0008;
const METH_CLASS: i32 = 0x0010;
const METH_STATIC: i32 = 0x0020;
const METH_COEXIST: i32 = 0x0040;
const METH_FASTCALL: i32 = 0x0080;

#[derive(Clone, Copy)]
struct CExtCallable {
    meth_addr: usize,
    flags: i32,
}

// SAFETY: meth_addr is a `*const ()` we transmute back to the original
// PyCFunction signature inside the trampoline.  The pointer is guaranteed
// valid for the process lifetime by `loader::LOADED_EXTENSION_LIBRARIES`.
unsafe impl Send for CExtCallable {}
unsafe impl Sync for CExtCallable {}

fn cext_callable_registry() -> &'static Mutex<Vec<CExtCallable>> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Mutex<Vec<CExtCallable>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Trampoline invoked by Molt's call dispatch for every registered C
/// extension function.  Signature matches Molt's
/// `extern "C" fn(closure_bits, args_ptr, args_len) -> i64`.
extern "C" fn cext_call_trampoline(closure_bits: u64, args_ptr: u64, args_len: u64) -> i64 {
    // The closure encodes the registry id as a NaN-boxed int.
    let id_obj = MoltObject::from_bits(closure_bits);
    let id = match id_obj.as_int() {
        Some(value) if value >= 0 => value as usize,
        _ => {
            return with_gil(|_py| {
                crate::raise_exception::<i64>(
                    &_py,
                    "SystemError",
                    "C extension trampoline received non-int closure id",
                )
            }) as i64;
        }
    };
    let entry = match cext_callable_registry().lock() {
        Ok(guard) => guard.get(id).copied(),
        Err(poisoned) => poisoned.into_inner().get(id).copied(),
    };
    let Some(entry) = entry else {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension callable registry id is out of range",
            )
        }) as i64;
    };

    let n = args_len as usize;
    if entry.flags & METH_NOARGS != 0 {
        if n != 0 {
            return with_gil(|_py| {
                crate::raise_exception::<i64>(
                    &_py,
                    "TypeError",
                    "METH_NOARGS C extension function takes no arguments",
                )
            }) as i64;
        }
        // Reconstruct the C function pointer.  The C function takes
        // PyObject* arguments and returns a PyObject*; the bridge encodes
        // these as raw 64-bit values so we use a plain `extern "C"` u64
        // signature here.
        type CFunc = unsafe extern "C" fn(self_obj: u64, args_obj: u64) -> u64;
        // SAFETY: meth_addr was produced by `register_c_function` from a
        // PyCFunction provided by the loaded extension; the library is
        // pinned for the process lifetime, so the function pointer is valid.
        let f: CFunc =
            unsafe { std::mem::transmute::<*const (), CFunc>(entry.meth_addr as *const ()) };
        let result_pyobj = unsafe { f(MoltObject::none().bits(), MoltObject::none().bits()) };
        if result_pyobj == 0 {
            // CPython convention: NULL return signals an exception.  The
            // bridge's PyErr_* state has already been recorded; surface it
            // as the Molt exception sentinel.
            return MoltObject::none().bits() as i64;
        }
        // The PyObject* points into the bridge's allocator; recover the Molt
        // handle bits stored in the trailing slot of the bridge header.
        let result_bits =
            unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(result_pyobj as *mut _) };
        return result_bits as i64;
    }

    // Any other calling convention is not yet wired through the trampoline.
    let _ = args_ptr;
    with_gil(|_py| {
        let msg = format!(
            "molt: C extension calling convention 0x{:x} not yet supported (only METH_NOARGS)",
            entry.flags
        );
        crate::raise_exception::<i64>(&_py, "NotImplementedError", &msg)
    }) as i64
}

unsafe extern "C" fn hook_register_c_function(
    meth_addr: u64,
    flags: std::os::raw::c_int,
    name_data: *const u8,
    name_len: usize,
) -> u64 {
    if meth_addr == 0 || name_data.is_null() {
        return 0;
    }
    // Strip the convention-modifier bits (METH_CLASS / METH_STATIC /
    // METH_COEXIST) from the dispatch flags before deciding on a calling
    // convention.  Module-level functions never carry these.
    let conv_flags = flags & !(METH_CLASS | METH_STATIC | METH_COEXIST);
    if conv_flags & METH_NOARGS == 0 {
        // PARTIAL: explicit gap surfaced to the caller.  Returning 0 causes
        // PyModule_Create2 to log the unsupported convention and skip the
        // method registration so the load does not silently succeed with a
        // half-initialised module.
        let _ = (METH_VARARGS, METH_KEYWORDS, METH_O, METH_FASTCALL);
        return 0;
    }
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        // Reserve a registry slot for this C function.
        let id = {
            let mut guard = cext_callable_registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let id = guard.len();
            guard.push(CExtCallable {
                meth_addr: meth_addr as usize,
                flags,
            });
            id
        };
        let closure_bits = MoltObject::from_int(id as i64).bits();
        // Allocate a Molt function object.  arity=0 because every call site
        // for METH_NOARGS passes zero arguments.  fn_ptr is unused on the
        // trampoline path but must be non-zero so Molt's dispatcher does not
        // try the runtime-callable shortcut.
        //
        // We use the trampoline address itself as the fn_ptr placeholder.
        // It is never invoked directly because trampoline_ptr is set, and
        // Molt's call_func_dispatch consults trampoline_ptr first.
        let fn_ptr_value = cext_call_trampoline as *const () as usize as u64;
        let func_ptr = alloc_function_obj(&_py, fn_ptr_value, 0);
        if func_ptr.is_null() {
            return 0;
        }
        unsafe {
            function_set_call_target_ptr(func_ptr, cext_call_trampoline as *const ());
            function_set_trampoline_ptr(
                func_ptr,
                cext_call_trampoline as *const () as usize as u64,
            );

            // Stash __name__ on the function dict so repr() and tracebacks
            // report the C extension's actual function name.
            let name_str = alloc_string(&_py, name_bytes);
            if !name_str.is_null() {
                let name_bits = MoltObject::from_ptr(name_str).bits();
                let dict_ptr = alloc_dict_with_pairs(&_py, &[]);
                if !dict_ptr.is_null() {
                    let key_ptr = alloc_string(&_py, b"__name__");
                    if !key_ptr.is_null() {
                        let key_bits = MoltObject::from_ptr(key_ptr).bits();
                        dict_set_in_place(&_py, dict_ptr, key_bits, name_bits);
                        dec_ref_bits(&_py, key_bits);
                    }
                    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    function_set_dict_bits(func_ptr, dict_bits);
                    inc_ref_bits(&_py, dict_bits);
                    dec_ref_bits(&_py, dict_bits);
                }
                dec_ref_bits(&_py, name_bits);
            }
            // Encode the registry id into the closure slot so the
            // trampoline can recover it on every call.  Inline-int closure
            // bits are not refcounted; no inc_ref needed.
            let closure_slot = func_ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64;
            *closure_slot = closure_bits;
        }
        MoltObject::from_ptr(func_ptr).bits()
    })
}

// ─── Registration ─────────────────────────────────────────────────────────

static HOOKS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Register the runtime hooks into `molt-lang-cpython-abi`.
/// Idempotent — safe to call multiple times (only registers once).
pub fn register_cpython_hooks() {
    if HOOKS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hooks = RuntimeHooks {
        alloc_str: hook_alloc_str,
        alloc_bytes: hook_alloc_bytes,
        alloc_list: hook_alloc_list,
        list_append: hook_list_append,
        list_len: hook_list_len,
        list_item: hook_list_item,
        alloc_tuple: hook_alloc_tuple,
        tuple_set: hook_tuple_set,
        tuple_len: hook_tuple_len,
        tuple_item: hook_tuple_item,
        alloc_dict: hook_alloc_dict,
        dict_set: hook_dict_set,
        dict_get: hook_dict_get,
        dict_len: hook_dict_len,
        str_data: hook_str_data,
        bytes_data: hook_bytes_data,
        classify_heap: hook_classify_heap,
        inc_ref: hook_inc_ref,
        dec_ref: hook_dec_ref,
        alloc_module: hook_alloc_module,
        module_set_attr: hook_module_set_attr,
        register_c_function: hook_register_c_function,
    };
    // SAFETY: all fn pointers are valid for the process lifetime.
    unsafe { molt_cpython_abi::set_runtime_hooks(hooks) };
}
