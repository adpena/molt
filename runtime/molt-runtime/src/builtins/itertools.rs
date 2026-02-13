use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;

use crate::builtins::numbers::index_i64_from_obj;
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_class_obj, alloc_function_obj,
    alloc_list, alloc_object, alloc_string, alloc_tuple, builtin_classes, class_dict_bits,
    dec_ref_bits, dict_set_in_place, exception_pending, inc_ref_bits, init_atomic_bits,
    intern_static_name, is_truthy, molt_add, molt_class_set_base, molt_eq, molt_iter,
    molt_iter_next, obj_from_bits, object_class_bits, object_set_class_bits, object_type_id,
    raise_exception, raise_not_iterable, seq_vec_ref,
};

static ITER_SELF_FN: AtomicU64 = AtomicU64::new(0);
static KWD_MARK_BITS: AtomicU64 = AtomicU64::new(0);

static CHAIN_CLASS: AtomicU64 = AtomicU64::new(0);
static ISLICE_CLASS: AtomicU64 = AtomicU64::new(0);
static REPEAT_CLASS: AtomicU64 = AtomicU64::new(0);
static COUNT_CLASS: AtomicU64 = AtomicU64::new(0);
static CYCLE_CLASS: AtomicU64 = AtomicU64::new(0);
static ACCUMULATE_CLASS: AtomicU64 = AtomicU64::new(0);
static PAIRWISE_CLASS: AtomicU64 = AtomicU64::new(0);
static GROUPBY_CLASS: AtomicU64 = AtomicU64::new(0);
static GROUPBY_ITER_CLASS: AtomicU64 = AtomicU64::new(0);
static TEE_ITER_CLASS: AtomicU64 = AtomicU64::new(0);

static CHAIN_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static ISLICE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static REPEAT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static COUNT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static CYCLE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static ACCUMULATE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static PAIRWISE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static GROUPBY_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static GROUPBY_ITER_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static TEE_NEXT_FN: AtomicU64 = AtomicU64::new(0);

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
        let ptr = alloc_object(_py, total, crate::TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_kwd_mark() -> u64 {
    crate::with_gil_entry!(_py, { kwd_mark_bits(_py) })
}

fn iter_self_bits(_py: &PyToken<'_>) -> u64 {
    builtin_func_bits(
        _py,
        &ITER_SELF_FN,
        crate::molt_itertools_iter_self as *const () as usize as u64,
        1,
    )
}

fn itertools_class(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    layout_size: i64,
    next_slot: &AtomicU64,
    next_fn: u64,
) -> u64 {
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
                let iter_name = intern_static_name(
                    _py,
                    &crate::runtime_state(_py).interned.iter_name,
                    b"__iter__",
                );
                unsafe { dict_set_in_place(_py, dict_ptr, iter_name, iter_self_bits(_py)) };
                let next_name = intern_static_name(
                    _py,
                    &crate::runtime_state(_py).interned.next_name,
                    b"__next__",
                );
                let next_bits = builtin_func_bits(_py, next_slot, next_fn, 1);
                unsafe { dict_set_in_place(_py, dict_ptr, next_name, next_bits) };
            }
        }
        class_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_iter_self(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

unsafe fn chain_iterables_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn chain_current_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn chain_set_iterables_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn chain_set_current_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn islice_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn islice_stop(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn islice_step(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn islice_idx(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn islice_next_idx(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn islice_has_stop(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn islice_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn islice_set_stop(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn islice_set_step(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn islice_set_idx(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn islice_set_next_idx(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn islice_set_has_stop(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn repeat_obj_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn repeat_times(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn repeat_set_obj_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn repeat_set_times(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn count_current_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn count_step_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn count_set_current_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn count_set_step_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn cycle_saved_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn cycle_index(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn cycle_set_saved_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn cycle_set_index(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn accumulate_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn accumulate_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn accumulate_total_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn accumulate_initial_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn accumulate_started(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn accumulate_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn accumulate_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn accumulate_set_total_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn accumulate_set_initial_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn accumulate_set_started(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn pairwise_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn pairwise_prev_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn pairwise_started(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn pairwise_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn pairwise_set_prev_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn pairwise_set_started(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn groupby_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn groupby_keyfunc_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn groupby_tgt_key_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn groupby_curr_key_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn groupby_curr_val_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn groupby_done(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn groupby_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn groupby_set_keyfunc_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn groupby_set_tgt_key_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn groupby_set_curr_key_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn groupby_set_curr_val_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn groupby_set_done(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn groupby_iter_parent_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn groupby_iter_target_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn groupby_iter_set_parent_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn groupby_iter_set_target_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn tee_data_ptr(ptr: *mut u8) -> *mut TeeData {
    unsafe { *(ptr as *mut *mut TeeData) }
}

unsafe fn tee_index(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<*mut TeeData>()) as *const i64) }
}

unsafe fn tee_set_data_ptr(ptr: *mut u8, data: *mut TeeData) {
    unsafe {
        *(ptr as *mut *mut TeeData) = data;
    }
}

unsafe fn tee_set_index(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<*mut TeeData>()) as *mut i64) = val;
    }
}

struct TeeData {
    refcount: usize,
    iter_bits: u64,
    values: Vec<u64>,
    done: bool,
}

fn chain_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &CHAIN_CLASS,
        "chain",
        24,
        &CHAIN_NEXT_FN,
        crate::molt_itertools_chain_next as *const () as usize as u64,
    )
}

fn islice_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &ISLICE_CLASS,
        "islice",
        56,
        &ISLICE_NEXT_FN,
        crate::molt_itertools_islice_next as *const () as usize as u64,
    )
}

fn repeat_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &REPEAT_CLASS,
        "repeat",
        24,
        &REPEAT_NEXT_FN,
        crate::molt_itertools_repeat_next as *const () as usize as u64,
    )
}

fn count_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &COUNT_CLASS,
        "count",
        24,
        &COUNT_NEXT_FN,
        crate::molt_itertools_count_next as *const () as usize as u64,
    )
}

fn cycle_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &CYCLE_CLASS,
        "cycle",
        24,
        &CYCLE_NEXT_FN,
        crate::molt_itertools_cycle_next as *const () as usize as u64,
    )
}

fn accumulate_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &ACCUMULATE_CLASS,
        "accumulate",
        48,
        &ACCUMULATE_NEXT_FN,
        crate::molt_itertools_accumulate_next as *const () as usize as u64,
    )
}

fn pairwise_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &PAIRWISE_CLASS,
        "pairwise",
        32,
        &PAIRWISE_NEXT_FN,
        crate::molt_itertools_pairwise_next as *const () as usize as u64,
    )
}

fn groupby_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &GROUPBY_CLASS,
        "groupby",
        56,
        &GROUPBY_NEXT_FN,
        crate::molt_itertools_groupby_next as *const () as usize as u64,
    )
}

fn groupby_iter_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &GROUPBY_ITER_CLASS,
        "groupby_iterator",
        24,
        &GROUPBY_ITER_NEXT_FN,
        crate::molt_itertools_groupby_iter_next as *const () as usize as u64,
    )
}

fn tee_iter_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &TEE_ITER_CLASS,
        "tee",
        24,
        &TEE_NEXT_FN,
        crate::molt_itertools_tee_next as *const () as usize as u64,
    )
}

fn iter_next_pair(_py: &PyToken<'_>, iter_bits: u64) -> Option<(u64, bool)> {
    let pair_bits = molt_iter_next(iter_bits);
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
pub extern "C" fn molt_itertools_chain(iterables_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterables_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterables_bits);
        }
        let class_bits = chain_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            chain_set_iterables_bits(inst_ptr, iter_bits);
            chain_set_current_bits(inst_ptr, 0);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_chain_from_iterable(iterables_bits: u64) -> u64 {
    molt_itertools_chain(iterables_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_chain_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        loop {
            let current_bits = unsafe { chain_current_bits(self_ptr) };
            if current_bits == 0 || obj_from_bits(current_bits).is_none() {
                let iterables_bits = unsafe { chain_iterables_bits(self_ptr) };
                let Some((next_iterable_bits, done)) = iter_next_pair(_py, iterables_bits) else {
                    return MoltObject::none().bits();
                };
                if done {
                    return raise_exception::<u64>(_py, "StopIteration", "");
                }
                let next_iter_bits = molt_iter(next_iterable_bits);
                if obj_from_bits(next_iter_bits).is_none() {
                    return raise_not_iterable(_py, next_iterable_bits);
                }
                unsafe {
                    chain_set_current_bits(self_ptr, next_iter_bits);
                }
                continue;
            }
            let Some((val_bits, done)) = iter_next_pair(_py, current_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                dec_ref_bits(_py, current_bits);
                unsafe {
                    chain_set_current_bits(self_ptr, 0);
                }
                continue;
            }
            return val_bits;
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_islice(
    iterable_bits: u64,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = kwd_mark_bits(_py);
        let stop_only = stop_bits == missing;
        let start_obj = obj_from_bits(start_bits);
        let stop_obj = obj_from_bits(stop_bits);
        let step_obj = obj_from_bits(step_bits);

        let step = if step_bits == missing || step_obj.is_none() {
            1
        } else {
            let val = index_i64_from_obj(
                _py,
                step_bits,
                "Step for islice() must be a positive integer or None.",
            );
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            val
        };
        if step <= 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Step for islice() must be a positive integer or None.",
            );
        }

        let mut start = 0i64;
        let mut stop = 0i64;
        let has_stop: bool;
        if stop_only {
            if start_obj.is_none() {
                has_stop = false;
            } else {
                let val = index_i64_from_obj(
                    _py,
                    start_bits,
                    "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                );
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                    );
                }
                stop = val;
                has_stop = true;
            }
        } else {
            if !start_obj.is_none() {
                let val = index_i64_from_obj(
                    _py,
                    start_bits,
                    "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                );
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                    );
                }
                start = val;
            }
            if stop_obj.is_none() {
                has_stop = false;
            } else {
                let val = index_i64_from_obj(
                    _py,
                    stop_bits,
                    "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                );
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.",
                    );
                }
                stop = val;
                has_stop = true;
            }
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = islice_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            islice_set_iter_bits(inst_ptr, iter_bits);
            islice_set_stop(inst_ptr, stop);
            islice_set_step(inst_ptr, step);
            islice_set_idx(inst_ptr, 0);
            islice_set_next_idx(inst_ptr, start);
            islice_set_has_stop(inst_ptr, if has_stop { 1 } else { 0 });
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_islice_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let iter_bits = unsafe { islice_iter_bits(self_ptr) };
        let stop = unsafe { islice_stop(self_ptr) };
        let step = unsafe { islice_step(self_ptr) };
        let has_stop = unsafe { islice_has_stop(self_ptr) } != 0;
        let mut idx = unsafe { islice_idx(self_ptr) };
        let mut next_idx = unsafe { islice_next_idx(self_ptr) };
        if has_stop && next_idx >= stop {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        loop {
            if has_stop && idx >= stop {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            if idx == next_idx {
                idx += 1;
                next_idx += step;
                unsafe {
                    islice_set_idx(self_ptr, idx);
                    islice_set_next_idx(self_ptr, next_idx);
                }
                return val_bits;
            }
            idx += 1;
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_repeat(obj_bits: u64, times_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let times = if obj_from_bits(times_bits).is_none() {
            -1
        } else {
            let val = index_i64_from_obj(_py, times_bits, "repeat() arg 2 must be int");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if val < 0 { 0 } else { val }
        };
        let class_bits = repeat_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            repeat_set_obj_bits(inst_ptr, obj_bits);
            repeat_set_times(inst_ptr, times);
        }
        inc_ref_bits(_py, obj_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_repeat_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let obj_bits = unsafe { repeat_obj_bits(self_ptr) };
        let times = unsafe { repeat_times(self_ptr) };
        if times == 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        if times > 0 {
            unsafe { repeat_set_times(self_ptr, times - 1) };
        }
        inc_ref_bits(_py, obj_bits);
        obj_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_count(start_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_bits = count_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            count_set_current_bits(inst_ptr, start_bits);
            count_set_step_bits(inst_ptr, step_bits);
        }
        inc_ref_bits(_py, start_bits);
        inc_ref_bits(_py, step_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_count_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let current_bits = unsafe { count_current_bits(self_ptr) };
        let step_bits = unsafe { count_step_bits(self_ptr) };
        inc_ref_bits(_py, current_bits);
        let next_bits = molt_add(current_bits, step_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, current_bits);
        unsafe {
            count_set_current_bits(self_ptr, next_bits);
        }
        current_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_cycle(iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let mut values: Vec<u64> = Vec::new();
        loop {
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                break;
            }
            values.push(val_bits);
        }
        dec_ref_bits(_py, iter_bits);
        let list_ptr = alloc_list(_py, values.as_slice());
        if list_ptr.is_null() {
            for bits in values.iter() {
                dec_ref_bits(_py, *bits);
            }
            return MoltObject::none().bits();
        }
        for bits in values.iter() {
            dec_ref_bits(_py, *bits);
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let class_bits = cycle_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, list_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, list_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            cycle_set_saved_bits(inst_ptr, list_bits);
            cycle_set_index(inst_ptr, 0);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_cycle_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let list_bits = unsafe { cycle_saved_bits(self_ptr) };
        let list_ptr = obj_from_bits(list_bits).as_ptr();
        let Some(list_ptr) = list_ptr else {
            return raise_exception::<u64>(_py, "StopIteration", "");
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let values = seq_vec_ref(list_ptr);
            if values.is_empty() {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let idx = cycle_index(self_ptr) as usize % values.len();
            let val_bits = values[idx];
            cycle_set_index(self_ptr, (idx + 1) as i64);
            inc_ref_bits(_py, val_bits);
            val_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_accumulate(
    iterable_bits: u64,
    func_bits: u64,
    initial_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = accumulate_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        let missing = kwd_mark_bits(_py);
        unsafe {
            accumulate_set_iter_bits(inst_ptr, iter_bits);
            accumulate_set_func_bits(inst_ptr, func_bits);
            accumulate_set_total_bits(inst_ptr, 0);
            accumulate_set_initial_bits(inst_ptr, initial_bits);
            accumulate_set_started(inst_ptr, 0);
        }
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            inc_ref_bits(_py, func_bits);
        }
        if initial_bits != 0 && initial_bits != missing {
            inc_ref_bits(_py, initial_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_accumulate_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let iter_bits = unsafe { accumulate_iter_bits(self_ptr) };
        let func_bits = unsafe { accumulate_func_bits(self_ptr) };
        let initial_bits = unsafe { accumulate_initial_bits(self_ptr) };
        let missing = kwd_mark_bits(_py);
        let started = unsafe { accumulate_started(self_ptr) } != 0;
        if !started {
            unsafe { accumulate_set_started(self_ptr, 1) };
            if initial_bits != 0 && initial_bits != missing {
                unsafe { accumulate_set_total_bits(self_ptr, initial_bits) };
                inc_ref_bits(_py, initial_bits);
                return initial_bits;
            }
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            unsafe { accumulate_set_total_bits(self_ptr, val_bits) };
            inc_ref_bits(_py, val_bits);
            return val_bits;
        }
        let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        if done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let total_bits = unsafe { accumulate_total_bits(self_ptr) };
        let next_bits = if func_bits == 0 || obj_from_bits(func_bits).is_none() {
            molt_add(total_bits, val_bits)
        } else {
            unsafe { crate::call_callable2(_py, func_bits, total_bits, val_bits) }
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, total_bits);
        unsafe { accumulate_set_total_bits(self_ptr, next_bits) };
        inc_ref_bits(_py, next_bits);
        next_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_pairwise(iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = pairwise_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            pairwise_set_iter_bits(inst_ptr, iter_bits);
            pairwise_set_prev_bits(inst_ptr, 0);
            pairwise_set_started(inst_ptr, 0);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_pairwise_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let iter_bits = unsafe { pairwise_iter_bits(self_ptr) };
        let started = unsafe { pairwise_started(self_ptr) } != 0;
        let mut prev_bits = unsafe { pairwise_prev_bits(self_ptr) };
        if !started {
            let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            prev_bits = val_bits;
            unsafe {
                pairwise_set_prev_bits(self_ptr, prev_bits);
                pairwise_set_started(self_ptr, 1);
            }
        }
        let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        if done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let tuple_ptr = alloc_tuple(_py, &[prev_bits, val_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe { pairwise_set_prev_bits(self_ptr, val_bits) };
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_product(iterables_bits: u64, repeat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let repeat = index_i64_from_obj(_py, repeat_bits, "repeat argument must be an integer");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if repeat < 0 {
            return raise_exception::<_>(_py, "ValueError", "repeat argument cannot be negative");
        }
        if repeat == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            return iter_bits;
        }
        let iterables_ptr = obj_from_bits(iterables_bits).as_ptr();
        let Some(iterables_ptr) = iterables_ptr else {
            return raise_exception::<_>(_py, "TypeError", "product expects a tuple");
        };
        unsafe {
            if object_type_id(iterables_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "product expects a tuple");
            }
            let iterables = seq_vec_ref(iterables_ptr);
            let mut pools: Vec<Vec<u64>> = Vec::new();
            for &iterable_bits in iterables.iter() {
                let Some(tuple_bits) = crate::tuple_from_iter_bits(_py, iterable_bits) else {
                    return MoltObject::none().bits();
                };
                let tuple_ptr = obj_from_bits(tuple_bits).as_ptr().unwrap();
                let pool = seq_vec_ref(tuple_ptr).clone();
                dec_ref_bits(_py, tuple_bits);
                pools.push(pool);
            }
            let mut all_pools: Vec<Vec<u64>> = Vec::new();
            for _ in 0..repeat {
                for pool in pools.iter() {
                    all_pools.push(pool.clone());
                }
            }
            if all_pools.is_empty() {
                let tuple_ptr = alloc_tuple(_py, &[]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
                dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
                if list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let list_bits = MoltObject::from_ptr(list_ptr).bits();
                let iter_bits = molt_iter(list_bits);
                dec_ref_bits(_py, list_bits);
                return iter_bits;
            }
            for pool in all_pools.iter() {
                if pool.is_empty() {
                    let list_ptr = alloc_list(_py, &[]);
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let list_bits = MoltObject::from_ptr(list_ptr).bits();
                    let iter_bits = molt_iter(list_bits);
                    dec_ref_bits(_py, list_bits);
                    return iter_bits;
                }
            }
            let mut indices: Vec<usize> = vec![0; all_pools.len()];
            let mut result: Vec<u64> = all_pools.iter().map(|pool| pool[0]).collect();
            let mut out: Vec<u64> = Vec::new();
            let first_tuple_ptr = alloc_tuple(_py, result.as_slice());
            if first_tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(first_tuple_ptr).bits());
            loop {
                let mut idx = all_pools.len();
                let mut advanced = false;
                while idx > 0 {
                    idx -= 1;
                    let pool = &all_pools[idx];
                    if indices[idx] + 1 < pool.len() {
                        indices[idx] += 1;
                        result[idx] = pool[indices[idx]];
                        for jdx in idx + 1..all_pools.len() {
                            indices[jdx] = 0;
                            result[jdx] = all_pools[jdx][0];
                        }
                        let tuple_ptr = alloc_tuple(_py, result.as_slice());
                        if tuple_ptr.is_null() {
                            for bits in out.iter() {
                                dec_ref_bits(_py, *bits);
                            }
                            return MoltObject::none().bits();
                        }
                        out.push(MoltObject::from_ptr(tuple_ptr).bits());
                        advanced = true;
                        break;
                    }
                }
                if !advanced {
                    break;
                }
            }
            let list_ptr = alloc_list(_py, out.as_slice());
            if list_ptr.is_null() {
                for bits in out.iter() {
                    dec_ref_bits(_py, *bits);
                }
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            for bits in out.iter() {
                dec_ref_bits(_py, *bits);
            }
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            iter_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_permutations(iterable_bits: u64, r_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pool_bits) = (unsafe { crate::tuple_from_iter_bits(_py, iterable_bits) }) else {
            return MoltObject::none().bits();
        };
        let pool_ptr = obj_from_bits(pool_bits).as_ptr().unwrap();
        let pool = unsafe { seq_vec_ref(pool_ptr) };
        let n = pool.len();
        let r = if obj_from_bits(r_bits).is_none() {
            n as i64
        } else {
            let val = index_i64_from_obj(_py, r_bits, "r must be an integer");
            if exception_pending(_py) {
                dec_ref_bits(_py, pool_bits);
                return MoltObject::none().bits();
            }
            val
        };
        if r < 0 {
            dec_ref_bits(_py, pool_bits);
            return raise_exception::<_>(_py, "ValueError", "r must be non-negative");
        }
        if r as usize > n {
            dec_ref_bits(_py, pool_bits);
            let list_ptr = alloc_list(_py, &[]);
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            return iter_bits;
        }
        if r as usize == 0 {
            dec_ref_bits(_py, pool_bits);
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            return iter_bits;
        }
        let r_usize = r as usize;
        let mut indices: Vec<usize> = (0..n).collect();
        let mut cycles: Vec<isize> = (0..r_usize).map(|i| (n - i) as isize).collect();
        let mut out: Vec<u64> = Vec::new();
        let first: Vec<u64> = indices[..r_usize].iter().map(|&i| pool[i]).collect();
        let tuple_ptr = alloc_tuple(_py, first.as_slice());
        if tuple_ptr.is_null() {
            dec_ref_bits(_py, pool_bits);
            return MoltObject::none().bits();
        }
        out.push(MoltObject::from_ptr(tuple_ptr).bits());
        loop {
            let mut idx = r_usize;
            let mut advanced = false;
            while idx > 0 {
                idx -= 1;
                cycles[idx] -= 1;
                if cycles[idx] == 0 {
                    let removed = indices.remove(idx);
                    indices.push(removed);
                    cycles[idx] = (n - idx) as isize;
                } else {
                    let j = cycles[idx] as usize;
                    indices.swap(idx, n - j);
                    let next: Vec<u64> = indices[..r_usize].iter().map(|&i| pool[i]).collect();
                    let tup_ptr = alloc_tuple(_py, next.as_slice());
                    if tup_ptr.is_null() {
                        for bits in out.iter() {
                            dec_ref_bits(_py, *bits);
                        }
                        dec_ref_bits(_py, pool_bits);
                        return MoltObject::none().bits();
                    }
                    out.push(MoltObject::from_ptr(tup_ptr).bits());
                    advanced = true;
                    break;
                }
            }
            if !advanced {
                break;
            }
        }
        dec_ref_bits(_py, pool_bits);
        let list_ptr = alloc_list(_py, out.as_slice());
        if list_ptr.is_null() {
            for bits in out.iter() {
                dec_ref_bits(_py, *bits);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        for bits in out.iter() {
            dec_ref_bits(_py, *bits);
        }
        let iter_bits = molt_iter(list_bits);
        dec_ref_bits(_py, list_bits);
        iter_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_combinations(iterable_bits: u64, r_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pool_bits) = (unsafe { crate::tuple_from_iter_bits(_py, iterable_bits) }) else {
            return MoltObject::none().bits();
        };
        let pool_ptr = obj_from_bits(pool_bits).as_ptr().unwrap();
        let pool = unsafe { seq_vec_ref(pool_ptr) };
        let n = pool.len();
        let r = index_i64_from_obj(_py, r_bits, "r must be an integer");
        if exception_pending(_py) {
            dec_ref_bits(_py, pool_bits);
            return MoltObject::none().bits();
        }
        if r < 0 {
            dec_ref_bits(_py, pool_bits);
            return raise_exception::<_>(_py, "ValueError", "r must be non-negative");
        }
        if r as usize > n {
            dec_ref_bits(_py, pool_bits);
            let list_ptr = alloc_list(_py, &[]);
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            return iter_bits;
        }
        if r == 0 {
            dec_ref_bits(_py, pool_bits);
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let iter_bits = molt_iter(list_bits);
            dec_ref_bits(_py, list_bits);
            return iter_bits;
        }
        let r_usize = r as usize;
        let mut indices: Vec<usize> = (0..r_usize).collect();
        let mut out: Vec<u64> = Vec::new();
        let first: Vec<u64> = indices.iter().map(|&i| pool[i]).collect();
        let tuple_ptr = alloc_tuple(_py, first.as_slice());
        if tuple_ptr.is_null() {
            dec_ref_bits(_py, pool_bits);
            return MoltObject::none().bits();
        }
        out.push(MoltObject::from_ptr(tuple_ptr).bits());
        loop {
            let mut idx = r_usize;
            let mut found = false;
            while idx > 0 {
                idx -= 1;
                if indices[idx] != idx + n - r_usize {
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
            indices[idx] += 1;
            for j in idx + 1..r_usize {
                indices[j] = indices[j - 1] + 1;
            }
            let next: Vec<u64> = indices.iter().map(|&i| pool[i]).collect();
            let tup_ptr = alloc_tuple(_py, next.as_slice());
            if tup_ptr.is_null() {
                for bits in out.iter() {
                    dec_ref_bits(_py, *bits);
                }
                dec_ref_bits(_py, pool_bits);
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(tup_ptr).bits());
        }
        dec_ref_bits(_py, pool_bits);
        let list_ptr = alloc_list(_py, out.as_slice());
        if list_ptr.is_null() {
            for bits in out.iter() {
                dec_ref_bits(_py, *bits);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        for bits in out.iter() {
            dec_ref_bits(_py, *bits);
        }
        let iter_bits = molt_iter(list_bits);
        dec_ref_bits(_py, list_bits);
        iter_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_groupby(iterable_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = groupby_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        let missing = crate::missing_bits(_py);
        unsafe {
            groupby_set_iter_bits(inst_ptr, iter_bits);
            groupby_set_keyfunc_bits(inst_ptr, key_bits);
            groupby_set_tgt_key_bits(inst_ptr, missing);
            groupby_set_curr_key_bits(inst_ptr, missing);
            groupby_set_curr_val_bits(inst_ptr, missing);
            groupby_set_done(inst_ptr, 0);
        }
        if key_bits != 0 && !obj_from_bits(key_bits).is_none() {
            inc_ref_bits(_py, key_bits);
        }
        inst_bits
    })
}

fn groupby_advance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let iter_bits = unsafe { groupby_iter_bits(ptr) };
    let keyfunc_bits = unsafe { groupby_keyfunc_bits(ptr) };
    let missing = crate::missing_bits(_py);
    let Some((val_bits, done)) = iter_next_pair(_py, iter_bits) else {
        return false;
    };
    if done {
        unsafe {
            groupby_set_done(ptr, 1);
            groupby_set_curr_key_bits(ptr, missing);
        }
        return true;
    }
    let key_bits = if keyfunc_bits == 0 || obj_from_bits(keyfunc_bits).is_none() {
        inc_ref_bits(_py, val_bits);
        val_bits
    } else {
        let res_bits = unsafe { crate::call_callable1(_py, keyfunc_bits, val_bits) };
        if exception_pending(_py) {
            return false;
        }
        res_bits
    };
    let curr_key_bits = unsafe { groupby_curr_key_bits(ptr) };
    let curr_val_bits = unsafe { groupby_curr_val_bits(ptr) };
    if curr_key_bits != 0 && !obj_from_bits(curr_key_bits).is_none() && curr_key_bits != missing {
        dec_ref_bits(_py, curr_key_bits);
    }
    if curr_val_bits != 0 && !obj_from_bits(curr_val_bits).is_none() && curr_val_bits != missing {
        dec_ref_bits(_py, curr_val_bits);
    }
    unsafe {
        groupby_set_curr_key_bits(ptr, key_bits);
        groupby_set_curr_val_bits(ptr, val_bits);
    }
    inc_ref_bits(_py, val_bits);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_groupby_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        if unsafe { groupby_done(self_ptr) } != 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let missing = crate::missing_bits(_py);
        let curr_key_bits = unsafe { groupby_curr_key_bits(self_ptr) };
        if curr_key_bits == missing {
            if !groupby_advance(_py, self_ptr) {
                return MoltObject::none().bits();
            }
            if unsafe { groupby_done(self_ptr) } != 0 {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
        }
        loop {
            let tgt_key_bits = unsafe { groupby_tgt_key_bits(self_ptr) };
            let curr_key_bits = unsafe { groupby_curr_key_bits(self_ptr) };
            if tgt_key_bits != missing {
                let eq_bits = molt_eq(tgt_key_bits, curr_key_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if is_truthy(_py, obj_from_bits(eq_bits)) {
                    if !groupby_advance(_py, self_ptr) {
                        return MoltObject::none().bits();
                    }
                    if unsafe { groupby_done(self_ptr) } != 0 {
                        return raise_exception::<u64>(_py, "StopIteration", "");
                    }
                    continue;
                }
            }
            break;
        }
        let curr_key_bits = unsafe { groupby_curr_key_bits(self_ptr) };
        unsafe { groupby_set_tgt_key_bits(self_ptr, curr_key_bits) };
        inc_ref_bits(_py, curr_key_bits);
        let class_bits = groupby_iter_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, curr_key_bits);
            return MoltObject::none().bits();
        };
        let iter_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, curr_key_bits);
            return MoltObject::none().bits();
        }
        let iter_ptr = obj_from_bits(iter_bits).as_ptr().unwrap();
        unsafe {
            groupby_iter_set_parent_bits(iter_ptr, self_bits);
            groupby_iter_set_target_bits(iter_ptr, curr_key_bits);
        }
        inc_ref_bits(_py, self_bits);
        let pair_ptr = alloc_tuple(_py, &[curr_key_bits, iter_bits]);
        if pair_ptr.is_null() {
            dec_ref_bits(_py, curr_key_bits);
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, curr_key_bits);
        dec_ref_bits(_py, iter_bits);
        MoltObject::from_ptr(pair_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_groupby_iter_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let parent_bits = unsafe { groupby_iter_parent_bits(self_ptr) };
        let target_bits = unsafe { groupby_iter_target_bits(self_ptr) };
        let parent_ptr = obj_from_bits(parent_bits).as_ptr();
        let Some(parent_ptr) = parent_ptr else {
            return raise_exception::<u64>(_py, "StopIteration", "");
        };
        if unsafe { groupby_done(parent_ptr) } != 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let curr_key_bits = unsafe { groupby_curr_key_bits(parent_ptr) };
        let eq_bits = molt_eq(curr_key_bits, target_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !is_truthy(_py, obj_from_bits(eq_bits)) {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let val_bits = unsafe { groupby_curr_val_bits(parent_ptr) };
        inc_ref_bits(_py, val_bits);
        if !groupby_advance(_py, parent_ptr) {
            dec_ref_bits(_py, val_bits);
            return MoltObject::none().bits();
        }
        val_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_tee(iterable_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = index_i64_from_obj(_py, n_bits, "n must be an integer");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if n < 0 {
            return raise_exception::<_>(_py, "ValueError", "n must be >= 0");
        }
        if n == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        inc_ref_bits(_py, iter_bits);
        let data = Box::new(TeeData {
            refcount: n as usize,
            iter_bits,
            values: Vec::new(),
            done: false,
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = tee_iter_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).iter_bits);
                drop(Box::from_raw(data_ptr));
            };
            return MoltObject::none().bits();
        };
        let mut iters: Vec<u64> = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
            if obj_from_bits(inst_bits).is_none() {
                for bits in iters.iter() {
                    dec_ref_bits(_py, *bits);
                }
                unsafe {
                    dec_ref_bits(_py, (*data_ptr).iter_bits);
                    drop(Box::from_raw(data_ptr));
                };
                return MoltObject::none().bits();
            }
            let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
            unsafe {
                tee_set_data_ptr(inst_ptr, data_ptr);
                tee_set_index(inst_ptr, 0);
            }
            iters.push(inst_bits);
        }
        let tuple_ptr = alloc_tuple(_py, iters.as_slice());
        if tuple_ptr.is_null() {
            for bits in iters.iter() {
                dec_ref_bits(_py, *bits);
            }
            unsafe {
                dec_ref_bits(_py, (*data_ptr).iter_bits);
                drop(Box::from_raw(data_ptr));
            };
            return MoltObject::none().bits();
        }
        for bits in iters.iter() {
            dec_ref_bits(_py, *bits);
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_tee_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { tee_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        let idx = unsafe { tee_index(self_ptr) } as usize;
        if idx < data.values.len() {
            let val_bits = data.values[idx];
            unsafe { tee_set_index(self_ptr, (idx + 1) as i64) };
            inc_ref_bits(_py, val_bits);
            return val_bits;
        }
        if data.done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let Some((val_bits, done)) = iter_next_pair(_py, data.iter_bits) else {
            return MoltObject::none().bits();
        };
        if done {
            data.done = true;
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        data.values.push(val_bits);
        inc_ref_bits(_py, val_bits);
        unsafe { tee_set_index(self_ptr, (idx + 1) as i64) };
        val_bits
    })
}

pub(crate) fn itertools_drop_instance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    if class_bits == 0 {
        return false;
    }
    let class = CHAIN_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iterables_bits = unsafe { chain_iterables_bits(ptr) };
        let current_bits = unsafe { chain_current_bits(ptr) };
        if iterables_bits != 0 && !obj_from_bits(iterables_bits).is_none() {
            dec_ref_bits(_py, iterables_bits);
        }
        if current_bits != 0 && !obj_from_bits(current_bits).is_none() {
            dec_ref_bits(_py, current_bits);
        }
        return true;
    }
    let class = ISLICE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iter_bits = unsafe { islice_iter_bits(ptr) };
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        return true;
    }
    let class = REPEAT_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let obj_bits = unsafe { repeat_obj_bits(ptr) };
        if obj_bits != 0 && !obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, obj_bits);
        }
        return true;
    }
    let class = COUNT_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let current_bits = unsafe { count_current_bits(ptr) };
        let step_bits = unsafe { count_step_bits(ptr) };
        if current_bits != 0 && !obj_from_bits(current_bits).is_none() {
            dec_ref_bits(_py, current_bits);
        }
        if step_bits != 0 && !obj_from_bits(step_bits).is_none() {
            dec_ref_bits(_py, step_bits);
        }
        return true;
    }
    let class = CYCLE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let list_bits = unsafe { cycle_saved_bits(ptr) };
        if list_bits != 0 && !obj_from_bits(list_bits).is_none() {
            dec_ref_bits(_py, list_bits);
        }
        return true;
    }
    let class = ACCUMULATE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iter_bits = unsafe { accumulate_iter_bits(ptr) };
        let func_bits = unsafe { accumulate_func_bits(ptr) };
        let total_bits = unsafe { accumulate_total_bits(ptr) };
        let initial_bits = unsafe { accumulate_initial_bits(ptr) };
        let missing = kwd_mark_bits(_py);
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            dec_ref_bits(_py, func_bits);
        }
        if total_bits != 0 && !obj_from_bits(total_bits).is_none() {
            dec_ref_bits(_py, total_bits);
        }
        if initial_bits != 0 && initial_bits != missing && !obj_from_bits(initial_bits).is_none() {
            dec_ref_bits(_py, initial_bits);
        }
        return true;
    }
    let class = PAIRWISE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iter_bits = unsafe { pairwise_iter_bits(ptr) };
        let prev_bits = unsafe { pairwise_prev_bits(ptr) };
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        if prev_bits != 0 && !obj_from_bits(prev_bits).is_none() {
            dec_ref_bits(_py, prev_bits);
        }
        return true;
    }
    let class = GROUPBY_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iter_bits = unsafe { groupby_iter_bits(ptr) };
        let keyfunc_bits = unsafe { groupby_keyfunc_bits(ptr) };
        let tgt_bits = unsafe { groupby_tgt_key_bits(ptr) };
        let curr_key_bits = unsafe { groupby_curr_key_bits(ptr) };
        let curr_val_bits = unsafe { groupby_curr_val_bits(ptr) };
        let missing = crate::missing_bits(_py);
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        if keyfunc_bits != 0 && !obj_from_bits(keyfunc_bits).is_none() {
            dec_ref_bits(_py, keyfunc_bits);
        }
        for bits in [tgt_bits, curr_key_bits, curr_val_bits] {
            if bits != 0 && bits != missing && !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        return true;
    }
    let class = GROUPBY_ITER_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let parent_bits = unsafe { groupby_iter_parent_bits(ptr) };
        let target_bits = unsafe { groupby_iter_target_bits(ptr) };
        if parent_bits != 0 && !obj_from_bits(parent_bits).is_none() {
            dec_ref_bits(_py, parent_bits);
        }
        if target_bits != 0 && !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return true;
    }
    let class = TEE_ITER_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { tee_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = &mut *data_ptr;
                if data.refcount > 0 {
                    data.refcount -= 1;
                    if data.refcount == 0 {
                        if data.iter_bits != 0 && !obj_from_bits(data.iter_bits).is_none() {
                            dec_ref_bits(_py, data.iter_bits);
                        }
                        for bits in data.values.drain(..) {
                            dec_ref_bits(_py, bits);
                        }
                        drop(Box::from_raw(data_ptr));
                    }
                }
            }
        }
        return true;
    }
    false
}
