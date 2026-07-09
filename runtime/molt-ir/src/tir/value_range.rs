use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::numeric_facts::{INLINE_INT47_HI, IntRange};
use crate::tir::values::ValueId;
/// A known container length: a compile-time constant or "same SSA value as".
#[derive(Debug, Clone, PartialEq, Eq)]
enum KnownLength {
    Constant(i64),
    SameAs(ValueId),
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Per-function integer value-range facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueRangeResult {
    /// Loop-invariant range that holds for a value *everywhere in the function*
    /// (constants) or *everywhere in its loop body* (induction variables).
    global_range: HashMap<ValueId, IntRange>,
    /// Per-(block, value) narrowed range from edge-sensitive guards. A query
    /// at block `b` for value `v` first consults this, then `global_range`.
    block_range: HashMap<(BlockId, ValueId), IntRange>,
    /// container value → known length.
    container_length: HashMap<ValueId, KnownLength>,
    /// `len(c)` result value → the container `c` (for `i < len(c)` proofs).
    len_of: HashMap<ValueId, ValueId>,
    /// constant-int values (for length/bound comparison).
    const_int: HashMap<ValueId, i64>,
    /// Edge-sensitive symbolic upper bound: at block `bid`, value `var` is
    /// provably `< bound` (an SSA value). Recorded from header guards
    /// `Lt(var, bound)` and used for the `index < len(container)` symbolic
    /// proof when the numeric length is not a constant.
    symbolic_lt_bound: HashMap<(BlockId, ValueId), ValueId>,
    /// Transparent-copy resolution: value → canonical source through plain SSA
    /// copies (`is_plain_value_copy`). Lowering threads the IV / length / index
    /// through copies; query methods resolve to the canonical value so a fact
    /// recorded on the source is found when querying any copy of it (and vice
    /// versa). A plain copy is the identity, so this is exact, not lossy.
    copy_src: HashMap<ValueId, ValueId>,
}

impl ValueRangeResult {
    /// Follow plain-copy edges to the canonical source of `v` (bounded walk).
    pub fn resolve(&self, mut v: ValueId) -> ValueId {
        for _ in 0..64 {
            match self.copy_src.get(&v) {
                Some(&src) if src != v => v = src,
                _ => break,
            }
        }
        v
    }

    /// Record a transparent copy edge. Query methods resolve through these
    /// edges so producers can record a fact on the canonical value and consumers
    /// can ask about any equivalent copy.
    pub fn record_copy_source(&mut self, value: ValueId, source: ValueId) {
        self.copy_src.insert(value, source);
    }

    /// Record a known integer constant. Returns `true` when this value gained a
    /// new constant fact.
    pub fn record_const_int(&mut self, value: ValueId, constant: i64) -> bool {
        let value = self.resolve(value);
        self.const_int.insert(value, constant).is_none()
    }

    /// Return the known integer constant for `value`, after copy resolution.
    pub fn const_int_of(&self, value: ValueId) -> Option<i64> {
        self.const_int.get(&self.resolve(value)).copied()
    }

    /// Iterate known integer constants for analysis seeding.
    pub fn const_int_facts(&self) -> impl Iterator<Item = (ValueId, i64)> + '_ {
        self.const_int
            .iter()
            .map(|(&value, &constant)| (value, constant))
    }

    /// Record a loop-invariant/global range for a canonical value.
    pub fn record_global_range(&mut self, value: ValueId, range: IntRange) {
        let value = self.resolve(value);
        self.global_range.insert(value, range);
    }

    /// Meet `range` into the existing global range for `value`.
    pub fn meet_global_range(&mut self, value: ValueId, range: IntRange) {
        let value = self.resolve(value);
        let existing = self
            .global_range
            .get(&value)
            .copied()
            .unwrap_or(IntRange::FULL_I64);
        self.global_range.insert(value, existing.meet(range));
    }

    /// Whether a global range fact exists for `value`, after copy resolution.
    pub fn has_global_range(&self, value: ValueId) -> bool {
        self.global_range.contains_key(&self.resolve(value))
    }

    /// The global range fact for `value`, after copy resolution.
    pub fn global_range_of(&self, value: ValueId) -> Option<IntRange> {
        self.global_range.get(&self.resolve(value)).copied()
    }

    /// Iterate global ranges for diagnostics.
    pub fn global_ranges(&self) -> impl Iterator<Item = (ValueId, IntRange)> + '_ {
        self.global_range
            .iter()
            .map(|(&value, &range)| (value, range))
    }

    /// Meet `range` into the edge-sensitive range for `(bid, value)`.
    pub fn meet_block_range(&mut self, bid: BlockId, value: ValueId, range: IntRange) {
        let value = self.resolve(value);
        let existing = self
            .block_range
            .get(&(bid, value))
            .copied()
            .or_else(|| self.global_range.get(&value).copied())
            .unwrap_or(IntRange::FULL_I64);
        self.block_range.insert((bid, value), existing.meet(range));
    }

    /// Record a compile-time-known container length.
    pub fn record_container_length_constant(&mut self, container: ValueId, len: i64) {
        let container = self.resolve(container);
        self.container_length
            .insert(container, KnownLength::Constant(len));
    }

    /// Record a container length equal to another SSA value.
    pub fn record_container_length_same_as(&mut self, container: ValueId, len_value: ValueId) {
        let container = self.resolve(container);
        let len_value = self.resolve(len_value);
        self.container_length
            .insert(container, KnownLength::SameAs(len_value));
    }

    /// True when the container's known length is exactly `len`.
    pub fn container_length_is_constant(&self, container: ValueId, len: i64) -> bool {
        self.container_length
            .get(&self.resolve(container))
            .is_some_and(|length| matches!(length, KnownLength::Constant(value) if *value == len))
    }

    /// Record that a `len(container)` result value came from `container`.
    pub fn record_len_of(&mut self, len_value: ValueId, container: ValueId) {
        let len_value = self.resolve(len_value);
        let container = self.resolve(container);
        self.len_of.insert(len_value, container);
    }

    /// The proven range of `v` at block `bid`: the guard-narrowed range if one
    /// exists, else the global (loop-invariant / constant) range, else
    /// `FULL_I64` (unknown). Resolves `v` through plain copies first.
    pub fn range_at(&self, bid: BlockId, v: ValueId) -> IntRange {
        let v = self.resolve(v);
        if let Some(r) = self.block_range.get(&(bid, v)) {
            return *r;
        }
        self.global_range
            .get(&v)
            .copied()
            .unwrap_or(IntRange::FULL_I64)
    }

    /// The proven loop-invariant / constant range of `v` (ignoring per-block
    /// guard narrowing). `FULL_I64` if unknown.
    pub fn range_of(&self, v: ValueId) -> IntRange {
        let v = self.resolve(v);
        self.global_range
            .get(&v)
            .copied()
            .unwrap_or(IntRange::FULL_I64)
    }

    /// CONSERVATIVELY prove `0 <= index < len(container)` for an `Index` /
    /// `StoreIndex` at block `bid`. Returns `true` only when both bounds are
    /// provable; any uncertainty returns `false` (the bounds check stays).
    ///
    /// This is the BCE memory-safety query. A false positive is a silent
    /// out-of-bounds access, so every path that does not *prove* safety must
    /// fall through to `false`.
    pub fn proves_index_in_bounds(&self, bid: BlockId, container: ValueId, index: ValueId) -> bool {
        let container = self.resolve(container);
        // `range_at` resolves the index itself.
        let idx_range = self.range_at(bid, index);

        // Lower bound: index >= 0. A negative index needs Python wraparound, so
        // it is never bce_safe here.
        if !idx_range.is_non_negative() {
            return false;
        }

        // Upper bound: index < len(container). We need a known upper bound on
        // the index AND a known length.
        let idx_hi = idx_range.hi;
        if idx_hi == i64::MAX {
            // Unbounded above → cannot prove.
            return false;
        }

        match self.container_length.get(&container) {
            Some(KnownLength::Constant(len)) => {
                // index <= idx_hi < len  ⟺  idx_hi < len.
                idx_hi < *len
            }
            Some(KnownLength::SameAs(len_val)) => {
                // The length equals SSA value `len_val`. Prove `idx_hi < len_val`
                // only when `len_val` has a known constant value `> idx_hi`.
                if let Some(len_lo) = self.const_int.get(&self.resolve(*len_val)) {
                    return idx_hi < *len_lo;
                }
                // Otherwise the numeric bound is unprovable here; the symbolic
                // `index < len(container)` path discharges it instead.
                false
            }
            None => false,
        }
    }

    /// True if a guard at `bid` proves `index < len(container)` *symbolically*,
    /// i.e. the index is guarded `Lt(index, b)` where `b == len(container)`
    /// (the post-`iter_devirt` `while i < len(lst)` shape). Combined with the
    /// numeric `index >= 0` proof, this discharges the bound when the numeric
    /// length is not a constant.
    pub fn proves_index_lt_len_symbolically(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let container = self.resolve(container);
        let index = self.resolve(index);
        // index must be provably >= 0 at bid.
        if !self.range_at(bid, index).is_non_negative() {
            return false;
        }
        // Look for a recorded symbolic bound `index < bound_val` where
        // `bound_val == len(container)`.
        if let Some(&bound_val) = self.symbolic_lt_bound.get(&(bid, index)) {
            let bound_val = self.resolve(bound_val);
            if let Some(&bound_container) = self.len_of.get(&bound_val) {
                return self.resolve(bound_container) == container;
            }
        }
        false
    }

    /// BCE-only index safety query. It is strictly narrower than the raw-int
    /// carrier proof: even when the ordinary numeric or symbolic bound proves
    /// the access safe, the index must also fit the inline-int47 window. A
    /// full-range checked-overflow carrier can therefore never become `bce_safe`
    /// by sharing representation facts.
    pub fn proves_index_in_bounds_conservatively(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let proven = self.proves_index_in_bounds(bid, container, index)
            || self.proves_index_lt_len_symbolically(bid, container, index);
        proven
            && (self.range_at(bid, index).fits_inline_int47()
                || self.symbolic_index_bound_fits_inline_window(bid, container, index))
    }

    /// CONSERVATIVELY prove `v`'s entire proven range fits the signed 47-bit
    /// inline window. Unknown range ⇒ `false`.
    pub fn fits_inline_int47(&self, v: ValueId) -> bool {
        match self.global_range.get(&self.resolve(v)) {
            Some(r) => r.fits_inline_int47(),
            None => false,
        }
    }

    fn symbolic_index_bound_fits_inline_window(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let container = self.resolve(container);
        let index = self.resolve(index);
        let Some(&bound_val) = self.symbolic_lt_bound.get(&(bid, index)) else {
            return false;
        };
        let bound_range = self.range_at(bid, bound_val);
        if bound_range.hi <= INLINE_INT47_HI.saturating_add(1) {
            return true;
        }
        let bound_val = self.resolve(bound_val);
        if self
            .len_of
            .get(&bound_val)
            .is_none_or(|bound_container| self.resolve(*bound_container) != container)
        {
            return false;
        }
        match self.container_length.get(&container) {
            Some(KnownLength::Constant(len)) => *len <= INLINE_INT47_HI.saturating_add(1),
            Some(KnownLength::SameAs(len_val)) => {
                self.range_at(bid, *len_val).hi <= INLINE_INT47_HI.saturating_add(1)
            }
            None => false,
        }
    }

    /// Record the edge-sensitive symbolic fact `var < bound` at block `bid`.
    /// `var` and `bound` are stored as their canonical (resolved) sources.
    pub fn record_symbolic_lt(&mut self, bid: BlockId, var: ValueId, bound: ValueId) {
        let var = self.resolve(var);
        let bound = self.resolve(bound);
        self.symbolic_lt_bound.insert((bid, var), bound);
    }

    /// Test-only: directly seed the global range of `v` to `[lo, hi]`. Used by
    /// sibling-pass unit tests (e.g. LICM's throw-disproof gate) that need to
    /// exercise range-dependent logic against a hand-built result without
    /// standing up a full TIR function + the analysis pipeline. The
    /// `global_range` field is private, so this is the sanctioned cross-module
    /// test seam.
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_global_range_for_test(&mut self, v: ValueId, lo: i64, hi: i64) {
        let v = self.resolve(v);
        self.global_range.insert(v, IntRange::new(lo, hi));
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_methods_store_facts_on_resolved_values() {
        let bid = BlockId(7);
        let source = ValueId(10);
        let copy = ValueId(11);
        let container = ValueId(20);
        let container_copy = ValueId(21);
        let len = ValueId(30);
        let len_copy = ValueId(31);
        let index = ValueId(40);
        let index_copy = ValueId(41);

        let mut result = ValueRangeResult::default();
        result.record_copy_source(copy, source);
        result.record_copy_source(container_copy, container);
        result.record_copy_source(len_copy, len);
        result.record_copy_source(index_copy, index);

        assert!(result.record_const_int(copy, 7));
        assert_eq!(result.const_int_of(source), Some(7));
        assert_eq!(result.const_int_of(copy), Some(7));

        result.record_global_range(index_copy, IntRange::new(0, 8));
        result.meet_block_range(bid, index_copy, IntRange::new(0, 2));
        assert_eq!(result.range_of(index), IntRange::new(0, 8));
        assert_eq!(result.range_at(bid, index), IntRange::new(0, 2));
        assert_eq!(result.range_at(bid, index_copy), IntRange::new(0, 2));

        result.record_container_length_same_as(container_copy, len_copy);
        assert!(result.record_const_int(len_copy, 3));
        assert!(result.proves_index_in_bounds(bid, container, index));

        result.record_len_of(len_copy, container_copy);
        result.record_symbolic_lt(bid, index_copy, len_copy);
        assert!(result.proves_index_lt_len_symbolically(bid, container, index));
    }
}
