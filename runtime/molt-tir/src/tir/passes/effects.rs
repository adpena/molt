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
/// (`runtime/molt-backend/src/tir/op_kinds.toml`'s per-OpCode `may_throw` column,
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
/// (`runtime/molt-backend/src/tir/op_kinds.toml`'s per-OpCode `side_effecting`
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
// three hand-maintained allowlists drifting apart, this oracle is the single
// place that property is decided. LICM and GVN derive their predicates from it.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OpEffects {
    pub consistent: bool,
    pub effect_free: bool,
    pub nothrow: bool,
}

impl OpEffects {
    const PURE: OpEffects = OpEffects {
        consistent: true,
        effect_free: true,
        nothrow: true,
    };
    /// Deterministic + side-effect-free, but may raise (e.g. `ZeroDivisionError`
    /// for integer division). Safe to CSE (a dominating duplicate guards the
    /// throw) but NOT safe to hoist above a guard.
    const PURE_MAY_THROW: OpEffects = OpEffects {
        consistent: true,
        effect_free: true,
        nothrow: false,
    };
    /// Not classified as a pure computation by this oracle.
    const IMPURE: OpEffects = OpEffects {
        consistent: false,
        effect_free: false,
        nothrow: false,
    };
}

/// Per-opcode purity classification — the single source of truth from which the
/// LICM (`opcode_is_pure_movable`) and GVN (`opcode_is_type_gated_numberable`)
/// predicates are derived.
///
/// Only opcodes that are genuinely deterministic *computations* return a `PURE`
/// / `PURE_MAY_THROW` triple. Everything else (calls, loads, stores, iteration,
/// allocation, control flow, runtime-state reads, …) is `IMPURE` — even when it
/// is not in `opcode_is_side_effecting`, because referential transparency is a
/// strictly stronger property than "DCE may not drop it".
/// The purity classification is the single-source-of-truth op-kind registry
/// (`runtime/molt-backend/src/tir/op_kinds.toml`'s per-OpCode `purity` column,
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
fn opcode_effects(opcode: OpCode) -> OpEffects {
    use crate::tir::op_kinds_generated::OpcodePurity;
    match crate::tir::op_kinds_generated::opcode_purity_table(opcode) {
        OpcodePurity::Pure => OpEffects::PURE,
        OpcodePurity::PureMayThrow => OpEffects::PURE_MAY_THROW,
        OpcodePurity::Impure => OpEffects::IMPURE,
    }
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
/// therefore preserved. This is the shared purity core GVN consumes.
///
/// `opcode_is_pure_movable` ⊆ `opcode_is_cse_safe` (dropping the `nothrow`
/// requirement only ever adds ops).
#[inline]
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

/// True if `opcode` belongs to the GVN *type-gated* numberable family: a
/// CSE-safe arithmetic/comparison/bitwise/boolean computation whose purity is
/// conditional on its operands being primitive types. GVN enforces that operand
/// precondition separately (`is_primitive_type`); this predicate supplies only
/// the opcode-level purity half, derived from the oracle.
///
/// Box/unbox (`is_always_numberable`) and constants are CSE-safe too but are
/// handled by GVN's unconditional / const-local paths, not this type-gated one,
/// so they are intentionally excluded here.
#[inline]
pub(super) fn opcode_is_type_gated_numberable(opcode: OpCode) -> bool {
    // Exclude the ops GVN routes through its other (non-type-gated) paths:
    // box/unbox go through `is_always_numberable`; constants through the
    // const-local path; `BuildSlice` is not value-numbered by GVN today.
    if matches!(
        opcode,
        OpCode::BoxVal
            | OpCode::UnboxVal
            | OpCode::ConstInt
            | OpCode::ConstBigInt
            | OpCode::ConstFloat
            | OpCode::ConstStr
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstBytes
            | OpCode::BuildSlice
    ) {
        return false;
    }
    opcode_is_cse_safe(opcode)
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
    //
    // These tests pin the oracle-derived movable / CSE / type-gated-numberable
    // sets to their historical values and assert the structural invariants that
    // keep LICM and GVN sound. If a new opcode is added to one set without the
    // other, or an op is mis-classified pure, these fail loudly — which is the
    // entire point of collapsing the three hand-maintained allowlists into one
    // oracle. Update the EXPECTED arrays here ONLY when you can justify, op by
    // op, that the new classification is pure (consistent + effect-free) and —
    // for the movable set — also never-throwing.

    /// Compile-time forcing function: an exhaustive (wildcard-free) match over
    /// `OpCode`. Adding a variant to the enum makes this fail to compile,
    /// forcing whoever adds it to also list it in `all_opcodes()` below and
    /// thereby give it a deliberate purity classification in the oracle tests.
    fn assert_opcode_is_listed(op: OpCode) {
        use OpCode::*;
        match op {
            Add
            | CheckedAdd
            | Sub
            | Mul
            | InplaceAdd
            | InplaceSub
            | InplaceMul
            | Div
            | FloorDiv
            | Mod
            | Pow
            | Neg
            | Pos
            | Eq
            | Ne
            | Lt
            | Le
            | Gt
            | Ge
            | Is
            | IsNot
            | In
            | NotIn
            | BitAnd
            | BitOr
            | BitXor
            | BitNot
            | Shl
            | Shr
            | And
            | Or
            | Not
            | Bool
            | Alloc
            | StackAlloc
            | ObjectNewBound
            | ObjectNewBoundStack
            | Free
            | LoadAttr
            | StoreAttr
            | DelAttr
            | Index
            | StoreIndex
            | DelIndex
            | DeleteVar
            | Call
            | CallMethod
            | CallBuiltin
            | OrdAt
            | BoxVal
            | UnboxVal
            | TypeGuard
            | IncRef
            | DecRef
            | DelBoundary
            | BuildList
            | BuildDict
            | BuildTuple
            | BuildSet
            | BuildSlice
            | GetIter
            | IterNext
            | IterNextUnboxed
            | ForIter
            | AllocTask
            | StateSwitch
            | StateTransition
            | StateYield
            | ChanSendYield
            | ChanRecvYield
            | ClosureLoad
            | ClosureStore
            | Yield
            | YieldFrom
            | Raise
            | CheckException
            | ExceptionPending
            | FunctionDefaultsVersion
            | TryStart
            | TryEnd
            | StateBlockStart
            | StateBlockEnd
            | ConstInt
            | ConstBigInt
            | ConstFloat
            | ConstStr
            | ConstBool
            | ConstNone
            | ConstBytes
            | Copy
            | Import
            | ImportFrom
            | ModuleCacheGet
            | ModuleCacheSet
            | ModuleCacheDel
            | ModuleGetAttr
            | ModuleImportFrom
            | ModuleGetGlobal
            | ModuleGetName
            | ModuleSetAttr
            | ModuleDelGlobal
            | ModuleDelGlobalIfPresent
            | WarnStderr
            | ScfIf
            | ScfFor
            | ScfWhile
            | ScfYield
            | Deopt => {}
        }
    }

    /// Every `OpCode` variant. Kept exhaustive by `assert_opcode_is_listed`.
    fn all_opcodes() -> Vec<OpCode> {
        use OpCode::*;
        [
            Add,
            CheckedAdd,
            Sub,
            Mul,
            InplaceAdd,
            InplaceSub,
            InplaceMul,
            Div,
            FloorDiv,
            Mod,
            Pow,
            Neg,
            Pos,
            Eq,
            Ne,
            Lt,
            Le,
            Gt,
            Ge,
            Is,
            IsNot,
            In,
            NotIn,
            BitAnd,
            BitOr,
            BitXor,
            BitNot,
            Shl,
            Shr,
            And,
            Or,
            Not,
            Bool,
            Alloc,
            StackAlloc,
            ObjectNewBound,
            ObjectNewBoundStack,
            Free,
            LoadAttr,
            StoreAttr,
            DelAttr,
            Index,
            StoreIndex,
            DelIndex,
            DeleteVar,
            Call,
            CallMethod,
            CallBuiltin,
            OrdAt,
            BoxVal,
            UnboxVal,
            TypeGuard,
            IncRef,
            DecRef,
            DelBoundary,
            BuildList,
            BuildDict,
            BuildTuple,
            BuildSet,
            BuildSlice,
            GetIter,
            IterNext,
            IterNextUnboxed,
            ForIter,
            AllocTask,
            StateSwitch,
            StateTransition,
            StateYield,
            ChanSendYield,
            ChanRecvYield,
            ClosureLoad,
            ClosureStore,
            Yield,
            YieldFrom,
            Raise,
            CheckException,
            ExceptionPending,
            FunctionDefaultsVersion,
            TryStart,
            TryEnd,
            StateBlockStart,
            StateBlockEnd,
            ConstInt,
            ConstBigInt,
            ConstFloat,
            ConstStr,
            ConstBool,
            ConstNone,
            ConstBytes,
            Copy,
            Import,
            ImportFrom,
            ModuleCacheGet,
            ModuleCacheSet,
            ModuleCacheDel,
            ModuleGetAttr,
            ModuleImportFrom,
            ModuleGetGlobal,
            ModuleGetName,
            ModuleSetAttr,
            ModuleDelGlobal,
            ModuleDelGlobalIfPresent,
            WarnStderr,
            ScfIf,
            ScfFor,
            ScfWhile,
            ScfYield,
            Deopt,
        ]
        .into_iter()
        .collect()
    }

    /// The exact `is_hoistable` opcode set as it stood before S3 unified the
    /// predicates (the LICM allowlist, minus the dynamic `is_plain_value_copy`
    /// check which is an op-instance property, not an opcode property).
    const EXPECTED_MOVABLE: &[OpCode] = &[
        OpCode::Add,
        OpCode::Sub,
        OpCode::Mul,
        OpCode::InplaceAdd,
        OpCode::InplaceSub,
        OpCode::InplaceMul,
        OpCode::Neg,
        OpCode::Pos,
        OpCode::Eq,
        OpCode::Ne,
        OpCode::Lt,
        OpCode::Le,
        OpCode::Gt,
        OpCode::Ge,
        OpCode::Is,
        OpCode::IsNot,
        OpCode::BitAnd,
        OpCode::BitOr,
        OpCode::BitXor,
        OpCode::BitNot,
        // NOTE: `Shl`/`Shr` are deliberately NOT movable. They raise
        // `ValueError("negative shift count")` for a negative count, so they are
        // `pure_may_throw` — CSE-safe under dominance but UNSOUND to hoist above
        // a guard (a guard may be what proves the count non-negative). They
        // therefore live in `EXPECTED_TYPE_GATED_NUMBERABLE` (CSE) but not here
        // (LICM), exactly like `Div`/`FloorDiv`/`Mod`/`Pow`.
        OpCode::And,
        OpCode::Or,
        OpCode::Not,
        OpCode::Bool,
        OpCode::ConstInt,
        OpCode::ConstBigInt,
        OpCode::ConstFloat,
        OpCode::ConstStr,
        OpCode::ConstBool,
        OpCode::ConstNone,
        OpCode::ConstBytes,
        OpCode::BoxVal,
        OpCode::UnboxVal,
        OpCode::TypeGuard,
        OpCode::BuildSlice,
    ];

    /// The exact `is_typed_numberable` opcode set as it stood before S3 (the
    /// GVN type-gated allowlist).
    const EXPECTED_TYPE_GATED_NUMBERABLE: &[OpCode] = &[
        OpCode::Add,
        OpCode::Sub,
        OpCode::Mul,
        OpCode::InplaceAdd,
        OpCode::InplaceSub,
        OpCode::InplaceMul,
        OpCode::Div,
        OpCode::FloorDiv,
        OpCode::Mod,
        OpCode::Pow,
        OpCode::Neg,
        OpCode::Pos,
        OpCode::Eq,
        OpCode::Ne,
        OpCode::Lt,
        OpCode::Le,
        OpCode::Gt,
        OpCode::Ge,
        OpCode::Is,
        OpCode::IsNot,
        OpCode::BitAnd,
        OpCode::BitOr,
        OpCode::BitXor,
        OpCode::BitNot,
        OpCode::Shl,
        OpCode::Shr,
        OpCode::And,
        OpCode::Or,
        OpCode::Not,
        OpCode::Bool,
        OpCode::TypeGuard,
    ];

    #[test]
    fn movable_set_is_byte_identical_to_historical_licm_list() {
        // Touch the forcing function so it is compiled (and so a newly added
        // OpCode variant fails the build until it is classified here).
        for &op in all_opcodes().iter() {
            assert_opcode_is_listed(op);
        }
        for &op in all_opcodes().iter() {
            let expected = EXPECTED_MOVABLE.contains(&op);
            assert_eq!(
                opcode_is_pure_movable(op),
                expected,
                "{op:?}: oracle pure_movable disagrees with historical LICM is_hoistable list"
            );
        }
    }

    #[test]
    fn type_gated_numberable_set_is_byte_identical_to_historical_gvn_list() {
        for &op in all_opcodes().iter() {
            let expected = EXPECTED_TYPE_GATED_NUMBERABLE.contains(&op);
            assert_eq!(
                opcode_is_type_gated_numberable(op),
                expected,
                "{op:?}: oracle type-gated-numberable disagrees with historical GVN is_typed_numberable list"
            );
        }
    }

    #[test]
    fn movable_implies_cse_safe() {
        // Dropping the `nothrow` requirement can only ADD ops: every movable op
        // must also be CSE-safe. (LICM ⊆ GVN purity core.)
        for &op in all_opcodes().iter() {
            if opcode_is_pure_movable(op) {
                assert!(
                    opcode_is_cse_safe(op),
                    "{op:?}: pure_movable but not cse_safe — invariant movable ⊆ cse violated"
                );
            }
        }
    }

    #[test]
    fn movable_ops_are_never_side_effecting_or_may_throw() {
        // The whole point of `pure_movable`: it must be a strict subset of the
        // DCE oracle's "neither side-effecting nor may-throw" set. If any
        // movable op were side-effecting or may-throw, hoisting it out of a
        // loop would be unsound.
        for &op in all_opcodes().iter() {
            if opcode_is_pure_movable(op) {
                assert!(
                    !opcode_is_side_effecting(op),
                    "{op:?}: classified pure_movable but opcode_is_side_effecting — UNSOUND to hoist"
                );
                assert!(
                    !opcode_may_throw(op),
                    "{op:?}: classified pure_movable but opcode_may_throw — UNSOUND to hoist"
                );
            }
        }
    }

    #[test]
    fn cse_safe_ops_are_never_side_effecting() {
        // CSE may replace a may-throw op (dominance guards the throw), but it
        // must NEVER replace a side-effecting op: collapsing two side-effecting
        // ops into one would drop an effect. So cse_safe ⊆ !side_effecting.
        for &op in all_opcodes().iter() {
            if opcode_is_cse_safe(op) {
                assert!(
                    !opcode_is_side_effecting(op),
                    "{op:?}: classified cse_safe but opcode_is_side_effecting — UNSOUND to value-number"
                );
            }
        }
    }

    #[test]
    fn cse_extra_over_movable_is_exactly_the_pure_may_throw_arith() {
        // The only ops that are CSE-safe but NOT movable are the pure-but-
        // may-throw primitive arithmetic ops. This pins the documented reason
        // GVN's set is larger than LICM's: dropping `nothrow` admits exactly the
        // `pure_may_throw` family — {Div, FloorDiv, Mod, Pow} (zero divisor /
        // `0 ** -1`) plus {Shl, Shr} (negative shift count) — and nothing else.
        let mut extra: Vec<OpCode> = all_opcodes()
            .into_iter()
            .filter(|&op| opcode_is_cse_safe(op) && !opcode_is_pure_movable(op))
            .collect();
        extra.sort_by_key(|op| format!("{op:?}"));

        let mut expected = vec![
            OpCode::Div,
            OpCode::FloorDiv,
            OpCode::Mod,
            OpCode::Pow,
            OpCode::Shl,
            OpCode::Shr,
        ];
        expected.sort_by_key(|op| format!("{op:?}"));

        assert_eq!(
            extra, expected,
            "CSE-extra-over-movable must be exactly the pure_may_throw family \
             {{Div, FloorDiv, Mod, Pow, Shl, Shr}}"
        );

        // The named `opcode_is_pure_may_throw` predicate (consumed by LICM's
        // throw-disproven hoist gate, #49) must pin to exactly that same family —
        // it is `cse_safe && !pure_movable` by definition, asserted here so a
        // future op-kind reclassification cannot silently widen the set LICM
        // would attempt to conditionally hoist.
        let mut named: Vec<OpCode> = all_opcodes()
            .into_iter()
            .filter(|&op| opcode_is_pure_may_throw(op))
            .collect();
        named.sort_by_key(|op| format!("{op:?}"));
        assert_eq!(
            named, expected,
            "opcode_is_pure_may_throw must be exactly {{Div, FloorDiv, Mod, Pow, Shl, Shr}}"
        );
    }
}
