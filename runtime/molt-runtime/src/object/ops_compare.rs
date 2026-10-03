// Comparison operations and helpers.
// Split from ops.rs for compilation-unit size reduction.

use crate::*;
use molt_obj_model::MoltObject;
use molt_obj_model::sequence_compare::RichCompareOp;
use std::cmp::Ordering;

pub(crate) mod builtin_families;
mod sequence;

use super::ops::{is_float_extended, simd_bytes_eq, simd_find_first_mismatch};

pub(crate) fn compare_type_error(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: &str,
) -> u64 {
    let msg = format!(
        "'{}' not supported between instances of '{}' and '{}'",
        op,
        type_name(_py, lhs),
        type_name(_py, rhs),
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

#[derive(Clone, Copy)]
pub(crate) enum CompareOutcome {
    Ordered(Ordering),
    Unordered,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
pub(crate) enum CompareBoolOutcome {
    True,
    False,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
pub(crate) enum CompareValueOutcome {
    Value(u64),
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
pub(crate) enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

fn is_number(obj: MoltObject) -> bool {
    to_i64(obj).is_some() || is_float_extended(obj) || bigint_ptr_from_bits(obj.bits()).is_some()
}

fn builtin_comparison_operands(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> bool {
    [lhs, rhs].into_iter().all(|value| {
        let Some(ptr) = value.as_ptr() else {
            return true;
        };
        unsafe { crate::object::iterable::builtin_receiver(_py, ptr) }
    })
}

fn compare_numbers_outcome(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if let Some(ordering) = compare_numbers(lhs, rhs) {
        return CompareOutcome::Ordered(ordering);
    }
    if is_number(lhs) && is_number(rhs) {
        return CompareOutcome::Unordered;
    }
    CompareOutcome::NotComparable
}

unsafe fn compare_string_bytes(lhs: *mut u8, rhs: *mut u8) -> Ordering {
    unsafe {
        molt_obj_model::byte_compare::compare_bytes(
            std::slice::from_raw_parts(string_bytes(lhs), string_len(lhs)),
            std::slice::from_raw_parts(string_bytes(rhs), string_len(rhs)),
        )
    }
}

unsafe fn compare_bytes_like(lhs: *mut u8, rhs: *mut u8) -> Ordering {
    unsafe {
        molt_obj_model::byte_compare::compare_bytes(
            std::slice::from_raw_parts(bytes_data(lhs), bytes_len(lhs)),
            std::slice::from_raw_parts(bytes_data(rhs), bytes_len(rhs)),
        )
    }
}

unsafe fn compare_sequence(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
) -> CompareOutcome {
    // Three-way ordering consumers use the same value-producing sequence
    // contract as the six Python operators; only this boundary consumes truth.
    unsafe {
        match comparison_value_to_bool(
            _py,
            compare_sequence_value(_py, lhs_ptr, rhs_ptr, CompareOp::Lt),
        ) {
            CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
            CompareBoolOutcome::False => {}
            CompareBoolOutcome::Error => return CompareOutcome::Error,
            CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        }
        match comparison_value_to_bool(
            _py,
            compare_sequence_value(_py, rhs_ptr, lhs_ptr, CompareOp::Lt),
        ) {
            CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
            CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
            CompareBoolOutcome::Error => CompareOutcome::Error,
            CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        }
    }
}

fn compare_builtin_equality_value(
    py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: RichCompareOp,
) -> CompareValueOutcome {
    debug_assert!(op.is_equality());
    if !builtin_comparison_operands(py, lhs, rhs) {
        return CompareValueOutcome::NotComparable;
    }
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::Ordered(ordering) => return comparison_order_to_value(ordering, op),
        CompareOutcome::Unordered => {
            return CompareValueOutcome::Value(
                MoltObject::from_bool(op == RichCompareOp::Ne).bits(),
            );
        }
        CompareOutcome::Error => return CompareValueOutcome::Error,
        CompareOutcome::NotComparable => {}
    }
    if lhs.is_none() && rhs.is_none() {
        return comparison_order_to_value(Ordering::Equal, op);
    }
    // Even builtin descriptors may produce an arbitrary owned value (for
    // example GenericAlias delegates to the rich equality of its arguments).
    // Only explicit truth consumers may coerce that value or release it.
    compare_builtin_storage(py, lhs, rhs, op)
}

fn compare_builtin_storage(
    py: &PyToken<'_>,
    left: MoltObject,
    right: MoltObject,
    op: RichCompareOp,
) -> CompareValueOutcome {
    if let Some(family) = SequenceComparison::for_value(left) {
        return family.compare(
            py,
            left.as_ptr().expect("admitted sequence"),
            right.bits(),
            op,
        );
    }
    if let Some(family) = builtin_families::family_for_value(py, left) {
        return family.compare(py, left, right, op);
    }
    CompareValueOutcome::NotComparable
}

/// Consume an owned rich-comparison result only at a truth-valued boundary.
pub(crate) fn comparison_value_to_bool(
    _py: &PyToken<'_>,
    outcome: CompareValueOutcome,
) -> CompareBoolOutcome {
    let bits = match outcome {
        CompareValueOutcome::Value(bits) => bits,
        CompareValueOutcome::NotComparable => return CompareBoolOutcome::NotComparable,
        CompareValueOutcome::Error => return CompareBoolOutcome::Error,
    };
    let previous = exception_last_bits_noinc(_py);
    let truthy = is_truthy(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    if exception_pending(_py) && exception_last_bits_noinc(_py) != previous {
        CompareBoolOutcome::Error
    } else if truthy {
        CompareBoolOutcome::True
    } else {
        CompareBoolOutcome::False
    }
}

/// Declaring storage families for native lexicographic comparisons. Source
/// operators perform subclass/reflected dispatch; an explicit base descriptor
/// validates storage and bypasses overrides on the outer container only.
#[derive(Clone, Copy)]
pub(crate) enum SequenceComparison {
    List,
    Tuple,
    String,
    Bytes,
    Bytearray,
}

fn rich_order(op: CompareOp) -> RichCompareOp {
    match op {
        CompareOp::Lt => RichCompareOp::Lt,
        CompareOp::Le => RichCompareOp::Le,
        CompareOp::Gt => RichCompareOp::Gt,
        CompareOp::Ge => RichCompareOp::Ge,
    }
}

fn ordering_op(op: RichCompareOp) -> CompareOp {
    match op {
        RichCompareOp::Lt => CompareOp::Lt,
        RichCompareOp::Le => CompareOp::Le,
        RichCompareOp::Gt => CompareOp::Gt,
        RichCompareOp::Ge => CompareOp::Ge,
        RichCompareOp::Eq | RichCompareOp::Ne => unreachable!("ordering operation"),
    }
}

impl SequenceComparison {
    fn for_value(value: MoltObject) -> Option<Self> {
        let ptr = value.as_ptr()?;
        match unsafe { object_type_id(ptr) } {
            TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => Some(Self::List),
            TYPE_ID_TUPLE => Some(Self::Tuple),
            TYPE_ID_STRING => Some(Self::String),
            TYPE_ID_BYTES => Some(Self::Bytes),
            TYPE_ID_BYTEARRAY => Some(Self::Bytearray),
            _ => None,
        }
    }

    pub(crate) fn owner(self, py: &PyToken<'_>) -> u64 {
        let b = builtin_classes(py);
        match self {
            Self::List => b.list,
            Self::Tuple => b.tuple,
            Self::String => b.str,
            Self::Bytes => b.bytes,
            Self::Bytearray => b.bytearray,
        }
    }

    fn storage(self, bits: u64) -> Option<*mut u8> {
        if matches!(self, Self::List) {
            return crate::object::ops_list::list_storage_ptr(bits);
        }
        obj_from_bits(bits).as_ptr().filter(|&ptr| unsafe {
            object_type_id(ptr)
                == match self {
                    Self::Tuple => TYPE_ID_TUPLE,
                    Self::String => TYPE_ID_STRING,
                    Self::Bytes => TYPE_ID_BYTES,
                    Self::Bytearray => TYPE_ID_BYTEARRAY,
                    Self::List => unreachable!(),
                }
        })
    }

    pub(crate) fn invoke(self, py: &PyToken<'_>, left: u64, right: u64, op: RichCompareOp) -> u64 {
        let Some(lhs) = self.storage(left) else {
            let expected = match self {
                Self::List => "list",
                Self::Tuple => "tuple",
                Self::String => "str",
                Self::Bytes => "bytes",
                Self::Bytearray => "bytearray",
            };
            return raise_exception(
                py,
                "TypeError",
                &format!(
                    "descriptor '{}' requires a '{}' object but received a '{}'",
                    op.method_name(),
                    expected,
                    type_name(py, obj_from_bits(left)),
                ),
            );
        };
        match self.compare(py, lhs, right, op) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::NotComparable => {
                crate::builtins::methods::not_implemented_bits(py)
            }
            CompareValueOutcome::Error => MoltObject::none().bits(),
        }
    }

    // No callback may run while a mutable byte span is borrowed. Sequence
    // element comparison uses the existing pin/reacquire authority instead.
    fn compare(
        self,
        py: &PyToken<'_>,
        lhs: *mut u8,
        right: u64,
        op: RichCompareOp,
    ) -> CompareValueOutcome {
        if matches!(self, Self::Bytearray) {
            use crate::object::buffer_exports::{ScopedBuffer, supports_buffer};
            if !supports_buffer(py, MoltObject::from_ptr(lhs).bits()) || !supports_buffer(py, right)
            {
                return CompareValueOutcome::NotComparable;
            }
            let mut compared = None;
            crate::builtins::exceptions::with_saved_raised_exception(py, || {
                // Self is exported before the peer can allocate or re-enter.
                // This pins the compared span and rejects callback resizing.
                // CPython's bytearray slot explicitly clears failed simple-
                // buffer acquisition and declines that pair.
                let acquire = |bits| {
                    let buffer = ScopedBuffer::new(py, bits).ok()?;
                    let len = buffer.contiguous_len().ok()?;
                    Some((buffer, len))
                };
                let Some((left, left_len)) = acquire(MoltObject::from_ptr(lhs).bits()) else {
                    if exception_pending(py) {
                        crate::clear_exception(py);
                    }
                    return true;
                };
                let Some((right, right_len)) = acquire(right) else {
                    if exception_pending(py) {
                        crate::clear_exception(py);
                    }
                    drop(left);
                    return true;
                };
                let order = unsafe {
                    let left_bytes = if left_len == 0 {
                        &[]
                    } else {
                        std::slice::from_raw_parts(left.view().data, left_len)
                    };
                    let right_bytes = if right_len == 0 {
                        &[]
                    } else {
                        std::slice::from_raw_parts(right.view().data, right_len)
                    };
                    molt_obj_model::byte_compare::compare_bytes(left_bytes, right_bytes)
                };
                // Buffer release order is observable for exporter callbacks.
                drop(left);
                drop(right);
                compared = Some(comparison_order_to_value(order, op));
                true
            });
            return compared.unwrap_or(CompareValueOutcome::NotComparable);
        }
        let Some(rhs) = self.storage(right) else {
            return CompareValueOutcome::NotComparable;
        };
        unsafe {
            if matches!(self, Self::String | Self::Bytes) {
                let order = if matches!(self, Self::String) {
                    compare_string_bytes(lhs, rhs)
                } else {
                    compare_bytes_like(lhs, rhs)
                };
                return comparison_order_to_value(order, op);
            }
            if matches!(self, Self::List) {
                crate::object::ops_list::promote_specialized_list_to_list(py, lhs);
                if exception_pending(py) {
                    return CompareValueOutcome::Error;
                }
                crate::object::ops_list::promote_specialized_list_to_list(py, rhs);
                if exception_pending(py) {
                    return CompareValueOutcome::Error;
                }
            }
            sequence::compare(py, lhs, rhs, op)
        }
    }
}

fn comparison_order_to_value(order: Ordering, op: RichCompareOp) -> CompareValueOutcome {
    CompareValueOutcome::Value(MoltObject::from_bool(op.test(order)).bits())
}

/// Compare the retained contents of two cells. The extracted values remain
/// owned across arbitrary rich-comparison callbacks even if either callback
/// clears or replaces its source cell.
fn compare_cell_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: RichCompareOp,
) -> Option<CompareValueOutcome> {
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return None;
    };
    if unsafe { object_type_id(lhs_ptr) } != TYPE_ID_CELL
        || unsafe { object_type_id(rhs_ptr) } != TYPE_ID_CELL
    {
        return None;
    }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        _py,
        "maximum recursion depth exceeded in comparison",
    ) else {
        return Some(CompareValueOutcome::Error);
    };
    let left = unsafe { crate::object::cells::cell_value_bits(lhs_ptr) };
    let right = unsafe { crate::object::cells::cell_value_bits(rhs_ptr) };
    inc_ref_bits(_py, left);
    inc_ref_bits(_py, right);
    let left_missing = crate::builtins::methods::is_missing_bits(_py, left);
    let right_missing = crate::builtins::methods::is_missing_bits(_py, right);
    let outcome = if left_missing || right_missing {
        let ordering = match (left_missing, right_missing) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => unreachable!(),
        };
        CompareValueOutcome::Value(MoltObject::from_bool(op.test(ordering)).bits())
    } else {
        match op {
            RichCompareOp::Eq => {
                compare_object_eq_value(_py, obj_from_bits(left), obj_from_bits(right))
            }
            RichCompareOp::Ne => {
                compare_object_ne_value(_py, obj_from_bits(left), obj_from_bits(right))
            }
            RichCompareOp::Lt | RichCompareOp::Le | RichCompareOp::Gt | RichCompareOp::Ge => {
                compare_object_value_for_op(
                    _py,
                    obj_from_bits(left),
                    obj_from_bits(right),
                    ordering_op(op),
                )
            }
        }
    };
    dec_ref_bits(_py, right);
    dec_ref_bits(_py, left);
    Some(outcome)
}

/// Direct `cell` comparison methods admit only a physical cell receiver and
/// return `NotImplemented` for any non-cell peer so the generic rich-compare
/// dispatcher can try reflection or its normal fallback without re-entering
/// the same cell method.
fn cell_compare_method(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    method: &str,
    op: RichCompareOp,
) -> u64 {
    let lhs = obj_from_bits(lhs_bits);
    if crate::object::cells::cell_ptr_from_bits(lhs_bits).is_none() {
        let received = type_name(_py, lhs);
        return raise_exception::<_>(
            _py,
            "TypeError",
            &format!("descriptor '{method}' requires a 'cell' object but received a '{received}'"),
        );
    }
    if crate::object::cells::cell_ptr_from_bits(rhs_bits).is_none() {
        return crate::builtins::methods::not_implemented_bits(_py);
    }
    match compare_cell_value(_py, lhs, obj_from_bits(rhs_bits), op) {
        Some(CompareValueOutcome::Value(bits)) => bits,
        Some(CompareValueOutcome::NotComparable) | None => {
            crate::builtins::methods::not_implemented_bits(_py)
        }
        Some(CompareValueOutcome::Error) => MoltObject::none().bits(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__eq__", RichCompareOp::Eq)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__ne__", RichCompareOp::Ne)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_lt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__lt__", RichCompareOp::Lt)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_le(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__le__", RichCompareOp::Le)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_gt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__gt__", RichCompareOp::Gt)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cell_ge(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cell_compare_method(_py, a, b, "__ge__", RichCompareOp::Ge)
    })
}

fn compare_object_eq_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareValueOutcome {
    if let Some(outcome) = compare_cell_value(_py, lhs, rhs, RichCompareOp::Eq) {
        return outcome;
    }
    match compare_builtin_equality_value(_py, lhs, rhs, RichCompareOp::Eq) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let name = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
    match rich_compare_value(_py, lhs, rhs, name, name) {
        CompareValueOutcome::NotComparable => {
            CompareValueOutcome::Value(MoltObject::from_bool(lhs.bits() == rhs.bits()).bits())
        }
        outcome => outcome,
    }
}

fn compare_object_ne_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareValueOutcome {
    if let Some(outcome) = compare_cell_value(_py, lhs, rhs, RichCompareOp::Ne) {
        return outcome;
    }
    match compare_builtin_equality_value(_py, lhs, rhs, RichCompareOp::Ne) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let name = intern_static_name(_py, &runtime_state(_py).interned.ne_name, b"__ne__");
    match rich_compare_value(_py, lhs, rhs, name, name) {
        CompareValueOutcome::NotComparable => {
            CompareValueOutcome::Value(MoltObject::from_bool(lhs.bits() != rhs.bits()).bits())
        }
        outcome => outcome,
    }
}

pub(crate) fn compare_object_eq_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareBoolOutcome {
    // Container element comparison uses Python's identity-or-equality contract;
    // the value-producing equality operation deliberately has no such shortcut.
    if lhs.bits() == rhs.bits() {
        return CompareBoolOutcome::True;
    }
    comparison_value_to_bool(_py, compare_object_eq_value(_py, lhs, rhs))
}

fn compare_objects_builtin(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if !builtin_comparison_operands(_py, lhs, rhs) {
        return CompareOutcome::NotComparable;
    }
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return CompareOutcome::NotComparable;
    };
    unsafe {
        let ltype = object_type_id(lhs_ptr);
        let rtype = object_type_id(rhs_ptr);
        if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
            return CompareOutcome::Ordered(compare_string_bytes(lhs_ptr, rhs_ptr));
        }
        if (ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY)
            && (rtype == TYPE_ID_BYTES || rtype == TYPE_ID_BYTEARRAY)
        {
            return CompareOutcome::Ordered(compare_bytes_like(lhs_ptr, rhs_ptr));
        }
        if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
        if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
    }
    CompareOutcome::NotComparable
}

fn compare_op_symbol(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Lt => "<",
        CompareOp::Le => "<=",
        CompareOp::Gt => ">",
        CompareOp::Ge => ">=",
    }
}

fn compare_op_method_names(_py: &PyToken<'_>, op: CompareOp) -> (u64, u64) {
    match op {
        CompareOp::Lt => (
            intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__"),
            intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__"),
        ),
        CompareOp::Le => (
            intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__"),
            intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__"),
        ),
        CompareOp::Gt => (
            intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__"),
            intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__"),
        ),
        CompareOp::Ge => (
            intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__"),
            intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__"),
        ),
    }
}

fn compare_object_value_for_op(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareValueOutcome {
    match compare_builtin_value(_py, lhs, rhs, op) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (name, reflected) = compare_op_method_names(_py, op);
    match rich_compare_value(_py, lhs, rhs, name, reflected) {
        CompareValueOutcome::NotComparable => {
            compare_type_error(_py, lhs, rhs, compare_op_symbol(op));
            CompareValueOutcome::Error
        }
        outcome => outcome,
    }
}

/// `PyObject_RichCompareBool(lhs, rhs, op)` for an ordering operator: the
/// operator's one rich comparison, TypeError when neither operand implements
/// it, then the truth of its result. Never `NotComparable`.
pub(crate) fn rich_compare_op_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareBoolOutcome {
    comparison_value_to_bool(_py, compare_object_value_for_op(_py, lhs, rhs, op))
}

unsafe fn compare_sequence_value(
    py: &PyToken<'_>,
    left: *mut u8,
    right: *mut u8,
    op: CompareOp,
) -> CompareValueOutcome {
    unsafe { sequence::compare(py, left, right, rich_order(op)) }
}

fn compare_builtin_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareValueOutcome {
    if !builtin_comparison_operands(_py, lhs, rhs) {
        return CompareValueOutcome::NotComparable;
    }
    if let Some(outcome) = compare_cell_value(_py, lhs, rhs, rich_order(op)) {
        return outcome;
    }
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::Ordered(ordering) => comparison_order_to_value(ordering, rich_order(op)),
        CompareOutcome::Unordered => {
            CompareValueOutcome::Value(MoltObject::from_bool(false).bits())
        }
        CompareOutcome::Error => CompareValueOutcome::Error,
        CompareOutcome::NotComparable => compare_builtin_storage(_py, lhs, rhs, rich_order(op)),
    }
}

pub(crate) fn rich_compare_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareBoolOutcome {
    comparison_value_to_bool(
        _py,
        rich_compare_value(_py, lhs, rhs, op_name_bits, reverse_name_bits),
    )
}

/// Invoke one type slot, never an instance attribute. This also serves
/// object.__ne__, which delegates to its receiver's __eq__ slot, not a second
/// complete reflected comparison.
pub(crate) fn rich_compare_method_value(
    _py: &PyToken<'_>,
    receiver: MoltObject,
    other: MoltObject,
    name: u64,
) -> CompareValueOutcome {
    let Some(class) = obj_from_bits(type_of_bits(_py, receiver.bits())).as_ptr() else {
        return CompareValueOutcome::NotComparable;
    };
    let previous = exception_last_bits_noinc(_py);
    let changed = || exception_pending(_py) && exception_last_bits_noinc(_py) != previous;
    let owner = MoltObject::from_ptr(class).bits();
    inc_ref_bits(_py, owner);
    let outcome = (|| unsafe {
        let Some(raw) = class_attr_lookup_raw_mro(_py, class, name) else {
            return if changed() {
                CompareValueOutcome::Error
            } else {
                CompareValueOutcome::NotComparable
            };
        };
        let Some(result) = crate::builtins::attr::descriptor_special_call1(
            _py,
            raw,
            class,
            Some(receiver.bits()),
            other.bits(),
            crate::builtins::attr::DescriptorCallPolicy::RichComparison,
        ) else {
            return if changed() {
                CompareValueOutcome::Error
            } else {
                CompareValueOutcome::NotComparable
            };
        };
        if changed() {
            dec_ref_bits(_py, result);
            return CompareValueOutcome::Error;
        }
        if is_not_implemented_bits(_py, result) {
            dec_ref_bits(_py, result);
            CompareValueOutcome::NotComparable
        } else {
            CompareValueOutcome::Value(result)
        }
    })();
    dec_ref_bits(_py, owner);
    outcome
}

pub(crate) fn rich_compare_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareValueOutcome {
    let left_class = type_of_bits(_py, lhs.bits());
    let right_class = type_of_bits(_py, rhs.bits());
    // Rich comparisons give a strict subtype priority even when its method is
    // inherited unchanged. Arithmetic's different-implementation gate is wrong here.
    let reflected_first = left_class != right_class && issubclass_bits(right_class, left_class);
    if reflected_first {
        match rich_compare_method_value(_py, rhs, lhs, reverse_name_bits) {
            CompareValueOutcome::NotComparable => {}
            outcome => return outcome,
        }
    }
    match rich_compare_method_value(_py, lhs, rhs, op_name_bits) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    // Equal types still receive a reflected attempt after NotImplemented.
    if !reflected_first {
        return rich_compare_method_value(_py, rhs, lhs, reverse_name_bits);
    }
    CompareValueOutcome::NotComparable
}

fn rich_compare_order(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
    let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
    match rich_compare_bool(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
        CompareBoolOutcome::False => {}
        CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => return CompareOutcome::Error,
    }
    match rich_compare_bool(_py, rhs, lhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
        CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
        CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => CompareOutcome::Error,
    }
}

pub(crate) fn compare_objects(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareOutcome {
    match compare_objects_builtin(_py, lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    rich_compare_order(_py, lhs, rhs)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Lt) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_le(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Le) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Gt) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ge(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Ge) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_eq_value(_py, obj_from_bits(a), obj_from_bits(b)) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_ne_value(_py, obj_from_bits(a), obj_from_bits(b)) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

/// Compare physical Unicode contents without hashing or Python callbacks.
/// Identifier classification and sealed layout maps admit str subclasses but
/// must never invoke their rich equality. Real dictionary keys use rich lookup.
///
/// # Safety
/// Both handles, when pointers, must remain live under the caller's Python token.
#[inline]
pub(crate) unsafe fn string_storage_equal(left_bits: u64, right_bits: u64) -> bool {
    unsafe {
        let (Some(left), Some(right)) = (
            obj_from_bits(left_bits).as_ptr(),
            obj_from_bits(right_bits).as_ptr(),
        ) else {
            return false;
        };
        if object_type_id(left) != TYPE_ID_STRING || object_type_id(right) != TYPE_ID_STRING {
            return false;
        }
        let len = string_len(left);
        string_len(right) == len
            && (left == right || simd_bytes_eq(string_bytes(left), string_bytes(right), len))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(unsafe { string_storage_equal(a, b) }).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(a == b).bits() })
}

#[cfg(test)]
mod sequence_tests;
