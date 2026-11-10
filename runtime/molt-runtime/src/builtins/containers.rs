use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{
    BIND_KIND_PACKED_BUILTIN, MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_DICT_ITEMS_VIEW,
    TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_FROZENSET, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_SET,
    builtin_classes, builtin_func_bits, builtin_func_bits_with_bind_kind,
    builtin_func_bits_with_defaults_tuple, dict_clear_method, dict_copy_method,
    dict_fromkeys_method, dict_get_method, dict_items_method, dict_keys_method,
    dict_popitem_method, dict_setdefault_method, dict_update_method, dict_values_method,
    molt_dict_pop_method, molt_frozenset_copy_method, molt_frozenset_difference_multi,
    molt_frozenset_intersection_multi, molt_frozenset_isdisjoint, molt_frozenset_issubset,
    molt_frozenset_issuperset, molt_frozenset_symmetric_difference, molt_frozenset_union_multi,
    molt_list_add_method, molt_list_append, molt_list_clear, molt_list_copy, molt_list_count,
    molt_list_extend, molt_list_imul_method, molt_list_index_range, molt_list_insert,
    molt_list_mul_method, molt_list_pop, molt_list_remove, molt_list_reverse, molt_list_sort,
    molt_reversed_builtin, molt_set_add, molt_set_clear, molt_set_copy_method,
    molt_set_difference_multi, molt_set_difference_update_multi, molt_set_discard,
    molt_set_intersection_multi, molt_set_intersection_update_multi, molt_set_isdisjoint,
    molt_set_issubset, molt_set_issuperset, molt_set_pop, molt_set_remove,
    molt_set_symmetric_difference, molt_set_symmetric_difference_update, molt_set_union_multi,
    molt_set_update_multi, molt_tuple_count, molt_tuple_index_range, obj_from_bits, object_type_id,
};

pub(crate) fn is_set_like_type(type_id: u32) -> bool {
    type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET
}

pub(crate) fn is_set_view_type(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW)
}

crate::builtins::methods::native_method_table!(dict_method_bits, publish_dict_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Dict, {

}, {

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).dict).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::dict_new),
        )),
        crate::NameId::DunderInit => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::dict_init),
        )),
        crate::NameId::DunderRepr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderRepr).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::dict_repr_slot),
            1,
        )),
        crate::NameId::DunderOr => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderOr),
            fn_addr!(crate::object::ops_arith::native_slots::dict_or_slot), 2,
        )),
        crate::NameId::DunderRor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderRor),
            fn_addr!(crate::object::ops_arith::native_slots::dict_ror_slot), 2,
        )),
        crate::NameId::DunderIor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderIor),
            fn_addr!(crate::object::ops_arith::native_slots::dict_ior_slot), 2,
        )),
        crate::NameId::Keys => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Keys),
            fn_addr!(dict_keys_method),
            1,
        )),
        crate::NameId::Values => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Values),
            fn_addr!(dict_values_method),
            1,
        )),
        crate::NameId::Items => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Items),
            fn_addr!(dict_items_method),
            1,
        )),
        crate::NameId::Get => {
            if cfg!(target_arch = "wasm32")
                && runtime_flags::enabled(RuntimeFlag::MOLT_WASM_DICT_METHOD_DEBUG)
            {
                eprintln!(
                    "molt wasm dict_method:get fn=0x{:x}",
                    fn_addr!(dict_get_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Get).with_text_signature("($self, key, default=None, /)"),
                fn_addr!(dict_get_method),
                3,
                &[none],
            ))
        },
        crate::NameId::Pop => {
            let miss = crate::builtins::methods::missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Pop).with_text_signature("($self, key, default=<unrepresentable>, /)"),
                fn_addr!(molt_dict_pop_method),
                3,
                &[miss],
            ))
        },
        crate::NameId::Clear => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Clear),
            fn_addr!(dict_clear_method),
            1,
        )),
        crate::NameId::Copy => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Copy),
            fn_addr!(dict_copy_method),
            1,
        )),
        crate::NameId::Popitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Popitem).with_text_signature("($self, /)"),
            fn_addr!(dict_popitem_method),
            1,
        )),
        crate::NameId::Setdefault => {
            if cfg!(target_arch = "wasm32")
                && runtime_flags::enabled(RuntimeFlag::MOLT_WASM_DICT_METHOD_DEBUG)
            {
                eprintln!(
                    "molt wasm dict_method:setdefault fn=0x{:x}",
                    fn_addr!(dict_setdefault_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Setdefault).with_text_signature("($self, key, default=None, /)"),
                fn_addr!(dict_setdefault_method),
                3,
                &[none],
            ))
        },
        crate::NameId::Update => {
            let miss = crate::builtins::methods::missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::Update),
                fn_addr!(dict_update_method),
                2,
                &[miss],
            ))
        },
        crate::NameId::Fromkeys => {
            if cfg!(target_arch = "wasm32")
                && runtime_flags::enabled(RuntimeFlag::MOLT_WASM_DICT_METHOD_DEBUG)
            {
                eprintln!(
                    "molt wasm dict_method:fromkeys fn=0x{:x}",
                    fn_addr!(dict_fromkeys_method)
                );
            }
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).dict, crate::NameId::Fromkeys).with_text_signature("($type, iterable, value=None, /)"),
                fn_addr!(dict_fromkeys_method),
                3,
                &[none],
            ))
        },
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        crate::NameId::DunderSetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderSetitem).with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        crate::NameId::DunderDelitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderDelitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).dict, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        crate::NameId::DunderReversed => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).dict, crate::NameId::DunderReversed).with_text_signature("($self, /)"),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
});

crate::builtins::methods::native_method_table!(set_method_bits, publish_set_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Set, {

}, {
        crate::NameId::DunderReduce => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::DunderReduce),
            fn_addr!(crate::molt_set_reduce),
            1,
        )),
        crate::NameId::DunderOr => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderOr),
            fn_addr!(crate::object::ops_arith::native_slots::set_or_slot), 2,
        )),
        crate::NameId::DunderRor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderRor),
            fn_addr!(crate::object::ops_arith::native_slots::set_ror_slot), 2,
        )),
        crate::NameId::DunderAnd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderAnd),
            fn_addr!(crate::object::ops_arith::native_slots::set_and_slot), 2,
        )),
        crate::NameId::DunderRand => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderRand),
            fn_addr!(crate::object::ops_arith::native_slots::set_rand_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::set_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::set_rsub_slot), 2,
        )),
        crate::NameId::DunderXor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderXor),
            fn_addr!(crate::object::ops_arith::native_slots::set_xor_slot), 2,
        )),
        crate::NameId::DunderRxor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderRxor),
            fn_addr!(crate::object::ops_arith::native_slots::set_rxor_slot), 2,
        )),
        crate::NameId::DunderIor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderIor),
            fn_addr!(crate::object::ops_arith::native_slots::set_ior_slot), 2,
        )),
        crate::NameId::DunderIand => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderIand),
            fn_addr!(crate::object::ops_arith::native_slots::set_iand_slot), 2,
        )),
        crate::NameId::DunderIsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderIsub),
            fn_addr!(crate::object::ops_arith::native_slots::set_isub_slot), 2,
        )),
        crate::NameId::DunderIxor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderIxor),
            fn_addr!(crate::object::ops_arith::native_slots::set_ixor_slot), 2,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).set).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::set_new),
        )),
        crate::NameId::DunderInit => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::set_init),
        )),
        crate::NameId::Add => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Add),
            fn_addr!(molt_set_add),
            2,
        )),
        crate::NameId::Discard => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Discard),
            fn_addr!(molt_set_discard),
            2,
        )),
        crate::NameId::Remove => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Remove),
            fn_addr!(molt_set_remove),
            2,
        )),
        crate::NameId::Pop => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Pop),
            fn_addr!(molt_set_pop),
            1,
        )),
        crate::NameId::Clear => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Clear),
            fn_addr!(molt_set_clear),
            1,
        )),
        crate::NameId::Update => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Update),
            fn_addr!(molt_set_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::LowerUnion => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::LowerUnion),
            fn_addr!(molt_set_union_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::Intersection => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Intersection),
            fn_addr!(molt_set_intersection_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::Difference => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Difference),
            fn_addr!(molt_set_difference_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::SymmetricDifference => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::SymmetricDifference),
            fn_addr!(molt_set_symmetric_difference),
            2,
        )),
        crate::NameId::IntersectionUpdate => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::IntersectionUpdate),
            fn_addr!(molt_set_intersection_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::DifferenceUpdate => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::DifferenceUpdate),
            fn_addr!(molt_set_difference_update_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::SymmetricDifferenceUpdate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::SymmetricDifferenceUpdate),
            fn_addr!(molt_set_symmetric_difference_update),
            2,
        )),
        crate::NameId::Isdisjoint => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Isdisjoint),
            fn_addr!(molt_set_isdisjoint),
            2,
        )),
        crate::NameId::Issubset => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Issubset).with_text_signature("($self, other, /)"),
            fn_addr!(molt_set_issubset),
            2,
        )),
        crate::NameId::Issuperset => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Issuperset).with_text_signature("($self, other, /)"),
            fn_addr!(molt_set_issuperset),
            2,
        )),
        crate::NameId::Copy => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::Copy),
            fn_addr!(molt_set_copy_method),
            1,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).set, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).set, crate::NameId::DunderContains),
            fn_addr!(crate::object::ops_set::molt_set_contains),
            2,
        )),
});

crate::builtins::methods::native_method_table!(frozenset_method_bits, publish_frozenset_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Frozenset, {

}, {
        crate::NameId::DunderReduce => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderReduce),
            fn_addr!(crate::molt_set_reduce),
            1,
        )),
        crate::NameId::DunderOr => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderOr),
            fn_addr!(crate::object::ops_arith::native_slots::set_or_slot), 2,
        )),
        crate::NameId::DunderRor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderRor),
            fn_addr!(crate::object::ops_arith::native_slots::set_ror_slot), 2,
        )),
        crate::NameId::DunderAnd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderAnd),
            fn_addr!(crate::object::ops_arith::native_slots::set_and_slot), 2,
        )),
        crate::NameId::DunderRand => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderRand),
            fn_addr!(crate::object::ops_arith::native_slots::set_rand_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::set_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::set_rsub_slot), 2,
        )),
        crate::NameId::DunderXor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderXor),
            fn_addr!(crate::object::ops_arith::native_slots::set_xor_slot), 2,
        )),
        crate::NameId::DunderRxor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderRxor),
            fn_addr!(crate::object::ops_arith::native_slots::set_rxor_slot), 2,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).frozenset).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::frozenset_new),
        )),
        crate::NameId::LowerUnion => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::LowerUnion),
            fn_addr!(molt_frozenset_union_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::Intersection => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Intersection),
            fn_addr!(molt_frozenset_intersection_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::Difference => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Difference),
            fn_addr!(molt_frozenset_difference_multi),
            2,
            BIND_KIND_PACKED_BUILTIN,
        )),
        crate::NameId::SymmetricDifference => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::SymmetricDifference),
            fn_addr!(molt_frozenset_symmetric_difference),
            2,
        )),
        crate::NameId::Isdisjoint => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Isdisjoint),
            fn_addr!(molt_frozenset_isdisjoint),
            2,
        )),
        crate::NameId::Issubset => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Issubset).with_text_signature("($self, other, /)"),
            fn_addr!(molt_frozenset_issubset),
            2,
        )),
        crate::NameId::Issuperset => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Issuperset).with_text_signature("($self, other, /)"),
            fn_addr!(molt_frozenset_issuperset),
            2,
        )),
        crate::NameId::Copy => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::Copy),
            fn_addr!(molt_frozenset_copy_method),
            1,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).frozenset, crate::NameId::DunderContains),
            fn_addr!(crate::object::ops_set::molt_set_contains),
            2,
        )),
});

crate::builtins::methods::native_method_table!(list_method_bits, publish_list_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::List, {

}, {
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).list).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::list_new),
        )),
        crate::NameId::Append => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Append).with_text_signature("($self, object, /)"),
            fn_addr!(molt_list_append),
            2,
        )),
        crate::NameId::Extend => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Extend).with_text_signature("($self, iterable, /)"),
            fn_addr!(molt_list_extend),
            2,
        )),
        crate::NameId::Insert => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Insert).with_text_signature("($self, index, object, /)"),
            fn_addr!(molt_list_insert),
            3,
        )),
        crate::NameId::Remove => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Remove).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_remove),
            2,
        )),
        crate::NameId::Pop => {
            let last_index = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Pop).with_text_signature("($self, index=-1, /)"),
                fn_addr!(molt_list_pop),
                2,
                &[last_index],
            ))
        },
        crate::NameId::Clear => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Clear).with_text_signature("($self, /)"),
            fn_addr!(molt_list_clear),
            1,
        )),
        crate::NameId::DunderInit => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::list_init),
        )),
        crate::NameId::Copy => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Copy).with_text_signature("($self, /)"),
            fn_addr!(molt_list_copy),
            1,
        )),
        crate::NameId::Reverse => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Reverse).with_text_signature("($self, /)"),
            fn_addr!(molt_list_reverse),
            1,
        )),
        crate::NameId::Count => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Count).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_count),
            2,
        )),
        crate::NameId::Index => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Index).with_text_signature("($self, value, start=0, stop=sys.maxsize, /)"),
            fn_addr!(molt_list_index_range),
            4,
        )),
        crate::NameId::Sort => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::Sort).with_text_signature("($self, /, *, key=None, reverse=False)"),
            fn_addr!(molt_list_sort),
            3,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderAdd).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_add_method),
            2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderMul).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_mul_method),
            2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderRmul).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_mul_method),
            2,
        )),
        crate::NameId::DunderIadd => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderIadd).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_list::list_iadd_slot),
            2,
        )),
        crate::NameId::DunderImul => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderImul).with_text_signature("($self, value, /)"),
            fn_addr!(molt_list_imul_method),
            2,
        )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::DunderGetitem).with_text_signature("($self, index, /)"),
            fn_addr!(crate::object::ops_list::list_getitem_slot),
            2,
        )),
        crate::NameId::DunderSetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderSetitem).with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops_list::list_setitem_slot),
            3,
        )),
        crate::NameId::DunderDelitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderDelitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops_list::list_delitem_slot),
            2,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_len_slot),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops_list::list_contains_slot),
            2,
        )),
        crate::NameId::DunderReversed => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).list, crate::NameId::DunderReversed).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_list::list_reversed_slot),
            1,
        )),
        crate::NameId::DunderRepr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).list, crate::NameId::DunderRepr).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::list_repr_slot),
            1,
        )),

});

/// Explicit base descriptors admit their physical receiver before delegating
/// to owner-agnostic builtin operations. Subclass overrides remain bypassed.
fn tuple_sequence_receiver(py: &PyToken<'_>, bits: u64, method: &str) -> bool {
    crate::object::tuple_storage::TupleStorage::admit(py, bits, method).is_some()
}

extern "C" fn tuple_iter_slot(__context: u64, bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        if !tuple_sequence_receiver(py, bits, "__iter__") {
            return MoltObject::none().bits();
        }
        crate::object::ops_iter::builtin_iter_slot(__attached_py.attached_context(), bits)
    })
}

extern "C" fn tuple_len_slot(__context: u64, bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        if !tuple_sequence_receiver(py, bits, "__len__") {
            return MoltObject::none().bits();
        }
        crate::object::ops_sys::molt_len_builtin_attached_body(py, bits)
    })
}

extern "C" fn tuple_getitem_slot(__context: u64, bits: u64, key: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        if !tuple_sequence_receiver(py, bits, "__getitem__") {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_getitem_builtin_attached_body(py, bits, key)
    })
}

extern "C" fn tuple_contains_slot(__context: u64, bits: u64, item: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        if !tuple_sequence_receiver(py, bits, "__contains__") {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_contains_builtin_attached_body(py, bits, item)
    })
}

crate::builtins::methods::native_method_table!(tuple_method_bits, publish_tuple_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Tuple, {


}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_tuple_getnewargs),
            1,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderAdd),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).tuple).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::tuple_new),
        )),
        crate::NameId::Count => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).tuple, crate::NameId::Count).with_text_signature("($self, value, /)"),
                fn_addr!(molt_tuple_count),
                2,
            )),
        crate::NameId::Index => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).tuple, crate::NameId::Index).with_text_signature("($self, value, start=0, stop=sys.maxsize, /)"),
                fn_addr!(molt_tuple_index_range),
                4,
            )),
            // Subclasses resolve these inherited slots through normal special
            // lookup. The builtin slot entries consume physical tuple storage;
            // they must not redispatch a subclass override recursively.,
        crate::NameId::DunderIter => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderIter).with_text_signature("($self, /)"),
                fn_addr!(tuple_iter_slot),
                1,
            )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderLen).with_text_signature("($self, /)"),
                fn_addr!(tuple_len_slot),
                1,
            )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
                fn_addr!(tuple_getitem_slot),
                2,
            )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).tuple, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
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

/// A stored hash can never be the Python error sentinel (-1). Its complement
/// leaves zero for a vacant row without a separate occupancy allocation.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StoredHash(std::num::NonZeroU64);

impl StoredHash {
    pub(crate) fn new(hash: u64) -> Option<Self> {
        std::num::NonZeroU64::new(!hash).map(Self)
    }
    pub(crate) fn get(self) -> u64 {
        !self.0.get()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DictEntry {
    pub(crate) key: u64,
    pub(crate) value: u64,
    pub(crate) hash: Option<StoredHash>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SetEntry {
    pub(crate) key: u64,
    pub(crate) hash: Option<StoredHash>,
}

/// The stable entries and index owners retain their accounting/lock identity.
/// `live`, physical entries length, and occupied index count are distinct facts.
#[repr(C)]
pub(crate) struct HashStorage<Entry> {
    pub(crate) entries: *mut Vec<Entry>,
    pub(crate) table: *mut Vec<usize>,
    pub(crate) live: usize,
    pub(crate) fill: usize,
}

/// This mode governs real dictionary key-table conversion. Both modes keep
/// the same sparse row representation; moving to general keys compacts the
/// table at the configured version's non-exact-string insertion phase.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DictKeyKind {
    #[default]
    Unicode,
    General,
}

/// Dictionary append capacity is consumed even when a probe reuses a deleted
/// index or popitem truncates the row extent. Sets retain occupied-slot fill.
#[repr(C)]
pub(crate) struct DictStorage {
    pub(crate) entries: *mut Vec<DictEntry>,
    pub(crate) table: *mut Vec<usize>,
    pub(crate) live: usize,
    pub(crate) usable: usize,
    pub(crate) kind: DictKeyKind,
}

const _: () = {
    assert!(std::mem::size_of::<DictEntry>() == 24);
    assert!(std::mem::size_of::<SetEntry>() == 16);
    assert!(std::mem::size_of::<DictStorage>() == 5 * std::mem::size_of::<usize>());
    assert!(std::mem::size_of::<HashStorage<SetEntry>>() == 4 * std::mem::size_of::<usize>());
};

pub(crate) unsafe fn dict_storage(ptr: *mut u8) -> &'static mut DictStorage {
    unsafe { &mut *ptr.cast() }
}
pub(crate) unsafe fn set_storage(ptr: *mut u8) -> &'static mut HashStorage<SetEntry> {
    unsafe { &mut *ptr.cast() }
}
pub(crate) unsafe fn dict_entries_ptr(ptr: *mut u8) -> *mut Vec<DictEntry> {
    unsafe { dict_storage(ptr).entries }
}
pub(crate) unsafe fn dict_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { dict_storage(ptr).table }
}
pub(crate) unsafe fn dict_entries(ptr: *mut u8) -> &'static mut Vec<DictEntry> {
    unsafe { &mut *dict_entries_ptr(ptr) }
}
pub(crate) unsafe fn dict_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe { &mut *dict_table_ptr(ptr) }
}
pub(crate) unsafe fn dict_len(ptr: *mut u8) -> usize {
    unsafe { dict_storage(ptr).live }
}
pub(crate) unsafe fn set_entries_ptr(ptr: *mut u8) -> *mut Vec<SetEntry> {
    unsafe { set_storage(ptr).entries }
}
pub(crate) unsafe fn set_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { set_storage(ptr).table }
}
pub(crate) unsafe fn set_entries(ptr: *mut u8) -> &'static mut Vec<SetEntry> {
    unsafe { &mut *set_entries_ptr(ptr) }
}
pub(crate) unsafe fn set_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe { &mut *set_table_ptr(ptr) }
}
pub(crate) unsafe fn set_len(ptr: *mut u8) -> usize {
    unsafe { set_storage(ptr).live }
}

/// Borrowed physical rows, in dictionary insertion order; no borrow may cross
/// an operation that can invoke Python or mutate the dictionary.
pub(crate) unsafe fn dict_live_entries(
    ptr: *mut u8,
) -> impl DoubleEndedIterator<Item = &'static DictEntry> {
    unsafe { dict_entries(ptr).iter().filter(|row| row.hash.is_some()) }
}

/// Sets walk their canonical probe table, including every operation whose
/// callbacks can observe traversal order. The cursor is a table position.
pub(crate) unsafe fn set_next_entry(ptr: *mut u8, cursor: &mut usize) -> Option<SetEntry> {
    unsafe {
        while *cursor < set_table(ptr).len() {
            let index = set_table(ptr)[*cursor];
            *cursor += 1;
            if index != 0 && index != usize::MAX {
                let row = *set_entries(ptr).get(index - 1)?;
                if row.hash.is_some() {
                    return Some(row);
                }
            }
        }
        None
    }
}

pub(crate) unsafe fn dict_next_entry(ptr: *mut u8, cursor: &mut usize) -> Option<DictEntry> {
    unsafe {
        while *cursor < dict_entries(ptr).len() {
            let row = dict_entries(ptr)[*cursor];
            *cursor += 1;
            if row.hash.is_some() {
                return Some(row);
            }
        }
        None
    }
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
