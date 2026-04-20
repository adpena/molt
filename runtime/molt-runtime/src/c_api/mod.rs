// Many Py* functions are kept for tests but their #[no_mangle] export lives in
// molt-lang-cpython-abi.  Suppress the dead-code warnings for those stubs.
#![allow(dead_code, non_snake_case, unused_imports)]

#[cfg(not(target_arch = "wasm32"))]
mod cpython_compat;
mod molt_api;

// Re-export all public C-API symbols so external references remain unchanged.
#[cfg(not(target_arch = "wasm32"))]
pub use cpython_compat::*;
pub use molt_api::*;

use self::molt_api::molt_capi_method_dispatch;
use crate::builtins::exceptions::molt_exception_new_from_class;
use crate::concurrency::gil::{gil_held, release_runtime_gil};
use crate::object::layout::{function_call_target_ptr, function_set_call_target_ptr};
use crate::state::runtime_state::{
    molt_runtime_ensure_gil, molt_runtime_init, molt_runtime_shutdown,
};
use crate::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
/// libmolt C-API surface version.
pub const MOLT_C_API_VERSION: u32 = 1;

/// Opaque object handle used by the libmolt C-API.
pub type MoltHandle = u64;

#[repr(C)]
pub struct MoltBufferView {
    pub data: *mut u8,
    pub len: u64,
    pub readonly: u32,
    pub reserved: u32,
    pub stride: i64,
    pub itemsize: u64,
    pub owner: MoltHandle,
}

#[derive(Default)]
struct CApiModuleMetadata {
    module_def_ptr: usize,
    module_state: Option<Box<[u8]>>,
}

#[derive(Default)]
struct CApiModuleStateRegistry {
    by_def: HashMap<usize, MoltHandle>,
    by_module: HashMap<usize, usize>,
}

static C_API_MODULE_METADATA: OnceLock<Mutex<HashMap<usize, CApiModuleMetadata>>> = OnceLock::new();
static C_API_MODULE_STATE_REGISTRY: OnceLock<Mutex<CApiModuleStateRegistry>> = OnceLock::new();

const C_API_METH_VARARGS: u32 = 0x0001;
const C_API_METH_KEYWORDS: u32 = 0x0002;
const C_API_METH_NOARGS: u32 = 0x0004;
const C_API_METH_O: u32 = 0x0008;
const C_API_METH_VARARGS_KEYWORDS: u32 = C_API_METH_VARARGS | C_API_METH_KEYWORDS;
const C_API_SUPPORTED_METH_FLAGS: u32 =
    C_API_METH_VARARGS | C_API_METH_KEYWORDS | C_API_METH_NOARGS | C_API_METH_O;

#[inline]
fn c_api_module_metadata_registry() -> &'static Mutex<HashMap<usize, CApiModuleMetadata>> {
    C_API_MODULE_METADATA.get_or_init(|| Mutex::new(HashMap::new()))
}

#[inline]
fn c_api_module_state_registry() -> &'static Mutex<CApiModuleStateRegistry> {
    C_API_MODULE_STATE_REGISTRY.get_or_init(|| Mutex::new(CApiModuleStateRegistry::default()))
}

#[inline]
fn none_bits() -> u64 {
    MoltObject::none().bits()
}

#[inline]
fn runtime_error_type_bits(_py: &PyToken<'_>) -> u64 {
    let bits = exception_type_bits_from_name(_py, "RuntimeError");
    if bits == 0 {
        builtin_classes(_py).exception
    } else {
        bits
    }
}

#[inline]
unsafe fn bytes_slice_from_raw<'a>(data: *const u8, len_bits: u64) -> Option<&'a [u8]> {
    let len = usize::try_from(len_bits).ok()?;
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) })
}

#[inline]
unsafe fn handles_slice_from_raw<'a>(
    data: *const MoltHandle,
    len_bits: u64,
) -> Option<&'a [MoltHandle]> {
    let len = usize::try_from(len_bits).ok()?;
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) })
}

#[inline]
fn bool_handle_to_i32(_py: &PyToken<'_>, bits: MoltHandle) -> i32 {
    if exception_pending(_py) {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
        return -1;
    }
    let out = if is_truthy(_py, obj_from_bits(bits)) {
        1
    } else {
        0
    };
    let truthy_error = exception_pending(_py);
    if bits != 0 {
        dec_ref_bits(_py, bits);
    }
    if truthy_error { -1 } else { out }
}

#[inline]
fn set_exception_from_message(_py: &PyToken<'_>, exc_type_bits: u64, message: &[u8]) -> i32 {
    let msg_ptr = alloc_string(_py, message);
    if msg_ptr.is_null() {
        return -1;
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let class_bits = if exc_type_bits == 0 || obj_from_bits(exc_type_bits).is_none() {
        runtime_error_type_bits(_py)
    } else {
        exc_type_bits
    };
    let exc_bits = molt_exception_new_from_class(class_bits, msg_bits);
    dec_ref_bits(_py, msg_bits);
    if obj_from_bits(exc_bits).is_none() {
        return -1;
    }
    let _ = molt_exception_set_last(exc_bits);
    dec_ref_bits(_py, exc_bits);
    if exception_pending(_py) { 0 } else { -1 }
}

#[inline]
fn raise_i32(_py: &PyToken<'_>, kind: &str, message: &str) -> i32 {
    let _ = raise_exception::<u64>(_py, kind, message);
    -1
}

#[inline]
fn require_type_handle(_py: &PyToken<'_>, type_bits: MoltHandle) -> Result<*mut u8, i32> {
    let Some(type_ptr) = obj_from_bits(type_bits).as_ptr() else {
        return Err(raise_i32(_py, "TypeError", "type object expected"));
    };
    unsafe {
        if object_type_id(type_ptr) != TYPE_ID_TYPE {
            return Err(raise_i32(_py, "TypeError", "type object expected"));
        }
    }
    Ok(type_ptr)
}

#[inline]
fn require_module_handle(_py: &PyToken<'_>, module_bits: MoltHandle) -> Result<*mut u8, i32> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        return Err(raise_i32(_py, "TypeError", "module object expected"));
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return Err(raise_i32(_py, "TypeError", "module object expected"));
        }
    }
    Ok(module_ptr)
}

#[inline]
fn require_string_handle(
    _py: &PyToken<'_>,
    value_bits: MoltHandle,
    label: &str,
) -> Result<*mut u8, i32> {
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_i32(_py, "TypeError", &format!("{label} must be str")));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_STRING {
            return Err(raise_i32(_py, "TypeError", &format!("{label} must be str")));
        }
    }
    Ok(value_ptr)
}

#[inline]
fn module_add_object_impl(
    _py: &PyToken<'_>,
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_bits: MoltHandle,
) -> i32 {
    if require_string_handle(_py, name_bits, "module attribute name").is_err() {
        return -1;
    }
    let set_out = molt_module_set_attr(module_bits, name_bits, value_bits);
    if set_out != 0 {
        dec_ref_bits(_py, set_out);
    }
    if exception_pending(_py) { -1 } else { 0 }
}

#[inline]
fn module_get_object_impl(
    _py: &PyToken<'_>,
    module_bits: MoltHandle,
    name_bits: MoltHandle,
) -> MoltHandle {
    if require_string_handle(_py, name_bits, "module attribute name").is_err() {
        return none_bits();
    }
    molt_module_get_attr(module_bits, name_bits)
}

#[inline]
fn module_ptr_key(module_ptr: *mut u8) -> usize {
    module_ptr as usize
}

#[inline]
fn alloc_zeroed_state(_py: &PyToken<'_>, size: usize) -> Result<Box<[u8]>, i32> {
    let mut data: Vec<u8> = Vec::new();
    if data.try_reserve_exact(size).is_err() {
        return Err(raise_i32(_py, "MemoryError", "out of memory"));
    }
    data.resize(size, 0);
    Ok(data.into_boxed_slice())
}

#[inline]
fn c_api_module_state_registry_remove_module(
    _py: &PyToken<'_>,
    module_key: usize,
) -> Option<MoltHandle> {
    let registry = c_api_module_state_registry();
    let mut guard = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let def_key = guard.by_module.remove(&module_key)?;
    guard.by_def.remove(&def_key)
}

#[inline]
fn c_api_module_state_registry_remove_def(_py: &PyToken<'_>, def_key: usize) -> Option<MoltHandle> {
    let registry = c_api_module_state_registry();
    let mut guard = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let bits = guard.by_def.remove(&def_key)?;
    if let Some(module_ptr) = obj_from_bits(bits).as_ptr() {
        guard.by_module.remove(&module_ptr_key(module_ptr));
    }
    Some(bits)
}

#[inline]
fn c_api_module_state_registry_clear(_py: &PyToken<'_>) -> Vec<MoltHandle> {
    let registry = c_api_module_state_registry();
    let mut guard = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let out = guard.by_def.values().copied().collect::<Vec<_>>();
    guard.by_def.clear();
    guard.by_module.clear();
    out
}

pub(crate) fn c_api_module_teardown(_py: &PyToken<'_>) {
    {
        let registry = c_api_module_metadata_registry();
        let mut guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clear();
    }
    for bits in c_api_module_state_registry_clear(_py) {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

pub(crate) fn c_api_module_on_module_teardown(_py: &PyToken<'_>, module_ptr: *mut u8) {
    if module_ptr.is_null() {
        return;
    }
    let module_key = module_ptr_key(module_ptr);
    {
        let registry = c_api_module_metadata_registry();
        let mut guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(&module_key);
    }
    if let Some(bits) = c_api_module_state_registry_remove_module(_py, module_key)
        && let Some(bits_ptr) = obj_from_bits(bits).as_ptr()
        && bits_ptr != module_ptr
    {
        dec_ref_bits(_py, bits);
    }
}

#[inline]
fn c_api_method_flags_supported(flags: u32) -> bool {
    if flags & !C_API_SUPPORTED_METH_FLAGS != 0 {
        return false;
    }
    matches!(
        flags,
        C_API_METH_VARARGS | C_API_METH_VARARGS_KEYWORDS | C_API_METH_NOARGS | C_API_METH_O
    )
}

#[inline]
fn c_api_method_set_attr_bytes(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &'static [u8],
    value_bits: u64,
) -> i32 {
    let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name) else {
        if exception_pending(_py) {
            return -1;
        }
        return raise_i32(
            _py,
            "RuntimeError",
            "failed to intern function attribute name",
        );
    };
    let set_out = molt_set_attr_name(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    if set_out != 0 {
        dec_ref_bits(_py, set_out);
    }
    if exception_pending(_py) { -1 } else { 0 }
}

#[inline]
fn c_api_method_set_name(_py: &PyToken<'_>, func_bits: u64, name_bytes: &[u8]) -> Result<(), i32> {
    let name_ptr = alloc_string(_py, name_bytes);
    if name_ptr.is_null() {
        return Err(-1);
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let rc = c_api_method_set_attr_bytes(_py, func_bits, b"__name__", name_bits);
    dec_ref_bits(_py, name_bits);
    if rc < 0 {
        return Err(-1);
    }
    Ok(())
}

#[inline]
fn c_api_method_set_doc(
    _py: &PyToken<'_>,
    func_bits: u64,
    doc_bytes: Option<&[u8]>,
) -> Result<(), i32> {
    let Some(doc_bytes) = doc_bytes else {
        return Ok(());
    };
    let doc_ptr = alloc_string(_py, doc_bytes);
    if doc_ptr.is_null() {
        return Err(-1);
    }
    let doc_bits = MoltObject::from_ptr(doc_ptr).bits();
    let rc = c_api_method_set_attr_bytes(_py, func_bits, b"__doc__", doc_bits);
    dec_ref_bits(_py, doc_bits);
    if rc < 0 {
        return Err(-1);
    }
    Ok(())
}

#[inline]
fn c_api_method_build_function(
    _py: &PyToken<'_>,
    self_bits: u64,
    name_bytes: &[u8],
    method_target: *const (),
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<u64, i32> {
    let _nursery_guard = crate::object::NurserySuspendGuard::new();
    if method_target.is_null() {
        return Err(raise_i32(
            _py,
            "TypeError",
            "C-API method pointer must not be NULL",
        ));
    }
    if !c_api_method_flags_supported(method_flags) {
        return Err(raise_i32(
            _py,
            "TypeError",
            "unsupported C-API method flags (expected VARARGS, VARARGS|KEYWORDS, NOARGS, or O)",
        ));
    }
    if name_bytes.is_empty() {
        return Err(raise_i32(_py, "TypeError", "method name must not be empty"));
    }

    let callback_func_ptr = alloc_function_obj(_py, 0, 0);
    if callback_func_ptr.is_null() {
        return Err(-1);
    }
    unsafe {
        function_set_call_target_ptr(callback_func_ptr, method_target);
    }
    let callback_bits = MoltObject::from_ptr(callback_func_ptr).bits();
    let flags_bits = MoltObject::from_int(i64::from(method_flags)).bits();
    let closure_ptr = alloc_tuple(_py, &[self_bits, flags_bits, callback_bits]);
    dec_ref_bits(_py, callback_bits);
    if closure_ptr.is_null() {
        return Err(-1);
    }
    let closure_bits = MoltObject::from_ptr(closure_ptr).bits();
    let func_ptr = alloc_function_obj(_py, fn_addr!(molt_capi_method_dispatch), 2);
    if func_ptr.is_null() {
        dec_ref_bits(_py, closure_bits);
        return Err(-1);
    }
    unsafe {
        function_set_call_target_ptr(func_ptr, molt_capi_method_dispatch as *const ());
        function_set_closure_bits(_py, func_ptr, closure_bits);
    }
    dec_ref_bits(_py, closure_bits);
    let func_bits = MoltObject::from_ptr(func_ptr).bits();
    let _ = crate::molt_function_set_builtin(func_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, func_bits);
        return Err(-1);
    }
    if c_api_method_set_attr_bytes(
        _py,
        func_bits,
        b"__molt_bind_kind__",
        MoltObject::from_int(crate::BIND_KIND_CAPI_METHOD).bits(),
    ) < 0
        || c_api_method_set_name(_py, func_bits, name_bytes).is_err()
        || c_api_method_set_doc(_py, func_bits, doc_bytes).is_err()
    {
        dec_ref_bits(_py, func_bits);
        return Err(-1);
    }
    Ok(func_bits)
}

#[inline]
fn c_api_method_decode_callback_target(
    _py: &PyToken<'_>,
    callback_bits: u64,
    label: &str,
) -> Result<*const (), u64> {
    let Some(callback_ptr) = obj_from_bits(callback_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid {label}: callback payload is missing"),
        ));
    };
    unsafe {
        if object_type_id(callback_ptr) != TYPE_ID_FUNCTION {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("invalid {label}: callback payload must be a function object"),
            ));
        }
        let target = function_call_target_ptr(callback_ptr);
        if target.is_null() {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("invalid {label}: callback target is missing"),
            ));
        }
        Ok(target)
    }
}

#[inline]
fn c_api_method_decode_closure(
    _py: &PyToken<'_>,
    closure_bits: u64,
) -> Result<(u64, u32, *const ()), u64> {
    let Some(closure_ptr) = obj_from_bits(closure_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "invalid C-API method closure: missing closure tuple",
        ));
    };
    unsafe {
        if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid C-API method closure: expected tuple",
            ));
        }
        let slots = seq_vec_ref(closure_ptr);
        if slots.len() != 3 {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid C-API method closure: expected 3-tuple",
            ));
        }
        let self_bits = slots[0];
        let flags_i64 = to_i64(obj_from_bits(slots[1])).ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid C-API method closure: flags must be int",
            )
        })?;
        let flags = u32::try_from(flags_i64).map_err(|_| {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid C-API method closure: flags out of range",
            )
        })?;
        if !c_api_method_flags_supported(flags) {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid C-API method closure: unsupported method flags",
            ));
        }
        let callback_target = c_api_method_decode_callback_target(_py, slots[2], "method closure")?;
        Ok((self_bits, flags, callback_target))
    }
}

#[inline]
fn c_api_method_require_tuple(_py: &PyToken<'_>, args_tuple_bits: u64) -> Result<*mut u8, u64> {
    let Some(args_ptr) = obj_from_bits(args_tuple_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "C-API method wrapper expects tuple arguments",
        ));
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "C-API method wrapper expects tuple arguments",
            ));
        }
    }
    Ok(args_ptr)
}

#[inline]
fn c_api_method_kwargs_present(_py: &PyToken<'_>, kwargs_bits: u64) -> Result<bool, u64> {
    if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
        return Ok(false);
    }
    let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "keyword arguments must be dict or None",
        ));
    };
    unsafe {
        if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "keyword arguments must be dict or None",
            ));
        }
        Ok(dict_order(kwargs_ptr).len() >= 2)
    }
}

#[inline]
fn c_api_method_tuple_len(_py: &PyToken<'_>, args_ptr: *mut u8) -> usize {
    unsafe {
        debug_assert!(object_type_id(args_ptr) == TYPE_ID_TUPLE);
        seq_vec_ref(args_ptr).len()
    }
}

#[inline]
fn callargs_builder_for_call(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let mut pos: &[u64] = &[];
    if !obj_from_bits(args_bits).is_none() {
        let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "args must be tuple or list");
        };
        unsafe {
            let type_id = object_type_id(args_ptr);
            if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
                return raise_exception::<u64>(_py, "TypeError", "args must be tuple or list");
            }
            pos = seq_vec_ref(args_ptr);
        }
    }

    let builder_bits = molt_callargs_new(pos.len() as u64, 0);
    if builder_bits == 0 || obj_from_bits(builder_bits).is_none() {
        return none_bits();
    }

    for &val in pos {
        let _ = unsafe { molt_callargs_push_pos(builder_bits, val) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return none_bits();
        }
    }

    if !obj_from_bits(kwargs_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return none_bits();
        }
    }

    molt_call_bind(callable_bits, builder_bits)
}

#[inline]
fn len_bits_to_i64(_py: &PyToken<'_>, len_bits: u64) -> i64 {
    match to_i64(obj_from_bits(len_bits)) {
        Some(v) => v,
        None => {
            let _ =
                raise_exception::<u64>(_py, "OverflowError", "sequence length does not fit in i64");
            -1
        }
    }
}

#[cfg(test)]
mod tests;
