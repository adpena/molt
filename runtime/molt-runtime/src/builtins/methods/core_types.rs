use super::common::{
    builtin_classmethod_bits, builtin_func_bits, builtin_func_bits_with_bind_kind,
    runtime_python_at_least,
};
use crate::PyToken;
use crate::*;

pub(crate) fn type_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_getattribute,
            fn_addr!(molt_type_getattribute),
            2,
        )),
        "__call__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_call,
            fn_addr!(molt_type_call),
            1,
        )),
        "__new__" => Some(builtin_func_bits_with_bind_kind(
            _py,
            &runtime_state(_py).method_cache.type_new,
            fn_addr!(molt_type_new),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        "__init__" => Some(builtin_func_bits_with_bind_kind(
            _py,
            &runtime_state(_py).method_cache.type_init,
            fn_addr!(molt_type_init),
            5,
            BIND_KIND_TYPE_NEW_INIT,
        )),
        "__prepare__" => Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.type_prepare,
            fn_addr!(molt_type_prepare),
            3,
        )),
        "__instancecheck__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_instancecheck,
            fn_addr!(molt_type_instancecheck),
            2,
        )),
        "__subclasscheck__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_subclasscheck,
            fn_addr!(molt_type_subclasscheck),
            2,
        )),
        "mro" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_mro,
            fn_addr!(molt_type_mro),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn object_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__dir__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_dir,
            fn_addr!(molt_object_dir_method),
            1,
        )),
        "__format__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_format,
            fn_addr!(molt_object_format_method),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_hash,
            fn_addr!(molt_object_hash),
            1,
        )),
        "__getstate__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_getstate,
            fn_addr!(molt_object_getstate),
            1,
        )),
        "__lt__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_lt,
            fn_addr!(molt_object_lt_method),
            2,
        )),
        "__le__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_le,
            fn_addr!(molt_object_le_method),
            2,
        )),
        "__gt__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_gt,
            fn_addr!(molt_object_gt_method),
            2,
        )),
        "__ge__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_ge,
            fn_addr!(molt_object_ge_method),
            2,
        )),
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_getattribute,
            fn_addr!(molt_object_getattribute),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_new,
            fn_addr!(molt_object_new_bound),
            1,
        )),
        "__init__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_init,
            fn_addr!(molt_object_init),
            1,
        )),
        "__init_subclass__" => {
            if matches!(
                std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                Some("1")
            ) {
                let slot = &runtime_state(_py).method_cache.object_init_subclass;
                let existing = slot.load(std::sync::atomic::Ordering::Acquire);
                eprintln!(
                    "molt object.__init_subclass__ slot_ptr={:p} existing_bits=0x{:x} fn_obj=0x{:x} fn_type_init=0x{:x}",
                    slot,
                    existing,
                    fn_addr!(molt_object_init_subclass),
                    fn_addr!(molt_type_init),
                );
            }
            let bits = builtin_func_bits(
                _py,
                &runtime_state(_py).method_cache.object_init_subclass,
                fn_addr!(molt_object_init_subclass),
                1,
            );
            if matches!(
                std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                Some("1")
            ) {
                if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                    if unsafe { object_type_id(ptr) } == TYPE_ID_FUNCTION {
                        unsafe {
                            eprintln!(
                                "molt object.__init_subclass__ func_bits=0x{:x} stored_fn_ptr=0x{:x} stored_call_target=0x{:x} mapped_call_target=0x{:x} stored_arity={}",
                                bits,
                                function_fn_ptr(ptr),
                                crate::object::layout::function_call_target_ptr(ptr) as usize,
                                crate::builtins::functions::runtime_callable_target_ptr(
                                    function_fn_ptr(ptr)
                                )
                                .unwrap_or(std::ptr::null())
                                    as usize,
                                function_arity(ptr),
                            );
                        }
                    } else {
                        eprintln!(
                            "molt object.__init_subclass__ func_bits=0x{:x} type_id={}",
                            bits,
                            unsafe { object_type_id(ptr) },
                        );
                    }
                } else {
                    eprintln!(
                        "molt object.__init_subclass__ func_bits=0x{:x} (immediate)",
                        bits
                    );
                }
            }
            Some(bits)
        }
        "__setattr__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_setattr,
            fn_addr!(molt_object_setattr),
            3,
        )),
        "__delattr__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_delattr,
            fn_addr!(molt_object_delattr),
            2,
        )),
        "__eq__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_eq,
            fn_addr!(molt_object_eq),
            2,
        )),
        "__ne__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_ne,
            fn_addr!(molt_object_ne),
            2,
        )),
        "__repr__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_repr,
            fn_addr!(molt_repr_from_obj),
            1,
        )),
        "__str__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_str,
            fn_addr!(molt_repr_from_obj),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn memoryview_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "_from_flags" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_from_flags,
            fn_addr!(molt_memoryview_from_flags),
            2,
        )),
        "count" if runtime_python_at_least(_py, 3, 14) => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_count,
            fn_addr!(molt_memoryview_count),
            2,
        )),
        "index" if runtime_python_at_least(_py, 3, 14) => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_index,
            fn_addr!(molt_memoryview_index),
            2,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_hex,
            fn_addr!(molt_memoryview_hex),
            3,
        )),
        "release" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_release,
            fn_addr!(molt_memoryview_release),
            1,
        )),
        "toreadonly" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_toreadonly,
            fn_addr!(molt_memoryview_toreadonly),
            1,
        )),
        "tobytes" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_tobytes,
            fn_addr!(molt_memoryview_tobytes),
            1,
        )),
        "tolist" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_tolist,
            fn_addr!(molt_memoryview_tolist),
            1,
        )),
        "cast" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_cast,
            fn_addr!(molt_memoryview_cast),
            4,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn range_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.range_count,
            fn_addr!(molt_range_count),
            2,
        )),
        "index" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.range_index,
            fn_addr!(molt_range_index),
            2,
        )),
        _ => None,
    }
}
