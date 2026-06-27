//! Direct bridge API for satellite sources compiled inside `molt-runtime`.
//!
//! Reduced stdlib profiles compile selected satellite source files directly by
//! `#[path]`. Those files depend on a `crate::bridge` access layer; this module
//! provides that API by calling runtime internals directly instead of crossing
//! the satellite extern-C boundary.

#![allow(dead_code)]

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::object::ops::string_obj_to_owned as runtime_string_obj_to_owned;
use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

pub fn raise_exception<T: ExceptionSentinel>(_py: &CoreGilToken, type_name: &str, msg: &str) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_exception::<u64>(py, type_name, msg))
    })
}

pub fn exception_pending(_py: &CoreGilToken) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::exception_pending(py) })
}

pub fn clear_exception(_py: &CoreGilToken) {
    crate::with_gil_entry_nopanic!(py, {
        crate::clear_exception(py);
    })
}

pub fn raise_os_error<T: ExceptionSentinel>(
    _py: &CoreGilToken,
    err: std::io::Error,
    ctx: &str,
) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_os_error::<u64>(py, err, ctx))
    })
}

pub fn raise_os_error_errno<T: ExceptionSentinel>(_py: &CoreGilToken, errno: i64, ctx: &str) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_os_error_errno::<u64>(py, errno, ctx))
    })
}

pub trait ExceptionSentinel {
    fn from_bits(bits: u64) -> Self;
}

impl ExceptionSentinel for u64 {
    #[inline]
    fn from_bits(bits: u64) -> Self {
        bits
    }
}

impl<T> ExceptionSentinel for Option<T> {
    #[inline]
    fn from_bits(_bits: u64) -> Self {
        None
    }
}

impl ExceptionSentinel for () {
    #[inline]
    fn from_bits(_bits: u64) -> Self {}
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

pub fn alloc_tuple(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_tuple(py, elems) })
}

pub fn alloc_list(_py: &CoreGilToken, elems: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_list(py, elems) })
}

pub fn alloc_string(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_string(py, data) })
}

pub fn alloc_string_bits(_py: &CoreGilToken, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

pub fn alloc_bytes(_py: &CoreGilToken, data: &[u8]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_bytes(py, data) })
}

pub fn alloc_dict_with_pairs(_py: &CoreGilToken, pairs: &[u64]) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, { crate::alloc_dict_with_pairs(py, pairs) })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the lifetime of this
/// call.
pub unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { crate::object_type_id(ptr) }
}

pub fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    runtime_string_obj_to_owned(obj)
}

pub fn is_truthy(_py: &CoreGilToken, obj: MoltObject) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::is_truthy(py, obj) })
}

/// # Safety
///
/// `ptr` must refer to a live Molt object that the runtime recognizes as a
/// bytes-like object when this function returns `Some`.
pub unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe { crate::object::memoryview::bytes_like_slice(ptr) }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

pub fn dec_ref_bits(_py: &CoreGilToken, bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        crate::dec_ref_bits(py, bits);
    })
}

pub fn inc_ref_bits(_py: &CoreGilToken, bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        crate::inc_ref_bits(py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

pub fn to_i64(obj: MoltObject) -> Option<i64> {
    crate::to_i64(obj)
}

pub fn to_f64(obj: MoltObject) -> Option<f64> {
    crate::to_f64(obj)
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

/// # Safety
///
/// `ptr` must refer to a live Molt sequence object backed by `Vec<u64>`.
pub unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe { crate::seq_vec_ref(ptr) }
}

pub unsafe fn dict_get_in_place(_py: &CoreGilToken, ptr: *mut u8, key_bits: u64) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::dict_get_in_place(py, ptr, key_bits) }
    })
}

// ---------------------------------------------------------------------------
// Attribute / callable helpers
// ---------------------------------------------------------------------------

pub fn attr_name_bits_from_bytes(_py: &CoreGilToken, name: &[u8]) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::attr::attr_name_bits_from_bytes(py, name)
    })
}

pub fn call_callable0(_py: &CoreGilToken, call_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable0(py, call_bits) }
    })
}

pub fn call_callable1(_py: &CoreGilToken, call_bits: u64, arg0: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable1(py, call_bits, arg0) }
    })
}

pub fn call_callable2(_py: &CoreGilToken, call_bits: u64, arg0: u64, arg1: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::call::dispatch::call_callable2(py, call_bits, arg0, arg1) }
    })
}

pub fn call_class_init_with_args(_py: &CoreGilToken, class_bits: u64, args: &[u64]) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let class_ptr = obj_from_bits(class_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        unsafe { crate::call_class_init_with_args(py, class_ptr, args) }
    })
}

pub fn missing_bits(_py: &CoreGilToken) -> u64 {
    crate::with_gil_entry_nopanic!(py, { crate::missing_bits(py) })
}

pub fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::molt_getattr_builtin(obj_bits, name_bits, default_bits)
}

pub fn attr_optional(_py: &CoreGilToken, obj_bits: u64, name: &[u8]) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if crate::with_gil_entry_nopanic!(py, {
            crate::builtins::attr::clear_attribute_error_if_pending(py)
        }) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

pub fn molt_repr_from_obj(bits: u64) -> u64 {
    crate::molt_repr_from_obj(bits)
}

pub fn molt_str_from_obj(bits: u64) -> u64 {
    crate::molt_str_from_obj(bits)
}

pub fn builtin_classes(_py: &CoreGilToken, name: &str) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let classes = crate::builtin_classes(py);
        match name {
            "list" => classes.list,
            _ => MoltObject::none().bits(),
        }
    })
}

pub fn resolve_global_bits(_py: &CoreGilToken, module: &str, name: &str) -> Result<u64, u64> {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::functions_pickle::pickle_resolve_global_bits(py, module, name)
    })
}

pub fn type_id_list() -> u32 {
    crate::TYPE_ID_LIST
}

pub fn type_id_tuple() -> u32 {
    crate::TYPE_ID_TUPLE
}

pub fn type_id_dict() -> u32 {
    crate::TYPE_ID_DICT
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

pub fn molt_iter(_py: &CoreGilToken, bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

pub fn molt_iter_bridge(_py: &CoreGilToken, bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

pub fn bridge_molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter_next(iter_bits)
}

pub fn molt_iter_next(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    if result == MoltObject::none().bits() {
        crate::with_gil_entry_nopanic!(py, {
            if crate::exception_pending(py) {
                None
            } else {
                Some(result)
            }
        })
    } else {
        Some(result)
    }
}

pub fn raise_not_iterable<T: ExceptionSentinel>(_py: &CoreGilToken, bits: u64) -> T {
    crate::with_gil_entry_nopanic!(py, {
        T::from_bits(crate::raise_not_iterable::<u64>(py, bits))
    })
}

pub fn tuple_from_iter_bits(_py: &CoreGilToken, iter_bits: u64) -> Option<u64> {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::tuple_from_iter_bits(py, iter_bits) }
    })
}

// ---------------------------------------------------------------------------
// Runtime extension state
// ---------------------------------------------------------------------------

pub type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
pub type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

pub fn runtime_state_get_or_init(
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(py, {
        crate::state::runtime_extension_state_get_or_init(
            crate::runtime_state(py),
            key,
            init,
            clear,
            drop,
        )
    })
}

/// # Safety
///
/// Must be called while the target runtime is alive. `bits` must be a cached
/// object handle owned by a runtime extension-state slot.
pub unsafe fn release_runtime_slot_bits(bits: u64) {
    if bits == 0 {
        return;
    }
    crate::with_gil_entry_nopanic!(py, {
        crate::object::release_shutdown_bits(py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric / arithmetic / callargs helpers
// ---------------------------------------------------------------------------

pub fn index_i64_from_obj(_py: &CoreGilToken, obj_bits: u64, err: &str) -> i64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::numbers::index_i64_from_obj(py, obj_bits, err)
    })
}

pub fn intern_static_name(_py: &CoreGilToken, key: &[u8]) -> u64 {
    let ptr = alloc_string(_py, key);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

pub fn bridge_molt_add(a: u64, b: u64) -> u64 {
    crate::molt_add(a, b)
}

pub fn bridge_molt_eq(a: u64, b: u64) -> u64 {
    crate::molt_eq(a, b)
}

pub fn bridge_callargs_new(pos_cap: u64, kw_cap: u64) -> u64 {
    crate::molt_callargs_new(pos_cap, kw_cap)
}

pub fn bridge_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe { crate::molt_callargs_expand_star(builder_bits, iterable_bits) }
}

pub fn bridge_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::molt_call_bind(call_bits, builder_bits)
}

// ---------------------------------------------------------------------------
// Itertools class/function helpers
// ---------------------------------------------------------------------------

pub fn alloc_instance_for_class(_py: &CoreGilToken, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe { crate::alloc_instance_for_class(py, class_ptr) }
    })
}

pub fn alloc_itertools_class(_py: &CoreGilToken, name: &str, layout_size: i64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let name_str_ptr = crate::alloc_string(py, name.as_bytes());
        if name_str_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_str_ptr).bits();
        let class_ptr = crate::alloc_class_obj(py, name_bits);
        crate::dec_ref_bits(py, name_bits);
        if class_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = crate::builtin_classes(py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                crate::object_set_class_bits(py, ptr, builtins.type_obj);
                crate::inc_ref_bits(py, builtins.type_obj);
            }
        }
        let _ = crate::molt_class_set_base(class_bits, builtins.object);
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let layout_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            unsafe { crate::dict_set_in_place(py, dict_ptr, layout_name, layout_bits) };
        }
        class_bits
    })
}

pub fn class_set_iter_next(
    _py: &CoreGilToken,
    class_bits: u64,
    iter_fn_bits: u64,
    next_fn_bits: u64,
) {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let iter_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.iter_name,
                b"__iter__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, iter_name, iter_fn_bits) };
            let next_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.next_name,
                b"__next__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, next_name, next_fn_bits) };
        }
    });
}

pub fn class_set_new(_py: &CoreGilToken, class_bits: u64, new_fn_bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let new_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.new_name,
                b"__new__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, new_name, new_fn_bits) };
        }
    });
}

pub fn alloc_function(_py: &CoreGilToken, fn_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = crate::alloc_function_obj(py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let builtins = crate::builtin_classes(py);
            let old_bits = crate::object_class_bits(ptr);
            if old_bits != builtins.builtin_function_or_method {
                if old_bits != 0 {
                    crate::dec_ref_bits(py, old_bits);
                }
                crate::object_set_class_bits(py, ptr, builtins.builtin_function_or_method);
                crate::inc_ref_bits(py, builtins.builtin_function_or_method);
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub fn alloc_function_with_defaults(
    _py: &CoreGilToken,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            (*crate::header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_IMMORTAL;
            let defaults_tuple_ptr = crate::alloc_tuple(py, defaults);
            if !defaults_tuple_ptr.is_null() {
                let defaults_name = crate::intern_static_name(
                    py,
                    &crate::runtime_state(py).interned.defaults_name,
                    b"__defaults__",
                );
                let defaults_bits = MoltObject::from_ptr(defaults_tuple_ptr).bits();
                crate::function_set_attr_bits(py, ptr, defaults_name, defaults_bits);
            }
            let builtins = crate::builtin_classes(py);
            let old_bits = crate::object_class_bits(ptr);
            if old_bits != builtins.builtin_function_or_method {
                if old_bits != 0 {
                    crate::dec_ref_bits(py, old_bits);
                }
                crate::object_set_class_bits(py, ptr, builtins.builtin_function_or_method);
                crate::inc_ref_bits(py, builtins.builtin_function_or_method);
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub fn alloc_kwd_mark(_py: &CoreGilToken) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let total = std::mem::size_of::<crate::MoltHeader>();
        let ptr = crate::alloc_object(py, total, crate::TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { crate::object_class_bits(ptr) }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

pub fn has_capability(_py: &CoreGilToken, name: &str) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::has_capability(py, name) })
}

pub enum AuditArg {
    None,
    Path(String),
}

pub fn audit_capability(
    _py: &CoreGilToken,
    operation: &'static str,
    capability: &'static str,
    arg: AuditArg,
) -> bool {
    let allowed = has_capability(_py, capability);
    let args = match arg {
        AuditArg::None => AuditArgs::None,
        AuditArg::Path(path) => AuditArgs::Path(path),
    };
    audit_capability_decision(operation, capability, args, allowed);
    allowed
}

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

pub fn molt_object_hash(bits: u64) -> u64 {
    crate::object::ops_sys::molt_object_hash(bits)
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

pub fn path_from_bits(_py: &CoreGilToken, bits: u64) -> Result<std::path::PathBuf, String> {
    crate::with_gil_entry_nopanic!(py, { crate::path_from_bits(py, bits) })
}

pub fn type_name(_py: &CoreGilToken, obj: MoltObject) -> String {
    crate::with_gil_entry_nopanic!(py, { crate::type_name(py, obj).into_owned() })
}
