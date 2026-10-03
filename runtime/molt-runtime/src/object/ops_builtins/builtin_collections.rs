// Collection-style builtins that consume iterables and materialize aggregate results.
// Kept out of ops_builtins.rs so call dispatch and object protocol slots do not share a compilation unit with reduction/sort algorithms.

use crate::object::iterable::OwnedIterator;
use crate::object::ops::{as_float_extended, float_result_bits};
use crate::object::ops_compare::{CompareBoolOutcome, CompareOp, rich_compare_op_bool};
use crate::object::ops_sys::runtime_target_at_least;
use crate::*;
use molt_cpython_abi::Py_ssize_t;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::os::raw::c_long;

// Builtin `sum()` reads int items as a C `long` into a `Py_ssize_t` total.
// Both are the target CPython's C data model, which is this build's: LP64
// Linux and macOS (64/64), LLP64 Windows (32/64), ILP32 wasm32-wasi (32/32).
// `struct.calcsize('l')` and `sys.maxsize` expose the same two widths.
const _: () = assert!(std::mem::size_of::<c_long>() <= std::mem::size_of::<Py_ssize_t>());

/// An exact integer value.
enum SumExactInt {
    Small(i128),
    Big(BigInt),
}

impl SumExactInt {
    /// `PyLong_CheckExact(obj)`: an int, not a bool or an int subclass.
    fn exact_int(obj: MoltObject) -> Option<Self> {
        if obj.is_int() {
            return Some(Self::Small(obj.as_int_unchecked() as i128));
        }
        bigint_ptr_from_bits(obj.bits()).map(|ptr| Self::Big(unsafe { bigint_ref(ptr).clone() }))
    }

    /// `PyLong_CheckExact(obj) || PyBool_Check(obj)`.
    fn exact(obj: MoltObject) -> Option<Self> {
        if let Some(value) = obj.as_bool() {
            return Some(Self::Small(i128::from(value)));
        }
        Self::exact_int(obj)
    }

    /// `PyLong_Check(obj)`: any int, bools and int subclasses included, by value.
    fn any(obj: MoltObject) -> Option<Self> {
        Self::exact(obj)
            .or_else(|| Self::exact(obj_from_bits(int_subclass_value_bits_raw(obj.bits())?)))
    }

    /// `PyLong_AsLongAndOverflow`: the value as a C `long`, if it is one.
    fn to_c_long(&self) -> Option<c_long> {
        match self {
            Self::Small(value) => c_long::try_from(*value).ok(),
            Self::Big(value) => c_long::try_from(value.to_i64()?).ok(),
        }
    }

    /// `PyLong_AsDouble`: correctly rounded; `None` where Python raises
    /// OverflowError.
    fn to_f64(&self) -> Option<f64> {
        let value = match self {
            Self::Small(value) => *value as f64,
            Self::Big(value) => value.to_f64()?,
        };
        value.is_finite().then_some(value)
    }
}

/// An inline int or a bool: the int items a fast step reads without allocating.
#[inline]
fn inline_int_or_bool(obj: MoltObject) -> Option<i64> {
    obj.as_int().or_else(|| obj.as_bool().map(i64::from))
}

/// `PyFloat_Check(obj)`: a float, float subclasses included, by value.
fn any_float_value(obj: MoltObject) -> Option<f64> {
    as_float_extended(obj)
}

/// CPython's `CompensatedSum`: Neumaier's improvement of Kahan–Babuška
/// summation.
#[derive(Clone, Copy)]
struct CompensatedSum {
    hi: f64,
    lo: f64,
}

impl CompensatedSum {
    #[inline]
    fn new(value: f64) -> Self {
        Self { hi: value, lo: 0.0 }
    }

    #[inline]
    fn add(&mut self, x: f64) {
        let t = self.hi + x;
        if self.hi.abs() >= x.abs() {
            self.lo += (self.hi - t) + x;
        } else {
            self.lo += (x - t) + self.hi;
        }
        self.hi = t;
    }

    /// Before 3.14 the float phase adds a C `long` int item uncompensated.
    #[inline]
    fn add_uncompensated(&mut self, x: f64) {
        self.hi += x;
    }

    /// The compensation is left out when it is zero, which keeps a negative
    /// zero, and when it is not finite, which keeps an infinite or overflowed
    /// sum from becoming NaN.
    #[inline]
    fn value(self) -> f64 {
        if self.lo != 0.0 && self.lo.is_finite() {
            self.hi + self.lo
        } else {
            self.hi
        }
    }
}

fn raise_int_to_float_overflow(_py: &PyToken<'_>) -> bool {
    raise_exception::<()>(_py, "OverflowError", "int too large to convert to float");
    false
}

/// Python's `left + right`, owned, releasing `left` as CPython releases its
/// previous result; `None` with the exception pending. A `None` result from
/// `__add__` is a value, not a failure.
fn sum_add_releasing(_py: &PyToken<'_>, left: u64, right: u64) -> Option<u64> {
    let result = molt_add(left, right);
    dec_ref_bits(_py, left);
    if exception_pending(_py) {
        return None;
    }
    Some(result)
}

/// The specialized phases of `builtin_sum_impl`, in the order CPython runs
/// them. A phase is entered only from an earlier one and only with a result of
/// exactly its type; once `+` has gone generic, no phase is entered again.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SumPhase {
    Long,
    Float,
    Complex,
}

/// Builtin `sum()`: CPython's `builtin_sum_impl` for the target Python. It
/// differs from `acc + item` in an explicit loop only inside its specialized
/// phases, which compensate float additions, and in which items stay in them:
///
/// - the C `long` phase, entered only from an exact int start that is a C
///   `long`, adds exact int and bool items that are C `long`s while the
///   `Py_ssize_t` total does not overflow;
/// - the float phase compensates exact float items. Before 3.14 it adds int
///   items that are C `long`s uncompensated and leaves on any other int; from
///   3.14 it converts every int like `float()` and compensates it;
/// - from 3.14, the complex phase compensates the parts of exact complex
///   items, and adds int and float items to the real part alone, which keeps
///   the sign of an imaginary zero;
/// - any other item leaves a phase through `+`. Once no later phase accepts
///   that result, its owned object is kept through every generic `+` and return.
enum BuiltinSum {
    Long(Py_ssize_t),
    Float(CompensatedSum),
    Complex {
        re: CompensatedSum,
        im: CompensatedSum,
    },
    /// The generic total, owned even when it is an exact builtin number.
    Object(u64),
}

impl BuiltinSum {
    fn new(_py: &PyToken<'_>, start_bits: u64, py314: bool) -> Self {
        if let Some(value) =
            SumExactInt::exact_int(obj_from_bits(start_bits)).and_then(|value| value.to_c_long())
        {
            return Self::Long(value as Py_ssize_t);
        }
        // An int outside C long, or any other generic start, remains the
        // original owned object if iteration is empty.
        inc_ref_bits(_py, start_bits);
        Self::resume(_py, start_bits, SumPhase::Long, py314)
    }

    /// The state once the `after` phase has produced `result`, owned.
    fn resume(_py: &PyToken<'_>, result: u64, after: SumPhase, py314: bool) -> Self {
        if after < SumPhase::Float
            && crate::builtins::numbers::is_exact_float(_py, result)
            && let Some(value) = as_float_extended(obj_from_bits(result))
        {
            dec_ref_bits(_py, result);
            return Self::Float(CompensatedSum::new(value));
        }
        if py314
            && after < SumPhase::Complex
            && let Some(ptr) = complex_ptr_from_bits(result)
        {
            let value = unsafe { *complex_ref(ptr) };
            dec_ref_bits(_py, result);
            return Self::Complex {
                re: CompensatedSum::new(value.re),
                im: CompensatedSum::new(value.im),
            };
        }
        Self::Object(result)
    }

    /// One item that runs no Python code and allocates nothing. `false`
    /// leaves the state unchanged for [`Self::step`].
    fn try_fast_step(&mut self, bits: u64, py314: bool) -> bool {
        let obj = obj_from_bits(bits);
        match self {
            Self::Long(total) => {
                let Some(value) =
                    inline_int_or_bool(obj).and_then(|value| c_long::try_from(value).ok())
                else {
                    return false;
                };
                match total.checked_add(value as Py_ssize_t) {
                    Some(next) => {
                        *total = next;
                        true
                    }
                    None => false,
                }
            }
            Self::Float(total) => {
                if let Some(x) = obj.as_float() {
                    total.add(x);
                    return true;
                }
                let Some(value) = inline_int_or_bool(obj) else {
                    return false;
                };
                if py314 {
                    // An inline int converts to a float exactly.
                    total.add(value as f64);
                    true
                } else if let Ok(value) = c_long::try_from(value) {
                    total.add_uncompensated(value as f64);
                    true
                } else {
                    false
                }
            }
            Self::Complex { re, im } => {
                if let Some(ptr) = complex_ptr_from_bits(bits) {
                    let value = unsafe { *complex_ref(ptr) };
                    re.add(value.re);
                    im.add(value.im);
                    return true;
                }
                if let Some(x) = obj.as_float() {
                    re.add(x);
                    return true;
                }
                let Some(value) = inline_int_or_bool(obj) else {
                    return false;
                };
                re.add(value as f64);
                true
            }
            _ => false,
        }
    }

    /// One item, which may run Python code. `false` with the exception pending.
    fn step(&mut self, _py: &PyToken<'_>, bits: u64, py314: bool) -> bool {
        if self.try_fast_step(bits, py314) {
            return true;
        }
        let obj = obj_from_bits(bits);
        let state = std::mem::replace(self, Self::Object(MoltObject::none().bits()));
        *self = match state {
            Self::Long(total) => {
                if let Some(value) = SumExactInt::exact(obj) {
                    match value
                        .to_c_long()
                        .and_then(|value| total.checked_add(value as Py_ssize_t))
                    {
                        Some(next) => Self::Long(next),
                        None => {
                            let total_bits = int_bits_from_i64(_py, total as i64);
                            let Some(result) = sum_add_releasing(_py, total_bits, bits) else {
                                return false;
                            };
                            Self::Object(result)
                        }
                    }
                } else if crate::builtins::numbers::is_exact_float(_py, bits)
                    && let Some(x) = as_float_extended(obj)
                {
                    // `int + float` is an exact float: the float phase.
                    Self::Float(CompensatedSum::new(total as f64 + x))
                } else {
                    let total_bits = int_bits_from_i64(_py, total as i64);
                    let Some(result) = sum_add_releasing(_py, total_bits, bits) else {
                        return false;
                    };
                    Self::resume(_py, result, SumPhase::Long, py314)
                }
            }
            Self::Float(mut total) => {
                let stays = if crate::builtins::numbers::is_exact_float(_py, bits)
                    && let Some(x) = as_float_extended(obj)
                {
                    total.add(x);
                    true
                } else if let Some(value) = SumExactInt::any(obj) {
                    if py314 {
                        let Some(x) = value.to_f64() else {
                            return raise_int_to_float_overflow(_py);
                        };
                        total.add(x);
                        true
                    } else if let Some(value) = value.to_c_long() {
                        total.add_uncompensated(value as f64);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if stays {
                    Self::Float(total)
                } else {
                    let total_bits = float_result_bits(_py, total.value());
                    let Some(result) = sum_add_releasing(_py, total_bits, bits) else {
                        return false;
                    };
                    Self::resume(_py, result, SumPhase::Float, py314)
                }
            }
            Self::Complex { mut re, mut im } => {
                let stays = if let Some(ptr) = complex_ptr_from_bits(bits) {
                    let value = unsafe { *complex_ref(ptr) };
                    re.add(value.re);
                    im.add(value.im);
                    true
                } else if let Some(value) = SumExactInt::any(obj) {
                    let Some(x) = value.to_f64() else {
                        return raise_int_to_float_overflow(_py);
                    };
                    re.add(x);
                    true
                } else if let Some(x) = any_float_value(obj) {
                    re.add(x);
                    true
                } else {
                    false
                };
                if stays {
                    Self::Complex { re, im }
                } else {
                    let total_bits = complex_bits(_py, re.value(), im.value());
                    let Some(result) = sum_add_releasing(_py, total_bits, bits) else {
                        return false;
                    };
                    Self::Object(result)
                }
            }
            Self::Object(total) => {
                let Some(result) = sum_add_releasing(_py, total, bits) else {
                    return false;
                };
                Self::Object(result)
            }
        };
        true
    }

    fn finish(self, _py: &PyToken<'_>) -> u64 {
        match self {
            Self::Long(total) => int_bits_from_i64(_py, total as i64),
            Self::Float(total) => float_result_bits(_py, total.value()),
            Self::Complex { re, im } => complex_bits(_py, re.value(), im.value()),
            Self::Object(total) => total,
        }
    }

    fn release(self, _py: &PyToken<'_>) {
        if let Self::Object(total) = self {
            dec_ref_bits(_py, total);
        }
    }
}

/// `ptr` is an instance of the builtin class itself, not of a subclass.
#[inline]
unsafe fn sum_exact_instance(ptr: *mut u8, builtin_class_bits: u64) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    class_bits == 0 || class_bits == builtin_class_bits
}

/// An exact list or tuple, read as its iterator reads it: items whose addition
/// runs no Python code under one borrow, and every other item live, so that
/// Python code which mutates the sequence is observed as the loop observes it.
fn sum_sequence(_py: &PyToken<'_>, sum: &mut BuiltinSum, ptr: *mut u8, py314: bool) -> bool {
    let mut index = 0usize;
    loop {
        let consumed = unsafe {
            crate::object::seq_access::with_borrowed(ptr, |items| {
                let mut consumed = 0usize;
                for &bits in items.get(index..).unwrap_or(&[]) {
                    if !sum.try_fast_step(bits, py314) {
                        break;
                    }
                    consumed += 1;
                }
                consumed
            })
        };
        index += consumed;
        let Some(item) = (unsafe { crate::object::seq_access::pin_item(_py, ptr, index) }) else {
            return true;
        };
        index += 1;
        if !sum.step(_py, item.bits(), py314) {
            return false;
        }
    }
}

/// Where builtin `sum()` reads its items. Acquiring an exact list's or tuple's
/// iterator runs no Python code and cannot fail; any other iterable's is
/// acquired before the start is checked, as CPython orders them.
enum SumSource<'a, 'py> {
    Sequence(*mut u8),
    FlatInts(*mut u8),
    FlatBools(*mut u8),
    Iterator(OwnedIterator<'a, 'py>),
}

/// `None` with the exception pending: the iterable's own `__iter__` exception,
/// or TypeError when it has none.
fn sum_source<'a, 'py>(_py: &'a PyToken<'py>, iter_bits: u64) -> Option<SumSource<'a, 'py>> {
    if let Some(ptr) = obj_from_bits(iter_bits).as_ptr() {
        let builtins = builtin_classes(_py);
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_LIST if sum_exact_instance(ptr, builtins.list) => {
                    return Some(SumSource::Sequence(ptr));
                }
                TYPE_ID_TUPLE if sum_exact_instance(ptr, builtins.tuple) => {
                    return Some(SumSource::Sequence(ptr));
                }
                TYPE_ID_LIST_INT if sum_exact_instance(ptr, builtins.list) => {
                    return Some(SumSource::FlatInts(ptr));
                }
                TYPE_ID_LIST_BOOL if sum_exact_instance(ptr, builtins.list) => {
                    return Some(SumSource::FlatBools(ptr));
                }
                _ => {}
            }
        }
    }
    OwnedIterator::new(_py, iter_bits).map(SumSource::Iterator)
}

/// Every item of `source` into `sum`: the result, owned, or `None` with the
/// exception pending. What CPython releases is released in its order: each
/// item after its addition, and after a failed `next()` the partial result
/// before the iterator.
fn sum_items<'a, 'py>(
    _py: &'a PyToken<'py>,
    mut sum: BuiltinSum,
    source: SumSource<'a, 'py>,
    iter_bits: u64,
    py314: bool,
) -> Option<u64> {
    let mut iter = match source {
        SumSource::Sequence(ptr) => {
            if sum_sequence(_py, &mut sum, ptr, py314) {
                return Some(sum.finish(_py));
            }
            sum.release(_py);
            return None;
        }
        // Generic totals use canonical `+`, which may run Python code and
        // mutate the source. Read the list's iterator for that owned lane.
        SumSource::FlatInts(_) | SumSource::FlatBools(_)
            if matches!(sum, BuiltinSum::Object(_)) =>
        {
            let Some(iter) = OwnedIterator::new(_py, iter_bits) else {
                sum.release(_py);
                return None;
            };
            iter
        }
        SumSource::FlatInts(ptr) => {
            let storage = unsafe { crate::object::layout::list_int_vec_ref(ptr) };
            for &value in storage.as_slice() {
                let bits = int_bits_from_i64(_py, value);
                let stepped = sum.step(_py, bits, py314);
                dec_ref_bits(_py, bits);
                if !stepped {
                    sum.release(_py);
                    return None;
                }
            }
            return Some(sum.finish(_py));
        }
        SumSource::FlatBools(ptr) => {
            let storage = unsafe { crate::object::layout::list_bool_vec_ref(ptr) };
            for &value in storage.as_slice() {
                if !sum.step(_py, MoltObject::from_bool(value != 0).bits(), py314) {
                    sum.release(_py);
                    return None;
                }
            }
            return Some(sum.finish(_py));
        }
        SumSource::Iterator(iter) => iter,
    };
    loop {
        match iter.next() {
            Ok(Some(item)) => {
                let stepped = sum.step(_py, item, py314);
                dec_ref_bits(_py, item);
                if !stepped {
                    sum.release(_py);
                    return None;
                }
            }
            Ok(None) => {
                // Releasing the iterator finalizes a generator the caller did
                // not keep, when CPython's `sum()` would.
                drop(iter);
                return Some(sum.finish(_py));
            }
            Err(()) => {
                sum.release(_py);
                return None;
            }
        }
    }
}

/// `PyUnicode_Check`, `PyBytes_Check` or `PyByteArray_Check` on the start,
/// subclasses included: the TypeError message `sum()` raises for it.
fn sum_rejected_start(_py: &PyToken<'_>, start_bits: u64) -> Option<&'static str> {
    obj_from_bits(start_bits).as_ptr()?;
    let builtins = builtin_classes(_py);
    let class_bits = type_of_bits(_py, start_bits);
    [
        (
            builtins.str,
            "sum() can't sum strings [use ''.join(seq) instead]",
        ),
        (
            builtins.bytes,
            "sum() can't sum bytes [use b''.join(seq) instead]",
        ),
        (
            builtins.bytearray,
            "sum() can't sum bytearray [use b''.join(seq) instead]",
        ),
    ]
    .into_iter()
    .find(|&(class, _)| issubclass_bits(class_bits, class))
    .map(|(_, message)| message)
}

/// Builtin `min()`/`max()`: CPython's `min_max`. Each item's value (its key,
/// or the item) is compared to the best one's with one rich comparison
/// (`value < best` for `min`, `value > best` for `max`), whose truth decides.
/// Items, values and the iterator are released where CPython releases them.
fn molt_minmax_builtin(
    _py: &PyToken<'_>,
    args_bits: u64,
    key_bits: u64,
    default_bits: u64,
    op: CompareOp,
    name: &str,
) -> u64 {
    let missing = missing_bits(_py);
    let args = obj_from_bits(args_bits)
        .as_ptr()
        .and_then(|args_ptr| unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                args.first().copied().map(|first| (args.len(), first))
            })
            .flatten()
        });
    let Some((args_len, first_arg)) = args else {
        let msg = format!("{name} expected at least 1 argument, got 0");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    let has_default = default_bits != missing;
    if args_len > 1 && has_default {
        let msg =
            format!("Cannot specify a default for {name}() with multiple positional arguments");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let iterable = if args_len > 1 { args_bits } else { first_arg };
    let Some(mut iter) = OwnedIterator::new(_py, iterable) else {
        return MoltObject::none().bits();
    };
    let use_key = !obj_from_bits(key_bits).is_none();
    // The best item and the value it is compared by, both owned.
    let mut best: Option<(u64, u64)> = None;
    let completed = loop {
        let item = match iter.next() {
            Ok(Some(item)) => item,
            Ok(None) => break true,
            Err(()) => break false,
        };
        let value = if use_key {
            let value = unsafe { call_callable1(_py, key_bits, item) };
            if exception_pending(_py) {
                dec_ref_bits(_py, value);
                dec_ref_bits(_py, item);
                break false;
            }
            value
        } else {
            inc_ref_bits(_py, item);
            item
        };
        let Some((best_item, best_value)) = best else {
            best = Some((item, value));
            continue;
        };
        match rich_compare_op_bool(_py, obj_from_bits(value), obj_from_bits(best_value), op) {
            CompareBoolOutcome::True => {
                dec_ref_bits(_py, best_value);
                dec_ref_bits(_py, best_item);
                best = Some((item, value));
            }
            CompareBoolOutcome::False => {
                dec_ref_bits(_py, item);
                dec_ref_bits(_py, value);
            }
            CompareBoolOutcome::NotComparable | CompareBoolOutcome::Error => {
                dec_ref_bits(_py, value);
                dec_ref_bits(_py, item);
                break false;
            }
        }
    };
    if !completed {
        if let Some((best_item, best_value)) = best {
            dec_ref_bits(_py, best_value);
            dec_ref_bits(_py, best_item);
        }
        return MoltObject::none().bits();
    }
    let result = match best {
        Some((best_item, best_value)) => {
            dec_ref_bits(_py, best_value);
            best_item
        }
        None if has_default => {
            inc_ref_bits(_py, default_bits);
            default_bits
        }
        None => {
            let msg = format!("{name}() iterable argument is empty");
            raise_exception::<u64>(_py, "ValueError", &msg)
        }
    };
    drop(iter);
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, CompareOp::Lt, "min")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, CompareOp::Gt, "max")
    })
}

/// Builtin `sorted()`: CPython's `PySequence_List(iterable)` and that list's
/// `sort()`, so the list constructor owns iteration and `list.sort` owns
/// ordering.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sorted_builtin(iter_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(list_bits) = (unsafe { crate::object::ops::list_from_iter_bits(_py, iter_bits) })
        else {
            return MoltObject::none().bits();
        };
        let _ = molt_list_sort(list_bits, key_bits, reverse_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, list_bits);
            return MoltObject::none().bits();
        }
        list_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sum_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(source) = sum_source(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        if let Some(message) = sum_rejected_start(_py, start_bits) {
            // The raise precedes the iterator's release, as in CPython.
            let raised = raise_exception::<u64>(_py, "TypeError", message);
            drop(source);
            return raised;
        }
        let py314 = runtime_target_at_least(_py, 3, 14);
        let sum = BuiltinSum::new(_py, start_bits, py314);
        sum_items(_py, sum, source, iter_bits, py314).unwrap_or_else(|| MoltObject::none().bits())
    })
}
