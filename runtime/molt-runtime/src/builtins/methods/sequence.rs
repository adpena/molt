use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use super::singletons::missing_bits;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
use crate::object::ops_hash::molt_str_hash_method;
use crate::*;

crate::builtins::methods::native_method_table!(slice_method_bits, publish_slice_methods, _py, name, [],
    comparison: crate::object::ops_compare::builtin_families::BuiltinComparison::Slice, {

}, {
        crate::NameId::Indices => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).slice, crate::NameId::Indices),
            fn_addr!(molt_slice_indices),
            2,
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).slice, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_addr!(molt_slice_hash),
            1,
        )),
        crate::NameId::DunderReduce => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).slice, crate::NameId::DunderReduce),
            fn_addr!(molt_slice_reduce),
            1,
        )),
        crate::NameId::DunderReduceEx => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, crate::NameId::DunderReduceEx).with_text_signature("($self, protocol, /)"),
            fn_addr!(molt_slice_reduce_ex),
            2,
        )),
});

crate::builtins::methods::native_method_table!(string_method_bits, publish_string_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::String, {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_str_getnewargs),
            1,
        )),
        crate::NameId::DunderFormat => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::DunderFormat).with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderMod => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderMod),
            fn_addr!(crate::object::ops_arith::native_slots::string_mod_slot), 2,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).str).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::str_new),
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderAdd).with_text_signature("($self, value, /)"),
            fn_addr!(molt_str_add_method),
            2,
        )),
        crate::NameId::DunderHash => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderHash).with_text_signature("($self, /)"),
            fn_addr!(molt_str_hash_method),
            1,
        )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        crate::NameId::DunderStr => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderStr).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::string_str_slot),
            1,
        )),
        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        crate::NameId::Count => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Count),
                fn_addr!(molt_string_count_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Startswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Startswith),
                fn_addr!(molt_string_startswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Endswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Endswith),
                fn_addr!(molt_string_endswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Find => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Find),
                fn_addr!(molt_string_find_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rfind => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rfind),
                fn_addr!(molt_string_rfind_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Index => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Index),
                fn_addr!(molt_string_index_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rindex => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rindex),
                fn_addr!(molt_string_rindex_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Format => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Format),
            fn_addr!(molt_string_format_method),
            3,
        )),
        crate::NameId::FormatMap => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::FormatMap),
            fn_addr!(molt_string_format_map),
            2,
        )),
        crate::NameId::Isidentifier => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isidentifier).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isidentifier),
            1,
        )),
        crate::NameId::Isdigit => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isdigit).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isdigit),
            1,
        )),
        crate::NameId::Isdecimal => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isdecimal).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isdecimal),
            1,
        )),
        crate::NameId::Isnumeric => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isnumeric).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isnumeric),
            1,
        )),
        crate::NameId::Isspace => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isspace).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isspace),
            1,
        )),
        crate::NameId::Isalpha => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isalpha).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isalpha),
            1,
        )),
        crate::NameId::Isalnum => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isalnum).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isalnum),
            1,
        )),
        crate::NameId::Islower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Islower).with_text_signature("($self, /)"),
            fn_addr!(molt_string_islower),
            1,
        )),
        crate::NameId::Isupper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isupper).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isupper),
            1,
        )),
        crate::NameId::Isascii => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isascii).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isascii),
            1,
        )),
        crate::NameId::Istitle => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Istitle).with_text_signature("($self, /)"),
            fn_addr!(molt_string_istitle),
            1,
        )),
        crate::NameId::Isprintable => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Isprintable).with_text_signature("($self, /)"),
            fn_addr!(molt_string_isprintable),
            1,
        )),
        crate::NameId::Upper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Upper).with_text_signature("($self, /)"),
            fn_addr!(molt_string_upper),
            1,
        )),
        crate::NameId::Lower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Lower).with_text_signature("($self, /)"),
            fn_addr!(molt_string_lower),
            1,
        )),
        crate::NameId::Casefold => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Casefold).with_text_signature("($self, /)"),
            fn_addr!(molt_string_casefold),
            1,
        )),
        crate::NameId::Capitalize => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Capitalize).with_text_signature("($self, /)"),
            fn_addr!(molt_string_capitalize),
            1,
        )),
        crate::NameId::Title => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Title).with_text_signature("($self, /)"),
            fn_addr!(molt_string_title),
            1,
        )),
        crate::NameId::Swapcase => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Swapcase).with_text_signature("($self, /)"),
            fn_addr!(molt_string_swapcase),
            1,
        )),
        crate::NameId::Strip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Strip).with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_strip),
                2,
                &[none],
            ))
        },
        crate::NameId::Lstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Lstrip).with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_lstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Rstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rstrip).with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_rstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Split => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Split).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_string_split_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Rsplit => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rsplit).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_string_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Splitlines => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Splitlines).with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_string_splitlines),
                2,
                &[none],
            ))
        },
        crate::NameId::Partition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Partition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_string_partition),
            2,
        )),
        crate::NameId::Rpartition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rpartition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_string_rpartition),
            2,
        )),
        crate::NameId::Replace => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Replace).with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_string_replace),
                4,
                &[neg_one],
            ))
        },
        crate::NameId::Removeprefix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Removeprefix).with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_string_removeprefix),
            2,
        )),
        crate::NameId::Removesuffix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Removesuffix).with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_string_removesuffix),
            2,
        )),
        crate::NameId::Zfill => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Zfill).with_text_signature("($self, width, /)"),
            fn_addr!(molt_string_zfill),
            2,
        )),
        crate::NameId::Center => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Center).with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_center),
                3,
                &[miss],
            ))
        },
        crate::NameId::Ljust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Ljust).with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_ljust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Rjust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Rjust).with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_rjust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Expandtabs => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Expandtabs).with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_string_expandtabs),
                2,
                &[miss],
            ))
        },
        crate::NameId::Join => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Join).with_text_signature("($self, iterable, /)"),
            fn_addr!(molt_string_join),
            2,
        )),
        crate::NameId::Translate => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Translate).with_text_signature("($self, table, /)"),
            fn_addr!(molt_string_translate),
            2,
        )),
        crate::NameId::Maketrans => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).str, crate::NameId::Maketrans),
                fn_addr!(molt_string_maketrans),
                3,
                &[none, none],
            ))
        },
        crate::NameId::Encode => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, crate::NameId::Encode).with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_string_encode),
            3,
        )),
});

crate::builtins::methods::native_method_table!(bytes_method_bits, publish_bytes_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Bytes, {

}, {
        crate::NameId::DunderGetnewargs => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderGetnewargs),
            fn_addr!(crate::molt_bytes_getnewargs),
            1,
        )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderAdd),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        crate::NameId::DunderBytes => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderBytes),
            fn_addr!(crate::object::ops_bytes::bytes_bytes), 1,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).bytes).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytes_new),
        )),
        crate::NameId::Fromhex => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Fromhex).with_text_signature("($type, string, /)"),
            fn_addr!(molt_bytes_fromhex), 2,
        )),

        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        crate::NameId::Count => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Count),
                fn_addr!(molt_bytes_count_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Find => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Find),
                fn_addr!(molt_bytes_find_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Index => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Index),
                fn_addr!(molt_bytes_index_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rfind => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rfind),
                fn_addr!(molt_bytes_rfind_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rindex => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rindex),
                fn_addr!(molt_bytes_rindex_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Split => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Split).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytes_split_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Rsplit => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rsplit).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytes_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Strip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Strip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_strip),
                2,
                &[none],
            ))
        },
        crate::NameId::Lstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Lstrip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_lstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Rstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rstrip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_rstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Startswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Startswith),
                fn_addr!(molt_bytes_startswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Endswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Endswith),
                fn_addr!(molt_bytes_endswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::DunderReversed => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::DunderReversed),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        crate::NameId::Splitlines => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Splitlines).with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_bytes_splitlines),
                2,
                &[none],
            ))
        },
        crate::NameId::Partition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Partition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytes_partition),
            2,
        )),
        crate::NameId::Rpartition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rpartition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytes_rpartition),
            2,
        )),
        crate::NameId::Replace => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Replace).with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_bytes_replace),
                4,
                &[neg_one],
            ))
        },
        crate::NameId::Removeprefix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Removeprefix).with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_bytes_removeprefix),
            2,
        )),
        crate::NameId::Removesuffix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Removesuffix).with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_bytes_removesuffix),
            2,
        )),
        crate::NameId::Join => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Join).with_text_signature("($self, iterable_of_bytes, /)"),
            fn_addr!(molt_bytes_join),
            2,
        )),
        crate::NameId::Capitalize => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Capitalize),
            fn_addr!(molt_bytes_capitalize),
            1,
        )),
        crate::NameId::Upper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Upper),
            fn_addr!(molt_bytes_upper),
            1,
        )),
        crate::NameId::Lower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Lower),
            fn_addr!(molt_bytes_lower),
            1,
        )),
        crate::NameId::Swapcase => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Swapcase),
            fn_addr!(molt_bytes_swapcase),
            1,
        )),
        crate::NameId::Title => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Title),
            fn_addr!(molt_bytes_title),
            1,
        )),
        crate::NameId::Isalpha => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isalpha),
            fn_addr!(molt_bytes_isalpha),
            1,
        )),
        crate::NameId::Isalnum => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isalnum),
            fn_addr!(molt_bytes_isalnum),
            1,
        )),
        crate::NameId::Isdigit => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isdigit),
            fn_addr!(molt_bytes_isdigit),
            1,
        )),
        crate::NameId::Isspace => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isspace),
            fn_addr!(molt_bytes_isspace),
            1,
        )),
        crate::NameId::Islower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Islower),
            fn_addr!(molt_bytes_islower),
            1,
        )),
        crate::NameId::Isupper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isupper),
            fn_addr!(molt_bytes_isupper),
            1,
        )),
        crate::NameId::Istitle => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Istitle),
            fn_addr!(molt_bytes_istitle),
            1,
        )),
        crate::NameId::Isascii => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Isascii),
            fn_addr!(molt_bytes_isascii),
            1,
        )),
        crate::NameId::Hex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Hex).with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_bytes_hex),
            3,
        )),
        crate::NameId::Zfill => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Zfill).with_text_signature("($self, width, /)"),
            fn_addr!(molt_bytes_zfill),
            2,
        )),
        crate::NameId::Center => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Center).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_center),
                3,
                &[miss],
            ))
        },
        crate::NameId::Ljust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Ljust).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_ljust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Rjust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Rjust).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_rjust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Expandtabs => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Expandtabs).with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_bytes_expandtabs),
                2,
                &[miss],
            ))
        },
        crate::NameId::Translate => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Translate).with_text_signature("($self, table, /, delete=b'')"),
                fn_addr!(molt_bytes_translate),
                3,
                &[miss],
            ))
        },
        crate::NameId::Maketrans => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).bytes, crate::NameId::Maketrans),
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        crate::NameId::Decode => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, crate::NameId::Decode).with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_bytes_decode),
            3,
        )),
});

crate::builtins::methods::native_method_table!(bytearray_method_bits, publish_bytearray_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Bytearray, {

}, {
        crate::NameId::DunderReduceEx => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderReduceEx),
            fn_addr!(crate::molt_bytearray_reduce_ex),
            2,
        )),
        crate::NameId::DunderReduce => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderReduce),
            fn_addr!(crate::molt_bytearray_reduce),
            1,
        )),
        crate::NameId::DunderGetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderGetitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        crate::NameId::DunderMul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderMul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderRmul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderRmul),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        crate::NameId::DunderAdd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderAdd),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        crate::NameId::DunderIadd => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderIadd),
            fn_addr!(crate::object::ops_arith::native_slots::bytearray_iadd_slot), 2,
        )),
        crate::NameId::DunderImul => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderImul),
            fn_addr!(crate::object::ops_arith::native_slots::bytearray_imul_slot), 2,
        )),

        crate::NameId::DunderNew => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).bytearray).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytearray_new),
        )),
        crate::NameId::DunderInit => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderInit).with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytearray_init),
        )),
        crate::NameId::Fromhex => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Fromhex).with_text_signature("($type, string, /)"),
            fn_addr!(molt_bytearray_fromhex), 2,
        )),

        crate::NameId::DunderIter => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderIter).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        crate::NameId::DunderLen => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderLen).with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        crate::NameId::DunderContains => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderContains).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        crate::NameId::Extend => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Extend).with_text_signature("($self, iterable_of_ints, /)"),
            fn_addr!(molt_bytearray_extend),
            2,
        )),
        crate::NameId::Append => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Append).with_text_signature("($self, item, /)"),
            fn_addr!(molt_bytearray_append),
            2,
        )),
        crate::NameId::Insert => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Insert).with_text_signature("($self, index, item, /)"),
            fn_addr!(molt_bytearray_insert),
            3,
        )),
        crate::NameId::Pop => {
            let last_index = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Pop).with_text_signature("($self, index=-1, /)"),
                fn_addr!(molt_bytearray_pop),
                2,
                &[last_index],
            ))
        },
        crate::NameId::Remove => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Remove).with_text_signature("($self, value, /)"),
            fn_addr!(molt_bytearray_remove),
            2,
        )),
        crate::NameId::Reverse => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Reverse).with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_reverse),
            1,
        )),
        crate::NameId::Resize [if runtime_python_at_least(_py, 3, 14)  ] => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Resize),
            fn_addr!(molt_bytearray_resize),
            2,
        )),
        crate::NameId::Copy => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Copy).with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_copy),
            1,
        )),
        crate::NameId::Hex => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Hex).with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_bytearray_hex),
            3,
        )),
        crate::NameId::Translate => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Translate).with_text_signature("($self, table, /, delete=b'')"),
                fn_addr!(molt_bytearray_translate),
                3,
                &[miss],
            ))
        },
        crate::NameId::Maketrans => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).bytearray, crate::NameId::Maketrans),
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        crate::NameId::Clear => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Clear).with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_clear),
            1,
        )),
        crate::NameId::Count => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Count),
                fn_addr!(molt_bytearray_count_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Find => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Find),
                fn_addr!(molt_bytearray_find_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Index => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Index),
                fn_addr!(molt_bytearray_index_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rfind => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rfind),
                fn_addr!(molt_bytearray_rfind_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Rindex => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rindex),
                fn_addr!(molt_bytearray_rindex_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Split => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Split).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytearray_split_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Rsplit => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rsplit).with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytearray_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        crate::NameId::Strip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Strip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_strip),
                2,
                &[none],
            ))
        },
        crate::NameId::Lstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Lstrip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_lstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Rstrip => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rstrip).with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_rstrip),
                2,
                &[none],
            ))
        },
        crate::NameId::Startswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Startswith),
                fn_addr!(molt_bytearray_startswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::Endswith => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Endswith),
                fn_addr!(molt_bytearray_endswith_method),
                4,
                &[none, none],
            ))
        },
        crate::NameId::DunderReversed => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderReversed),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        crate::NameId::DunderSetitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderSetitem).with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        crate::NameId::DunderDelitem => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, crate::NameId::DunderDelitem).with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
        crate::NameId::Splitlines => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Splitlines).with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_bytearray_splitlines),
                2,
                &[none],
            ))
        },
        crate::NameId::Partition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Partition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytearray_partition),
            2,
        )),
        crate::NameId::Rpartition => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rpartition).with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytearray_rpartition),
            2,
        )),
        crate::NameId::Replace => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Replace).with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_bytearray_replace),
                4,
                &[neg_one],
            ))
        },
        crate::NameId::Removeprefix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Removeprefix).with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_bytearray_removeprefix),
            2,
        )),
        crate::NameId::Removesuffix => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Removesuffix).with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_bytearray_removesuffix),
            2,
        )),
        crate::NameId::Join => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Join).with_text_signature("($self, iterable_of_bytes, /)"),
            fn_addr!(molt_bytearray_join),
            2,
        )),
        crate::NameId::Capitalize => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Capitalize),
            fn_addr!(molt_bytearray_capitalize),
            1,
        )),
        crate::NameId::Upper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Upper),
            fn_addr!(molt_bytearray_upper),
            1,
        )),
        crate::NameId::Lower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Lower),
            fn_addr!(molt_bytearray_lower),
            1,
        )),
        crate::NameId::Swapcase => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Swapcase),
            fn_addr!(molt_bytearray_swapcase),
            1,
        )),
        crate::NameId::Title => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Title),
            fn_addr!(molt_bytearray_title),
            1,
        )),
        crate::NameId::Isalpha => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isalpha),
            fn_addr!(molt_bytearray_isalpha),
            1,
        )),
        crate::NameId::Isalnum => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isalnum),
            fn_addr!(molt_bytearray_isalnum),
            1,
        )),
        crate::NameId::Isdigit => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isdigit),
            fn_addr!(molt_bytearray_isdigit),
            1,
        )),
        crate::NameId::Isspace => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isspace),
            fn_addr!(molt_bytearray_isspace),
            1,
        )),
        crate::NameId::Islower => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Islower),
            fn_addr!(molt_bytearray_islower),
            1,
        )),
        crate::NameId::Isupper => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isupper),
            fn_addr!(molt_bytearray_isupper),
            1,
        )),
        crate::NameId::Istitle => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Istitle),
            fn_addr!(molt_bytearray_istitle),
            1,
        )),
        crate::NameId::Isascii => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Isascii),
            fn_addr!(molt_bytearray_isascii),
            1,
        )),
        crate::NameId::Zfill => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Zfill).with_text_signature("($self, width, /)"),
            fn_addr!(molt_bytearray_zfill),
            2,
        )),
        crate::NameId::Center => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Center).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_center),
                3,
                &[miss],
            ))
        },
        crate::NameId::Ljust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Ljust).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_ljust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Rjust => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Rjust).with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_rjust),
                3,
                &[miss],
            ))
        },
        crate::NameId::Expandtabs => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Expandtabs).with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_bytearray_expandtabs),
                2,
                &[miss],
            ))
        },
        crate::NameId::Decode => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, crate::NameId::Decode).with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_bytearray_decode),
            3,
        )),
});
