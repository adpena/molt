// Collection-style builtins that consume iterables and materialize aggregate results.
// Kept out of ops_builtins.rs so call dispatch and object protocol slots do not share a compilation unit with reduction/sort algorithms.

use crate::object::ops::{as_float_extended, float_result_bits};
use crate::object::ops_arith::binary_type_error;
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::cmp::Ordering;

enum SumExactInt {
    Small(i128),
    Big(BigInt),
}

impl SumExactInt {
    fn from_obj(obj: MoltObject) -> Option<Self> {
        if obj.is_int() {
            return Some(Self::Small(obj.as_int_unchecked() as i128));
        }
        if obj.is_bool() {
            return Some(Self::Small(if (obj.bits() & 0x1) == 1 { 1 } else { 0 }));
        }
        if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
            return Some(Self::Big(unsafe { bigint_ref(ptr).clone() }));
        }
        if let Some(bits) = int_subclass_value_bits_raw(obj.bits()) {
            let value = obj_from_bits(bits);
            if value.is_int() {
                return Some(Self::Small(value.as_int_unchecked() as i128));
            }
            if value.is_bool() {
                return Some(Self::Small(if (value.bits() & 0x1) == 1 { 1 } else { 0 }));
            }
            if let Some(ptr) = bigint_ptr_from_bits(bits) {
                return Some(Self::Big(unsafe { bigint_ref(ptr).clone() }));
            }
        }
        None
    }

    fn add_i128(&mut self, value: i128) {
        match self {
            Self::Small(acc) => {
                if let Some(next) = acc.checked_add(value) {
                    *acc = next;
                } else {
                    *self = Self::Big(BigInt::from(*acc) + BigInt::from(value));
                }
            }
            Self::Big(acc) => *acc += BigInt::from(value),
        }
    }

    fn add_exact(&mut self, value: Self) {
        match value {
            Self::Small(value) => self.add_i128(value),
            Self::Big(value) => match self {
                Self::Small(acc) => *self = Self::Big(BigInt::from(*acc) + value),
                Self::Big(acc) => *acc += value,
            },
        }
    }

    fn to_f64(&self) -> Option<f64> {
        match self {
            Self::Small(value) => value.to_f64(),
            Self::Big(value) => value.to_f64(),
        }
    }

    fn into_bits(self, _py: &PyToken<'_>) -> u64 {
        match self {
            Self::Small(value) => int_bits_from_i128(_py, value),
            Self::Big(value) => int_bits_from_bigint(_py, value),
        }
    }
}

#[inline]
fn sum_float_accumulate(fsum: &mut f64, comp: &mut f64, x: f64) {
    let t = *fsum + x;
    if fsum.abs() >= x.abs() {
        *comp += (*fsum - t) + x;
    } else {
        *comp += (x - t) + *fsum;
    }
    *fsum = t;
}

#[inline]
fn sum_return_original_start(_py: &PyToken<'_>, start_bits: u64) -> u64 {
    inc_ref_bits(_py, start_bits);
    start_bits
}

#[inline]
fn sum_next_iterator_value(_py: &PyToken<'_>, iter_obj: u64) -> Result<Option<u64>, u64> {
    let pair_bits = molt_iter_next(iter_obj);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "object is not an iterator",
        ));
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "object is not an iterator",
            ));
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "object is not an iterator",
            ));
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        if is_truthy(_py, obj_from_bits(done_bits)) {
            Ok(None)
        } else {
            Ok(Some(val_bits))
        }
    }
}

fn sum_generic_from(
    _py: &PyToken<'_>,
    iter_obj: u64,
    mut total_bits: u64,
    mut total_owned: bool,
) -> u64 {
    loop {
        let val_bits = match sum_next_iterator_value(_py, iter_obj) {
            Ok(Some(val_bits)) => val_bits,
            Ok(None) => {
                if !total_owned {
                    inc_ref_bits(_py, total_bits);
                }
                return total_bits;
            }
            Err(error_bits) => return error_bits,
        };
        let next_bits = molt_add(total_bits, val_bits);
        if obj_from_bits(next_bits).is_none() {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return binary_type_error(_py, obj_from_bits(total_bits), obj_from_bits(val_bits), "+");
        }
        total_bits = next_bits;
        total_owned = true;
    }
}

fn sum_float_from(_py: &PyToken<'_>, iter_obj: u64, mut fsum: f64, mut comp: f64) -> u64 {
    loop {
        let val_bits = match sum_next_iterator_value(_py, iter_obj) {
            Ok(Some(val_bits)) => val_bits,
            Ok(None) => return float_result_bits(_py, fsum + comp),
            Err(error_bits) => return error_bits,
        };
        let val_obj = obj_from_bits(val_bits);
        if let Some(x) = as_float_extended(val_obj) {
            sum_float_accumulate(&mut fsum, &mut comp, x);
            continue;
        }
        if let Some(value) = SumExactInt::from_obj(val_obj) {
            let Some(x) = value.to_f64() else {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            };
            sum_float_accumulate(&mut fsum, &mut comp, x);
            continue;
        }
        let total_bits = float_result_bits(_py, fsum + comp);
        let next_bits = molt_add(total_bits, val_bits);
        if obj_from_bits(next_bits).is_none() {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return binary_type_error(_py, obj_from_bits(total_bits), obj_from_bits(val_bits), "+");
        }
        return sum_generic_from(_py, iter_obj, next_bits, true);
    }
}

#[inline]
fn minmax_compare(_py: &PyToken<'_>, best_key_bits: u64, cand_key_bits: u64) -> CompareOutcome {
    compare_objects(
        _py,
        obj_from_bits(cand_key_bits),
        obj_from_bits(best_key_bits),
    )
}

fn molt_minmax_builtin(
    _py: &PyToken<'_>,
    args_bits: u64,
    key_bits: u64,
    default_bits: u64,
    want_max: bool,
    name: &str,
) -> u64 {
    let missing = missing_bits(_py);
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        let msg = format!("{name} expected at least 1 argument, got 0");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let args = seq_vec_ref(args_ptr);
        if args.is_empty() {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let has_default = default_bits != missing;
        if args.len() > 1 && has_default {
            let msg =
                format!("Cannot specify a default for {name}() with multiple positional arguments");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let mut best_bits;
        let mut best_key_bits: u64;
        if args.len() == 1 {
            let iter_bits = molt_iter(args[0]);
            if obj_from_bits(iter_bits).is_none() {
                return raise_not_iterable(_py, args[0]);
            }
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                if has_default {
                    inc_ref_bits(_py, default_bits);
                    return default_bits;
                }
                let msg = format!("{name}() iterable argument is empty");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            best_bits = val_bits;
            if use_key {
                best_key_bits = call_callable1(_py, key_bits, best_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            } else {
                best_key_bits = best_bits;
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    inc_ref_bits(_py, best_bits);
                    return best_bits;
                }
                let cand_key_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                    CompareOutcome::Ordered(ordering) => {
                        if want_max {
                            ordering == Ordering::Greater
                        } else {
                            ordering == Ordering::Less
                        }
                    }
                    CompareOutcome::Unordered => false,
                    CompareOutcome::NotComparable => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return compare_type_error(
                            _py,
                            obj_from_bits(cand_key_bits),
                            obj_from_bits(best_key_bits),
                            if want_max { ">" } else { "<" },
                        );
                    }
                    CompareOutcome::Error => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if replace {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    best_bits = val_bits;
                    best_key_bits = cand_key_bits;
                } else if use_key {
                    dec_ref_bits(_py, cand_key_bits);
                }
            }
        }
        best_bits = args[0];
        if use_key {
            best_key_bits = call_callable1(_py, key_bits, best_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        } else {
            best_key_bits = best_bits;
        }
        for &val_bits in args.iter().skip(1) {
            let cand_key_bits = if use_key {
                let res_bits = call_callable1(_py, key_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                res_bits
            } else {
                val_bits
            };
            let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                CompareOutcome::Ordered(ordering) => {
                    if want_max {
                        ordering == Ordering::Greater
                    } else {
                        ordering == Ordering::Less
                    }
                }
                CompareOutcome::Unordered => false,
                CompareOutcome::NotComparable => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return compare_type_error(
                        _py,
                        obj_from_bits(cand_key_bits),
                        obj_from_bits(best_key_bits),
                        if want_max { ">" } else { "<" },
                    );
                }
                CompareOutcome::Error => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if replace {
                if use_key {
                    dec_ref_bits(_py, best_key_bits);
                }
                best_bits = val_bits;
                best_key_bits = cand_key_bits;
            } else if use_key {
                dec_ref_bits(_py, cand_key_bits);
            }
        }
        if use_key {
            dec_ref_bits(_py, best_key_bits);
        }
        inc_ref_bits(_py, best_bits);
        best_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, false, "min")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, true, "max")
    })
}

struct SortItem {
    key_bits: u64,
    value_bits: u64,
}

enum SortError {
    NotComparable(u64, u64),
    Exception,
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sorted_builtin(iter_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let reverse = is_truthy(_py, obj_from_bits(reverse_bits));
        let mut items: Vec<SortItem> = Vec::new();
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(_py, item.key_bits);
                    }
                }
                // If an exception is pending, propagate it; otherwise the
                // iterator returned a non-pointer sentinel — treat as done
                // and fall through to build the (possibly empty) sorted list.
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                break;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_val_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                items.push(SortItem {
                    key_bits: key_val_bits,
                    value_bits: val_bits,
                });
            }
        }
        let mut error: Option<SortError> = None;
        items.sort_by(|left, right| {
            if error.is_some() {
                return Ordering::Equal;
            }
            let outcome = compare_objects(
                _py,
                obj_from_bits(left.key_bits),
                obj_from_bits(right.key_bits),
            );
            match outcome {
                CompareOutcome::Ordered(ordering) => {
                    if reverse {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                }
                CompareOutcome::Unordered => Ordering::Equal,
                CompareOutcome::NotComparable => {
                    error = Some(SortError::NotComparable(left.key_bits, right.key_bits));
                    Ordering::Equal
                }
                CompareOutcome::Error => {
                    error = Some(SortError::Exception);
                    Ordering::Equal
                }
            }
        });
        if let Some(error) = error {
            if use_key {
                for item in items.drain(..) {
                    dec_ref_bits(_py, item.key_bits);
                }
            }
            match error {
                SortError::NotComparable(left_bits, right_bits) => {
                    let msg = format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        type_name(_py, obj_from_bits(left_bits)),
                        type_name(_py, obj_from_bits(right_bits)),
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                SortError::Exception => {
                    return MoltObject::none().bits();
                }
            }
        }
        let mut out: Vec<u64> = Vec::with_capacity(items.len());
        for item in items.iter() {
            out.push(item.value_bits);
        }
        if use_key {
            for item in items.drain(..) {
                dec_ref_bits(_py, item.key_bits);
            }
        }
        let list_ptr = alloc_list(_py, &out);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sum_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let start_obj = obj_from_bits(start_bits);
        if let Some(ptr) = start_obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum strings [use ''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTES {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytes [use b''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytearray [use b''.join(seq) instead]",
                    );
                }
            }
        }
        // Fast path: if the iterable is a list or tuple of integers, sum
        // directly without going through the iterator protocol.  This avoids
        // allocating a (value, done) tuple per element.
        {
            let iter_obj_check = obj_from_bits(iter_bits);
            if let Some(ptr) = iter_obj_check.as_ptr() {
                let type_id = unsafe { object_type_id(ptr) };
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let elems = unsafe { seq_vec_ref(ptr) };
                    if elems.is_empty() && SumExactInt::from_obj(start_obj).is_some() {
                        return sum_return_original_start(_py, start_bits);
                    }
                    if let Some(mut acc) = SumExactInt::from_obj(start_obj) {
                        let mut all_int = true;
                        for &bits in elems.iter() {
                            let elem = obj_from_bits(bits);
                            if let Some(i) = SumExactInt::from_obj(elem) {
                                acc.add_exact(i);
                            } else {
                                all_int = false;
                                break;
                            }
                        }
                        if all_int {
                            return acc.into_bits(_py);
                        }
                    }
                }
                // Specialized list[int] — elements are raw i64, no NaN-boxing.
                if type_id == TYPE_ID_LIST_INT {
                    let elems = unsafe { crate::object::layout::list_int_vec_ref(ptr) };
                    if elems.len() == 0 && SumExactInt::from_obj(start_obj).is_some() {
                        return sum_return_original_start(_py, start_bits);
                    }
                    if let Some(mut acc) = SumExactInt::from_obj(start_obj) {
                        for &raw in elems.iter() {
                            acc.add_i128(raw as i128);
                        }
                        return acc.into_bits(_py);
                    }
                }
                // Specialized list[bool] — elements are raw u8 (0/1).
                // sum([True, False, True]) == 2
                if type_id == TYPE_ID_LIST_BOOL {
                    let elems = unsafe { crate::object::layout::list_bool_vec_ref(ptr) };
                    if elems.len() == 0 && SumExactInt::from_obj(start_obj).is_some() {
                        return sum_return_original_start(_py, start_bits);
                    }
                    if let Some(mut acc) = SumExactInt::from_obj(start_obj) {
                        for &raw in elems.iter() {
                            acc.add_i128(raw as i128);
                        }
                        return acc.into_bits(_py);
                    }
                }
            }
        }
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        if let Some(mut acc) = SumExactInt::from_obj(start_obj) {
            let mut consumed_any = false;
            loop {
                let val_bits = match sum_next_iterator_value(_py, iter_obj) {
                    Ok(Some(val_bits)) => val_bits,
                    Ok(None) => {
                        if !consumed_any {
                            return sum_return_original_start(_py, start_bits);
                        }
                        return acc.into_bits(_py);
                    }
                    Err(error_bits) => return error_bits,
                };
                consumed_any = true;
                let val_obj = obj_from_bits(val_bits);
                if let Some(value) = SumExactInt::from_obj(val_obj) {
                    acc.add_exact(value);
                    continue;
                }
                if let Some(x) = as_float_extended(val_obj) {
                    let Some(mut fsum) = acc.to_f64() else {
                        return raise_exception::<u64>(
                            _py,
                            "OverflowError",
                            "int too large to convert to float",
                        );
                    };
                    let mut comp = 0.0_f64;
                    sum_float_accumulate(&mut fsum, &mut comp, x);
                    return sum_float_from(_py, iter_obj, fsum, comp);
                }
                let total_bits = acc.into_bits(_py);
                let next_bits = molt_add(total_bits, val_bits);
                if obj_from_bits(next_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return binary_type_error(
                        _py,
                        obj_from_bits(total_bits),
                        obj_from_bits(val_bits),
                        "+",
                    );
                }
                return sum_generic_from(_py, iter_obj, next_bits, true);
            }
        }
        if let Some(start_val) = as_float_extended(start_obj) {
            return sum_float_from(_py, iter_obj, start_val, 0.0_f64);
        }
        sum_generic_from(_py, iter_obj, start_bits, false)
    })
}
