use molt_runtime_core::prelude::*;

use super::alloc_string;

pub fn index_i64_from_obj(_py: &CoreGilToken, obj_bits: u64, err: &str) -> i64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::numbers::index_i64_from_obj(py, obj_bits, err)
    })
}

pub fn intern_static_name(_py: &CoreGilToken, key: &[u8]) -> u64 {
    let ptr = alloc_string(_py, key);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

pub fn bridge_molt_add(a: u64, b: u64) -> u64 {
    crate::molt_add(a, b)
}

pub fn bridge_molt_eq(a: u64, b: u64) -> u64 {
    crate::molt_eq(a, b)
}

pub fn bridge_callargs_new(pos_cap: u64, kw_cap: u64) -> u64 {
    crate::molt_callargs_new(pos_cap, kw_cap)
}

pub fn bridge_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe { crate::molt_callargs_expand_star(builder_bits, iterable_bits) }
}

pub fn bridge_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::molt_call_bind(call_bits, builder_bits)
}

pub fn alloc_instance_for_class(_py: &CoreGilToken, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe { crate::alloc_instance_for_class(py, class_ptr) }
    })
}

pub fn alloc_itertools_class(_py: &CoreGilToken, name: &str, layout_size: i64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let name_str_ptr = crate::alloc_string(py, name.as_bytes());
        if name_str_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_str_ptr).bits();
        let class_ptr = crate::alloc_class_obj(py, name_bits);
        crate::dec_ref_bits(py, name_bits);
        if class_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = crate::builtin_classes(py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                crate::object_set_class_bits(py, ptr, builtins.type_obj);
                crate::inc_ref_bits(py, builtins.type_obj);
            }
        }
        let _ = crate::molt_class_set_base(class_bits, builtins.object);
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let layout_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            unsafe { crate::dict_set_in_place(py, dict_ptr, layout_name, layout_bits) };
        }
        class_bits
    })
}

pub fn class_set_iter_next(
    _py: &CoreGilToken,
    class_bits: u64,
    iter_fn_bits: u64,
    next_fn_bits: u64,
) {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let iter_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.iter_name,
                b"__iter__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, iter_name, iter_fn_bits) };
            let next_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.next_name,
                b"__next__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, next_name, next_fn_bits) };
        }
    });
}

pub fn class_set_new(_py: &CoreGilToken, class_bits: u64, new_fn_bits: u64) {
    crate::with_gil_entry_nopanic!(py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { crate::object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        {
            let new_name = crate::intern_static_name(
                py,
                &crate::runtime_state(py).interned.new_name,
                b"__new__",
            );
            unsafe { crate::dict_set_in_place(py, dict_ptr, new_name, new_fn_bits) };
        }
    });
}

pub fn alloc_function(_py: &CoreGilToken, fn_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let builtins = crate::builtin_classes(py);
            let old_bits = crate::object_class_bits(ptr);
            if old_bits != builtins.builtin_function_or_method {
                if old_bits != 0 {
                    crate::dec_ref_bits(py, old_bits);
                }
                crate::object_set_class_bits(py, ptr, builtins.builtin_function_or_method);
                crate::inc_ref_bits(py, builtins.builtin_function_or_method);
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub fn alloc_function_with_defaults(
    _py: &CoreGilToken,
    fn_ptr: u64,
    arity: u64,
    defaults: &[u64],
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            (*crate::header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_IMMORTAL;
            let defaults_tuple_ptr = crate::alloc_tuple(py, defaults);
            if !defaults_tuple_ptr.is_null() {
                let defaults_name = crate::intern_static_name(
                    py,
                    &crate::runtime_state(py).interned.defaults_name,
                    b"__defaults__",
                );
                let defaults_bits = MoltObject::from_ptr(defaults_tuple_ptr).bits();
                crate::function_set_attr_bits(py, ptr, defaults_name, defaults_bits);
            }
            let builtins = crate::builtin_classes(py);
            let old_bits = crate::object_class_bits(ptr);
            if old_bits != builtins.builtin_function_or_method {
                if old_bits != 0 {
                    crate::dec_ref_bits(py, old_bits);
                }
                crate::object_set_class_bits(py, ptr, builtins.builtin_function_or_method);
                crate::inc_ref_bits(py, builtins.builtin_function_or_method);
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub fn alloc_kwd_mark(_py: &CoreGilToken) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let total = std::mem::size_of::<crate::MoltHeader>();
        let ptr = crate::alloc_object(py, total, crate::TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
///
/// `ptr` must be a valid Molt runtime object pointer for the duration of this
/// call.
pub unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { crate::object_class_bits(ptr) }
}
