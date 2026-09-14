use super::common::{
    builtin_func_bits, builtin_func_bits_with_defaults_tuple, builtin_variadic_func_bits,
};
use crate::PyToken;
use crate::*;

pub(crate) fn staticmethod_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || {
        let cache = &runtime_state(_py).method_cache;
        match name {
            "__new__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.staticmethod_type_new,
                fn_addr!(molt_staticmethod_type_new),
            )),
            "__init__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.staticmethod_init,
                fn_addr!(molt_staticmethod_init),
            )),
            "__get__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.staticmethod_get,
                fn_addr!(molt_staticmethod_get),
            )),
            "__call__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.staticmethod_call,
                fn_addr!(molt_staticmethod_call),
            )),
            _ => None,
        }
    })
}

pub(crate) fn classmethod_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || {
        let cache = &runtime_state(_py).method_cache;
        match name {
            "__new__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.classmethod_type_new,
                fn_addr!(molt_classmethod_type_new),
            )),
            "__init__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.classmethod_init,
                fn_addr!(molt_classmethod_init),
            )),
            "__get__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.classmethod_get,
                fn_addr!(molt_classmethod_get),
            )),
            _ => None,
        }
    })
}

pub(crate) fn property_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || {
        let cache = &runtime_state(_py).method_cache;
        match name {
            "__new__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_type_new,
                fn_addr!(molt_property_type_new),
            )),
            "__init__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_init,
                fn_addr!(molt_property_init),
            )),
            "__get__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_get,
                fn_addr!(molt_property_get),
            )),
            "__set__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_set,
                fn_addr!(molt_property_set),
            )),
            "__delete__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_delete,
                fn_addr!(molt_property_delete),
            )),
            "__set_name__" => Some(builtin_variadic_func_bits(
                _py,
                &cache.property_set_name,
                fn_addr!(molt_property_set_name),
            )),
            "getter" => Some(builtin_func_bits(
                _py,
                &cache.property_getter,
                fn_addr!(molt_property_getter),
                2,
            )),
            "setter" => Some(builtin_func_bits(
                _py,
                &cache.property_setter,
                fn_addr!(molt_property_setter),
                2,
            )),
            "deleter" => Some(builtin_func_bits(
                _py,
                &cache.property_deleter,
                fn_addr!(molt_property_deleter),
                2,
            )),
            _ => None,
        }
    })
}

pub(crate) fn generator_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_iter,
            fn_addr!(crate::object::ops_iter::builtin_iter_slot),
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
    })
}

pub(crate) fn coroutine_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || match name {
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.coroutine_close,
            fn_addr!(molt_coroutine_close_method),
            1,
        )),
        _ => None,
    })
}

pub(crate) fn asyncgen_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || match name {
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
    })
}

pub(crate) fn weakref_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    super::method_dispatch(_py, || {
        let cache = &runtime_state(_py).method_cache;
        match name {
            "__init__" => Some(builtin_func_bits_with_defaults_tuple(
                _py,
                &cache.weakref_init,
                fn_addr!(molt_weakref_init),
                3,
                &[MoltObject::none().bits()],
            )),
            "__call__" => Some(builtin_func_bits(
                _py,
                &cache.weakref_call,
                fn_addr!(molt_weakref_call),
                1,
            )),
            "__eq__" => Some(builtin_func_bits(
                _py,
                &cache.weakref_eq,
                fn_addr!(molt_weakref_eq),
                2,
            )),
            "__ne__" => Some(builtin_func_bits(
                _py,
                &cache.weakref_ne,
                fn_addr!(molt_weakref_ne),
                2,
            )),
            "__repr__" => Some(builtin_func_bits(
                _py,
                &cache.weakref_repr,
                fn_addr!(molt_weakref_repr),
                1,
            )),
            "__hash__" => Some(builtin_func_bits(
                _py,
                &cache.weakref_hash,
                fn_addr!(molt_weakref_hash),
                1,
            )),
            _ => None,
        }
    })
}
