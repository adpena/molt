use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::object::ops_hash::{molt_float_hash_method, molt_int_hash_method};
use crate::*;

crate::builtins::methods::native_method_table!(int_method_bits, publish_int_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Int, {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_int_getnewargs),
            1,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAdd).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRadd).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderSub).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRsub).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderMul).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRmul).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderTruediv).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_truediv_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRtruediv).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rtruediv_slot), 2,
        )),
        crate::NameId::DunderFloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloordiv).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_floordiv_slot), 2,
        )),
        crate::NameId::DunderRfloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRfloordiv).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rfloordiv_slot), 2,
        )),
        crate::NameId::DunderMod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderMod).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_mod_slot), 2,
        )),
        crate::NameId::DunderRmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRmod).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rmod_slot), 2,
        )),
        crate::NameId::DunderDivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderDivmod).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_divmod_slot), 2,
        )),
        crate::NameId::DunderRdivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRdivmod).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rdivmod_slot), 2,
        )),
        crate::NameId::DunderLshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderLshift).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_lshift_slot), 2,
        )),
        crate::NameId::DunderRlshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRlshift).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rlshift_slot), 2,
        )),
        crate::NameId::DunderRshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRshift).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rshift_slot), 2,
        )),
        crate::NameId::DunderRrshift => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRrshift).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rrshift_slot), 2,
        )),
        crate::NameId::DunderAnd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAnd).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_and_slot), 2,
        )),
        crate::NameId::DunderRand => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRand).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rand_slot), 2,
        )),
        crate::NameId::DunderOr => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderOr).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_or_slot), 2,
        )),
        crate::NameId::DunderRor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRor).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_ror_slot), 2,
        )),
        crate::NameId::DunderXor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderXor).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_xor_slot), 2,
        )),
        crate::NameId::DunderRxor => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRxor).with_text_signature("($self, value, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rxor_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderPow).with_text_signature("($self, value, mod=None, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderRpow).with_text_signature("($self, value, mod=None, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderNeg).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderPos).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_pos_slot), 1,
        )),
        crate::NameId::DunderInvert => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderInvert).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_invert_slot), 1,
        )),
        crate::NameId::DunderRound => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderRound).with_text_signature(if runtime_python_at_least(_py, 3, 14) { "($self, ndigits=None, /)" } else { "($self, ndigits=<unrepresentable>, /)" }),
            fn_addr!(crate::object::ops_arith::rounding::int_round_slot),
            2,
            &[if runtime_python_at_least(_py, 3, 14) { MoltObject::none().bits() } else { missing_bits(_py) }],
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        crate::NameId::DunderFloat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloat).with_text_signature("($self, /)"),
            fn_addr!(crate::builtins::numbers::int_float_slot),
            1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderAbs).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_abs_slot),
            1,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderBool).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::int_bool_slot),
            1,
        )),
        crate::NameId::DunderCeil => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderCeil),
            fn_addr!(molt_int_ceil_method),
            1,
        )),
        crate::NameId::DunderFloor => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderFloor),
            fn_addr!(molt_int_floor_method),
            1,
        )),
        crate::NameId::DunderTrunc => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::DunderTrunc),
            fn_addr!(molt_int_trunc_method),
            1,
        )),
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).int).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::int_new),
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_addr!(molt_int_hash_method),
            1,
        )),
        crate::NameId::DunderInt => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderInt).with_text_signature("($self, /)"),
            fn_addr!(molt_int_int),
            1,
        )),
        crate::NameId::DunderIndex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).int, crate::NameId::DunderIndex).with_text_signature("($self, /)"),
            fn_addr!(molt_int_index),
            1,
        )),
        crate::NameId::BitLength => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::BitLength).with_text_signature("($self, /)"),
            fn_addr!(molt_int_bit_length),
            1,
        )),
        crate::NameId::BitCount => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::BitCount).with_text_signature("($self, /)"),
            fn_addr!(molt_int_bit_count),
            1,
        )),
        crate::NameId::AsIntegerRatio => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::AsIntegerRatio).with_text_signature("($self, /)"),
            fn_addr!(molt_int_as_integer_ratio),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::Conjugate),
            fn_addr!(molt_int_conjugate),
            1,
        )),
        crate::NameId::IsInteger => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::IsInteger).with_text_signature("($self, /)"),
            fn_addr!(molt_int_is_integer),
            1,
        )),
        crate::NameId::ToBytes => {
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).int, crate::NameId::ToBytes).with_text_signature("($self, /, length=1, byteorder='big', *, signed=False)"),
                fn_addr!(molt_int_to_bytes),
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
                fn_addr!(molt_int_from_bytes),
                4,
                &[missing_bits(_py), MoltObject::from_bool(false).bits()],
            ))
        },
});

crate::builtins::methods::native_method_table!(float_method_bits, publish_float_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Float, {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_float_getnewargs),
            1,
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderNeg).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderPos).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_pos_slot), 1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderAbs).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_abs_slot), 1,
        )),
        crate::NameId::DunderInt => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderInt).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_int_slot), 1,
        )),
        crate::NameId::DunderRound => Some(builtin_func_bits_with_defaults_tuple(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderRound).with_text_signature("($self, ndigits=None, /)"),
            fn_addr!(crate::object::ops_arith::rounding::float_round_slot),
            2,
            &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderAdd),
            fn_addr!(crate::object::ops_arith::native_slots::float_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRadd),
            fn_addr!(crate::object::ops_arith::native_slots::float_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::float_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::float_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::float_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::float_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderTruediv),
            fn_addr!(crate::object::ops_arith::native_slots::float_truediv_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRtruediv),
            fn_addr!(crate::object::ops_arith::native_slots::float_rtruediv_slot), 2,
        )),
        crate::NameId::DunderFloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderFloordiv),
            fn_addr!(crate::object::ops_arith::native_slots::float_floordiv_slot), 2,
        )),
        crate::NameId::DunderRfloordiv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRfloordiv),
            fn_addr!(crate::object::ops_arith::native_slots::float_rfloordiv_slot), 2,
        )),
        crate::NameId::DunderMod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderMod),
            fn_addr!(crate::object::ops_arith::native_slots::float_mod_slot), 2,
        )),
        crate::NameId::DunderRmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRmod),
            fn_addr!(crate::object::ops_arith::native_slots::float_rmod_slot), 2,
        )),
        crate::NameId::DunderDivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderDivmod),
            fn_addr!(crate::object::ops_arith::native_slots::float_divmod_slot), 2,
        )),
        crate::NameId::DunderRdivmod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRdivmod),
            fn_addr!(crate::object::ops_arith::native_slots::float_rdivmod_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderPow),
            fn_addr!(crate::object::ops_arith::native_slots::float_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderRpow),
            fn_addr!(crate::object::ops_arith::native_slots::float_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderBool).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_arith::native_slots::float_bool_slot), 1,
        )),
        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).float).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::float_new),
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_addr!(molt_float_hash_method),
            1,
        )),
        crate::NameId::DunderFloat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).float, crate::NameId::DunderFloat).with_text_signature("($self, /)"),
            fn_addr!(molt_float_float),
            1,
        )),
        crate::NameId::AsIntegerRatio => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::AsIntegerRatio).with_text_signature("($self, /)"),
            fn_addr!(molt_float_as_integer_ratio),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::Conjugate).with_text_signature("($self, /)"),
            fn_addr!(molt_float_conjugate),
            1,
        )),
        crate::NameId::Hex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::Hex).with_text_signature("($self, /)"),
            fn_addr!(molt_float_hex),
            1,
        )),
        crate::NameId::IsInteger => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).float, crate::NameId::IsInteger).with_text_signature("($self, /)"),
            fn_addr!(molt_float_is_integer),
            1,
        )),
});

crate::builtins::methods::native_method_table!(float_class_method_bits, publish_float_class_methods, _py, name, [], {

}, {
        crate::NameId::Fromhex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::Fromhex).with_text_signature("($type, string, /)"),
            fn_addr!(molt_float_fromhex),
            2,
        )),
        crate::NameId::DunderGetformat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::DunderGetformat).with_text_signature("($type, typestr, /)"),
            fn_addr!(molt_float_getformat),
            2,
        )),
        crate::NameId::FromNumber [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).float, crate::NameId::FromNumber),
            fn_addr!(molt_float_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_class_method_bits, publish_complex_class_methods, _py, name, [], {

}, {
        crate::NameId::FromNumber [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).complex, crate::NameId::FromNumber),
            fn_addr!(molt_complex_from_number),
            2,
        )),
});

crate::builtins::methods::native_method_table!(complex_method_bits, publish_complex_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Complex, {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_complex_getnewargs),
            1,
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderAdd),
            fn_addr!(crate::object::ops_arith::native_slots::complex_add_slot), 2,
        )),
        crate::NameId::DunderRadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRadd),
            fn_addr!(crate::object::ops_arith::native_slots::complex_radd_slot), 2,
        )),
        crate::NameId::DunderSub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderSub),
            fn_addr!(crate::object::ops_arith::native_slots::complex_sub_slot), 2,
        )),
        crate::NameId::DunderRsub => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRsub),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rsub_slot), 2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::complex_mul_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rmul_slot), 2,
        )),
        crate::NameId::DunderTruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderTruediv),
            fn_addr!(crate::object::ops_arith::native_slots::complex_div_slot), 2,
        )),
        crate::NameId::DunderRtruediv => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRtruediv),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rdiv_slot), 2,
        )),
        crate::NameId::DunderPow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderPow),
            fn_addr!(crate::object::ops_arith::native_slots::complex_pow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderRpow => Some(builtin_func_bits_with_defaults_tuple(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderRpow),
            fn_addr!(crate::object::ops_arith::native_slots::complex_rpow_slot), 3, &[MoltObject::none().bits()],
        )),
        crate::NameId::DunderNeg => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderNeg),
            fn_addr!(crate::object::ops_arith::native_slots::complex_neg_slot), 1,
        )),
        crate::NameId::DunderPos => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderPos),
            fn_addr!(crate::object::ops_arith::native_slots::complex_pos_slot), 1,
        )),
        crate::NameId::DunderAbs => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderAbs),
            fn_addr!(crate::object::ops_arith::native_slots::complex_abs_slot), 1,
        )),
        crate::NameId::DunderBool => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).complex, crate::NameId::DunderBool),
            fn_addr!(crate::object::ops_arith::native_slots::complex_bool_slot), 1,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).complex).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::complex_new),
        )),
        crate::NameId::DunderComplex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::DunderComplex).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_convert::complex_complex),
            1,
        )),
        crate::NameId::Conjugate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).complex, crate::NameId::Conjugate).with_text_signature("($self, /)"),
            fn_addr!(molt_complex_conjugate),
            1,
        )),
});

crate::builtins::methods::native_method_table!(bool_method_bits, publish_bool_methods, _py, name, [], {}, {
    crate::NameId::DunderAnd => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderAnd).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_and_slot), 2,
    )),
    crate::NameId::DunderRand => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRand).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_and_slot), 2,
    )),
    crate::NameId::DunderOr => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderOr).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_or_slot), 2,
    )),
    crate::NameId::DunderRor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRor).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_or_slot), 2,
    )),
    crate::NameId::DunderXor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderXor).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_xor_slot), 2,
    )),
    crate::NameId::DunderRxor => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderRxor).with_text_signature("($self, value, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_xor_slot), 2,
    )),
    crate::NameId::DunderInvert => Some(builtin_func_bits(
        _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bool, crate::NameId::DunderInvert).with_text_signature("($self, /)"),
        fn_addr!(crate::object::ops_arith::native_slots::bool_invert_slot), 1,
    )),
});
