use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::object::ops_hash::{molt_float_hash_method, molt_int_hash_method};
use crate::*;

crate::builtins::methods::native_method_table!(int_method_bits, publish_int_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Int, {

}, {
        "__round__" => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "__round__").with_text_signature(if runtime_python_at_least(_py, 3, 14) { "($self, ndigits=None, /)" } else { "($self, ndigits=<unrepresentable>, /)" }),
            fn_addr!(crate::object::ops_arith::rounding::int_round_slot),
            2,
            &[if runtime_python_at_least(_py, 3, 14) { MoltObject::none().bits() } else { missing_bits(_py) }],
        )),
        "__format__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "__format__").with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        "__float__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__float__").with_text_signature("($self, /)"),
            fn_addr!(crate::builtins::numbers::int_float_slot),
            1,
        )),
        "__abs__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__abs__").with_text_signature("($self, /)"),
            fn_addr!(molt_int_abs_method),
            1,
        )),
        "__add__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__add__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_int_add_method),
            2,
        )),
        "__and__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__and__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_int_and_method),
            2,
        )),
        "__bool__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__bool__").with_text_signature("($self, /)"),
            fn_addr!(molt_int_bool_method),
            1,
        )),
        "__ceil__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "__ceil__"),
            fn_addr!(molt_int_ceil_method),
            1,
        )),
        "__divmod__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__divmod__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_int_divmod_method),
            2,
        )),
        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).int).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::int_new),
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__hash__").with_text_signature("($self, /)"),
            fn_addr!(molt_int_hash_method),
            1,
        )),
        "__int__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__int__").with_text_signature("($self, /)"),
            fn_addr!(molt_int_int),
            1,
        )),
        "__index__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, "__index__").with_text_signature("($self, /)"),
            fn_addr!(molt_int_index),
            1,
        )),
        "bit_length" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "bit_length").with_text_signature("($self, /)"),
            fn_addr!(molt_int_bit_length),
            1,
        )),
        "bit_count" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "bit_count").with_text_signature("($self, /)"),
            fn_addr!(molt_int_bit_count),
            1,
        )),
        "as_integer_ratio" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "as_integer_ratio").with_text_signature("($self, /)"),
            fn_addr!(molt_int_as_integer_ratio),
            1,
        )),
        "conjugate" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "conjugate"),
            fn_addr!(molt_int_conjugate),
            1,
        )),
        "is_integer" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "is_integer").with_text_signature("($self, /)"),
            fn_addr!(molt_int_is_integer),
            1,
        )),
        "to_bytes" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, "to_bytes").with_text_signature("($self, /, length=1, byteorder='big', *, signed=False)"),
                fn_addr!(molt_int_to_bytes),
                4,
                &[zero],
            ))
        },
});

crate::builtins::methods::native_method_table!(int_class_method_bits, publish_int_class_methods, _py, name, [], {

}, {
        "from_bytes" => {
            let zero = MoltObject::from_int(0).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).int, "from_bytes").with_text_signature("($type, /, bytes, byteorder='big', *, signed=False)"),
                fn_addr!(molt_int_from_bytes),
                4,
                &[zero],
            ))
        },
});

crate::builtins::methods::native_method_table!(float_method_bits, publish_float_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Float, {

}, {
        "__round__" => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "__round__").with_text_signature("($self, ndigits=None, /)"),
            fn_addr!(crate::object::ops_arith::rounding::float_round_slot),
            2,
            &[MoltObject::none().bits()],
        )),
        "__add__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__add__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_add_slot), 2,
        )),
        "__radd__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__radd__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_radd_slot), 2,
        )),
        "__sub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_sub_slot), 2,
        )),
        "__rsub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rsub_slot), 2,
        )),
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_mul_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rmul_slot), 2,
        )),
        "__truediv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__truediv__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_truediv_slot), 2,
        )),
        "__rtruediv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rtruediv__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rtruediv_slot), 2,
        )),
        "__floordiv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__floordiv__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_floordiv_slot), 2,
        )),
        "__rfloordiv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rfloordiv__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rfloordiv_slot), 2,
        )),
        "__mod__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__mod__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_mod_slot), 2,
        )),
        "__rmod__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rmod__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rmod_slot), 2,
        )),
        "__divmod__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__divmod__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_divmod_slot), 2,
        )),
        "__rdivmod__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rdivmod__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rdivmod_slot), 2,
        )),
        "__pow__" => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__pow__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        "__rpow__" => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__rpow__"),
            fn_addr!(crate::object::ops_arith::native_slots::float_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        "__format__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "__format__").with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        "__bool__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__bool__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_bool_slot), 1,
        )),
        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).float).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::float_new),
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__hash__").with_text_signature("($self, /)"),
            fn_addr!(molt_float_hash_method),
            1,
        )),
        "__float__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, "__float__").with_text_signature("($self, /)"),
            fn_addr!(molt_float_float),
            1,
        )),
        "as_integer_ratio" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "as_integer_ratio").with_text_signature("($self, /)"),
            fn_addr!(molt_float_as_integer_ratio),
            1,
        )),
        "conjugate" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "conjugate").with_text_signature("($self, /)"),
            fn_addr!(molt_float_conjugate),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "hex").with_text_signature("($self, /)"),
            fn_addr!(molt_float_hex),
            1,
        )),
        "is_integer" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, "is_integer").with_text_signature("($self, /)"),
            fn_addr!(molt_float_is_integer),
            1,
        )),
});

crate::builtins::methods::native_method_table!(float_class_method_bits, publish_float_class_methods, _py, name, [], {

}, {
        "fromhex" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, "fromhex").with_text_signature("($type, string, /)"),
            fn_addr!(molt_float_fromhex),
            2,
        )),
        "from_number" if runtime_python_at_least(_py, 3, 14)  => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, "from_number"),
            fn_addr!(molt_float_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_class_method_bits, publish_complex_class_methods, _py, name, [], {

}, {
        "from_number" if runtime_python_at_least(_py, 3, 14)  => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).complex, "from_number"),
            fn_addr!(molt_complex_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_method_bits, publish_complex_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Complex, {

}, {
        "__format__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, "__format__").with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        "__add__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__add__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_add_slot), 2,
        )),
        "__radd__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__radd__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_radd_slot), 2,
        )),
        "__sub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__sub__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_sub_slot), 2,
        )),
        "__rsub__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__rsub__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rsub_slot), 2,
        )),
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_mul_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rmul_slot), 2,
        )),
        "__truediv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__truediv__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_div_slot), 2,
        )),
        "__rtruediv__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__rtruediv__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rdiv_slot), 2,
        )),
        "__pow__" => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__pow__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        "__rpow__" => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__rpow__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        "__neg__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__neg__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_neg_slot), 1,
        )),
        "__pos__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__pos__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_pos_slot), 1,
        )),
        "__abs__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__abs__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_abs_slot), 1,
        )),
        "__bool__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, "__bool__"),
            fn_addr!(crate::object::ops_arith::native_slots::complex_bool_slot), 1,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).complex).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::complex_new),
        )),
        "__complex__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, "__complex__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_convert::complex_complex),
            1,
        )),
        "conjugate" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, "conjugate").with_text_signature("($self, /)"),
            fn_addr!(molt_complex_conjugate),
            1,
        )),
});
