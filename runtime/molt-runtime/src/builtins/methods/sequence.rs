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
        "indices" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).slice, "indices"),
            fn_addr!(molt_slice_indices),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).slice, "__hash__").with_text_signature("($self, /)"),
            fn_addr!(molt_slice_hash),
            1,
        )),
        "__reduce__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).slice, "__reduce__"),
            fn_addr!(molt_slice_reduce),
            1,
        )),
        "__reduce_ex__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).object, "__reduce_ex__").with_text_signature("($self, protocol, /)"),
            fn_addr!(molt_slice_reduce_ex),
            2,
        )),
});

crate::builtins::methods::native_method_table!(string_method_bits, publish_string_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::String, {

}, {
        "__format__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "__format__").with_text_signature("($self, format_spec, /)"),
            fn_addr!(molt_string_format),
            2,
        )),
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__mod__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__mod__"),
            fn_addr!(crate::object::ops_arith::native_slots::string_mod_slot), 2,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).str).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::str_new),
        )),
        "__add__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__add__").with_text_signature("($self, value, /)"),
            fn_addr!(molt_str_add_method),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__hash__").with_text_signature("($self, /)"),
            fn_addr!(molt_str_hash_method),
            1,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__getitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_getitem_builtin),
            2,
        )),
        "__str__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__str__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_format::string_str_slot),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).str, "__contains__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "count"),
                fn_addr!(molt_string_count_method),
                4,
                &[none, none],
            ))
        },
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "startswith"),
                fn_addr!(molt_string_startswith_method),
                4,
                &[none, none],
            ))
        },
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "endswith"),
                fn_addr!(molt_string_endswith_method),
                4,
                &[none, none],
            ))
        },
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "find"),
                fn_addr!(molt_string_find_method),
                4,
                &[none, none],
            ))
        },
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rfind"),
                fn_addr!(molt_string_rfind_method),
                4,
                &[none, none],
            ))
        },
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "index"),
                fn_addr!(molt_string_index_method),
                4,
                &[none, none],
            ))
        },
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rindex"),
                fn_addr!(molt_string_rindex_method),
                4,
                &[none, none],
            ))
        },
        "format" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "format"),
            fn_addr!(molt_string_format_method),
            3,
        )),
        "format_map" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "format_map"),
            fn_addr!(molt_string_format_map),
            2,
        )),
        "isidentifier" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isidentifier").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isidentifier),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isdigit").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isdigit),
            1,
        )),
        "isdecimal" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isdecimal").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isdecimal),
            1,
        )),
        "isnumeric" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isnumeric").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isnumeric),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isspace").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isspace),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isalpha").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isalnum").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isalnum),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "islower").with_text_signature("($self, /)"),
            fn_addr!(molt_string_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isupper").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isupper),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isascii").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isascii),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "istitle").with_text_signature("($self, /)"),
            fn_addr!(molt_string_istitle),
            1,
        )),
        "isprintable" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "isprintable").with_text_signature("($self, /)"),
            fn_addr!(molt_string_isprintable),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "upper").with_text_signature("($self, /)"),
            fn_addr!(molt_string_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "lower").with_text_signature("($self, /)"),
            fn_addr!(molt_string_lower),
            1,
        )),
        "casefold" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "casefold").with_text_signature("($self, /)"),
            fn_addr!(molt_string_casefold),
            1,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "capitalize").with_text_signature("($self, /)"),
            fn_addr!(molt_string_capitalize),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "title").with_text_signature("($self, /)"),
            fn_addr!(molt_string_title),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "swapcase").with_text_signature("($self, /)"),
            fn_addr!(molt_string_swapcase),
            1,
        )),
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "strip").with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_strip),
                2,
                &[none],
            ))
        },
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "lstrip").with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_lstrip),
                2,
                &[none],
            ))
        },
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rstrip").with_text_signature("($self, chars=None, /)"),
                fn_addr!(molt_string_rstrip),
                2,
                &[none],
            ))
        },
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "split").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_string_split_max),
                3,
                &[neg_one],
            ))
        },
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rsplit").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_string_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "splitlines").with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_string_splitlines),
                2,
                &[none],
            ))
        },
        "partition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "partition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_string_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rpartition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_string_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "replace").with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_string_replace),
                4,
                &[neg_one],
            ))
        },
        "removeprefix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "removeprefix").with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_string_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "removesuffix").with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_string_removesuffix),
            2,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "zfill").with_text_signature("($self, width, /)"),
            fn_addr!(molt_string_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "center").with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_center),
                3,
                &[miss],
            ))
        },
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "ljust").with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_ljust),
                3,
                &[miss],
            ))
        },
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "rjust").with_text_signature("($self, width, fillchar=' ', /)"),
                fn_addr!(molt_string_rjust),
                3,
                &[miss],
            ))
        },
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "expandtabs").with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_string_expandtabs),
                2,
                &[miss],
            ))
        },
        "join" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "join").with_text_signature("($self, iterable, /)"),
            fn_addr!(molt_string_join),
            2,
        )),
        "translate" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "translate").with_text_signature("($self, table, /)"),
            fn_addr!(molt_string_translate),
            2,
        )),
        "maketrans" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).str, "maketrans"),
                fn_addr!(molt_string_maketrans),
                3,
                &[none, none],
            ))
        },
        "encode" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).str, "encode").with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_string_encode),
            3,
        )),
});

crate::builtins::methods::native_method_table!(bytes_method_bits, publish_bytes_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Bytes, {

}, {
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__add__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__add__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        "__bytes__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "__bytes__"),
            fn_addr!(crate::object::ops_bytes::bytes_bytes), 1,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).bytes).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytes_new),
        )),
        "fromhex" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).bytes, "fromhex").with_text_signature("($type, string, /)"),
            fn_addr!(molt_bytes_fromhex), 2,
        )),

        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytes, "__contains__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "count"),
                fn_addr!(molt_bytes_count_method),
                4,
                &[none, none],
            ))
        },
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "find"),
                fn_addr!(molt_bytes_find_method),
                4,
                &[none, none],
            ))
        },
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "index"),
                fn_addr!(molt_bytes_index_method),
                4,
                &[none, none],
            ))
        },
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rfind"),
                fn_addr!(molt_bytes_rfind_method),
                4,
                &[none, none],
            ))
        },
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rindex"),
                fn_addr!(molt_bytes_rindex_method),
                4,
                &[none, none],
            ))
        },
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "split").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytes_split_max),
                3,
                &[neg_one],
            ))
        },
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rsplit").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytes_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "strip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_strip),
                2,
                &[none],
            ))
        },
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "lstrip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_lstrip),
                2,
                &[none],
            ))
        },
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rstrip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytes_rstrip),
                2,
                &[none],
            ))
        },
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "startswith"),
                fn_addr!(molt_bytes_startswith_method),
                4,
                &[none, none],
            ))
        },
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "endswith"),
                fn_addr!(molt_bytes_endswith_method),
                4,
                &[none, none],
            ))
        },
        "__reversed__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "__reversed__"),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "splitlines").with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_bytes_splitlines),
                2,
                &[none],
            ))
        },
        "partition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "partition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytes_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rpartition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytes_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "replace").with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_bytes_replace),
                4,
                &[neg_one],
            ))
        },
        "removeprefix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "removeprefix").with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_bytes_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "removesuffix").with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_bytes_removesuffix),
            2,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "join").with_text_signature("($self, iterable_of_bytes, /)"),
            fn_addr!(molt_bytes_join),
            2,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "capitalize"),
            fn_addr!(molt_bytes_capitalize),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "upper"),
            fn_addr!(molt_bytes_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "lower"),
            fn_addr!(molt_bytes_lower),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "swapcase"),
            fn_addr!(molt_bytes_swapcase),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "title"),
            fn_addr!(molt_bytes_title),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isalpha"),
            fn_addr!(molt_bytes_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isalnum"),
            fn_addr!(molt_bytes_isalnum),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isdigit"),
            fn_addr!(molt_bytes_isdigit),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isspace"),
            fn_addr!(molt_bytes_isspace),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "islower"),
            fn_addr!(molt_bytes_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isupper"),
            fn_addr!(molt_bytes_isupper),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "istitle"),
            fn_addr!(molt_bytes_istitle),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "isascii"),
            fn_addr!(molt_bytes_isascii),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "hex").with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_bytes_hex),
            3,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "zfill").with_text_signature("($self, width, /)"),
            fn_addr!(molt_bytes_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "center").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_center),
                3,
                &[miss],
            ))
        },
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "ljust").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_ljust),
                3,
                &[miss],
            ))
        },
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "rjust").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytes_rjust),
                3,
                &[miss],
            ))
        },
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "expandtabs").with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_bytes_expandtabs),
                2,
                &[miss],
            ))
        },
        "translate" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "translate").with_text_signature("($self, table, /, delete=b'')"),
                fn_addr!(molt_bytes_translate),
                3,
                &[miss],
            ))
        },
        "maketrans" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).bytes, "maketrans"),
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        "decode" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytes, "decode").with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_bytes_decode),
            3,
        )),
});

crate::builtins::methods::native_method_table!(bytearray_method_bits, publish_bytearray_methods, _py, name, [], comparison: crate::object::ops_compare::SequenceComparison::Bytearray, {

}, {
        "__mul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__mul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__rmul__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_repeat_slot), 2,
        )),
        "__add__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__add__"),
            fn_addr!(crate::object::ops_arith::native_slots::sequence_add_slot), 2,
        )),
        "__iadd__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__iadd__"),
            fn_addr!(crate::object::ops_arith::native_slots::bytearray_iadd_slot), 2,
        )),
        "__imul__" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__imul__"),
            fn_addr!(crate::object::ops_arith::native_slots::bytearray_imul_slot), 2,
        )),

        "__new__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::constructor(builtin_classes(_py).bytearray).with_text_signature("($type, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytearray_new),
        )),
        "__init__" => Some(crate::builtins::methods::builtin_variadic_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__init__").with_text_signature("($self, /, *args, **kwargs)"),
            fn_addr!(crate::builtins::types::native_constructors::bytearray_init),
        )),
        "fromhex" => Some(builtin_func_bits(
            _py, NativeCallableSpec::declared(NativeCallableKind::ClassMethodDescriptor, builtin_classes(_py).bytearray, "fromhex").with_text_signature("($type, string, /)"),
            fn_addr!(molt_bytearray_fromhex), 2,
        )),

        "__iter__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__iter__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__len__").with_text_signature("($self, /)"),
            fn_addr!(crate::object::ops_sys::molt_len_builtin),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__contains__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_contains_builtin),
            2,
        )),
        "extend" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "extend").with_text_signature("($self, iterable_of_ints, /)"),
            fn_addr!(molt_bytearray_extend),
            2,
        )),
        "append" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "append").with_text_signature("($self, item, /)"),
            fn_addr!(molt_bytearray_append),
            2,
        )),
        "insert" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "insert").with_text_signature("($self, index, item, /)"),
            fn_addr!(molt_bytearray_insert),
            3,
        )),
        "pop" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "pop").with_text_signature("($self, index=-1, /)"),
                fn_addr!(molt_bytearray_pop),
                2,
                &[none],
            ))
        },
        "remove" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "remove").with_text_signature("($self, value, /)"),
            fn_addr!(molt_bytearray_remove),
            2,
        )),
        "reverse" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "reverse").with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_reverse),
            1,
        )),
        "resize" if runtime_python_at_least(_py, 3, 14)  => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "resize"),
            fn_addr!(molt_bytearray_resize),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "copy").with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_copy),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "hex").with_text_signature("($self, /, sep=<unrepresentable>, bytes_per_sep=1)"),
            fn_addr!(molt_bytearray_hex),
            3,
        )),
        "translate" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "translate").with_text_signature("($self, table, /, delete=b'')"),
                fn_addr!(molt_bytearray_translate),
                3,
                &[miss],
            ))
        },
        "maketrans" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::Function, builtin_classes(_py).bytearray, "maketrans"),
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "clear").with_text_signature("($self, /)"),
            fn_addr!(molt_bytearray_clear),
            1,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "count"),
                fn_addr!(molt_bytearray_count_method),
                4,
                &[none, none],
            ))
        },
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "find"),
                fn_addr!(molt_bytearray_find_method),
                4,
                &[none, none],
            ))
        },
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "index"),
                fn_addr!(molt_bytearray_index_method),
                4,
                &[none, none],
            ))
        },
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rfind"),
                fn_addr!(molt_bytearray_rfind_method),
                4,
                &[none, none],
            ))
        },
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rindex"),
                fn_addr!(molt_bytearray_rindex_method),
                4,
                &[none, none],
            ))
        },
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "split").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytearray_split_max),
                3,
                &[neg_one],
            ))
        },
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rsplit").with_text_signature("($self, /, sep=None, maxsplit=-1)"),
                fn_addr!(molt_bytearray_rsplit_max),
                3,
                &[neg_one],
            ))
        },
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "strip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_strip),
                2,
                &[none],
            ))
        },
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "lstrip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_lstrip),
                2,
                &[none],
            ))
        },
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rstrip").with_text_signature("($self, bytes=None, /)"),
                fn_addr!(molt_bytearray_rstrip),
                2,
                &[none],
            ))
        },
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "startswith"),
                fn_addr!(molt_bytearray_startswith_method),
                4,
                &[none, none],
            ))
        },
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "endswith"),
                fn_addr!(molt_bytearray_endswith_method),
                4,
                &[none, none],
            ))
        },
        "__reversed__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "__reversed__"),
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__setitem__").with_text_signature("($self, key, value, /)"),
            fn_addr!(crate::object::ops::molt_setitem_builtin_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, builtin_classes(_py).bytearray, "__delitem__").with_text_signature("($self, key, /)"),
            fn_addr!(crate::object::ops::molt_delitem_builtin_method),
            2,
        )),
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "splitlines").with_text_signature("($self, /, keepends=False)"),
                fn_addr!(molt_bytearray_splitlines),
                2,
                &[none],
            ))
        },
        "partition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "partition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytearray_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rpartition").with_text_signature("($self, sep, /)"),
            fn_addr!(molt_bytearray_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "replace").with_text_signature("($self, old, new, count=-1, /)"),
                fn_addr!(molt_bytearray_replace),
                4,
                &[neg_one],
            ))
        },
        "removeprefix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "removeprefix").with_text_signature("($self, prefix, /)"),
            fn_addr!(molt_bytearray_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "removesuffix").with_text_signature("($self, suffix, /)"),
            fn_addr!(molt_bytearray_removesuffix),
            2,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "join").with_text_signature("($self, iterable_of_bytes, /)"),
            fn_addr!(molt_bytearray_join),
            2,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "capitalize"),
            fn_addr!(molt_bytearray_capitalize),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "upper"),
            fn_addr!(molt_bytearray_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "lower"),
            fn_addr!(molt_bytearray_lower),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "swapcase"),
            fn_addr!(molt_bytearray_swapcase),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "title"),
            fn_addr!(molt_bytearray_title),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isalpha"),
            fn_addr!(molt_bytearray_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isalnum"),
            fn_addr!(molt_bytearray_isalnum),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isdigit"),
            fn_addr!(molt_bytearray_isdigit),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isspace"),
            fn_addr!(molt_bytearray_isspace),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "islower"),
            fn_addr!(molt_bytearray_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isupper"),
            fn_addr!(molt_bytearray_isupper),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "istitle"),
            fn_addr!(molt_bytearray_istitle),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "isascii"),
            fn_addr!(molt_bytearray_isascii),
            1,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "zfill").with_text_signature("($self, width, /)"),
            fn_addr!(molt_bytearray_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "center").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_center),
                3,
                &[miss],
            ))
        },
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "ljust").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_ljust),
                3,
                &[miss],
            ))
        },
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "rjust").with_text_signature("($self, width, fillchar=b' ', /)"),
                fn_addr!(molt_bytearray_rjust),
                3,
                &[miss],
            ))
        },
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "expandtabs").with_text_signature("($self, /, tabsize=8)"),
                fn_addr!(molt_bytearray_expandtabs),
                2,
                &[miss],
            ))
        },
        "decode" => Some(builtin_func_bits(
            _py,
            NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, builtin_classes(_py).bytearray, "decode").with_text_signature("($self, /, encoding='utf-8', errors='strict')"),
            fn_addr!(molt_bytearray_decode),
            3,
        )),
});
