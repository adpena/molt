use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, builtin_variadic_func_bits,
};
use crate::PyToken;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(staticmethod_method_bits, publish_staticmethod_methods, _py, name, [], {


}, {
        "__new__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).staticmethod).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_type_new),
            )),
        "__init__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_init),
            )),
        "__get__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, "__get__").with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_staticmethod_get),
            )),
        "__call__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).staticmethod, "__call__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_staticmethod_call),
            )),
});

crate::builtins::methods::native_method_table!(classmethod_method_bits, publish_classmethod_methods, _py, name, [], {


}, {
        "__new__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).classmethod).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_classmethod_type_new),
            )),
        "__init__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).classmethod, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_classmethod_init),
            )),
        "__get__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).classmethod, "__get__").with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_classmethod_get),
            )),
});

crate::builtins::methods::native_method_table!(property_method_bits, publish_property_methods, _py, name, [], {


}, {
        "__new__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::constructor(builtin_classes(_py).property).with_text_signature("($type, *args, **kwargs)"),
                fn_addr!(molt_property_type_new),
            )),
        "__init__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_property_init),
            )),
        "__get__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, "__get__").with_text_signature("($self, instance, owner=None, /)"),
                fn_addr!(molt_property_get),
            )),
        "__set__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, "__set__").with_text_signature("($self, instance, value, /)"),
                fn_addr!(molt_property_set),
            )),
        "__delete__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).property, "__delete__").with_text_signature("($self, instance, /)"),
                fn_addr!(molt_property_delete),
            )),
        "__set_name__" => Some(builtin_variadic_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, "__set_name__"),
                fn_addr!(molt_property_set_name),
            )),
        "getter" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, "getter"),
                fn_addr!(molt_property_getter),
                2,
            )),
        "setter" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, "setter"),
                fn_addr!(molt_property_setter),
                2,
            )),
        "deleter" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).property, "deleter"),
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
        $( $name:literal => ($kind:ident, $symbol:path, $arity:tt) ),+ $(,)?
    }) => {
        const $names: &[&str] = &[$($name),+];
        pub(crate) fn $lookup(py: &PyToken<'_>, name: &str) -> Option<u64> {
            super::method_dispatch(py, || match name {
                $( $name => Some(suspension_methods!(@bind py,
                    NativeCallableSpec::declared(NativeCallableKind::$kind, builtin_classes(py).$owner, $name),
                    $symbol, $arity)), )+
                _ => None,
            })
        }
        pub(crate) fn $publish(py: &PyToken<'_>) -> bool {
            for name in $names {
                let _ = $lookup(py, name);
                if exception_pending(py) { return false; }
            }
            true
        }
    };
}

suspension_methods!(generator_method_bits, GENERATOR_METHOD_NAMES, publish_generator_methods, generator, {
    "__iter__" => (WrapperDescriptor, crate::object::ops_iter::builtin_iter_slot, 1),
    "__next__" => (WrapperDescriptor, molt_generator_next_method, 1),
    "send" => (MethodDescriptor, molt_generator_send_method, 2),
    "throw" => (MethodDescriptor, molt_generator_throw_method, variadic),
    "close" => (MethodDescriptor, molt_generator_close_method, 1),
});
suspension_methods!(coroutine_method_bits, COROUTINE_METHOD_NAMES, publish_coroutine_methods, coroutine, {
    "__await__" => (WrapperDescriptor, molt_awaitable_await, 1),
    "send" => (MethodDescriptor, molt_coroutine_send_method, 2),
    "throw" => (MethodDescriptor, molt_coroutine_throw_method, variadic),
    "close" => (MethodDescriptor, molt_coroutine_close_method, 1),
});
suspension_methods!(coroutine_wrapper_method_bits, COROUTINE_WRAPPER_METHOD_NAMES, publish_coroutine_wrapper_methods, coroutine_wrapper, {
    "__iter__" => (WrapperDescriptor, molt_coroutine_wrapper_iter, 1),
    "__next__" => (WrapperDescriptor, molt_coroutine_wrapper_next, 1),
    "send" => (MethodDescriptor, molt_coroutine_send_method, 2),
    "throw" => (MethodDescriptor, molt_coroutine_throw_method, variadic),
    "close" => (MethodDescriptor, molt_coroutine_close_method, 1),
});
// The async-generator operation awaitables share one protocol implementation;
// each class keeps its own descriptors, as CPython's two types do.
suspension_methods!(asyncgen_asend_method_bits, ASYNCGEN_ASEND_METHOD_NAMES, publish_asyncgen_asend_methods, async_generator_asend, {
    "__await__" => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    "__iter__" => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    "__next__" => (WrapperDescriptor, molt_asyncgen_awaitable_next, 1),
    "send" => (MethodDescriptor, molt_asyncgen_awaitable_send, 2),
    "throw" => (MethodDescriptor, molt_asyncgen_awaitable_throw, variadic),
    "close" => (MethodDescriptor, molt_asyncgen_awaitable_close, 1),
});
suspension_methods!(asyncgen_athrow_method_bits, ASYNCGEN_ATHROW_METHOD_NAMES, publish_asyncgen_athrow_methods, async_generator_athrow, {
    "__await__" => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    "__iter__" => (WrapperDescriptor, molt_asyncgen_awaitable_self, 1),
    "__next__" => (WrapperDescriptor, molt_asyncgen_awaitable_next, 1),
    "send" => (MethodDescriptor, molt_asyncgen_awaitable_send, 2),
    "throw" => (MethodDescriptor, molt_asyncgen_awaitable_throw, variadic),
    "close" => (MethodDescriptor, molt_asyncgen_awaitable_close, 1),
});

crate::builtins::methods::native_method_table!(asyncgen_method_bits, publish_asyncgen_methods, _py, name, [], {

}, {
        "__aiter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).async_generator, "__aiter__").with_text_signature("($self, /)"),
            fn_addr!(molt_asyncgen_aiter),
            1,
        )),
        "__anext__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).async_generator, "__anext__").with_text_signature("($self, /)"),
            fn_addr!(molt_asyncgen_anext),
            1,
        )),
        "asend" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, "asend"),
            fn_addr!(molt_asyncgen_asend),
            2,
        )),
        "athrow" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, "athrow"),
            fn_addr!(molt_asyncgen_athrow),
            2,
        )),
        "aclose" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).async_generator, "aclose"),
            fn_addr!(molt_asyncgen_aclose),
            1,
        )),
});

crate::builtins::methods::native_method_table!(weakref_method_bits, publish_weakref_methods, _py, name, [], {


}, {
        "__init__" => Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_weakref_init),
                3,
                &[MoltObject::none().bits()],
            )),
        "__call__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__call__").with_text_signature("($self, /, *args, **kwargs)"),
                fn_addr!(molt_weakref_call),
                1,
            )),
        "__eq__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__eq__").with_text_signature("($self, value, /)"),
                fn_addr!(molt_weakref_eq),
                2,
            )),
        "__ne__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__ne__").with_text_signature("($self, value, /)"),
                fn_addr!(molt_weakref_ne),
                2,
            )),
        "__repr__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__repr__").with_text_signature("($self, /)"),
                fn_addr!(molt_weakref_repr),
                1,
            )),
        "__hash__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).reference_type, "__hash__").with_text_signature("($self, /)"),
                fn_addr!(molt_weakref_hash),
                1,
            )),
});

crate::builtins::methods::native_method_table!(generic_alias_method_bits, publish_generic_alias_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::GenericAlias, {
}, {
    "__mro_entries__" => Some(builtin_func_bits(
        py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).generic_alias, "__mro_entries__"),
        fn_addr!(molt_generic_alias_mro_entries), 2,
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
        extern "C" fn length(view: u64) -> u64 {
            crate::with_gil_entry_nopanic!(py, {
                let Some(pointer) = obj_from_bits(view)
                    .as_ptr()
                    .filter(|pointer| unsafe { object_type_id(*pointer) } == $kind)
                else {
                    return raise_exception::<_>(
                        py,
                        "TypeError",
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
                "__len__",
            )
            .with_text_signature("($self, /)"),
            fn_addr!(length),
            1,
        ))
    }};
}

crate::builtins::methods::native_method_table!(
    dict_values_method_bits, publish_dict_values_methods, py, name, [], {}, {
        "__len__" => dict_view_length_descriptor!(py, dict_values, TYPE_ID_DICT_VALUES_VIEW),
    }
);

crate::builtins::methods::native_method_table!(
    dict_keys_method_bits, publish_dict_keys_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::DictKeys,
    {}, {
        "__or__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__or__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_or_slot), 2)),
        "__ror__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__ror__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_ror_slot), 2)),
        "__and__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__and__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_and_slot), 2)),
        "__rand__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__rand__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rand_slot), 2)),
        "__sub__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_sub_slot), 2)),
        "__rsub__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rsub_slot), 2)),
        "__xor__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__xor__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_xor_slot), 2)),
        "__rxor__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_keys, "__rxor__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rxor_slot), 2)),
        "__len__" => dict_view_length_descriptor!(py, dict_keys, TYPE_ID_DICT_KEYS_VIEW),
        "__contains__" => {
            extern "C" fn contains(view: u64, item: u64) -> u64 {
                crate::with_gil_entry_nopanic!(py, {
                    crate::object::ops_compare::builtin_families::BuiltinComparison::DictKeys
                        .invoke_contains(py, view, item)
                })
            }
            Some(builtin_func_bits(py,
                NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                    builtin_classes(py).dict_keys, "__contains__")
                    .with_text_signature("($self, key, /)"),
                fn_addr!(contains), 2))
        },
    }
);

crate::builtins::methods::native_method_table!(
    dict_items_method_bits, publish_dict_items_methods, py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::DictItems,
    {}, {
        "__or__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__or__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_or_slot), 2)),
        "__ror__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__ror__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_ror_slot), 2)),
        "__and__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__and__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_and_slot), 2)),
        "__rand__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__rand__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rand_slot), 2)),
        "__sub__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_sub_slot), 2)),
        "__rsub__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rsub_slot), 2)),
        "__xor__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__xor__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_xor_slot), 2)),
        "__rxor__" => Some(builtin_func_bits(py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                builtin_classes(py).dict_items, "__rxor__"),
            fn_addr!(crate::object::ops_arith::native_slots::view_rxor_slot), 2)),
        "__len__" => dict_view_length_descriptor!(py, dict_items, TYPE_ID_DICT_ITEMS_VIEW),
        "__contains__" => {
            extern "C" fn contains(view: u64, item: u64) -> u64 {
                crate::with_gil_entry_nopanic!(py, {
                    crate::object::ops_compare::builtin_families::BuiltinComparison::DictItems
                        .invoke_contains(py, view, item)
                })
            }
            Some(builtin_func_bits(py,
                NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor,
                    builtin_classes(py).dict_items, "__contains__")
                    .with_text_signature("($self, key, /)"),
                fn_addr!(contains), 2))
        },
    }
);

// Each public iterator class publishes its actual descriptor owner through the
// existing declaration macro. The method body shares the iterator state owner.
crate::builtins::methods::native_method_table!(dict_keyiterator_method_bits, publish_dict_keyiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_keyiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_valueiterator_method_bits, publish_dict_valueiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_valueiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_itemiterator_method_bits, publish_dict_itemiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_itemiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reversekeyiterator_method_bits, publish_dict_reversekeyiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reversekeyiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reversevalueiterator_method_bits, publish_dict_reversevalueiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reversevalueiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(dict_reverseitemiterator_method_bits, publish_dict_reverseitemiterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).dict_reverseitemiterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
crate::builtins::methods::native_method_table!(set_iterator_method_bits, publish_set_iterator_methods, py, name, [], {
}, {
    "__length_hint__" => Some(builtin_func_bits(py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(py).set_iterator, "__length_hint__"),
        fn_addr!(crate::object::ops_iter::hash_iterator_length_hint), 1)),
});
