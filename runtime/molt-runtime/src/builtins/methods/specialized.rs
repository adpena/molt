use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, builtin_variadic_func_bits,
};
use crate::BuiltinExc;
use crate::PyToken;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(staticmethod_method_bits, publish_staticmethod_methods, _py, name, [], {


}, {
        crate::NameId::DunderNew => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).staticmethod).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_type_new),
            )),
        crate::NameId::DunderInit => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_init),
            )),
        crate::NameId::DunderGet => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, crate::NameId::DunderGet).with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_staticmethod_get),
            )),
        crate::NameId::DunderCall => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, crate::NameId::DunderCall).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_call),
            )),
});

crate::builtins::methods::native_method_table!(classmethod_method_bits, publish_classmethod_methods, _py, name, [], {


}, {
        crate::NameId::DunderNew => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).classmethod).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_classmethod_type_new),
            )),
        crate::NameId::DunderInit => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).classmethod, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_classmethod_init),
            )),
        crate::NameId::DunderGet => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).classmethod, crate::NameId::DunderGet).with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_classmethod_get),
            )),
});

crate::builtins::methods::native_method_table!(property_method_bits, publish_property_methods, _py, name, [], {


}, {
        crate::NameId::DunderNew => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).property).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_property_type_new),
            )),
        crate::NameId::DunderInit => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_property_init),
            )),
        crate::NameId::DunderGet => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, crate::NameId::DunderGet).with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_property_get),
            )),
        crate::NameId::DunderSet => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, crate::NameId::DunderSet).with_text_signature("($self, instance, value, /)"),
                fn_addr!(molt_property_set),
            )),
        crate::NameId::DunderDelete => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, crate::NameId::DunderDelete).with_text_signature("($self, instance, /)"),
                fn_addr!(molt_property_delete),
            )),
        crate::NameId::DunderSetName => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, crate::NameId::DunderSetName),
                fn_addr!(molt_property_set_name),
            )),
        crate::NameId::Getter => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, crate::NameId::Getter),
                fn_addr!(molt_property_getter),
                2,
            )),
        crate::NameId::Setter => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, crate::NameId::Setter),
                fn_addr!(molt_property_setter),
                2,
            )),
        crate::NameId::Deleter => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, crate::NameId::Deleter),
                fn_addr!(molt_property_deleter),
                2,
            )),
});

// One declaration supplies callable lookup and dir() for the suspended
// execution families. The ordinary MethodCache remains the sole callable owner.
macro_rules! suspension_methods {
    (@bind $py:ident, $spec:expr, $symbol:path, variadic) => {
        builtin_variadic_func_bits($py, $spec, fn_addr!($symbol))
    };
    (@bind $py:ident, $spec:expr, $symbol:path, $arity:literal) => {
        builtin_func_bits($py, $spec, fn_addr!($symbol), $arity)
    };
    ($lookup:ident, $names:ident, $publish:ident, $owner:ident, {
        $( $name:path => ($kind:ident, $symbol:path, $arity:tt) ),+ $(,)?
    }) => {
        const $names: &[crate::NameId] = &[$($name),+];
        pub(crate) fn $lookup(py: &PyToken<'_>, name: crate::NameId) -> Result<Option<u64>, ()> {
            super::method_dispatch(py, || Ok(match name {
                $( $name => Some(suspension_methods!(@bind py,
                    NativeCallableSpec::declared(NativeCallableKind::$kind, builtin_classes(py).$owner, $name),
                    $symbol, $arity)), )+
                _ => None,
            }))
        }
        pub(crate) fn $publish(py: &PyToken<'_>) -> bool {
            for &name in $names {
                if $lookup(py, name).is_err() { return false; }
            }
            true
        }
    };
}

suspension_methods!(generator_method_bits, GENERATOR_METHOD_NAMES, publish_generator_methods, generator, {
    crate::NameId::DunderIter => (WrapperDescriptor, crate::object::ops_iter::builtin_iter_slot, 1),
    crate::NameId::DunderNext => (WrapperDescriptor, molt_generator_next_method, 1),
    crate::NameId::Send => (MethodDescriptor, molt_generator_send_method, 2),
    crate::NameId::Throw => (MethodDescriptor, molt_generator_throw_method, variadic),
    crate::NameId::Close => (MethodDescriptor, molt_generator_close_method, 1),
});
suspension_methods!(coroutine_method_bits, COROUTINE_METHOD_NAMES, publish_coroutine_methods, coroutine, {
    crate::NameId::DunderAwait => (WrapperDescriptor, molt_awaitable_await, 1),
    crate::NameId::Send => (MethodDescriptor, molt_coroutine_send_method, 2),
    crate::NameId::Throw => (MethodDescriptor, molt_coroutine_throw_method, variadic),
    crate::NameId::Close => (MethodDescriptor, molt_coroutine_close_method, 1),
});
suspension_methods!(coroutine_wrapper_method_bits, COROUTINE_WRAPPER_METHOD_NAMES, publish_coroutine_wrapper_methods, coroutine_wrapper, {
    crate::NameId::DunderIter => (WrapperDescriptor, molt_coroutine_wrapper_iter, 1),
    crate::NameId::DunderNext => (WrapperDescriptor, molt_coroutine_wrapper_next, 1),
    crate::NameId::Send => (MethodDescriptor, molt_coroutine_send_method, 2),
    crate::NameId::Throw => (MethodDescriptor, molt_coroutine_throw_method, variadic),
    crate::NameId::Close => (MethodDescriptor, molt_coroutine_close_method, 1),
});
// The async-generator operation awaitables share one protocol implementation;
// each class keeps its own descriptors, as CPython's two types do.
suspension_methods!(asyncgen_asend_method_bits, ASYNCGEN_ASEND_METHOD_NAMES, publish_asyncgen_asend_methods, async_generator_asend, {
    crate::NameId::DunderAwait => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    crate::NameId::DunderIter => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    crate::NameId::DunderNext => (WrapperDescriptor, molt_asyncgen_awaitable_next, 1),
    crate::NameId::Send => (MethodDescriptor, molt_asyncgen_awaitable_send, 2),
    crate::NameId::Throw => (MethodDescriptor, molt_asyncgen_awaitable_throw, variadic),
    crate::NameId::Close => (MethodDescriptor, molt_asyncgen_awaitable_close, 1),
});
suspension_methods!(asyncgen_athrow_method_bits, ASYNCGEN_ATHROW_METHOD_NAMES, publish_asyncgen_athrow_methods, async_generator_athrow, {
    crate::NameId::DunderAwait => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    crate::NameId::DunderIter => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    crate::NameId::DunderNext => (WrapperDescriptor, molt_asyncgen_awaitable_next, 1),
    crate::NameId::Send => (MethodDescriptor, molt_asyncgen_awaitable_send, 2),
    crate::NameId::Throw => (MethodDescriptor, molt_asyncgen_awaitable_throw, variadic),
    crate::NameId::Close => (MethodDescriptor, molt_asyncgen_awaitable_close, 1),
});

crate::builtins::methods::native_method_table!(asyncgen_method_bits, publish_asyncgen_methods, _py, name, [], {

}, {
        crate::NameId::DunderAiter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).async_generator, crate::NameId::DunderAiter).with_text_signature("($self, /)"),
            fn_addr!(molt_asyncgen_aiter),
            1,
        )),
        crate::NameId::DunderAnext => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).async_generator, crate::NameId::DunderAnext).with_text_signature("($self, /)"),
            fn_addr!(molt_asyncgen_anext),
            1,
        )),
        crate::NameId::Asend => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, crate::NameId::Asend),
            fn_addr!(molt_asyncgen_asend),
            2,
        )),
        crate::NameId::Athrow => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, crate::NameId::Athrow),
            fn_addr!(molt_asyncgen_athrow),
            2,
        )),
        crate::NameId::Aclose => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, crate::NameId::Aclose),
            fn_addr!(molt_asyncgen_aclose),
            1,
        )),
});

crate::builtins::methods::native_method_table!(weakref_method_bits, publish_weakref_methods, _py, name, [], {


}, {
        crate::NameId::DunderInit => Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_weakref_init),
                3,
                &[MoltObject::none().bits()],
            )),
        crate::NameId::DunderCall => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderCall).with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_weakref_call),
                1,
            )),
        crate::NameId::DunderEq => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderEq).with_text_signature("($self, value, /)"),
                fn_addr!(molt_weakref_eq),
                2,
            )),
        crate::NameId::DunderNe => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderNe).with_text_signature("($self, value, /)"),
                fn_addr!(molt_weakref_ne),
                2,
            )),
        crate::NameId::DunderRepr => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderRepr).with_text_signature("($self, /)"),
                fn_addr!(molt_weakref_repr),
                1,
            )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, crate::NameId::DunderHash).with_text_signature("($self, /)"),
                fn_addr!(molt_weakref_hash),
                1,
            )),
});

crate::builtins::methods::native_method_table!(generic_alias_method_bits, publish_generic_alias_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::GenericAlias, {
}, {
    crate::NameId::DunderMroEntries => Some(builtin_func_bits(
        py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).generic_alias, crate::NameId::DunderMroEntries),
        fn_addr!(molt_generic_alias_mro_entries), 2,
    )),
    crate::NameId::DunderInstancecheck => Some(builtin_func_bits(
        py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).generic_alias, crate::NameId::DunderInstancecheck),
        fn_addr!(crate::builtins::types::descriptor_objects::generic_alias_instancecheck), 2,
    )),
    crate::NameId::DunderSubclasscheck => Some(builtin_func_bits(
        py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).generic_alias, crate::NameId::DunderSubclasscheck),
        fn_addr!(crate::builtins::types::descriptor_objects::generic_alias_subclasscheck), 2,
    )),
});

crate::builtins::methods::native_method_table!(
    union_method_bits, publish_union_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Union,
    {}, {}
);

// Native view length descriptors share the existing live dictionary-storage
// owner. Each expansion retains its exact declaring class and receiver kind;
// neither a Python __len__ lookup nor mapping admission supplies the result.
macro_rules! dict_view_length_descriptor {
    ($py:ident, $owner:ident, $kind:ident) => {{
        extern "C" fn length(__context: u64, view: u64) -> u64 {
            let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

            crate::with_attached_entry_nopanic!(__attached_py, py, {
                let Some(pointer) = obj_from_bits(view)
                    .as_ptr()
                    .filter(|pointer| unsafe { object_type_id(*pointer) } == $kind)
                else {
                    return raise_exception::<_>(
                        py,
                        BuiltinExc::TypeError,
                        &format!(
                            "descriptor '__len__' requires a '{}' object but received a '{}'",
                            stringify!($owner),
                            type_name(py, obj_from_bits(view)),
                        ),
                    );
                };
                MoltObject::from_int(
                    unsafe { crate::builtins::containers::dict_view_len(pointer) } as i64,
                )
                .bits()
            })
        }
        Some(builtin_func_bits(
            $py,
            NativeCallableSpec::declared(
                NativeCallableKind::WrapperDescriptor,
                builtin_classes($py).$owner,
                crate::NameId::DunderLen,
            )
            .with_text_signature("($self, /)"),
            fn_addr!(length),
            1,
        ))
    }};
}

crate::builtins::methods::native_method_table!(
    dict_values_method_bits, publish_dict_values_methods, py, name, [], {}, {
        crate::NameId::DunderLen => dict_view_length_descriptor!(py, dict_values, TYPE_ID_DICT_VALUES_VIEW),
    }
);

crate::builtins::methods::native_method_table!(
    dict_keys_method_bits, publish_dict_keys_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::DictKeys,
    {}, {
        crate::NameId::DunderOr => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderOr),
            fn_addr!(crate::object::ops_arith::native_slots::view_or_slot), 2)),
        crate::NameId::DunderRor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderRor),
            fn_addr!(crate::object::ops_arith::native_slots::view_ror_slot), 2)),
        crate::NameId::DunderAnd => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderAnd),
            fn_addr!(crate::object::ops_arith::native_slots::view_and_slot), 2)),
        crate::NameId::DunderRand => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderRand),
            fn_addr!(crate::object::ops_arith::native_slots::view_rand_slot), 2)),
        crate::NameId::DunderSub => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::view_sub_slot), 2)),
        crate::NameId::DunderRsub => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::view_rsub_slot), 2)),
        crate::NameId::DunderXor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderXor),
            fn_addr!(crate::object::ops_arith::native_slots::view_xor_slot), 2)),
        crate::NameId::DunderRxor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderRxor),
            fn_addr!(crate::object::ops_arith::native_slots::view_rxor_slot), 2)),
        crate::NameId::DunderLen => dict_view_length_descriptor!(py, dict_keys, TYPE_ID_DICT_KEYS_VIEW),
        crate::NameId::DunderContains => {
            extern "C" fn contains(__context: u64, view: u64, item: u64) -> u64 {
 let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

                crate::with_attached_entry_nopanic!(__attached_py, py, {
                    crate::object::ops_compare::builtin_families::BuiltinComparison::DictKeys
                        .invoke_contains(py, view, item)
                })
            }
            Some(builtin_func_bits(py,
                NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_keys, crate::NameId::DunderContains)
                    .with_text_signature("($self, key, /)"),
                fn_addr!(contains), 2))
        },
    }
);

crate::builtins::methods::native_method_table!(
    dict_items_method_bits, publish_dict_items_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::DictItems,
    {}, {
        crate::NameId::DunderOr => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderOr),
            fn_addr!(crate::object::ops_arith::native_slots::view_or_slot), 2)),
        crate::NameId::DunderRor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderRor),
            fn_addr!(crate::object::ops_arith::native_slots::view_ror_slot), 2)),
        crate::NameId::DunderAnd => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderAnd),
            fn_addr!(crate::object::ops_arith::native_slots::view_and_slot), 2)),
        crate::NameId::DunderRand => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderRand),
            fn_addr!(crate::object::ops_arith::native_slots::view_rand_slot), 2)),
        crate::NameId::DunderSub => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::view_sub_slot), 2)),
        crate::NameId::DunderRsub => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::view_rsub_slot), 2)),
        crate::NameId::DunderXor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderXor),
            fn_addr!(crate::object::ops_arith::native_slots::view_xor_slot), 2)),
        crate::NameId::DunderRxor => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderRxor),
            fn_addr!(crate::object::ops_arith::native_slots::view_rxor_slot), 2)),
        crate::NameId::DunderLen => dict_view_length_descriptor!(py, dict_items, TYPE_ID_DICT_ITEMS_VIEW),
        crate::NameId::DunderContains => {
            extern "C" fn contains(__context: u64, view: u64, item: u64) -> u64 {
 let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

                crate::with_attached_entry_nopanic!(__attached_py, py, {
                    crate::object::ops_compare::builtin_families::BuiltinComparison::DictItems
                        .invoke_contains(py, view, item)
                })
            }
            Some(builtin_func_bits(py,
                NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(py).dict_items, crate::NameId::DunderContains)
                    .with_text_signature("($self, key, /)"),
                fn_addr!(contains), 2))
        },
    }
);

// Each public iterator class publishes its actual descriptor owner through the
// existing declaration macro. The method body shares the iterator state owner.
crate::builtins::methods::native_method_table!(dict_keyiterator_method_bits, publish_dict_keyiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_keyiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_valueiterator_method_bits, publish_dict_valueiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_valueiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_itemiterator_method_bits, publish_dict_itemiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_itemiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reversekeyiterator_method_bits, publish_dict_reversekeyiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reversekeyiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reversevalueiterator_method_bits, publish_dict_reversevalueiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reversevalueiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reverseitemiterator_method_bits, publish_dict_reverseitemiterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reverseitemiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(set_iterator_method_bits, publish_set_iterator_methods, py, name, [], {
}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).set_iterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});

crate::builtins::methods::native_method_table!(list_iterator_method_bits, publish_list_iterator_methods, py, name, [], {}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).list_iterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::list_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(list_reverseiterator_method_bits, publish_list_reverseiterator_methods, py, name, [], {}, {
    crate::NameId::DunderLengthHint => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).list_reverseiterator, crate::NameId::DunderLengthHint),
        fn_addr!(crate::object::ops_iter::list_iterator_length_hint), 1)),
});
