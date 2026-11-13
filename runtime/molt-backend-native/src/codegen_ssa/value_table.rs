//! The lowered value of every TIR SSA value in one function, with the carrier
//! the backend holds it in.
//!
//! One dense slot per `ValueId` (TIR numbers values `0..next_value`), so a
//! lookup is an index, not a hash. Three facts, three owners:
//! - A value's *type* is TIR's proof; `TirFunction::value_types` owns it and
//!   this table does not store it. Storing it here let a boxed transport
//!   overwrite a proven type (measured 2026-11-13: 16% of the backend's type
//!   records replaced a TIR type with `DynBox`).
//! - The *planned carrier* is `molt_ir::repr::Repr`, the representation plan's
//!   proof-level lattice of what a value may be held as (`repr_facts`).
//! - The *held carrier* (`HeldCarrier`, here) is what the backend actually
//!   holds now: an instruction-selection fact. Measured 2026-11-13 on
//!   bench_math_numeric, bench_calls and fannkuch: 98.6% of values are held
//!   in their planned carrier; the backend holds 1,782 planned-boxed ints and
//!   1,188 planned-boxed comparison results raw, and 268 planned-raw values
//!   boxed. Those deviations are open work (docs/design/foundation/
//!   codegen_walker.md, W1b): each converges on the plan or moves into it.

use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// The machine carrier the backend holds a lowered value in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeldCarrier {
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

impl HeldCarrier {
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
    repr: Option<HeldCarrier>,
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
    pub(crate) fn set_repr(&mut self, id: ValueId, repr: HeldCarrier) {
        self.slot_mut(id).repr = Some(repr);
    }

    /// The lowered value of `id`, if the walk has produced it.
    pub(crate) fn value(&self, id: ValueId) -> Option<V> {
        self.slots.get(id.0 as usize).and_then(|slot| slot.value)
    }

    /// The representation recorded for `id`, if any.
    pub(crate) fn repr(&self, id: ValueId) -> Option<HeldCarrier> {
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
        table.set_repr(ValueId(1), HeldCarrier::RawI64);
        assert_eq!(table.value(ValueId(1)), Some(7));
        assert_eq!(table.repr(ValueId(1)), Some(HeldCarrier::RawI64));
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
        assert_eq!(HeldCarrier::of(&TirType::I64), HeldCarrier::RawI64);
        assert_eq!(HeldCarrier::of(&TirType::F64), HeldCarrier::RawF64);
        assert_eq!(HeldCarrier::of(&TirType::Bool), HeldCarrier::RawBool);
        assert_eq!(HeldCarrier::of(&TirType::Never), HeldCarrier::Never);
        for boxed in [
            TirType::DynBox,
            TirType::None,
            TirType::BigInt,
            TirType::Str,
            TirType::Box(Box::new(TirType::I64)),
        ] {
            assert_eq!(HeldCarrier::of(&boxed), HeldCarrier::Boxed, "{boxed:?}");
            assert_eq!(HeldCarrier::of(&boxed).machine_type(), TirType::DynBox);
        }
    }

    #[test]
    #[should_panic(expected = "outside the function's value numbering")]
    fn a_value_past_the_counter_is_refused() {
        let mut table: ValueTable<u64> = ValueTable::with_capacity(2);
        table.set_value(ValueId(2), 1);
    }
}
