//! FFI bridge for `molt-runtime-itertools`.
//!
//! Exports C API functions that the itertools crate needs but that are not
//! already available as `#[no_mangle]` symbols.

use crate::*;
use molt_runtime_core::RuntimeVtable;
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// Class system helpers for itertools class setup
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_alloc_instance_for_class(class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe { alloc_instance_for_class(_py, class_ptr) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_call_callable1(call_bits: u64, arg0_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe { call_callable1(_py, call_bits, arg0_bits) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_call_callable2_bridge(
    call_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe { call_callable2(_py, call_bits, arg0_bits, arg1_bits) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_tuple_from_iter(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match unsafe { tuple_from_iter_bits(_py, iter_bits) } {
            Some(bits) => bits,
            None => 0, // signal failure (not None — caller checks for 0)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_alloc_class(
    name_ptr: *const u8,
    name_len: usize,
    layout_size: i64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len)) };
        let name_str_ptr = alloc_string(_py, name.as_bytes());
        if name_str_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_str_ptr).bits();
        let class_ptr = alloc_class_obj(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        if class_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                object_set_class_bits(_py, ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
        let _ = molt_class_set_base(class_bits, builtins.object);
        // Set __molt_layout_size__ in class dict
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let layout_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
        }
        class_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_class_set_iter_next(
    class_bits: u64,
    iter_fn_bits: u64,
    next_fn_bits: u64,
) {
    crate::with_gil_entry!(_py, {
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let iter_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.iter_name,
                b"__iter__",
            );
            unsafe { dict_set_in_place(_py, dict_ptr, iter_name, iter_fn_bits) };
            let next_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.next_name,
                b"__next__",
            );
            unsafe { dict_set_in_place(_py, dict_ptr, next_name, next_fn_bits) };
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_alloc_function(fn_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let builtins = builtin_classes(_py);
            let old_bits = object_class_bits(ptr);
            if old_bits != builtins.builtin_function_or_method {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, ptr, builtins.builtin_function_or_method);
                inc_ref_bits(_py, builtins.builtin_function_or_method);
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_alloc_kwd_mark() -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(_py, total, TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_object_class_bits(ptr: *mut u8) -> u64 {
    unsafe { object_class_bits(ptr) }
}

// ---------------------------------------------------------------------------
// RuntimeVtable — re-use the existing serial bridge vtable.
//
// The itertools crate fetches this via __molt_itertools_get_vtable() but the
// vtable contents are identical to the serial bridge.
// ---------------------------------------------------------------------------

/// Re-export the existing serial bridge vtable for itertools.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_itertools_get_vtable() -> *const RuntimeVtable {
    // Delegate to the serial vtable since it has all the same function pointers.
    __molt_serial_get_vtable()
}
