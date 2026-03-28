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
static BATCHED_CLASS: AtomicU64 = AtomicU64::new(0);
static COMBINATIONS_CLASS: AtomicU64 = AtomicU64::new(0);
static COMBINATIONS_WITH_REPLACEMENT_CLASS: AtomicU64 = AtomicU64::new(0);
static COMPRESS_CLASS: AtomicU64 = AtomicU64::new(0);
static DROPWHILE_CLASS: AtomicU64 = AtomicU64::new(0);
static FILTERFALSE_CLASS: AtomicU64 = AtomicU64::new(0);
static PAIRWISE_CLASS: AtomicU64 = AtomicU64::new(0);
static GROUPBY_CLASS: AtomicU64 = AtomicU64::new(0);
static GROUPBY_ITER_CLASS: AtomicU64 = AtomicU64::new(0);
static PRODUCT_CLASS: AtomicU64 = AtomicU64::new(0);
static PERMUTATIONS_CLASS: AtomicU64 = AtomicU64::new(0);
static STARMAP_CLASS: AtomicU64 = AtomicU64::new(0);
static TAKEWHILE_CLASS: AtomicU64 = AtomicU64::new(0);
static TEE_ITER_CLASS: AtomicU64 = AtomicU64::new(0);
static ZIP_LONGEST_CLASS: AtomicU64 = AtomicU64::new(0);

static CHAIN_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static ISLICE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static REPEAT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static COUNT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static CYCLE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static ACCUMULATE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static BATCHED_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static COMBINATIONS_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static COMBINATIONS_WITH_REPLACEMENT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static COMPRESS_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static DROPWHILE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static FILTERFALSE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static PAIRWISE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static GROUPBY_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static GROUPBY_ITER_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static PRODUCT_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static PERMUTATIONS_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static STARMAP_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static TAKEWHILE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static TEE_NEXT_FN: AtomicU64 = AtomicU64::new(0);
static ZIP_LONGEST_NEXT_FN: AtomicU64 = AtomicU64::new(0);

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
        // molt_class_set_base may fail during early initialisation (e.g. when
        // builtins.object has not been fully wired up yet) and leave a pending
        // exception.  Clear it so that subsequent calls from the iterator
        // constructor do not see a stale exception and bail out early —
        // the class dict already has __iter__ and __next__ so the iterator
        // protocol works even without a fully-resolved base.
        if exception_pending(_py) {
            crate::builtins::exceptions::clear_exception(_py);
        }
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

#[inline]
fn islice_advance_idx(idx: i64) -> i64 {
    idx.saturating_add(1)
}

#[inline]
fn islice_advance_next_idx(next_idx: i64, step: i64) -> i64 {
    next_idx.saturating_add(step)
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

unsafe fn batched_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn batched_n(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn batched_strict(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn batched_done(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn batched_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn batched_set_n(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn batched_set_strict(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn batched_set_done(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn combinations_data_ptr(ptr: *mut u8) -> *mut CombinationsData {
    unsafe { *(ptr as *const *mut CombinationsData) }
}

unsafe fn combinations_set_data_ptr(ptr: *mut u8, data: *mut CombinationsData) {
    unsafe {
        *(ptr as *mut *mut CombinationsData) = data;
    }
}

unsafe fn combinations_with_replacement_data_ptr(
    ptr: *mut u8,
) -> *mut CombinationsWithReplacementData {
    unsafe { *(ptr as *const *mut CombinationsWithReplacementData) }
}

unsafe fn combinations_with_replacement_set_data_ptr(
    ptr: *mut u8,
    data: *mut CombinationsWithReplacementData,
) {
    unsafe {
        *(ptr as *mut *mut CombinationsWithReplacementData) = data;
    }
}

unsafe fn product_data_ptr(ptr: *mut u8) -> *mut ProductData {
    unsafe { *(ptr as *const *mut ProductData) }
}

unsafe fn product_set_data_ptr(ptr: *mut u8, data: *mut ProductData) {
    unsafe {
        *(ptr as *mut *mut ProductData) = data;
    }
}

unsafe fn permutations_data_ptr(ptr: *mut u8) -> *mut PermutationsData {
    unsafe { *(ptr as *const *mut PermutationsData) }
}

unsafe fn permutations_set_data_ptr(ptr: *mut u8, data: *mut PermutationsData) {
    unsafe {
        *(ptr as *mut *mut PermutationsData) = data;
    }
}

unsafe fn compress_data_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn compress_selectors_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn compress_set_data_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn compress_set_selectors_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn dropwhile_predicate_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn dropwhile_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn dropwhile_dropping(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn dropwhile_set_predicate_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn dropwhile_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn dropwhile_set_dropping(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn filterfalse_predicate_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn filterfalse_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn filterfalse_set_predicate_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn filterfalse_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
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

unsafe fn starmap_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn starmap_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn starmap_set_func_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn starmap_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn takewhile_predicate_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn takewhile_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn takewhile_done(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

unsafe fn takewhile_set_predicate_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn takewhile_set_iter_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn takewhile_set_done(ptr: *mut u8, val: i64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut i64) = val;
    }
}

unsafe fn zip_longest_data_ptr(ptr: *mut u8) -> *mut ZipLongestData {
    unsafe { *(ptr as *const *mut ZipLongestData) }
}

unsafe fn zip_longest_set_data_ptr(ptr: *mut u8, data: *mut ZipLongestData) {
    unsafe {
        *(ptr as *mut *mut ZipLongestData) = data;
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

struct CombinationsData {
    pool_bits: u64,
    indices: Vec<usize>,
    n: usize,
    r: usize,
    row_buf: Vec<u64>,
    first: bool,
    done: bool,
}

struct CombinationsWithReplacementData {
    pool_bits: u64,
    indices: Vec<usize>,
    n: usize,
    r: usize,
    row_buf: Vec<u64>,
    first: bool,
    done: bool,
}

struct ProductData {
    pools_bits: Vec<u64>,
    indices: Vec<usize>,
    row_buf: Vec<u64>,
    first: bool,
    done: bool,
}

struct PermutationsData {
    pool_bits: u64,
    indices: Vec<usize>,
    cycles: Vec<usize>,
    n: usize,
    r: usize,
    row_buf: Vec<u64>,
    first: bool,
    done: bool,
}

struct ZipLongestData {
    iter_bits: Vec<u64>,
    active: usize,
    fillvalue_bits: u64,
    row_buf: Vec<u64>,
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

fn batched_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &BATCHED_CLASS,
        "batched",
        40,
        &BATCHED_NEXT_FN,
        crate::molt_itertools_batched_next as *const () as usize as u64,
    )
}

fn combinations_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &COMBINATIONS_CLASS,
        "combinations",
        16,
        &COMBINATIONS_NEXT_FN,
        crate::molt_itertools_combinations_next as *const () as usize as u64,
    )
}

fn combinations_with_replacement_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &COMBINATIONS_WITH_REPLACEMENT_CLASS,
        "combinations_with_replacement",
        16,
        &COMBINATIONS_WITH_REPLACEMENT_NEXT_FN,
        crate::molt_itertools_combinations_with_replacement_next as *const () as usize as u64,
    )
}

fn compress_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &COMPRESS_CLASS,
        "compress",
        24,
        &COMPRESS_NEXT_FN,
        crate::molt_itertools_compress_next as *const () as usize as u64,
    )
}

fn dropwhile_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &DROPWHILE_CLASS,
        "dropwhile",
        32,
        &DROPWHILE_NEXT_FN,
        crate::molt_itertools_dropwhile_next as *const () as usize as u64,
    )
}

fn filterfalse_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &FILTERFALSE_CLASS,
        "filterfalse",
        24,
        &FILTERFALSE_NEXT_FN,
        crate::molt_itertools_filterfalse_next as *const () as usize as u64,
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

fn product_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &PRODUCT_CLASS,
        "product",
        16,
        &PRODUCT_NEXT_FN,
        crate::molt_itertools_product_next as *const () as usize as u64,
    )
}

fn permutations_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &PERMUTATIONS_CLASS,
        "permutations",
        16,
        &PERMUTATIONS_NEXT_FN,
        crate::molt_itertools_permutations_next as *const () as usize as u64,
    )
}

fn starmap_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &STARMAP_CLASS,
        "starmap",
        24,
        &STARMAP_NEXT_FN,
        crate::molt_itertools_starmap_next as *const () as usize as u64,
    )
}

fn takewhile_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &TAKEWHILE_CLASS,
        "takewhile",
        32,
        &TAKEWHILE_NEXT_FN,
        crate::molt_itertools_takewhile_next as *const () as usize as u64,
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

fn zip_longest_class(_py: &PyToken<'_>) -> u64 {
    itertools_class(
        _py,
        &ZIP_LONGEST_CLASS,
        "zip_longest",
        16,
        &ZIP_LONGEST_NEXT_FN,
        crate::molt_itertools_zip_longest_next as *const () as usize as u64,
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
                idx = islice_advance_idx(idx);
                // CPython's islice_next (Modules/itertoolsmodule.c) keeps bounded
                // Py_ssize_t counters and clamps on overflow; we mirror that bounded
                // arithmetic model with saturation so release-mode signed overflow can
                // never wrap negative.
                next_idx = islice_advance_next_idx(next_idx, step);
                unsafe {
                    islice_set_idx(self_ptr, idx);
                    islice_set_next_idx(self_ptr, next_idx);
                }
                return val_bits;
            }
            idx = islice_advance_idx(idx);
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
pub extern "C" fn molt_itertools_batched(iterable_bits: u64, n_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = index_i64_from_obj(_py, n_bits, "n must be an integer");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if n < 1 {
            return raise_exception::<u64>(_py, "ValueError", "n must be at least one");
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = batched_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        let strict = strict_bits != 0
            && !obj_from_bits(strict_bits).is_none()
            && is_truthy(_py, obj_from_bits(strict_bits));
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            batched_set_iter_bits(inst_ptr, iter_bits);
            batched_set_n(inst_ptr, n);
            batched_set_strict(inst_ptr, if strict { 1 } else { 0 });
            batched_set_done(inst_ptr, 0);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_batched_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        if unsafe { batched_done(self_ptr) } != 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let iter_bits = unsafe { batched_iter_bits(self_ptr) };
        if iter_bits == 0 || obj_from_bits(iter_bits).is_none() {
            unsafe { batched_set_done(self_ptr, 1) };
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let n = unsafe { batched_n(self_ptr) } as usize;
        let strict = unsafe { batched_strict(self_ptr) } != 0;
        let mut chunk: Vec<u64> = Vec::with_capacity(n);
        for _ in 0..n {
            let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                unsafe {
                    batched_set_done(self_ptr, 1);
                    batched_set_iter_bits(self_ptr, 0);
                }
                dec_ref_bits(_py, iter_bits);
                if chunk.is_empty() {
                    return raise_exception::<u64>(_py, "StopIteration", "");
                }
                if strict {
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "batched(): incomplete batch",
                    );
                }
                break;
            }
            chunk.push(value_bits);
        }
        let tuple_ptr = alloc_tuple(_py, chunk.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_compress(data_bits: u64, selectors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data_iter_bits = molt_iter(data_bits);
        if obj_from_bits(data_iter_bits).is_none() {
            return raise_not_iterable(_py, data_bits);
        }
        let selectors_iter_bits = molt_iter(selectors_bits);
        if obj_from_bits(selectors_iter_bits).is_none() {
            dec_ref_bits(_py, data_iter_bits);
            return raise_not_iterable(_py, selectors_bits);
        }
        let class_bits = compress_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            dec_ref_bits(_py, data_iter_bits);
            dec_ref_bits(_py, selectors_iter_bits);
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            dec_ref_bits(_py, data_iter_bits);
            dec_ref_bits(_py, selectors_iter_bits);
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            compress_set_data_iter_bits(inst_ptr, data_iter_bits);
            compress_set_selectors_iter_bits(inst_ptr, selectors_iter_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_compress_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_iter_bits = unsafe { compress_data_iter_bits(self_ptr) };
        let selectors_iter_bits = unsafe { compress_selectors_iter_bits(self_ptr) };
        loop {
            let Some((data_val_bits, data_done)) = iter_next_pair(_py, data_iter_bits) else {
                return MoltObject::none().bits();
            };
            if data_done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let Some((selector_bits, selectors_done)) = iter_next_pair(_py, selectors_iter_bits)
            else {
                return MoltObject::none().bits();
            };
            if selectors_done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            if is_truthy(_py, obj_from_bits(selector_bits)) {
                return data_val_bits;
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_dropwhile(predicate_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = dropwhile_class(_py);
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
            dropwhile_set_predicate_bits(inst_ptr, predicate_bits);
            dropwhile_set_iter_bits(inst_ptr, iter_bits);
            dropwhile_set_dropping(inst_ptr, 1);
        }
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            inc_ref_bits(_py, predicate_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_dropwhile_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let predicate_bits = unsafe { dropwhile_predicate_bits(self_ptr) };
        let iter_bits = unsafe { dropwhile_iter_bits(self_ptr) };
        if unsafe { dropwhile_dropping(self_ptr) } == 0 {
            let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            return value_bits;
        }
        loop {
            let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let pred_out_bits = unsafe { crate::call_callable1(_py, predicate_bits, value_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if is_truthy(_py, obj_from_bits(pred_out_bits)) {
                continue;
            }
            unsafe {
                dropwhile_set_dropping(self_ptr, 0);
                dropwhile_set_predicate_bits(self_ptr, 0);
            }
            if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
                dec_ref_bits(_py, predicate_bits);
            }
            return value_bits;
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_filterfalse(predicate_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = filterfalse_class(_py);
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
            filterfalse_set_predicate_bits(inst_ptr, predicate_bits);
            filterfalse_set_iter_bits(inst_ptr, iter_bits);
        }
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            inc_ref_bits(_py, predicate_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_filterfalse_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let predicate_bits = unsafe { filterfalse_predicate_bits(self_ptr) };
        let iter_bits = unsafe { filterfalse_iter_bits(self_ptr) };
        let use_identity = predicate_bits == 0 || obj_from_bits(predicate_bits).is_none();
        loop {
            let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
            let truthy = if use_identity {
                is_truthy(_py, obj_from_bits(value_bits))
            } else {
                let predicate_out =
                    unsafe { crate::call_callable1(_py, predicate_bits, value_bits) };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                is_truthy(_py, obj_from_bits(predicate_out))
            };
            if !truthy {
                return value_bits;
            }
        }
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
        let mut pools_bits: Vec<u64> = Vec::new();
        let mut done = false;
        if repeat > 0 {
            let Some(iterables_ptr) = obj_from_bits(iterables_bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "product expects a tuple");
            };
            if unsafe { object_type_id(iterables_ptr) } != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "product expects a tuple");
            }
            let iterables = unsafe { seq_vec_ref(iterables_ptr) };
            let mut base_pools: Vec<u64> = Vec::with_capacity(iterables.len());
            for &iterable_bits in iterables.iter() {
                let Some(tuple_bits) = (unsafe { crate::tuple_from_iter_bits(_py, iterable_bits) })
                else {
                    for bits in base_pools.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                if !done {
                    let tuple_ptr = obj_from_bits(tuple_bits).as_ptr().unwrap();
                    if unsafe { seq_vec_ref(tuple_ptr) }.is_empty() {
                        done = true;
                    }
                }
                base_pools.push(tuple_bits);
            }
            pools_bits.reserve(base_pools.len() * repeat as usize);
            for _ in 0..repeat {
                for &bits in base_pools.iter() {
                    inc_ref_bits(_py, bits);
                    pools_bits.push(bits);
                }
            }
            for bits in base_pools.into_iter() {
                dec_ref_bits(_py, bits);
            }
        }
        let indices = vec![0usize; pools_bits.len()];
        let row_buf = Vec::with_capacity(pools_bits.len());
        let data = Box::new(ProductData {
            pools_bits,
            indices,
            row_buf,
            first: true,
            done,
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = product_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                for bits in (*data_ptr).pools_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            unsafe {
                for bits in (*data_ptr).pools_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe { product_set_data_ptr(inst_ptr, data_ptr) };
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_product_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { product_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        if data.done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        if data.first {
            data.first = false;
        } else if data.pools_bits.is_empty() {
            data.done = true;
            return raise_exception::<u64>(_py, "StopIteration", "");
        } else {
            let mut advanced = false;
            for idx in (0..data.indices.len()).rev() {
                let pool_bits = data.pools_bits[idx];
                let pool_ptr = obj_from_bits(pool_bits).as_ptr().unwrap();
                let pool_len = unsafe { seq_vec_ref(pool_ptr) }.len();
                if data.indices[idx] + 1 < pool_len {
                    data.indices[idx] += 1;
                    for j in idx + 1..data.indices.len() {
                        data.indices[j] = 0;
                    }
                    advanced = true;
                    break;
                }
            }
            if !advanced {
                data.done = true;
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
        }
        if data.pools_bits.is_empty() {
            data.done = true;
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        data.row_buf.clear();
        for (idx, &pool_bits) in data.pools_bits.iter().enumerate() {
            let pool_ptr = obj_from_bits(pool_bits).as_ptr().unwrap();
            let pool = unsafe { seq_vec_ref(pool_ptr) };
            data.row_buf.push(pool[data.indices[idx]]);
        }
        let tuple_ptr = alloc_tuple(_py, data.row_buf.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
        let r_usize = r as usize;
        let done = r_usize > n;
        let indices = if done || r_usize == 0 {
            Vec::new()
        } else {
            (0..n).collect()
        };
        let cycles = if done || r_usize == 0 {
            Vec::new()
        } else {
            (0..r_usize).map(|i| n - i).collect()
        };
        let data = Box::new(PermutationsData {
            pool_bits,
            indices,
            cycles,
            n,
            r: r_usize,
            row_buf: Vec::with_capacity(r_usize),
            first: true,
            done,
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = permutations_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe { permutations_set_data_ptr(inst_ptr, data_ptr) };
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_permutations_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { permutations_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        if data.done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        if data.first {
            data.first = false;
        } else if data.r == 0 {
            data.done = true;
            return raise_exception::<u64>(_py, "StopIteration", "");
        } else {
            let mut advanced = false;
            for idx in (0..data.r).rev() {
                if data.cycles[idx] > 1 {
                    data.cycles[idx] -= 1;
                    let swap_idx = data.n - data.cycles[idx];
                    data.indices.swap(idx, swap_idx);
                    advanced = true;
                    break;
                }
                data.cycles[idx] = data.n - idx;
                data.indices[idx..].rotate_left(1);
            }
            if !advanced {
                data.done = true;
                return raise_exception::<u64>(_py, "StopIteration", "");
            }
        }
        if data.r == 0 {
            data.done = true;
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let pool_ptr = obj_from_bits(data.pool_bits).as_ptr().unwrap();
        let pool = unsafe { seq_vec_ref(pool_ptr) };
        data.row_buf.clear();
        for &pool_idx in data.indices.iter().take(data.r) {
            data.row_buf.push(pool[pool_idx]);
        }
        let tuple_ptr = alloc_tuple(_py, data.row_buf.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
        let r_usize = r as usize;
        let done = r_usize > n;
        let indices = if done || r_usize == 0 {
            Vec::new()
        } else {
            (0..r_usize).collect()
        };
        let data = Box::new(CombinationsData {
            pool_bits,
            indices,
            n,
            r: r_usize,
            row_buf: Vec::with_capacity(r_usize),
            first: true,
            done,
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = combinations_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe { combinations_set_data_ptr(inst_ptr, data_ptr) };
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_combinations_with_replacement(
    iterable_bits: u64,
    r_bits: u64,
) -> u64 {
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
        let r_usize = r as usize;
        let done = n == 0 && r_usize > 0;
        let indices = if done || r_usize == 0 {
            Vec::new()
        } else {
            vec![0; r_usize]
        };
        let data = Box::new(CombinationsWithReplacementData {
            pool_bits,
            indices,
            n,
            r: r_usize,
            row_buf: Vec::with_capacity(r_usize),
            first: true,
            done,
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = combinations_with_replacement_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            unsafe {
                dec_ref_bits(_py, (*data_ptr).pool_bits);
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe { combinations_with_replacement_set_data_ptr(inst_ptr, data_ptr) };
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_combinations_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { combinations_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        if data.done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        if data.first {
            data.first = false;
        } else if data.r == 0 {
            data.done = true;
            return raise_exception::<u64>(_py, "StopIteration", "");
        } else {
            let mut pivot = None;
            for idx in (0..data.r).rev() {
                if data.indices[idx] != idx + data.n - data.r {
                    pivot = Some(idx);
                    break;
                }
            }
            let Some(pivot_idx) = pivot else {
                data.done = true;
                return raise_exception::<u64>(_py, "StopIteration", "");
            };
            data.indices[pivot_idx] += 1;
            for idx in pivot_idx + 1..data.r {
                data.indices[idx] = data.indices[idx - 1] + 1;
            }
        }
        if data.r == 0 {
            data.done = true;
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let pool_ptr = obj_from_bits(data.pool_bits).as_ptr().unwrap();
        let pool = unsafe { seq_vec_ref(pool_ptr) };
        data.row_buf.clear();
        for &idx in data.indices.iter() {
            data.row_buf.push(pool[idx]);
        }
        let tuple_ptr = alloc_tuple(_py, data.row_buf.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_combinations_with_replacement_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { combinations_with_replacement_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        if data.done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        if data.first {
            data.first = false;
        } else if data.r == 0 {
            data.done = true;
            return raise_exception::<u64>(_py, "StopIteration", "");
        } else {
            let mut pivot = None;
            for idx in (0..data.r).rev() {
                if data.indices[idx] != data.n - 1 {
                    pivot = Some(idx);
                    break;
                }
            }
            let Some(pivot_idx) = pivot else {
                data.done = true;
                return raise_exception::<u64>(_py, "StopIteration", "");
            };
            data.indices[pivot_idx] += 1;
            let fill = data.indices[pivot_idx];
            for idx in pivot_idx + 1..data.r {
                data.indices[idx] = fill;
            }
        }
        if data.r == 0 {
            data.done = true;
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let pool_ptr = obj_from_bits(data.pool_bits).as_ptr().unwrap();
        let pool = unsafe { seq_vec_ref(pool_ptr) };
        data.row_buf.clear();
        for &idx in data.indices.iter() {
            data.row_buf.push(pool[idx]);
        }
        let tuple_ptr = alloc_tuple(_py, data.row_buf.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
pub extern "C" fn molt_itertools_starmap(func_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = starmap_class(_py);
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
            starmap_set_func_bits(inst_ptr, func_bits);
            starmap_set_iter_bits(inst_ptr, iter_bits);
        }
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            inc_ref_bits(_py, func_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_starmap_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let func_bits = unsafe { starmap_func_bits(self_ptr) };
        let iter_bits = unsafe { starmap_iter_bits(self_ptr) };
        let Some((args_bits, done)) = iter_next_pair(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        if done {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let builder_bits = crate::molt_callargs_new(0, 0);
        if obj_from_bits(builder_bits).is_none() {
            return MoltObject::none().bits();
        }
        let _ = unsafe { crate::molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        crate::molt_call_bind(func_bits, builder_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_takewhile(predicate_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let class_bits = takewhile_class(_py);
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
            takewhile_set_predicate_bits(inst_ptr, predicate_bits);
            takewhile_set_iter_bits(inst_ptr, iter_bits);
            takewhile_set_done(inst_ptr, 0);
        }
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            inc_ref_bits(_py, predicate_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_takewhile_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        if unsafe { takewhile_done(self_ptr) } != 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let predicate_bits = unsafe { takewhile_predicate_bits(self_ptr) };
        let iter_bits = unsafe { takewhile_iter_bits(self_ptr) };
        let finalize_done = |py: &PyToken<'_>| {
            unsafe {
                takewhile_set_done(self_ptr, 1);
                takewhile_set_predicate_bits(self_ptr, 0);
                takewhile_set_iter_bits(self_ptr, 0);
            }
            if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
                dec_ref_bits(py, predicate_bits);
            }
            if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
                dec_ref_bits(py, iter_bits);
            }
            raise_exception::<u64>(py, "StopIteration", "")
        };
        let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        if done {
            return finalize_done(_py);
        }
        let pred_out_bits = unsafe { crate::call_callable1(_py, predicate_bits, value_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !is_truthy(_py, obj_from_bits(pred_out_bits)) {
            return finalize_done(_py);
        }
        value_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_zip_longest(iterables_bits: u64, fillvalue_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(iterables_ptr) = obj_from_bits(iterables_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "zip_longest expects a tuple");
        };
        if unsafe { object_type_id(iterables_ptr) } != TYPE_ID_TUPLE {
            return raise_exception::<u64>(_py, "TypeError", "zip_longest expects a tuple");
        }
        let iterables = unsafe { seq_vec_ref(iterables_ptr) };
        let mut iter_bits_vec = Vec::with_capacity(iterables.len());
        for &iterable_bits in iterables.iter() {
            let iter_bits = molt_iter(iterable_bits);
            if obj_from_bits(iter_bits).is_none() {
                for bits in iter_bits_vec.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                return raise_not_iterable(_py, iterable_bits);
            }
            iter_bits_vec.push(iter_bits);
        }
        if fillvalue_bits != 0 && !obj_from_bits(fillvalue_bits).is_none() {
            inc_ref_bits(_py, fillvalue_bits);
        }
        let capacity = iter_bits_vec.len();
        let data = Box::new(ZipLongestData {
            active: iter_bits_vec.len(),
            iter_bits: iter_bits_vec,
            fillvalue_bits,
            row_buf: Vec::with_capacity(capacity),
        });
        let data_ptr = Box::into_raw(data);
        let class_bits = zip_longest_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            unsafe {
                for bits in (*data_ptr).iter_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                if fillvalue_bits != 0 && !obj_from_bits(fillvalue_bits).is_none() {
                    dec_ref_bits(_py, fillvalue_bits);
                }
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            unsafe {
                for bits in (*data_ptr).iter_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                if fillvalue_bits != 0 && !obj_from_bits(fillvalue_bits).is_none() {
                    dec_ref_bits(_py, fillvalue_bits);
                }
                drop(Box::from_raw(data_ptr));
            }
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe { zip_longest_set_data_ptr(inst_ptr, data_ptr) };
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_itertools_zip_longest_next(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let data_ptr = unsafe { zip_longest_data_ptr(self_ptr) };
        if data_ptr.is_null() {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let data = unsafe { &mut *data_ptr };
        if data.active == 0 {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let fillvalue_bits = data.fillvalue_bits;
        let out = &mut data.row_buf;
        out.clear();
        let mut produced_value = false;
        for iter_bits_ref in data.iter_bits.iter_mut() {
            let iter_bits = *iter_bits_ref;
            if iter_bits == 0 {
                out.push(fillvalue_bits);
                continue;
            }
            let Some((value_bits, done)) = iter_next_pair(_py, iter_bits) else {
                return MoltObject::none().bits();
            };
            if done {
                *iter_bits_ref = 0;
                if data.active > 0 {
                    data.active -= 1;
                }
                dec_ref_bits(_py, iter_bits);
                out.push(fillvalue_bits);
            } else {
                produced_value = true;
                out.push(value_bits);
            }
        }
        if !produced_value {
            return raise_exception::<u64>(_py, "StopIteration", "");
        }
        let tuple_ptr = alloc_tuple(_py, out.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
    let class = BATCHED_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let iter_bits = unsafe { batched_iter_bits(ptr) };
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        return true;
    }
    let class = COMBINATIONS_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { combinations_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = Box::from_raw(data_ptr);
                if data.pool_bits != 0 && !obj_from_bits(data.pool_bits).is_none() {
                    dec_ref_bits(_py, data.pool_bits);
                }
            }
        }
        return true;
    }
    let class = COMBINATIONS_WITH_REPLACEMENT_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { combinations_with_replacement_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = Box::from_raw(data_ptr);
                if data.pool_bits != 0 && !obj_from_bits(data.pool_bits).is_none() {
                    dec_ref_bits(_py, data.pool_bits);
                }
            }
        }
        return true;
    }
    let class = COMPRESS_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_iter_bits = unsafe { compress_data_iter_bits(ptr) };
        let selectors_iter_bits = unsafe { compress_selectors_iter_bits(ptr) };
        if data_iter_bits != 0 && !obj_from_bits(data_iter_bits).is_none() {
            dec_ref_bits(_py, data_iter_bits);
        }
        if selectors_iter_bits != 0 && !obj_from_bits(selectors_iter_bits).is_none() {
            dec_ref_bits(_py, selectors_iter_bits);
        }
        return true;
    }
    let class = DROPWHILE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let predicate_bits = unsafe { dropwhile_predicate_bits(ptr) };
        let iter_bits = unsafe { dropwhile_iter_bits(ptr) };
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            dec_ref_bits(_py, predicate_bits);
        }
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        return true;
    }
    let class = FILTERFALSE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let predicate_bits = unsafe { filterfalse_predicate_bits(ptr) };
        let iter_bits = unsafe { filterfalse_iter_bits(ptr) };
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            dec_ref_bits(_py, predicate_bits);
        }
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
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
    let class = PRODUCT_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { product_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = Box::from_raw(data_ptr);
                for bits in data.pools_bits.iter().copied() {
                    if bits != 0 && !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
            }
        }
        return true;
    }
    let class = PERMUTATIONS_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { permutations_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = Box::from_raw(data_ptr);
                if data.pool_bits != 0 && !obj_from_bits(data.pool_bits).is_none() {
                    dec_ref_bits(_py, data.pool_bits);
                }
            }
        }
        return true;
    }
    let class = STARMAP_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let func_bits = unsafe { starmap_func_bits(ptr) };
        let iter_bits = unsafe { starmap_iter_bits(ptr) };
        if func_bits != 0 && !obj_from_bits(func_bits).is_none() {
            dec_ref_bits(_py, func_bits);
        }
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
        return true;
    }
    let class = TAKEWHILE_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let predicate_bits = unsafe { takewhile_predicate_bits(ptr) };
        let iter_bits = unsafe { takewhile_iter_bits(ptr) };
        if predicate_bits != 0 && !obj_from_bits(predicate_bits).is_none() {
            dec_ref_bits(_py, predicate_bits);
        }
        if iter_bits != 0 && !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
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
    let class = ZIP_LONGEST_CLASS.load(Ordering::Acquire);
    if class_bits == class {
        let data_ptr = unsafe { zip_longest_data_ptr(ptr) };
        if !data_ptr.is_null() {
            unsafe {
                let data = Box::from_raw(data_ptr);
                for bits in data.iter_bits.iter().copied() {
                    if bits != 0 && !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
                let fillvalue_bits = data.fillvalue_bits;
                if fillvalue_bits != 0 && !obj_from_bits(fillvalue_bits).is_none() {
                    dec_ref_bits(_py, fillvalue_bits);
                }
            }
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{islice_advance_idx, islice_advance_next_idx};

    #[test]
    fn islice_idx_increment_saturates_at_i64_max() {
        assert_eq!(islice_advance_idx(i64::MAX - 1), i64::MAX);
        assert_eq!(islice_advance_idx(i64::MAX), i64::MAX);
    }

    #[test]
    fn islice_next_increment_saturates_instead_of_wrapping_negative() {
        let near_max = i64::MAX - 1;
        let step = 4;
        let wrapped = near_max.wrapping_add(step);
        assert!(
            wrapped < 0,
            "control check: wrapping add should go negative"
        );
        assert_eq!(islice_advance_next_idx(near_max, step), i64::MAX);
    }

    #[test]
    fn islice_next_increment_is_monotonic_near_upper_bound() {
        let mut next_idx = i64::MAX - 3;
        for _ in 0..8 {
            let previous = next_idx;
            next_idx = islice_advance_next_idx(next_idx, 2);
            assert!(
                next_idx >= previous,
                "next index must not decrease near i64::MAX"
            );
        }
        assert_eq!(next_idx, i64::MAX);
    }
}
