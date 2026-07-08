use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, runtime_python_at_least,
};
use super::singletons::missing_bits;
use crate::PyToken;
use crate::object::ops_hash::molt_str_hash_method;
use crate::*;

pub(crate) fn slice_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "indices" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_indices,
            fn_addr!(molt_slice_indices),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_hash,
            fn_addr!(molt_slice_hash),
            1,
        )),
        "__eq__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_eq,
            fn_addr!(molt_slice_eq),
            2,
        )),
        "__reduce__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_reduce,
            fn_addr!(molt_slice_reduce),
            1,
        )),
        "__reduce_ex__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_reduce_ex,
            fn_addr!(molt_slice_reduce_ex),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn string_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__add__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_add,
            fn_addr!(molt_str_add_method),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_hash,
            fn_addr!(molt_str_hash_method),
            1,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_getitem,
            fn_addr!(molt_getitem_method),
            2,
        )),
        "__str__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_str,
            fn_addr!(molt_str_from_obj),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_count,
                fn_addr!(molt_string_count_method),
                4,
                &[none, none],
            ))
        }
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_startswith,
                fn_addr!(molt_string_startswith_method),
                4,
                &[none, none],
            ))
        }
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_endswith,
                fn_addr!(molt_string_endswith_method),
                4,
                &[none, none],
            ))
        }
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_find,
                fn_addr!(molt_string_find_method),
                4,
                &[none, none],
            ))
        }
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_rfind,
                fn_addr!(molt_string_rfind_method),
                4,
                &[none, none],
            ))
        }
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_index,
                fn_addr!(molt_string_index_method),
                4,
                &[none, none],
            ))
        }
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_rindex,
                fn_addr!(molt_string_rindex_method),
                4,
                &[none, none],
            ))
        }
        "format" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_format,
            fn_addr!(molt_string_format_method),
            3,
        )),
        "format_map" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_format_map,
            fn_addr!(molt_string_format_map),
            2,
        )),
        "isidentifier" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isidentifier,
            fn_addr!(molt_string_isidentifier),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isdigit,
            fn_addr!(molt_string_isdigit),
            1,
        )),
        "isdecimal" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isdecimal,
            fn_addr!(molt_string_isdecimal),
            1,
        )),
        "isnumeric" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isnumeric,
            fn_addr!(molt_string_isnumeric),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isspace,
            fn_addr!(molt_string_isspace),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isalpha,
            fn_addr!(molt_string_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isalnum,
            fn_addr!(molt_string_isalnum),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_islower,
            fn_addr!(molt_string_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isupper,
            fn_addr!(molt_string_isupper),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isascii,
            fn_addr!(molt_string_isascii),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_istitle,
            fn_addr!(molt_string_istitle),
            1,
        )),
        "isprintable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isprintable,
            fn_addr!(molt_string_isprintable),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_upper,
            fn_addr!(molt_string_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_lower,
            fn_addr!(molt_string_lower),
            1,
        )),
        "casefold" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_casefold,
            fn_addr!(molt_string_casefold),
            1,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_capitalize,
            fn_addr!(molt_string_capitalize),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_title,
            fn_addr!(molt_string_title),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_swapcase,
            fn_addr!(molt_string_swapcase),
            1,
        )),
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_strip,
                fn_addr!(molt_string_strip),
                2,
                &[none],
            ))
        }
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_lstrip,
                fn_addr!(molt_string_lstrip),
                2,
                &[none],
            ))
        }
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_rstrip,
                fn_addr!(molt_string_rstrip),
                2,
                &[none],
            ))
        }
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_split,
                fn_addr!(molt_string_split_max),
                3,
                &[neg_one],
            ))
        }
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_rsplit,
                fn_addr!(molt_string_rsplit_max),
                3,
                &[neg_one],
            ))
        }
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_splitlines,
                fn_addr!(molt_string_splitlines),
                2,
                &[none],
            ))
        }
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_partition,
            fn_addr!(molt_string_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_rpartition,
            fn_addr!(molt_string_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_replace,
                fn_addr!(molt_string_replace),
                4,
                &[neg_one],
            ))
        }
        "removeprefix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_removeprefix,
            fn_addr!(molt_string_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_removesuffix,
            fn_addr!(molt_string_removesuffix),
            2,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_zfill,
            fn_addr!(molt_string_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_center,
                fn_addr!(molt_string_center),
                3,
                &[miss],
            ))
        }
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_ljust,
                fn_addr!(molt_string_ljust),
                3,
                &[miss],
            ))
        }
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_rjust,
                fn_addr!(molt_string_rjust),
                3,
                &[miss],
            ))
        }
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_expandtabs,
                fn_addr!(molt_string_expandtabs),
                2,
                &[miss],
            ))
        }
        "join" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_join,
            fn_addr!(molt_string_join),
            2,
        )),
        "translate" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_translate,
            fn_addr!(molt_string_translate),
            2,
        )),
        "maketrans" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.str_maketrans,
                fn_addr!(molt_string_maketrans),
                3,
                &[none, none],
            ))
        }
        "encode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_encode,
            fn_addr!(molt_string_encode),
            3,
        )),
        _ => None,
    }
}

pub(crate) fn bytes_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_count,
                fn_addr!(molt_bytes_count_method),
                4,
                &[none, none],
            ))
        }
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_find,
                fn_addr!(molt_bytes_find_method),
                4,
                &[none, none],
            ))
        }
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_index,
                fn_addr!(molt_bytes_index_method),
                4,
                &[none, none],
            ))
        }
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_rfind,
                fn_addr!(molt_bytes_rfind_method),
                4,
                &[none, none],
            ))
        }
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_rindex,
                fn_addr!(molt_bytes_rindex_method),
                4,
                &[none, none],
            ))
        }
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_split,
                fn_addr!(molt_bytes_split_max),
                3,
                &[neg_one],
            ))
        }
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_rsplit,
                fn_addr!(molt_bytes_rsplit_max),
                3,
                &[neg_one],
            ))
        }
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_strip,
                fn_addr!(molt_bytes_strip),
                2,
                &[none],
            ))
        }
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_lstrip,
                fn_addr!(molt_bytes_lstrip),
                2,
                &[none],
            ))
        }
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_rstrip,
                fn_addr!(molt_bytes_rstrip),
                2,
                &[none],
            ))
        }
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_startswith,
                fn_addr!(molt_bytes_startswith_method),
                4,
                &[none, none],
            ))
        }
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_endswith,
                fn_addr!(molt_bytes_endswith_method),
                4,
                &[none, none],
            ))
        }
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_splitlines,
                fn_addr!(molt_bytes_splitlines),
                2,
                &[none],
            ))
        }
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_partition,
            fn_addr!(molt_bytes_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_rpartition,
            fn_addr!(molt_bytes_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_replace,
                fn_addr!(molt_bytes_replace),
                4,
                &[neg_one],
            ))
        }
        "removeprefix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_removeprefix,
            fn_addr!(molt_bytes_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_removesuffix,
            fn_addr!(molt_bytes_removesuffix),
            2,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_join,
            fn_addr!(molt_bytes_join),
            2,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_capitalize,
            fn_addr!(molt_bytes_capitalize),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_upper,
            fn_addr!(molt_bytes_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_lower,
            fn_addr!(molt_bytes_lower),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_swapcase,
            fn_addr!(molt_bytes_swapcase),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_title,
            fn_addr!(molt_bytes_title),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isalpha,
            fn_addr!(molt_bytes_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isalnum,
            fn_addr!(molt_bytes_isalnum),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isdigit,
            fn_addr!(molt_bytes_isdigit),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isspace,
            fn_addr!(molt_bytes_isspace),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_islower,
            fn_addr!(molt_bytes_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isupper,
            fn_addr!(molt_bytes_isupper),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_istitle,
            fn_addr!(molt_bytes_istitle),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_isascii,
            fn_addr!(molt_bytes_isascii),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_hex,
            fn_addr!(molt_bytes_hex),
            3,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_zfill,
            fn_addr!(molt_bytes_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_center,
                fn_addr!(molt_bytes_center),
                3,
                &[miss],
            ))
        }
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_ljust,
                fn_addr!(molt_bytes_ljust),
                3,
                &[miss],
            ))
        }
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_rjust,
                fn_addr!(molt_bytes_rjust),
                3,
                &[miss],
            ))
        }
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_expandtabs,
                fn_addr!(molt_bytes_expandtabs),
                2,
                &[miss],
            ))
        }
        "translate" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytes_translate,
                fn_addr!(molt_bytes_translate),
                3,
                &[miss],
            ))
        }
        "maketrans" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_maketrans,
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        "decode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_decode,
            fn_addr!(molt_bytes_decode),
            3,
        )),
        _ => None,
    }
}

pub(crate) fn bytearray_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "extend" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_extend,
            fn_addr!(molt_bytearray_extend),
            2,
        )),
        "append" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_append,
            fn_addr!(molt_bytearray_append),
            2,
        )),
        "insert" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_insert,
            fn_addr!(molt_bytearray_insert),
            3,
        )),
        "pop" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_pop,
                fn_addr!(molt_bytearray_pop),
                2,
                &[none],
            ))
        }
        "remove" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_remove,
            fn_addr!(molt_bytearray_remove),
            2,
        )),
        "reverse" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_reverse,
            fn_addr!(molt_bytearray_reverse),
            1,
        )),
        "resize" if runtime_python_at_least(_py, 3, 14) => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_resize,
            fn_addr!(molt_bytearray_resize),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_copy,
            fn_addr!(molt_bytearray_copy),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_hex,
            fn_addr!(molt_bytearray_hex),
            3,
        )),
        "translate" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_translate,
                fn_addr!(molt_bytearray_translate),
                3,
                &[miss],
            ))
        }
        "maketrans" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_maketrans,
            fn_addr!(molt_bytes_maketrans),
            2,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_clear,
            fn_addr!(molt_bytearray_clear),
            1,
        )),
        "count" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_count,
                fn_addr!(molt_bytearray_count_method),
                4,
                &[none, none],
            ))
        }
        "find" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_find,
                fn_addr!(molt_bytearray_find_method),
                4,
                &[none, none],
            ))
        }
        "index" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_index,
                fn_addr!(molt_bytearray_index_method),
                4,
                &[none, none],
            ))
        }
        "rfind" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_rfind,
                fn_addr!(molt_bytearray_rfind_method),
                4,
                &[none, none],
            ))
        }
        "rindex" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_rindex,
                fn_addr!(molt_bytearray_rindex_method),
                4,
                &[none, none],
            ))
        }
        "split" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_split,
                fn_addr!(molt_bytearray_split_max),
                3,
                &[neg_one],
            ))
        }
        "rsplit" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_rsplit,
                fn_addr!(molt_bytearray_rsplit_max),
                3,
                &[neg_one],
            ))
        }
        "strip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_strip,
                fn_addr!(molt_bytearray_strip),
                2,
                &[none],
            ))
        }
        "lstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_lstrip,
                fn_addr!(molt_bytearray_lstrip),
                2,
                &[none],
            ))
        }
        "rstrip" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_rstrip,
                fn_addr!(molt_bytearray_rstrip),
                2,
                &[none],
            ))
        }
        "startswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_startswith,
                fn_addr!(molt_bytearray_startswith_method),
                4,
                &[none, none],
            ))
        }
        "endswith" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_endswith,
                fn_addr!(molt_bytearray_endswith_method),
                4,
                &[none, none],
            ))
        }
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        "splitlines" => {
            let none = MoltObject::none().bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_splitlines,
                fn_addr!(molt_bytearray_splitlines),
                2,
                &[none],
            ))
        }
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_partition,
            fn_addr!(molt_bytearray_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_rpartition,
            fn_addr!(molt_bytearray_rpartition),
            2,
        )),
        "replace" => {
            let neg_one = MoltObject::from_int(-1).bits();
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_replace,
                fn_addr!(molt_bytearray_replace),
                4,
                &[neg_one],
            ))
        }
        "removeprefix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_removeprefix,
            fn_addr!(molt_bytearray_removeprefix),
            2,
        )),
        "removesuffix" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_removesuffix,
            fn_addr!(molt_bytearray_removesuffix),
            2,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_join,
            fn_addr!(molt_bytearray_join),
            2,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_capitalize,
            fn_addr!(molt_bytearray_capitalize),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_upper,
            fn_addr!(molt_bytearray_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_lower,
            fn_addr!(molt_bytearray_lower),
            1,
        )),
        "swapcase" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_swapcase,
            fn_addr!(molt_bytearray_swapcase),
            1,
        )),
        "title" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_title,
            fn_addr!(molt_bytearray_title),
            1,
        )),
        "isalpha" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isalpha,
            fn_addr!(molt_bytearray_isalpha),
            1,
        )),
        "isalnum" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isalnum,
            fn_addr!(molt_bytearray_isalnum),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isdigit,
            fn_addr!(molt_bytearray_isdigit),
            1,
        )),
        "isspace" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isspace,
            fn_addr!(molt_bytearray_isspace),
            1,
        )),
        "islower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_islower,
            fn_addr!(molt_bytearray_islower),
            1,
        )),
        "isupper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isupper,
            fn_addr!(molt_bytearray_isupper),
            1,
        )),
        "istitle" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_istitle,
            fn_addr!(molt_bytearray_istitle),
            1,
        )),
        "isascii" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_isascii,
            fn_addr!(molt_bytearray_isascii),
            1,
        )),
        "zfill" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_zfill,
            fn_addr!(molt_bytearray_zfill),
            2,
        )),
        "center" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_center,
                fn_addr!(molt_bytearray_center),
                3,
                &[miss],
            ))
        }
        "ljust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_ljust,
                fn_addr!(molt_bytearray_ljust),
                3,
                &[miss],
            ))
        }
        "rjust" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_rjust,
                fn_addr!(molt_bytearray_rjust),
                3,
                &[miss],
            ))
        }
        "expandtabs" => {
            let miss = missing_bits(_py);
            Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &runtime_state(_py).method_cache.bytearray_expandtabs,
                fn_addr!(molt_bytearray_expandtabs),
                2,
                &[miss],
            ))
        }
        "decode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_decode,
            fn_addr!(molt_bytearray_decode),
            3,
        )),
        _ => None,
    }
}
