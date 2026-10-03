//! Exact fused-loop reductions: `vec_sum`, `vec_prod`, `vec_min` and `vec_max`.
//!
//! Each kernel runs one bounded chunk of an ordinary Python loop over the loop's
//! own iterator, `for x in it:`, whose one-statement body updates an
//! accumulator: `acc += x` (either operand order), `acc *= x`, or the min/max
//! update `if x < acc: acc = x` / `if acc < x: acc = x`.
//!
//! A kernel returns the tuple `(result, last, count, more)`. It consumes, from
//! the iterator's current position, `count` items of an exact list, tuple or
//! range whose updates provably run no Python code: the accumulator and every
//! item are exact ints, bools or floats, and releasing the loop target's
//! previous value runs nothing. `result` is then exactly the accumulator the
//! loop leaves after those items and `last` the loop target's value. It stops
//! before the first item it does not admit, leaving that item and the rest to
//! the loop; at the iterator's end; or after one chunk, with `more` true. The
//! caller publishes the loop target and accumulator after each nonempty chunk,
//! so a signal handler or pending call serviced at its loop's back edge
//! observes the state the loop has there, and rereads both for the next chunk.
//!
//! Arithmetic is the loop's own: exact integers of any size, IEEE-754 float
//! operations in iteration order and Python's int-to-float promotion. Nothing is
//! compensated, reassociated, truncated or wrapped, and no result depends on an
//! earlier call or the environment. Builtin `sum()` is a different operation
//! with its own algorithm (`ops_builtins/builtin_collections.rs`).

use super::ops::{
    float_result_bits, heap_float_value, range_components_i64, range_len_i128,
    range_value_at_index_i64,
};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;

/// Integers of at most this magnitude convert to `f64` exactly.
const EXACT_F64_INT: i128 = 1 << 53;

/// A chunk consumes at most this many items before its loop's back edge.
const VEC_CHUNK: usize = 1 << 12;

/// Past this many bits an int accumulator's own arithmetic dominates, and a
/// chunk ends after each item, as often as the loop observes pending work.
const BIG_ACC_CHUNK_BITS: u64 = 1 << 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reduction {
    Sum,
    Prod,
    Min,
    Max,
}

/// An exact builtin number: arithmetic and comparison between two of them run
/// no Python code.
#[derive(Clone)]
enum Num {
    Int(i128),
    Big(BigInt),
    Float(f64),
}

/// `ptr` is an instance of the builtin class itself, not of a subclass.
#[inline]
unsafe fn is_exact_instance(ptr: *mut u8, builtin_class_bits: u64) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    class_bits == 0 || class_bits == builtin_class_bits
}

/// The value of an exact int, bool or float; `None` for anything whose
/// arithmetic could run Python code.
fn exact_number(py: &PyToken<'_>, bits: u64) -> Option<Num> {
    let obj = obj_from_bits(bits);
    if let Some(value) = obj.as_int() {
        return Some(Num::Int(i128::from(value)));
    }
    if let Some(value) = obj.as_float() {
        return Some(Num::Float(value));
    }
    if let Some(value) = obj.as_bool() {
        return Some(Num::Int(i128::from(value)));
    }
    let ptr = obj.as_ptr()?;
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_BIGINT if is_exact_instance(ptr, builtin_classes(py).int) => {
                Some(Num::Big(bigint_ref(ptr).clone()))
            }
            TYPE_ID_FLOAT if is_exact_instance(ptr, builtin_classes(py).float) => {
                Some(Num::Float(heap_float_value(ptr)))
            }
            _ => None,
        }
    }
}

/// Releasing a fused loop target's previous value cannot run Python code: it
/// is unbound, a value without a reference count, or an exact str, bytes, int
/// or float, none of which has a finalizer or weak references. A fused loop
/// binds its target once per chunk, after the fact, so it may do so only then.
pub(crate) fn loop_target_release_is_inert(py: &PyToken<'_>, bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return true;
    };
    if is_missing_bits(py, bits) {
        return true;
    }
    let builtins = builtin_classes(py);
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_STRING => is_exact_instance(ptr, builtins.str),
            TYPE_ID_BYTES => is_exact_instance(ptr, builtins.bytes),
            TYPE_ID_BIGINT => is_exact_instance(ptr, builtins.int),
            TYPE_ID_FLOAT => is_exact_instance(ptr, builtins.float),
            _ => false,
        }
    }
}

/// Python's int-to-float promotion: the nearest float, ties to even.
#[inline]
fn int_to_f64(value: i128) -> f64 {
    value as f64
}

/// `acc = acc + item` for exact numbers. False, with `acc` unchanged, where
/// Python could raise (an int too large for a float) or for two NaNs: which of
/// them the sum returns depends on the source's operand order, which the op
/// does not carry. The loop itself then runs that item.
fn add_in_place(acc: &mut Num, item: Num) -> bool {
    let next = match (&mut *acc, item) {
        (Num::Int(a), Num::Int(b)) => match a.checked_add(b) {
            Some(sum) => Num::Int(sum),
            None => Num::Big(BigInt::from(*a) + BigInt::from(b)),
        },
        (Num::Int(a), Num::Big(b)) => Num::Big(BigInt::from(*a) + b),
        (Num::Big(a), Num::Int(b)) => {
            *a += BigInt::from(b);
            return true;
        }
        (Num::Big(a), Num::Big(b)) => {
            *a += b;
            return true;
        }
        (Num::Float(a), Num::Float(b)) if a.is_nan() && b.is_nan() => return false,
        (Num::Float(a), Num::Float(b)) => Num::Float(*a + b),
        (Num::Float(a), Num::Int(b)) => Num::Float(*a + int_to_f64(b)),
        (Num::Int(a), Num::Float(b)) => Num::Float(int_to_f64(*a) + b),
        (Num::Float(_), Num::Big(_)) | (Num::Big(_), Num::Float(_)) => return false,
    };
    *acc = next;
    true
}

/// `acc = acc * item` for exact numbers, under the same conditions as
/// [`add_in_place`].
fn mul_in_place(acc: &mut Num, item: Num) -> bool {
    let next = match (&mut *acc, item) {
        (Num::Int(a), Num::Int(b)) => match a.checked_mul(b) {
            Some(product) => Num::Int(product),
            None => Num::Big(BigInt::from(*a) * BigInt::from(b)),
        },
        (Num::Int(a), Num::Big(b)) => Num::Big(BigInt::from(*a) * b),
        (Num::Big(a), Num::Int(b)) => {
            *a *= BigInt::from(b);
            return true;
        }
        (Num::Big(a), Num::Big(b)) => {
            *a *= b;
            return true;
        }
        (Num::Float(a), Num::Float(b)) if a.is_nan() && b.is_nan() => return false,
        (Num::Float(a), Num::Float(b)) => Num::Float(*a * b),
        (Num::Float(a), Num::Int(b)) => Num::Float(*a * int_to_f64(b)),
        (Num::Int(a), Num::Float(b)) => Num::Float(int_to_f64(*a) * b),
        (Num::Float(_), Num::Big(_)) | (Num::Big(_), Num::Float(_)) => return false,
    };
    *acc = next;
    true
}

/// Python's `left < right` for exact numbers; int/float comparison is exact.
/// `None` for a big int against a float, which the loop compares itself.
fn less(left: &Num, right: &Num) -> Option<bool> {
    let exact_float = |value: i128| (-EXACT_F64_INT..=EXACT_F64_INT).contains(&value);
    Some(match (left, right) {
        (Num::Int(a), Num::Int(b)) => a < b,
        (Num::Int(a), Num::Big(b)) => BigInt::from(*a) < *b,
        (Num::Big(a), Num::Int(b)) => *a < BigInt::from(*b),
        (Num::Big(a), Num::Big(b)) => a < b,
        (Num::Float(a), Num::Float(b)) => a < b,
        (Num::Int(a), Num::Float(b)) if exact_float(*a) => int_to_f64(*a) < *b,
        (Num::Float(a), Num::Int(b)) if exact_float(*b) => *a < int_to_f64(*b),
        _ => return None,
    })
}

/// Whether the min/max update rebinds the accumulator to `item`: `item < acc`
/// for min, `acc < item` for max. Both source spellings of each compare the
/// same way for exact numbers, NaN included.
fn replaces(reduction: Reduction, item: &Num, acc: &Num) -> Option<bool> {
    if reduction == Reduction::Min {
        less(item, acc)
    } else {
        less(acc, item)
    }
}

/// An item as the loop binds it: an object of the sequence (borrowed), or a
/// value a flat list or a range holds unboxed.
#[derive(Clone, Copy)]
enum Item {
    Object(u64),
    Int(i64),
    Bool(bool),
}

impl Item {
    /// The item as an owned reference.
    fn into_owned_bits(self, py: &PyToken<'_>) -> u64 {
        match self {
            Item::Object(bits) => {
                inc_ref_bits(py, bits);
                bits
            }
            Item::Int(value) => int_bits_from_i64(py, value),
            Item::Bool(value) => MoltObject::from_bool(value).bits(),
        }
    }
}

/// One chunk's progress: the accumulator's value and, for min/max, the item
/// the loop keeps bound (`None`: the accumulator object the chunk started
/// with), the items consumed and the last of them.
struct Chunk {
    reduction: Reduction,
    acc: Num,
    // Preserve the selected occurrence, not only its numeric value: two
    // iterations may yield equal but distinct heap integers.
    kept: Option<(usize, Item)>,
    count: usize,
    last: Option<Item>,
    /// The chunk ended at its size bound, not at a declined item or the end.
    full: bool,
}

impl Chunk {
    /// One loop iteration; false, with nothing changed, when the loop itself
    /// must run `item`.
    fn step(&mut self, item: Item, value: Num) -> bool {
        let applied = match self.reduction {
            Reduction::Sum => add_in_place(&mut self.acc, value),
            Reduction::Prod => mul_in_place(&mut self.acc, value),
            Reduction::Min | Reduction::Max => match replaces(self.reduction, &value, &self.acc) {
                Some(true) => {
                    self.acc = value;
                    self.kept = Some((self.count, item));
                    true
                }
                Some(false) => true,
                None => false,
            },
        };
        if applied {
            self.count += 1;
            self.last = Some(item);
            if self.count == VEC_CHUNK
                || matches!(&self.acc, Num::Big(big) if big.bits() > BIG_ACC_CHUNK_BITS)
            {
                self.full = true;
            }
        }
        applied
    }

    /// Whether the chunk has room for another item.
    #[inline]
    fn open(&self) -> bool {
        !self.full
    }
}

/// The items of `iter_ptr`'s target from `start`, while the chunk admits them.
/// Returns the iterator position after the consumed items; `None` when the
/// target is not a sequence the kernels read.
unsafe fn fold_iterator_target(
    py: &PyToken<'_>,
    chunk: &mut Chunk,
    iter_ptr: *mut u8,
    start: usize,
) -> Option<usize> {
    let target_ptr = obj_from_bits(unsafe { iter_target_bits(iter_ptr) }).as_ptr()?;
    let builtins = builtin_classes(py);
    unsafe {
        match object_type_id(target_ptr) {
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                let class_bits = if object_type_id(target_ptr) == TYPE_ID_LIST {
                    builtins.list
                } else {
                    builtins.tuple
                };
                if !is_exact_instance(target_ptr, class_bits) {
                    return None;
                }
                // No Python code runs while the chunk holds the borrow.
                crate::object::seq_access::with_borrowed(target_ptr, |items| {
                    for &bits in items.get(start..).unwrap_or(&[]) {
                        if !chunk.open() {
                            break;
                        }
                        let Some(value) = exact_number(py, bits) else {
                            break;
                        };
                        if !chunk.step(Item::Object(bits), value) {
                            break;
                        }
                    }
                });
                Some(start + chunk.count)
            }
            TYPE_ID_LIST_INT => {
                if !is_exact_instance(target_ptr, builtins.list) {
                    return None;
                }
                let storage = crate::object::layout::list_int_vec_ref(target_ptr);
                for &value in storage.as_slice().get(start..).unwrap_or(&[]) {
                    if !chunk.open() || !chunk.step(Item::Int(value), Num::Int(i128::from(value))) {
                        break;
                    }
                }
                Some(start + chunk.count)
            }
            TYPE_ID_LIST_BOOL => {
                if !is_exact_instance(target_ptr, builtins.list) {
                    return None;
                }
                let storage = crate::object::layout::list_bool_vec_ref(target_ptr);
                for &value in storage.as_slice().get(start..).unwrap_or(&[]) {
                    let value = value != 0;
                    if !chunk.open() || !chunk.step(Item::Bool(value), Num::Int(i128::from(value)))
                    {
                        break;
                    }
                }
                Some(start + chunk.count)
            }
            TYPE_ID_RANGE => {
                if !is_exact_instance(target_ptr, builtins.range) {
                    return None;
                }
                let (first, stop, step) = range_components_i64(target_ptr)?;
                let len = range_len_i128(first, stop, step);
                let mut index = start as i128;
                while index < len && chunk.open() {
                    let Some(value) = range_value_at_index_i64(first, stop, step, index) else {
                        break;
                    };
                    if !chunk.step(Item::Int(value), Num::Int(i128::from(value))) {
                        break;
                    }
                    index += 1;
                }
                Some(start + chunk.count)
            }
            _ => None,
        }
    }
}

/// One chunk of the loop over `iter_bits`: the owned `(result, last, count,
/// more)` tuple, or `None` bits with an allocation failure pending.
fn vec_reduction_chunk(
    py: &PyToken<'_>,
    reduction: Reduction,
    iter_bits: u64,
    acc_bits: u64,
    target_bits: u64,
) -> u64 {
    let none = MoltObject::none().bits();
    let declined = |py: &PyToken<'_>| -> u64 {
        let tuple = alloc_tuple(
            py,
            &[
                none,
                none,
                MoltObject::from_int(0).bits(),
                MoltObject::from_bool(false).bits(),
            ],
        );
        if tuple.is_null() {
            none
        } else {
            MoltObject::from_ptr(tuple).bits()
        }
    };
    if !loop_target_release_is_inert(py, target_bits) {
        return declined(py);
    }
    let Some(acc) = exact_number(py, acc_bits) else {
        return declined(py);
    };
    let Some(iter_ptr) = obj_from_bits(iter_bits).as_ptr() else {
        return declined(py);
    };
    let mut chunk = Chunk {
        reduction,
        acc,
        kept: None,
        count: 0,
        last: None,
        full: false,
    };
    unsafe {
        if object_type_id(iter_ptr) != TYPE_ID_ITER {
            return declined(py);
        }
        let start = iter_index(iter_ptr);
        if start == ITER_EXHAUSTED {
            return declined(py);
        }
        let Some(next) = fold_iterator_target(py, &mut chunk, iter_ptr, start) else {
            return declined(py);
        };
        if chunk.count == 0 {
            return declined(py);
        }
        // The loop's iterator advances past exactly the consumed items; the
        // ordinary loop finds the rest, or finishes the iterator at its end.
        iter_set_index(iter_ptr, next);
    }
    // Preserve result-first allocation/failure order. Only the same selected
    // iteration may reuse that owner for the final loop-target binding.
    let selected_last = matches!(reduction, Reduction::Min | Reduction::Max)
        && matches!(chunk.kept, Some((index, _)) if index + 1 == chunk.count);
    let result = match reduction {
        Reduction::Sum | Reduction::Prod => match chunk.acc {
            Num::Int(value) => int_bits_from_i128(py, value),
            Num::Big(value) => int_bits_from_bigint(py, value),
            Num::Float(value) => float_result_bits(py, value),
        },
        Reduction::Min | Reduction::Max => match chunk.kept {
            Some((_, item)) => item.into_owned_bits(py),
            None => {
                inc_ref_bits(py, acc_bits);
                acc_bits
            }
        },
    };
    if exception_pending(py) {
        dec_ref_bits(py, result);
        return none;
    }
    let last = if selected_last {
        inc_ref_bits(py, result);
        result
    } else {
        chunk
            .last
            .map(|item| item.into_owned_bits(py))
            .unwrap_or(none)
    };
    if exception_pending(py) {
        dec_ref_bits(py, result);
        dec_ref_bits(py, last);
        return none;
    }
    let tuple = alloc_tuple(
        py,
        &[
            result,
            last,
            int_bits_from_i64(py, chunk.count as i64),
            MoltObject::from_bool(chunk.full).bits(),
        ],
    );
    dec_ref_bits(py, result);
    dec_ref_bits(py, last);
    if tuple.is_null() {
        return none;
    }
    MoltObject::from_ptr(tuple).bits()
}

/// One chunk of `for x in it: acc = acc + x` (or `x + acc`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum(iter_bits: u64, acc_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        vec_reduction_chunk(_py, Reduction::Sum, iter_bits, acc_bits, target_bits)
    })
}

/// One chunk of `for x in it: acc = acc * x` (or `x * acc`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_prod(iter_bits: u64, acc_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        vec_reduction_chunk(_py, Reduction::Prod, iter_bits, acc_bits, target_bits)
    })
}

/// One chunk of `for x in it: if x < acc: acc = x`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_min(iter_bits: u64, acc_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        vec_reduction_chunk(_py, Reduction::Min, iter_bits, acc_bits, target_bits)
    })
}

/// One chunk of `for x in it: if acc < x: acc = x`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_max(iter_bits: u64, acc_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        vec_reduction_chunk(_py, Reduction::Max, iter_bits, acc_bits, target_bits)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(value: i128) -> Num {
        Num::Int(value)
    }

    fn float_of(value: &Num) -> f64 {
        match value {
            Num::Float(value) => *value,
            _ => panic!("expected a float result"),
        }
    }

    fn add(acc: Num, item: Num) -> Option<Num> {
        let mut acc = acc;
        add_in_place(&mut acc, item).then_some(acc)
    }

    fn mul(acc: Num, item: Num) -> Option<Num> {
        let mut acc = acc;
        mul_in_place(&mut acc, item).then_some(acc)
    }

    fn chunk(reduction: Reduction, acc: Num) -> Chunk {
        Chunk {
            reduction,
            acc,
            kept: None,
            count: 0,
            last: None,
            full: false,
        }
    }

    #[test]
    fn explicit_float_addition_is_sequential_not_compensated() {
        let mut acc = Num::Float(0.0);
        for _ in 0..10 {
            assert!(add_in_place(&mut acc, Num::Float(0.1)));
        }
        // The ordinary loop, unlike builtin sum(), accumulates rounding error.
        assert_eq!(float_of(&acc), 0.9999999999999999);
    }

    #[test]
    fn int_accumulator_promotes_only_when_a_float_arrives() {
        assert!(matches!(add(int(2), int(3)), Some(Num::Int(5))));
        assert_eq!(float_of(&add(int(1), Num::Float(0.5)).unwrap()), 1.5);
        assert!(matches!(add(int(i128::MAX), int(1)), Some(Num::Big(_))));
        assert!(matches!(mul(int(i128::MAX), int(2)), Some(Num::Big(_))));
    }

    #[test]
    fn signed_zero_nan_and_infinity_follow_ieee_addition() {
        assert!(float_of(&add(Num::Float(-0.0), Num::Float(-0.0)).unwrap()).is_sign_negative());
        assert!(float_of(&add(int(0), Num::Float(-0.0)).unwrap()).is_sign_positive());
        assert!(float_of(&add(Num::Float(f64::INFINITY), Num::Float(1.0)).unwrap()).is_infinite());
        assert!(
            float_of(&add(Num::Float(f64::INFINITY), Num::Float(f64::NEG_INFINITY)).unwrap())
                .is_nan()
        );
        // One NaN operand is the result whatever the order; two NaNs are not.
        assert!(float_of(&add(Num::Float(f64::NAN), Num::Float(1.0)).unwrap()).is_nan());
        assert!(add(Num::Float(f64::NAN), Num::Float(-f64::NAN)).is_none());
        assert!(mul(Num::Float(-f64::NAN), Num::Float(f64::NAN)).is_none());
    }

    #[test]
    fn declined_items_leave_the_accumulator_untouched() {
        let big = BigInt::from(i128::MAX) * BigInt::from(i128::MAX);
        let mut acc = Num::Big(big.clone());
        assert!(!add_in_place(&mut acc, Num::Float(1.0)));
        assert!(matches!(&acc, Num::Big(value) if *value == big));
        let mut acc = Num::Float(1.0);
        assert!(!mul_in_place(&mut acc, Num::Big(big)));
        assert_eq!(float_of(&acc), 1.0);
    }

    #[test]
    fn comparisons_are_exact_and_nan_never_replaces() {
        assert_eq!(less(&int(1), &Num::Float(1.5)), Some(true));
        assert_eq!(less(&Num::Float(f64::NAN), &int(1)), Some(false));
        assert_eq!(less(&int(1), &Num::Float(f64::NAN)), Some(false));
        assert_eq!(less(&int(EXACT_F64_INT + 1), &Num::Float(1.0)), None);
        assert_eq!(replaces(Reduction::Min, &int(1), &int(1)), Some(false));
        assert_eq!(replaces(Reduction::Max, &int(2), &int(1)), Some(true));
    }

    /// A chunk stops at its size bound, and a declined item is not consumed.
    #[test]
    fn chunks_are_bounded_and_stop_before_a_declined_item() {
        let mut sum = chunk(Reduction::Sum, int(0));
        let mut stepped = 0usize;
        while sum.open() && sum.step(Item::Int(1), int(1)) {
            stepped += 1;
        }
        assert_eq!(stepped, VEC_CHUNK);
        assert!(sum.full && matches!(sum.acc, Num::Int(value) if value == VEC_CHUNK as i128));

        let mut sum = chunk(Reduction::Sum, Num::Float(f64::NAN));
        assert!(sum.step(Item::Int(1), int(1)));
        assert!(!sum.step(Item::Object(0), Num::Float(f64::NAN)));
        assert_eq!(sum.count, 1);
        assert!(!sum.full);

        // A huge int accumulator ends the chunk after each item.
        let mut prod = chunk(Reduction::Prod, Num::Big(BigInt::from(1) << 5000u32));
        assert!(prod.step(Item::Int(3), int(3)));
        assert!(prod.full && prod.count == 1);
    }

    #[test]
    fn min_and_max_keep_the_item_itself() {
        let mut min = chunk(Reduction::Min, int(5));
        assert!(min.step(Item::Int(7), int(7)));
        assert!(min.kept.is_none());
        assert!(min.step(Item::Int(3), int(3)));
        assert!(matches!(min.kept, Some((1, Item::Int(3)))));
        assert!(matches!(min.last, Some(Item::Int(3))));
        let mut max = chunk(Reduction::Max, Num::Float(1.0));
        assert!(max.step(Item::Bool(true), int(1)));
        assert!(max.kept.is_none(), "1 < 1.0 is false: the float stays");
    }

    fn chunk_fields(bits: u64) -> [u64; 4] {
        let ptr = obj_from_bits(bits).as_ptr().expect("owned chunk tuple");
        unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                items.try_into().expect("four chunk fields")
            })
            .expect("chunk result is a tuple")
        }
    }

    fn heap_refs(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("physical heap integer");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn minmax_heap_winner_and_last_share_each_chunk_owner() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let wide = 1_i64 << 62;
            let len = VEC_CHUNK + 1;
            for reduction in [Reduction::Min, Reduction::Max] {
                for boxed_list in [false, true] {
                    let step = if reduction == Reduction::Min { -1 } else { 1 };
                    let first = if step < 0 { wide + len as i64 } else { wide };
                    let values: Vec<i64> = (0..len).map(|i| first + step * i as i64).collect();
                    let sequence = if boxed_list {
                        MoltObject::from_ptr(
                            crate::object::builders::alloc_list_int_from_raw_slice(_py, &values)
                                .expect("boxed heap-integer source"),
                        )
                        .bits()
                    } else {
                        let start = int_bits_from_i64(_py, first);
                        let stop = int_bits_from_i64(_py, first + step * len as i64);
                        let range =
                            alloc_range(_py, start, stop, MoltObject::from_int(step).bits());
                        assert!(!range.is_null());
                        dec_ref_bits(_py, start);
                        dec_ref_bits(_py, stop);
                        MoltObject::from_ptr(range).bits()
                    };
                    let iter = molt_iter(sequence);
                    let mut acc = int_bits_from_i64(_py, first - step);
                    let mut target = MoltObject::none().bits();
                    for count in [VEC_CHUNK, 1] {
                        let tuple = vec_reduction_chunk(_py, reduction, iter, acc, target);
                        assert!(!exception_pending(_py));
                        let fields = chunk_fields(tuple);
                        assert_eq!(obj_from_bits(fields[2]).as_int(), Some(count as i64));
                        assert_eq!(obj_from_bits(fields[3]).as_bool(), Some(count == VEC_CHUNK));
                        assert_eq!(fields[0], fields[1], "one selected iteration, one object");
                        assert_eq!(
                            heap_refs(fields[0]),
                            if boxed_list { 3 } else { 2 },
                            "tuple edges plus the original boxed container owner"
                        );
                        if boxed_list {
                            let last_index = if count == VEC_CHUNK {
                                VEC_CHUNK - 1
                            } else {
                                VEC_CHUNK
                            };
                            let read = molt_index(
                                sequence,
                                MoltObject::from_int(last_index as i64).bits(),
                            );
                            assert_eq!(read, fields[0], "a later list read retains the winner");
                            dec_ref_bits(_py, read);
                        }
                        inc_ref_bits(_py, fields[0]);
                        inc_ref_bits(_py, fields[1]);
                        dec_ref_bits(_py, acc);
                        dec_ref_bits(_py, target);
                        acc = fields[0];
                        target = fields[1];
                        dec_ref_bits(_py, tuple);
                        assert_eq!(
                            heap_refs(acc),
                            if boxed_list { 3 } else { 2 },
                            "published bindings and any original container owner"
                        );
                    }
                    dec_ref_bits(_py, acc);
                    dec_ref_bits(_py, target);
                    dec_ref_bits(_py, iter);
                    dec_ref_bits(_py, sequence);
                }
            }
        });
    }

    #[test]
    fn minmax_tied_heap_occurrences_keep_their_own_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let wide = 1_i64 << 62;
            for reduction in [Reduction::Min, Reduction::Max] {
                for source in ["prior", "first", "same-object"] {
                    let item = int_bits_from_i64(_py, wide);
                    let sequence = if source == "same-object" {
                        let ptr = alloc_tuple(_py, &[item, item]);
                        assert!(!ptr.is_null());
                        MoltObject::from_ptr(ptr).bits()
                    } else {
                        MoltObject::from_ptr(
                            crate::object::builders::alloc_list_int_from_raw_slice(
                                _py,
                                &[wide, wide],
                            )
                            .expect("boxed heap-integer source"),
                        )
                        .bits()
                    };
                    let seed = if source == "prior" {
                        wide
                    } else if reduction == Reduction::Min {
                        wide + 1
                    } else {
                        wide - 1
                    };
                    let acc = int_bits_from_i64(_py, seed);
                    let iter = molt_iter(sequence);
                    let item_refs = heap_refs(item);
                    let tuple =
                        vec_reduction_chunk(_py, reduction, iter, acc, MoltObject::none().bits());
                    assert!(!exception_pending(_py));
                    let fields = chunk_fields(tuple);
                    assert_eq!(obj_from_bits(fields[2]).as_int(), Some(2));
                    if source == "same-object" {
                        assert_eq!(fields[0], item);
                        assert_eq!(fields[1], item);
                        assert_eq!(heap_refs(item), item_refs + 2);
                    } else {
                        assert_ne!(
                            fields[0], fields[1],
                            "equal values are separate occurrences"
                        );
                        assert_eq!(fields[0] == acc, source == "prior");
                        assert_eq!(heap_refs(fields[0]), 2);
                        let first = molt_index(sequence, MoltObject::from_int(0).bits());
                        assert_eq!(fields[0] == first, source == "first");
                        dec_ref_bits(_py, first);
                        assert_eq!(heap_refs(fields[1]), 2);
                        let last = molt_index(sequence, MoltObject::from_int(1).bits());
                        assert_eq!(fields[1], last);
                        dec_ref_bits(_py, last);
                    }
                    dec_ref_bits(_py, tuple);
                    assert_eq!(heap_refs(acc), 1);
                    assert_eq!(heap_refs(item), item_refs);
                    dec_ref_bits(_py, iter);
                    dec_ref_bits(_py, sequence);
                    assert_eq!(heap_refs(item), 1);
                    dec_ref_bits(_py, acc);
                    dec_ref_bits(_py, item);
                }
            }
        });
    }

    #[test]
    fn prior_chunk_ties_retain_original_winner_and_zero_count_publishes_nothing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let wide = 1_i64 << 62;
            for reduction in [Reduction::Min, Reduction::Max] {
                let ptr = crate::object::builders::alloc_list_int_from_raw_iter(
                    py,
                    VEC_CHUNK + 1,
                    |_| wide,
                )
                .unwrap();
                let sequence = MoltObject::from_ptr(ptr).bits();
                assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_LIST);
                let first = molt_index(sequence, MoltObject::from_int(0).bits());
                let iter = molt_iter(sequence);
                let seed = int_bits_from_i64(
                    py,
                    if reduction == Reduction::Min {
                        wide + 1
                    } else {
                        wide - 1
                    },
                );
                let first_tuple =
                    vec_reduction_chunk(py, reduction, iter, seed, MoltObject::none().bits());
                let first_fields = chunk_fields(first_tuple);
                assert_eq!(first_fields[0], first);
                let second_tuple =
                    vec_reduction_chunk(py, reduction, iter, first_fields[0], first_fields[1]);
                let second_fields = chunk_fields(second_tuple);
                assert_eq!(obj_from_bits(second_fields[2]).as_int(), Some(1));
                assert_eq!(
                    second_fields[0], first,
                    "a later tied chunk keeps the incoming owner"
                );
                assert_ne!(second_fields[0], second_fields[1]);
                let acc_refs = heap_refs(second_fields[0]);
                let target_refs = heap_refs(second_fields[1]);
                let empty =
                    vec_reduction_chunk(py, reduction, iter, second_fields[0], second_fields[1]);
                let empty_fields = chunk_fields(empty);
                assert_eq!(obj_from_bits(empty_fields[2]).as_int(), Some(0));
                assert_eq!(empty_fields[0], MoltObject::none().bits());
                assert_eq!(empty_fields[1], MoltObject::none().bits());
                assert_eq!(heap_refs(second_fields[0]), acc_refs);
                assert_eq!(heap_refs(second_fields[1]), target_refs);
                for bits in [
                    empty,
                    second_tuple,
                    first_tuple,
                    seed,
                    iter,
                    sequence,
                    first,
                ] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        });
    }
}
