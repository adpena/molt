//! Julia-style effects analysis for TIR functions and operations.
//!
//! Classifies known builtins and methods by their effects:
//! - `consistent`: pure function of inputs (same inputs -> same output)
//! - `effect_free`: no observable side effects
//! - `nothrow`: never raises exceptions (for valid inputs within domain)
//!
//! When all three properties hold, the function is PURE and eligible for
//! concrete evaluation (constant folding at compile time) when all arguments
//! are compile-time constants.
//!
//! The concrete evaluation logic lives in `sccp.rs`, which uses the effects
//! classification from this module to gate constant folding of calls.

use crate::tir::effect_proof::tir_has_static_module_class_binding_effect_proof;
use crate::tir::ops::{OpCode, TirOp};

/// Whether `opcode` may raise — DCE must preserve it even when its result is
/// dead. EXHAUSTIVE over the `OpCode` enum: the classification lives in the
/// single-source-of-truth op-kind registry
/// (`runtime/molt-tir/src/tir/op_kinds.toml`'s per-OpCode `may_throw` column,
/// generated into [`crate::tir::op_kinds_generated`]; see
/// `docs/design/foundation/25_op_kind_registry.md`). Because the generated match
/// has no wildcard arm, a newly added `OpCode` variant fails to compile until it
/// is given an explicit effect classification in the table — the structural kill
/// for the `matches!`-default-false trap (the ModuleImportFrom lesson) that
/// silently classified a new opcode as no-throw.
#[inline]
pub(super) fn opcode_may_throw(opcode: OpCode) -> bool {
    crate::tir::op_kinds_generated::opcode_may_throw_table(opcode)
}

#[inline]
pub(super) fn op_may_throw(op: &TirOp) -> bool {
    !tir_has_static_module_class_binding_effect_proof(op) && opcode_may_throw(op.opcode)
}

/// Whether `opcode` has an observable side effect — DCE must preserve it even
/// when its result is dead. EXHAUSTIVE over the `OpCode` enum via the
/// single-source-of-truth op-kind registry
/// (`runtime/molt-tir/src/tir/op_kinds.toml`'s per-OpCode `side_effecting`
/// column, generated into [`crate::tir::op_kinds_generated`]; see
/// `docs/design/foundation/25_op_kind_registry.md`).
///
/// Safety-critical rationale preserved from the original hand-list (now encoded
/// in the table):
/// - `ExceptionPending` reads the mutable runtime exception-pending flag; it MUST
///   re-read each iteration and never be CSE'd / LICM-hoisted out of a loop or
///   eliminated when its result looks unused — else the `loop_break_if_exception`
///   it feeds could be deleted, re-opening the iterator-consumer infinite-loop /
///   OOM bug. (Hence side_effecting = true.)
/// - `ObjectNewBound` is side-effecting (a class-instance allocation may run a
///   finalizer/GC hook, like `Alloc`); `ObjectNewBoundStack` is intentionally NOT
///   (escape analysis only converts to it when the result provably does not
///   escape, and the lowered Cranelift StackSlot has no finalizer).
/// - Module reads (`ModuleCacheGet`, `ModuleGetAttr`, …) and mutations are
///   preserved even when their synthetic result is unused; a specific op instance
///   may opt out via a validated `effect_proof` (see `op_is_side_effecting`).
///
/// Because the generated match has no wildcard arm, a newly added `OpCode`
/// variant fails to compile until classified — the structural kill for the
/// `matches!`-default-false trap.
#[inline]
fn opcode_is_side_effecting(opcode: OpCode) -> bool {
    crate::tir::op_kinds_generated::opcode_is_side_effecting_table(opcode)
}

#[inline]
pub(super) fn op_is_side_effecting(op: &TirOp) -> bool {
    !tir_has_static_module_class_binding_effect_proof(op) && opcode_is_side_effecting(op.opcode)
}

#[inline]
pub(super) fn op_has_observable_effect_when_dead(op: &TirOp) -> bool {
    if op_is_side_effecting(op) || op_may_throw(op) {
        return true;
    }
    op.opcode == OpCode::Copy && op.attrs.contains_key("_original_kind")
}

// ───────────────────────────────────────────────────────────────────────────
// Unified pure-op oracle (single source of truth for movability / CSE).
//
// `opcode_is_side_effecting` / `opcode_may_throw` above answer the question DCE
// asks: *"if this op's result is dead, must I still keep it?"* — i.e. "side
// effect OR may-throw". That predicate is intentionally permissive: it leaves
// ops like `LoadAttr`, `Index`, `GetIter`, `IterNext`, `ForIter`, `ClosureLoad`
// OUT of `opcode_is_side_effecting` (DCE preserves them via their may-throw
// flag), even though they are NOT referentially transparent (`__getattr__`,
// iterator advancement, `__getitem__` dispatch can each return different values
// on repeated evaluation or mutate observable state).
//
// LICM (hoist/sink) and GVN (value-numbering / CSE) ask a STRICTER question:
// *"is this op a deterministic, side-effect-free computation?"* — a positive
// purity property that `!opcode_is_side_effecting` does NOT capture. To avoid
// hand-maintained allowlists drifting apart, this oracle is the single place
// that property is decided. LICM derives its movable predicate directly from
// it; GVN's generated numbering-role table is validated against the same
// CSE-safe purity core while retaining pass-specific roles for constants and
// type gates.
//
// The classification is the same three-axis model `FunctionEffects` already
// uses, lifted to the opcode level:
//   - `consistent`  : same operand value-numbers ⇒ same result (referentially
//                      transparent; no observation of mutable runtime state).
//   - `effect_free` : evaluating it has no observable side effect (no I/O, no
//                      store/mutation, no refcount/alloc effect, no dunder
//                      dispatch that could mutate shared state).
//   - `nothrow`     : never raises for the inputs it is applied to.
//
// PRECONDITION on the arithmetic/comparison/bitwise/boolean family (`Add`, …,
// `Div`, …): their purity holds only when operands are primitive (`int`/`float`
// /`bool`/`None`). On `DynBox` operands these ops can dispatch a user dunder
// (`__add__`, `__eq__`, …) with arbitrary side effects. By the point LICM and
// GVN run (after `unboxing` + `canonicalize_post` — see `passes::mod`), any op
// still spelled as a bare arithmetic opcode operates on lowered/typed operands;
// GVN additionally enforces the primitive-operand precondition explicitly via
// `is_primitive_type` at its call site. This oracle classifies that family as
// pure under that precondition — it does NOT relieve a caller of a type gate it
// independently needs.
/// Per-opcode purity classification — the single source of truth from which the
/// LICM (`opcode_is_pure_movable`) predicate and GVN's generated numbering-role
/// purity invariant are derived.
///
/// Only opcodes that are genuinely deterministic *computations* return a `PURE`
/// / `PURE_MAY_THROW` triple. Everything else (calls, loads, stores, iteration,
/// allocation, control flow, runtime-state reads, …) is `IMPURE` — even when it
/// is not in `opcode_is_side_effecting`, because referential transparency is a
/// strictly stronger property than "DCE may not drop it".
/// The purity classification is the single-source-of-truth op-kind registry
/// (`runtime/molt-tir/src/tir/op_kinds.toml`'s per-OpCode `purity` column,
/// generated into [`crate::tir::op_kinds_generated`]; see
/// `docs/design/foundation/25_op_kind_registry.md`). The registry maps each
/// `OpCode` to the `(consistent, effect_free, nothrow)` triple this returns:
///   - `"pure"`           => PURE — the type-gated arithmetic/comparison/bitwise/
///     boolean family (on primitive operands, see the PRECONDITION above), the
///     box/unbox transforms, `TypeGuard`, the constant materializers (incl.
///     `ConstBigInt`, effect-free like `ConstStr`), and `BuildSlice`.
///   - `"pure_may_throw"` => PURE_MAY_THROW — `Div`/`FloorDiv`/`Mod`/`Pow` (may
///     raise `ZeroDivisionError` / `0 ** -1`; CSE-safe under dominance, NOT
///     hoistable above a guard).
///   - `"impure"`         => IMPURE — everything else, INCLUDING `CheckedAdd`,
///     which is *semantically* pure but is deliberately impure for hoist/CSE
///     because it is a 2-result op that GVN/LICM have not been verified for.
///
/// EXHAUSTIVE over the enum (the generated triple has no wildcard arm), so a new
/// `OpCode` variant must be classified in the table before it compiles.
#[inline]
fn opcode_effects(opcode: OpCode) -> crate::tir::op_kinds_generated::OpcodeEffects {
    crate::tir::op_kinds_generated::opcode_effects_table(opcode)
}

/// True if `opcode` is a deterministic, side-effect-free, never-throwing
/// computation — safe to hoist out of / sink past a loop (LICM) and to sink
/// past any guard. This is the shared purity core LICM consumes.
///
/// LICM additionally permits `TirOp::is_plain_value_copy()` (a structural SSA
/// copy), which is a property of the *op instance* (its attrs/arity), not the
/// opcode, so that check stays in the pass.
#[inline]
pub(super) fn opcode_is_pure_movable(opcode: OpCode) -> bool {
    let e = opcode_effects(opcode);
    e.consistent && e.effect_free && e.nothrow
}

/// True if `opcode` is a deterministic, side-effect-free computation that may
/// nonetheless raise — safe to **value-number / CSE** but NOT to hoist above a
/// guard. CSE only ever replaces a duplicate that is *dominated* by the leader,
/// so if the leader raises, control never reaches the replaced op; the throw is
/// therefore preserved. GVN consumes generated numbering roles; test invariants
/// keep every numbered role tied to this purity core.
///
/// `opcode_is_pure_movable` ⊆ `opcode_is_cse_safe` (dropping the `nothrow`
/// requirement only ever adds ops).
#[inline]
#[cfg(test)]
pub(super) fn opcode_is_cse_safe(opcode: OpCode) -> bool {
    let e = opcode_effects(opcode);
    e.consistent && e.effect_free
}

/// True if `opcode` is a deterministic, side-effect-free computation that may
/// raise — i.e. `PURE_MAY_THROW` exactly: CSE-safe but NOT unconditionally
/// hoistable. This is the family `{Div, FloorDiv, Mod, Pow, Shl, Shr}` (divide
/// by zero / `0 ** -1` / out-of-range or negative shift count).
///
/// It is precisely `opcode_is_cse_safe && !opcode_is_pure_movable` (CSE-safe but
/// not nothrow), exposed as a named predicate so LICM can identify the ops whose
/// hoistability becomes conditional on their throw-condition being *disproven*
/// at the hoist site (e.g. a value-range proof that a shift count is in `[0, 63]`
/// or a divisor is non-zero). The opcode-level classification alone never makes
/// such an op hoistable — the caller MUST supply the per-instance throw-disproof.
#[inline]
pub(super) fn opcode_is_pure_may_throw(opcode: OpCode) -> bool {
    let e = opcode_effects(opcode);
    e.consistent && e.effect_free && !e.nothrow
}

/// Effect classification for a function or method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionEffects {
    /// Same inputs always produce the same output (referentially transparent).
    pub consistent: bool,
    /// No observable side effects (no I/O, no mutation of shared state).
    pub effect_free: bool,
    /// Never raises exceptions for valid inputs within the function's domain.
    pub nothrow: bool,
}

impl FunctionEffects {
    /// Returns `true` if the function is fully pure: consistent, effect-free,
    /// and nothrow. Only pure functions are eligible for concrete eval.
    #[inline]
    pub fn is_pure(&self) -> bool {
        self.consistent && self.effect_free && self.nothrow
    }
}

/// A fully pure function: consistent, effect-free, nothrow.
const PURE: FunctionEffects = FunctionEffects {
    consistent: true,
    effect_free: true,
    nothrow: true,
};

/// Look up effects for a known builtin function by name.
///
/// Returns `None` for unknown builtins -- the caller must treat unknown
/// functions as potentially having all effects (impure, side-effecting,
/// may-throw).
///
/// Only functions that are 100% pure for ALL valid inputs are classified
/// here. Functions that throw on invalid inputs (e.g. `int("abc")`) are
/// excluded because the nothrow property must hold unconditionally.
pub fn builtin_effects(name: &str) -> Option<FunctionEffects> {
    match name {
        // Type constructors (for literal/constant args only)
        "bool" | "int" | "float" => Some(PURE),

        // Numeric functions
        "abs" | "min" | "max" => Some(PURE),

        // Sequence functions (pure when operating on constant data)
        "len" | "sorted" | "reversed" | "sum" => Some(PURE),

        // String/repr (pure for constant primitive values)
        "str" | "repr" | "hash" => Some(PURE),

        // Character/encoding
        "chr" | "ord" | "hex" | "oct" | "bin" => Some(PURE),

        // Constructors that produce fresh immutable sequences
        "range" | "enumerate" | "zip" | "map" | "filter" | "tuple" | "frozenset" => Some(PURE),

        // Math module (pure numerical functions)
        "math.sqrt" | "math.floor" | "math.ceil" | "math.log" | "math.log2" | "math.log10"
        | "math.exp" | "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos"
        | "math.atan" | "math.atan2" | "math.fabs" | "math.pow" | "math.gcd" | "math.lcm"
        | "math.isfinite" | "math.isinf" | "math.isnan" | "math.copysign" | "math.trunc"
        | "math.hypot" => Some(PURE),

        // Explicitly NOT pure (I/O, random, time, mutation):
        // print, input, open, random.*, time.*, os.*, sys.*
        _ => None,
    }
}

/// Look up effects for a known method call on a type.
///
/// `receiver_type` is a hint about the receiver's type (e.g. "str", "list").
/// `method_name` is the method being called (e.g. "upper", "strip").
///
/// Only methods that are pure for all valid inputs are classified here.
/// Mutating methods (list.append, dict.update, etc.) are never pure.
pub fn method_effects(receiver_type: &str, method_name: &str) -> Option<FunctionEffects> {
    match (receiver_type, method_name) {
        // str methods -- strings are immutable, all these return new strings
        ("str", "upper")
        | ("str", "lower")
        | ("str", "strip")
        | ("str", "lstrip")
        | ("str", "rstrip")
        | ("str", "title")
        | ("str", "capitalize")
        | ("str", "casefold")
        | ("str", "swapcase")
        | ("str", "center")
        | ("str", "ljust")
        | ("str", "rjust")
        | ("str", "zfill")
        | ("str", "replace")
        | ("str", "join")
        | ("str", "split")
        | ("str", "rsplit")
        | ("str", "splitlines")
        | ("str", "startswith")
        | ("str", "endswith")
        | ("str", "find")
        | ("str", "rfind")
        | ("str", "index")
        | ("str", "rindex")
        | ("str", "count")
        | ("str", "isalpha")
        | ("str", "isdigit")
        | ("str", "isalnum")
        | ("str", "isspace")
        | ("str", "isupper")
        | ("str", "islower")
        | ("str", "istitle")
        | ("str", "isidentifier")
        | ("str", "isprintable")
        | ("str", "isdecimal")
        | ("str", "isnumeric")
        | ("str", "encode")
        | ("str", "expandtabs")
        | ("str", "removeprefix")
        | ("str", "removesuffix")
        | ("str", "partition")
        | ("str", "rpartition")
        | ("str", "maketrans")
        | ("str", "translate") => Some(PURE),

        // tuple methods -- tuples are immutable
        ("tuple", "count") | ("tuple", "index") => Some(PURE),

        // frozenset methods -- immutable
        ("frozenset", "union")
        | ("frozenset", "intersection")
        | ("frozenset", "difference")
        | ("frozenset", "symmetric_difference")
        | ("frozenset", "issubset")
        | ("frozenset", "issuperset")
        | ("frozenset", "isdisjoint")
        | ("frozenset", "copy") => Some(PURE),

        // int/float methods
        ("int", "bit_length")
        | ("int", "bit_count")
        | ("int", "to_bytes")
        | ("int", "conjugate")
        | ("float", "is_integer")
        | ("float", "hex")
        | ("float", "conjugate") => Some(PURE),

        // Explicitly NOT pure: list.append, list.extend, dict.update, set.add, etc.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::op_kinds_generated::{
        GvnNumberingRole, opcode_effects_table, opcode_gvn_numbering_role_table,
    };

    #[test]
    fn pure_builtins_are_pure() {
        for name in &[
            "len",
            "abs",
            "min",
            "max",
            "sum",
            "sorted",
            "bool",
            "int",
            "float",
            "str",
            "repr",
            "hash",
            "chr",
            "ord",
            "hex",
            "oct",
            "bin",
            "range",
            "enumerate",
            "zip",
            "math.sqrt",
            "math.floor",
            "math.ceil",
            "math.log",
        ] {
            let fx = builtin_effects(name).unwrap_or_else(|| panic!("{name} should have effects"));
            assert!(fx.is_pure(), "{name} should be pure");
        }
    }

    #[test]
    fn unknown_builtins_return_none() {
        assert!(builtin_effects("print").is_none());
        assert!(builtin_effects("input").is_none());
        assert!(builtin_effects("open").is_none());
        assert!(builtin_effects("random.random").is_none());
        assert!(builtin_effects("time.time").is_none());
    }

    #[test]
    fn str_methods_are_pure() {
        for method in &[
            "upper",
            "lower",
            "strip",
            "split",
            "replace",
            "startswith",
            "endswith",
            "find",
            "count",
            "join",
        ] {
            let fx = method_effects("str", method)
                .unwrap_or_else(|| panic!("str.{method} should have effects"));
            assert!(fx.is_pure(), "str.{method} should be pure");
        }
    }

    #[test]
    fn mutating_methods_are_unknown() {
        assert!(method_effects("list", "append").is_none());
        assert!(method_effects("list", "extend").is_none());
        assert!(method_effects("dict", "update").is_none());
        assert!(method_effects("set", "add").is_none());
    }

    // ── Unified pure-op oracle (S3) ────────────────────────────────────────
    // Generated pure-op oracle invariants.
    //
    // Opcode membership comes from op_kinds_generated::ALL_OPCODES. The table's
    // exhaustiveness is pinned by tests/test_gen_op_kinds.py and by rustc through
    // the generated wildcard-free matches; this module only verifies that the
    // consumer predicates preserve the generated effect lattice.

    fn all_opcodes() -> impl Iterator<Item = OpCode> {
        crate::tir::op_kinds_generated::ALL_OPCODES.iter().copied()
    }

    #[test]
    fn opcode_effects_delegates_to_generated_table() {
        for op in all_opcodes() {
            assert_eq!(
                opcode_effects(op),
                opcode_effects_table(op),
                "{op:?}: effects.rs must read the generated effect table"
            );
        }
    }

    #[test]
    fn pure_movable_matches_generated_effect_triple() {
        for op in all_opcodes() {
            let e = opcode_effects_table(op);
            let expected = e.consistent && e.effect_free && e.nothrow;
            assert_eq!(
                opcode_is_pure_movable(op),
                expected,
                "{op:?}: pure_movable must be derived from generated effects"
            );
            if expected {
                assert!(
                    !opcode_is_side_effecting(op),
                    "{op:?}: pure_movable op cannot be side-effecting"
                );
                assert!(
                    !opcode_may_throw(op),
                    "{op:?}: pure_movable op cannot be may-throw"
                );
            }
        }
    }

    #[test]
    fn cse_safe_matches_generated_effect_triple() {
        for op in all_opcodes() {
            let e = opcode_effects_table(op);
            let expected = e.consistent && e.effect_free;
            assert_eq!(
                opcode_is_cse_safe(op),
                expected,
                "{op:?}: cse_safe must be derived from generated effects"
            );
            if expected {
                assert!(
                    !opcode_is_side_effecting(op),
                    "{op:?}: CSE-safe op cannot be side-effecting"
                );
            }
        }
    }

    #[test]
    fn pure_may_throw_matches_generated_effect_triple() {
        for op in all_opcodes() {
            let e = opcode_effects_table(op);
            let expected = e.consistent && e.effect_free && !e.nothrow;
            assert_eq!(
                opcode_is_pure_may_throw(op),
                expected,
                "{op:?}: pure_may_throw must be derived from generated effects"
            );
        }
    }

    #[test]
    fn movable_implies_cse_safe() {
        for op in all_opcodes() {
            if opcode_is_pure_movable(op) {
                assert!(
                    opcode_is_cse_safe(op),
                    "{op:?}: pure_movable but not cse_safe"
                );
            }
        }
    }

    #[test]
    fn generated_gvn_numbering_roles_are_backed_by_effect_core() {
        for op in all_opcodes() {
            let role = opcode_gvn_numbering_role_table(op);
            if role != GvnNumberingRole::Never {
                assert!(
                    opcode_is_cse_safe(op),
                    "{op:?}: generated GVN numbering role is not CSE-safe"
                );
            }
        }
    }

    #[test]
    fn cse_extra_over_movable_matches_generated_pure_may_throw() {
        for op in all_opcodes() {
            assert_eq!(
                opcode_is_cse_safe(op) && !opcode_is_pure_movable(op),
                opcode_is_pure_may_throw(op),
                "{op:?}: CSE-extra-over-movable must equal generated pure_may_throw"
            );
        }
    }
}
