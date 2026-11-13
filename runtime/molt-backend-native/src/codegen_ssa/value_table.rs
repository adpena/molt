//! The lowered value of every TIR SSA value in one function, with its type.
//!
//! One dense slot per `ValueId` (TIR numbers values `0..next_value`), so a
//! lookup is an index, not a hash. The backend value and its `TirType` are one
//! slot: a value's type is a fact about that value, recorded where the value is
//! produced, never a second map that can disagree with the first.

use crate::tir::types::TirType;
use crate::tir::values::ValueId;

#[derive(Clone)]
struct Slot<V> {
    value: Option<V>,
    ty: Option<TirType>,
}

impl<V> Default for Slot<V> {
    fn default() -> Self {
        Self {
            value: None,
            ty: None,
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

    /// Record the type the lowered value of `id` carries.
    pub(crate) fn set_type(&mut self, id: ValueId, ty: TirType) {
        self.slot_mut(id).ty = Some(ty);
    }

    /// The lowered value of `id`, if the walk has produced it.
    pub(crate) fn value(&self, id: ValueId) -> Option<V> {
        self.slots.get(id.0 as usize).and_then(|slot| slot.value)
    }

    /// The type recorded for `id`, if any.
    pub(crate) fn ty(&self, id: ValueId) -> Option<&TirType> {
        self.slots
            .get(id.0 as usize)
            .and_then(|slot| slot.ty.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_and_its_type_are_one_slot_and_absence_is_explicit() {
        let mut table: ValueTable<u64> = ValueTable::with_capacity(2);
        assert_eq!(table.value(ValueId(0)), None);
        assert_eq!(table.ty(ValueId(0)), None);
        table.set_value(ValueId(1), 7);
        table.set_type(ValueId(1), TirType::I64);
        assert_eq!(table.value(ValueId(1)), Some(7));
        assert_eq!(table.ty(ValueId(1)), Some(&TirType::I64));
        table.set_value(ValueId(0), 3);
        assert_eq!(table.value(ValueId(0)), Some(3));
        assert_eq!(table.ty(ValueId(0)), None);
        // Absent ids past the end read as absent, never as a default type.
        assert_eq!(table.ty(ValueId(40)), None);
    }

    #[test]
    #[should_panic(expected = "outside the function's value numbering")]
    fn a_value_past_the_counter_is_refused() {
        let mut table: ValueTable<u64> = ValueTable::with_capacity(2);
        table.set_value(ValueId(2), 1);
    }
}
