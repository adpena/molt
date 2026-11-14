//! Publish constructors through the existing native callable and binder owners.
use super::public_objects_generated::{PublicObjectId, PublicSignatureSpec};
use crate::builtins::functions::native_callable::NativeCallableSpec;
use crate::*;

pub(crate) fn declare_constructor(py: &PyToken<'_>, identity: PublicObjectId, class: u64) -> bool {
    let Some(spec) =
        identity.constructor(crate::object::ops_sys::runtime_target_at_least(py, 3, 13))
    else {
        raise_exception::<u64>(
            py,
            BuiltinExc::SystemError,
            "public type has no constructor declaration",
        );
        return false;
    };
    let signature = spec.signature;
    let Some(defaults) = super::registry::materialize_intrinsic_defaults(py, signature.defaults)
    else {
        return false;
    };
    let constructor = crate::builtins::methods::builtin_func_bits_with_signature(
        py,
        NativeCallableSpec::constructor(class),
        spec.callback,
        signature.arity,
        &defaults,
        signature.posonly,
        signature.positional,
        signature.vararg,
        signature.varkw,
    );
    if constructor == 0 {
        return false;
    }
    configure_keyword_only(py, constructor, signature)
}

pub(super) fn configure_keyword_only(
    py: &PyToken<'_>,
    constructor: u64,
    spec: PublicSignatureSpec,
) -> bool {
    if spec.kwonly.is_empty() {
        return true;
    }
    let mut names = Vec::new();
    for name in spec.kwonly {
        let pointer = alloc_string(py, name.as_bytes());
        if pointer.is_null() {
            for value in names {
                dec_ref_bits(py, value);
            }
            return false;
        }
        names.push(MoltObject::from_ptr(pointer).bits());
    }
    let names_tuple = alloc_tuple(py, &names);
    if names_tuple.is_null() {
        for value in names {
            dec_ref_bits(py, value);
        }
        return false;
    }
    let Some(defaults) = super::registry::materialize_intrinsic_defaults(py, spec.kwdefaults)
    else {
        for value in names {
            dec_ref_bits(py, value);
        }
        if !names_tuple.is_null() {
            dec_ref_bits(py, MoltObject::from_ptr(names_tuple).bits());
        }
        return false;
    };
    let pairs: Vec<_> = names
        .iter()
        .copied()
        .zip(defaults)
        .flat_map(|(name, value)| [name, value])
        .collect();
    let defaults_dict = alloc_dict_with_pairs(py, &pairs);
    for value in names {
        dec_ref_bits(py, value);
    }
    if names_tuple.is_null() || defaults_dict.is_null() {
        if !names_tuple.is_null() {
            dec_ref_bits(py, MoltObject::from_ptr(names_tuple).bits());
        }
        if !defaults_dict.is_null() {
            dec_ref_bits(py, MoltObject::from_ptr(defaults_dict).bits());
        }
        return false;
    }
    let names = MoltObject::from_ptr(names_tuple).bits();
    let defaults = MoltObject::from_ptr(defaults_dict).bits();
    let pointer = obj_from_bits(constructor).as_ptr().unwrap();
    let configured = unsafe {
        crate::call::class_init::function_set_attr_name(
            py,
            pointer,
            crate::NameId::DunderMoltKwonlyNames,
            names,
        ) && crate::call::class_init::function_set_attr_name(
            py,
            pointer,
            crate::NameId::DunderKwdefaults,
            defaults,
        )
    };
    dec_ref_bits(py, names);
    dec_ref_bits(py, defaults);
    configured && !exception_pending(py)
}

/// Prove the exact generated binder facts without Python equality or callbacks.
pub(super) unsafe fn signature_matches(
    py: &PyToken<'_>,
    function: *mut u8,
    spec: PublicSignatureSpec,
) -> bool {
    use crate::object::function_metadata::FunctionMetadataField as Field;
    unsafe {
        if Field::BindKind.load(function).is_some()
            || !super::registry::function_defaults_are(py, function, spec.defaults)
            || !tuple_names_are(Field::ArgumentNames.load(function), spec.positional, false)
            || !tuple_names_are(Field::KeywordOnlyNames.load(function), spec.kwonly, true)
            || Field::PositionalOnly
                .load(function)
                .map_or(spec.posonly != 0, |bits| {
                    obj_from_bits(bits).as_int() != Some(spec.posonly as i64)
                })
        {
            return false;
        }
        for (field, enabled) in [
            (Field::Varargs, spec.vararg),
            (Field::VarKeywords, spec.varkw),
        ] {
            let bits = field.load(function).unwrap_or(MoltObject::none().bits());
            if bits
                != if enabled {
                    MoltObject::from_bool(true).bits()
                } else {
                    MoltObject::none().bits()
                }
            {
                return false;
            }
        }
        let keywords = Field::KeywordDefaults
            .load(function)
            .unwrap_or(MoltObject::none().bits());
        if spec.kwonly.is_empty() {
            return obj_from_bits(keywords).is_none();
        }
        let Some(dictionary) = obj_from_bits(keywords).as_ptr() else {
            return false;
        };
        if !crate::object::object_is_exact_builtin_dict(py, dictionary)
            || crate::dict_len(dictionary) != spec.kwonly.len()
        {
            return false;
        }
        spec.kwonly
            .iter()
            .zip(spec.kwdefaults)
            .all(|(name, &default)| {
                let hash = hash_string_bytes(py, name.as_bytes()) as u64;
                match dict_exact_string_lookup(py, dictionary, name.as_bytes(), hash) {
                    ExactStringLookup::Found(index) => {
                        crate::dict_entries(dictionary)[index].value
                            == super::registry::intrinsic_default_bits(py, default)
                    }
                    _ => false,
                }
            })
            && !exception_pending(py)
    }
}

unsafe fn tuple_names_are<T: AsRef<[u8]>>(
    bits: Option<u64>,
    names: &[T],
    absent_empty: bool,
) -> bool {
    let bits = bits.unwrap_or(MoltObject::none().bits());
    if obj_from_bits(bits).is_none() {
        return absent_empty && names.is_empty();
    }
    let Some(tuple) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(tuple) != TYPE_ID_TUPLE {
            return false;
        }
        crate::object::seq_access::with_immutable_tuple_slice(tuple, |values| {
            values.len() == names.len()
                && values.iter().zip(names).all(|(&bits, name)| {
                    crate::object::ops_format::with_string_bytes(obj_from_bits(bits), |bytes| {
                        bytes == name.as_ref()
                    }) == Some(true)
                })
        })
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod direct_call_tests {
    use super::super::public_objects_generated::{DIRECT_CALL_PUBLIC_OBJECTS, PublicObjectKind};

    /// The compiler replaces a call of each listed object with exactly its
    /// intrinsic's positional arity by a direct call of the intrinsic. That is
    /// sound only while calling the object binds its positional words straight
    /// to its entry: a declared call signature would bind keywords, defaults or
    /// varargs first, so no member may carry one on any target version.
    #[test]
    fn direct_call_public_objects_declare_no_call_signature() {
        assert!(!DIRECT_CALL_PUBLIC_OBJECTS.is_empty());
        for &id in DIRECT_CALL_PUBLIC_OBJECTS {
            assert_eq!(
                id.identity().kind,
                PublicObjectKind::BuiltinFunctionOrMethod,
                "{id:?}"
            );
            for python_314 in [false, true] {
                assert!(
                    id.call_signature(python_314).is_none(),
                    "{id:?} declares a call signature (python_314={python_314})"
                );
            }
        }
    }
}
