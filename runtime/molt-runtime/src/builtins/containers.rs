use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::{
    BIND_KIND_PACKED_BUILTIN, HashContext, MoltObject, PyToken, TYPE_ID_DICT,
    TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_FROZENSET, TYPE_ID_LIST_BOOL,
    TYPE_ID_LIST_INT, TYPE_ID_SET, alloc_tuple, builtin_classes, builtin_func_bits,
    builtin_func_bits_with_bind_kind, builtin_func_bits_with_defaults_tuple, dec_ref_bits,
    dict_clear_method, dict_copy_method, dict_fromkeys_method, dict_get_method, dict_items_method,
    dict_keys_method, dict_popitem_method, dict_setdefault_method, dict_update_method,
    dict_values_method, exception_pending, molt_dict_pop_method, molt_frozenset_copy_method,
    molt_frozenset_difference_multi, molt_frozenset_intersection_multi, molt_frozenset_isdisjoint,
    molt_frozenset_issubset, molt_frozenset_issuperset, molt_frozenset_symmetric_difference,
    molt_frozenset_union_multi, molt_list_add_method, molt_list_append, molt_list_clear,
    molt_list_copy, molt_list_count, molt_list_extend, molt_list_imul_method,
    molt_list_index_range, molt_list_insert, molt_list_mul_method, molt_list_pop, molt_list_remove,
    molt_list_reverse, molt_list_sort, molt_reversed_builtin, molt_set_add, molt_set_clear,
    molt_set_copy_method, molt_set_difference_multi, molt_set_difference_update_multi,
    molt_set_discard, molt_set_intersection_multi, molt_set_intersection_update_multi,
    molt_set_isdisjoint, molt_set_issubset, molt_set_issuperset, molt_set_new, molt_set_pop,
    molt_set_remove, molt_set_symmetric_difference, molt_set_symmetric_difference_update,
    molt_set_union_multi, molt_set_update_multi, molt_tuple_count, molt_tuple_index_range,
    obj_from_bits, object_type_id, set_add_in_place,
};

pub(crate) fn is_set_like_type(type_id: u32) -> bool {
    type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET
}

pub(crate) fn is_set_inplace_rhs_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_SET | TYPE_ID_FROZENSET | TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW
    )
}

pub(crate) fn is_set_view_type(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW)
}

crate::builtins::methods::native_method_table!(dict_method_bits, publish_dict_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Dict, {

}, {

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).dict).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::dict_new),
        )),
        "__init__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::dict_init),
        )),
        "keys" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "keys"),
            fn_addr!(dict_keys_method),
            1,
        )),
        "values" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "values"),
            fn_addr!(dict_values_method),
            1,
        )),
        "items" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "items"),
            fn_addr!(dict_items_method),
            1,
        )),
        "get" => {
            if cfg!(target_arch = "wasm32")
                && std::env::var("MOLT_WASM_DICT_METHOD_DEBUG").as_deref() == Ok("1")
            {
                eprintln!(
                    "molt wasm dict_method:get fn=0x{:x}",
                    fn_addr!(dict_get_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "get").with_text_signature("($self, key, default=None, /)"),
                fn_addr!(dict_get_method),
                3,
                &[none],
            ))
        },
        "pop" => {
            let miss = crate::builtins::methods::missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "pop").with_text_signature("($self, key, default=<unrepresentable>, /)"),
                fn_addr!(molt_dict_pop_method),
                3,
                &[miss],
            ))
        },
        "clear" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "clear"),
            fn_addr!(dict_clear_method),
            1,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "copy"),
            fn_addr!(dict_copy_method),
            1,
        )),
        "popitem" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "popitem").with_text_signature("($self, /)"),
            fn_addr!(dict_popitem_method),
            1,
        )),
        "setdefault" => {
            if cfg!(target_arch = "wasm32")
                && std::env::var("MOLT_WASM_DICT_METHOD_DEBUG").as_deref() == Ok("1")
            {
                eprintln!(
                    "molt wasm dict_method:setdefault fn=0x{:x}",
                    fn_addr!(dict_setdefault_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "setdefault").with_text_signature("($self, key, default=None, /)"),
                fn_addr!(dict_setdefault_method),
                3,
                &[none],
            ))
        },
        "update" => {
            let miss = crate::builtins::methods::missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "update"),
                fn_addr!(dict_update_method),
                2,
                &[miss],
            ))
        },
        "fromkeys" => {
            if cfg!(target_arch = "wasm32")
                && std::env::var("MOLT_WASM_DICT_METHOD_DEBUG").as_deref() == Ok("1")
            {
                eprintln!(
                    "molt wasm dict_method:fromkeys fn=0x{:x}",
                    fn_addr!(dict_fromkeys_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).dict, "fromkeys").with_text_signature("($type, iterable, value=None, /)"),
                fn_addr!(dict_fromkeys_method),
                3,
                &[none],
            ))
        },
        "__getitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "__getitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, "__setitem__").with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, "__delitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "__contains__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, "__reversed__").with_text_signature("($self, /)"),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
});

crate::builtins::methods::native_method_table!(set_method_bits, publish_set_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Set, {

}, {
        "__or__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__or__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_or_slot), 2,
        )),
        "__ror__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__ror__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_ror_slot), 2,
        )),
        "__and__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__and__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_and_slot), 2,
        )),
        "__rand__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__rand__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rand_slot), 2,
        )),
        "__sub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_sub_slot), 2,
        )),
        "__rsub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rsub_slot), 2,
        )),
        "__xor__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__xor__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_xor_slot), 2,
        )),
        "__rxor__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__rxor__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rxor_slot), 2,
        )),
        "__ior__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__ior__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_ior_slot), 2,
        )),
        "__iand__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__iand__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_iand_slot), 2,
        )),
        "__isub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__isub__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_isub_slot), 2,
        )),
        "__ixor__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__ixor__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_ixor_slot), 2,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).set).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::set_new),
        )),
        "__init__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::set_init),
        )),
        "add" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "add"),
            fn_addr!(molt_set_add),
            2,
        )),
        "discard" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "discard"),
            fn_addr!(molt_set_discard),
            2,
        )),
        "remove" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "remove"),
            fn_addr!(molt_set_remove),
            2,
        )),
        "pop" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "pop"),
            fn_addr!(molt_set_pop),
            1,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "clear"),
            fn_addr!(molt_set_clear),
            1,
        )),
        "update" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "update"),
            fn_addr!(molt_set_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "union" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "union"),
            fn_addr!(molt_set_union_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "intersection" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "intersection"),
            fn_addr!(molt_set_intersection_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "difference" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "difference"),
            fn_addr!(molt_set_difference_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "symmetric_difference" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "symmetric_difference"),
            fn_addr!(molt_set_symmetric_difference),
            2,
        )),
        "intersection_update" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "intersection_update"),
            fn_addr!(molt_set_intersection_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "difference_update" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "difference_update"),
            fn_addr!(molt_set_difference_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "symmetric_difference_update" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "symmetric_difference_update"),
            fn_addr!(molt_set_symmetric_difference_update),
            2,
        )),
        "isdisjoint" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "isdisjoint"),
            fn_addr!(molt_set_isdisjoint),
            2,
        )),
        "issubset" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "issubset").with_text_signature("($self, other, /)"),
            fn_addr!(molt_set_issubset),
            2,
        )),
        "issuperset" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "issuperset").with_text_signature("($self, other, /)"),
            fn_addr!(molt_set_issuperset),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "copy"),
            fn_addr!(molt_set_copy_method),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, "__contains__"),
            fn_addr!(crate::object::ops_set::molt_set_contains),
            2,
        )),
});

crate::builtins::methods::native_method_table!(frozenset_method_bits, publish_frozenset_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Frozenset, {

}, {
        "__or__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__or__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_or_slot), 2,
        )),
        "__ror__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__ror__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_ror_slot), 2,
        )),
        "__and__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__and__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_and_slot), 2,
        )),
        "__rand__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__rand__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rand_slot), 2,
        )),
        "__sub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_sub_slot), 2,
        )),
        "__rsub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rsub_slot), 2,
        )),
        "__xor__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__xor__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_xor_slot), 2,
        )),
        "__rxor__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__rxor__"),
            fn_addr!(crate::object::ops_arith::native_slots::set_rxor_slot), 2,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).frozenset).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::frozenset_new),
        )),
        "union" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "union"),
            fn_addr!(molt_frozenset_union_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "intersection" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "intersection"),
            fn_addr!(molt_frozenset_intersection_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "difference" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "difference"),
            fn_addr!(molt_frozenset_difference_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        "symmetric_difference" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "symmetric_difference"),
            fn_addr!(molt_frozenset_symmetric_difference),
            2,
        )),
        "isdisjoint" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "isdisjoint"),
            fn_addr!(molt_frozenset_isdisjoint),
            2,
        )),
        "issubset" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "issubset").with_text_signature("($self, other, /)"),
            fn_addr!(molt_frozenset_issubset),
            2,
        )),
        "issuperset" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "issuperset").with_text_signature("($self, other, /)"),
            fn_addr!(molt_frozenset_issuperset),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "copy"),
            fn_addr!(molt_frozenset_copy_method),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, "__contains__"),
            fn_addr!(crate::object::ops_set::molt_set_contains),
            2,
        )),
});

crate::builtins::methods::native_method_table!(list_method_bits, publish_list_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::List, {

}, {
        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).list).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::list_new),
        )),
        "append" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "append").with_text_signature("($self, object, /)"),
            fn_addr!(molt_list_append),
            2,
        )),
        "extend" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "extend").with_text_signature("($self, iterable, /)"),
            fn_addr!(molt_list_extend),
            2,
        )),
        "insert" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "insert").with_text_signature("($self, index, object, /)"),
            fn_addr!(molt_list_insert),
            3,
        )),
        "remove" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "remove").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_remove),
            2,
        )),
        "pop" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "pop").with_text_signature("($self, index=-1, /)"),
                fn_addr!(molt_list_pop),
                2,
                &[none],
            ))
        },
        "clear" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "clear").with_text_signature("($self, /)"),
            fn_addr!(molt_list_clear),
            1,
        )),
        "__init__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::list_init),
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "copy").with_text_signature("($self, /)"),
            fn_addr!(molt_list_copy),
            1,
        )),
        "reverse" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "reverse").with_text_signature("($self, /)"),
            fn_addr!(molt_list_reverse),
            1,
        )),
        "count" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "count").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_count),
            2,
        )),
        "index" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "index").with_text_signature("($self, value, start=0, stop=sys.maxsize, /)"),
            fn_addr!(molt_list_index_range),
            4,
        )),
        "sort" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "sort").with_text_signature("($self, /, *, key=None, reverse=False)"),
            fn_addr!(molt_list_sort),
            3,
        )),
        "__add__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__add__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_add_method),
            2,
        )),
        "__mul__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__mul__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_mul_method),
            2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__rmul__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_mul_method),
            2,
        )),
        "__iadd__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__iadd__").with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_list::list_iadd_slot),
            2,
        )),
        "__imul__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__imul__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_imul_method),
            2,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "__getitem__").with_text_signature("($self, index, /)"),
            fn_addr!(crate::object::ops_list::list_getitem_slot),
            2,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__setitem__").with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops_list::list_setitem_slot),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__delitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops_list::list_delitem_slot),
            2,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_len_slot),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__contains__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops_list::list_contains_slot),
            2,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, "__reversed__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_reversed_slot),
            1,
        )),
        "__repr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, "__repr__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::list_repr_slot),
            1,
        )),

});

/// Explicit base descriptors admit their physical receiver before delegating
/// to owner-agnostic builtin operations. Subclass overrides remain bypassed.
fn tuple_sequence_receiver(py: &PyToken<'_>, bits: u64, method: &str) -> bool {
    crate::object::tuple_storage::TupleStorage::admit(py, bits, method).is_some()
}

extern "C" fn tuple_iter_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !tuple_sequence_receiver(py, bits, "__iter__") {
            return MoltObject::none().bits();
        }
        crate::object::ops_iter::builtin_iter_slot(bits)
    })
}

extern "C" fn tuple_len_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !tuple_sequence_receiver(py, bits, "__len__") {
            return MoltObject::none().bits();
        }
        crate::object::ops_sys::molt_len_builtin(bits)
    })
}

extern "C" fn tuple_getitem_slot(bits: u64, key: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !tuple_sequence_receiver(py, bits, "__getitem__") {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_getitem_builtin(bits, key)
    })
}

extern "C" fn tuple_contains_slot(bits: u64, item: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !tuple_sequence_receiver(py, bits, "__contains__") {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_contains_builtin(bits, item)
    })
}

crate::builtins::methods::native_method_table!(tuple_method_bits, publish_tuple_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Tuple, {


}, {
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__add__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__add__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).tuple).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::tuple_new),
        )),
        "count" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).tuple, "count").with_text_signature("($self, value, /)"),
                fn_addr!(molt_tuple_count),
                2,
            )),
        "index" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).tuple, "index").with_text_signature("($self, value, start=0, stop=sys.maxsize, /)"),
                fn_addr!(molt_tuple_index_range),
                4,
            )),
            // Subclasses resolve these inherited slots through normal special
            // lookup. The builtin slot entries consume physical tuple storage;
            // they must not redispatch a subclass override recursively.,
        "__iter__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__iter__").with_text_signature("($self, /)"),
                fn_addr!(tuple_iter_slot),
                1,
            )),
        "__len__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__len__").with_text_signature("($self, /)"),
                fn_addr!(tuple_len_slot),
                1,
            )),
        "__getitem__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__getitem__").with_text_signature("($self, key, /)"),
                fn_addr!(tuple_getitem_slot),
                2,
            )),
        "__contains__" => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, "__contains__").with_text_signature("($self, key, /)"),
                fn_addr!(tuple_contains_slot),
                2,
            )),
});

pub(crate) unsafe fn list_len(ptr: *mut u8) -> usize {
    unsafe {
        let tid = object_type_id(ptr);
        if tid == TYPE_ID_LIST_INT {
            crate::object::layout::list_int_vec_ref(ptr).len()
        } else if tid == TYPE_ID_LIST_BOOL {
            crate::object::layout::list_bool_vec_ref(ptr).len()
        } else {
            crate::object::seq_access::len(ptr)
        }
    }
}

pub(crate) unsafe fn tuple_len(ptr: *mut u8) -> usize {
    unsafe { crate::object::seq_access::len(ptr) }
}

pub(crate) unsafe fn dict_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn dict_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) }
}

pub(crate) unsafe fn dict_hashes_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe {
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
            as *mut *mut Vec<u64>)
    }
}

pub(crate) unsafe fn dict_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = dict_order_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn dict_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe {
        let vec_ptr = dict_table_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn dict_hashes(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = dict_hashes_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn dict_len(ptr: *mut u8) -> usize {
    unsafe { dict_order(ptr).len() / 2 }
}

pub(crate) unsafe fn set_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn set_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) }
}

pub(crate) unsafe fn set_hashes_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe {
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
            as *mut *mut Vec<u64>)
    }
}

pub(crate) unsafe fn set_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = set_order_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn set_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe {
        let vec_ptr = set_table_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn set_hashes(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = set_hashes_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn set_len(ptr: *mut u8) -> usize {
    unsafe { set_order(ptr).len() }
}

pub(crate) unsafe fn dict_view_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn dict_view_len(ptr: *mut u8) -> usize {
    unsafe {
        let dict_bits = dict_view_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            return dict_len(dict_ptr);
        }
        0
    }
}

pub(crate) unsafe fn dict_view_entry(ptr: *mut u8, idx: usize) -> Option<(u64, u64)> {
    unsafe {
        let dict_bits = dict_view_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
            let order = dict_order(dict_ptr);
            let entry = idx * 2;
            if entry + 1 >= order.len() {
                return None;
            }
            return Some((order[entry], order[entry + 1]));
        }
        None
    }
}

pub(crate) unsafe fn dict_view_as_set_bits(
    _py: &PyToken<'_>,
    view_ptr: *mut u8,
    view_type: u32,
) -> Option<u64> {
    unsafe {
        if !is_set_view_type(view_type) {
            return None;
        }
        let len = dict_view_len(view_ptr);
        let set_bits = molt_set_new(len as u64);
        let set_ptr = obj_from_bits(set_bits).as_ptr()?;
        for idx in 0..len {
            if let Some((key_bits, val_bits)) = dict_view_entry(view_ptr, idx) {
                let (entry_bits, needs_drop) = if view_type == TYPE_ID_DICT_ITEMS_VIEW {
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    if tuple_ptr.is_null() {
                        dec_ref_bits(_py, set_bits);
                        return None;
                    }
                    (MoltObject::from_ptr(tuple_ptr).bits(), true)
                } else {
                    (key_bits, false)
                };
                set_add_in_place(_py, set_ptr, entry_bits, HashContext::SetElement);
                if needs_drop {
                    dec_ref_bits(_py, entry_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, set_bits);
                    return None;
                }
            }
        }
        Some(set_bits)
    }
}
