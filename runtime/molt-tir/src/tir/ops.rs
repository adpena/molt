use std::collections::HashMap;

use super::types::TirType;
use super::values::ValueId;

/// Dialect namespace for operations (MLIR-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum Dialect {
    /// Core Molt operations (arithmetic, memory, call, etc.).
    Molt,
    /// Structured control flow (if/for/while with regions).
    Scf,
    /// GPU offload operations (future).
    Gpu,
    /// Parallel execution (future).
    Par,
    /// SIMD vectorisation (future).
    Simd,
}

/// Operation code within a dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum OpCode {
    // Arithmetic
    Add,
    Sub,
    Mul,
    /// Signed 64-bit add with hardware-exact overflow detection.
    ///
    /// Operands: `[lhs: I64, rhs: I64]` — both raw i64 carriers.
    /// Two results: `results[0]` = the wrapping i64 sum, `results[1]` = the
    /// overflow flag (Bool, true iff the signed addition overflowed i64).
    ///
    /// CONTRACT (soundness-critical): when `results[1]` is true,
    /// `results[0]` holds the mathematically WRAPPED value and MUST NOT be
    /// observed as a Python int — feeding it to `molt_int_from_i64` would
    /// produce a wrong BigInt. Consumers (the `overflow_peel` dual-loop
    /// transform) branch on `results[1]` and seed the boxed continuation
    /// from the PRE-add operands, never from the wrapped sum.
    ///
    /// Lowering: native = Cranelift `sadd_overflow`; LLVM =
    /// `llvm.sadd.with.overflow.i64`; WASM = raw `i64.add` + the sign-bit
    /// identity `((lhs ^ sum) & (rhs ^ sum)) < 0`; Luau = the
    /// `molt_checked_i64_add` prelude helper `(a + b, false)` — f64 addition
    /// never wraps i64, so the overflow branch is correctly dead there.
    ///
    /// Deliberately NOT classified pure-movable/CSE-safe in the effects
    /// oracle: a 2-result op must not be value-numbered or hoisted until the
    /// multi-result handling of GVN/LICM is separately verified.
    CheckedAdd,
    /// Signed 64-bit multiply with hardware-exact overflow detection.
    ///
    /// Operands: `[lhs: I64, rhs: I64]` — both raw i64 carriers.
    /// Two results: `results[0]` = the wrapping i64 product, `results[1]` = the
    /// overflow flag (Bool, true iff the signed multiplication overflowed i64).
    ///
    /// CONTRACT (soundness-critical): when `results[1]` is true,
    /// `results[0]` holds the mathematically WRAPPED value and MUST NOT be
    /// observed as a Python int — feeding it to `molt_int_from_i64` would
    /// produce a wrong BigInt. Consumers (the `overflow_peel` dual-loop
    /// transform) branch on `results[1]` and seed the boxed continuation
    /// from the PRE-multiply operands, never from the wrapped product.
    ///
    /// Lowering: native = `imul` + the `smulhi`/`sshr` sign-extension identity
    /// (Cranelift 0.131 has NO `smul_overflow`, unlike `sadd_overflow`); the
    /// product overflows i64 iff the high 64 bits differ from the arithmetic
    /// shift of the low 64 bits (`hi != lo >> 63`) — a FULL 64-bit-exact flag,
    /// never a 47-bit `fits_inline` test. LLVM = `llvm.smul.with.overflow.i64`.
    /// WASM = boxed-lane-only v1 (no raw 64x64->128 overflow helper yet, a
    /// documented target limitation). Luau = the `molt_checked_i64_mul` prelude
    /// helper which returns `flag = true` whenever exactness cannot be proven
    /// (f64 loses mantissa bits in the i64 range, so a structural
    /// `return a*b, false` would be a silent wrong answer — the conservative
    /// flag forces the sound boxed slow loop).
    ///
    /// Deliberately NOT classified pure-movable/CSE-safe in the effects
    /// oracle: a 2-result op must not be value-numbered or hoisted until the
    /// multi-result handling of GVN/LICM is separately verified.
    CheckedMul,
    // In-place arithmetic (must roundtrip as inplace_* to preserve semantics)
    InplaceAdd,
    InplaceSub,
    InplaceMul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Neg,
    Pos,
    // Comparison
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
    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,
    // Boolean
    And,
    Or,
    Not,
    Bool,
    // Memory
    Alloc,
    StackAlloc,
    /// Allocate a vanilla user-class instance with `class_ref` operand.
    /// Lowers to `molt_object_new_bound(class_bits)`.  The frontend's
    /// class-instantiation fold emits this op for `Class(args)` call
    /// sites where the class layout is statically known and `__new__`
    /// is the default.  Distinct from generic `Alloc` because the
    /// allocation size and finalizer are derived from the class layout
    /// rather than a fixed heap-block descriptor.
    ObjectNewBound,
    /// Stack-allocated instance (escape analysis NoEscape variant of
    /// `ObjectNewBound`).  Only valid when the class has a fixed,
    /// non-extensible layout and the result does not escape the
    /// enclosing function.  Lowers to a Cranelift `StackSlot` of the
    /// class's slot count.
    ObjectNewBoundStack,
    Free,
    LoadAttr,
    StoreAttr,
    DelAttr,
    Index,
    StoreIndex,
    DelIndex,
    /// Delete a Python local slot: store the missing sentinel into the named
    /// local and release the previous slot occupant at this exact boundary.
    ///
    /// Operands: `[missing_sentinel, old_slot_value]`. Both operands are borrowed
    /// by the op; DropInsertion releases `old_slot_value` immediately after this
    /// op so finalizers observe the already-missing local.
    /// Result: the new local value in SSA form (`missing_sentinel`).
    /// Metadata: `_var` carries the Python local name through round-trips.
    DeleteVar,
    // Call
    Call,
    CallMethod,
    CallBuiltin,
    /// Fused `ord(container[index])`.
    ///
    /// Operands: `[container, index]`.
    /// Result: I64 code point on the successful path. This operation may
    /// raise for invalid indexing or invalid `ord()` inputs and therefore is
    /// not a transparent copy even though the legacy SimpleIR spelling is a
    /// compact helper op.
    OrdAt,
    // Box/unbox
    BoxVal,
    UnboxVal,
    TypeGuard,
    // Refcount
    IncRef,
    DecRef,
    /// Python lifetime boundary: a function-scope `del x` of a (non-closure)
    /// local. Operand 0 is the binding's current value. The frontend emits it
    /// so the boundary FACT survives to the backend (pre-#58 the `del` lowered
    /// to nothing and the release timing was whatever SSA-last-use happened to
    /// be). The terminal drop phase NORMALIZES it: rewritten in place to the
    /// releasing `DecRef` when the root is pass-owned (droppable), deleted
    /// otherwise (raw/param/borrowed — CPython's frame-slot decref is equally
    /// unobservable there). On targets where the drop phase does not run (the
    /// dormant-native value-tracking lane) codegen ignores it; its operand USE
    /// pins the native `last_use` to the `del` statement, which IS the correct
    /// release point for that lane. side_effecting=true so DCE keeps it.
    DelBoundary,
    // Build containers
    BuildList,
    BuildDict,
    BuildTuple,
    BuildSet,
    BuildSlice,
    // Iteration
    GetIter,
    IterNext,
    /// Fused iter_next that produces (value, done_flag) directly,
    /// bypassing the tuple allocation + index ops.  Two results:
    /// results[0] = value, results[1] = done_flag.
    IterNextUnboxed,
    ForIter,
    // Generator
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
    // Exception
    Raise,
    CheckException,
    /// Read the runtime "exception pending" flag as a boolean value
    /// (`molt_exception_pending_fast() != 0`).  Side-effecting / non-foldable:
    /// it observes mutable runtime state that no SSA/SCCP/GVN pass may prove
    /// constant, so the value (and any branch derived from it) is preserved.
    /// Used as the condition of the `loop_break_if_exception` CondBranch that
    /// exits an iterator-consumer loop on a mid-iteration raise.
    ExceptionPending,
    /// Read a function object's `__defaults__`/`__kwdefaults__` mutation version
    /// stamp (`molt_function_defaults_version_slot`).  Takes one arg (the
    /// function object) and yields the version as an inline int.  Side-effecting
    /// / non-foldable for the same reason as `ExceptionPending`: the slot is
    /// mutable runtime state (any `func.__defaults__ = ...` reassignment bumps
    /// it), so no SSA/SCCP/GVN/LICM pass may hoist or fold the read across a
    /// potential mutation.  The compile-time defaults-devirt deopt guard reads
    /// it once per call and branches `version == 0` (baked literal, fast) vs
    /// `!= 0` (live `__defaults__`/`__kwdefaults__` read).
    FunctionDefaultsVersion,
    TryStart,
    TryEnd,
    StateBlockStart,
    StateBlockEnd,
    // Constants
    ConstInt,
    /// Arbitrary-precision integer constant whose value does not fit the
    /// raw i64 fast-path window. The decimal text lives in the `s_value`
    /// attr; lowering materializes it via `molt_bigint_from_str(ptr, len)`
    /// and the result is ALWAYS a boxed heap int (`DynBox` carrier — never
    /// `I64`, never RawI64Safe). First-class (not a `Copy` fallback) so the
    /// TIR-consuming LLVM backend defines the result value: as a fallback
    /// `Copy` with zero operands the result was silently left undefined and
    /// resolved to the `None` sentinel — a miscompile, not an error.
    ConstBigInt,
    ConstFloat,
    ConstStr,
    ConstBool,
    ConstNone,
    ConstBytes,
    // SSA
    Copy,
    // Import
    Import,
    ImportFrom,
    /// Read a module object from the runtime module cache by name.
    ///
    /// Operands: `[module_name_value]`.
    /// Result: dynamic Molt value (`module` object on hit, `None` on miss).
    /// This validates that the name is a string and increments the returned
    /// module handle on cache hits, so optimization passes must not treat it
    /// as a pure value copy.
    ModuleCacheGet,
    /// Write a module object into the runtime module cache by name.
    ///
    /// Operands: `[module_name_value, module_value]`.
    /// Result: none. This mutates the runtime module cache, refcounts the
    /// cached module entry, synchronizes `sys.modules`, and may run special
    /// bootstrap side effects for `sys`.
    ModuleCacheSet,
    /// Remove a module object from the runtime module cache by name.
    ///
    /// Operands: `[module_name_value]`.
    /// Result: none. This mutates the runtime module cache and `sys.modules`
    /// and may raise when the name is not a string.
    ModuleCacheDel,
    /// Read an attribute from a runtime module object.
    ///
    /// Operands: `[module_value, attr_name_value]`.
    /// Result: dynamic Molt value. This may raise when the module does
    /// not define the requested attribute, so optimization passes must
    /// not treat it as a pure value copy.
    ModuleGetAttr,
    /// Bind an attribute for a `from MODULE import name` statement.
    ///
    /// Operands: `[module_value, attr_name_value]`.
    /// Result: dynamic Molt value (the imported binding).
    ///
    /// Distinct from [`OpCode::ModuleGetAttr`] because CPython's `IMPORT_FROM`
    /// has import-specific failure semantics: a missing attribute is first
    /// retried as a `sys.modules["{module}.{name}"]` submodule lookup
    /// (circular-import recovery) and, on miss, raises
    /// `ImportError("cannot import name ...")` rather than `AttributeError`.
    /// For every optimization pass it behaves identically to `ModuleGetAttr`
    /// (effectful, may raise, DCE-preserved, DynBox result); only the runtime
    /// entrypoint and failure mode differ.
    ModuleImportFrom,
    /// Resolve a module global using CPython LOAD_GLOBAL semantics.
    ///
    /// Operands: `[module_value, global_name_value]`.
    /// Result: dynamic Molt value. This may fall back through builtins or
    /// raise `NameError`, so it is observable even when the result is dead.
    ModuleGetGlobal,
    /// Read a named module attribute through the `module_get_name` runtime
    /// entrypoint used by the current native/WASM import surface.
    ///
    /// Operands: `[module_value, attr_name_value]`.
    /// Result: dynamic Molt value. This delegates to module attribute lookup
    /// and therefore may raise.
    ModuleGetName,
    /// Set a module attribute through the runtime module dictionary path.
    ///
    /// Operands: `[module_value, attr_name_value, value]`.
    /// Result: none. This is not equivalent to generic StoreAttr: module
    /// assignment has module-dict and annotation-specific runtime semantics.
    ModuleSetAttr,
    /// Delete a module global through CPython module dictionary semantics.
    ///
    /// Operands: `[module_value, global_name_value]`.
    /// Result: none. This mutates the module dictionary and raises `NameError`
    /// when the binding is absent.
    ModuleDelGlobal,
    /// Delete a module global when present, suppressing absent-name errors.
    ///
    /// Operands: `[module_value, global_name_value]`.
    /// Result: none. This mutates the module dictionary and is used for
    /// compiler-generated cleanup where CPython requires missing cleanup names
    /// to be ignored. Invalid module operands remain observable errors.
    ModuleDelGlobalIfPresent,
    // IO / diagnostics
    WarnStderr,
    // Structured control flow (scf dialect)
    ScfIf,
    ScfFor,
    ScfWhile,
    ScfYield,
    // Deoptimization
    Deopt,
}

/// Attribute value for operation metadata.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum AttrValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// Attribute dictionary attached to an operation.
pub type AttrDict = HashMap<String, AttrValue>;

pub const SOURCE_LINE_ATTR: &str = "_source_line";
pub const SOURCE_COL_ATTR: &str = "_col_offset";
pub const SOURCE_END_COL_ATTR: &str = "_end_col_offset";
pub const SOURCE_FILE_ATTR: &str = "_source_file";

/// Stable source-site coordinates carried through TIR attrs.
///
/// `source_span` remains the byte-range carrier. This fact is explicitly
/// line/column based because the current frontend authority is the SimpleIR
/// `line` marker plus expression-level column offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSite {
    pub line: u32,
    pub col: Option<u32>,
    pub end_col: Option<u32>,
}

impl SourceSite {
    pub fn from_line_col(line: i64, col: Option<i64>, end_col: Option<i64>) -> Option<Self> {
        if line <= 0 || line > u32::MAX as i64 {
            return None;
        }
        let col = col.and_then(|value| u32::try_from(value).ok());
        let end_col = end_col.and_then(|value| u32::try_from(value).ok());
        Some(Self {
            line: line as u32,
            col,
            end_col,
        })
    }

    pub fn from_attrs(attrs: &AttrDict) -> Option<Self> {
        let Some(AttrValue::Int(line)) = attrs.get(SOURCE_LINE_ATTR) else {
            return None;
        };
        let col = match attrs.get(SOURCE_COL_ATTR) {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        };
        let end_col = match attrs.get(SOURCE_END_COL_ATTR) {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        };
        Self::from_line_col(*line, col, end_col)
    }

    pub fn write_attrs(self, attrs: &mut AttrDict) {
        attrs.insert(SOURCE_LINE_ATTR.into(), AttrValue::Int(self.line as i64));
        if let Some(col) = self.col {
            attrs.insert(SOURCE_COL_ATTR.into(), AttrValue::Int(col as i64));
        }
        if let Some(end_col) = self.end_col {
            attrs.insert(SOURCE_END_COL_ATTR.into(), AttrValue::Int(end_col as i64));
        }
    }
}

/// A single operation in the TIR.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TirOp {
    /// Dialect this operation belongs to.
    pub dialect: Dialect,
    /// The operation code.
    pub opcode: OpCode,
    /// SSA value operands (inputs).
    pub operands: Vec<ValueId>,
    /// SSA value results (outputs).
    pub results: Vec<ValueId>,
    /// Metadata / immediate attributes.
    pub attrs: AttrDict,
    /// Source location for diagnostics (byte offset range).
    pub source_span: Option<(u32, u32)>,
}

impl TirOp {
    pub fn source_site(&self) -> Option<SourceSite> {
        SourceSite::from_attrs(&self.attrs)
    }

    pub fn set_source_site(&mut self, site: SourceSite) {
        site.write_attrs(&mut self.attrs);
    }

    pub fn inherit_source_from(&mut self, other: &TirOp) {
        self.source_span = other.source_span;
        if let Some(site) = other.source_site() {
            self.set_source_site(site);
        }
    }

    /// True only for a structural SSA value copy.
    ///
    /// `OpCode::Copy` is also the legacy fallback carrier for SimpleIR
    /// operations that have not yet been promoted into first-class TIR
    /// opcodes.  Those fallback copies carry semantic attributes such as
    /// `_original_kind` and must not be propagated, hoisted, vectorized, or
    /// otherwise treated as transparent copies.
    #[inline]
    pub fn is_plain_value_copy(&self) -> bool {
        self.opcode == OpCode::Copy
            && self.operands.len() == 1
            && self.results.len() == 1
            && self.attrs.is_empty()
    }
}

/// Build a representation-matched dead placeholder constant for an SSA edge.
///
/// This is for values that are required to satisfy SSA arity but are proven dead
/// by surrounding control flow before any observation. `I64` and `BigInt` both
/// use `ConstInt(0)` so representation planning sees the same scalar default
/// lane, while reference-like or otherwise dynamic continuations receive
/// `ConstNone`.
pub fn dead_placeholder_const_for_type(ty: &TirType, result: ValueId) -> TirOp {
    let (opcode, attrs) = match ty {
        TirType::I64 | TirType::BigInt => {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(0));
            (OpCode::ConstInt, attrs)
        }
        TirType::Bool => {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Bool(false));
            (OpCode::ConstBool, attrs)
        }
        TirType::F64 => {
            let mut attrs = AttrDict::new();
            attrs.insert("f_value".into(), AttrValue::Float(0.0));
            (OpCode::ConstFloat, attrs)
        }
        _ => (OpCode::ConstNone, AttrDict::new()),
    };
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: Vec::new(),
        results: vec![result],
        attrs,
        source_span: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_placeholder_constants_match_representation_defaults() {
        for ty in [TirType::I64, TirType::BigInt] {
            let op = dead_placeholder_const_for_type(&ty, ValueId(7));
            assert_eq!(op.opcode, OpCode::ConstInt);
            assert_eq!(op.attrs.get("value"), Some(&AttrValue::Int(0)));
            assert_eq!(op.results, vec![ValueId(7)]);
        }

        let bool_op = dead_placeholder_const_for_type(&TirType::Bool, ValueId(8));
        assert_eq!(bool_op.opcode, OpCode::ConstBool);
        assert_eq!(bool_op.attrs.get("value"), Some(&AttrValue::Bool(false)));

        let float_op = dead_placeholder_const_for_type(&TirType::F64, ValueId(9));
        assert_eq!(float_op.opcode, OpCode::ConstFloat);
        assert_eq!(float_op.attrs.get("f_value"), Some(&AttrValue::Float(0.0)));

        let dynbox_op = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(10));
        assert_eq!(dynbox_op.opcode, OpCode::ConstNone);
        assert!(dynbox_op.attrs.is_empty());
    }
}
