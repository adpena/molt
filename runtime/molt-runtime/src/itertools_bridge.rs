//! FFI bridge for `molt-runtime-itertools`.
//!
//! Exports C API functions that the itertools crate needs but that are not
//! already available as `#[no_mangle]` symbols.

use crate::BuiltinExc;
use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

// ---------------------------------------------------------------------------
// Runtime-scoped extension state for the itertools satellite
// ---------------------------------------------------------------------------

/// Register/fetch the itertools satellite's per-interpreter slot box. The box
/// is created on first use and `clear`+`drop`-ed by the runtime on interpreter
/// teardown (see `runtime_extension_states_clear_and_drop`), so the satellite's
/// cached class/function handles are scoped to the interpreter exactly like the
/// in-tree `RuntimeState.itertools` field.
#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_itertools_runtime_state_get_or_init"))]
pub extern "C" fn __molt_itertools_runtime_state_get_or_init(
    __molt_context: u64,
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    }
}

/// Release the ordinary owned reference held by an itertools satellite slot.
/// Canonical immortal values remain owned by the runtime's canonical pool.

pub(crate) fn molt_itertools_release_slot_bits_attached_body(_py: &PyToken<'_>, bits: u64) {
    if bits == 0 {
        return;
    }
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::dec_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// Class system helpers for itertools class setup
// ---------------------------------------------------------------------------

pub(crate) fn molt_itertools_validate_constructor_class_attached_body(
    _py: &PyToken<'_>,
    actual: u64,
    expected: u64,
) -> bool {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::itertools_class::validate_constructor_class(py, actual, expected)
    }
}

pub(crate) fn molt_itertools_constructor_repr_attached_body(
    _py: &PyToken<'_>,
    instance: u64,
    first: u64,
    second: u64,
) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::itertools_class::constructor_repr(py, instance, first, second)
    }
}

pub(crate) fn molt_itertools_alloc_instance_for_class_attached_body(
    _py: &PyToken<'_>,
    class_bits: u64,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe { alloc_instance_for_class(_py, class_ptr) }
    }
}

pub(crate) fn molt_itertools_call_callable1_attached_body(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { call_callable1(_py, call_bits, arg0_bits) }
    }
}

pub(crate) fn molt_itertools_call_callable2_bridge_attached_body(
    _py: &PyToken<'_>,
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        unsafe { call_callable2(_py, call_bits, arg0_bits, arg1_bits) }
    }
}

pub(crate) fn molt_itertools_tuple_from_iter_attached_body(
    _py: &PyToken<'_>,
    iter_bits: u64,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let bits: u64 = unsafe { tuple_from_iter_bits(_py, iter_bits) }.unwrap_or_default();
        bits // signal failure (not None — caller checks for 0)
    }
}

pub(crate) fn molt_itertools_alloc_class_attached_body(
    _py: &PyToken<'_>,
    name_ptr: *const u8,
    name_len: usize,
    layout_size: i64,
    shape_id: u16,
    iter_fn: u64,
    next_fn: u64,
    new_fn: u64,
    new_arity: u64,
    defaults_ptr: *const u64,
    defaults_len: usize,
) -> u64 {
    {
        let py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if name_ptr.is_null() || name_len > isize::MAX as usize {
            return raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "invalid itertools class name span",
            );
        }
        let Ok(name) =
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_ptr, name_len) })
        else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "invalid itertools class name",
            );
        };
        let Some(shape) = crate::object::ObjectShapeId::from_u16(shape_id) else {
            return raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "invalid itertools shape id",
            );
        };
        let constructor = if new_fn == 0 {
            if new_arity != 0 || defaults_len != 0 {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::SystemError,
                    "invalid absent itertools constructor",
                );
            }
            None
        } else {
            let defaults = if defaults_len == 0 {
                &[]
            } else {
                if defaults_ptr.is_null()
                    || !defaults_ptr.is_aligned()
                    || defaults_len > isize::MAX as usize / std::mem::size_of::<u64>()
                {
                    return raise_exception::<u64>(
                        py,
                        BuiltinExc::SystemError,
                        "invalid itertools defaults span",
                    );
                }
                unsafe { std::slice::from_raw_parts(defaults_ptr, defaults_len) }
            };
            Some((new_fn, new_arity, defaults))
        };
        crate::itertools_class::alloc_itertools_class(
            py,
            name,
            layout_size,
            shape,
            iter_fn,
            next_fn,
            constructor,
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { object_class_bits(ptr) }
}

// ---------------------------------------------------------------------------
// Additional C API exports for functions the itertools crate needs.
// These symbols are resolved by the linker at link time.
// ---------------------------------------------------------------------------

/// Object type ID — exposed for the itertools crate.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

/// index_i64_from_obj — exposed for the itertools crate.

pub(crate) fn molt_index_i64_from_obj_attached_body(
    _py: &PyToken<'_>,
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
) -> i64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        builtins::numbers::index_i64_from_obj(_py, obj_bits, err)
    }
}

/// intern_static_name — exposed for the itertools crate.

pub(crate) fn molt_intern_static_name_attached_body(
    _py: &PyToken<'_>,
    key_ptr: *const u8,
    key_len: usize,
) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        let ptr = alloc_string(_py, key);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    }
}

/// raise_not_iterable — exposed for the itertools crate.

pub(crate) fn molt_raise_not_iterable_attached_body(_py: &PyToken<'_>, bits: u64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        raise_not_iterable::<u64>(_py, bits)
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/attached_exports_generated/itertools_bridge.rs"
));
