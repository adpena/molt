//! Residual parameters of native GenericAlias and UnionType objects.
//!
//! CPython's _Py_make_parameters policy is independent of typing's additional
//! declaration/default-order policy. Both consume actual substitution methods;
//! no user-visible class spelling identifies a parameter.

use super::*;
use crate::BuiltinExc;
use crate::object::seq_access::pin_tuple;
use crate::state::recursion::RecursionGuard;
use molt_runtime_core::ProtectedRuntimeValue;

fn optional_attribute<'py>(
    py: &'py PyToken<'_>,
    value: u64,
    name: u64,
) -> Result<Option<ProtectedRuntimeValue<'py>>, ()> {
    let Some(pointer) = maybe_ptr_from_bits(value) else {
        return Ok(None);
    };
    unsafe {
        attr_lookup_ptr_allow_missing(py, pointer, name).map(|value| {
            value.map(|bits| ProtectedRuntimeValue::from_owned_bits(py.core_token(), bits))
        })
    }
}

fn append_parameter<'py>(
    py: &'py PyToken<'_>,
    parameters: &mut Vec<ProtectedRuntimeValue<'py>>,
    value: u64,
) -> Result<(), ()> {
    if parameters.iter().any(|parameter| parameter.bits() == value) {
        return Ok(());
    }
    if parameters.try_reserve(1).is_err() {
        raise_exception::<()>(
            py,
            BuiltinExc::MemoryError,
            "type parameter collection allocation failed",
        );
        return Err(());
    }
    parameters.push(ProtectedRuntimeValue::retain(py.core_token(), value));
    Ok(())
}

fn collect_parameters(py: &PyToken<'_>, arguments: u64) -> Result<u64, ()> {
    let Some(_recursion) = RecursionGuard::enter_with_message(
        py,
        "maximum recursion depth exceeded in __parameter__ calculation",
    ) else {
        return Err(());
    };
    let Some(mut pointer) = obj_from_bits(arguments).as_ptr() else {
        raise_exception::<()>(
            py,
            BuiltinExc::SystemError,
            "type parameter arguments require a sequence",
        );
        return Err(());
    };
    // CPython normalizes raw lists with PySequence_Tuple: an exact list uses
    // its stable snapshot, while a list subclass retains its real __iter__.
    // Tuples, including subclasses, use their immutable physical item order.
    let _normalized = if unsafe { object_type_id(pointer) } == TYPE_ID_LIST {
        let Some(bits) = (unsafe { crate::object::ops::tuple_from_iter_bits(py, arguments) })
        else {
            return Err(());
        };
        pointer = obj_from_bits(bits)
            .as_ptr()
            .expect("tuple conversion result");
        Some(unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), bits) })
    } else {
        None
    };
    let Some(arguments) = (unsafe { pin_tuple(py, pointer) }) else {
        if !exception_pending(py) {
            raise_exception::<()>(
                py,
                BuiltinExc::SystemError,
                "type parameter arguments require a sequence",
            );
        }
        return Err(());
    };
    let Some(substitution_name) = attr_name_bits_from_bytes(py, b"__typing_subst__") else {
        return Err(());
    };
    let substitution_name =
        unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), substitution_name) };
    let Some(parameters_name) = attr_name_bits_from_bytes(py, b"__parameters__") else {
        return Err(());
    };
    let parameters_name =
        unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), parameters_name) };
    let descend_sequences = crate::object::ops_sys::runtime_target_at_least(py, 3, 14);
    let mut parameters = Vec::new();
    for &argument in arguments.iter() {
        if obj_from_bits(argument)
            .as_ptr()
            .is_some_and(|pointer| unsafe { object_type_id(pointer) == TYPE_ID_TYPE })
        {
            // A class's descriptor is not an instance parameter producer.
            continue;
        }
        if optional_attribute(py, argument, substitution_name.bits())?.is_some() {
            append_parameter(py, &mut parameters, argument)?;
            continue;
        }
        let mut nested = optional_attribute(py, argument, parameters_name.bits())?;
        if nested.is_none()
            && descend_sequences
            && obj_from_bits(argument)
                .as_ptr()
                .is_some_and(|pointer| unsafe {
                    matches!(object_type_id(pointer), TYPE_ID_TUPLE | TYPE_ID_LIST)
                })
        {
            let bits = collect_parameters(py, argument)?;
            nested = Some(unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), bits) });
        }
        if let Some(nested) = nested
            && let Some(pointer) = obj_from_bits(nested.bits()).as_ptr()
            && unsafe { object_type_id(pointer) } == TYPE_ID_TUPLE
        {
            let Some(values) = (unsafe { pin_tuple(py, pointer) }) else {
                return Err(());
            };
            for &value in values.iter() {
                append_parameter(py, &mut parameters, value)?;
            }
        }
    }
    let pointer = alloc_tuple(py, ProtectedRuntimeValue::as_bits_slice(&parameters));
    if pointer.is_null() {
        return Err(());
    }
    Ok(MoltObject::from_ptr(pointer).bits())
}

/// The receiver is pinned by ordinary attribute lookup. Cache fields own one
/// reference and are included in the receiver's canonical GC edge projection.
/// PyToken/GilGuard serializes load-then-retain. Atomic publication does not
/// replace that custody; a no-GIL runtime needs an object lock or retained slot.
pub(super) unsafe fn alias_parameters(py: &PyToken<'_>, pointer: *mut u8) -> Result<u64, ()> {
    let (arguments, cache) = unsafe {
        match object_type_id(pointer) {
            TYPE_ID_GENERIC_ALIAS => (
                generic_alias_args_bits(pointer),
                crate::object::layout::generic_alias_parameters_cache(pointer),
            ),
            TYPE_ID_UNION => (
                union_type_args_bits(pointer),
                crate::object::layout::union_type_parameters_cache(pointer),
            ),
            _ => unreachable!("parameter cache requires an alias owner"),
        }
    };
    let cached = unsafe { &*cache }.load(Ordering::Acquire);
    if !obj_from_bits(cached).is_none() {
        inc_ref_bits(py, cached);
        return Ok(cached);
    }
    let result = collect_parameters(py, arguments)?;
    // A getter may re-enter and fill this cache before our collection completes.
    // CPython 3.12/3.14 both let the completing outer result overwrite that
    // inner publication. First-wins CAS would change the observed parameter.
    inc_ref_bits(py, result); // Independent caller reference before retirement.
    let displaced = unsafe { &*cache }.swap(result, Ordering::AcqRel);
    drop(unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), displaced) });
    Ok(result)
}
