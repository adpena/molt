use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;

use crate::builtins::methods::not_implemented_bits;
use crate::builtins::numbers::index_i64_from_obj;
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_TUPLE, alloc_class_obj, alloc_function_obj, alloc_string,
    alloc_tuple, attr_name_bits_from_bytes, builtin_classes, call_callable2, class_dict_bits,
    dec_ref_bits, dict_get_in_place, dict_order, dict_set_in_place, dict_update_apply,
    dict_update_set_in_place, exception_pending, inc_ref_bits, init_atomic_bits,
    intern_static_name, is_truthy, molt_class_set_base, molt_getattr_builtin, molt_is_callable,
    molt_iter, molt_object_setattr, molt_repr_from_obj, obj_from_bits, object_class_bits,
    object_set_class_bits, object_type_id, raise_exception, raise_not_iterable, seq_vec_ref,
    string_obj_to_owned, to_i64,
};

static KWD_MARK_BITS: AtomicU64 = AtomicU64::new(0);

static PARTIAL_CLASS: AtomicU64 = AtomicU64::new(0);
static PARTIAL_CALL_FN: AtomicU64 = AtomicU64::new(0);
static PARTIAL_REPR_FN: AtomicU64 = AtomicU64::new(0);

static CMPKEY_CLASS: AtomicU64 = AtomicU64::new(0);
static CMPKEY_LT_FN: AtomicU64 = AtomicU64::new(0);
static CMPKEY_LE_FN: AtomicU64 = AtomicU64::new(0);
static CMPKEY_GT_FN: AtomicU64 = AtomicU64::new(0);
static CMPKEY_GE_FN: AtomicU64 = AtomicU64::new(0);
static CMPKEY_EQ_FN: AtomicU64 = AtomicU64::new(0);
static CMPKEY_NE_FN: AtomicU64 = AtomicU64::new(0);

static LRU_WRAPPER_CLASS: AtomicU64 = AtomicU64::new(0);
static LRU_FACTORY_CLASS: AtomicU64 = AtomicU64::new(0);
static CACHEINFO_CLASS: AtomicU64 = AtomicU64::new(0);

static LRU_CALL_FN: AtomicU64 = AtomicU64::new(0);
static LRU_CACHE_INFO_FN: AtomicU64 = AtomicU64::new(0);
static LRU_CACHE_CLEAR_FN: AtomicU64 = AtomicU64::new(0);
static LRU_CACHE_PARAMS_FN: AtomicU64 = AtomicU64::new(0);
static LRU_FACTORY_CALL_FN: AtomicU64 = AtomicU64::new(0);

static CACHEINFO_ITER_FN: AtomicU64 = AtomicU64::new(0);
static CACHEINFO_REPR_FN: AtomicU64 = AtomicU64::new(0);
static CACHEINFO_GETATTR_FN: AtomicU64 = AtomicU64::new(0);

fn builtin_func_bits(_py: &PyToken<'_>, slot: &AtomicU64, fn_ptr: u64, arity: u64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                let old_bits = object_class_bits(ptr);
                if old_bits != builtin_bits {
                    if old_bits != 0 {
                        dec_ref_bits(_py, old_bits);
                    }
                    object_set_class_bits(_py, ptr, builtin_bits);
                    inc_ref_bits(_py, builtin_bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn kwd_mark_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(_py, &KWD_MARK_BITS, || {
        let total = std::mem::size_of::<crate::MoltHeader>();
        let ptr = crate::alloc_object(_py, total, crate::TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_kwd_mark() -> u64 {
    crate::with_gil_entry!(_py, { kwd_mark_bits(_py) })
}

fn functools_class(_py: &PyToken<'_>, slot: &AtomicU64, name: &str, layout_size: i64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
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
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                let layout_name = intern_static_name(
                    _py,
                    &crate::runtime_state(_py).interned.molt_layout_size,
                    b"__molt_layout_size__",
                );
                let layout_bits = MoltObject::from_int(layout_size).bits();
                unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
            }
        }
        class_bits
    })
}

fn set_class_method(_py: &PyToken<'_>, class_bits: u64, name: &str, fn_bits: u64) {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return;
    };
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, fn_bits) };
    dec_ref_bits(_py, name_bits);
}

fn partial_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = functools_class(_py, &PARTIAL_CLASS, "partial", 32);
    let call_bits = builtin_func_bits(
        _py,
        &PARTIAL_CALL_FN,
        crate::molt_functools_partial_call as *const () as usize as u64,
        3,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &PARTIAL_REPR_FN,
        crate::molt_functools_partial_repr as *const () as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__call__", call_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    // mark __call__ as vararg/varkw
    if let Some(call_ptr) = obj_from_bits(call_bits).as_ptr() {
        let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
        if !dict_ptr.is_null() {
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            unsafe { crate::function_set_dict_bits(call_ptr, dict_bits) };
            let vararg_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_vararg,
                b"__molt_vararg__",
            );
            let varkw_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_varkw,
                b"__molt_varkw__",
            );
            let arg_names_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_arg_names,
                b"__molt_arg_names__",
            );
            let self_name_ptr = alloc_string(_py, b"self");
            if !self_name_ptr.is_null() {
                let self_name_bits = MoltObject::from_ptr(self_name_ptr).bits();
                let arg_names_ptr = alloc_tuple(_py, &[self_name_bits]);
                dec_ref_bits(_py, self_name_bits);
                if !arg_names_ptr.is_null() {
                    let arg_names_bits = MoltObject::from_ptr(arg_names_ptr).bits();
                    unsafe { dict_set_in_place(_py, dict_ptr, arg_names_name, arg_names_bits) };
                    dec_ref_bits(_py, arg_names_bits);
                }
            }
            unsafe {
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    vararg_name,
                    MoltObject::from_bool(true).bits(),
                );
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    varkw_name,
                    MoltObject::from_bool(true).bits(),
                );
            }
        }
    }
    class_bits
}

fn cmpkey_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = functools_class(_py, &CMPKEY_CLASS, "_CmpKey", 24);
    let lt_bits = builtin_func_bits(
        _py,
        &CMPKEY_LT_FN,
        crate::molt_functools_cmpkey_lt as *const () as usize as u64,
        2,
    );
    let le_bits = builtin_func_bits(
        _py,
        &CMPKEY_LE_FN,
        crate::molt_functools_cmpkey_le as *const () as usize as u64,
        2,
    );
    let gt_bits = builtin_func_bits(
        _py,
        &CMPKEY_GT_FN,
        crate::molt_functools_cmpkey_gt as *const () as usize as u64,
        2,
    );
    let ge_bits = builtin_func_bits(
        _py,
        &CMPKEY_GE_FN,
        crate::molt_functools_cmpkey_ge as *const () as usize as u64,
        2,
    );
    let eq_bits = builtin_func_bits(
        _py,
        &CMPKEY_EQ_FN,
        crate::molt_functools_cmpkey_eq as *const () as usize as u64,
        2,
    );
    let ne_bits = builtin_func_bits(
        _py,
        &CMPKEY_NE_FN,
        crate::molt_functools_cmpkey_ne as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__lt__", lt_bits);
    set_class_method(_py, class_bits, "__le__", le_bits);
    set_class_method(_py, class_bits, "__gt__", gt_bits);
    set_class_method(_py, class_bits, "__ge__", ge_bits);
    set_class_method(_py, class_bits, "__eq__", eq_bits);
    set_class_method(_py, class_bits, "__ne__", ne_bits);
    class_bits
}

fn lru_wrapper_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = functools_class(_py, &LRU_WRAPPER_CLASS, "_LruCacheWrapper", 64);
    let call_bits = builtin_func_bits(
        _py,
        &LRU_CALL_FN,
        crate::molt_functools_lru_call as *const () as usize as u64,
        3,
    );
    let info_bits = builtin_func_bits(
        _py,
        &LRU_CACHE_INFO_FN,
        crate::molt_functools_lru_cache_info as *const () as usize as u64,
        1,
    );
    let clear_bits = builtin_func_bits(
        _py,
        &LRU_CACHE_CLEAR_FN,
        crate::molt_functools_lru_cache_clear as *const () as usize as u64,
        1,
    );
    let params_bits = builtin_func_bits(
        _py,
        &LRU_CACHE_PARAMS_FN,
        crate::molt_functools_lru_cache_params as *const () as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__call__", call_bits);
    set_class_method(_py, class_bits, "cache_info", info_bits);
    set_class_method(_py, class_bits, "cache_clear", clear_bits);
    set_class_method(_py, class_bits, "cache_parameters", params_bits);
    // mark __call__ as vararg/varkw
    if let Some(call_ptr) = obj_from_bits(call_bits).as_ptr() {
        let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
        if !dict_ptr.is_null() {
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            unsafe { crate::function_set_dict_bits(call_ptr, dict_bits) };
            let vararg_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_vararg,
                b"__molt_vararg__",
            );
            let varkw_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_varkw,
                b"__molt_varkw__",
            );
            let arg_names_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_arg_names,
                b"__molt_arg_names__",
            );
            let self_name_ptr = alloc_string(_py, b"self");
            if !self_name_ptr.is_null() {
                let self_name_bits = MoltObject::from_ptr(self_name_ptr).bits();
                let arg_names_ptr = alloc_tuple(_py, &[self_name_bits]);
                dec_ref_bits(_py, self_name_bits);
                if !arg_names_ptr.is_null() {
                    let arg_names_bits = MoltObject::from_ptr(arg_names_ptr).bits();
                    unsafe { dict_set_in_place(_py, dict_ptr, arg_names_name, arg_names_bits) };
                    dec_ref_bits(_py, arg_names_bits);
                }
            }
            unsafe {
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    vararg_name,
                    MoltObject::from_bool(true).bits(),
                );
                dict_set_in_place(
                    _py,
                    dict_ptr,
                    varkw_name,
                    MoltObject::from_bool(true).bits(),
                );
            }
        }
    }
    class_bits
}

fn lru_factory_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = functools_class(_py, &LRU_FACTORY_CLASS, "_LruCacheFactory", 24);
    let call_bits = builtin_func_bits(
        _py,
        &LRU_FACTORY_CALL_FN,
        crate::molt_functools_lru_factory_call as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__call__", call_bits);
    class_bits
}

fn cacheinfo_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = functools_class(_py, &CACHEINFO_CLASS, "CacheInfo", 40);
    let iter_bits = builtin_func_bits(
        _py,
        &CACHEINFO_ITER_FN,
        crate::molt_functools_cacheinfo_iter as *const () as usize as u64,
        1,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &CACHEINFO_REPR_FN,
        crate::molt_functools_cacheinfo_repr as *const () as usize as u64,
        1,
    );
    let getattr_bits = builtin_func_bits(
        _py,
        &CACHEINFO_GETATTR_FN,
        crate::molt_functools_cacheinfo_getattr as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__iter__", iter_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    set_class_method(_py, class_bits, "__getattr__", getattr_bits);
    class_bits
}

unsafe fn partial_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn partial_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn partial_kwargs_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn partial_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn partial_set_args_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn partial_set_kwargs_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn cmpkey_obj_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn cmpkey_cmp_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn cmpkey_set_obj_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn cmpkey_set_cmp_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn lru_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn lru_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_typed_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_cache_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut *mut Vec<u64>) }
}
unsafe fn lru_hits(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn lru_misses(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn lru_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn lru_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_typed_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_cache_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn lru_set_order_ptr(ptr: *mut u8, order: *mut Vec<u64>) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut *mut Vec<u64>) = order;
    }
}
unsafe fn lru_set_hits(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}
unsafe fn lru_set_misses(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(6 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn lru_factory_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}
unsafe fn lru_factory_typed_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn lru_factory_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}
unsafe fn lru_factory_set_typed_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn cacheinfo_hits(ptr: *mut u8) -> i64 {
    unsafe { *(ptr as *const i64) }
}
unsafe fn cacheinfo_misses(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn cacheinfo_maxsize_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}
unsafe fn cacheinfo_currsize(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const i64) }
}
unsafe fn cacheinfo_set_hits(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr as *mut i64) = val;
    }
}
unsafe fn cacheinfo_set_misses(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}
unsafe fn cacheinfo_set_maxsize_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}
unsafe fn cacheinfo_set_currsize(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

fn extend_positional_from_call_arg(arg_bits: u64, out: &mut Vec<u64>) {
    let Some(arg_ptr) = obj_from_bits(arg_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(arg_ptr) == TYPE_ID_TUPLE {
            out.extend_from_slice(seq_vec_ref(arg_ptr));
            return;
        }
    }
    out.push(arg_bits);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial(func_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(func_bits)));
        if !callable {
            return raise_exception::<_>(_py, "TypeError", "partial() requires a callable");
        }
        let class_bits = partial_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            partial_set_func_bits(inst_ptr, func_bits);
            partial_set_args_bits(inst_ptr, args_bits);
            partial_set_kwargs_bits(inst_ptr, kwargs_bits);
        }
        inc_ref_bits(_py, func_bits);
        if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
            inc_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial_call(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { partial_func_bits(self_ptr) };
        let stored_args_bits = unsafe { partial_args_bits(self_ptr) };
        let stored_kwargs_bits = unsafe { partial_kwargs_bits(self_ptr) };
        let mut pos: Vec<u64> = Vec::new();
        extend_positional_from_call_arg(stored_args_bits, &mut pos);
        extend_positional_from_call_arg(args_bits, &mut pos);
        let merged_kwargs_bits =
            if stored_kwargs_bits != 0 && !obj_from_bits(stored_kwargs_bits).is_none() {
                let copy_bits = crate::dict_copy_method(stored_kwargs_bits) as u64;
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
                    let _ = unsafe {
                        dict_update_apply(_py, copy_bits, dict_update_set_in_place, kwargs_bits)
                    };
                    if exception_pending(_py) {
                        dec_ref_bits(_py, copy_bits);
                        return MoltObject::none().bits();
                    }
                }
                copy_bits
            } else if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
                inc_ref_bits(_py, kwargs_bits);
                kwargs_bits
            } else {
                MoltObject::none().bits()
            };
        let builder_bits = crate::molt_callargs_new(pos.len() as u64, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, merged_kwargs_bits);
            return MoltObject::none().bits();
        }
        for &arg_bits in pos.iter() {
            unsafe {
                let _ = crate::molt_callargs_push_pos(builder_bits, arg_bits);
            }
        }
        if merged_kwargs_bits != 0 && !obj_from_bits(merged_kwargs_bits).is_none() {
            if let Some(dict_ptr) = obj_from_bits(merged_kwargs_bits).as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let order = dict_order(dict_ptr);
                        let mut idx = 0;
                        while idx + 1 < order.len() {
                            let _ = crate::molt_callargs_push_kw(
                                builder_bits,
                                order[idx],
                                order[idx + 1],
                            );
                            if exception_pending(_py) {
                                dec_ref_bits(_py, merged_kwargs_bits);
                                return MoltObject::none().bits();
                            }
                            idx += 2;
                        }
                    }
                }
            }
            dec_ref_bits(_py, merged_kwargs_bits);
        }
        crate::molt_call_bind(func_bits, builder_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_partial_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { partial_func_bits(self_ptr) };
        let args_bits = unsafe { partial_args_bits(self_ptr) };
        let kwargs_bits = unsafe { partial_kwargs_bits(self_ptr) };
        let func_repr_bits = molt_repr_from_obj(func_bits);
        let func_repr = string_obj_to_owned(obj_from_bits(func_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, func_repr_bits);
        let args_repr_bits = molt_repr_from_obj(args_bits);
        let args_repr = string_obj_to_owned(obj_from_bits(args_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, args_repr_bits);
        let mut out = String::new();
        out.push_str("functools.partial(");
        out.push_str(&func_repr);
        out.push_str(", ");
        out.push_str(&args_repr);
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            let kw_repr_bits = molt_repr_from_obj(kwargs_bits);
            let kw_repr = string_obj_to_owned(obj_from_bits(kw_repr_bits)).unwrap_or_default();
            dec_ref_bits(_py, kw_repr_bits);
            out.push_str(", ");
            out.push_str(&kw_repr);
        }
        out.push(')');
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_reduce(
    func_bits: u64,
    iterable_bits: u64,
    initializer_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let mut value_bits = initializer_bits;
        if initializer_bits == kwd_mark_bits(_py) {
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "reduce() of empty sequence with no initial value",
                );
            }
            value_bits = val_bits;
            inc_ref_bits(_py, value_bits);
        } else {
            inc_ref_bits(_py, value_bits);
        }
        loop {
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                dec_ref_bits(_py, value_bits);
                return MoltObject::none().bits();
            };
            if done {
                dec_ref_bits(_py, iter_bits);
                return value_bits;
            }
            let next_bits = unsafe { call_callable2(_py, func_bits, value_bits, val_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, value_bits);
                return MoltObject::none().bits();
            }
            dec_ref_bits(_py, value_bits);
            value_bits = next_bits;
            inc_ref_bits(_py, value_bits);
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_update_wrapper(
    wrapper_bits: u64,
    wrapped_bits: u64,
    assigned_bits: u64,
    updated_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = crate::missing_bits(_py);
        let assigned_iter = molt_iter(assigned_bits);
        if !obj_from_bits(assigned_iter).is_none() {
            loop {
                let Some((val_bits, done)) = iter_next_pair(_py, assigned_iter) else {
                    break;
                };
                if done {
                    break;
                }
                let name = string_obj_to_owned(obj_from_bits(val_bits)).unwrap_or_default();
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    continue;
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = molt_getattr_builtin(wrapped_bits, name_bits, missing);
                if exception_pending(_py) {
                    crate::molt_exception_clear();
                } else if value_bits != missing {
                    let _ = molt_object_setattr(wrapper_bits, name_bits, value_bits);
                    if exception_pending(_py) {
                        crate::molt_exception_clear();
                    }
                }
                dec_ref_bits(_py, name_bits);
            }
        }
        let updated_iter = molt_iter(updated_bits);
        if !obj_from_bits(updated_iter).is_none() {
            loop {
                let Some((val_bits, done)) = iter_next_pair(_py, updated_iter) else {
                    break;
                };
                if done {
                    break;
                }
                let name = string_obj_to_owned(obj_from_bits(val_bits)).unwrap_or_default();
                if name == "__dict__" {
                    let target_bits = molt_getattr_builtin(wrapper_bits, val_bits, missing);
                    let source_bits = molt_getattr_builtin(wrapped_bits, val_bits, missing);
                    if exception_pending(_py) {
                        crate::molt_exception_clear();
                        continue;
                    }
                    if target_bits != missing && source_bits != missing {
                        if let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() {
                            unsafe {
                                if object_type_id(target_ptr) == TYPE_ID_DICT {
                                    let _order = dict_order(target_ptr);
                                    let source_ptr = obj_from_bits(source_bits).as_ptr();
                                    if let Some(source_ptr) = source_ptr {
                                        if object_type_id(source_ptr) == TYPE_ID_DICT {
                                            let src_order = dict_order(source_ptr);
                                            let mut idx = 0;
                                            while idx + 1 < src_order.len() {
                                                let key_bits = src_order[idx];
                                                let val_bits = src_order[idx + 1];
                                                if let Some(key_str) =
                                                    string_obj_to_owned(obj_from_bits(key_bits))
                                                {
                                                    if key_str.starts_with("__molt_") {
                                                        idx += 2;
                                                        continue;
                                                    }
                                                }
                                                dict_set_in_place(
                                                    _py, target_ptr, key_bits, val_bits,
                                                );
                                                idx += 2;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let wrapped_name = intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.wrapped_name,
            b"__wrapped__",
        );
        let _ = molt_object_setattr(wrapper_bits, wrapped_name, wrapped_bits);
        if exception_pending(_py) {
            crate::molt_exception_clear();
        }
        inc_ref_bits(_py, wrapper_bits);
        wrapper_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_wraps(
    wrapped_bits: u64,
    assigned_bits: u64,
    updated_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let tuple_ptr = alloc_tuple(_py, &[wrapped_bits, assigned_bits, updated_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let closure_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let func_ptr = alloc_function_obj(
            _py,
            crate::molt_functools_wraps_call as *const () as usize as u64,
            1,
        );
        if func_ptr.is_null() {
            dec_ref_bits(_py, closure_bits);
            return MoltObject::none().bits();
        }
        unsafe { crate::function_set_closure_bits(_py, func_ptr, closure_bits) };
        dec_ref_bits(_py, closure_bits);
        MoltObject::from_ptr(func_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_wraps_call(closure_bits: u64, wrapper_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(closure_ptr);
            if elems.len() < 3 {
                return MoltObject::none().bits();
            }
            molt_functools_update_wrapper(wrapper_bits, elems[0], elems[1], elems[2])
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmp_to_key(cmp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_bits = cmpkey_class(_py);
        let tuple_ptr = alloc_tuple(_py, &[cmp_bits, class_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let closure_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let func_ptr = alloc_function_obj(
            _py,
            crate::molt_functools_cmp_key_func as *const () as usize as u64,
            1,
        );
        if func_ptr.is_null() {
            dec_ref_bits(_py, closure_bits);
            return MoltObject::none().bits();
        }
        unsafe { crate::function_set_closure_bits(_py, func_ptr, closure_bits) };
        dec_ref_bits(_py, closure_bits);
        MoltObject::from_ptr(func_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmp_key_func(closure_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(closure_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let cmp_bits = elems[0];
            let class_bits = elems[1];
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let inst_bits = crate::alloc_instance_for_class(_py, class_ptr);
            if obj_from_bits(inst_bits).is_none() {
                return MoltObject::none().bits();
            }
            let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
            cmpkey_set_obj_bits(inst_ptr, obj_bits);
            cmpkey_set_cmp_bits(inst_ptr, cmp_bits);
            inc_ref_bits(_py, obj_bits);
            inc_ref_bits(_py, cmp_bits);
            inst_bits
        }
    })
}

fn cmpkey_compare(_py: &PyToken<'_>, self_bits: u64, other_bits: u64) -> Option<i64> {
    let self_ptr = obj_from_bits(self_bits).as_ptr()?;
    let other_ptr = obj_from_bits(other_bits).as_ptr()?;
    let self_class = unsafe { object_class_bits(self_ptr) };
    let other_class = unsafe { object_class_bits(other_ptr) };
    if self_class == 0 || other_class == 0 || self_class != other_class {
        return None;
    }
    let obj_bits = unsafe { cmpkey_obj_bits(self_ptr) };
    let cmp_bits = unsafe { cmpkey_cmp_bits(self_ptr) };
    let other_obj_bits = unsafe { cmpkey_obj_bits(other_ptr) };
    let res_bits = unsafe { call_callable2(_py, cmp_bits, obj_bits, other_obj_bits) };
    if exception_pending(_py) {
        return Some(0);
    }
    let val = index_i64_from_obj(_py, res_bits, "cmp_to_key comparison must be int");
    if exception_pending(_py) {
        return Some(0);
    }
    Some(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_lt(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val < 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_le(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val <= 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_gt(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val > 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_ge(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return not_implemented_bits(_py);
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val >= 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val == 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cmpkey_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = cmpkey_compare(_py, self_bits, other_bits) else {
            return MoltObject::from_bool(true).bits();
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(val != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_total_ordering(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_ptr = obj_from_bits(cls_bits).as_ptr();
        let Some(cls_ptr) = cls_ptr else {
            return raise_exception::<_>(_py, "TypeError", "total_ordering expects a class");
        };
        unsafe {
            if object_type_id(cls_ptr) != crate::TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "total_ordering expects a class");
            }
        }
        let dict_bits = unsafe { class_dict_bits(cls_ptr) };
        let dict_ptr = obj_from_bits(dict_bits).as_ptr();
        let Some(dict_ptr) = dict_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
        }
        let lt_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.lt_name, b"__lt__");
        let le_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.le_name, b"__le__");
        let gt_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.gt_name, b"__gt__");
        let ge_name =
            intern_static_name(_py, &crate::runtime_state(_py).interned.ge_name, b"__ge__");
        let root = if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_some() } {
            "lt"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_some() } {
            "le"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_some() } {
            "gt"
        } else if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_some() } {
            "ge"
        } else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "total_ordering requires at least one ordering operation: < <= > >=",
            );
        };
        let mut missing: Vec<(&'static str, i64, i64, i64)> = Vec::new();
        // op_code: 0=lt,1=le,2=gt,3=ge; swap, negate
        match root {
            "lt" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 0, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 0, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 0, 0, 1));
                }
            }
            "le" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 1, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 1, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 1, 0, 1));
                }
            }
            "gt" => {
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 2, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, ge_name).is_none() } {
                    missing.push(("__ge__", 2, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 2, 0, 1));
                }
            }
            _ => {
                if unsafe { dict_get_in_place(_py, dict_ptr, le_name).is_none() } {
                    missing.push(("__le__", 3, 1, 0));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, gt_name).is_none() } {
                    missing.push(("__gt__", 3, 1, 1));
                }
                if unsafe { dict_get_in_place(_py, dict_ptr, lt_name).is_none() } {
                    missing.push(("__lt__", 3, 0, 1));
                }
            }
        }
        for (name, op_code, swap, negate) in missing {
            let closure_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(op_code).bits(),
                    MoltObject::from_int(swap).bits(),
                    MoltObject::from_int(negate).bits(),
                ],
            );
            if closure_ptr.is_null() {
                continue;
            }
            let closure_bits = MoltObject::from_ptr(closure_ptr).bits();
            let func_ptr = alloc_function_obj(
                _py,
                crate::molt_functools_total_ordering_op as *const () as usize as u64,
                2,
            );
            if func_ptr.is_null() {
                dec_ref_bits(_py, closure_bits);
                continue;
            }
            unsafe { crate::function_set_closure_bits(_py, func_ptr, closure_bits) };
            dec_ref_bits(_py, closure_bits);
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
                dec_ref_bits(_py, func_bits);
                continue;
            };
            unsafe { dict_set_in_place(_py, dict_ptr, name_bits, func_bits) };
            dec_ref_bits(_py, name_bits);
        }
        inc_ref_bits(_py, cls_bits);
        cls_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_total_ordering_op(
    closure_bits: u64,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let closure_ptr = obj_from_bits(closure_bits).as_ptr();
        let Some(closure_ptr) = closure_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(closure_ptr);
            if elems.len() < 3 {
                return MoltObject::none().bits();
            }
            let op_code = to_i64(obj_from_bits(elems[0])).unwrap_or(0);
            let swap = to_i64(obj_from_bits(elems[1])).unwrap_or(0) != 0;
            let negate = to_i64(obj_from_bits(elems[2])).unwrap_or(0) != 0;
            let (lhs, rhs) = if swap {
                (other_bits, self_bits)
            } else {
                (self_bits, other_bits)
            };
            let res_bits = match op_code {
                0 => crate::molt_lt(lhs, rhs),
                1 => crate::molt_le(lhs, rhs),
                2 => crate::molt_gt(lhs, rhs),
                _ => crate::molt_ge(lhs, rhs),
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let truth = is_truthy(_py, obj_from_bits(res_bits));
            let out = if negate { !truth } else { truth };
            MoltObject::from_bool(out).bits()
        }
    })
}

fn iter_next_pair(_py: &PyToken<'_>, iter_bits: u64) -> Option<(u64, bool)> {
    let pair_bits = crate::molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let pair_ptr = pair_obj.as_ptr()?;
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Some((val_bits, done))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache(maxsize_bits: u64, typed_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let typed = is_truthy(_py, obj_from_bits(typed_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(maxsize_bits)));
        if callable {
            let wrapper_bits = build_lru_wrapper(
                _py,
                maxsize_bits,
                MoltObject::from_int(128).bits(),
                MoltObject::from_bool(typed).bits(),
            );
            if obj_from_bits(wrapper_bits).is_none() {
                return MoltObject::none().bits();
            }
            return molt_functools_update_wrapper(
                wrapper_bits,
                maxsize_bits,
                default_wrapper_assignments(_py),
                default_wrapper_updates(_py),
            );
        }
        let maxsize_bits = if obj_from_bits(maxsize_bits).is_none() {
            maxsize_bits
        } else {
            let mut maxsize = index_i64_from_obj(_py, maxsize_bits, "maxsize must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if maxsize < 0 {
                maxsize = 0;
            }
            MoltObject::from_int(maxsize).bits()
        };
        let class_bits = lru_factory_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            lru_factory_set_maxsize_bits(inst_ptr, maxsize_bits);
            lru_factory_set_typed_bits(inst_ptr, MoltObject::from_bool(typed).bits());
        }
        inc_ref_bits(_py, maxsize_bits);
        inst_bits
    })
}

fn build_lru_wrapper(_py: &PyToken<'_>, func_bits: u64, maxsize_bits: u64, typed_bits: u64) -> u64 {
    let class_bits = lru_wrapper_class(_py);
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
    if obj_from_bits(inst_bits).is_none() {
        return MoltObject::none().bits();
    }
    let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
    let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        dec_ref_bits(_py, inst_bits);
        return MoltObject::none().bits();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let order = Box::new(Vec::<u64>::new());
    let order_ptr = Box::into_raw(order);
    unsafe {
        lru_set_func_bits(inst_ptr, func_bits);
        lru_set_maxsize_bits(inst_ptr, maxsize_bits);
        lru_set_typed_bits(inst_ptr, typed_bits);
        lru_set_cache_bits(inst_ptr, dict_bits);
        lru_set_order_ptr(inst_ptr, order_ptr);
        lru_set_hits(inst_ptr, 0);
        lru_set_misses(inst_ptr, 0);
    }
    inc_ref_bits(_py, func_bits);
    inc_ref_bits(_py, maxsize_bits);
    inc_ref_bits(_py, typed_bits);
    inst_bits
}

fn default_wrapper_assignments(_py: &PyToken<'_>) -> u64 {
    // tuple of __module__, __name__, __qualname__, __doc__, __annotations__
    let names = [
        "__module__",
        "__name__",
        "__qualname__",
        "__doc__",
        "__annotations__",
    ];
    let mut elems: Vec<u64> = Vec::with_capacity(names.len());
    for name in names.iter() {
        let ptr = alloc_string(_py, name.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        elems.push(MoltObject::from_ptr(ptr).bits());
    }
    let tuple_ptr = alloc_tuple(_py, elems.as_slice());
    for bits in elems.iter() {
        dec_ref_bits(_py, *bits);
    }
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn default_wrapper_updates(_py: &PyToken<'_>) -> u64 {
    let ptr = alloc_string(_py, b"__dict__");
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[bits]);
    dec_ref_bits(_py, bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn make_lru_key(_py: &PyToken<'_>, args_bits: u64, kwargs_bits: u64, typed: bool) -> u64 {
    let mut parts: Vec<u64> = Vec::new();
    extend_positional_from_call_arg(args_bits, &mut parts);
    if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
        parts.push(kwd_mark_bits(_py));
        if let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let order = dict_order(dict_ptr);
                    let mut idx = 0;
                    while idx + 1 < order.len() {
                        let pair_ptr = alloc_tuple(_py, &[order[idx], order[idx + 1]]);
                        if pair_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        parts.push(MoltObject::from_ptr(pair_ptr).bits());
                        idx += 2;
                    }
                }
            }
        }
    }
    if typed {
        let mut typed_args: Vec<u64> = Vec::new();
        extend_positional_from_call_arg(args_bits, &mut typed_args);
        for val_bits in typed_args {
            let type_bits = crate::type_of_bits(_py, val_bits);
            parts.push(type_bits);
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            if let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let order = dict_order(dict_ptr);
                        let mut idx = 0;
                        while idx + 1 < order.len() {
                            let val_bits = order[idx + 1];
                            let type_bits = crate::type_of_bits(_py, val_bits);
                            parts.push(type_bits);
                            idx += 2;
                        }
                    }
                }
            }
        }
    }
    let tuple_ptr = alloc_tuple(_py, parts.as_slice());
    for bits in parts.iter() {
        dec_ref_bits(_py, *bits);
    }
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_call(self_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { lru_func_bits(self_ptr) };
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_typed_bits(self_ptr) };
        let typed = is_truthy(_py, obj_from_bits(typed_bits));
        let maxsize = if obj_from_bits(maxsize_bits).is_none() {
            None
        } else {
            let mut val = index_i64_from_obj(_py, maxsize_bits, "maxsize must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if val < 0 {
                val = 0;
            }
            Some(val)
        };
        if maxsize == Some(0) {
            let misses = unsafe { lru_misses(self_ptr) } + 1;
            unsafe { lru_set_misses(self_ptr, misses) };
            let builder_bits = crate::molt_callargs_new(0, 0);
            if builder_bits == 0 {
                return MoltObject::none().bits();
            }
            let mut call_pos: Vec<u64> = Vec::new();
            extend_positional_from_call_arg(args_bits, &mut call_pos);
            for val_bits in call_pos {
                unsafe {
                    let _ = crate::molt_callargs_push_pos(builder_bits, val_bits);
                }
            }
            if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
                if let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
                    unsafe {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            let order = dict_order(dict_ptr);
                            let mut idx = 0;
                            while idx + 1 < order.len() {
                                let _ = crate::molt_callargs_push_kw(
                                    builder_bits,
                                    order[idx],
                                    order[idx + 1],
                                );
                                idx += 2;
                            }
                        }
                    }
                }
            }
            return crate::molt_call_bind(func_bits, builder_bits);
        }
        let key_bits = make_lru_key(_py, args_bits, kwargs_bits, typed);
        if obj_from_bits(key_bits).is_none() {
            return MoltObject::none().bits();
        }
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        let cache_ptr = obj_from_bits(cache_bits).as_ptr();
        let Some(cache_ptr) = cache_ptr else {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(cache_ptr) != TYPE_ID_DICT {
                dec_ref_bits(_py, key_bits);
                return MoltObject::none().bits();
            }
            if let Some(val_bits) = dict_get_in_place(_py, cache_ptr, key_bits) {
                let hits = lru_hits(self_ptr) + 1;
                lru_set_hits(self_ptr, hits);
                let order_ptr = lru_order_ptr(self_ptr);
                if !order_ptr.is_null() {
                    let order = &mut *order_ptr;
                    if let Some(pos) = order.iter().position(|&bits| bits == key_bits) {
                        let removed = order.remove(pos);
                        dec_ref_bits(_py, removed);
                    }
                    order.push(key_bits);
                    inc_ref_bits(_py, key_bits);
                }
                dec_ref_bits(_py, key_bits);
                inc_ref_bits(_py, val_bits);
                return val_bits;
            }
        }
        let misses = unsafe { lru_misses(self_ptr) } + 1;
        unsafe { lru_set_misses(self_ptr, misses) };
        let builder_bits = crate::molt_callargs_new(0, 0);
        if builder_bits == 0 {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        let mut call_pos: Vec<u64> = Vec::new();
        extend_positional_from_call_arg(args_bits, &mut call_pos);
        for val_bits in call_pos {
            unsafe {
                let _ = crate::molt_callargs_push_pos(builder_bits, val_bits);
            }
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            if let Some(dict_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let order = dict_order(dict_ptr);
                        let mut idx = 0;
                        while idx + 1 < order.len() {
                            let _ = crate::molt_callargs_push_kw(
                                builder_bits,
                                order[idx],
                                order[idx + 1],
                            );
                            idx += 2;
                        }
                    }
                }
            }
        }
        let result_bits = crate::molt_call_bind(func_bits, builder_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, key_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            dict_set_in_place(_py, cache_ptr, key_bits, result_bits);
        }
        let order_ptr = unsafe { lru_order_ptr(self_ptr) };
        if !order_ptr.is_null() {
            let order = unsafe { &mut *order_ptr };
            order.push(key_bits);
            inc_ref_bits(_py, key_bits);
            if let Some(maxsize) = maxsize {
                if order.len() > maxsize.max(0) as usize {
                    let oldest = order.remove(0);
                    unsafe {
                        let _ = crate::dict_del_in_place(_py, cache_ptr, oldest);
                    }
                    dec_ref_bits(_py, oldest);
                }
            }
        }
        dec_ref_bits(_py, key_bits);
        result_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_info(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = unsafe { lru_hits(self_ptr) };
        let misses = unsafe { lru_misses(self_ptr) };
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        let currsize = if let Some(cache_ptr) = obj_from_bits(cache_bits).as_ptr() {
            unsafe {
                if object_type_id(cache_ptr) == TYPE_ID_DICT {
                    dict_order(cache_ptr).len() / 2
                } else {
                    0
                }
            }
        } else {
            0
        };
        let class_bits = cacheinfo_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            cacheinfo_set_hits(inst_ptr, hits);
            cacheinfo_set_misses(inst_ptr, misses);
            cacheinfo_set_maxsize_bits(inst_ptr, maxsize_bits);
            cacheinfo_set_currsize(inst_ptr, currsize as i64);
        }
        inc_ref_bits(_py, maxsize_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_iter(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = MoltObject::from_int(unsafe { cacheinfo_hits(self_ptr) }).bits();
        let misses = MoltObject::from_int(unsafe { cacheinfo_misses(self_ptr) }).bits();
        let maxsize_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
        let currsize = MoltObject::from_int(unsafe { cacheinfo_currsize(self_ptr) }).bits();
        let tuple_ptr = alloc_tuple(_py, &[hits, misses, maxsize_bits, currsize]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let iter_bits = crate::molt_iter(tuple_bits);
        dec_ref_bits(_py, tuple_bits);
        iter_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let hits = unsafe { cacheinfo_hits(self_ptr) };
        let misses = unsafe { cacheinfo_misses(self_ptr) };
        let maxsize_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
        let maxsize_repr_bits = molt_repr_from_obj(maxsize_bits);
        let maxsize_repr =
            string_obj_to_owned(obj_from_bits(maxsize_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, maxsize_repr_bits);
        let currsize = unsafe { cacheinfo_currsize(self_ptr) };
        let out = format!(
            "CacheInfo(hits={hits}, misses={misses}, maxsize={maxsize_repr}, currsize={currsize})"
        );
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_cacheinfo_getattr(self_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "attribute name must be string");
        };
        match name.as_str() {
            "hits" => MoltObject::from_int(unsafe { cacheinfo_hits(self_ptr) }).bits(),
            "misses" => MoltObject::from_int(unsafe { cacheinfo_misses(self_ptr) }).bits(),
            "maxsize" => {
                let value_bits = unsafe { cacheinfo_maxsize_bits(self_ptr) };
                inc_ref_bits(_py, value_bits);
                value_bits
            }
            "currsize" => MoltObject::from_int(unsafe { cacheinfo_currsize(self_ptr) }).bits(),
            _ => {
                let msg = format!("'CacheInfo' object has no attribute '{name}'");
                raise_exception::<u64>(_py, "AttributeError", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_clear(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let cache_bits = unsafe { lru_cache_bits(self_ptr) };
        if let Some(cache_ptr) = obj_from_bits(cache_bits).as_ptr() {
            unsafe {
                if object_type_id(cache_ptr) == TYPE_ID_DICT {
                    crate::dict_clear_in_place(_py, cache_ptr);
                }
            }
        }
        let order_ptr = unsafe { lru_order_ptr(self_ptr) };
        if !order_ptr.is_null() {
            unsafe {
                let order = &mut *order_ptr;
                for bits in order.drain(..) {
                    dec_ref_bits(_py, bits);
                }
            }
        }
        unsafe {
            lru_set_hits(self_ptr, 0);
            lru_set_misses(self_ptr, 0);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_cache_params(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let maxsize_bits = unsafe { lru_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_typed_bits(self_ptr) };
        let key1_ptr = alloc_string(_py, b"maxsize");
        let key2_ptr = alloc_string(_py, b"typed");
        if key1_ptr.is_null() || key2_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let key1_bits = MoltObject::from_ptr(key1_ptr).bits();
        let key2_bits = MoltObject::from_ptr(key2_ptr).bits();
        let dict_ptr =
            crate::alloc_dict_with_pairs(_py, &[key1_bits, maxsize_bits, key2_bits, typed_bits]);
        dec_ref_bits(_py, key1_bits);
        dec_ref_bits(_py, key2_bits);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_functools_lru_factory_call(self_bits: u64, func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let maxsize_bits = unsafe { lru_factory_maxsize_bits(self_ptr) };
        let typed_bits = unsafe { lru_factory_typed_bits(self_ptr) };
        let wrapper_bits = build_lru_wrapper(_py, func_bits, maxsize_bits, typed_bits);
        if obj_from_bits(wrapper_bits).is_none() {
            return MoltObject::none().bits();
        }
        molt_functools_update_wrapper(
            wrapper_bits,
            func_bits,
            default_wrapper_assignments(_py),
            default_wrapper_updates(_py),
        )
    })
}

pub(crate) fn functools_drop_instance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    if class_bits == 0 {
        return false;
    }
    let class = PARTIAL_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let func_bits = unsafe { partial_func_bits(ptr) };
        let args_bits = unsafe { partial_args_bits(ptr) };
        let kwargs_bits = unsafe { partial_kwargs_bits(ptr) };
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            dec_ref_bits(_py, func_bits);
        }
        if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            dec_ref_bits(_py, kwargs_bits);
        }
        return true;
    }
    let class = CMPKEY_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let obj_bits = unsafe { cmpkey_obj_bits(ptr) };
        let cmp_bits = unsafe { cmpkey_cmp_bits(ptr) };
        if obj_bits != 0 && !obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, obj_bits);
        }
        if cmp_bits != 0 && !obj_from_bits(cmp_bits).is_none() {
            dec_ref_bits(_py, cmp_bits);
        }
        return true;
    }
    let class = LRU_WRAPPER_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let func_bits = unsafe { lru_func_bits(ptr) };
        let maxsize_bits = unsafe { lru_maxsize_bits(ptr) };
        let typed_bits = unsafe { lru_typed_bits(ptr) };
        let cache_bits = unsafe { lru_cache_bits(ptr) };
        let order_ptr = unsafe { lru_order_ptr(ptr) };
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            dec_ref_bits(_py, func_bits);
        }
        if maxsize_bits != 0 && !obj_from_bits(maxsize_bits).is_none() {
            dec_ref_bits(_py, maxsize_bits);
        }
        if typed_bits != 0 && !obj_from_bits(typed_bits).is_none() {
            dec_ref_bits(_py, typed_bits);
        }
        if cache_bits != 0 && !obj_from_bits(cache_bits).is_none() {
            dec_ref_bits(_py, cache_bits);
        }
        if !order_ptr.is_null() {
            unsafe {
                let order = Box::from_raw(order_ptr);
                for bits in order.iter() {
                    dec_ref_bits(_py, *bits);
                }
            }
        }
        return true;
    }
    let class = LRU_FACTORY_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let maxsize_bits = unsafe { lru_factory_maxsize_bits(ptr) };
        let typed_bits = unsafe { lru_factory_typed_bits(ptr) };
        if maxsize_bits != 0 && !obj_from_bits(maxsize_bits).is_none() {
            dec_ref_bits(_py, maxsize_bits);
        }
        if typed_bits != 0 && !obj_from_bits(typed_bits).is_none() {
            dec_ref_bits(_py, typed_bits);
        }
        return true;
    }
    let class = CACHEINFO_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let maxsize_bits = unsafe { cacheinfo_maxsize_bits(ptr) };
        if maxsize_bits != 0 && !obj_from_bits(maxsize_bits).is_none() {
            dec_ref_bits(_py, maxsize_bits);
        }
        return true;
    }
    false
}
