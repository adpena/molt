//! The lowered value of every TIR SSA value in one function, with the machine
//! representation the backend chose for it.
//!
//! One dense slot per `ValueId` (TIR numbers values `0..next_value`), so a
//! lookup is an index, not a hash. A value's *type* is not stored here: TIR
//! proved it, and `TirFunction::value_types` is its one authority. The table
//! records only what the backend decides, the representation: a raw machine
//! word or a boxed word. Storing the type here as well let a boxed transport
//! overwrite a proven type (measured 2026-11-13: 16% of the backend's type
//! records replaced a TIR type with `DynBox`; docs/design/foundation/
//! codegen_walker.md, W1b).

use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// How a lowered value is held at the machine level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Repr {
    /// A raw 64-bit integer.
    RawI64,
    /// A raw 64-bit float.
    RawF64,
    /// A raw one-bit boolean.
    RawBool,
    /// The bottom type's placeholder; no value reaches it.
    Never,
    /// A NaN-boxed word: every object, `None`, and any value not held raw.
    Boxed,
}

impl Repr {
    /// The representation a value of `ty` has when the backend holds it as
    /// that type: raw for `I64`, `F64` and `Bool`, boxed for everything else
    /// (the machine mapping of `llvm_backend::types::lower_type`).
    pub(crate) fn of(ty: &TirType) -> Self {
        match ty {
            TirType::I64 => Self::RawI64,
            TirType::F64 => Self::RawF64,
            TirType::Bool => Self::RawBool,
            TirType::Never => Self::Never,
            _ => Self::Boxed,
        }
    }

    /// The type that names this representation where a lowering helper
    /// takes a type for a machine-level choice: `DynBox` for a boxed word.
    pub(crate) fn machine_type(self) -> TirType {
        match self {
            Self::RawI64 => TirType::I64,
            Self::RawF64 => TirType::F64,
            Self::RawBool => TirType::Bool,
            Self::Never => TirType::Never,
            Self::Boxed => TirType::DynBox,
        }
    }
}

#[derive(Clone)]
struct Slot<V> {
    value: Option<V>,
    repr: Option<Repr>,
}

impl<V> Default for Slot<V> {
    fn default() -> Self {
        Self {
            value: None,
            repr: None,
        }
    }
}

/// Lowered values of one function, indexed by `ValueId`.
pub(crate) struct ValueTable<V> {
    slots: Vec<Slot<V>>,
}

impl<V: Copy> ValueTable<V> {
    /// A table for a function whose values are numbered below `next_value`.
    pub(crate) fn with_capacity(next_value: u32) -> Self {
        let mut slots = Vec::new();
        slots.resize_with(next_value as usize, Slot::default);
        Self { slots }
    }

    fn slot_mut(&mut self, id: ValueId) -> &mut Slot<V> {
        let len = self.slots.len();
        // A value past the function's counter means a pass created it without
        // bumping `next_value`, so two values could share an id: refuse.
        self.slots.get_mut(id.0 as usize).unwrap_or_else(|| {
            panic!(
                "value %{} is outside the function's value numbering (next_value {len})",
                id.0
            )
        })
    }

    /// Record the lowered value of `id`.
    pub(crate) fn set_value(&mut self, id: ValueId, value: V) {
        self.slot_mut(id).value = Some(value);
    }

    /// Record how the lowered value of `id` is held.
    pub(crate) fn set_repr(&mut self, id: ValueId, repr: Repr) {
        self.slot_mut(id).repr = Some(repr);
    }

    /// The lowered value of `id`, if the walk has produced it.
    pub(crate) fn value(&self, id: ValueId) -> Option<V> {
        self.slots.get(id.0 as usize).and_then(|slot| slot.value)
    }

    /// The representation recorded for `id`, if any.
    pub(crate) fn repr(&self, id: ValueId) -> Option<Repr> {
        self.slots.get(id.0 as usize).and_then(|slot| slot.repr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_and_its_representation_are_one_slot_and_absence_is_explicit() {
        let mut table: ValueTable<u64> = ValueTable::with_capacity(2);
        assert_eq!(table.value(ValueId(0)), None);
        assert_eq!(table.repr(ValueId(0)), None);
        table.set_value(ValueId(1), 7);
        table.set_repr(ValueId(1), Repr::RawI64);
        assert_eq!(table.value(ValueId(1)), Some(7));
        assert_eq!(table.repr(ValueId(1)), Some(Repr::RawI64));
        table.set_value(ValueId(0), 3);
        assert_eq!(table.value(ValueId(0)), Some(3));
        assert_eq!(table.repr(ValueId(0)), None);
        // Absent ids past the end read as absent, never as a default.
        assert_eq!(table.repr(ValueId(40)), None);
    }

    #[test]
    fn every_type_maps_to_the_machine_representation_lower_type_gives_it() {
        // Raw only for the three scalar types; every object, None, a boxed
        // int and a union travel as one boxed word.
        assert_eq!(Repr::of(&TirType::I64), Repr::RawI64);
        assert_eq!(Repr::of(&TirType::F64), Repr::RawF64);
        assert_eq!(Repr::of(&TirType::Bool), Repr::RawBool);
        assert_eq!(Repr::of(&TirType::Never), Repr::Never);
        for boxed in [
            TirType::DynBox,
            TirType::None,
            TirType::BigInt,
            TirType::Str,
            TirType::Box(Box::new(TirType::I64)),
        ] {
            assert_eq!(Repr::of(&boxed), Repr::Boxed, "{boxed:?}");
            assert_eq!(Repr::of(&boxed).machine_type(), TirType::DynBox);
        }
    }

    #[test]
    #[should_panic(expected = "outside the function's value numbering")]
    fn a_value_past_the_counter_is_refused() {
        let mut table: ValueTable<u64> = ValueTable::with_capacity(2);
        table.set_value(ValueId(2), 1);
    }
}
