use super::common::{
    builtin_func_bits, builtin_func_bits_with_bind_kind, builtin_func_bits_with_defaults_tuple,
    runtime_python_at_least,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(type_method_bits, publish_type_methods, _py, name, [], {

}, {
        crate::NameId::DunderDir => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderDir).with_text_signature("($self, /)"),
            fn_addr!(molt_type_dir_method),
            1,
        )),
        crate::NameId::DunderSetattr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderSetattr).with_text_signature("($self, name, value, /)"),
            fn_addr!(type_setattr),
            3,
        )),
        crate::NameId::DunderDelattr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderDelattr).with_text_signature("($self, name, /)"),
            fn_addr!(type_delattr),
            2,
        )),
        crate::NameId::DunderGetattribute => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderGetattribute).with_text_signature("($self, name, /)"),
            fn_addr!(molt_type_getattribute),
            2,
        )),
        crate::NameId::DunderCall => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderCall).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_type_call),
            1,
        )),
        crate::NameId::DunderNew => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).type_obj).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(molt_type_new),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        crate::NameId::DunderInit => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_type_init),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        crate::NameId::DunderPrepare => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderPrepare),
            fn_addr!(molt_type_prepare),
            3,
        )),
        crate::NameId::DunderInstancecheck => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderInstancecheck).with_text_signature("($self, instance, /)"),
            fn_addr!(molt_type_instancecheck),
            2,
        )),
        crate::NameId::DunderSubclasscheck => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, crate::NameId::DunderSubclasscheck).with_text_signature("($self, subclass, /)"),
            fn_addr!(molt_type_subclasscheck),
            2,
        )),
        crate::NameId::Mro => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, crate::NameId::Mro).with_text_signature("($self, /)"),
            fn_addr!(molt_type_mro),
            1,
        )),
});

crate::builtins::methods::native_method_table!(object_method_bits, publish_object_methods, _py, name, [], {

}, {
        crate::NameId::DunderReduce => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderReduce).with_text_signature("($self, /)"),
            fn_addr!(molt_object_reduce),
            1,
        )),
        crate::NameId::DunderReduceEx => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderReduceEx).with_text_signature("($self, protocol, /)"),
            fn_addr!(molt_object_reduce_ex),
            2,
        )),
        crate::NameId::DunderDir => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderDir).with_text_signature("($self, /)"),
            fn_addr!(molt_object_dir_method),
            1,
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_object_format_method),
            2,
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_addr!(molt_object_hash),
            1,
        )),
        crate::NameId::DunderGetstate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderGetstate).with_text_signature("($self, /)"),
            fn_addr!(molt_object_getstate),
            1,
        )),
        crate::NameId::DunderLt => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderLt).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_lt_method),
            2,
        )),
        crate::NameId::DunderLe => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderLe).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_le_method),
            2,
        )),
        crate::NameId::DunderGt => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderGt).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_gt_method),
            2,
        )),
        crate::NameId::DunderGe => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderGe).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_ge_method),
            2,
        )),
        crate::NameId::DunderGetattribute => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderGetattribute).with_text_signature("($self, name, /)"),
            fn_addr!(molt_object_getattribute),
            2,
        )),
        crate::NameId::DunderNew => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).object).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(molt_object_new_bound),
            1,
        )),
        crate::NameId::DunderInit => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_object_init),
            1,
        )),
        // Class hooks bind the lookup owner even for class-mode super.,
        crate::NameId::DunderInitSubclass => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderInitSubclass),
            fn_addr!(molt_object_init_subclass),
            1,
        )),
        // CPython accepts any arguments (METH_VARARGS); a missing subclass is
        // the case real code reaches.
        crate::NameId::DunderSubclasshook => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderSubclasshook),
            fn_addr!(molt_object_subclasshook),
            2,
            &[missing_bits(_py)],
        )),
        crate::NameId::DunderSetattr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderSetattr).with_text_signature("($self, name, value, /)"),
            fn_addr!(molt_object_setattr),
            3,
        )),
        crate::NameId::DunderDelattr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderDelattr).with_text_signature("($self, name, /)"),
            fn_addr!(molt_object_delattr),
            2,
        )),
        crate::NameId::DunderEq => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderEq).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_eq),
            2,
        )),
        crate::NameId::DunderNe => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderNe).with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_ne),
            2,
        )),
        crate::NameId::DunderRepr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderRepr).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::object_repr_slot),
            1,
        )),
        crate::NameId::DunderStr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, crate::NameId::DunderStr).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::object_str_slot),
            1,
        )),
});

crate::builtins::methods::native_method_table!(memoryview_method_bits, publish_memoryview_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::MemoryView, {}, {
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).memoryview)
                .with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::memoryview_new),
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderIter)
                .with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot), 1,
        )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderGetitem)
                .with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin), 2,
        )),
        crate::NameId::PrivateFromFlags => Some(builtin_func_bits(
            _py,
            NativeCallableSpec {
                self_bits: Some(builtin_classes(_py).memoryview),
                ..NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).memoryview, crate::NameId::PrivateFromFlags)
            }.with_text_signature("($type, /, object, flags)"),
            fn_addr!(molt_memoryview_from_flags),
            2,
        )),
        crate::NameId::Count [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Count),
            fn_addr!(molt_memoryview_count),
            2,
        )),
        crate::NameId::Index [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Index),
            fn_addr!(molt_memoryview_index),
            2,
        )),
        crate::NameId::Hex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Hex).with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_memoryview_hex),
            3,
        )),
        crate::NameId::DunderEnter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderEnter),
            fn_addr!(crate::object::ops_memoryview::memoryview_enter_method),
            1,
        )),
        crate::NameId::DunderExit => {
            let spec = NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderExit);
            let spec = if runtime_python_at_least(_py, 3, 15) {
                spec.with_text_signature("($self, /, *exc_info)")
            } else { spec };
            Some(crate::builtins::methods::builtin_variadic_func_bits(
                _py, spec, fn_addr!(crate::object::ops_memoryview::memoryview_exit_method),
            ))
        },
        crate::NameId::Release => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Release).with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_release),
            1,
        )),
        crate::NameId::Toreadonly => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Toreadonly).with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_toreadonly),
            1,
        )),
        crate::NameId::Tobytes => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Tobytes).with_text_signature("($self, /, order='C')"),
            fn_addr!(crate::object::ops_memoryview::memoryview_tobytes_method),
        )),
        crate::NameId::Tolist => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Tolist).with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_tolist),
            1,
        )),
        crate::NameId::Cast => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, crate::NameId::Cast).with_text_signature("($self, /, format, shape=<unrepresentable>)"),
            fn_addr!(molt_memoryview_cast),
            4,
        )),
        crate::NameId::DunderSetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderSetitem).with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        crate::NameId::DunderDelitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, crate::NameId::DunderDelitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
});

crate::builtins::methods::native_method_table!(range_method_bits, publish_range_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Range, {

}, {
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).range, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        crate::NameId::Count => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).range, crate::NameId::Count),
            fn_addr!(molt_range_count),
            2,
        )),
        crate::NameId::Index => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).range, crate::NameId::Index),
            fn_addr!(molt_range_index),
            2,
        )),
});

pub(crate) extern "C" fn type_setattr(__context: u64, receiver: u64, name: u64, value: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::builtins::attributes::explicit_type_mutate_attr_name(receiver, name, Some(value))
}

pub(crate) extern "C" fn type_delattr(__context: u64, receiver: u64, name: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::builtins::attributes::explicit_type_mutate_attr_name(receiver, name, None)
}
