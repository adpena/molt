use super::common::{builtin_func_bits, builtin_func_bits_with_bind_kind, runtime_python_at_least};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(type_method_bits, publish_type_methods, _py, name, [], {

}, {
        "__dir__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, "__dir__").with_text_signature("($self, /)"),
            fn_addr!(molt_type_dir_method),
            1,
        )),
        "__setattr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, "__setattr__").with_text_signature("($self, name, value, /)"),
            fn_addr!(type_setattr),
            3,
        )),
        "__delattr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, "__delattr__").with_text_signature("($self, name, /)"),
            fn_addr!(type_delattr),
            2,
        )),
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, "__getattribute__").with_text_signature("($self, name, /)"),
            fn_addr!(molt_type_getattribute),
            2,
        )),
        "__call__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, "__call__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_type_call),
            1,
        )),
        "__new__" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).type_obj).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(molt_type_new),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        "__init__" => Some(builtin_func_bits_with_bind_kind(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).type_obj, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_type_init),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        "__prepare__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).type_obj, "__prepare__"),
            fn_addr!(molt_type_prepare),
            3,
        )),
        "__instancecheck__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, "__instancecheck__").with_text_signature("($self, instance, /)"),
            fn_addr!(molt_type_instancecheck),
            2,
        )),
        "__subclasscheck__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, "__subclasscheck__").with_text_signature("($self, subclass, /)"),
            fn_addr!(molt_type_subclasscheck),
            2,
        )),
        "mro" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).type_obj, "mro").with_text_signature("($self, /)"),
            fn_addr!(molt_type_mro),
            1,
        )),
});

crate::builtins::methods::native_method_table!(object_method_bits, publish_object_methods, _py, name, [], {

}, {
        "__dir__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, "__dir__").with_text_signature("($self, /)"),
            fn_addr!(molt_object_dir_method),
            1,
        )),
        "__format__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, "__format__").with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_object_format_method),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__hash__").with_text_signature("($self, /)"),
            fn_addr!(molt_object_hash),
            1,
        )),
        "__getstate__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, "__getstate__").with_text_signature("($self, /)"),
            fn_addr!(molt_object_getstate),
            1,
        )),
        "__lt__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__lt__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_lt_method),
            2,
        )),
        "__le__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__le__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_le_method),
            2,
        )),
        "__gt__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__gt__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_gt_method),
            2,
        )),
        "__ge__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__ge__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_ge_method),
            2,
        )),
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__getattribute__").with_text_signature("($self, name, /)"),
            fn_addr!(molt_object_getattribute),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).object).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(molt_object_new_bound),
            1,
        )),
        "__init__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(molt_object_init),
            1,
        )),
        // Class hooks bind the lookup owner even for class-mode super.,
        "__init_subclass__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).object, "__init_subclass__"),
            fn_addr!(molt_object_init_subclass),
            1,
        )),
        "__setattr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__setattr__").with_text_signature("($self, name, value, /)"),
            fn_addr!(molt_object_setattr),
            3,
        )),
        "__delattr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__delattr__").with_text_signature("($self, name, /)"),
            fn_addr!(molt_object_delattr),
            2,
        )),
        "__eq__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__eq__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_eq),
            2,
        )),
        "__ne__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__ne__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_object_ne),
            2,
        )),
        "__repr__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__repr__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::object_repr_slot),
            1,
        )),
        "__str__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).object, "__str__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::object_str_slot),
            1,
        )),
});

crate::builtins::methods::native_method_table!(memoryview_method_bits, publish_memoryview_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::MemoryView, {}, {
        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::constructor(builtin_classes(_py).memoryview)
                .with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::memoryview_new),
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, "__iter__")
                .with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot), 1,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, "__getitem__")
                .with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin), 2,
        )),
        "_from_flags" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec {
                self_bits: Some(builtin_classes(_py).memoryview),
                ..NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).memoryview, "_from_flags")
            }.with_text_signature("($type, /, object, flags)"),
            fn_addr!(molt_memoryview_from_flags),
            2,
        )),
        "count" if runtime_python_at_least(_py, 3, 14)  => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "count"),
            fn_addr!(molt_memoryview_count),
            2,
        )),
        "index" if runtime_python_at_least(_py, 3, 14)  => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "index"),
            fn_addr!(molt_memoryview_index),
            2,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "hex").with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_memoryview_hex),
            3,
        )),
        "release" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "release").with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_release),
            1,
        )),
        "toreadonly" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "toreadonly").with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_toreadonly),
            1,
        )),
        "tobytes" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "tobytes").with_text_signature("($self, /, order='C')"),
            fn_addr!(crate::object::ops_memoryview::memoryview_tobytes_method),
        )),
        "tolist" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "tolist").with_text_signature("($self, /)"),
            fn_addr!(molt_memoryview_tolist),
            1,
        )),
        "cast" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).memoryview, "cast").with_text_signature("($self, /, format, shape=<unrepresentable>)"),
            fn_addr!(molt_memoryview_cast),
            4,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, "__setitem__").with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).memoryview, "__delitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
});

crate::builtins::methods::native_method_table!(range_method_bits, publish_range_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Range, {

}, {
        "count" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).range, "count"),
            fn_addr!(molt_range_count),
            2,
        )),
        "index" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).range, "index"),
            fn_addr!(molt_range_index),
            2,
        )),
});

pub(crate) extern "C" fn type_setattr(receiver: u64, name: u64, value: u64) -> u64 {
    crate::builtins::attributes::explicit_type_mutate_attr_name(receiver, name, Some(value))
}

pub(crate) extern "C" fn type_delattr(receiver: u64, name: u64) -> u64 {
    crate::builtins::attributes::explicit_type_mutate_attr_name(receiver, name, None)
}
