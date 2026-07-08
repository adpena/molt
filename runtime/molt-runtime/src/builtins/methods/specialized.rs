use super::common::builtin_func_bits;
use crate::PyToken;
use crate::*;

pub(crate) fn property_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "getter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_getter,
            fn_addr!(molt_property_getter),
            2,
        )),
        "setter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_setter,
            fn_addr!(molt_property_setter),
            2,
        )),
        "deleter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_deleter,
            fn_addr!(molt_property_deleter),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn generator_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_next,
            fn_addr!(molt_generator_next_method),
            1,
        )),
        "send" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_send,
            fn_addr!(molt_generator_send_method),
            2,
        )),
        "throw" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_throw,
            fn_addr!(molt_generator_throw_method),
            2,
        )),
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_close,
            fn_addr!(molt_generator_close_method),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn coroutine_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.coroutine_close,
            fn_addr!(molt_coroutine_close_method),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn asyncgen_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__aiter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_aiter,
            fn_addr!(molt_asyncgen_aiter),
            1,
        )),
        "__anext__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_anext,
            fn_addr!(molt_asyncgen_anext),
            1,
        )),
        "asend" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_asend,
            fn_addr!(molt_asyncgen_asend),
            2,
        )),
        "athrow" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_athrow,
            fn_addr!(molt_asyncgen_athrow),
            2,
        )),
        "aclose" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_aclose,
            fn_addr!(molt_asyncgen_aclose),
            1,
        )),
        _ => None,
    }
}
