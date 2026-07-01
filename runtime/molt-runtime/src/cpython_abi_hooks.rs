//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use std::ffi::CStr;
use std::ptr;

use molt_cpython_abi::abi_types::{
    METH_CLASS, METH_COEXIST, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O,
    METH_STATIC, METH_VARARGS, MoltTypeTag, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionWithKeywords, PyModuleDef, PyObject,
};
use molt_cpython_abi::{MoltBufferView as AbiMoltBufferView, RuntimeHooks};
use molt_obj_model::MoltObject;
use num_traits::ToPrimitive;

use crate::builtins::containers::{dict_len, dict_order, list_len, tuple_len};
use crate::builtins::numbers::{int_bits_from_i64, int_bits_from_i128, to_bigint, to_i64};
use crate::concurrency::gil::with_gil;
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity,
    alloc_module_obj, alloc_string, alloc_tuple_with_capacity,
};
use crate::object::layout::{
    function_set_call_target_ptr, function_set_dict_bits, function_set_trampoline_ptr,
    module_dict_bits, seq_vec, seq_vec_ref,
};
use crate::object::ops::{dict_del_in_place, dict_set_in_place};
use crate::object::type_ids::{
    TYPE_ID_BIGINT, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_SET,
    TYPE_ID_STRING, TYPE_ID_TUPLE,
};
use crate::object::{
    HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, MoltHeader, bytes_data, bytes_len, dec_ref_bits,
    header_from_obj_ptr, inc_ref_bits, object_type_id, string_bytes, string_len,
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

unsafe extern "C" fn hook_int_from_i64(value: i64) -> u64 {
    with_gil(|_py| int_bits_from_i64(&_py, value))
}

unsafe extern "C" fn hook_int_from_u64(value: u64) -> u64 {
    with_gil(|_py| int_bits_from_i128(&_py, value as i128))
}

unsafe extern "C" fn hook_int_as_i64(bits: u64) -> i64 {
    with_gil(|_py| to_i64(MoltObject::from_bits(bits)).unwrap_or(-1))
}

unsafe extern "C" fn hook_int_as_i64_checked(bits: u64, out: *mut i64) -> i32 {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| match to_i64(MoltObject::from_bits(bits)) {
        Some(value) => {
            unsafe {
                *out = value;
            }
            0
        }
        None => -1,
    })
}

unsafe extern "C" fn hook_int_as_u64_checked(bits: u64, out: *mut u64) -> i32 {
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = to_i64(obj) {
            if value < 0 {
                return -1;
            }
            unsafe {
                *out = value as u64;
            }
            return 0;
        }
        if let Some(value) = to_bigint(obj).and_then(|value| value.to_u64()) {
            unsafe {
                *out = value;
            }
            return 0;
        }
        -1
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

unsafe extern "C" fn hook_dict_del(dict_bits: u64, key_bits: u64) -> i32 {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let ptr = match obj.as_ptr() {
            Some(p) => p,
            None => return -1,
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return -1;
        }
        if unsafe { dict_del_in_place(&_py, ptr, key_bits) } {
            0
        } else {
            -1
        }
    })
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

unsafe extern "C" fn hook_buffer_acquire(bits: u64, out_view: *mut AbiMoltBufferView) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    let mut view = crate::c_api::MoltBufferView {
        data: std::ptr::null_mut(),
        len: 0,
        readonly: 1,
        ndim: 1,
        itemsize: 1,
        offset: 0,
        owner: 0,
        base: 0,
        shape: [0; crate::MOLT_BUFFER_MAX_NDIM],
        strides: [0; crate::MOLT_BUFFER_MAX_NDIM],
        format: [0; crate::MOLT_BUFFER_FORMAT_CAP],
    };
    let rc = unsafe { crate::c_api::molt_buffer_acquire(bits, &mut view as *mut _) };
    if rc != 0 {
        return rc;
    }
    unsafe {
        *out_view = AbiMoltBufferView {
            data: view.data,
            len: view.len,
            readonly: view.readonly,
            ndim: view.ndim,
            itemsize: view.itemsize,
            offset: view.offset,
            owner: view.owner,
            base: view.base,
            shape: view.shape,
            strides: view.strides,
            format: view.format,
        };
    }
    0
}

unsafe extern "C" fn hook_buffer_release(view: *mut AbiMoltBufferView) -> i32 {
    if view.is_null() {
        return -1;
    }
    let mut runtime_view = unsafe {
        crate::c_api::MoltBufferView {
            data: (*view).data,
            len: (*view).len,
            readonly: (*view).readonly,
            ndim: (*view).ndim,
            itemsize: (*view).itemsize,
            offset: (*view).offset,
            owner: (*view).owner,
            base: (*view).base,
            shape: (*view).shape,
            strides: (*view).strides,
            format: (*view).format,
        }
    };
    let rc = unsafe { crate::c_api::molt_buffer_release(&mut runtime_view as *mut _) };
    unsafe {
        *view = AbiMoltBufferView::default();
    }
    rc
}

unsafe extern "C" fn hook_object_get_attr(obj_bits: u64, name_bits: u64) -> u64 {
    crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits)
}

unsafe extern "C" fn hook_object_set_attr(obj_bits: u64, name_bits: u64, value_bits: u64) -> i32 {
    match crate::builtins::attributes::molt_set_attr_name(obj_bits, name_bits, value_bits) {
        0 => 0,
        _ => -1,
    }
}

unsafe extern "C" fn hook_object_format(obj_bits: u64, spec_bits: u64) -> u64 {
    crate::molt_format_builtin(obj_bits, spec_bits)
}

unsafe extern "C" fn hook_sys_get_object_borrowed(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() {
        return 0;
    }
    let name = match std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_data, name_len) })
    {
        Ok(name) => name,
        Err(_) => return 0,
    };
    match name {
        "argv" => crate::molt_sys_argv(),
        "builtin_module_names" => crate::molt_sys_builtin_module_names(),
        "executable" => crate::molt_sys_executable(),
        "flags" => crate::molt_sys_flags_payload(),
        "hexversion" => crate::molt_sys_hexversion(),
        "modules" => crate::molt_sys_modules(),
        "path" => crate::molt_sys_path(),
        "version" => crate::molt_sys_version(),
        "version_info" => crate::molt_sys_version_info(),
        _ => 0,
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
        TYPE_ID_BIGINT => MoltTypeTag::Int as u8,
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
#[derive(Clone, Copy, Eq, PartialEq)]
enum CExtDispatchKind {
    NoArgs,
    OneObject,
    VarArgs,
    VarArgsKeywords,
    FastCall,
    FastCallKeywords,
}

impl CExtDispatchKind {
    fn from_flags(flags: i32) -> Option<Self> {
        let conv_flags = flags & !(METH_CLASS | METH_STATIC | METH_COEXIST);
        if conv_flags & METH_METHOD != 0 {
            return None;
        }
        let fastcall = conv_flags & METH_FASTCALL != 0;
        let keywords = conv_flags & METH_KEYWORDS != 0;
        let varargs = conv_flags & METH_VARARGS != 0;
        if fastcall {
            let allowed = METH_FASTCALL | METH_KEYWORDS;
            if conv_flags & !allowed != 0 {
                return None;
            }
            return Some(if keywords {
                Self::FastCallKeywords
            } else {
                Self::FastCall
            });
        }
        if keywords {
            let allowed = METH_VARARGS | METH_KEYWORDS;
            if conv_flags & !allowed != 0 || !varargs {
                return None;
            }
            return Some(Self::VarArgsKeywords);
        }
        match conv_flags {
            METH_NOARGS => Some(Self::NoArgs),
            METH_O => Some(Self::OneObject),
            METH_VARARGS => Some(Self::VarArgs),
            _ => None,
        }
    }

    fn arity(self) -> u64 {
        match self {
            Self::NoArgs => 0,
            Self::OneObject => 1,
            Self::VarArgs | Self::VarArgsKeywords | Self::FastCall | Self::FastCallKeywords => 0,
        }
    }

    fn is_variadic(self) -> bool {
        !matches!(self, Self::NoArgs | Self::OneObject)
    }
}

#[derive(Clone, Copy)]
struct CExtCallable {
    meth_addr: usize,
    flags: i32,
    self_bits: u64,
    dispatch_kind: CExtDispatchKind,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_prepare_static_extension() -> u64 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    MoltObject::from_bool(true).bits()
}

unsafe fn static_module_def_to_bits(def: *mut PyModuleDef) -> Option<u64> {
    if def.is_null() {
        return None;
    }
    let name = unsafe { (*def).m_name };
    if name.is_null() {
        return None;
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if name_bytes.is_empty() {
        return None;
    }
    let module_bits = unsafe { hook_alloc_module(name_bytes.as_ptr(), name_bytes.len()) };
    let module_ptr = MoltObject::from_bits(module_bits).as_ptr()?;
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return None;
    }
    if !unsafe { register_static_module_methods(def, module_bits) } {
        return None;
    }
    Some(module_bits)
}

unsafe fn register_static_module_methods(def: *mut PyModuleDef, module_bits: u64) -> bool {
    let mut cursor = unsafe { (*def).m_methods };
    if cursor.is_null() {
        return true;
    }
    unsafe {
        while !(*cursor).ml_name.is_null() {
            let entry = &*cursor;
            let Some(method) = entry.ml_meth else {
                return false;
            };
            let name_bytes = CStr::from_ptr(entry.ml_name).to_bytes();
            let func_bits = hook_register_c_function(
                method as *const () as usize as u64,
                entry.ml_flags,
                module_bits,
                name_bytes.as_ptr(),
                name_bytes.len(),
            );
            if func_bits == 0 {
                return false;
            }
            let rc = hook_module_set_attr(
                module_bits,
                name_bytes.as_ptr(),
                name_bytes.len(),
                func_bits,
            );
            hook_dec_ref(func_bits);
            if rc != 0 {
                return false;
            }
            cursor = cursor.add(1);
        }
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_pyinit_module_to_bits(result_pyobj: u64) -> u64 {
    with_gil(|_py| {
        if result_pyobj == 0 {
            return crate::raise_exception::<u64>(
                &_py,
                "ImportError",
                "static extension PyInit returned NULL",
            );
        }
        let module_bits = unsafe {
            molt_cpython_abi::bridge::read_bridge_header_bits(
                result_pyobj as *mut molt_cpython_abi::abi_types::PyObject,
            )
        };
        if let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() {
            unsafe {
                if object_type_id(module_ptr) == TYPE_ID_MODULE {
                    return module_bits;
                }
                return crate::raise_exception::<u64>(
                    &_py,
                    "ImportError",
                    "static extension PyInit returned a non-module object",
                );
            }
        }
        if let Some(module_bits) =
            unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) }
        {
            return module_bits;
        }
        crate::raise_exception::<u64>(
            &_py,
            "ImportError",
            "static extension PyInit returned an invalid module handle",
        )
    })
}

unsafe fn cext_pyobject_from_bits(bits: u64) -> *mut PyObject {
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .lock()
            .handle_to_pyobj(bits)
    }
}

unsafe fn cext_tuple_for_args(args: &[u64]) -> Option<(u64, *mut PyObject)> {
    let tuple_bits = unsafe { hook_alloc_tuple(args.len()) };
    if tuple_bits == 0 {
        return None;
    }
    for (index, &arg_bits) in args.iter().enumerate() {
        unsafe { hook_tuple_set(tuple_bits, index, arg_bits) };
    }
    let tuple_obj = unsafe { cext_pyobject_from_bits(tuple_bits) };
    if tuple_obj.is_null() {
        unsafe { hook_dec_ref(tuple_bits) };
        return None;
    }
    Some((tuple_bits, tuple_obj))
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
    let args = if n == 0 {
        &[][..]
    } else if args_ptr == 0 {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension trampoline received null args pointer",
            )
        }) as i64;
    } else {
        unsafe { std::slice::from_raw_parts(args_ptr as *const u64, n) }
    };

    let mut temp_pyobjects: Vec<*mut PyObject> = Vec::new();
    let mut temp_tuple_bits: Option<u64> = None;
    let self_obj = unsafe { cext_pyobject_from_bits(entry.self_bits) };
    if !self_obj.is_null() {
        temp_pyobjects.push(self_obj);
    }

    let result_pyobj = unsafe {
        match entry.dispatch_kind {
            CExtDispatchKind::NoArgs => {
                if !args.is_empty() {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "TypeError",
                            "METH_NOARGS C extension function takes no arguments",
                        )
                    }) as i64;
                }
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, ptr::null_mut())
            }
            CExtDispatchKind::OneObject => {
                if args.len() != 1 {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "TypeError",
                            "METH_O C extension function takes exactly one argument",
                        )
                    }) as i64;
                }
                let arg = cext_pyobject_from_bits(args[0]);
                temp_pyobjects.push(arg);
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, arg)
            }
            CExtDispatchKind::VarArgs => {
                let Some((tuple_bits, tuple_obj)) = cext_tuple_for_args(args) else {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    }) as i64;
                };
                temp_tuple_bits = Some(tuple_bits);
                temp_pyobjects.push(tuple_obj);
                let f: PyCFunction = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj)
            }
            CExtDispatchKind::VarArgsKeywords => {
                let Some((tuple_bits, tuple_obj)) = cext_tuple_for_args(args) else {
                    return with_gil(|_py| {
                        crate::raise_exception::<i64>(
                            &_py,
                            "MemoryError",
                            "failed to allocate C extension args tuple",
                        )
                    }) as i64;
                };
                temp_tuple_bits = Some(tuple_bits);
                temp_pyobjects.push(tuple_obj);
                let f: PyCFunctionWithKeywords = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, tuple_obj, ptr::null_mut())
            }
            CExtDispatchKind::FastCall => {
                let mut fast_args = Vec::with_capacity(args.len());
                for &arg_bits in args {
                    let arg = cext_pyobject_from_bits(arg_bits);
                    temp_pyobjects.push(arg);
                    fast_args.push(arg);
                }
                let fast_ptr = if fast_args.is_empty() {
                    ptr::null_mut()
                } else {
                    fast_args.as_mut_ptr()
                };
                let f: PyCFunctionFast = std::mem::transmute(entry.meth_addr as *const ());
                f(self_obj, fast_ptr, fast_args.len() as Py_ssize_t)
            }
            CExtDispatchKind::FastCallKeywords => {
                let mut fast_args = Vec::with_capacity(args.len());
                for &arg_bits in args {
                    let arg = cext_pyobject_from_bits(arg_bits);
                    temp_pyobjects.push(arg);
                    fast_args.push(arg);
                }
                let fast_ptr = if fast_args.is_empty() {
                    ptr::null_mut()
                } else {
                    fast_args.as_mut_ptr()
                };
                let f: PyCFunctionFastWithKeywords =
                    std::mem::transmute(entry.meth_addr as *const ());
                f(
                    self_obj,
                    fast_ptr,
                    fast_args.len() as Py_ssize_t,
                    ptr::null_mut(),
                )
            }
        }
    };

    let result_bits = if result_pyobj.is_null() {
        None
    } else {
        Some(unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(result_pyobj) })
    };
    for temp in temp_pyobjects {
        unsafe { molt_cpython_abi::api::refcount::Py_XDECREF(temp) };
    }
    if let Some(tuple_bits) = temp_tuple_bits {
        unsafe { hook_dec_ref(tuple_bits) };
    }
    match result_bits {
        Some(bits) => bits as i64,
        None => with_gil(|_py| {
            let msg = format!(
                "C extension function returned NULL for convention flags 0x{:x}",
                entry.flags
            );
            crate::raise_exception::<i64>(&_py, "RuntimeError", &msg)
        }) as i64,
    }
}

unsafe extern "C" fn hook_register_c_function(
    meth_addr: u64,
    flags: std::os::raw::c_int,
    self_bits: u64,
    name_data: *const u8,
    name_len: usize,
) -> u64 {
    if meth_addr == 0 || name_data.is_null() {
        return 0;
    }
    let Some(dispatch_kind) = CExtDispatchKind::from_flags(flags) else {
        return 0;
    };
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
                self_bits,
                dispatch_kind,
            });
            id
        };
        let closure_bits = MoltObject::from_int(id as i64).bits();
        // Allocate a Molt function object. fn_ptr is unused on the trampoline
        // path but must be non-zero so Molt's dispatcher does not try the
        // runtime-callable shortcut.
        //
        // We use the trampoline address itself as the fn_ptr placeholder.
        // It is never invoked directly because trampoline_ptr is set, and
        // Molt's call_func_dispatch consults trampoline_ptr first.
        let fn_ptr_value = cext_call_trampoline as *const () as usize as u64;
        let func_ptr = alloc_function_obj(&_py, fn_ptr_value, dispatch_kind.arity());
        if func_ptr.is_null() {
            return 0;
        }
        unsafe {
            function_set_call_target_ptr(func_ptr, cext_call_trampoline as *const ());
            function_set_trampoline_ptr(
                func_ptr,
                cext_call_trampoline as *const () as usize as u64,
            );
            if dispatch_kind.is_variadic() {
                (*header_from_obj_ptr(func_ptr)).flags |= HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE;
            }

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
        int_from_i64: hook_int_from_i64,
        int_from_u64: hook_int_from_u64,
        int_as_i64: hook_int_as_i64,
        int_as_i64_checked: hook_int_as_i64_checked,
        int_as_u64_checked: hook_int_as_u64_checked,
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
        dict_del: hook_dict_del,
        dict_len: hook_dict_len,
        str_data: hook_str_data,
        bytes_data: hook_bytes_data,
        buffer_acquire: hook_buffer_acquire,
        buffer_release: hook_buffer_release,
        object_get_attr: hook_object_get_attr,
        object_set_attr: hook_object_set_attr,
        object_format: hook_object_format,
        sys_get_object_borrowed: hook_sys_get_object_borrowed,
        classify_heap: hook_classify_heap,
        inc_ref: hook_inc_ref,
        dec_ref: hook_dec_ref,
        alloc_module: hook_alloc_module,
        module_set_attr: hook_module_set_attr,
        register_c_function: hook_register_c_function,
    };
    // SAFETY: all fn pointers are valid for the process lifetime.
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{PyModuleDef_Base, PyObject};

    #[test]
    fn pyinit_module_to_bits_accepts_static_module_def_pointer() {
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"static_def_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits =
            molt_cpython_abi_pyinit_module_to_bits((&mut def as *mut PyModuleDef) as usize as u64);
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("PyModuleDef pointer must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
    }
}
