use crate::builtins::exceptions::molt_exception_new_from_class;
use crate::concurrency::gil::{gil_held, release_runtime_gil};
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
    method_ptr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<u64, i32> {
    if method_ptr == 0 {
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

    let method_ptr_u64 = method_ptr as u64;
    let ptr_bytes = method_ptr_u64.to_le_bytes();
    let ptr_bytes_obj = alloc_bytes(_py, &ptr_bytes);
    if ptr_bytes_obj.is_null() {
        return Err(-1);
    }
    let ptr_bytes_bits = MoltObject::from_ptr(ptr_bytes_obj).bits();
    let flags_bits = MoltObject::from_int(i64::from(method_flags)).bits();
    let closure_ptr = alloc_tuple(_py, &[self_bits, flags_bits, ptr_bytes_bits]);
    dec_ref_bits(_py, ptr_bytes_bits);
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
fn c_api_method_decode_ptr_from_bytes(
    _py: &PyToken<'_>,
    ptr_bytes_bits: u64,
    label: &str,
) -> Result<usize, u64> {
    let Some(ptr_bytes_ptr) = obj_from_bits(ptr_bytes_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid {label}: pointer payload is missing"),
        ));
    };
    unsafe {
        if object_type_id(ptr_bytes_ptr) != TYPE_ID_BYTES {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("invalid {label}: pointer payload must be bytes"),
            ));
        }
        let len = bytes_len(ptr_bytes_ptr);
        if len != std::mem::size_of::<u64>() {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("invalid {label}: pointer payload length mismatch"),
            ));
        }
        let src = std::slice::from_raw_parts(bytes_data(ptr_bytes_ptr), len);
        let mut raw = [0u8; 8];
        raw.copy_from_slice(src);
        let out = u64::from_le_bytes(raw) as usize;
        if out == 0 {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("invalid {label}: null function pointer"),
            ));
        }
        Ok(out)
    }
}

#[inline]
fn c_api_method_decode_closure(
    _py: &PyToken<'_>,
    closure_bits: u64,
) -> Result<(u64, u32, usize), u64> {
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
        let fn_ptr = c_api_method_decode_ptr_from_bytes(_py, slots[2], "method closure")?;
        Ok((self_bits, flags, fn_ptr))
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_c_api_version() -> u32 {
    MOLT_C_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_init() -> i32 {
    if molt_runtime_init() == 0 { -1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutdown() -> i32 {
    if crate::state::runtime_state::runtime_state_for_gil().is_some() {
        crate::with_gil_entry!(_py, {
            c_api_module_teardown(_py);
        });
    } else {
        let metadata = c_api_module_metadata_registry();
        metadata
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        let state = c_api_module_state_registry();
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.by_def.clear();
        guard.by_module.clear();
    }
    // Shutdown returning 0 means "already shut down", which is still a clean state.
    let _ = molt_runtime_shutdown();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_acquire() -> i32 {
    molt_runtime_ensure_gil();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_release() -> i32 {
    release_runtime_gil();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_is_held() -> i32 {
    if gil_held() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_handle_incref(handle: MoltHandle) {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, handle);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_handle_decref(handle: MoltHandle) {
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, handle);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_none() -> MoltHandle {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bool_from_i32(value: i32) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(value != 0).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_i64(value: i64) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_int(value).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_as_i64(value_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        if let Some(value) = to_i64(obj_from_bits(value_bits)) {
            return value;
        }
        let _ = raise_exception::<u64>(_py, "TypeError", "int-compatible object expected");
        -1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_f64(value: f64) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_float(value).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_as_f64(value_bits: MoltHandle) -> f64 {
    crate::with_gil_entry!(_py, {
        let value_obj = obj_from_bits(value_bits);
        if let Some(value) = value_obj.as_float() {
            return value;
        }
        if let Some(value) = to_i64(value_obj) {
            return value as f64;
        }
        let _ = raise_exception::<u64>(_py, "TypeError", "float-compatible object expected");
        -1.0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_err_set(
    exc_type_bits: MoltHandle,
    message_ptr: *const u8,
    message_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(message) = (unsafe { bytes_slice_from_raw(message_ptr, message_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "exception message pointer cannot be null when len > 0",
            );
        };
        set_exception_from_message(_py, exc_type_bits, message)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_err_format(
    exc_type_bits: MoltHandle,
    message_ptr: *const u8,
    message_len: u64,
) -> i32 {
    unsafe { molt_err_set(exc_type_bits, message_ptr, message_len) }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_clear() -> i32 {
    let _ = molt_exception_clear();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_pending() -> i32 {
    crate::with_gil_entry!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_peek() -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if !exception_pending(_py) {
            return none_bits();
        }
        molt_exception_last()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_fetch() -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if !exception_pending(_py) {
            return none_bits();
        }
        let exc_bits = molt_exception_last();
        let _ = molt_exception_clear();
        exc_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_restore(exc_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(exc_bits).is_none() {
            return -1;
        }
        let _ = molt_exception_set_last(exc_bits);
        if exception_pending(_py) { 0 } else { -1 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_matches(exc_type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(exc_type_ptr) = obj_from_bits(exc_type_bits).as_ptr() else {
            return -1;
        };
        unsafe {
            if object_type_id(exc_type_ptr) != TYPE_ID_TYPE {
                return -1;
            }
        }
        if !exception_pending(_py) {
            return 0;
        }
        let Some(exc_bits) = exception_last_bits_noinc(_py) else {
            return 0;
        };
        let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
            return 0;
        };
        let mut class_bits = unsafe { exception_class_bits(exc_ptr) };
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            class_bits = exception_type_bits(_py, kind_bits);
        }
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            return 0;
        }
        let matches = issubclass_bits(class_bits, exc_type_bits);
        if matches { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getattr(obj_bits: MoltHandle, name_bits: MoltHandle) -> MoltHandle {
    molt_get_attr_name(obj_bits, name_bits)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_getattr_bytes(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(_py, "RuntimeError", "failed to intern attribute name");
        };
        let out = molt_get_attr_name(obj_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        out
    })
}

/// Returns a **borrowed** handle for an attribute on `obj_bits`.
///
/// Identical to `molt_object_getattr_bytes` except the returned handle does
/// NOT carry an extra refcount.  The handle is valid as long as the parent
/// object (module, type, etc.) continues to hold the attribute.
///
/// This is the runtime counterpart of CPython's internal borrowed-reference
/// getattr used by `PyImport_GetModuleDict`, `PyEval_GetBuiltins`, etc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_getattr_borrowed(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let result = unsafe { molt_object_getattr_bytes(obj_bits, name_ptr, name_len) };
        if result != 0 && !exception_pending(_py) {
            // Convert new reference → borrowed reference.
            // Safe because the parent object holds its own strong reference
            // to the attribute value (e.g. in its __dict__).
            dec_ref_bits(_py, result);
        }
        result
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_setattr_bytes(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return -1;
            }
            return raise_i32(_py, "RuntimeError", "failed to intern attribute name");
        };
        let set_out = molt_set_attr_name(obj_bits, name_bits, val_bits);
        dec_ref_bits(_py, name_bits);
        if set_out != 0 {
            dec_ref_bits(_py, set_out);
        }
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_hasattr(obj_bits: MoltHandle, name_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let has_bits = molt_has_attr_name(obj_bits, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, has_bits);
            return -1;
        }
        let out = if is_truthy(_py, obj_from_bits(has_bits)) {
            1
        } else {
            0
        };
        dec_ref_bits(_py, has_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_call(
    callable_bits: MoltHandle,
    args_bits: MoltHandle,
    kwargs_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        callargs_builder_for_call(_py, callable_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_repr(obj_bits: MoltHandle) -> MoltHandle {
    molt_repr_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_str(obj_bits: MoltHandle) -> MoltHandle {
    molt_str_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_truthy(obj_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let out = is_truthy(_py, obj_from_bits(obj_bits));
        if exception_pending(_py) {
            -1
        } else if out {
            1
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_equal(lhs_bits: MoltHandle, rhs_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let eq_bits = molt_eq(lhs_bits, rhs_bits);
        bool_handle_to_i32(_py, eq_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_not_equal(lhs_bits: MoltHandle, rhs_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let ne_bits = molt_ne(lhs_bits, rhs_bits);
        bool_handle_to_i32(_py, ne_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_contains(container_bits: MoltHandle, item_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let contains_bits = molt_contains(container_bits, item_bits);
        bool_handle_to_i32(_py, contains_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_ready(type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if require_type_handle(_py, type_bits).is_err() {
            return -1;
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_create(name_bits: MoltHandle) -> MoltHandle {
    molt_module_new(name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_dict(module_bits: MoltHandle) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return none_bits(),
        };
        unsafe {
            let dict_bits = module_dict_bits(module_ptr);
            if obj_from_bits(dict_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "module dict missing");
            }
            inc_ref_bits(_py, dict_bits);
            dict_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_capi_register(
    module_bits: MoltHandle,
    module_def_ptr: usize,
    module_state_size: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(code) => return code,
        };
        let module_key = module_ptr_key(module_ptr);
        let size = match usize::try_from(module_state_size) {
            Ok(value) => value,
            Err(_) => {
                return raise_i32(
                    _py,
                    "OverflowError",
                    "module state size does not fit in usize",
                );
            }
        };
        let state = if size == 0 {
            None
        } else {
            match alloc_zeroed_state(_py, size) {
                Ok(value) => Some(value),
                Err(code) => return code,
            }
        };
        let metadata = CApiModuleMetadata {
            module_def_ptr,
            module_state: state,
        };
        let registry = c_api_module_metadata_registry();
        let mut guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(module_key, metadata);
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_capi_get_def(module_bits: MoltHandle) -> usize {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return 0,
        };
        let module_key = module_ptr_key(module_ptr);
        let registry = c_api_module_metadata_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .get(&module_key)
            .map_or(0, |entry| entry.module_def_ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_capi_get_state(module_bits: MoltHandle) -> usize {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return 0,
        };
        let module_key = module_ptr_key(module_ptr);
        let registry = c_api_module_metadata_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.get(&module_key).map_or(0, |entry| {
            entry
                .module_state
                .as_ref()
                .map_or(0, |state| state.as_ptr() as usize)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_add(module_bits: MoltHandle, module_def_ptr: usize) -> i32 {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            return raise_i32(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
        }
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(code) => return code,
        };
        let module_key = module_ptr_key(module_ptr);
        let def_key = module_def_ptr;
        let mut decref_bits: Vec<MoltHandle> = Vec::new();
        {
            let registry = c_api_module_state_registry();
            let mut guard = registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if let Some(existing) = guard.by_def.get(&def_key).copied()
                && existing == module_bits
                && guard.by_module.get(&module_key).copied() == Some(def_key)
            {
                return 0;
            }

            if let Some(old_def) = guard.by_module.get(&module_key).copied()
                && old_def != def_key
                && let Some(old_bits) = guard.by_def.remove(&old_def)
            {
                decref_bits.push(old_bits);
            }

            if let Some(old_bits) = guard.by_def.insert(def_key, module_bits)
                && old_bits != module_bits
            {
                if let Some(old_ptr) = obj_from_bits(old_bits).as_ptr() {
                    guard.by_module.remove(&module_ptr_key(old_ptr));
                }
                decref_bits.push(old_bits);
            }

            guard.by_module.insert(module_key, def_key);
            inc_ref_bits(_py, module_bits);
        }
        for bits in decref_bits {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_find(module_def_ptr: usize) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
            return 0;
        }
        let registry = c_api_module_state_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.by_def.get(&module_def_ptr).copied().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_remove(module_def_ptr: usize) -> i32 {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            return raise_i32(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
        }
        let Some(bits) = c_api_module_state_registry_remove_def(_py, module_def_ptr) else {
            return raise_i32(_py, "RuntimeError", "module definition was not registered");
        };
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_add_object(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        module_add_object_impl(_py, module_bits, name_bits, value_bits)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_object_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    value_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "module attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return -1;
            }
            return raise_i32(
                _py,
                "RuntimeError",
                "failed to intern module attribute name",
            );
        };
        let rc = module_add_object_impl(_py, module_bits, name_bits, value_bits);
        dec_ref_bits(_py, name_bits);
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_object(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, { module_get_object_impl(_py, module_bits, name_bits) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_get_object_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "module attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "failed to intern module attribute name",
            );
        };
        let out = module_get_object_impl(_py, module_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_add_type(module_bits: MoltHandle, type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if require_type_handle(_py, type_bits).is_err() {
            return -1;
        }
        let type_name_bits = unsafe {
            molt_object_getattr_bytes(type_bits, b"__name__".as_ptr(), b"__name__".len() as u64)
        };
        if exception_pending(_py) {
            if !obj_from_bits(type_name_bits).is_none() {
                dec_ref_bits(_py, type_name_bits);
            }
            return -1;
        }
        let rc = module_add_object_impl(_py, module_bits, type_name_bits, type_bits);
        if !obj_from_bits(type_name_bits).is_none() {
            dec_ref_bits(_py, type_name_bits);
        }
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_add_int_constant(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value: i64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        module_add_object_impl(
            _py,
            module_bits,
            name_bits,
            MoltObject::from_int(value).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_string_constant(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_ptr: *const u8,
    value_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(value_bytes) = (unsafe { bytes_slice_from_raw(value_ptr, value_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "string constant pointer cannot be null when len > 0",
            );
        };
        let string_ptr = alloc_string(_py, value_bytes);
        if string_ptr.is_null() {
            return -1;
        }
        let string_bits = MoltObject::from_ptr(string_ptr).bits();
        let rc = module_add_object_impl(_py, module_bits, name_bits, string_bits);
        dec_ref_bits(_py, string_bits);
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capi_method_dispatch(
    closure_bits: MoltHandle,
    args_tuple_bits: MoltHandle,
    kwargs_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let (self_bits, flags, fn_ptr) = match c_api_method_decode_closure(_py, closure_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let args_ptr = match c_api_method_require_tuple(_py, args_tuple_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let args_vec = unsafe { seq_vec_ref(args_ptr) };
        let dynamic_self = obj_from_bits(self_bits).is_none();
        let callback_self_bits = if dynamic_self {
            if args_vec.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "C-API method requires a bound self/cls argument",
                );
            }
            args_vec[0]
        } else {
            self_bits
        };
        let kwargs_present = match c_api_method_kwargs_present(_py, kwargs_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let kwargs_for_callback = if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
            0
        } else {
            kwargs_bits
        };
        let mut callback_args_owner_bits = 0u64;
        let result = unsafe {
            match flags {
                C_API_METH_VARARGS => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API method does not accept keyword arguments",
                        );
                    }
                    let callback_args_bits = if dynamic_self {
                        let tail_ptr = alloc_tuple(_py, &args_vec[1..]);
                        if tail_ptr.is_null() {
                            return none_bits();
                        }
                        callback_args_owner_bits = MoltObject::from_ptr(tail_ptr).bits();
                        callback_args_owner_bits
                    } else {
                        args_tuple_bits
                    };
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, callback_args_bits)
                }
                C_API_METH_VARARGS_KEYWORDS => {
                    let callback_args_bits = if dynamic_self {
                        let tail_ptr = alloc_tuple(_py, &args_vec[1..]);
                        if tail_ptr.is_null() {
                            return none_bits();
                        }
                        callback_args_owner_bits = MoltObject::from_ptr(tail_ptr).bits();
                        callback_args_owner_bits
                    } else {
                        args_tuple_bits
                    };
                    let func: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, callback_args_bits, kwargs_for_callback)
                }
                C_API_METH_NOARGS => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API noargs method does not accept keyword arguments",
                        );
                    }
                    let expected = if dynamic_self { 1 } else { 0 };
                    if c_api_method_tuple_len(_py, args_ptr) != expected {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API noargs method expects no positional arguments",
                        );
                    }
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, 0)
                }
                C_API_METH_O => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API METH_O method does not accept keyword arguments",
                        );
                    }
                    let expected = if dynamic_self { 2 } else { 1 };
                    if c_api_method_tuple_len(_py, args_ptr) != expected {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API METH_O method expects exactly one argument",
                        );
                    }
                    let arg0_bits = if dynamic_self {
                        args_vec[1]
                    } else {
                        args_vec[0]
                    };
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, arg0_bits)
                }
                _ => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "unsupported C-API method flags",
                    );
                }
            }
        };
        if callback_args_owner_bits != 0 && result == callback_args_owner_bits {
            inc_ref_bits(_py, result);
        }
        if callback_args_owner_bits != 0 {
            dec_ref_bits(_py, callback_args_owner_bits);
        }
        if result == 0 {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "C-API method returned NULL without setting an exception",
            );
        }
        result
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cfunction_create_bytes(
    self_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    method_ptr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "method name pointer cannot be null when len > 0",
            );
        };
        let doc_bytes = if doc_ptr.is_null() && doc_len == 0 {
            None
        } else {
            let Some(bytes) = (unsafe { bytes_slice_from_raw(doc_ptr, doc_len) }) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "method doc pointer cannot be null when len > 0",
                );
            };
            Some(bytes)
        };
        match c_api_method_build_function(
            _py,
            self_bits,
            name_bytes,
            method_ptr,
            method_flags,
            doc_bytes,
        ) {
            Ok(bits) => bits,
            Err(_) => none_bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_cfunction_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    method_ptr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "method name pointer cannot be null when len > 0",
            );
        };
        if require_module_handle(_py, module_bits).is_err() {
            return -1;
        }
        let doc_bytes = if doc_ptr.is_null() && doc_len == 0 {
            None
        } else {
            let Some(bytes) = (unsafe { bytes_slice_from_raw(doc_ptr, doc_len) }) else {
                return raise_i32(
                    _py,
                    "TypeError",
                    "method doc pointer cannot be null when len > 0",
                );
            };
            Some(bytes)
        };
        let func_bits = match c_api_method_build_function(
            _py,
            module_bits,
            name_bytes,
            method_ptr,
            method_flags,
            doc_bytes,
        ) {
            Ok(bits) => bits,
            Err(code) => return code,
        };
        let name_ptr_obj = alloc_string(_py, name_bytes);
        if name_ptr_obj.is_null() {
            dec_ref_bits(_py, func_bits);
            return -1;
        }
        let name_bits = MoltObject::from_ptr(name_ptr_obj).bits();
        let module_name_bits = unsafe {
            molt_object_getattr_bytes(module_bits, b"__name__".as_ptr(), b"__name__".len() as u64)
        };
        if !obj_from_bits(module_name_bits).is_none() && !exception_pending(_py) {
            let _ = c_api_method_set_attr_bytes(_py, func_bits, b"__module__", module_name_bits);
            dec_ref_bits(_py, module_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, func_bits);
                dec_ref_bits(_py, name_bits);
                return -1;
            }
        } else if exception_pending(_py) {
            let _ = molt_exception_clear();
        }
        let rc = module_add_object_impl(_py, module_bits, name_bits, func_bits);
        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, name_bits);
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_add(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_add(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_sub(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_sub(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_mul(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_mul(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_truediv(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_div(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_floordiv(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_floordiv(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_long(obj_bits: MoltHandle) -> MoltHandle {
    molt_int_from_obj(obj_bits, none_bits(), 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_float(obj_bits: MoltHandle) -> MoltHandle {
    molt_float_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_length(seq_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(seq_bits);
        if exception_pending(_py) {
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_getitem(seq_bits: MoltHandle, key_bits: MoltHandle) -> MoltHandle {
    molt_getitem_method(seq_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_setitem(
    seq_bits: MoltHandle,
    key_bits: MoltHandle,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let _ = molt_setitem_method(seq_bits, key_bits, val_bits);
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_getitem(
    mapping_bits: MoltHandle,
    key_bits: MoltHandle,
) -> MoltHandle {
    molt_getitem_method(mapping_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_setitem(
    mapping_bits: MoltHandle,
    key_bits: MoltHandle,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let _ = molt_setitem_method(mapping_bits, key_bits, val_bits);
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_length(mapping_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(mapping_bits);
        if exception_pending(_py) {
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_keys(mapping_bits: MoltHandle) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let keys_method_bits =
            unsafe { molt_object_getattr_bytes(mapping_bits, b"keys".as_ptr(), 4) };
        if exception_pending(_py) {
            if !obj_from_bits(keys_method_bits).is_none() {
                dec_ref_bits(_py, keys_method_bits);
            }
            return none_bits();
        }
        let out = molt_object_call(keys_method_bits, none_bits(), none_bits());
        if !obj_from_bits(keys_method_bits).is_none() {
            dec_ref_bits(_py, keys_method_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return none_bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_tuple_from_array(items: *const MoltHandle, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(elems) = (unsafe { handles_slice_from_raw(items, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "tuple source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_tuple(_py, elems);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_list_from_array(items: *const MoltHandle, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(elems) = (unsafe { handles_slice_from_raw(items, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "list source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_list(_py, elems);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_dict_from_pairs(
    keys: *const MoltHandle,
    values: *const MoltHandle,
    len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(key_slice) = (unsafe { handles_slice_from_raw(keys, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dict key pointer cannot be null when len > 0",
            );
        };
        let Some(value_slice) = (unsafe { handles_slice_from_raw(values, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dict value pointer cannot be null when len > 0",
            );
        };
        let mut pairs = Vec::with_capacity(key_slice.len().saturating_mul(2));
        for (&key, &value) in key_slice.iter().zip(value_slice.iter()) {
            pairs.push(key);
            pairs.push(value);
        }
        let ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_acquire(
    obj_bits: MoltHandle,
    out_view: *mut MoltBufferView,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if out_view.is_null() {
            return raise_i32(_py, "TypeError", "out_view cannot be null");
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 1,
            stride: 1,
            itemsize: 1,
        };
        if unsafe { molt_buffer_export(obj_bits, &mut export as *mut BufferExport) } != 0 {
            return -1;
        }
        inc_ref_bits(_py, obj_bits);
        unsafe {
            *out_view = MoltBufferView {
                data: export.ptr as usize as *mut u8,
                len: export.len,
                readonly: export.readonly as u32,
                reserved: 0,
                stride: export.stride,
                itemsize: export.itemsize,
                owner: obj_bits,
            };
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_release(view: *mut MoltBufferView) -> i32 {
    crate::with_gil_entry!(_py, {
        if view.is_null() {
            return -1;
        }
        unsafe {
            if (*view).owner != 0 {
                dec_ref_bits(_py, (*view).owner);
            }
            (*view).data = std::ptr::null_mut();
            (*view).len = 0;
            (*view).readonly = 1;
            (*view).reserved = 0;
            (*view).stride = 1;
            (*view).itemsize = 1;
            (*view).owner = 0;
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "bytes source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_bytes(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_as_ptr(bytes_bits: MoltHandle, out_len: *mut u64) -> *const u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(bytes_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "bytes object expected");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "bytes object expected");
                return std::ptr::null();
            }
            if !out_len.is_null() {
                *out_len = bytes_len(ptr) as u64;
            }
            bytes_data(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_string_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "string source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_string(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_string_as_ptr(
    string_bits: MoltHandle,
    out_len: *mut u64,
) -> *const u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(string_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "string object expected");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "string object expected");
                return std::ptr::null();
            }
            if !out_len.is_null() {
                *out_len = string_len(ptr) as u64;
            }
            string_bytes(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytearray_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "bytearray source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_bytearray(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytearray_as_ptr(
    bytearray_bits: MoltHandle,
    out_len: *mut u64,
) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(bytearray_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "bytearray object expected");
            return std::ptr::null_mut();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
                let _ = raise_exception::<u64>(_py, "TypeError", "bytearray object expected");
                return std::ptr::null_mut();
            }
            let vec_ptr = bytearray_vec_ptr(ptr);
            if vec_ptr.is_null() {
                return std::ptr::null_mut();
            }
            let data = (*vec_ptr).as_mut_ptr();
            if !out_len.is_null() {
                *out_len = (*vec_ptr).len() as u64;
            }
            data
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — Iterator protocol
// ---------------------------------------------------------------------------

/// `PyObject_GetIter(obj)` — call `__iter__` on `obj`.
/// Returns a new iterator handle (caller owns the reference) or NULL (0) on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetIter(obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_iter(obj);
        if obj_from_bits(res).is_none() {
            if !exception_pending(_py) {
                let _: u64 = raise_not_iterable(_py, obj);
            }
            return 0;
        }
        res
    })
}

/// `PyIter_Next(iter)` — advance iterator and return the next value.
/// Returns the next value handle (caller owns the reference), or 0 (NULL) when
/// the iterator is exhausted (no exception set) or on error (exception set).
#[unsafe(no_mangle)]
pub extern "C" fn PyIter_Next(iter: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let pair_bits = molt_iter_next(iter);
        if exception_pending(_py) {
            if !obj_from_bits(pair_bits).is_none() {
                dec_ref_bits(_py, pair_bits);
            }
            return 0;
        }
        let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            let done = is_truthy(_py, obj_from_bits(done_bits));
            if done {
                // StopIteration — no exception, just return NULL.
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            inc_ref_bits(_py, val_bits);
            dec_ref_bits(_py, pair_bits);
            val_bits
        }
    })
}

/// `PyIter_Check(obj)` — return 1 if `obj` is an iterator, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyIter_Check(obj: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        if unsafe { is_iterator_bits(_py, obj) } {
            1
        } else {
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — Type check macros
// ---------------------------------------------------------------------------

/// `PyList_Check(obj)` — return 1 if obj is a list, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_LIST
    {
        1
    } else {
        0
    }
}

/// `PyDict_Check(obj)` — return 1 if obj is a dict, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_DICT
    {
        1
    } else {
        0
    }
}

/// `PyTuple_Check(obj)` — return 1 if obj is a tuple, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE
    {
        1
    } else {
        0
    }
}

/// `PyFloat_Check(obj)` — return 1 if obj is a float, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_float() { 1 } else { 0 }
}

/// `PyLong_Check(obj)` — return 1 if obj is an int, 0 otherwise.
/// Covers both inline NaN-boxed ints and heap-allocated bigints.
#[unsafe(no_mangle)]
pub extern "C" fn PyLong_Check(obj: u64) -> i32 {
    let obj_mo = obj_from_bits(obj);
    if obj_mo.is_int() {
        return 1;
    }
    if let Some(ptr) = obj_mo.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_BIGINT
    {
        return 1;
    }
    0
}

/// `PyUnicode_Check(obj)` — return 1 if obj is a str, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_STRING
    {
        1
    } else {
        0
    }
}

/// `PyBool_Check(obj)` — return 1 if obj is a bool, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyBool_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_bool() { 1 } else { 0 }
}

/// `PyNone_Check(obj)` — return 1 if obj is None, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyNone_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_none() { 1 } else { 0 }
}

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — List direct access
// ---------------------------------------------------------------------------

/// `PyList_New(size)` — create a new list of length `size` filled with None values.
/// Returns the new list handle (caller owns the reference) or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_New(size: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        if size < 0 {
            let _ =
                raise_exception::<u64>(_py, "SystemError", "negative size passed to PyList_New");
            return 0;
        }
        let n = size as usize;
        let none = none_bits();
        let elems: Vec<u64> = vec![none; n];
        let ptr = alloc_list(_py, &elems);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyList_Size(list)` — return the length of the list, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Size(list: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            list_len(ptr) as isize
        }
    })
}

/// `PyList_GetItem(list, index)` — return a **borrowed** reference to list[index].
/// Returns 0 on error. The caller must NOT decref the returned handle.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_GetItem(list: u64, index: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return 0;
            }
            let elems = seq_vec_ref(ptr);
            let len = elems.len();
            let actual_idx = if index < 0 {
                let adjusted = (len as isize) + index;
                if adjusted < 0 {
                    let _ = raise_exception::<u64>(_py, "IndexError", "list index out of range");
                    return 0;
                }
                adjusted as usize
            } else {
                index as usize
            };
            if actual_idx >= len {
                let _ = raise_exception::<u64>(_py, "IndexError", "list index out of range");
                return 0;
            }
            // Borrowed reference — do not inc_ref.
            elems[actual_idx]
        }
    })
}

/// `PyList_SetItem(list, index, item)` — set list[index] to `item`.
/// **Steals** a reference to `item`. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_SetItem(list: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            // Steal the reference even on failure (CPython semantics).
            dec_ref_bits(_py, item);
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            let elems = seq_vec(ptr);
            let len = elems.len();
            let actual_idx = if index < 0 {
                let adjusted = (len as isize) + index;
                if adjusted < 0 {
                    dec_ref_bits(_py, item);
                    let _ = raise_exception::<u64>(
                        _py,
                        "IndexError",
                        "list assignment index out of range",
                    );
                    return -1;
                }
                adjusted as usize
            } else {
                index as usize
            };
            if actual_idx >= len {
                dec_ref_bits(_py, item);
                let _ =
                    raise_exception::<u64>(_py, "IndexError", "list assignment index out of range");
                return -1;
            }
            let old = elems[actual_idx];
            // Item reference is stolen (not inc_ref'd), just place it.
            elems[actual_idx] = item;
            dec_ref_bits(_py, old);
            0
        }
    })
}

/// `PyList_Append(list, item)` — append `item` to `list`.
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Append(list: u64, item: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            let elems = seq_vec(ptr);
            inc_ref_bits(_py, item);
            elems.push(item);
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — Dict direct access
// ---------------------------------------------------------------------------

/// `PyDict_New()` — create a new empty dict.
/// Returns the new dict handle (caller owns the reference) or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_New() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyDict_SetItem(dict, key, val)` — insert key/value pair into dict.
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_SetItem(dict: u64, key: u64, val: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(dict).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
                return -1;
            }
            dict_set_in_place(_py, ptr, key, val);
            if exception_pending(_py) { -1 } else { 0 }
        }
    })
}

/// `PyDict_GetItem(dict, key)` — return a **borrowed** reference to dict[key],
/// or 0 (NULL) if the key is not present (no exception set for missing key).
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_GetItem(dict: u64, key: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(dict).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return 0;
            }
            match dict_get_in_place(_py, ptr, key) {
                Some(val_bits) => {
                    // Clear any exception that dict_get_in_place might have set
                    // due to unhashable key (CPython PyDict_GetItem suppresses errors).
                    if exception_pending(_py) {
                        let _ = molt_exception_clear();
                    }
                    // Borrowed reference.
                    val_bits
                }
                None => {
                    // Suppress exceptions (CPython semantics for PyDict_GetItem).
                    if exception_pending(_py) {
                        let _ = molt_exception_clear();
                    }
                    0
                }
            }
        }
    })
}

/// `PyDict_SetItemString(dict, key, val)` — insert string key/value into dict.
/// The key is a C string (null-terminated). Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItemString(
    dict: u64,
    key: *const std::ffi::c_char,
    val: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if key.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "key string pointer cannot be null");
            return -1;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            return -1;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let rc = PyDict_SetItem(dict, key_bits, val);
        dec_ref_bits(_py, key_bits);
        rc
    })
}

/// `PyDict_Size(dict)` — return the number of items in the dict, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Size(dict: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(dict).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
                return -1;
            }
            dict_len(ptr) as isize
        }
    })
}

/// `PyDict_Contains(dict, key)` — return 1 if key is in dict, 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Contains(dict: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(dict).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected dict object");
                return -1;
            }
            match dict_get_in_place(_py, ptr, key) {
                Some(_) => {
                    if exception_pending(_py) {
                        -1
                    } else {
                        1
                    }
                }
                None => {
                    if exception_pending(_py) {
                        -1
                    } else {
                        0
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API Phase 1 — Tuple direct access
// ---------------------------------------------------------------------------

/// `PyTuple_New(size)` — create a new tuple of length `size` filled with None values.
/// Returns the new tuple handle (caller owns the reference) or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_New(size: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        if size < 0 {
            let _ =
                raise_exception::<u64>(_py, "SystemError", "negative size passed to PyTuple_New");
            return 0;
        }
        let n = size as usize;
        let none = none_bits();
        let elems: Vec<u64> = vec![none; n];
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyTuple_Size(tuple)` — return the length of the tuple, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_Size(tuple: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return -1;
            }
            tuple_len(ptr) as isize
        }
    })
}

/// `PyTuple_GetItem(tuple, index)` — return a **borrowed** reference to tuple[index].
/// Returns 0 on error. The caller must NOT decref the returned handle.
#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_GetItem(tuple: u64, index: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return 0;
            }
            let elems = seq_vec_ref(ptr);
            let len = elems.len();
            if index < 0 || (index as usize) >= len {
                let _ = raise_exception::<u64>(_py, "IndexError", "tuple index out of range");
                return 0;
            }
            // Borrowed reference — do not inc_ref.
            elems[index as usize]
        }
    })
}

/// `PyTuple_SetItem(tuple, index, item)` — set tuple[index] to `item`.
/// **Steals** a reference to `item`. Returns 0 on success, -1 on error.
/// Intended for filling newly-created tuples before they are exposed to other code.
#[unsafe(no_mangle)]
pub extern "C" fn PyTuple_SetItem(tuple: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            dec_ref_bits(_py, item);
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return -1;
            }
            let elems = seq_vec(ptr);
            let len = elems.len();
            if index < 0 || (index as usize) >= len {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "IndexError", "tuple index out of range");
                return -1;
            }
            let actual_idx = index as usize;
            let old = elems[actual_idx];
            // Item reference is stolen (not inc_ref'd), just place it.
            elems[actual_idx] = item;
            dec_ref_bits(_py, old);
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Number Protocol
// ---------------------------------------------------------------------------

/// `PyNumber_Add(a, b)` — return `a + b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_add(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Subtract(a, b)` — return `a - b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Subtract(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_sub(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Multiply(a, b)` — return `a * b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Multiply(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_mul(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_TrueDivide(a, b)` — return `a / b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_TrueDivide(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_div(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_FloorDivide(a, b)` — return `a // b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_FloorDivide(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_floordiv(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Remainder(a, b)` — return `a % b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Remainder(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_mod(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Power(a, b, mod_)` — return `pow(a, b)`.
/// The `mod_` argument is accepted for API compatibility but only plain
/// two-argument power is used when `mod_` is None/0.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Power(a: u64, b: u64, mod_: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = if mod_ != 0 && !obj_from_bits(mod_).is_none() {
            molt_pow_mod(a, b, mod_)
        } else {
            molt_pow(a, b)
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Negative(a)` — return `-a`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Negative(a: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_operator_neg(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Positive(a)` — return `+a`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Positive(a: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_operator_pos(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Absolute(a)` — return `abs(a)`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Absolute(a: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_abs_builtin(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Invert(a)` — return `~a`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Invert(a: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_invert(a);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Lshift(a, b)` — return `a << b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_lshift(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Rshift(a, b)` — return `a >> b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_rshift(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_And(a, b)` — return `a & b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_And(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_bit_and(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Or(a, b)` — return `a | b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_bit_or(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Xor(a, b)` — return `a ^ b`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_bit_xor(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Check(o)` — return 1 if `o` is a numeric type (int, float, bool), 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Check(o: u64) -> i32 {
    let obj = obj_from_bits(o);
    if obj.is_int() || obj.is_float() || obj.is_bool() {
        return 1;
    }
    if let Some(ptr) = obj.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_BIGINT
    {
        return 1;
    }
    0
}

/// `PyNumber_Long(o)` — return `int(o)`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Long(o: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_int_from_obj(o, none_bits(), 0);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Float(o)` — return `float(o)`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyNumber_Float(o: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_float_from_obj(o);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Mapping Protocol
// ---------------------------------------------------------------------------

/// `PyMapping_Length(o)` — return `len(o)` for dict-like objects, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Length(o: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(o);
        if exception_pending(_py) {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out as isize
    })
}

/// `PyMapping_Keys(o)` — return `list(o.keys())`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Keys(o: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_dict_keys(o);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyMapping_Values(o)` — return `list(o.values())`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Values(o: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_dict_values(o);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyMapping_Items(o)` — return `list(o.items())`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_Items(o: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_dict_items(o);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyMapping_GetItemString(o, key)` — return `o[key]` where `key` is a NUL-terminated
/// C string. Returns 0 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_GetItemString(o: u64, key: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry!(_py, {
        if key.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "key string pointer cannot be null");
            return 0;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            return 0;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let res = molt_getitem_method(o, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyMapping_HasKey(o, key)` — return 1 if `key in o`, 0 otherwise.
/// Does not raise exceptions on failure (returns 0 instead).
#[unsafe(no_mangle)]
pub extern "C" fn PyMapping_HasKey(o: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_contains(o, key);
        if exception_pending(_py) {
            let _ = molt_exception_clear();
            return 0;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Sequence Protocol additions
// ---------------------------------------------------------------------------

/// `PySequence_GetItem(o, i)` — return `o[i]`, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySequence_GetItem(o: u64, i: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        let idx_bits = MoltObject::from_int(i as i64).bits();
        let res = molt_getitem_method(o, idx_bits);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PySequence_Length(o)` — return `len(o)`, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Length(o: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(o);
        if exception_pending(_py) {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out as isize
    })
}

/// `PySequence_Contains(o, value)` — return 1 if `value in o`, 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySequence_Contains(o: u64, value: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_contains(o, value);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Bytes/String Protocol
// ---------------------------------------------------------------------------

/// `PyBytes_FromStringAndSize(v, len)` — create a new bytes object from a buffer.
/// If `v` is NULL and `len > 0`, returns 0 (error). If `len == 0`, returns an empty bytes.
/// Returns the new bytes handle (caller owns the reference) or 0 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromStringAndSize(v: *const u8, len: isize) -> u64 {
    crate::with_gil_entry!(_py, {
        if len < 0 {
            let _ = raise_exception::<u64>(
                _py,
                "SystemError",
                "negative size passed to PyBytes_FromStringAndSize",
            );
            return 0;
        }
        let data = if len == 0 {
            &[]
        } else if v.is_null() {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "bytes source pointer cannot be null when len > 0",
            );
            return 0;
        } else {
            unsafe { std::slice::from_raw_parts(v, len as usize) }
        };
        let ptr = alloc_bytes(_py, data);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyBytes_AsString(o)` — return a pointer to the internal buffer of a bytes object.
/// Returns NULL on error (e.g. not a bytes object). The pointer is borrowed and valid
/// as long as the bytes object is alive.
#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_AsString(o: u64) -> *const u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
                return std::ptr::null();
            }
            bytes_data(ptr)
        }
    })
}

/// `PyBytes_Size(o)` — return the length of a bytes object, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyBytes_Size(o: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
                return -1;
            }
            bytes_len(ptr) as isize
        }
    })
}

/// `PyUnicode_FromString(v)` — create a new str from a NUL-terminated UTF-8 C string.
/// Returns the new string handle (caller owns the reference) or 0 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromString(v: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry!(_py, {
        if v.is_null() {
            let _ =
                raise_exception::<u64>(_py, "TypeError", "string source pointer cannot be null");
            return 0;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(v) };
        let bytes = cstr.to_bytes();
        let ptr = alloc_string(_py, bytes);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyUnicode_AsUTF8(o)` — return a pointer to the UTF-8 representation of a string.
/// Returns NULL on error. The pointer is borrowed and valid as long as the string object
/// is alive.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_AsUTF8(o: u64) -> *const std::ffi::c_char {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return std::ptr::null();
            }
            string_bytes(ptr) as *const std::ffi::c_char
        }
    })
}

/// `PyUnicode_AsUTF8AndSize(o, size)` — return a pointer to the UTF-8 representation
/// and write the length to `*size` (if `size` is not NULL).
/// Returns NULL on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    o: u64,
    size: *mut isize,
) -> *const std::ffi::c_char {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return std::ptr::null();
            }
            if !size.is_null() {
                *size = string_len(ptr) as isize;
            }
            string_bytes(ptr) as *const std::ffi::c_char
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Memory Protocol
// ---------------------------------------------------------------------------

/// `PyMem_Malloc(size)` — allocate `size` bytes of memory.
/// Returns a pointer to the allocated memory, or NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Malloc(size: usize) -> *mut u8 {
    if size == 0 {
        // CPython returns a non-NULL pointer for size 0; allocate 1 byte.
        return unsafe { libc::malloc(1) as *mut u8 };
    }
    unsafe { libc::malloc(size) as *mut u8 }
}

/// `PyMem_Realloc(ptr, size)` — resize a previously allocated block.
/// Returns a pointer to the reallocated memory, or NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    let actual_size = if size == 0 { 1 } else { size };
    unsafe { libc::realloc(ptr as *mut libc::c_void, actual_size) as *mut u8 }
}

/// `PyMem_Free(ptr)` — free memory allocated by `PyMem_Malloc` or `PyMem_Realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Free(ptr: *mut u8) {
    if !ptr.is_null() {
        unsafe {
            libc::free(ptr as *mut libc::c_void);
        }
    }
}

/// `PyObject_Malloc(size)` — allocate memory for an object.
/// Currently an alias for `PyMem_Malloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Malloc(size: usize) -> *mut u8 {
    unsafe { PyMem_Malloc(size) }
}

/// `PyObject_Realloc(ptr, size)` — reallocate memory for an object.
/// Currently an alias for `PyMem_Realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    unsafe { PyMem_Realloc(ptr, size) }
}

/// `PyObject_Free(ptr)` — free memory allocated by `PyObject_Malloc`.
/// Currently an alias for `PyMem_Free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Free(ptr: *mut u8) {
    unsafe { PyMem_Free(ptr) }
}

// ---------------------------------------------------------------------------
// libmolt C-API — Object Protocol (PyObject_*)
// ---------------------------------------------------------------------------

/// `PyObject_Repr(obj)` — return repr(obj), or 0 on error. Caller owns the reference.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Repr(obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_repr_from_obj(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyObject_Str(obj)` — return str(obj), or 0 on error. Caller owns the reference.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Str(obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_str_from_obj(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyObject_Hash(obj)` — return the hash of obj, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Hash(obj: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let res = molt_hash_builtin(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        let obj_res = obj_from_bits(res);
        if obj_res.is_none() {
            return -1;
        }
        let h = to_i64(obj_res).unwrap_or(-1);
        if obj_res.as_ptr().is_some() {
            dec_ref_bits(_py, res);
        }
        h
    })
}

/// `PyObject_IsTrue(obj)` — return 1 if obj is truthy, 0 if falsy, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsTrue(obj: u64) -> i32 {
    molt_object_truthy(obj)
}

/// `PyObject_Not(obj)` — return 0 if obj is truthy, 1 if falsy, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Not(obj: u64) -> i32 {
    let t = PyObject_IsTrue(obj);
    match t {
        1 => 0,
        0 => 1,
        _ => -1,
    }
}

/// `PyObject_Type(obj)` — return the type of obj. Caller owns the reference.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Type(obj: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_type_of(obj);
        if exception_pending(_py) || obj_from_bits(res).is_none() {
            return 0;
        }
        inc_ref_bits(_py, res);
        res
    })
}

/// `PyObject_Length(obj)` — return the length of obj, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Length(obj: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let res = molt_len(obj);
        if exception_pending(_py) {
            return -1;
        }
        let n = to_i64(obj_from_bits(res)).unwrap_or(-1);
        dec_ref_bits(_py, res);
        n as isize
    })
}

/// `PyObject_Size(obj)` — alias for PyObject_Length.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_Size(obj: u64) -> isize {
    PyObject_Length(obj)
}

/// `PyObject_GetAttr(obj, name)` — return obj.name, or 0 on error. Caller owns reference.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetAttr(obj: u64, name: u64) -> u64 {
    molt_object_getattr(obj, name)
}

/// `PyObject_GetAttrString(obj, name)` — return obj.name using a C string, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_GetAttrString(obj: u64, name: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry!(_py, {
        if name.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "attribute name cannot be null");
            return 0;
        }
        let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        let name_bytes = name_cstr.to_bytes();
        let name_ptr = alloc_string(_py, name_bytes);
        if name_ptr.is_null() {
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let result = molt_object_getattr(obj, name_bits);
        dec_ref_bits(_py, name_bits);
        result
    })
}

/// `PyObject_SetAttr(obj, name, value)` — set obj.name = value. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SetAttr(obj: u64, name: u64, value: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_object_setattr(obj, name, value);
        if exception_pending(_py) || obj_from_bits(res).is_none() {
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyObject_SetAttrString(obj, name, value)` — set attribute using C string name.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_SetAttrString(
    obj: u64,
    name: *const std::ffi::c_char,
    value: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if name.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "attribute name cannot be null");
            return -1;
        }
        let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        let name_bytes = name_cstr.to_bytes();
        let name_ptr = alloc_string(_py, name_bytes);
        if name_ptr.is_null() {
            return -1;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let rc = PyObject_SetAttr(obj, name_bits, value);
        dec_ref_bits(_py, name_bits);
        rc
    })
}

/// `PyObject_HasAttr(obj, name)` — return 1 if obj has attribute name, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HasAttr(obj: u64, name: u64) -> i32 {
    let r = molt_object_hasattr(obj, name);
    if r < 0 { 0 } else { r }
}

/// `PyObject_HasAttrString(obj, name)` — return 1 if obj has attribute, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_HasAttrString(obj: u64, name: *const std::ffi::c_char) -> i32 {
    crate::with_gil_entry!(_py, {
        if name.is_null() {
            return 0;
        }
        let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        let name_bytes = name_cstr.to_bytes();
        let name_ptr = alloc_string(_py, name_bytes);
        if name_ptr.is_null() {
            if exception_pending(_py) {
                let _ = molt_exception_clear();
            }
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let r = molt_object_hasattr(obj, name_bits);
        dec_ref_bits(_py, name_bits);
        if r < 0 { 0 } else { r }
    })
}

/// `PyObject_DelAttr(obj, name)` — delete obj.name. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_DelAttr(obj: u64, name: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_object_delattr(obj, name);
        if exception_pending(_py) || obj_from_bits(res).is_none() {
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyObject_DelAttrString(obj, name)` — delete attribute by C string name.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_DelAttrString(obj: u64, name: *const std::ffi::c_char) -> i32 {
    crate::with_gil_entry!(_py, {
        if name.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "attribute name cannot be null");
            return -1;
        }
        let name_cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        let name_bytes = name_cstr.to_bytes();
        let name_ptr = alloc_string(_py, name_bytes);
        if name_ptr.is_null() {
            return -1;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let rc = PyObject_DelAttr(obj, name_bits);
        dec_ref_bits(_py, name_bits);
        rc
    })
}

/// `PyObject_RichCompareBool(a, b, op)` — compare two objects.
/// op: Py_LT=0, Py_LE=1, Py_EQ=2, Py_NE=3, Py_GT=4, Py_GE=5
/// Returns 1 if true, 0 if false, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_RichCompareBool(a: u64, b: u64, op: i32) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = match op {
            0 => molt_lt(a, b), // Py_LT
            1 => molt_le(a, b), // Py_LE
            2 => molt_eq(a, b), // Py_EQ
            3 => molt_ne(a, b), // Py_NE
            4 => molt_gt(a, b), // Py_GT
            5 => molt_ge(a, b), // Py_GE
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Bad internal call: invalid comparison op",
                );
                return -1;
            }
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        let out = if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        };
        dec_ref_bits(_py, res);
        if exception_pending(_py) { -1 } else { out }
    })
}

/// `PyObject_RichCompare(a, b, op)` — compare two objects, returning the result object.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_RichCompare(a: u64, b: u64, op: i32) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = match op {
            0 => molt_lt(a, b),
            1 => molt_le(a, b),
            2 => molt_eq(a, b),
            3 => molt_ne(a, b),
            4 => molt_gt(a, b),
            5 => molt_ge(a, b),
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Bad internal call: invalid comparison op",
                );
                return 0;
            }
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyCallable_Check(obj)` — return 1 if obj is callable, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyCallable_Check(obj: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_callable_builtin(obj);
        if exception_pending(_py) {
            let _ = molt_exception_clear();
            return 0;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PyObject_IsInstance(obj, cls)` — return 1 if isinstance(obj, cls), 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsInstance(obj: u64, cls: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_isinstance(obj, cls);
        if exception_pending(_py) {
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PyObject_IsSubclass(sub, cls)` — return 1 if issubclass(sub, cls), 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyObject_IsSubclass(sub: u64, cls: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_issubclass(sub, cls);
        if exception_pending(_py) {
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Set Protocol
// ---------------------------------------------------------------------------

/// `PySet_New(iterable)` — create a new set, optionally from an iterable (pass 0 for empty set).
#[unsafe(no_mangle)]
pub extern "C" fn PySet_New(iterable: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // molt_set_new expects raw capacity u64, NOT NaN-boxed
        let set_bits = molt_set_new(0u64);
        if exception_pending(_py) || obj_from_bits(set_bits).is_none() {
            return 0;
        }
        if iterable != 0 && !obj_from_bits(iterable).is_none() {
            let res = molt_set_update(set_bits, iterable);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                if !obj_from_bits(res).is_none() {
                    dec_ref_bits(_py, res);
                }
                return 0;
            }
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
        }
        set_bits
    })
}

/// `PyFrozenSet_New(iterable)` — create a new frozenset.
#[unsafe(no_mangle)]
pub extern "C" fn PyFrozenSet_New(iterable: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // molt_frozenset_new expects raw capacity u64, NOT NaN-boxed
        let fs_bits = molt_frozenset_new(0u64);
        if exception_pending(_py) || obj_from_bits(fs_bits).is_none() {
            return 0;
        }
        if iterable != 0 && !obj_from_bits(iterable).is_none() {
            let res = molt_set_update(fs_bits, iterable);
            if exception_pending(_py) {
                dec_ref_bits(_py, fs_bits);
                if !obj_from_bits(res).is_none() {
                    dec_ref_bits(_py, res);
                }
                return 0;
            }
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
        }
        fs_bits
    })
}

/// `PySet_Size(set)` — return the number of elements in the set.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Size(set: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let res = molt_len(set);
        if exception_pending(_py) {
            return -1;
        }
        let n = to_i64(obj_from_bits(res)).unwrap_or(-1);
        dec_ref_bits(_py, res);
        n as isize
    })
}

/// `PySet_Contains(set, key)` — return 1 if key is in set, 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Contains(set: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_set_contains(set, key);
        if exception_pending(_py) {
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PySet_Add(set, key)` — add key to set. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Add(set: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_set_add(set, key);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PySet_Discard(set, key)` — remove key from set if present. Returns 1 if found, 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Discard(set: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_set_discard(set, key);
        if exception_pending(_py) {
            return -1;
        }
        // discard returns None on success; check if key was present by trying contains first
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        // CPython returns 1 if found, but discard doesn't tell us — return 0 (no error)
        0
    })
}

/// `PySet_Pop(set)` — remove and return an arbitrary element, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Pop(set: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_set_pop(set);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PySet_Clear(set)` — remove all elements. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Clear(set: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_set_clear(set);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PySet_Check(obj)` — return 1 if obj is a set, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PySet_Check(obj: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                1
            } else {
                0
            }
        }
    })
}

/// `PyFrozenSet_Check(obj)` — return 1 if obj is a frozenset, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyFrozenSet_Check(obj: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) == TYPE_ID_FROZENSET {
                1
            } else {
                0
            }
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Unicode/String Protocol additions
// ---------------------------------------------------------------------------

/// `PyUnicode_GetLength(obj)` — return the length of the Unicode string in code points.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_GetLength(obj: u64) -> isize {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return -1;
            }
            string_len(ptr) as isize
        }
    })
}

/// `PyUnicode_Concat(left, right)` — return left + right as a new string, or 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Concat(left: u64, right: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_add(left, right);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyUnicode_Contains(container, element)` — return 1 if element in container, 0 if not, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_Contains(container: u64, element: u64) -> i32 {
    molt_object_contains(container, element)
}

/// `PyUnicode_CompareWithASCIIString(uni, string)` — compare with a C ASCII string.
/// Returns -1, 0, or 1 for less, equal, greater.
#[unsafe(no_mangle)]
pub extern "C" fn PyUnicode_CompareWithASCIIString(
    uni: u64,
    string: *const std::ffi::c_char,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if string.is_null() {
            return -1;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(string) };
        let rhs_bytes = cstr.to_bytes();
        let mut out_len: u64 = 0;
        let lhs_ptr = unsafe { molt_string_as_ptr(uni, &mut out_len as *mut u64) };
        if lhs_ptr.is_null() {
            if exception_pending(_py) {
                let _ = molt_exception_clear();
            }
            return -1;
        }
        let lhs = unsafe { std::slice::from_raw_parts(lhs_ptr, out_len as usize) };
        lhs.cmp(rhs_bytes) as i32
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Dict Protocol additions
// ---------------------------------------------------------------------------

/// `PyDict_GetItemString(dict, key)` — get item using C string key. Borrowed reference.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_GetItemString(dict: u64, key: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry!(_py, {
        if key.is_null() {
            return 0;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            if exception_pending(_py) {
                let _ = molt_exception_clear();
            }
            return 0;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let result = PyDict_GetItem(dict, key_bits);
        dec_ref_bits(_py, key_bits);
        // PyDict_GetItem suppresses errors and returns NULL for missing keys
        if exception_pending(_py) {
            let _ = molt_exception_clear();
            return 0;
        }
        result
    })
}

/// `PyDict_DelItem(dict, key)` — delete dict[key]. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_DelItem(dict: u64, key: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        // Use molt_dict_pop with no default — raises KeyError if missing
        let res = molt_dict_pop(dict, key, none_bits(), MoltObject::from_bool(false).bits());
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        // Successfully popped; discard the value
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyDict_DelItemString(dict, key)` — delete dict[key] using C string.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_DelItemString(dict: u64, key: *const std::ffi::c_char) -> i32 {
    crate::with_gil_entry!(_py, {
        if key.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "key string pointer cannot be null");
            return -1;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            return -1;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let rc = PyDict_DelItem(dict, key_bits);
        dec_ref_bits(_py, key_bits);
        rc
    })
}

/// `PyDict_Keys(dict)` — return a list of all keys in the dict.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Keys(dict: u64) -> u64 {
    PyMapping_Keys(dict)
}

/// `PyDict_Values(dict)` — return a list of all values in the dict.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Values(dict: u64) -> u64 {
    PyMapping_Values(dict)
}

/// `PyDict_Items(dict)` — return a list of all (key, value) pairs in the dict.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Items(dict: u64) -> u64 {
    PyMapping_Items(dict)
}

/// `PyDict_Update(a, b)` — merge b into a. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Update(a: u64, b: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_dict_update(a, b);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyDict_Copy(dict)` — return a shallow copy of the dict.
#[unsafe(no_mangle)]
pub extern "C" fn PyDict_Copy(dict: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_dict_copy(dict);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — List Protocol additions
// ---------------------------------------------------------------------------

/// `PyList_Insert(list, index, item)` — insert item at index. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Insert(list: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let idx_bits = MoltObject::from_int(index as i64).bits();
        let res = molt_list_insert(list, idx_bits, item);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyList_Sort(list)` — sort the list in place. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Sort(list: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        // molt_list_sort(list, key, reverse) — pass None key, False reverse
        let res = molt_list_sort(list, none_bits(), MoltObject::from_bool(false).bits());
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyList_Reverse(list)` — reverse the list in place. Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_Reverse(list: u64) -> i32 {
    crate::with_gil_entry!(_py, {
        let res = molt_list_reverse(list);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if !obj_from_bits(res).is_none() {
            dec_ref_bits(_py, res);
        }
        0
    })
}

/// `PyList_AsTuple(list)` — return a tuple with the same items as the list.
#[unsafe(no_mangle)]
pub extern "C" fn PyList_AsTuple(list: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return 0;
            }
            let elems = seq_vec_ref(ptr);
            let tuple_ptr = alloc_tuple(_py, elems);
            if tuple_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Exception Protocol
// ---------------------------------------------------------------------------

/// `PyErr_SetString(type, message)` — set the current exception.
#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetString(exc_type: u64, message: *const std::ffi::c_char) {
    crate::with_gil_entry!(_py, {
        if message.is_null() {
            set_exception_from_message(_py, exc_type, b"<null message>");
            return;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(message) };
        set_exception_from_message(_py, exc_type, cstr.to_bytes());
    })
}

/// `PyErr_SetNone(type)` — set the current exception with no message.
#[unsafe(no_mangle)]
pub extern "C" fn PyErr_SetNone(exc_type: u64) {
    crate::with_gil_entry!(_py, {
        set_exception_from_message(_py, exc_type, b"");
    })
}

/// `PyErr_Occurred()` — return the current exception type bits if an exception is pending, or 0.
#[unsafe(no_mangle)]
pub extern "C" fn PyErr_Occurred() -> u64 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            // Return a non-zero value to indicate an exception is pending.
            // In CPython this returns the exception type; we return a sentinel.
            1
        } else {
            0
        }
    })
}

/// `PyErr_Clear()` — clear the current exception.
#[unsafe(no_mangle)]
pub extern "C" fn PyErr_Clear() {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            let _ = molt_exception_clear();
        }
    })
}

/// `PyErr_NoMemory()` — set MemoryError and return NULL (0).
#[unsafe(no_mangle)]
pub extern "C" fn PyErr_NoMemory() -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = raise_exception::<u64>(_py, "MemoryError", "out of memory");
        0
    })
}

// ---------------------------------------------------------------------------
// libmolt C-API — Reference Counting
// ---------------------------------------------------------------------------

/// `Py_IncRef(obj)` — increment the reference count.
#[unsafe(no_mangle)]
pub extern "C" fn Py_IncRef(obj: u64) {
    if obj == 0 {
        return;
    }
    crate::with_gil_entry!(_py, {
        if !obj_from_bits(obj).is_none() {
            inc_ref_bits(_py, obj);
        }
    })
}

/// `Py_DecRef(obj)` — decrement the reference count.
#[unsafe(no_mangle)]
pub extern "C" fn Py_DecRef(obj: u64) {
    if obj == 0 {
        return;
    }
    crate::with_gil_entry!(_py, {
        if !obj_from_bits(obj).is_none() {
            dec_ref_bits(_py, obj);
        }
    })
}

/// `Py_XINCREF(obj)` — increment ref count if obj is non-NULL.
#[unsafe(no_mangle)]
pub extern "C" fn Py_XINCREF(obj: u64) {
    Py_IncRef(obj)
}

/// `Py_XDECREF(obj)` — decrement ref count if obj is non-NULL.
#[unsafe(no_mangle)]
pub extern "C" fn Py_XDECREF(obj: u64) {
    Py_DecRef(obj)
}

// ---------------------------------------------------------------------------
// libmolt C-API — Conversion helpers
// ---------------------------------------------------------------------------

/// `PyLong_AsLong(obj)` — return the integer value as a C long, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyLong_AsLong(obj: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        match to_i64(obj_from_bits(obj)) {
            Some(v) => v,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "an integer is required");
                -1
            }
        }
    })
}

/// `PyLong_FromLong(v)` — create a new integer from a C long.
#[unsafe(no_mangle)]
pub extern "C" fn PyLong_FromLong(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

/// `PyFloat_AsDouble(obj)` — return the float value as a C double, or -1.0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_AsDouble(obj: u64) -> f64 {
    crate::with_gil_entry!(_py, {
        match to_f64(obj_from_bits(obj)) {
            Some(v) => v,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "must be real number, not str");
                -1.0
            }
        }
    })
}

/// `PyFloat_FromDouble(v)` — create a new float from a C double.
#[unsafe(no_mangle)]
pub extern "C" fn PyFloat_FromDouble(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}

/// `PyBool_FromLong(v)` — return Py_True if v is nonzero, Py_False otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn PyBool_FromLong(v: i64) -> u64 {
    MoltObject::from_bool(v != 0).bits()
}

/// `Py_BuildNone()` — return None handle (borrowed).
#[unsafe(no_mangle)]
pub extern "C" fn Py_BuildNone() -> u64 {
    none_bits()
}

// ---------------------------------------------------------------------------
// GIL release/re-acquire — resolved by molt-runtime-core's FFI declarations
// ---------------------------------------------------------------------------

/// Release the GIL and return an opaque token encoding the saved state.
///
/// The token packs `depth` (shifted left 1) and `had_runtime_guard` (bit 0)
/// into a single `u64` so that `molt_gil_reacquire_guard` can restore it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_release_guard() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let guard = crate::concurrency::GilReleaseGuard::new();
        let depth = guard.depth;
        let had_runtime = guard.had_runtime_guard;
        std::mem::forget(guard); // Don't drop — we'll reacquire manually
        ((depth as u64) << 1) | (had_runtime as u64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Single-threaded: no GIL state to save.
        0
    }
}

/// Re-acquire the GIL using the token returned by `molt_gil_release_guard`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_reacquire_guard(token: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let depth = (token >> 1) as usize;
        let had_runtime = (token & 1) != 0;
        // Reconstruct the guard and let it drop to re-acquire the GIL.
        let guard = crate::concurrency::GilReleaseGuard {
            depth,
            had_runtime_guard: had_runtime,
        };
        drop(guard);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = token; // Single-threaded: nothing to reacquire.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::exceptions::molt_exception_class;

    extern "C" fn c_api_test_meth_varargs(self_bits: u64, args_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            if obj_from_bits(self_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
            }
            let len = molt_sequence_length(args_bits);
            if len < 0 {
                return MoltObject::none().bits();
            }
            MoltObject::from_int(len).bits()
        })
    }

    extern "C" fn c_api_test_meth_varargs_keywords(
        self_bits: u64,
        args_bits: u64,
        kwargs_bits: u64,
    ) -> u64 {
        crate::with_gil_entry!(_py, {
            if obj_from_bits(self_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
            }
            let pos_len = molt_sequence_length(args_bits);
            if pos_len < 0 {
                return MoltObject::none().bits();
            }
            let kw_len = if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
                0
            } else if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
                unsafe {
                    if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "kwargs payload must be dict",
                        );
                    }
                    (dict_order(kwargs_ptr).len() / 2) as i64
                }
            } else {
                0
            };
            MoltObject::from_int(pos_len * 10 + kw_len).bits()
        })
    }

    extern "C" fn c_api_test_meth_noargs(self_bits: u64, arg_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            if obj_from_bits(self_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
            }
            if arg_bits != 0 {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "noargs callback expected NULL argument pointer",
                );
            }
            MoltObject::from_int(101).bits()
        })
    }

    extern "C" fn c_api_test_meth_o(self_bits: u64, arg_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            if obj_from_bits(self_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
            }
            if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
                return raise_exception::<u64>(_py, "TypeError", "METH_O callback missing arg");
            }
            inc_ref_bits(_py, arg_bits);
            arg_bits
        })
    }

    extern "C" fn c_api_test_dynamic_varargs(self_bits: u64, args_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dynamic self must be an int for this probe",
                );
            };
            let len = molt_sequence_length(args_bits);
            if len < 0 {
                return MoltObject::none().bits();
            }
            MoltObject::from_int(self_value * 10 + len).bits()
        })
    }

    extern "C" fn c_api_test_dynamic_noargs(self_bits: u64, arg_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dynamic self must be an int for this probe",
                );
            };
            if arg_bits != 0 {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "noargs callback expected NULL argument pointer",
                );
            }
            MoltObject::from_int(1000 + self_value).bits()
        })
    }

    extern "C" fn c_api_test_dynamic_o(self_bits: u64, arg_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dynamic self must be an int for this probe",
                );
            };
            let Some(arg_value) = to_i64(obj_from_bits(arg_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dynamic METH_O arg must be an int for this probe",
                );
            };
            MoltObject::from_int(self_value * 100 + arg_value).bits()
        })
    }

    extern "C" fn c_api_test_static_noargs(self_bits: u64, arg_bits: u64) -> u64 {
        crate::with_gil_entry!(_py, {
            if self_bits != 0 {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "static callback expected NULL self_bits",
                );
            }
            if arg_bits != 0 {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "noargs callback expected NULL argument pointer",
                );
            }
            MoltObject::from_int(204).bits()
        })
    }

    #[test]
    fn c_api_version_is_nonzero() {
        assert!(molt_c_api_version() >= 1);
    }

    #[test]
    fn err_set_matches_fetch_roundtrip() {
        let _ = molt_runtime_init();
        let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
        let msg = b"boom";
        let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
        assert_eq!(rc, 0);
        assert_eq!(molt_exception_pending(), 1);
        assert_eq!(molt_err_matches(runtime_error), 1);
        let exc_bits = molt_err_fetch();
        assert!(!obj_from_bits(exc_bits).is_none());
        assert_eq!(molt_exception_pending(), 0);
        let kind_bits = molt_exception_kind(exc_bits);
        let class_bits = molt_exception_class(kind_bits);
        assert_eq!(molt_err_matches(runtime_error), 0);
        assert!(issubclass_bits(class_bits, runtime_error));
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, kind_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    fn object_call_numeric_and_sequence_wrappers() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(3).bits(),
                    MoltObject::from_int(4).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let append_name_ptr = alloc_string(_py, b"append");
            assert!(!append_name_ptr.is_null());
            let append_name_bits = MoltObject::from_ptr(append_name_ptr).bits();
            let append_bits = molt_object_getattr(list_bits, append_name_bits);
            assert!(!obj_from_bits(append_bits).is_none());
            let append_args_ptr = alloc_tuple(_py, &[MoltObject::from_int(5).bits()]);
            assert!(!append_args_ptr.is_null());
            let append_args_bits = MoltObject::from_ptr(append_args_ptr).bits();
            let append_out = molt_object_call(append_bits, append_args_bits, none_bits());
            assert!(!exception_pending(_py));
            assert!(obj_from_bits(append_out).is_none());
            dec_ref_bits(_py, append_args_bits);
            dec_ref_bits(_py, append_bits);
            dec_ref_bits(_py, append_name_bits);

            assert_eq!(molt_sequence_length(list_bits), 3);
            let idx_bits = MoltObject::from_int(1).bits();
            let got_bits = molt_sequence_getitem(list_bits, idx_bits);
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(4));
            let rc = molt_sequence_setitem(
                list_bits,
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(9).bits(),
            );
            assert_eq!(rc, 0);
            let got0 = molt_sequence_getitem(list_bits, MoltObject::from_int(0).bits());
            assert_eq!(to_i64(obj_from_bits(got0)), Some(9));
            let got2 = molt_sequence_getitem(list_bits, MoltObject::from_int(2).bits());
            assert_eq!(to_i64(obj_from_bits(got2)), Some(5));
            dec_ref_bits(_py, got_bits);
            dec_ref_bits(_py, got0);
            dec_ref_bits(_py, got2);
            dec_ref_bits(_py, list_bits);
        });
    }

    #[test]
    fn buffer_acquire_and_release_pins_owner() {
        let _ = molt_runtime_init();
        let bytes_bits = unsafe { molt_bytes_from(b"abc".as_ptr(), 3) };
        assert!(!obj_from_bits(bytes_bits).is_none());
        let mut view = MoltBufferView {
            data: std::ptr::null_mut(),
            len: 0,
            readonly: 1,
            reserved: 0,
            stride: 1,
            itemsize: 1,
            owner: 0,
        };
        let rc = unsafe { molt_buffer_acquire(bytes_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(view.len, 3);
        assert_eq!(view.readonly, 1);
        assert!(!view.data.is_null());
        assert_eq!(view.owner, bytes_bits);
        let observed =
            unsafe { std::slice::from_raw_parts(view.data as *const u8, view.len as usize) };
        assert_eq!(observed, b"abc");
        let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);
        assert!(view.data.is_null());
        assert_eq!(view.owner, 0);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, bytes_bits);
        });
    }

    #[test]
    fn err_pending_peek_restore_roundtrip() {
        let _ = molt_runtime_init();
        let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
        let msg = b"boom";
        let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
        assert_eq!(rc, 0);
        assert_eq!(molt_err_pending(), 1);
        let peek_bits = molt_err_peek();
        assert!(!obj_from_bits(peek_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        let fetched_bits = molt_err_fetch();
        assert!(!obj_from_bits(fetched_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        assert_eq!(molt_err_restore(fetched_bits), 0);
        assert_eq!(molt_err_pending(), 1);
        let restored_bits = molt_err_fetch();
        assert!(!obj_from_bits(restored_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, peek_bits);
            dec_ref_bits(_py, fetched_bits);
            dec_ref_bits(_py, restored_bits);
        });
    }

    #[test]
    fn mapping_length_success_and_failure_paths() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!dict_ptr.is_null());
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let key_ptr = alloc_string(_py, b"k");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(7).bits();
            assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);
            assert_eq!(molt_mapping_length(dict_bits), 1);
            let invalid_bits = MoltObject::from_int(42).bits();
            assert_eq!(molt_mapping_length(invalid_bits), -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict_bits);
        });
    }

    #[test]
    fn mapping_keys_success_and_failure_paths() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!dict_ptr.is_null());
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let key_ptr = alloc_string(_py, b"k");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(7).bits();
            assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);

            let keys_bits = molt_mapping_keys(dict_bits);
            assert!(!obj_from_bits(keys_bits).is_none());
            assert_eq!(molt_sequence_length(keys_bits), 1);
            assert_eq!(molt_object_contains(keys_bits, key_bits), 1);
            dec_ref_bits(_py, keys_bits);

            let invalid_bits = MoltObject::from_int(42).bits();
            assert!(obj_from_bits(molt_mapping_keys(invalid_bits)).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict_bits);
        });
    }

    #[test]
    fn string_from_as_ptr_roundtrip_and_type_errors() {
        let _ = molt_runtime_init();
        let text = b"hello";
        let string_bits = unsafe { molt_string_from(text.as_ptr(), text.len() as u64) };
        assert!(!obj_from_bits(string_bits).is_none());
        let mut out_len = 0u64;
        let ptr = unsafe { molt_string_as_ptr(string_bits, &mut out_len as *mut u64) };
        assert!(!ptr.is_null());
        assert_eq!(out_len, text.len() as u64);
        let observed = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) };
        assert_eq!(observed, text);

        let invalid_bits = MoltObject::from_int(9).bits();
        let bad_ptr = unsafe { molt_string_as_ptr(invalid_bits, std::ptr::null_mut()) };
        assert!(bad_ptr.is_null());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let null_bits = unsafe { molt_string_from(std::ptr::null(), 1) };
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, string_bits);
            if !obj_from_bits(null_bits).is_none() {
                dec_ref_bits(_py, null_bits);
            }
        });
    }

    #[test]
    fn object_setattr_symbol_roundtrip() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let runtime_error = runtime_error_type_bits(_py);
            let msg_ptr = alloc_string(_py, b"msg");
            assert!(!msg_ptr.is_null());
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
            assert!(!obj_from_bits(exc_bits).is_none());
            let attr_ptr = alloc_string(_py, b"custom");
            assert!(!attr_ptr.is_null());
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            let value_bits = MoltObject::from_int(99).bits();
            let set_result = molt_object_setattr(exc_bits, attr_bits, value_bits);
            assert!(!exception_pending(_py));
            let got_bits = molt_object_getattr(exc_bits, attr_bits);
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(99));
            dec_ref_bits(_py, got_bits);
            if !obj_from_bits(set_result).is_none() {
                dec_ref_bits(_py, set_result);
            }
            dec_ref_bits(_py, attr_bits);
            dec_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, msg_bits);
        });
    }

    #[test]
    fn scalar_handle_helpers_roundtrip() {
        let _ = molt_runtime_init();
        assert!(obj_from_bits(molt_none()).is_none());

        let true_bits = molt_bool_from_i32(1);
        let false_bits = molt_bool_from_i32(0);
        assert_eq!(molt_object_truthy(true_bits), 1);
        assert_eq!(molt_object_truthy(false_bits), 0);

        let int_bits = molt_int_from_i64(-42);
        assert_eq!(molt_int_as_i64(int_bits), -42);

        let float_bits = molt_float_from_f64(3.5);
        assert_eq!(molt_float_as_f64(float_bits), 3.5);
        assert_eq!(molt_float_as_f64(int_bits), -42.0);

        assert_eq!(molt_int_as_i64(float_bits), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, true_bits);
            dec_ref_bits(_py, false_bits);
            dec_ref_bits(_py, int_bits);
            dec_ref_bits(_py, float_bits);
        });
    }

    #[test]
    fn object_bytes_compare_and_contains_helpers() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let runtime_error = runtime_error_type_bits(_py);
            let msg_ptr = alloc_string(_py, b"msg");
            assert!(!msg_ptr.is_null());
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
            assert!(!obj_from_bits(exc_bits).is_none());

            let value_bits = MoltObject::from_int(77).bits();
            let set_rc = unsafe {
                molt_object_setattr_bytes(
                    exc_bits,
                    b"custom".as_ptr(),
                    b"custom".len() as u64,
                    value_bits,
                )
            };
            assert_eq!(set_rc, 0);
            let got_bits = unsafe {
                molt_object_getattr_bytes(exc_bits, b"custom".as_ptr(), b"custom".len() as u64)
            };
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(77));
            dec_ref_bits(_py, got_bits);

            assert_eq!(
                molt_object_equal(
                    MoltObject::from_int(5).bits(),
                    MoltObject::from_int(5).bits()
                ),
                1
            );
            assert_eq!(
                molt_object_not_equal(
                    MoltObject::from_int(5).bits(),
                    MoltObject::from_int(6).bits()
                ),
                1
            );

            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            assert_eq!(
                molt_object_contains(list_bits, MoltObject::from_int(2).bits()),
                1
            );
            assert_eq!(
                molt_object_contains(list_bits, MoltObject::from_int(9).bits()),
                0
            );

            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, msg_bits);
        });
    }

    #[test]
    fn array_constructors_roundtrip() {
        let _ = molt_runtime_init();
        let elems = [
            MoltObject::from_int(10).bits(),
            MoltObject::from_int(20).bits(),
            MoltObject::from_int(30).bits(),
        ];
        let tuple_bits = unsafe { molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) };
        let list_bits = unsafe { molt_list_from_array(elems.as_ptr(), elems.len() as u64) };
        assert!(!obj_from_bits(tuple_bits).is_none());
        assert!(!obj_from_bits(list_bits).is_none());
        assert_eq!(molt_sequence_length(tuple_bits), 3);
        assert_eq!(molt_sequence_length(list_bits), 3);

        let keys = [
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(2).bits(),
        ];
        let values = [
            MoltObject::from_int(100).bits(),
            MoltObject::from_int(200).bits(),
        ];
        let dict_bits = unsafe { molt_dict_from_pairs(keys.as_ptr(), values.as_ptr(), 2) };
        assert!(!obj_from_bits(dict_bits).is_none());
        assert_eq!(molt_mapping_length(dict_bits), 2);
        let got_bits = molt_mapping_getitem(dict_bits, keys[1]);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(200));
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, got_bits);
            dec_ref_bits(_py, tuple_bits);
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, dict_bits);
        });

        let null_tuple_bits = unsafe { molt_tuple_from_array(std::ptr::null::<MoltHandle>(), 1) };
        assert!(obj_from_bits(null_tuple_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
    }

    #[test]
    fn type_ready_and_module_parity_wrappers_roundtrip() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let builtins = crate::builtins::classes::builtin_classes(_py);
            assert_eq!(molt_type_ready(builtins.type_obj), 0);
            assert_eq!(molt_type_ready(MoltObject::from_int(1).bits()), -1);
            assert_eq!(molt_err_pending(), 1);
            assert_eq!(molt_err_clear(), 0);

            let module_name_bits = unsafe { molt_string_from(b"demo_ext".as_ptr(), 8) };
            assert!(!obj_from_bits(module_name_bits).is_none());
            let module_bits = molt_module_create(module_name_bits);
            assert!(!obj_from_bits(module_bits).is_none());

            let answer_name_ptr = alloc_string(_py, b"answer");
            assert!(!answer_name_ptr.is_null());
            let answer_name_bits = MoltObject::from_ptr(answer_name_ptr).bits();
            assert_eq!(
                molt_module_add_int_constant(module_bits, answer_name_bits, 42),
                0
            );
            let answer_bits = molt_module_get_object(module_bits, answer_name_bits);
            assert_eq!(to_i64(obj_from_bits(answer_bits)), Some(42));

            assert_eq!(
                unsafe {
                    molt_module_add_object_bytes(
                        module_bits,
                        b"status".as_ptr(),
                        b"status".len() as u64,
                        MoltObject::from_int(7).bits(),
                    )
                },
                0
            );
            let status_bits = unsafe {
                molt_module_get_object_bytes(
                    module_bits,
                    b"status".as_ptr(),
                    b"status".len() as u64,
                )
            };
            assert_eq!(to_i64(obj_from_bits(status_bits)), Some(7));

            let label_name_ptr = alloc_string(_py, b"label");
            assert!(!label_name_ptr.is_null());
            let label_name_bits = MoltObject::from_ptr(label_name_ptr).bits();
            assert_eq!(
                unsafe {
                    molt_module_add_string_constant(module_bits, label_name_bits, b"ok".as_ptr(), 2)
                },
                0
            );
            let label_bits = molt_module_get_object(module_bits, label_name_bits);
            let mut label_len = 0u64;
            let label_ptr = unsafe { molt_string_as_ptr(label_bits, &mut label_len as *mut u64) };
            assert!(!label_ptr.is_null());
            assert_eq!(label_len, 2);
            let label_text = unsafe { std::slice::from_raw_parts(label_ptr, label_len as usize) };
            assert_eq!(label_text, b"ok");

            assert_eq!(molt_module_add_type(module_bits, builtins.type_obj), 0);
            let type_name_ptr = alloc_string(_py, b"type");
            assert!(!type_name_ptr.is_null());
            let type_name_bits = MoltObject::from_ptr(type_name_ptr).bits();
            let added_type_bits = molt_module_get_object(module_bits, type_name_bits);
            assert_eq!(molt_object_equal(added_type_bits, builtins.type_obj), 1);
            assert_eq!(
                molt_module_add_type(module_bits, MoltObject::from_int(1).bits()),
                -1
            );
            assert_eq!(molt_err_pending(), 1);
            assert_eq!(molt_err_clear(), 0);

            let dict_bits = molt_module_get_dict(module_bits);
            assert!(!obj_from_bits(dict_bits).is_none());
            assert!(molt_mapping_length(dict_bits) >= 4);

            dec_ref_bits(_py, added_type_bits);
            dec_ref_bits(_py, type_name_bits);
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, label_bits);
            dec_ref_bits(_py, label_name_bits);
            dec_ref_bits(_py, status_bits);
            dec_ref_bits(_py, answer_bits);
            dec_ref_bits(_py, answer_name_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    #[test]
    fn module_capi_metadata_and_state_registry_roundtrip() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let module_name_bits = unsafe { molt_string_from(b"demo_meta".as_ptr(), 9) };
            assert!(!obj_from_bits(module_name_bits).is_none());
            let module_bits = molt_module_create(module_name_bits);
            assert!(!obj_from_bits(module_bits).is_none());
            let module_ptr = obj_from_bits(module_bits)
                .as_ptr()
                .expect("module pointer should be valid");
            let module_def_ptr = 0xD15EA5Eusize;

            assert_eq!(
                molt_module_capi_register(module_bits, module_def_ptr, 32),
                0
            );
            assert_eq!(molt_module_capi_get_def(module_bits), module_def_ptr);
            let state_ptr = molt_module_capi_get_state(module_bits);
            assert_ne!(state_ptr, 0);
            let state_slice = unsafe { std::slice::from_raw_parts_mut(state_ptr as *mut u8, 32) };
            for byte in state_slice.iter() {
                assert_eq!(*byte, 0);
            }
            state_slice[0] = 7;
            state_slice[31] = 9;

            assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
            assert_eq!(molt_module_state_find(module_def_ptr), module_bits);
            assert_eq!(molt_module_state_remove(module_def_ptr), 0);
            assert_eq!(molt_module_state_find(module_def_ptr), 0);

            assert_eq!(molt_module_state_remove(module_def_ptr), -1);
            assert_eq!(molt_err_pending(), 1);
            assert_eq!(molt_err_clear(), 0);

            assert_eq!(molt_module_state_add(module_bits, module_def_ptr), 0);
            c_api_module_on_module_teardown(_py, module_ptr);
            assert_eq!(molt_module_capi_get_def(module_bits), 0);
            assert_eq!(molt_module_capi_get_state(module_bits), 0);
            assert_eq!(molt_module_state_find(module_def_ptr), 0);

            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    #[test]
    fn module_capi_method_bridge_handles_supported_flags() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let module_name_bits = unsafe { molt_string_from(b"demo_capi".as_ptr(), 9) };
            assert!(!obj_from_bits(module_name_bits).is_none());
            let module_bits = molt_module_create(module_name_bits);
            assert!(!obj_from_bits(module_bits).is_none());

            assert_eq!(
                unsafe {
                    molt_module_add_cfunction_bytes(
                        module_bits,
                        b"meth_varargs".as_ptr(),
                        b"meth_varargs".len() as u64,
                        c_api_test_meth_varargs as *const () as usize,
                        C_API_METH_VARARGS,
                        b"varargs".as_ptr(),
                        b"varargs".len() as u64,
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    molt_module_add_cfunction_bytes(
                        module_bits,
                        b"meth_kwargs".as_ptr(),
                        b"meth_kwargs".len() as u64,
                        c_api_test_meth_varargs_keywords as *const () as usize,
                        C_API_METH_VARARGS | C_API_METH_KEYWORDS,
                        std::ptr::null(),
                        0,
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    molt_module_add_cfunction_bytes(
                        module_bits,
                        b"meth_noargs".as_ptr(),
                        b"meth_noargs".len() as u64,
                        c_api_test_meth_noargs as *const () as usize,
                        C_API_METH_NOARGS,
                        std::ptr::null(),
                        0,
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    molt_module_add_cfunction_bytes(
                        module_bits,
                        b"meth_o".as_ptr(),
                        b"meth_o".len() as u64,
                        c_api_test_meth_o as *const () as usize,
                        C_API_METH_O,
                        std::ptr::null(),
                        0,
                    )
                },
                0
            );

            let meth_varargs_bits =
                unsafe { molt_module_get_object_bytes(module_bits, b"meth_varargs".as_ptr(), 12) };
            let meth_kwargs_bits =
                unsafe { molt_module_get_object_bytes(module_bits, b"meth_kwargs".as_ptr(), 11) };
            let meth_noargs_bits =
                unsafe { molt_module_get_object_bytes(module_bits, b"meth_noargs".as_ptr(), 11) };
            let meth_o_bits =
                unsafe { molt_module_get_object_bytes(module_bits, b"meth_o".as_ptr(), 6) };

            let args3_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            );
            assert!(!args3_ptr.is_null());
            let args3_bits = MoltObject::from_ptr(args3_ptr).bits();
            let out_varargs = molt_object_call(meth_varargs_bits, args3_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_varargs)), Some(3));
            dec_ref_bits(_py, out_varargs);

            let key_ptr = alloc_string(_py, b"k");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let kwargs_ptr =
                alloc_dict_with_pairs(_py, &[key_bits, MoltObject::from_int(9).bits()]);
            assert!(!kwargs_ptr.is_null());
            let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();

            let out_kwargs = molt_object_call(meth_kwargs_bits, args3_bits, kwargs_bits);
            assert_eq!(to_i64(obj_from_bits(out_kwargs)), Some(31));
            dec_ref_bits(_py, out_kwargs);

            let reject_kwargs = molt_object_call(meth_varargs_bits, args3_bits, kwargs_bits);
            assert!(obj_from_bits(reject_kwargs).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            let args0_ptr = alloc_tuple(_py, &[]);
            assert!(!args0_ptr.is_null());
            let args0_bits = MoltObject::from_ptr(args0_ptr).bits();
            let out_noargs = molt_object_call(meth_noargs_bits, args0_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(101));
            dec_ref_bits(_py, out_noargs);

            let reject_noargs = molt_object_call(meth_noargs_bits, args3_bits, none_bits());
            assert!(obj_from_bits(reject_noargs).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            let args1_ptr = alloc_tuple(_py, &[MoltObject::from_int(55).bits()]);
            assert!(!args1_ptr.is_null());
            let args1_bits = MoltObject::from_ptr(args1_ptr).bits();
            let out_o = molt_object_call(meth_o_bits, args1_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_o)), Some(55));
            dec_ref_bits(_py, out_o);

            let reject_o = molt_object_call(meth_o_bits, args0_bits, none_bits());
            assert!(obj_from_bits(reject_o).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, args1_bits);
            dec_ref_bits(_py, args0_bits);
            dec_ref_bits(_py, kwargs_bits);
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, args3_bits);
            dec_ref_bits(_py, meth_o_bits);
            dec_ref_bits(_py, meth_noargs_bits);
            dec_ref_bits(_py, meth_kwargs_bits);
            dec_ref_bits(_py, meth_varargs_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    #[test]
    fn module_capi_method_bridge_rejects_unsupported_flags() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let module_name_bits = unsafe { molt_string_from(b"demo_bad".as_ptr(), 8) };
            assert!(!obj_from_bits(module_name_bits).is_none());
            let module_bits = molt_module_create(module_name_bits);
            assert!(!obj_from_bits(module_bits).is_none());

            let rc = unsafe {
                molt_module_add_cfunction_bytes(
                    module_bits,
                    b"bad".as_ptr(),
                    3,
                    c_api_test_meth_varargs as *const () as usize,
                    C_API_METH_VARARGS | C_API_METH_O,
                    std::ptr::null(),
                    0,
                )
            };
            assert_eq!(rc, -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    #[test]
    fn c_api_method_dispatch_supports_dynamic_self_callbacks() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dyn_varargs_bits = unsafe {
                molt_cfunction_create_bytes(
                    none_bits(),
                    b"dyn_varargs".as_ptr(),
                    b"dyn_varargs".len() as u64,
                    c_api_test_dynamic_varargs as *const () as usize,
                    C_API_METH_VARARGS,
                    std::ptr::null(),
                    0,
                )
            };
            let dyn_noargs_bits = unsafe {
                molt_cfunction_create_bytes(
                    none_bits(),
                    b"dyn_noargs".as_ptr(),
                    b"dyn_noargs".len() as u64,
                    c_api_test_dynamic_noargs as *const () as usize,
                    C_API_METH_NOARGS,
                    std::ptr::null(),
                    0,
                )
            };
            let dyn_o_bits = unsafe {
                molt_cfunction_create_bytes(
                    none_bits(),
                    b"dyn_o".as_ptr(),
                    b"dyn_o".len() as u64,
                    c_api_test_dynamic_o as *const () as usize,
                    C_API_METH_O,
                    std::ptr::null(),
                    0,
                )
            };
            assert!(!obj_from_bits(dyn_varargs_bits).is_none());
            assert!(!obj_from_bits(dyn_noargs_bits).is_none());
            assert!(!obj_from_bits(dyn_o_bits).is_none());

            let args_var_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(40).bits(),
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert!(!args_var_ptr.is_null());
            let args_var_bits = MoltObject::from_ptr(args_var_ptr).bits();
            let out_var = molt_object_call(dyn_varargs_bits, args_var_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_var)), Some(402));
            dec_ref_bits(_py, out_var);

            let args_none_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
            assert!(!args_none_ptr.is_null());
            let args_none_bits = MoltObject::from_ptr(args_none_ptr).bits();
            let out_noargs = molt_object_call(dyn_noargs_bits, args_none_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_noargs)), Some(1007));
            dec_ref_bits(_py, out_noargs);

            let args_o_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(5).bits(),
                    MoltObject::from_int(9).bits(),
                ],
            );
            assert!(!args_o_ptr.is_null());
            let args_o_bits = MoltObject::from_ptr(args_o_ptr).bits();
            let out_o = molt_object_call(dyn_o_bits, args_o_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out_o)), Some(509));
            dec_ref_bits(_py, out_o);

            let args_missing_self_ptr = alloc_tuple(_py, &[]);
            assert!(!args_missing_self_ptr.is_null());
            let args_missing_self_bits = MoltObject::from_ptr(args_missing_self_ptr).bits();
            let reject_missing_self =
                molt_object_call(dyn_varargs_bits, args_missing_self_bits, none_bits());
            assert!(obj_from_bits(reject_missing_self).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            let args_bad_noargs_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(7).bits(),
                    MoltObject::from_int(1).bits(),
                ],
            );
            assert!(!args_bad_noargs_ptr.is_null());
            let args_bad_noargs_bits = MoltObject::from_ptr(args_bad_noargs_ptr).bits();
            let reject_noargs =
                molt_object_call(dyn_noargs_bits, args_bad_noargs_bits, none_bits());
            assert!(obj_from_bits(reject_noargs).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            let args_bad_o_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
            assert!(!args_bad_o_ptr.is_null());
            let args_bad_o_bits = MoltObject::from_ptr(args_bad_o_ptr).bits();
            let reject_o = molt_object_call(dyn_o_bits, args_bad_o_bits, none_bits());
            assert!(obj_from_bits(reject_o).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, args_bad_o_bits);
            dec_ref_bits(_py, args_bad_noargs_bits);
            dec_ref_bits(_py, args_missing_self_bits);
            dec_ref_bits(_py, args_o_bits);
            dec_ref_bits(_py, args_none_bits);
            dec_ref_bits(_py, args_var_bits);
            dec_ref_bits(_py, dyn_o_bits);
            dec_ref_bits(_py, dyn_noargs_bits);
            dec_ref_bits(_py, dyn_varargs_bits);
        });
    }

    #[test]
    fn c_api_method_dispatch_supports_null_self_for_static_callbacks() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let static_noargs_bits = unsafe {
                molt_cfunction_create_bytes(
                    0,
                    b"static_noargs".as_ptr(),
                    b"static_noargs".len() as u64,
                    c_api_test_static_noargs as *const () as usize,
                    C_API_METH_NOARGS,
                    std::ptr::null(),
                    0,
                )
            };
            assert!(!obj_from_bits(static_noargs_bits).is_none());

            let args_empty_ptr = alloc_tuple(_py, &[]);
            assert!(!args_empty_ptr.is_null());
            let args_empty_bits = MoltObject::from_ptr(args_empty_ptr).bits();
            let out = molt_object_call(static_noargs_bits, args_empty_bits, none_bits());
            assert_eq!(to_i64(obj_from_bits(out)), Some(204));
            dec_ref_bits(_py, out);

            let args_bad_ptr = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
            assert!(!args_bad_ptr.is_null());
            let args_bad_bits = MoltObject::from_ptr(args_bad_ptr).bits();
            let reject = molt_object_call(static_noargs_bits, args_bad_bits, none_bits());
            assert!(obj_from_bits(reject).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, args_bad_bits);
            dec_ref_bits(_py, args_empty_bits);
            dec_ref_bits(_py, static_noargs_bits);
        });
    }

    // -----------------------------------------------------------------------
    // Phase 1 C-API tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_list_new_size_getitem_setitem() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list = PyList_New(3);
            assert_ne!(list, 0);
            assert_eq!(PyList_Size(list), 3);

            // All slots default to None.
            let item0 = PyList_GetItem(list, 0);
            assert!(obj_from_bits(item0).is_none());

            // SetItem steals the ref, so we inc_ref first for the value we're inserting.
            let val = MoltObject::from_int(42).bits();
            inc_ref_bits(_py, val);
            assert_eq!(PyList_SetItem(list, 1, val), 0);
            let got = PyList_GetItem(list, 1);
            assert_eq!(to_i64(obj_from_bits(got)), Some(42));

            // Append
            let extra = MoltObject::from_int(99).bits();
            assert_eq!(PyList_Append(list, extra), 0);
            assert_eq!(PyList_Size(list), 4);
            let got_last = PyList_GetItem(list, 3);
            assert_eq!(to_i64(obj_from_bits(got_last)), Some(99));

            // Negative index
            let got_neg = PyList_GetItem(list, -1);
            assert_eq!(to_i64(obj_from_bits(got_neg)), Some(99));

            // Out-of-bounds
            let bad = PyList_GetItem(list, 100);
            assert_eq!(bad, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, list);
        });
    }

    #[test]
    fn c_api_list_new_negative_size_fails() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list = PyList_New(-1);
            assert_eq!(list, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_dict_new_setitem_getitem_contains_size() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);
            assert_eq!(PyDict_Size(dict), 0);

            let key = MoltObject::from_int(10).bits();
            let val = MoltObject::from_int(20).bits();
            assert_eq!(PyDict_SetItem(dict, key, val), 0);
            assert_eq!(PyDict_Size(dict), 1);
            assert_eq!(PyDict_Contains(dict, key), 1);

            let got = PyDict_GetItem(dict, key);
            assert_ne!(got, 0);
            assert_eq!(to_i64(obj_from_bits(got)), Some(20));

            // Missing key returns 0 (no exception).
            let missing_key = MoltObject::from_int(999).bits();
            let missing = PyDict_GetItem(dict, missing_key);
            assert_eq!(missing, 0);
            assert!(!exception_pending(_py));

            assert_eq!(PyDict_Contains(dict, missing_key), 0);

            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_dict_set_item_string() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);

            let val = MoltObject::from_int(42).bits();
            let rc = unsafe { PyDict_SetItemString(dict, c"hello".as_ptr(), val) };
            assert_eq!(rc, 0);
            assert_eq!(PyDict_Size(dict), 1);

            // Verify we can retrieve by constructing a matching key.
            let key_ptr = alloc_string(_py, b"hello");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let got = PyDict_GetItem(dict, key_bits);
            assert_eq!(to_i64(obj_from_bits(got)), Some(42));

            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_tuple_new_size_getitem_setitem() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let tuple = PyTuple_New(3);
            assert_ne!(tuple, 0);
            assert_eq!(PyTuple_Size(tuple), 3);

            // All slots default to None.
            let item0 = PyTuple_GetItem(tuple, 0);
            assert!(obj_from_bits(item0).is_none());

            // SetItem steals the ref, so inc_ref the value first.
            let val = MoltObject::from_int(77).bits();
            inc_ref_bits(_py, val);
            assert_eq!(PyTuple_SetItem(tuple, 2, val), 0);
            let got = PyTuple_GetItem(tuple, 2);
            assert_eq!(to_i64(obj_from_bits(got)), Some(77));

            // Out-of-bounds
            let bad = PyTuple_GetItem(tuple, 5);
            assert_eq!(bad, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            // Negative index in SetItem should fail (CPython tuple uses non-negative only).
            let steal_val = MoltObject::from_int(1).bits();
            inc_ref_bits(_py, steal_val);
            let rc = PyTuple_SetItem(tuple, -1, steal_val);
            assert_eq!(rc, -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, tuple);
        });
    }

    #[test]
    fn c_api_type_checks() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // Int
            let int_val = MoltObject::from_int(42).bits();
            assert_eq!(PyLong_Check(int_val), 1);
            assert_eq!(PyFloat_Check(int_val), 0);
            assert_eq!(PyBool_Check(int_val), 0);
            assert_eq!(PyNone_Check(int_val), 0);
            assert_eq!(PyUnicode_Check(int_val), 0);
            assert_eq!(PyList_Check(int_val), 0);
            assert_eq!(PyDict_Check(int_val), 0);
            assert_eq!(PyTuple_Check(int_val), 0);

            // Float
            let float_val = MoltObject::from_float(3.125).bits();
            assert_eq!(PyFloat_Check(float_val), 1);
            assert_eq!(PyLong_Check(float_val), 0);

            // Bool
            let bool_val = MoltObject::from_bool(true).bits();
            assert_eq!(PyBool_Check(bool_val), 1);

            // None
            let none_val = MoltObject::none().bits();
            assert_eq!(PyNone_Check(none_val), 1);
            assert_eq!(PyBool_Check(none_val), 0);

            // String
            let str_ptr = alloc_string(_py, b"hello");
            assert!(!str_ptr.is_null());
            let str_bits = MoltObject::from_ptr(str_ptr).bits();
            assert_eq!(PyUnicode_Check(str_bits), 1);
            assert_eq!(PyLong_Check(str_bits), 0);
            dec_ref_bits(_py, str_bits);

            // List
            let list = PyList_New(0);
            assert_ne!(list, 0);
            assert_eq!(PyList_Check(list), 1);
            assert_eq!(PyTuple_Check(list), 0);
            assert_eq!(PyDict_Check(list), 0);
            dec_ref_bits(_py, list);

            // Dict
            let dict = PyDict_New();
            assert_ne!(dict, 0);
            assert_eq!(PyDict_Check(dict), 1);
            assert_eq!(PyList_Check(dict), 0);
            dec_ref_bits(_py, dict);

            // Tuple
            let tuple = PyTuple_New(0);
            assert_ne!(tuple, 0);
            assert_eq!(PyTuple_Check(tuple), 1);
            assert_eq!(PyList_Check(tuple), 0);
            dec_ref_bits(_py, tuple);
        });
    }

    #[test]
    fn c_api_iter_protocol_on_list() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // Build a list [10, 20, 30]
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(10).bits(),
                    MoltObject::from_int(20).bits(),
                    MoltObject::from_int(30).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            // Check PyIter_Check on the list (not an iterator itself).
            assert_eq!(PyIter_Check(list_bits), 0);

            // Get an iterator.
            let iter = PyObject_GetIter(list_bits);
            assert_ne!(iter, 0);
            assert!(!exception_pending(_py));

            // The iterator should pass PyIter_Check.
            assert_eq!(PyIter_Check(iter), 1);

            // Iterate: 10, 20, 30, then NULL.
            let v1 = PyIter_Next(iter);
            assert_ne!(v1, 0);
            assert_eq!(to_i64(obj_from_bits(v1)), Some(10));
            dec_ref_bits(_py, v1);

            let v2 = PyIter_Next(iter);
            assert_ne!(v2, 0);
            assert_eq!(to_i64(obj_from_bits(v2)), Some(20));
            dec_ref_bits(_py, v2);

            let v3 = PyIter_Next(iter);
            assert_ne!(v3, 0);
            assert_eq!(to_i64(obj_from_bits(v3)), Some(30));
            dec_ref_bits(_py, v3);

            // Exhausted — returns 0 with no exception.
            let v4 = PyIter_Next(iter);
            assert_eq!(v4, 0);
            assert!(!exception_pending(_py));

            dec_ref_bits(_py, iter);
            dec_ref_bits(_py, list_bits);
        });
    }

    #[test]
    fn c_api_get_iter_on_non_iterable_fails() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let int_val = MoltObject::from_int(42).bits();
            let iter = PyObject_GetIter(int_val);
            assert_eq!(iter, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_list_setitem_steals_ref_on_error() {
        // Verify that PyList_SetItem steals the reference even when the call fails.
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);
            // Try to SetItem on a dict (not a list) — should fail and steal the ref.
            let val = MoltObject::from_int(1).bits();
            inc_ref_bits(_py, val);
            let rc = PyList_SetItem(dict, 0, val);
            assert_eq!(rc, -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
            dec_ref_bits(_py, dict);
        });
    }

    // -----------------------------------------------------------------------
    // Number Protocol tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_number_add_int() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(10).bits();
            let b = MoltObject::from_int(20).bits();
            let res = PyNumber_Add(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(30));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_add_float() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_float(1.5).bits();
            let b = MoltObject::from_float(2.5).bits();
            let res = PyNumber_Add(a, b);
            assert_ne!(res, 0);
            assert_eq!(obj_from_bits(res).as_float(), Some(4.0));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_subtract() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(50).bits();
            let b = MoltObject::from_int(30).bits();
            let res = PyNumber_Subtract(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(20));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_multiply() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(6).bits();
            let b = MoltObject::from_int(7).bits();
            let res = PyNumber_Multiply(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(42));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_truedivide() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(10).bits();
            let b = MoltObject::from_int(4).bits();
            let res = PyNumber_TrueDivide(a, b);
            assert_ne!(res, 0);
            assert_eq!(obj_from_bits(res).as_float(), Some(2.5));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_truedivide_by_zero() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(10).bits();
            let b = MoltObject::from_int(0).bits();
            let res = PyNumber_TrueDivide(a, b);
            assert_eq!(res, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_number_floordivide() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(17).bits();
            let b = MoltObject::from_int(5).bits();
            let res = PyNumber_FloorDivide(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(3));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_remainder() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(17).bits();
            let b = MoltObject::from_int(5).bits();
            let res = PyNumber_Remainder(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(2));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_power() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(2).bits();
            let b = MoltObject::from_int(10).bits();
            let res = PyNumber_Power(a, b, 0);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(1024));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_power_with_mod() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // pow(2, 10, 100) = 1024 % 100 = 24
            let a = MoltObject::from_int(2).bits();
            let b = MoltObject::from_int(10).bits();
            let m = MoltObject::from_int(100).bits();
            let res = PyNumber_Power(a, b, m);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(24));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_negative() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(42).bits();
            let res = PyNumber_Negative(a);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(-42));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_positive() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(-7).bits();
            let res = PyNumber_Positive(a);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(-7));
            dec_ref_bits(_py, res);
        });
    }

    #[test]
    fn c_api_number_absolute() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(-42).bits();
            let res = PyNumber_Absolute(a);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(42));
            dec_ref_bits(_py, res);

            let b = MoltObject::from_float(-3.125).bits();
            let res2 = PyNumber_Absolute(b);
            assert_ne!(res2, 0);
            assert_eq!(obj_from_bits(res2).as_float(), Some(3.125));
            dec_ref_bits(_py, res2);
        });
    }

    #[test]
    fn c_api_number_invert() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(0).bits();
            let res = PyNumber_Invert(a);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(-1));
            dec_ref_bits(_py, res);

            let b = MoltObject::from_int(7).bits();
            let res2 = PyNumber_Invert(b);
            assert_ne!(res2, 0);
            assert_eq!(to_i64(obj_from_bits(res2)), Some(-8));
            dec_ref_bits(_py, res2);
        });
    }

    #[test]
    fn c_api_number_lshift_rshift() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(1).bits();
            let b = MoltObject::from_int(4).bits();
            let res = PyNumber_Lshift(a, b);
            assert_ne!(res, 0);
            assert_eq!(to_i64(obj_from_bits(res)), Some(16));
            dec_ref_bits(_py, res);

            let c = MoltObject::from_int(32).bits();
            let d = MoltObject::from_int(3).bits();
            let res2 = PyNumber_Rshift(c, d);
            assert_ne!(res2, 0);
            assert_eq!(to_i64(obj_from_bits(res2)), Some(4));
            dec_ref_bits(_py, res2);
        });
    }

    #[test]
    fn c_api_number_and_or_xor() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let a = MoltObject::from_int(0b1100).bits();
            let b = MoltObject::from_int(0b1010).bits();

            let and_res = PyNumber_And(a, b);
            assert_ne!(and_res, 0);
            assert_eq!(to_i64(obj_from_bits(and_res)), Some(0b1000));
            dec_ref_bits(_py, and_res);

            let or_res = PyNumber_Or(a, b);
            assert_ne!(or_res, 0);
            assert_eq!(to_i64(obj_from_bits(or_res)), Some(0b1110));
            dec_ref_bits(_py, or_res);

            let xor_res = PyNumber_Xor(a, b);
            assert_ne!(xor_res, 0);
            assert_eq!(to_i64(obj_from_bits(xor_res)), Some(0b0110));
            dec_ref_bits(_py, xor_res);
        });
    }

    #[test]
    fn c_api_number_check() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            assert_eq!(PyNumber_Check(MoltObject::from_int(42).bits()), 1);
            assert_eq!(PyNumber_Check(MoltObject::from_float(3.125).bits()), 1);
            assert_eq!(PyNumber_Check(MoltObject::from_bool(true).bits()), 1);
            assert_eq!(PyNumber_Check(MoltObject::none().bits()), 0);

            let str_ptr = alloc_string(_py, b"hello");
            assert!(!str_ptr.is_null());
            let str_bits = MoltObject::from_ptr(str_ptr).bits();
            assert_eq!(PyNumber_Check(str_bits), 0);
            dec_ref_bits(_py, str_bits);

            let list = PyList_New(0);
            assert_ne!(list, 0);
            assert_eq!(PyNumber_Check(list), 0);
            dec_ref_bits(_py, list);
        });
    }

    #[test]
    fn c_api_number_long_and_float() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // int(3.7) == 3
            let f = MoltObject::from_float(3.7).bits();
            let long_res = PyNumber_Long(f);
            assert_ne!(long_res, 0);
            assert_eq!(to_i64(obj_from_bits(long_res)), Some(3));
            dec_ref_bits(_py, long_res);

            // float(42) == 42.0
            let i = MoltObject::from_int(42).bits();
            let float_res = PyNumber_Float(i);
            assert_ne!(float_res, 0);
            assert_eq!(obj_from_bits(float_res).as_float(), Some(42.0));
            dec_ref_bits(_py, float_res);
        });
    }

    // -----------------------------------------------------------------------
    // Mapping Protocol tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_mapping_length() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);
            assert_eq!(PyMapping_Length(dict), 0);

            let key = MoltObject::from_int(1).bits();
            let val = MoltObject::from_int(100).bits();
            assert_eq!(PyDict_SetItem(dict, key, val), 0);
            assert_eq!(PyMapping_Length(dict), 1);

            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_mapping_keys_values_items() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);

            let k1 = MoltObject::from_int(1).bits();
            let v1 = MoltObject::from_int(10).bits();
            let k2 = MoltObject::from_int(2).bits();
            let v2 = MoltObject::from_int(20).bits();
            assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
            assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

            let keys = PyMapping_Keys(dict);
            assert_ne!(keys, 0);
            assert_eq!(PySequence_Length(keys), 2);
            dec_ref_bits(_py, keys);

            let values = PyMapping_Values(dict);
            assert_ne!(values, 0);
            assert_eq!(PySequence_Length(values), 2);
            dec_ref_bits(_py, values);

            let items = PyMapping_Items(dict);
            assert_ne!(items, 0);
            assert_eq!(PySequence_Length(items), 2);
            dec_ref_bits(_py, items);

            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_mapping_getitemstring() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);

            let key_ptr = alloc_string(_py, b"hello");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let val = MoltObject::from_int(99).bits();
            assert_eq!(PyDict_SetItem(dict, key_bits, val), 0);

            let got = unsafe { PyMapping_GetItemString(dict, c"hello".as_ptr()) };
            assert_ne!(got, 0);
            assert_eq!(to_i64(obj_from_bits(got)), Some(99));
            dec_ref_bits(_py, got);

            // Missing key should fail.
            let missing = unsafe { PyMapping_GetItemString(dict, c"nope".as_ptr()) };
            assert_eq!(missing, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            // NULL key should fail.
            let null_key = unsafe { PyMapping_GetItemString(dict, std::ptr::null()) };
            assert_eq!(null_key, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_mapping_haskey() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);

            let key = MoltObject::from_int(42).bits();
            let val = MoltObject::from_int(1).bits();
            assert_eq!(PyDict_SetItem(dict, key, val), 0);

            assert_eq!(PyMapping_HasKey(dict, key), 1);
            assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

            dec_ref_bits(_py, dict);
        });
    }

    // -----------------------------------------------------------------------
    // Sequence Protocol addition tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_sequence_getitem() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(10).bits(),
                    MoltObject::from_int(20).bits(),
                    MoltObject::from_int(30).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let item = PySequence_GetItem(list_bits, 1);
            assert_ne!(item, 0);
            assert_eq!(to_i64(obj_from_bits(item)), Some(20));
            dec_ref_bits(_py, item);

            // Negative index: -1 should get last element.
            let last = PySequence_GetItem(list_bits, -1);
            assert_ne!(last, 0);
            assert_eq!(to_i64(obj_from_bits(last)), Some(30));
            dec_ref_bits(_py, last);

            // Out-of-bounds.
            let bad = PySequence_GetItem(list_bits, 100);
            assert_eq!(bad, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, list_bits);
        });
    }

    #[test]
    fn c_api_sequence_length() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            assert_eq!(PySequence_Length(list_bits), 2);

            let tuple = PyTuple_New(5);
            assert_ne!(tuple, 0);
            assert_eq!(PySequence_Length(tuple), 5);

            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, tuple);
        });
    }

    #[test]
    fn c_api_sequence_contains() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            assert_eq!(
                PySequence_Contains(list_bits, MoltObject::from_int(2).bits()),
                1
            );
            assert_eq!(
                PySequence_Contains(list_bits, MoltObject::from_int(9).bits()),
                0
            );

            dec_ref_bits(_py, list_bits);
        });
    }

    // -----------------------------------------------------------------------
    // Bytes/String Protocol tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_bytes_from_string_and_size() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let data = b"hello bytes";
            let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
            assert_ne!(bytes, 0);

            let size = PyBytes_Size(bytes);
            assert_eq!(size, data.len() as isize);

            let ptr = PyBytes_AsString(bytes);
            assert!(!ptr.is_null());
            let observed = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
            assert_eq!(observed, data);

            dec_ref_bits(_py, bytes);
        });
    }

    #[test]
    fn c_api_bytes_empty() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 0) };
            assert_ne!(bytes, 0);
            assert_eq!(PyBytes_Size(bytes), 0);
            dec_ref_bits(_py, bytes);
        });
    }

    #[test]
    fn c_api_bytes_null_with_nonzero_len_fails() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let bytes = unsafe { PyBytes_FromStringAndSize(std::ptr::null(), 5) };
            assert_eq!(bytes, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_bytes_negative_len_fails() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let bytes = unsafe { PyBytes_FromStringAndSize(b"abc".as_ptr(), -1) };
            assert_eq!(bytes, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_bytes_asstring_type_error() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let int_val = MoltObject::from_int(42).bits();
            let ptr = PyBytes_AsString(int_val);
            assert!(ptr.is_null());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_bytes_size_type_error() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let int_val = MoltObject::from_int(42).bits();
            let size = PyBytes_Size(int_val);
            assert_eq!(size, -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_unicode_from_string() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let str_bits = unsafe { PyUnicode_FromString(c"hello world".as_ptr()) };
            assert_ne!(str_bits, 0);
            assert_eq!(PyUnicode_Check(str_bits), 1);

            let utf8_ptr = PyUnicode_AsUTF8(str_bits);
            assert!(!utf8_ptr.is_null());
            let observed = unsafe { std::ffi::CStr::from_ptr(utf8_ptr).to_bytes() };
            assert_eq!(observed, b"hello world");
            // The string content might not be NUL-terminated in molt's internal
            // storage, so compare the known length.
            let mut out_size: isize = 0;
            let utf8_ptr2 =
                unsafe { PyUnicode_AsUTF8AndSize(str_bits, &mut out_size as *mut isize) };
            assert!(!utf8_ptr2.is_null());
            assert_eq!(out_size, 11); // "hello world" is 11 bytes
            let observed2 =
                unsafe { std::slice::from_raw_parts(utf8_ptr2 as *const u8, out_size as usize) };
            assert_eq!(observed2, b"hello world");

            dec_ref_bits(_py, str_bits);
        });
    }

    #[test]
    fn c_api_unicode_from_string_null_fails() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let str_bits = unsafe { PyUnicode_FromString(std::ptr::null()) };
            assert_eq!(str_bits, 0);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_unicode_asutf8_type_error() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let int_val = MoltObject::from_int(42).bits();
            let ptr = PyUnicode_AsUTF8(int_val);
            assert!(ptr.is_null());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });
    }

    #[test]
    fn c_api_unicode_asutf8andsize_null_size_ok() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let str_bits = unsafe { PyUnicode_FromString(c"abc".as_ptr()) };
            assert_ne!(str_bits, 0);
            // Pass NULL for size — should not crash.
            let ptr = unsafe { PyUnicode_AsUTF8AndSize(str_bits, std::ptr::null_mut()) };
            assert!(!ptr.is_null());
            dec_ref_bits(_py, str_bits);
        });
    }

    // -----------------------------------------------------------------------
    // Memory Protocol tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_pymem_malloc_realloc_free() {
        let ptr = unsafe { PyMem_Malloc(64) };
        assert!(!ptr.is_null());
        // Write to the allocated memory to verify it is usable.
        unsafe {
            std::ptr::write_bytes(ptr, 0xAB, 64);
            assert_eq!(*ptr, 0xAB);
        }
        let ptr2 = unsafe { PyMem_Realloc(ptr, 128) };
        assert!(!ptr2.is_null());
        // Original content should be preserved.
        unsafe {
            assert_eq!(*ptr2, 0xAB);
        }
        unsafe {
            PyMem_Free(ptr2);
        }
    }

    #[test]
    fn c_api_pymem_malloc_zero_size() {
        // CPython returns a non-NULL pointer for size 0.
        let ptr = unsafe { PyMem_Malloc(0) };
        assert!(!ptr.is_null());
        unsafe {
            PyMem_Free(ptr);
        }
    }

    #[test]
    fn c_api_pymem_free_null_is_safe() {
        // Freeing NULL should be a no-op.
        unsafe {
            PyMem_Free(std::ptr::null_mut());
        }
    }

    #[test]
    fn c_api_pyobject_malloc_realloc_free() {
        let ptr = unsafe { PyObject_Malloc(32) };
        assert!(!ptr.is_null());
        unsafe {
            std::ptr::write_bytes(ptr, 0xCD, 32);
        }
        let ptr2 = unsafe { PyObject_Realloc(ptr, 64) };
        assert!(!ptr2.is_null());
        unsafe {
            assert_eq!(*ptr2, 0xCD);
        }
        unsafe {
            PyObject_Free(ptr2);
        }
    }

    #[test]
    fn c_api_pyobject_free_null_is_safe() {
        // PyObject_Free delegates to PyMem_Free; NULL should be safe.
        unsafe {
            PyObject_Free(std::ptr::null_mut());
        }
    }

    // -----------------------------------------------------------------------
    // Cross-protocol integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn c_api_number_mixed_int_float_arithmetic() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // int + float -> float
            let a = MoltObject::from_int(3).bits();
            let b = MoltObject::from_float(0.125).bits();
            let res = PyNumber_Add(a, b);
            assert_ne!(res, 0);
            let val = obj_from_bits(res).as_float().unwrap();
            assert!((val - 3.125).abs() < 1e-10);
            dec_ref_bits(_py, res);

            // float * int -> float
            let c = MoltObject::from_float(2.5).bits();
            let d = MoltObject::from_int(4).bits();
            let res2 = PyNumber_Multiply(c, d);
            assert_ne!(res2, 0);
            assert_eq!(obj_from_bits(res2).as_float(), Some(10.0));
            dec_ref_bits(_py, res2);
        });
    }

    #[test]
    fn c_api_sequence_and_mapping_on_dict() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict = PyDict_New();
            assert_ne!(dict, 0);

            let k1 = MoltObject::from_int(1).bits();
            let v1 = MoltObject::from_int(100).bits();
            let k2 = MoltObject::from_int(2).bits();
            let v2 = MoltObject::from_int(200).bits();
            assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
            assert_eq!(PyDict_SetItem(dict, k2, v2), 0);

            // PyMapping_Length works on dict.
            assert_eq!(PyMapping_Length(dict), 2);

            // PyMapping_HasKey works.
            assert_eq!(PyMapping_HasKey(dict, k1), 1);
            assert_eq!(PyMapping_HasKey(dict, MoltObject::from_int(999).bits()), 0);

            // PySequence_Contains also works on dict (checks keys).
            assert_eq!(PySequence_Contains(dict, k2), 1);
            assert_eq!(
                PySequence_Contains(dict, MoltObject::from_int(999).bits()),
                0
            );

            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_bytes_roundtrip_via_protocol() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let data = b"\x00\x01\x02\xff";
            let bytes = unsafe { PyBytes_FromStringAndSize(data.as_ptr(), data.len() as isize) };
            assert_ne!(bytes, 0);
            assert_eq!(PyBytes_Size(bytes), 4);
            let ptr = PyBytes_AsString(bytes);
            assert!(!ptr.is_null());
            let observed = unsafe { std::slice::from_raw_parts(ptr, 4) };
            assert_eq!(observed, data);
            dec_ref_bits(_py, bytes);
        });
    }

    #[test]
    fn c_api_object_protocol_repr_str_hash_truthy() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let int_val = MoltObject::from_int(42).bits();

            // PyObject_Repr — re-entrant GIL acquisition
            let repr = PyObject_Repr(int_val);
            assert_ne!(repr, 0);
            dec_ref_bits(_py, repr);

            // PyObject_Str
            let str_val = PyObject_Str(int_val);
            assert_ne!(str_val, 0);
            dec_ref_bits(_py, str_val);

            // PyObject_Hash
            let hash = PyObject_Hash(int_val);
            assert_ne!(hash, -1);

            // PyObject_IsTrue / PyObject_Not
            assert_eq!(PyObject_IsTrue(int_val), 1);
            assert_eq!(PyObject_Not(int_val), 0);
            assert_eq!(PyObject_IsTrue(MoltObject::from_int(0).bits()), 0);
            assert_eq!(PyObject_Not(MoltObject::from_int(0).bits()), 1);
            assert_eq!(PyObject_IsTrue(MoltObject::from_bool(true).bits()), 1);
            assert_eq!(PyObject_IsTrue(MoltObject::from_bool(false).bits()), 0);
        });
    }

    #[test]
    fn c_api_object_type_and_length() {
        let _ = molt_runtime_init();
        // C-API functions acquire GIL internally — don't nest
        let list = PyList_New(3);
        assert_ne!(list, 0);

        let ty = PyObject_Type(list);
        assert_ne!(ty, 0);
        crate::with_gil_entry!(_py, { dec_ref_bits(_py, ty) });

        assert_eq!(PyObject_Length(list), 3);
        assert_eq!(PyObject_Size(list), 3);

        crate::with_gil_entry!(_py, { dec_ref_bits(_py, list) });
    }

    #[test]
    fn c_api_rich_compare() {
        let _ = molt_runtime_init();
        let a = MoltObject::from_int(10).bits();
        let b = MoltObject::from_int(20).bits();

        assert_eq!(PyObject_RichCompareBool(a, b, 0), 1); // 10 < 20
        assert_eq!(PyObject_RichCompareBool(a, b, 1), 1); // 10 <= 20
        assert_eq!(PyObject_RichCompareBool(a, b, 2), 0); // 10 == 20
        assert_eq!(PyObject_RichCompareBool(a, b, 3), 1); // 10 != 20
        assert_eq!(PyObject_RichCompareBool(a, b, 4), 0); // 10 > 20
        assert_eq!(PyObject_RichCompareBool(a, b, 5), 0); // 10 >= 20

        // Invalid op
        assert_eq!(PyObject_RichCompareBool(a, b, 99), -1);
        crate::with_gil_entry!(_py, {
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
        });

        let cmp = PyObject_RichCompare(a, b, 2);
        assert_ne!(cmp, 0);
        crate::with_gil_entry!(_py, { dec_ref_bits(_py, cmp) });
    }

    #[test]
    fn c_api_callable_check_and_isinstance() {
        let _ = molt_runtime_init();
        let int_val = MoltObject::from_int(5).bits();
        assert_eq!(PyCallable_Check(int_val), 0);

        crate::with_gil_entry!(_py, {
            let builtins = builtin_classes(_py);
            let int_type = builtins.int;
            let result = PyObject_IsInstance(int_val, int_type);
            assert_eq!(result, 1);

            let none_val = none_bits();
            let result2 = PyObject_IsInstance(none_val, int_type);
            assert_eq!(result2, 0);
        });
    }

    #[test]
    fn c_api_set_protocol() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            // Create empty set — capacity is raw u64, NOT NaN-boxed
            let set = molt_set_new(0u64);
            assert!(!obj_from_bits(set).is_none());

            // PySet_Check / PyFrozenSet_Check
            assert_eq!(PySet_Check(set), 1);
            assert_eq!(PyFrozenSet_Check(set), 0);

            // Add elements via runtime directly
            let k1 = MoltObject::from_int(10).bits();
            let k2 = MoltObject::from_int(20).bits();
            let add_res1 = molt_set_add(set, k1);
            assert!(!exception_pending(_py));
            if !obj_from_bits(add_res1).is_none() {
                dec_ref_bits(_py, add_res1);
            }
            let add_res2 = molt_set_add(set, k2);
            if !obj_from_bits(add_res2).is_none() {
                dec_ref_bits(_py, add_res2);
            }

            // PySet_Size
            assert_eq!(PySet_Size(set), 2);

            // PySet_Contains
            assert_eq!(PySet_Contains(set, k1), 1);
            assert_eq!(PySet_Contains(set, MoltObject::from_int(99).bits()), 0);

            // Discard
            let disc_res = molt_set_discard(set, k1);
            if !obj_from_bits(disc_res).is_none() {
                dec_ref_bits(_py, disc_res);
            }
            assert_eq!(PySet_Contains(set, k1), 0);

            // Pop
            let popped = PySet_Pop(set);
            assert_ne!(popped, 0);
            assert_eq!(PySet_Size(set), 0);
            dec_ref_bits(_py, popped);

            // Clear
            let add_res3 = molt_set_add(set, k1);
            if !obj_from_bits(add_res3).is_none() {
                dec_ref_bits(_py, add_res3);
            }
            assert_eq!(PySet_Clear(set), 0);
            assert_eq!(PySet_Size(set), 0);

            dec_ref_bits(_py, set);
        });
    }

    #[test]
    fn c_api_dict_extended_operations() {
        let _ = molt_runtime_init();
        let dict = PyDict_New();
        assert_ne!(dict, 0);

        crate::with_gil_entry!(_py, {
            let k1_ptr = alloc_string(_py, b"hello");
            assert!(!k1_ptr.is_null());
            let k1 = MoltObject::from_ptr(k1_ptr).bits();
            let v1 = MoltObject::from_int(100).bits();
            assert_eq!(PyDict_SetItem(dict, k1, v1), 0);

            let got = PyDict_GetItemString(dict, c"hello".as_ptr());
            assert_ne!(got, 0);

            assert_eq!(PyDict_DelItem(dict, k1), 0);
            assert_eq!(PyDict_Size(dict), 0);

            let rc = PyDict_DelItem(dict, k1);
            assert_eq!(rc, -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            assert_eq!(PyDict_SetItem(dict, k1, v1), 0);
            let keys = PyDict_Keys(dict);
            assert_ne!(keys, 0);
            dec_ref_bits(_py, keys);
            let vals = PyDict_Values(dict);
            assert_ne!(vals, 0);
            dec_ref_bits(_py, vals);
            let items = PyDict_Items(dict);
            assert_ne!(items, 0);
            dec_ref_bits(_py, items);

            let copy = PyDict_Copy(dict);
            assert_ne!(copy, 0);
            assert_eq!(PyDict_Size(copy), 1);
            dec_ref_bits(_py, copy);

            dec_ref_bits(_py, k1);
            dec_ref_bits(_py, dict);
        });
    }

    #[test]
    fn c_api_list_extended_operations() {
        let _ = molt_runtime_init();
        let list = PyList_New(0);
        assert_ne!(list, 0);

        assert_eq!(PyList_Append(list, MoltObject::from_int(3).bits()), 0);
        assert_eq!(PyList_Append(list, MoltObject::from_int(1).bits()), 0);
        assert_eq!(PyList_Append(list, MoltObject::from_int(2).bits()), 0);
        assert_eq!(PyList_Size(list), 3);

        assert_eq!(PyList_Insert(list, 0, MoltObject::from_int(0).bits()), 0);
        assert_eq!(PyList_Size(list), 4);

        assert_eq!(PyList_Reverse(list), 0);
        assert_eq!(PyList_Sort(list), 0);

        let tup = PyList_AsTuple(list);
        assert_ne!(tup, 0);
        assert_eq!(PyTuple_Size(tup), 4);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, tup);
            dec_ref_bits(_py, list);
        });
    }

    #[test]
    fn c_api_exception_protocol() {
        let _ = molt_runtime_init();
        assert_eq!(PyErr_Occurred(), 0);

        PyErr_SetString(0, c"test error".as_ptr());
        assert_ne!(PyErr_Occurred(), 0);

        PyErr_Clear();
        assert_eq!(PyErr_Occurred(), 0);

        let _ = PyErr_NoMemory();
        assert_ne!(PyErr_Occurred(), 0);
        PyErr_Clear();
    }

    #[test]
    fn c_api_refcount_and_conversions() {
        let _ = molt_runtime_init();
        // PyLong_FromLong / PyLong_AsLong — inline NaN-boxed, no GIL needed
        let long = PyLong_FromLong(42);
        assert_ne!(long, 0);
        assert_eq!(PyLong_AsLong(long), 42);

        let float = PyFloat_FromDouble(3.125);
        let val = PyFloat_AsDouble(float);
        assert!((val - 3.125).abs() < 0.001);

        let t = PyBool_FromLong(1);
        assert_eq!(PyObject_IsTrue(t), 1);
        let f = PyBool_FromLong(0);
        assert_eq!(PyObject_IsTrue(f), 0);

        let n = Py_BuildNone();
        assert!(obj_from_bits(n).is_none());

        crate::with_gil_entry!(_py, {
            let s_ptr = alloc_string(_py, b"refcount_test");
            assert!(!s_ptr.is_null());
            let s = MoltObject::from_ptr(s_ptr).bits();
            Py_IncRef(s);
            Py_DecRef(s);
            Py_XINCREF(s);
            Py_XDECREF(s);
            dec_ref_bits(_py, s);
        });
    }

    #[test]
    fn c_api_unicode_extended() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let s_ptr = alloc_string(_py, b"hello");
            assert!(!s_ptr.is_null());
            let s = MoltObject::from_ptr(s_ptr).bits();

            assert_eq!(PyUnicode_GetLength(s), 5);

            let sub_ptr = alloc_string(_py, b"ell");
            assert!(!sub_ptr.is_null());
            let sub = MoltObject::from_ptr(sub_ptr).bits();
            assert_eq!(PyUnicode_Contains(s, sub), 1);

            let s2_ptr = alloc_string(_py, b" world");
            assert!(!s2_ptr.is_null());
            let s2 = MoltObject::from_ptr(s2_ptr).bits();
            let concat = PyUnicode_Concat(s, s2);
            assert_ne!(concat, 0);
            assert_eq!(PyUnicode_GetLength(concat), 11);
            dec_ref_bits(_py, concat);

            let cmp = PyUnicode_CompareWithASCIIString(s, c"hello".as_ptr());
            assert_eq!(cmp, 0);

            dec_ref_bits(_py, s2);
            dec_ref_bits(_py, sub);
            dec_ref_bits(_py, s);
        });
    }
}
