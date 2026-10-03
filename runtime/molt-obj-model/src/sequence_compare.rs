//! One lexicographic sequence contract for runtime storage and native C layouts.
//!
//! Consumers admit the declaring family and own storage access, references,
//! callbacks, and error transport. This module alone owns the comparison loop.

use std::cmp::Ordering;

/// CPython's six rich-comparison operation ordinals.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RichCompareOp {
    Lt = 0,
    Le = 1,
    Eq = 2,
    Ne = 3,
    Gt = 4,
    Ge = 5,
}

impl RichCompareOp {
    pub const fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Lt),
            1 => Some(Self::Le),
            2 => Some(Self::Eq),
            3 => Some(Self::Ne),
            4 => Some(Self::Gt),
            5 => Some(Self::Ge),
            _ => None,
        }
    }

    pub const fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }

    pub const fn reversed(self) -> Self {
        match self {
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Eq => Self::Eq,
            Self::Ne => Self::Ne,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
        }
    }

    pub const fn method_name(self) -> &'static str {
        match self {
            Self::Lt => "__lt__",
            Self::Le => "__le__",
            Self::Eq => "__eq__",
            Self::Ne => "__ne__",
            Self::Gt => "__gt__",
            Self::Ge => "__ge__",
        }
    }

    pub fn test(self, order: Ordering) -> bool {
        match self {
            Self::Lt => order == Ordering::Less,
            Self::Le => order != Ordering::Greater,
            Self::Eq => order == Ordering::Equal,
            Self::Ne => order != Ordering::Equal,
            Self::Gt => order == Ordering::Greater,
            Self::Ge => order != Ordering::Less,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceKind {
    List,
    Tuple,
}

/// Storage and callback custody for one admitted pair of sequences.
///
/// An item pins its value across callbacks; a tuple may borrow from its owned
/// immutable container instead. Item destruction must preserve an active error.
/// Mutable storage must never remain borrowed while `equal` or `order` runs.
pub trait SequenceCompareContext {
    type Item;
    type Value;
    type Error;

    fn lengths(&self) -> Result<(usize, usize), Self::Error>;

    /// Pin both current occupants. `None` means a list changed between length
    /// observation and pinning; the loop rereads its lengths and tries again.
    /// Missing/uninitialized tuple storage and callback errors are `Err`.
    fn pin_pair(&self, index: usize)
        -> Result<Option<(Self::Item, Self::Item)>, Self::Error>;

    /// Identity-or-rich-equality, consuming truth only at this element boundary.
    fn equal(&self, left: &Self::Item, right: &Self::Item) -> Result<bool, Self::Error>;

    /// Return the owned arbitrary rich-comparison result without truth coercion.
    /// The kernel only passes one of the four ordering operations here.
    fn order(
        &self,
        left: &Self::Item,
        right: &Self::Item,
        op: RichCompareOp,
    ) -> Result<Self::Value, Self::Error>;

    /// Produce a boolean result without invoking Python callbacks.
    fn boolean(&self, value: bool) -> Self::Value;

    /// Optional identity-equal prefix for immutable tuples, bounded by their
    /// common length. The runtime uses its existing SIMD identity scan here.
    fn equal_prefix(&self) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

/// CPython's list/tuple lexicographic contract, independent of storage layout.
///
/// Lists alone skip element callbacks for unequal initial lengths under Eq/Ne.
/// After a list equality callback both pins are released before lengths and the
/// current differing pair are reacquired. Tuples retain the immutable pair for
/// ordering, and never skip required equality callbacks just because sizes differ.
pub fn compare_sequences<C: SequenceCompareContext>(
    context: &C,
    kind: SequenceKind,
    op: RichCompareOp,
) -> Result<C::Value, C::Error> {
    let initial_lengths = context.lengths()?;
    if kind == SequenceKind::List
        && op.is_equality()
        && initial_lengths.0 != initial_lengths.1
    {
        return Ok(context.boolean(op == RichCompareOp::Ne));
    }
    let mut index = if kind == SequenceKind::Tuple {
        context.equal_prefix()?
    } else {
        0
    };
    loop {
        let (left_len, right_len) = if kind == SequenceKind::Tuple {
            initial_lengths
        } else {
            context.lengths()?
        };
        if index >= left_len.min(right_len) {
            return Ok(context.boolean(op.test(left_len.cmp(&right_len))));
        }
        let Some((left, right)) = context.pin_pair(index)? else {
            continue;
        };
        let equal = context.equal(&left, &right);
        if kind == SequenceKind::Tuple {
            let result = match equal {
                Err(error) => Some(Err(error)),
                Ok(true) => None,
                Ok(false) if op.is_equality() => {
                    Some(Ok(context.boolean(op == RichCompareOp::Ne)))
                }
                Ok(false) => Some(context.order(&left, &right, op)),
            };
            drop(left);
            drop(right);
            if let Some(result) = result {
                return result;
            }
            index += 1;
            continue;
        }

        // Releasing an item can itself run a destructor that mutates a list.
        // Match the declaring slot's left-then-right release order and preserve
        // the equality error while these edges are retired.
        drop(left);
        drop(right);
        if equal? {
            index += 1;
            continue;
        }
        let (left_len, right_len) = context.lengths()?;
        if index >= left_len.min(right_len) {
            return Ok(context.boolean(op.test(left_len.cmp(&right_len))));
        }
        if op.is_equality() {
            return Ok(context.boolean(op == RichCompareOp::Ne));
        }
        let Some((left, right)) = context.pin_pair(index)? else {
            continue;
        };
        let result = context.order(&left, &right, op);
        drop(left);
        drop(right);
        return result;
    }
}
