//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;

use molt_cpython_abi::abi_types::{
    METH_CLASS, METH_COEXIST, METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, METH_O,
    METH_STATIC, METH_VARARGS, MoltTypeTag, Py_ssize_t, PyCFunction, PyCFunctionFast,
    PyCFunctionFastWithKeywords, PyCFunctionWithKeywords, PyModule_Type, PyModuleDef,
    PyModuleDef_Type, PyObject, PyTypeObject,
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

fn abi_buffer_view_from_runtime(view: crate::MoltBufferView) -> AbiMoltBufferView {
    unsafe { std::mem::transmute::<crate::MoltBufferView, AbiMoltBufferView>(view) }
}

fn runtime_buffer_view_from_abi(view: AbiMoltBufferView) -> crate::MoltBufferView {
    unsafe { std::mem::transmute::<AbiMoltBufferView, crate::MoltBufferView>(view) }
}

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
    let mut view = crate::MoltBufferView::default();
    let rc = unsafe { crate::c_api::molt_buffer_acquire(bits, &mut view as *mut _) };
    if rc != 0 {
        return rc;
    }
    unsafe {
        *out_view = abi_buffer_view_from_runtime(view);
    }
    0
}

unsafe extern "C" fn hook_buffer_release(view: *mut AbiMoltBufferView) -> i32 {
    if view.is_null() {
        return -1;
    }
    let mut runtime_view = unsafe { runtime_buffer_view_from_abi(*view) };
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

unsafe extern "C" fn hook_module_get_dict(module_bits: u64) -> u64 {
    with_gil(|_py| {
        let module_obj = MoltObject::from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return 0;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return 0;
        }
        unsafe { module_dict_bits(module_ptr) }
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
unsafe extern "C" fn hook_module_capi_register(
    module_bits: u64,
    module_def_ptr: usize,
    module_state_size: u64,
) -> i32 {
    crate::c_api::molt_module_capi_register(module_bits, module_def_ptr, module_state_size)
}

unsafe extern "C" fn hook_module_capi_get_state(module_bits: u64) -> *mut u8 {
    crate::c_api::molt_module_capi_get_state(module_bits)
}

unsafe extern "C" fn hook_module_state_add(module_bits: u64, module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_add(module_bits, module_def_ptr)
}

unsafe extern "C" fn hook_module_state_find(module_def_ptr: usize) -> u64 {
    crate::c_api::molt_module_state_find(module_def_ptr)
}

unsafe extern "C" fn hook_module_state_remove(module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_remove(module_def_ptr)
}

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

#[repr(C)]
struct StaticLinkPyMethodDef {
    ml_name: *const c_char,
    ml_meth: *mut c_void,
    ml_flags: i32,
    ml_doc: *const c_char,
}

#[repr(C)]
struct StaticLinkPyModuleDef {
    m_base: *mut c_void,
    m_name: *const c_char,
    m_doc: *const c_char,
    m_size: Py_ssize_t,
    m_methods: *mut StaticLinkPyMethodDef,
    m_slots: *mut StaticLinkPyModuleDefSlot,
    m_traverse: *mut c_void,
    m_clear: *mut c_void,
    m_free: *mut c_void,
}

#[repr(C)]
struct StaticLinkPyModuleDefSlot {
    slot: c_int,
    value: *mut c_void,
}

const STATIC_PY_MOD_CREATE: c_int = 1;
const STATIC_PY_MOD_EXEC: c_int = 2;
const STATIC_PY_MOD_MULTIPLE_INTERPRETERS: c_int = 3;
const STATIC_PY_MOD_GIL: c_int = 4;

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
    let spec_obj = unsafe { static_module_spec_for_def_name(name_bytes)? };
    let module_obj =
        unsafe { molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(def, spec_obj, 0) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec_obj) };
    if module_obj.is_null() {
        return None;
    }
    let module_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(module_obj) };
    let module_ptr = MoltObject::from_bits(module_bits).as_ptr()?;
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return None;
    }
    Some(module_bits)
}

unsafe fn cext_bytes_from_raw<'a>(data: *const u8, len: u64) -> Result<&'a [u8], &'static str> {
    let len = usize::try_from(len).map_err(|_| "byte length does not fit in usize")?;
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err("byte pointer must not be NULL when length is non-zero");
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

unsafe fn cext_optional_bytes_from_raw<'a>(
    data: *const u8,
    len: u64,
) -> Result<Option<&'a [u8]>, &'static str> {
    if data.is_null() && len == 0 {
        return Ok(None);
    }
    Ok(Some(unsafe { cext_bytes_from_raw(data, len)? }))
}

unsafe fn cext_set_str_attr(
    obj_bits: u64,
    attr_name: &[u8],
    value_bytes: &[u8],
) -> Result<(), &'static str> {
    let value_bits = unsafe { hook_alloc_str(value_bytes.as_ptr(), value_bytes.len()) };
    if value_bits == 0 {
        return Err("failed to allocate C extension function metadata string");
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            obj_bits,
            attr_name.as_ptr(),
            attr_name.len() as u64,
            value_bits,
        )
    };
    unsafe { hook_dec_ref(value_bits) };
    if rc != 0 {
        return Err("failed to attach C extension function metadata");
    }
    Ok(())
}

unsafe fn cext_create_py_cfunction_bits(
    self_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<u64, &'static str> {
    if name_bytes.is_empty() {
        return Err("PyMethodDef name must not be empty");
    }
    if method_addr == 0 {
        return Err("PyMethodDef method pointer must not be NULL");
    }
    let flags = i32::try_from(method_flags).map_err(|_| "PyMethodDef flags do not fit in c_int")?;
    if CExtDispatchKind::from_flags(flags).is_none() {
        return Err("unsupported PyMethodDef flags for CPython ABI bridge");
    }
    let func_bits = unsafe {
        hook_register_c_function(
            method_addr as u64,
            flags,
            self_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
        )
    };
    if func_bits == 0 {
        return Err("failed to register PyMethodDef callback with CPython ABI bridge");
    }
    if let Some(doc_bytes) = doc_bytes
        && unsafe { cext_set_str_attr(func_bits, b"__doc__", doc_bytes) }.is_err()
    {
        unsafe { hook_dec_ref(func_bits) };
        return Err("failed to attach PyMethodDef __doc__");
    }
    Ok(func_bits)
}

unsafe fn cext_attach_module_name(func_bits: u64, module_bits: u64) -> Result<(), &'static str> {
    let module_name_attr = b"__name__";
    let module_name_bits = unsafe {
        crate::c_api::molt_object_getattr_bytes(
            module_bits,
            module_name_attr.as_ptr(),
            module_name_attr.len() as u64,
        )
    };
    if MoltObject::from_bits(module_name_bits).is_none() {
        let _ = crate::molt_exception_clear();
        return Ok(());
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            func_bits,
            b"__module__".as_ptr(),
            b"__module__".len() as u64,
            module_name_bits,
        )
    };
    unsafe { hook_dec_ref(module_name_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef __module__");
    }
    Ok(())
}

unsafe fn cext_add_py_cfunction_to_module(
    module_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<(), &'static str> {
    let func_bits = unsafe {
        cext_create_py_cfunction_bits(
            module_bits,
            name_bytes,
            method_addr,
            method_flags,
            doc_bytes,
        )?
    };
    if let Err(message) = unsafe { cext_attach_module_name(func_bits, module_bits) } {
        unsafe { hook_dec_ref(func_bits) };
        return Err(message);
    }
    let rc = unsafe {
        hook_module_set_attr(
            module_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
            func_bits,
        )
    };
    unsafe { hook_dec_ref(func_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef callback to module");
    }
    Ok(())
}

unsafe fn static_link_module_add_methods(
    module_bits: u64,
    methods: *mut StaticLinkPyMethodDef,
) -> Result<(), &'static str> {
    if methods.is_null() {
        return Ok(());
    }
    let mut cursor = methods;
    unsafe {
        while !(*cursor).ml_name.is_null() {
            let entry = &*cursor;
            if entry.ml_meth.is_null() {
                return Err("PyMethodDef method pointer must not be NULL");
            }
            let name_bytes = CStr::from_ptr(entry.ml_name).to_bytes();
            let doc_bytes = if entry.ml_doc.is_null() {
                None
            } else {
                Some(CStr::from_ptr(entry.ml_doc).to_bytes())
            };
            cext_add_py_cfunction_to_module(
                module_bits,
                name_bytes,
                entry.ml_meth as usize,
                entry.ml_flags as u32,
                doc_bytes,
            )?;
            cursor = cursor.add(1);
        }
    }
    Ok(())
}

unsafe fn static_link_module_exec_slots(
    module_bits: u64,
    slots: *mut StaticLinkPyModuleDefSlot,
    module_name: &str,
) -> Result<(), String> {
    if slots.is_null() {
        return Ok(());
    }
    let mut cursor = slots;
    unsafe {
        while (*cursor).slot != 0 {
            let slot = &*cursor;
            match slot.slot {
                STATIC_PY_MOD_CREATE => {
                    return Err(format!(
                        "{module_name}: static-link PyModuleDef Py_mod_create slot requires module creation bridge"
                    ));
                }
                STATIC_PY_MOD_EXEC => {
                    if slot.value.is_null() {
                        return Err(format!(
                            "{module_name}: static-link PyModuleDef Py_mod_exec slot is NULL"
                        ));
                    }
                    type ExecFn = unsafe extern "C" fn(module: *mut PyObject) -> c_int;
                    let exec: ExecFn = std::mem::transmute(slot.value);
                    let module_obj = cext_pyobject_from_bits(module_bits);
                    if module_obj.is_null() {
                        return Err(format!(
                            "{module_name}: static-link PyModuleDef Py_mod_exec module bridge failed"
                        ));
                    }
                    let rc = exec(module_obj);
                    molt_cpython_abi::api::refcount::Py_DECREF(module_obj);
                    if rc != 0 {
                        let detail = static_pyinit_import_error_message(
                            "static-link PyModuleDef Py_mod_exec slot returned non-zero",
                        );
                        return Err(format!("{module_name}: {detail}"));
                    }
                }
                STATIC_PY_MOD_MULTIPLE_INTERPRETERS | STATIC_PY_MOD_GIL => {}
                _ => {
                    return Err(format!(
                        "{module_name}: unsupported static-link PyModuleDef slot {}",
                        slot.slot
                    ));
                }
            }
            cursor = cursor.add(1);
        }
    }
    Ok(())
}

unsafe fn static_link_module_def_to_bits(
    def: *mut StaticLinkPyModuleDef,
) -> Result<Option<u64>, String> {
    if def.is_null() {
        return Ok(None);
    }
    if unsafe { !(*def).m_base.is_null() } {
        return Ok(None);
    }
    let name = unsafe { (*def).m_name };
    if name.is_null() {
        return Ok(None);
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if name_bytes.is_empty() {
        return Err("static-link PyModuleDef name must not be empty".to_string());
    }
    let module_name = String::from_utf8_lossy(name_bytes).into_owned();
    let module_bits = unsafe { hook_alloc_module(name_bytes.as_ptr(), name_bytes.len()) };
    if module_bits == 0 {
        return Err(format!("{module_name}: module allocation failed"));
    }

    let doc = unsafe { (*def).m_doc };
    if !doc.is_null() {
        let doc_bytes = unsafe { CStr::from_ptr(doc).to_bytes() };
        let doc_bits = unsafe { hook_alloc_str(doc_bytes.as_ptr(), doc_bytes.len()) };
        if doc_bits == 0 {
            unsafe { hook_dec_ref(module_bits) };
            return Err(format!("{module_name}: doc allocation failed"));
        }
        let doc_attr = b"__doc__";
        let set_result = unsafe {
            hook_module_set_attr(module_bits, doc_attr.as_ptr(), doc_attr.len(), doc_bits)
        };
        unsafe { hook_dec_ref(doc_bits) };
        if set_result != 0 {
            unsafe { hook_dec_ref(module_bits) };
            return Err(format!("{module_name}: doc registration failed"));
        }
    }

    let module_state_size = unsafe { (*def).m_size };
    let module_state_size = if module_state_size > 0 {
        module_state_size as u64
    } else {
        0
    };
    let module_def_ptr = def as usize;
    if crate::c_api::molt_module_capi_register(module_bits, module_def_ptr, module_state_size) != 0
    {
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: C-API metadata registration failed"));
    }
    if crate::c_api::molt_module_state_add(module_bits, module_def_ptr) != 0 {
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: module-state registration failed"));
    }

    let methods = unsafe { (*def).m_methods };
    if let Err(message) = unsafe { static_link_module_add_methods(module_bits, methods) } {
        let _ = crate::c_api::molt_module_state_remove(module_def_ptr);
        unsafe { hook_dec_ref(module_bits) };
        return Err(format!("{module_name}: {message}"));
    }

    let slots = unsafe { (*def).m_slots };
    if let Err(message) = unsafe { static_link_module_exec_slots(module_bits, slots, &module_name) }
    {
        let _ = crate::c_api::molt_module_state_remove(module_def_ptr);
        unsafe { hook_dec_ref(module_bits) };
        return Err(message);
    }

    Ok(Some(module_bits))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_py_cfunction_create_bytes(
    self_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> u64 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_create_py_cfunction_bits(
                self_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(bits) => bits,
            Err(message) => crate::raise_exception::<u64>(&_py, "TypeError", message),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_py_cfunction_bytes(
    module_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> i32 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    register_cpython_hooks();
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_add_py_cfunction_to_module(
                module_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(()) => 0,
            Err(message) => crate::raise_exception::<i32>(&_py, "TypeError", message),
        }
    })
}

unsafe fn static_pyinit_registered_bridge_module_bits(
    result_pyobj: *mut PyObject,
) -> Result<Option<u64>, &'static str> {
    let Some(module_bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .lock()
        .pyobj_to_handle(result_pyobj)
    else {
        return Ok(None);
    };
    let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
        return Err("static extension PyInit returned a non-module object");
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err("static extension PyInit returned a non-module object");
    }
    Ok(Some(module_bits))
}

unsafe fn static_pyinit_type_matches(
    result_pyobj: *mut PyObject,
    canonical: *mut PyTypeObject,
    type_name: &[u8],
) -> bool {
    if result_pyobj.is_null() {
        return false;
    }
    let actual = unsafe { (*result_pyobj).ob_type };
    if actual.is_null() {
        return false;
    }
    if std::ptr::eq(actual, canonical) {
        return true;
    }
    let actual_name = unsafe { (*actual).tp_name };
    if actual_name.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(actual_name).to_bytes() == type_name }
}

unsafe fn static_pyinit_is_module_def(result_pyobj: *mut PyObject) -> bool {
    unsafe { static_pyinit_type_matches(result_pyobj, &raw mut PyModuleDef_Type, b"moduledef") }
}

unsafe fn static_pyinit_is_bridge_module_object(result_pyobj: *mut PyObject) -> bool {
    unsafe { static_pyinit_type_matches(result_pyobj, &raw mut PyModule_Type, b"module") }
}

unsafe fn static_module_spec_for_def_name(name_bytes: &[u8]) -> Option<*mut PyObject> {
    let spec_type_name = b"importlib.machinery.ModuleSpec";
    let spec_bits = unsafe { hook_alloc_module(spec_type_name.as_ptr(), spec_type_name.len()) };
    if spec_bits == 0 {
        return None;
    }
    let none_bits = MoltObject::none().bits();
    let initialized = unsafe { static_module_spec_set_str(spec_bits, b"name", name_bytes) }
        && unsafe { static_module_spec_set_bits(spec_bits, b"loader", none_bits) }
        && unsafe { static_module_spec_set_bits(spec_bits, b"origin", none_bits) }
        && unsafe { static_module_spec_set_str(spec_bits, b"parent", b"") }
        && unsafe {
            static_module_spec_set_bits(spec_bits, b"submodule_search_locations", none_bits)
        };
    if !initialized {
        unsafe { hook_dec_ref(spec_bits) };
        return None;
    }
    Some(unsafe { cext_pyobject_from_bits(spec_bits) })
}

unsafe fn static_module_spec_set_str(spec_bits: u64, attr: &[u8], value: &[u8]) -> bool {
    let value_bits = unsafe { hook_alloc_str(value.as_ptr(), value.len()) };
    if value_bits == 0 {
        return false;
    }
    let out = unsafe { static_module_spec_set_bits(spec_bits, attr, value_bits) };
    unsafe { hook_dec_ref(value_bits) };
    out
}

unsafe fn static_module_spec_set_bits(spec_bits: u64, attr: &[u8], value_bits: u64) -> bool {
    unsafe { hook_module_set_attr(spec_bits, attr.as_ptr(), attr.len(), value_bits) == 0 }
}

fn static_pyinit_import_error_message(prefix: &str) -> String {
    if let Some(detail) = molt_cpython_abi::api::errors::take_current_error_message() {
        if detail.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}: {detail}")
        }
    } else {
        prefix.to_string()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_pyinit_module_to_bits(result_pyobj: u64) -> u64 {
    with_gil(|_py| {
        if result_pyobj == 0 {
            let message =
                static_pyinit_import_error_message("static extension PyInit returned NULL");
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        let result_ptr = result_pyobj as *mut PyObject;
        match unsafe { static_pyinit_registered_bridge_module_bits(result_ptr) } {
            Ok(Some(module_bits)) => return module_bits,
            Ok(None) => {}
            Err(message) => {
                return crate::raise_exception::<u64>(&_py, "ImportError", &message);
            }
        }
        match unsafe { static_link_module_def_to_bits(result_pyobj as *mut StaticLinkPyModuleDef) }
        {
            Ok(Some(module_bits)) => return module_bits,
            Ok(None) => {}
            Err(message) => {
                return crate::raise_exception::<u64>(&_py, "ImportError", &message);
            }
        }
        if unsafe { static_pyinit_is_module_def(result_ptr) } {
            if let Some(module_bits) =
                unsafe { static_module_def_to_bits(result_pyobj as *mut PyModuleDef) }
            {
                return module_bits;
            }
            let message = static_pyinit_import_error_message(
                "static extension PyInit returned an invalid module definition",
            );
            return crate::raise_exception::<u64>(&_py, "ImportError", message.as_str());
        }
        if unsafe { static_pyinit_is_bridge_module_object(result_ptr) } {
            let module_bits =
                unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(result_ptr) };
            if let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() {
                unsafe {
                    if object_type_id(module_ptr) == TYPE_ID_MODULE {
                        return module_bits;
                    }
                }
            }
        }
        let message = static_pyinit_import_error_message(
            "static extension PyInit returned an invalid module handle",
        );
        crate::raise_exception::<u64>(&_py, "ImportError", message.as_str())
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
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_cext_call_trampoline(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
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
        let raw_trampoline = molt_cpython_abi_cext_call_trampoline as *const ();
        let fn_ptr_value = crate::builtins::functions::runtime_fn_addr(
            "crate::molt_cpython_abi_cext_call_trampoline",
            raw_trampoline,
        );
        let func_ptr = alloc_function_obj(&_py, fn_ptr_value, dispatch_kind.arity());
        if func_ptr.is_null() {
            return 0;
        }
        unsafe {
            #[cfg(not(target_arch = "wasm32"))]
            function_set_call_target_ptr(func_ptr, raw_trampoline);
            function_set_trampoline_ptr(func_ptr, fn_ptr_value);
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
        module_get_dict: hook_module_get_dict,
        module_set_attr: hook_module_set_attr,
        module_capi_register: hook_module_capi_register,
        module_capi_get_state: hook_module_capi_get_state,
        module_state_add: hook_module_state_add,
        module_state_find: hook_module_state_find,
        module_state_remove: hook_module_state_remove,
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
    use molt_cpython_abi::abi_types::{
        PyExc_RuntimeError, PyModuleDef_Base, PyObject, PyTypeObject,
    };
    use std::os::raw::c_int;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard};

    static STATIC_LINK_EXEC_MODULE_BITS: AtomicU64 = AtomicU64::new(0);

    fn cpython_abi_test_guard() -> StdMutexGuard<'static, ()> {
        static LOCK: StdMutex<()> = StdMutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pending_exception_message_for_assertion() -> String {
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        if MoltObject::from_bits(exc_bits).is_none() {
            return "no pending exception".to_string();
        }
        with_gil(|_py| {
            let message = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .map(|exc_ptr| crate::format_exception_message(&_py, exc_ptr))
                .unwrap_or_else(|| "pending exception handle was not a heap object".to_string());
            dec_ref_bits(&_py, exc_bits);
            message
        })
    }

    #[test]
    fn cpython_abi_buffer_view_layout_matches_runtime_descriptor() {
        let _guard = cpython_abi_test_guard();
        macro_rules! assert_field {
            ($field:ident) => {
                assert_eq!(
                    std::mem::offset_of!(AbiMoltBufferView, $field),
                    std::mem::offset_of!(crate::MoltBufferView, $field),
                    concat!("MoltBufferView field offset drift: ", stringify!($field)),
                );
            };
        }

        assert_eq!(
            std::mem::size_of::<AbiMoltBufferView>(),
            std::mem::size_of::<crate::MoltBufferView>()
        );
        assert_eq!(
            std::mem::align_of::<AbiMoltBufferView>(),
            std::mem::align_of::<crate::MoltBufferView>()
        );
        assert_field!(data);
        assert_field!(len);
        assert_field!(readonly);
        assert_field!(ndim);
        assert_field!(itemsize);
        assert_field!(offset);
        assert_field!(owner);
        assert_field!(base);
        assert_field!(shape);
        assert_field!(strides);
        assert_field!(format);
    }

    #[test]
    fn pyinit_module_to_bits_accepts_static_module_def_pointer() {
        let _guard = cpython_abi_test_guard();
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

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("PyModuleDef pointer must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
    }

    #[test]
    fn pyinit_module_to_bits_accepts_split_wasm_moduledef_type_clone() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut app_moduledef_type: PyTypeObject = unsafe { std::mem::zeroed() };
        app_moduledef_type.tp_name = c"moduledef".as_ptr();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &mut app_moduledef_type,
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"split_wasm_static_def_module".as_ptr(),
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
            .expect("split-WASM PyModuleDef type clone must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
    }

    #[test]
    fn pyinit_module_to_bits_accepts_static_link_compact_module_def_without_methods_or_slots() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_compact_module".as_ptr(),
            m_doc: c"compact static-link module".as_ptr(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("compact static-link PyModuleDef must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            crate::c_api::molt_module_state_find((&mut def as *mut StaticLinkPyModuleDef) as usize),
            bits
        );
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    unsafe extern "C" fn static_link_exec_records_module(module_obj: *mut PyObject) -> c_int {
        if module_obj.is_null() {
            return -1;
        }
        let module_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(module_obj) };
        let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return -1;
        }
        STATIC_LINK_EXEC_MODULE_BITS.store(module_bits, AtomicOrdering::Relaxed);
        0
    }

    #[test]
    fn pyinit_module_to_bits_executes_static_link_py_mod_exec_and_metadata_slots() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        STATIC_LINK_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_records_module as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_MULTIPLE_INTERPRETERS,
                value: 2usize as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_GIL,
                value: 1usize as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_exec_slot_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("static-link Py_mod_exec module must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            STATIC_LINK_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            bits
        );
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    #[test]
    fn pyinit_module_to_bits_rejects_static_link_py_mod_create_slot() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_CREATE,
                value: 1usize as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_create_slot_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let message = with_gil(|_py| {
            let exc_ptr = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .expect("static-link slot bridge gap must raise a pending ImportError");
            let message = crate::format_exception_message(&_py, exc_ptr);
            dec_ref_bits(&_py, exc_bits);
            message
        });
        assert!(message.contains(
            "static-link PyModuleDef Py_mod_create slot requires module creation bridge"
        ));
    }

    unsafe extern "C" fn static_link_exec_sets_runtime_error(_module_obj: *mut PyObject) -> c_int {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_RuntimeError,
                c"missing PyArray dtype bootstrap".as_ptr(),
            );
        }
        -1
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_link_py_mod_exec_pending_error() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut slots = [
            StaticLinkPyModuleDefSlot {
                slot: STATIC_PY_MOD_EXEC,
                value: static_link_exec_sets_runtime_error as *mut c_void,
            },
            StaticLinkPyModuleDefSlot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_exec_error_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("static_link_exec_error_module"));
        assert!(message.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"));
        assert!(message.contains("missing PyArray dtype bootstrap"));
    }

    unsafe extern "C" fn pyobject_bridge_tuple_len_method(
        _self_obj: *mut PyObject,
        args_obj: *mut PyObject,
    ) -> *mut PyObject {
        if args_obj.is_null() {
            return std::ptr::null_mut();
        }
        let args_bits = unsafe { molt_cpython_abi::bridge::read_bridge_header_bits(args_obj) };
        let Some(args_ptr) = MoltObject::from_bits(args_bits).as_ptr() else {
            return std::ptr::null_mut();
        };
        if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
            return std::ptr::null_mut();
        }
        let len = unsafe { tuple_len(args_ptr) };
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(len as std::os::raw::c_long) }
    }

    #[test]
    fn pyinit_module_to_bits_registers_static_link_methods_through_pyobject_bridge() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        let mut methods = [
            StaticLinkPyMethodDef {
                ml_name: c"arg_count".as_ptr(),
                ml_meth: pyobject_bridge_tuple_len_method as *mut c_void,
                ml_flags: METH_VARARGS,
                ml_doc: c"return positional argument count".as_ptr(),
            },
            StaticLinkPyMethodDef {
                ml_name: std::ptr::null(),
                ml_meth: std::ptr::null_mut(),
                ml_flags: 0,
                ml_doc: std::ptr::null(),
            },
        ];
        let mut def = StaticLinkPyModuleDef {
            m_base: std::ptr::null_mut(),
            m_name: c"static_link_pyobject_method_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: methods.as_mut_ptr(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let bits = molt_cpython_abi_pyinit_module_to_bits(
            (&mut def as *mut StaticLinkPyModuleDef) as usize as u64,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("static-link PyMethodDef module must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);

        let method_bits =
            unsafe { crate::c_api::molt_module_get_object_bytes(bits, b"arg_count".as_ptr(), 9) };
        assert!(!MoltObject::from_bits(method_bits).is_none());
        let args_bits = unsafe { hook_alloc_tuple(3) };
        assert_ne!(args_bits, 0);
        unsafe {
            hook_tuple_set(args_bits, 0, MoltObject::from_int(1).bits());
            hook_tuple_set(args_bits, 1, MoltObject::from_int(2).bits());
            hook_tuple_set(args_bits, 2, MoltObject::from_int(3).bits());
        }

        let direct_out_bits = with_gil(|_py| unsafe {
            crate::call::function::call_function_obj_bound_vec(
                &_py,
                method_bits,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            )
        });
        assert_eq!(
            to_i64(MoltObject::from_bits(direct_out_bits)),
            Some(3),
            "direct static-link PyMethodDef trampoline failed: {}",
            pending_exception_message_for_assertion()
        );
        unsafe { hook_dec_ref(direct_out_bits) };

        let out_bits =
            crate::c_api::molt_object_call(method_bits, args_bits, MoltObject::none().bits());

        assert_eq!(
            to_i64(MoltObject::from_bits(out_bits)),
            Some(3),
            "public object-call route for static-link PyMethodDef failed: {}",
            pending_exception_message_for_assertion()
        );

        unsafe {
            hook_dec_ref(out_bits);
            hook_dec_ref(args_bits);
            hook_dec_ref(method_bits);
        }
        assert_eq!(
            crate::c_api::molt_module_state_remove(
                (&mut def as *mut StaticLinkPyModuleDef) as usize
            ),
            0
        );
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_pyinit_error_state() {
        let _guard = cpython_abi_test_guard();
        let _ = molt_cpython_abi_prepare_static_extension();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_RuntimeError,
                c"missing PyArray primitive".as_ptr(),
            );
        }

        let bits = molt_cpython_abi_pyinit_module_to_bits(0);

        assert!(MoltObject::from_bits(bits).is_none());
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let message = with_gil(|_py| {
            let exc_ptr = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .expect("PyInit NULL must raise a pending ImportError");
            let message = crate::format_exception_message(&_py, exc_ptr);
            dec_ref_bits(&_py, exc_bits);
            message
        });
        assert!(message.contains("static extension PyInit returned NULL"));
        assert!(message.contains("missing PyArray primitive"));
    }

    #[test]
    fn pyinit_module_to_bits_reports_invalid_handle_error_state() {
        let _guard = cpython_abi_test_guard();
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
            m_name: std::ptr::null(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                &raw mut PyExc_RuntimeError,
                c"module definition missing name".as_ptr(),
            );
        }

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(pyinit_result as usize as u64);

        assert!(MoltObject::from_bits(bits).is_none());
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        let message = with_gil(|_py| {
            let exc_ptr = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .expect("invalid PyInit handle must raise a pending ImportError");
            let message = crate::format_exception_message(&_py, exc_ptr);
            dec_ref_bits(&_py, exc_bits);
            message
        });
        assert!(message.contains("static extension PyInit returned an invalid module definition"));
        assert!(message.contains("module definition missing name"));
    }
}
