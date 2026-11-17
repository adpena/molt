use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::*;

crate::builtins::methods::native_method_table!(int_method_bits, publish_int_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Int => [int_richcompare_eq, int_richcompare_ne, int_richcompare_lt, int_richcompare_le, int_richcompare_gt, int_richcompare_ge], {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderGetnewargs),
            fn_ref!(molt_int_getnewargs),
            1,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAdd).with_text_signature("($self, value, /)"),
            fn_ref!(int_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRadd).with_text_signature("($self, value, /)"),
            fn_ref!(int_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderSub).with_text_signature("($self, value, /)"),
            fn_ref!(int_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRsub).with_text_signature("($self, value, /)"),
            fn_ref!(int_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderMul).with_text_signature("($self, value, /)"),
            fn_ref!(int_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRmul).with_text_signature("($self, value, /)"),
            fn_ref!(int_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderTruediv).with_text_signature("($self, value, /)"),
            fn_ref!(int_truediv_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRtruediv).with_text_signature("($self, value, /)"),
            fn_ref!(int_rtruediv_slot), 2,
        )),
        crate::NameId::DunderFloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloordiv).with_text_signature("($self, value, /)"),
            fn_ref!(int_floordiv_slot), 2,
        )),
        crate::NameId::DunderRfloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRfloordiv).with_text_signature("($self, value, /)"),
            fn_ref!(int_rfloordiv_slot), 2,
        )),
        crate::NameId::DunderMod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderMod).with_text_signature("($self, value, /)"),
            fn_ref!(int_mod_slot), 2,
        )),
        crate::NameId::DunderRmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRmod).with_text_signature("($self, value, /)"),
            fn_ref!(int_rmod_slot), 2,
        )),
        crate::NameId::DunderDivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderDivmod).with_text_signature("($self, value, /)"),
            fn_ref!(int_divmod_slot), 2,
        )),
        crate::NameId::DunderRdivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRdivmod).with_text_signature("($self, value, /)"),
            fn_ref!(int_rdivmod_slot), 2,
        )),
        crate::NameId::DunderLshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderLshift).with_text_signature("($self, value, /)"),
            fn_ref!(int_lshift_slot), 2,
        )),
        crate::NameId::DunderRlshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRlshift).with_text_signature("($self, value, /)"),
            fn_ref!(int_rlshift_slot), 2,
        )),
        crate::NameId::DunderRshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRshift).with_text_signature("($self, value, /)"),
            fn_ref!(int_rshift_slot), 2,
        )),
        crate::NameId::DunderRrshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRrshift).with_text_signature("($self, value, /)"),
            fn_ref!(int_rrshift_slot), 2,
        )),
        crate::NameId::DunderAnd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAnd).with_text_signature("($self, value, /)"),
            fn_ref!(int_and_slot), 2,
        )),
        crate::NameId::DunderRand => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRand).with_text_signature("($self, value, /)"),
            fn_ref!(int_rand_slot), 2,
        )),
        crate::NameId::DunderOr => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderOr).with_text_signature("($self, value, /)"),
            fn_ref!(int_or_slot), 2,
        )),
        crate::NameId::DunderRor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRor).with_text_signature("($self, value, /)"),
            fn_ref!(int_ror_slot), 2,
        )),
        crate::NameId::DunderXor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderXor).with_text_signature("($self, value, /)"),
            fn_ref!(int_xor_slot), 2,
        )),
        crate::NameId::DunderRxor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRxor).with_text_signature("($self, value, /)"),
            fn_ref!(int_rxor_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderPow).with_text_signature("($self, value, mod=None, /)"),
            fn_ref!(int_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRpow).with_text_signature("($self, value, mod=None, /)"),
            fn_ref!(int_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderNeg).with_text_signature("($self, /)"),
            fn_ref!(int_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderPos).with_text_signature("($self, /)"),
            fn_ref!(int_pos_slot), 1,
        )),
        crate::NameId::DunderInvert => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderInvert).with_text_signature("($self, /)"),
            fn_ref!(int_invert_slot), 1,
        )),
        crate::NameId::DunderRound => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderRound).with_text_signature(if runtime_python_at_least(_py, 3, 14) { "($self, ndigits=None, /)" } else { "($self, ndigits=<unrepresentable>, /)" }),
            fn_ref!(int_round_slot),
            2,
            &[if runtime_python_at_least(_py, 3, 14) { MoltObject::none().bits() } else { missing_bits(_py) }],
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_ref!(molt_string_format),
            2,
        )),
        crate::NameId::DunderFloat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloat).with_text_signature("($self, /)"),
            fn_ref!(int_float_slot),
            1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAbs).with_text_signature("($self, /)"),
            fn_ref!(int_abs_slot),
            1,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderBool).with_text_signature("($self, /)"),
            fn_ref!(int_bool_slot),
            1,
        )),
        crate::NameId::DunderCeil => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderCeil),
            fn_ref!(molt_int_ceil_method),
            1,
        )),
        crate::NameId::DunderFloor => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloor),
            fn_ref!(molt_int_floor_method),
            1,
        )),
        crate::NameId::DunderTrunc => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderTrunc),
            fn_ref!(molt_int_trunc_method),
            1,
        )),
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).int).with_text_signature("($type, *args, **kwargs)"),
            fn_ref!(int_new),
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_ref!(molt_int_hash_method),
            1,
        )),
        crate::NameId::DunderInt => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderInt).with_text_signature("($self, /)"),
            fn_ref!(molt_int_int),
            1,
        )),
        crate::NameId::DunderIndex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderIndex).with_text_signature("($self, /)"),
            fn_ref!(molt_int_index),
            1,
        )),
        crate::NameId::BitLength => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::BitLength).with_text_signature("($self, /)"),
            fn_ref!(molt_int_bit_length),
            1,
        )),
        crate::NameId::BitCount => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::BitCount).with_text_signature("($self, /)"),
            fn_ref!(molt_int_bit_count),
            1,
        )),
        crate::NameId::AsIntegerRatio => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::AsIntegerRatio).with_text_signature("($self, /)"),
            fn_ref!(molt_int_as_integer_ratio),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::Conjugate),
            fn_ref!(molt_int_conjugate),
            1,
        )),
        crate::NameId::IsInteger => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::IsInteger).with_text_signature("($self, /)"),
            fn_ref!(molt_int_is_integer),
            1,
        )),
        crate::NameId::ToBytes => {
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::ToBytes).with_text_signature("($self, /, length=1, byteorder='big', *, signed=False)"),
                fn_ref!(molt_int_to_bytes),
                4,
                &[
                    MoltObject::from_int(1).bits(),
                    missing_bits(_py),
                    MoltObject::from_bool(false).bits(),
                ],
            ))
        },
});

crate::builtins::methods::native_method_table!(int_class_method_bits, publish_int_class_methods, _py, name, [], {

}, {
        crate::NameId::FromBytes => {
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).int, crate::NameId::FromBytes).with_text_signature("($type, /, bytes, byteorder='big', *, signed=False)"),
                fn_ref!(molt_int_from_bytes),
                4,
                &[missing_bits(_py), MoltObject::from_bool(false).bits()],
            ))
        },
});

crate::builtins::methods::native_method_table!(float_method_bits, publish_float_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Float => [float_richcompare_eq, float_richcompare_ne, float_richcompare_lt, float_richcompare_le, float_richcompare_gt, float_richcompare_ge], {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderGetnewargs),
            fn_ref!(molt_float_getnewargs),
            1,
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderNeg).with_text_signature("($self, /)"),
            fn_ref!(float_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderPos).with_text_signature("($self, /)"),
            fn_ref!(float_pos_slot), 1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderAbs).with_text_signature("($self, /)"),
            fn_ref!(float_abs_slot), 1,
        )),
        crate::NameId::DunderInt => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderInt).with_text_signature("($self, /)"),
            fn_ref!(float_int_slot), 1,
        )),
        crate::NameId::DunderRound => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderRound).with_text_signature("($self, ndigits=None, /)"),
            fn_ref!(float_round_slot),
            2,
            &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderAdd),
            fn_ref!(float_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRadd),
            fn_ref!(float_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderSub),
            fn_ref!(float_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRsub),
            fn_ref!(float_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderMul),
            fn_ref!(float_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRmul),
            fn_ref!(float_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderTruediv),
            fn_ref!(float_truediv_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRtruediv),
            fn_ref!(float_rtruediv_slot), 2,
        )),
        crate::NameId::DunderFloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderFloordiv),
            fn_ref!(float_floordiv_slot), 2,
        )),
        crate::NameId::DunderRfloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRfloordiv),
            fn_ref!(float_rfloordiv_slot), 2,
        )),
        crate::NameId::DunderMod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderMod),
            fn_ref!(float_mod_slot), 2,
        )),
        crate::NameId::DunderRmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRmod),
            fn_ref!(float_rmod_slot), 2,
        )),
        crate::NameId::DunderDivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderDivmod),
            fn_ref!(float_divmod_slot), 2,
        )),
        crate::NameId::DunderRdivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRdivmod),
            fn_ref!(float_rdivmod_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderPow),
            fn_ref!(float_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRpow),
            fn_ref!(float_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_ref!(molt_string_format),
            2,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderBool).with_text_signature("($self, /)"),
            fn_ref!(float_bool_slot), 1,
        )),
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).float).with_text_signature("($type, *args, **kwargs)"),
            fn_ref!(float_new),
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_ref!(molt_float_hash_method),
            1,
        )),
        crate::NameId::DunderFloat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderFloat).with_text_signature("($self, /)"),
            fn_ref!(molt_float_float),
            1,
        )),
        crate::NameId::AsIntegerRatio => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::AsIntegerRatio).with_text_signature("($self, /)"),
            fn_ref!(molt_float_as_integer_ratio),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::Conjugate).with_text_signature("($self, /)"),
            fn_ref!(molt_float_conjugate),
            1,
        )),
        crate::NameId::Hex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::Hex).with_text_signature("($self, /)"),
            fn_ref!(molt_float_hex),
            1,
        )),
        crate::NameId::IsInteger => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::IsInteger).with_text_signature("($self, /)"),
            fn_ref!(molt_float_is_integer),
            1,
        )),
});

crate::builtins::methods::native_method_table!(float_class_method_bits, publish_float_class_methods, _py, name, [], {

}, {
        crate::NameId::Fromhex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::Fromhex).with_text_signature("($type, string, /)"),
            fn_ref!(molt_float_fromhex),
            2,
        )),
        crate::NameId::DunderGetformat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderGetformat).with_text_signature("($type, typestr, /)"),
            fn_ref!(molt_float_getformat),
            2,
        )),
        crate::NameId::FromNumber [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::FromNumber),
            fn_ref!(molt_float_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_class_method_bits, publish_complex_class_methods, _py, name, [], {

}, {
        crate::NameId::FromNumber [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).complex, crate::NameId::FromNumber),
            fn_ref!(molt_complex_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_method_bits, publish_complex_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Complex => [complex_richcompare_eq, complex_richcompare_ne, complex_richcompare_lt, complex_richcompare_le, complex_richcompare_gt, complex_richcompare_ge], {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderGetnewargs),
            fn_ref!(molt_complex_getnewargs),
            1,
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_ref!(molt_string_format),
            2,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderAdd),
            fn_ref!(complex_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRadd),
            fn_ref!(complex_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderSub),
            fn_ref!(complex_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRsub),
            fn_ref!(complex_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderMul),
            fn_ref!(complex_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRmul),
            fn_ref!(complex_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderTruediv),
            fn_ref!(complex_div_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRtruediv),
            fn_ref!(complex_rdiv_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderPow),
            fn_ref!(complex_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRpow),
            fn_ref!(complex_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderNeg),
            fn_ref!(complex_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderPos),
            fn_ref!(complex_pos_slot), 1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderAbs),
            fn_ref!(complex_abs_slot), 1,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderBool),
            fn_ref!(complex_bool_slot), 1,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).complex).with_text_signature("($type, *args, **kwargs)"),
            fn_ref!(complex_new),
        )),
        crate::NameId::DunderComplex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderComplex).with_text_signature("($self, /)"),
            fn_ref!(complex_complex),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::Conjugate).with_text_signature("($self, /)"),
            fn_ref!(molt_complex_conjugate),
            1,
        )),
});

crate::builtins::methods::native_method_table!(bool_method_bits, publish_bool_methods, _py, name, [], {}, {
    crate::NameId::DunderAnd => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderAnd).with_text_signature("($self, value, /)"),
        fn_ref!(bool_and_slot), 2,
    )),
    crate::NameId::DunderRand => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRand).with_text_signature("($self, value, /)"),
        fn_ref!(bool_and_slot), 2,
    )),
    crate::NameId::DunderOr => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderOr).with_text_signature("($self, value, /)"),
        fn_ref!(bool_or_slot), 2,
    )),
    crate::NameId::DunderRor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRor).with_text_signature("($self, value, /)"),
        fn_ref!(bool_or_slot), 2,
    )),
    crate::NameId::DunderXor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderXor).with_text_signature("($self, value, /)"),
        fn_ref!(bool_xor_slot), 2,
    )),
    crate::NameId::DunderRxor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRxor).with_text_signature("($self, value, /)"),
        fn_ref!(bool_xor_slot), 2,
    )),
    crate::NameId::DunderInvert => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderInvert).with_text_signature("($self, /)"),
        fn_ref!(bool_invert_slot), 1,
    )),
});
