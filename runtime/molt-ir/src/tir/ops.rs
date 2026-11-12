use std::collections::HashMap;

use super::types::TirType;
use super::values::ValueId;
use crate::ir::ParameterCustody;

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
    /// Publish the code-table location observed by callbacks and exceptions.
    FrameSetLocation,
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
    Free,
    LoadAttr,
    /// CPython's `LOAD_ATTR` in method form.
    ///
    /// Operands: `[receiver]`. Attr `name`: the attribute.
    /// Results: `[callable, self_or_null]`. When the attribute is a plain
    /// function on the receiver's type, `callable` is that function and
    /// `self_or_null` a new reference to the receiver; otherwise `callable`
    /// is the attribute and `self_or_null` the null sentinel
    /// (`molt_codegen_abi::loaded_method_null_bits`). Only operand 1 of one
    /// [`OpCode::CallLoadedMethod`] or [`OpCode::CallargsPushMethodReceiver`] may consume
    /// `self_or_null` (verified).
    LoadMethod,
    /// CPython's `LOAD_DEREF __class__` and `LOAD_SUPER_ATTR` in method form
    /// for zero-argument `super()`.
    ///
    /// Operands: `[class_cell, first_arg]`: the `__class__` cell and the
    /// method's first argument. The load reads the cell; an empty cell raises
    /// LOAD_DEREF's `NameError`. Attr `name`. Results as
    /// [`OpCode::LoadMethod`], and no super object is created when the
    /// attribute is a plain function.
    LoadSuperMethod,
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
    /// CPython's `CALL` after a method load.
    ///
    /// Operands: `[callable, self_or_null, args...]`, the results of one
    /// [`OpCode::LoadMethod`] or [`OpCode::LoadSuperMethod`] and the positional
    /// arguments. With a receiver it calls `callable(self, *args)`; with the
    /// null sentinel it calls `callable(*args)`. A source call: it adopts every
    /// operand (`argument_custody`).
    CallLoadedMethod,
    /// Prefix an existing Stack CallArgs builder with a loaded receiver.
    /// Operands: `[builder, self_or_null]`; no results. The builder is borrowed,
    /// the nullable word is adopted on both continuations. Null is skipped;
    /// a real receiver is retained by the builder before its loaded owner drops.
    /// This runs before argument evaluation, so only Python objects (callable
    /// and builder) ever need frame storage across suspension.
    CallargsPushMethodReceiver,
    /// Named lookup carries arguments only; metadata-free lookup carries
    /// [name, arguments...]. Use builtin_call() for the executable contract.
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
    // Late-TIR representation primitives and the cold runtime edge (HF-821)
    IsInlineInt,
    UnboxInlineInt,
    IsBoolWord,
    IsFloatWord,
    UnboxFloatWord,
    UnboxBoolWord,
    BoxInlineInt,
    CodeIs,
    DirectCall,
    CallRuntime,
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
    /// Exact Python sequence unpacking with a variable number of independently
    /// owned results.
    ///
    /// Operands: `[iterable]`. Results: one `DynBox` value per assignment
    /// target, including zero results for Python's empty unpack targets. The
    /// integer `value` attribute is the expected/result count and must equal
    /// `results.len()`. The runtime ABI initializes every output slot to `None`,
    /// then transactionally publishes one owned `+1` value per slot only after
    /// exact-arity success.
    UnpackSequence,
    ForIter,
    // Generator
    AllocTask,
    StateSwitch,
    StateTransition,
    StateYield,
    /// Store the next activation state; does not suspend.
    StateSet,
    /// Test the scheduler sentinel without Python truthiness or callbacks.
    IsPending,
    /// Register this activation on a future; does not return or transfer it.
    TaskWait,
    ClosureLoad,
    ClosureStore,
    /// Publish Python argument-zero/class-cell custody to the active frame.
    /// Three borrowed boxed operands, no result. Replacing owners can finalize
    /// and raise; this internal service is not a Python call-return poll point.
    FrameContextSet,
    Yield,
    YieldFrom,
    // Exception
    Raise,
    CheckException,
    /// Read the runtime "exception pending" flag as a boolean value
    /// (`molt_exception_pending() != 0`).  Side-effecting / non-foldable:
    /// it observes mutable runtime state that no SSA/SCCP/GVN pass may prove
    /// constant, so the value (and any branch derived from it) is preserved.
    /// Used as the condition of the `loop_break_if_exception` CondBranch that
    /// exits an iterator-consumer loop on a mid-iteration raise.
    ExceptionPending,
    /// Read the thread's pending-work word as a boolean: true when a signal
    /// handler, a pending call or a scheduler request waits for the next
    /// async-work poll. It runs no work and cannot raise. The word is set
    /// from other threads and from signal context, so the read is pinned at
    /// its program point. A promoted module-level loop branches on it to the
    /// unpromoted loop, whose poll services the work.
    AsyncWorkPending,
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
    /// Final-application ModuleRegistry row for canonical name in `s_value`.
    /// The result is a bounded u32 integer; reusable code never embeds its ID.
    ModuleSlot,
    ConstInt,
    /// Arbitrary-precision integer constant whose value does not fit the
    /// raw i64 fast-path window. The minimal signed big-endian bytes live in the `bytes`
    /// attr; lowering materializes it via `molt_bigint_from_bytes(ptr, len)`
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
    /// The slot words compiled reads of one module's globals use in this
    /// activation (call_anatomy.md 4.5). Operand: the module's `ModuleSlot`
    /// id. Result: an inline int of the words' address shifted right by
    /// `GLOBAL_SLOT_BASE_SHIFT`. Pure within an activation, so GVN merges
    /// and LICM hoists it; it never raises.
    GlobalSlotBase,
    /// A read of a module global through its slot (call_anatomy.md 4.5).
    /// Operands: `[base, module_value, global_name_value]`; the op carries
    /// its `global_slot`. A bound slot is the value; an unbound one takes
    /// `ModuleGetGlobal`'s by-name path on the last two operands, with the
    /// same results, exceptions and effects.
    GlobalSlotGet,
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
pub const SOURCE_OP_INDEX_ATTR: &str = "_source_op_idx";

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
    /// Preserved wire identity, separate from untyped payload attributes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::OpKind>,
    /// The runtime entry a `CallRuntime` op calls. The verifier requires it on
    /// `CallRuntime` and forbids it on every other op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::runtime_fn_generated::RuntimeFn>,
    /// Dialect this operation belongs to.
    pub dialect: Dialect,
    /// The operation code.
    pub opcode: OpCode,
    /// SSA value operands (inputs).
    pub operands: Vec<ValueId>,
    /// SSA value results (outputs).
    pub results: Vec<ValueId>,
    /// Metadata / immediate attributes.
    #[serde(deserialize_with = "deserialize_op_attrs")]
    pub attrs: AttrDict,
    /// Source location for diagnostics (byte offset range).
    pub source_span: Option<(u32, u32)>,
}

/// Legacy identity metadata is rejected, never interpreted as a second kind channel.
pub(crate) fn validate_op_attr_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), &'static str> {
    if keys.into_iter().any(|key| key == "_original_kind") {
        Err("legacy _original_kind attribute is forbidden; use the typed origin field")
    } else {
        Ok(())
    }
}

fn deserialize_op_attrs<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<AttrDict, D::Error> {
    let attrs = <AttrDict as serde::Deserialize>::deserialize(decoder)?;
    validate_op_attr_keys(attrs.keys().map(String::as_str)).map_err(serde::de::Error::custom)?;
    Ok(attrs)
}

/// Canonical TIR marker for an exception observation that must also service
/// pending calls and the eval breaker at a Python asynchronous-work boundary.
pub const ASYNC_WORK_POLL_ATTR: &str = "async_work_poll";
/// LocationId that an Origin check's cold edge stores before capture
/// (call_anatomy.md, "Location publication cost", M1).
pub const FRAME_LOCATION_ATTR: &str = "frame_location";
/// Proof carried into final TIR: every integer result of this op fits the
/// inline-int47 window. The late lowering stage records it before it splits
/// blocks, because the range analysis that found the proof reads loop shape
/// and the split hides that shape from a later run.
pub const INLINE_INT_PROOF_ATTR: &str = "inline_int_proof";
/// Proof carried into final TIR: this op cannot raise. The late lowering
/// stage records it on the ops that replace an op which could not raise (an
/// add of two exact ints), whose exception check is already gone.
pub const NOTHROW_PROOF_ATTR: &str = "nothrow_proof";
/// Proof carried into final TIR: the exact scalar type of this op's result,
/// as a closed tag (`exact_result_proof`). The late lowering stage records it
/// on the runtime call that replaces an op whose result type was exact; the
/// call's own contract says only "an object".
pub const EXACT_RESULT_PROOF_ATTR: &str = "exact_result_proof";

/// Attribute projection of a call's typed per-operand custody: the
/// [`ParameterCustody::encode`] bytes, one per operand, aligned with
/// `operands`. Absent means the op borrows every operand.
pub const ARGUMENT_CUSTODY_ATTR: &str = "argument_custody";
/// The two attributes of a module global slot (`OpIR::global_slot`). One
/// typed accessor pair reads and writes them; nothing else spells them.
const GLOBAL_SLOT_MODULE_ATTR: &str = "global_slot_module";
const GLOBAL_SLOT_INDEX_ATTR: &str = "global_slot_index";

/// Executable builtin identity, separate from positional arguments. A name
/// selects runtime builtin lookup; it does not prove a fixed callable, purity,
/// argument admission, or the result type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCallTarget<'a, T> {
    Named(&'a str),
    Dynamic(&'a T),
}

#[derive(Debug, Clone, Copy)]
pub struct BuiltinCallView<'a, T> {
    pub target: BuiltinCallTarget<'a, T>,
    pub arguments: &'a [T],
    /// Specialized source operations retain their backend-supported spelling.
    /// In particular, range_new is a primitive, unlike mutable builtin range.
    pub wire_kind: crate::OpKind,
}

impl<'a, T> BuiltinCallView<'a, T> {
    pub fn named_target(&self) -> Option<&'a str> {
        match &self.target {
            BuiltinCallTarget::Named(name) => Some(*name),
            BuiltinCallTarget::Dynamic(_) => None,
        }
    }
}

/// The sole CallBuiltin identity/operand convention. Named metadata carries
/// every positional argument in operands; a name is NEVER duplicated in slot
/// zero. Only metadata-free dynamic lookup takes its name from operand zero.
/// This generic projection also serves analyses over operand facts.
pub fn builtin_call_view<'a, T>(
    opcode: OpCode,
    origin: Option<crate::OpKind>,
    attrs: &'a AttrDict,
    operands: &'a [T],
) -> Option<BuiltinCallView<'a, T>> {
    if opcode != OpCode::CallBuiltin
        || [
            "callee",
            "runtime_symbol",
            "native_callable_symbol",
            "native_callable_export",
        ]
        .iter()
        .any(|key| attrs.contains_key(*key))
    {
        return None;
    }
    let string = |key: &str| -> Option<Option<&'a str>> {
        match attrs.get(key) {
            None => Some(None),
            Some(AttrValue::Str(value)) if !value.is_empty() => Some(Some(value.as_str())),
            _ => None,
        }
    };
    let (name, spelling) = (string("name")?, string("s_value")?);
    if name
        .zip(spelling)
        .is_some_and(|(name, spelling)| name != spelling)
    {
        return None;
    }
    let name = name.or(spelling);
    let (target, arguments, wire_kind) = match origin {
        Some(crate::OpKind::RangeNew)
            if name.is_none_or(|name| name == "range") && operands.len() == 3 =>
        {
            (
                BuiltinCallTarget::Named("range"),
                operands,
                crate::OpKind::RangeNew,
            )
        }
        Some(kind @ (crate::OpKind::Print | crate::OpKind::BuiltinPrint))
            if name.is_none_or(|name| {
                name == "print" || (kind == crate::OpKind::BuiltinPrint && name == "builtin_print")
            }) =>
        {
            (BuiltinCallTarget::Named("print"), operands, kind)
        }
        Some(_) => return None,
        None => match name {
            Some(name) => (
                BuiltinCallTarget::Named(name),
                operands,
                crate::OpKind::CallBuiltin,
            ),
            None => {
                let (name, arguments) = operands.split_first()?;
                (
                    BuiltinCallTarget::Dynamic(name),
                    arguments,
                    crate::OpKind::CallBuiltin,
                )
            }
        },
    };
    Some(BuiltinCallView {
        target,
        arguments,
        wire_kind,
    })
}

impl TirOp {
    /// A Molt-dialect op with no attributes, origin, runtime entry or span.
    pub fn new(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> Self {
        Self {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    pub fn exception_source_kind(&self) -> Option<super::op_kinds_generated::ExceptionEdgeKind> {
        match self.attrs.get("exception_source") {
            Some(AttrValue::Int(tag)) => {
                super::op_kinds_generated::ExceptionEdgeKind::from_tag(*tag)
            }
            _ => None,
        }
    }
    pub fn exception_edge_kind(&self) -> Option<super::op_kinds_generated::ExceptionEdgeKind> {
        match self.attrs.get("exception_edge") {
            Some(AttrValue::Int(tag)) => {
                super::op_kinds_generated::ExceptionEdgeKind::from_tag(*tag)
            }
            _ => None,
        }
    }
    pub fn set_exception_edge_kind(&mut self, kind: super::op_kinds_generated::ExceptionEdgeKind) {
        self.attrs
            .insert("exception_edge".into(), AttrValue::Int(kind as i64));
    }

    /// The site LocationId an Origin check materializes on its cold edges.
    pub fn frame_location(&self) -> Option<i64> {
        match self.attrs.get(FRAME_LOCATION_ATTR) {
            Some(AttrValue::Int(location)) => Some(*location),
            _ => None,
        }
    }

    pub fn set_frame_location(&mut self, location: i64) {
        self.attrs
            .insert(FRAME_LOCATION_ATTR.into(), AttrValue::Int(location));
    }

    pub fn with_origin(mut self, origin: crate::OpKind) -> Self {
        self.origin = Some(origin);
        self
    }

    /// The runtime entry that implements this numeric op on boxed operands:
    /// the entry of its wire identity (its origin, else its opcode's canonical
    /// kind). An in-place opcode or origin thus selects `molt_inplace_*`, which
    /// tries `__i<op>__` before the binary chain, as CPython requires; alias
    /// spellings (`shl`, `unary_neg`) share their entry. One rule for every
    /// backend and the late lowering stage.
    pub fn numeric_runtime(&self) -> Option<crate::runtime_fn_generated::RuntimeFn> {
        use crate::runtime_fn_generated::numeric_runtime_fn;
        self.origin.and_then(numeric_runtime_fn).or_else(|| {
            super::op_kinds_generated::opcode_canonical_kind_table(self.opcode)
                .and_then(numeric_runtime_fn)
        })
    }

    pub fn builtin_call(&self) -> Option<BuiltinCallView<'_, ValueId>> {
        (self.has_valid_shape() && self.results.len() <= 1)
            .then(|| builtin_call_view(self.opcode, self.origin, &self.attrs, &self.operands))
            .flatten()
    }

    /// The operand observed by the explicit length primitive. A mutable
    /// public call named len does not establish a length observation.
    pub fn length_argument(&self) -> Option<ValueId> {
        if !self.has_valid_shape() || self.results.len() != 1 {
            return None;
        }
        if self.opcode == OpCode::Copy
            && self.origin == Some(crate::OpKind::Len)
            && let [argument] = self.operands.as_slice()
        {
            return Some(*argument);
        }
        None
    }

    /// A direct fixed-offset field load: one object operand, one result,
    /// nonnegative byte offset, and no Python attribute dispatch. Guarded and
    /// generic forms may fall back to the full attribute protocol and are not
    /// admitted by this projection. A fused async-work poll is also excluded.
    pub fn plain_typed_slot_load(&self) -> Option<(ValueId, i64)> {
        if self.opcode != OpCode::LoadAttr
            || self.operands.len() != 1
            || self.results.len() != 1
            || self.is_async_work_poll()
        {
            return None;
        }
        if self.origin != Some(crate::OpKind::Load) {
            return None;
        }
        match self.attrs.get("value") {
            Some(AttrValue::Int(offset)) if *offset >= 0 => Some((self.operands[0], *offset)),
            _ => None,
        }
    }

    /// A direct fixed-offset field store: `[object, value]`, nonnegative byte
    /// offset, and no Python attribute dispatch. Guarded/generic stores have
    /// different operand contracts and are not admitted by this projection.
    /// Fused async-work polls are not plain stores: eliminating or forwarding
    /// through them would discard an operand-independent callback boundary.
    pub fn plain_typed_slot_store(&self) -> Option<(ValueId, i64)> {
        if self.opcode != OpCode::StoreAttr
            || self.operands.len() != 2
            || !self.results.is_empty()
            || self.is_async_work_poll()
        {
            return None;
        }
        if self.origin != Some(crate::OpKind::Store) {
            return None;
        }
        match self.attrs.get("value") {
            Some(AttrValue::Int(offset)) if *offset >= 0 => Some((self.operands[0], *offset)),
            _ => None,
        }
    }

    pub fn source_site(&self) -> Option<SourceSite> {
        SourceSite::from_attrs(&self.attrs)
    }

    pub fn set_source_site(&mut self, site: SourceSite) {
        site.write_attrs(&mut self.attrs);
    }

    pub fn source_op_index(&self) -> Option<usize> {
        let Some(AttrValue::Int(value)) = self.attrs.get(SOURCE_OP_INDEX_ATTR) else {
            return None;
        };
        usize::try_from(*value).ok()
    }

    pub fn set_source_op_index(&mut self, op_idx: usize) {
        let value = i64::try_from(op_idx).expect("source op index exceeds i64");
        self.attrs
            .insert(SOURCE_OP_INDEX_ATTR.into(), AttrValue::Int(value));
    }

    pub fn inherit_source_from(&mut self, other: &TirOp) {
        self.source_span = other.source_span;
        if let Some(site) = other.source_site() {
            self.set_source_site(site);
        }
        if let Some(op_idx) = other.source_op_index() {
            self.set_source_op_index(op_idx);
        }
    }

    /// Custody of the operand at `index`. This op adopts a `Transferred`
    /// operand's reference on its normal and its exceptional continuation,
    /// whether or not a callee runs. A projection that does not name every
    /// operand is malformed and is never guessed.
    pub fn operand_custody(&self, index: usize) -> ParameterCustody {
        let arity = self.operands.len();
        let custody = match self.attrs.get(ARGUMENT_CUSTODY_ATTR) {
            None => return ParameterCustody::Borrowed,
            Some(AttrValue::Bytes(encoded)) => ParameterCustody::decode(encoded, arity, index),
            Some(_) => None,
        };
        custody.unwrap_or_else(|| {
            panic!(
                "{:?}: malformed {ARGUMENT_CUSTODY_ATTR} for operand {index} of {arity}",
                self.opcode
            )
        })
    }

    /// Every operand's custody in operand order, or `None` when this op
    /// borrows every operand: the SimpleIR `argument_custody` projection.
    /// The module global slot this op reads or stores (call_anatomy.md 4.5).
    pub fn global_slot(&self) -> Option<crate::ir::GlobalSlotRef> {
        let module = match self.attrs.get(GLOBAL_SLOT_MODULE_ATTR)? {
            AttrValue::Str(module) => module.clone(),
            other => panic!("{:?}: malformed global slot module {other:?}", self.opcode),
        };
        let index = match self.attrs.get(GLOBAL_SLOT_INDEX_ATTR) {
            Some(&AttrValue::Int(index)) => u32::try_from(index).unwrap_or_else(|_| {
                panic!("{:?}: global slot index {index} out of range", self.opcode)
            }),
            other => panic!("{:?}: malformed global slot index {other:?}", self.opcode),
        };
        Some(crate::ir::GlobalSlotRef { module, index })
    }

    pub fn set_global_slot(&mut self, slot: Option<&crate::ir::GlobalSlotRef>) {
        match slot {
            Some(slot) => {
                self.attrs.insert(
                    GLOBAL_SLOT_MODULE_ATTR.into(),
                    AttrValue::Str(slot.module.clone()),
                );
                self.attrs.insert(
                    GLOBAL_SLOT_INDEX_ATTR.into(),
                    AttrValue::Int(i64::from(slot.index)),
                );
            }
            None => {
                self.attrs.remove(GLOBAL_SLOT_MODULE_ATTR);
                self.attrs.remove(GLOBAL_SLOT_INDEX_ATTR);
            }
        }
    }

    pub fn argument_custody(&self) -> Option<Vec<ParameterCustody>> {
        self.attrs.contains_key(ARGUMENT_CUSTODY_ATTR).then(|| {
            (0..self.operands.len())
                .map(|index| self.operand_custody(index))
                .collect()
        })
    }

    /// Project `custody`, one entry per operand. All-borrowed custody is the
    /// absent attribute, so the fact has one encoding.
    pub fn set_argument_custody(&mut self, custody: &[ParameterCustody]) {
        assert_eq!(
            custody.len(),
            self.operands.len(),
            "argument custody must name every operand"
        );
        match ParameterCustody::encode(custody) {
            Some(encoded) => {
                self.attrs
                    .insert(ARGUMENT_CUSTODY_ATTR.into(), AttrValue::Bytes(encoded));
            }
            None => {
                self.attrs.remove(ARGUMENT_CUSTODY_ATTR);
            }
        }
    }

    /// Whether this op carries the late stage's proof that its integer results
    /// fit the inline-int47 window.
    #[inline]
    pub fn has_inline_int_proof(&self) -> bool {
        matches!(
            self.attrs.get(INLINE_INT_PROOF_ATTR),
            Some(AttrValue::Bool(true))
        )
    }

    /// Record that every integer result of this op fits the inline-int47
    /// window. Only the late lowering stage may call it, with the proof in hand.
    pub fn mark_inline_int_proof(&mut self) {
        self.attrs
            .insert(INLINE_INT_PROOF_ATTR.into(), AttrValue::Bool(true));
    }

    /// Whether this operation is also the canonical pending-call/eval-breaker
    /// observation. Ordinary boundaries fuse the poll with CheckException;
    /// generated `finally` arbitration fuses it with the pending-object observer
    /// so exception replacement and context chaining remain on the authored path.
    #[inline]
    pub fn is_async_work_poll(&self) -> bool {
        matches!(
            self.attrs.get(ASYNC_WORK_POLL_ATTR),
            Some(AttrValue::Bool(true))
        )
    }

    /// Whether this op carries the late stage's proof that it cannot raise.
    #[inline]
    pub fn has_nothrow_proof(&self) -> bool {
        matches!(
            self.attrs.get(NOTHROW_PROOF_ATTR),
            Some(AttrValue::Bool(true))
        )
    }

    /// Record that this op cannot raise. Only the late lowering stage may
    /// call it, on an op that replaces one which could not raise.
    pub fn mark_nothrow_proof(&mut self) {
        self.attrs
            .insert(NOTHROW_PROOF_ATTR.into(), AttrValue::Bool(true));
    }

    /// The exact scalar type the late stage proved for this op's result.
    pub fn exact_result_proof(&self) -> Option<TirType> {
        match self.attrs.get(EXACT_RESULT_PROOF_ATTR) {
            Some(AttrValue::Int(0)) => Some(TirType::I64),
            Some(AttrValue::Int(1)) => Some(TirType::BigInt),
            Some(AttrValue::Int(2)) => Some(TirType::Bool),
            Some(AttrValue::Int(3)) => Some(TirType::F64),
            _ => None,
        }
    }

    /// Record the exact scalar type of this op's result. Returns false, and
    /// records nothing, for a type that is not an exact scalar.
    pub fn mark_exact_result_proof(&mut self, ty: &TirType) -> bool {
        let tag = match ty {
            TirType::I64 => 0,
            TirType::BigInt => 1,
            TirType::Bool => 2,
            TirType::F64 => 3,
            _ => return false,
        };
        self.attrs
            .insert(EXACT_RESULT_PROOF_ATTR.into(), AttrValue::Int(tag));
        true
    }

    /// Whether lowering this operation reads the thread's pending-work word:
    /// a poll, or the `AsyncWorkPending` test. A function with such an
    /// operation materializes the word's address once, at entry.
    pub fn reads_pending_work_word(&self) -> bool {
        self.is_async_work_poll() || self.opcode == OpCode::AsyncWorkPending
    }

    /// Whether this operation role can fuse an asynchronous-work poll without
    /// changing its control or value semantics.
    #[inline]
    pub fn can_carry_async_work_poll(&self) -> bool {
        self.opcode == OpCode::CheckException
            || (self.opcode == OpCode::Copy
                && self
                    .origin
                    .is_some_and(crate::OpKind::may_carry_async_work_poll_marker))
    }

    /// Fuse the canonical asynchronous-work poll into this observation.
    /// Returns whether the semantic fact changed.
    pub fn mark_async_work_poll(&mut self) -> bool {
        assert!(
            self.can_carry_async_work_poll(),
            "only an async-work observation carrier may carry the poll marker"
        );
        if self.is_async_work_poll() {
            return false;
        }
        self.attrs
            .insert(ASYNC_WORK_POLL_ATTR.into(), AttrValue::Bool(true));
        if self.opcode == OpCode::CheckException && self.exception_edge_kind().is_none() {
            self.set_exception_edge_kind(super::op_kinds_generated::ExceptionEdgeKind::Origin);
        }
        true
    }

    /// Retire only the asynchronous-work role, preserving the underlying
    /// exception observation, its SSA payload, and its exceptional successor.
    /// Redundant synchronous checks belong to exception-check elimination.
    pub fn clear_async_work_poll(&mut self) -> bool {
        let changed = self.is_async_work_poll();
        self.attrs.remove(ASYNC_WORK_POLL_ATTR);
        changed
    }

    /// True only for a structural SSA value copy.
    ///
    /// `OpCode::Copy` is also the legacy fallback carrier for SimpleIR
    /// operations that have not yet been promoted into first-class TIR
    /// opcodes. Those carriers keep a typed `origin` and must not be
    /// propagated, hoisted, vectorized, or
    /// otherwise treated as transparent copies.
    #[inline]
    pub fn is_plain_value_copy(&self) -> bool {
        self.opcode == OpCode::Copy
            && self.operands.len() == 1
            && self.results.len() == 1
            && self.origin.is_none()
            && self.attrs.is_empty()
    }

    /// Admit operand and result counts using the same generated schema as
    /// verification and exact result inference. Payload-dependent shapes still
    /// require their instance-specific checks.
    #[inline]
    pub fn has_valid_shape(&self) -> bool {
        super::op_kinds_generated::opcode_accepts_shape(
            self.opcode,
            self.operands.len(),
            self.results.len(),
        )
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
        origin: None,
        runtime: None,
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
    use crate::OpKind;

    #[test]
    fn builtin_call_has_one_identity_and_argument_authority() {
        let values = [ValueId(0), ValueId(1), ValueId(2)];
        let mut attrs = AttrDict::from([("name".into(), AttrValue::Str("len".into()))]);
        for count in 0..=3 {
            let call =
                builtin_call_view(OpCode::CallBuiltin, None, &attrs, &values[..count]).unwrap();
            assert_eq!(call.named_target(), Some("len"));
            assert_eq!(
                call.arguments,
                &values[..count],
                "named calls never discard operand zero"
            );
            assert_eq!(call.wire_kind, crate::OpKind::CallBuiltin);
        }
        attrs.clear();
        let call = builtin_call_view(OpCode::CallBuiltin, None, &attrs, &values).unwrap();
        assert_eq!(call.target, BuiltinCallTarget::Dynamic(&values[0]));
        assert_eq!(call.arguments, &values[1..]);
        assert!(builtin_call_view::<ValueId>(OpCode::CallBuiltin, None, &attrs, &[]).is_none());
        assert!(builtin_call_view(OpCode::Call, None, &attrs, &values).is_none());
        for kind in [
            crate::OpKind::RangeNew,
            crate::OpKind::Print,
            crate::OpKind::BuiltinPrint,
        ] {
            let name = if kind == crate::OpKind::RangeNew {
                "range"
            } else {
                "print"
            };
            attrs = AttrDict::from([
                ("name".into(), AttrValue::Str(name.into())),
                ("s_value".into(), AttrValue::Str(name.into())),
            ]);
            let call = builtin_call_view(OpCode::CallBuiltin, Some(kind), &attrs, &values).unwrap();
            assert_eq!(call.named_target(), Some(name));
            assert_eq!(call.wire_kind, kind);
            assert_eq!(call.arguments, values);
        }
        attrs.clear();
        for count in 0..3 {
            assert!(
                builtin_call_view(
                    OpCode::CallBuiltin,
                    Some(crate::OpKind::RangeNew),
                    &attrs,
                    &values[..count]
                )
                .is_none()
            );
        }
    }

    #[test]
    fn builtin_call_rejects_conflicting_or_foreign_dispatch_metadata() {
        let operands = [ValueId(1)];
        for attrs in [
            AttrDict::from([("name".into(), AttrValue::Int(1))]),
            AttrDict::from([("name".into(), AttrValue::Str(String::new()))]),
            AttrDict::from([
                ("name".into(), AttrValue::Str("len".into())),
                ("s_value".into(), AttrValue::Str("print".into())),
            ]),
            AttrDict::from([
                ("name".into(), AttrValue::Str("len".into())),
                ("callee".into(), AttrValue::Str("evil".into())),
            ]),
        ] {
            assert!(
                builtin_call_view(OpCode::CallBuiltin, None, &attrs, &operands).is_none(),
                "{attrs:?}"
            );
        }
    }

    #[test]
    fn unknown_and_retired_wire_spellings_fail_at_decode() {
        for spelling in [
            "unknown",
            "store_init",
            "guarded_field_init",
            "unknown_builtin",
        ] {
            let error =
                serde_json::from_value::<crate::OpKind>(serde_json::json!(spelling)).unwrap_err();
            assert!(error.to_string().contains(spelling));
        }
        let attrs = AttrDict::from([("name".into(), AttrValue::Str("len".into()))]);
        for origin in [crate::OpKind::Print, crate::OpKind::Store] {
            assert!(
                builtin_call_view(OpCode::CallBuiltin, Some(origin), &attrs, &[ValueId(1)])
                    .is_none()
            );
        }
    }

    #[test]
    fn length_projection_uses_identity_not_an_annotation() {
        let mut op = dead_placeholder_const_for_type(&TirType::I64, ValueId(2));
        op.opcode = OpCode::CallBuiltin;
        op.operands = vec![ValueId(1)];
        op.attrs = AttrDict::from([("name".into(), AttrValue::Str("len".into()))]);
        assert_eq!(op.length_argument(), None);
        op.origin = Some(OpKind::Print);
        assert_eq!(op.length_argument(), None);
        op.opcode = OpCode::Copy;
        op.attrs.clear();
        op.origin = Some(crate::OpKind::Len);
        assert_eq!(op.length_argument(), Some(ValueId(1)));
        op.results.push(ValueId(3));
        assert_eq!(op.length_argument(), None);
    }

    #[test]
    fn retiring_poll_role_preserves_exception_payload_and_source() {
        let original = TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![ValueId(7), ValueId(8)],
            results: vec![],
            attrs: AttrDict::from([
                ("value".into(), AttrValue::Int(90)),
                (
                    "exception_edge".into(),
                    AttrValue::Int(
                        super::super::op_kinds_generated::ExceptionEdgeKind::Origin as i64,
                    ),
                ),
            ]),
            source_span: Some((10, 20)),
        };
        let mut observation = original.clone();
        assert!(observation.mark_async_work_poll());
        assert!(observation.clear_async_work_poll());
        assert!(!observation.clear_async_work_poll());
        assert_eq!(observation.opcode, original.opcode);
        assert_eq!(observation.dialect, original.dialect);
        assert_eq!(observation.operands, original.operands);
        assert_eq!(observation.results, original.results);
        assert_eq!(observation.attrs, original.attrs);
        assert_eq!(observation.source_span, original.source_span);
    }

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

    #[test]
    fn inherit_source_from_copies_source_op_index() {
        let mut source = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(1));
        source.set_source_op_index(37);
        let mut replacement = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(2));

        replacement.inherit_source_from(&source);

        assert_eq!(replacement.source_op_index(), Some(37));
    }

    #[test]
    fn plain_slot_store_requires_complete_direct_offset_contract() {
        let mut op = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(2));
        op.opcode = OpCode::StoreAttr;
        op.operands = vec![ValueId(0), ValueId(1)];
        op.results.clear();
        op.origin = Some(OpKind::Store);
        op.attrs.insert("value".into(), AttrValue::Int(8));
        assert_eq!(op.plain_typed_slot_store(), Some((ValueId(0), 8)));
        op.attrs.remove("value");
        assert_eq!(op.plain_typed_slot_store(), None);
        op.attrs.insert("value".into(), AttrValue::Int(-1));
        assert_eq!(op.plain_typed_slot_store(), None);
        op.attrs.insert("value".into(), AttrValue::Int(0));
        for kind in [
            crate::OpKind::GuardedFieldSet,
            crate::OpKind::SetAttrGenericPtr,
            crate::OpKind::Add,
        ] {
            op.origin = Some(kind);
            assert_eq!(op.plain_typed_slot_store(), None);
        }
        op.origin = Some(OpKind::Store);
        op.operands.push(ValueId(3));
        assert_eq!(op.plain_typed_slot_store(), None);
        op.operands.pop();
        op.results.push(ValueId(4));
        assert_eq!(
            op.plain_typed_slot_store(),
            None,
            "a result-producing StoreAttr is not the statement-only slot store"
        );
    }

    #[test]
    fn plain_slot_store_rejects_generic_origins() {
        let mut op = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(2));
        op.opcode = OpCode::StoreAttr;
        op.operands = vec![ValueId(0), ValueId(1)];
        op.results.clear();
        op.attrs.insert("value".into(), AttrValue::Int(8));

        op.origin = Some(OpKind::Store);
        assert_eq!(op.plain_typed_slot_store(), Some((ValueId(0), 8)));
        for kind in [crate::OpKind::GuardedFieldSet, crate::OpKind::SetAttr] {
            op.origin = Some(kind);
            assert_eq!(op.plain_typed_slot_store(), None);
        }
    }

    #[test]
    fn plain_slot_load_requires_complete_direct_offset_contract() {
        let mut op = dead_placeholder_const_for_type(&TirType::DynBox, ValueId(1));
        op.opcode = OpCode::LoadAttr;
        op.operands = vec![ValueId(0)];
        op.results = vec![ValueId(1)];
        op.origin = Some(OpKind::Load);
        op.attrs.insert("value".into(), AttrValue::Int(8));
        assert_eq!(op.plain_typed_slot_load(), Some((ValueId(0), 8)));

        op.attrs.remove("value");
        assert_eq!(op.plain_typed_slot_load(), None);
        op.attrs.insert("value".into(), AttrValue::Int(-1));
        assert_eq!(op.plain_typed_slot_load(), None);
        op.attrs.insert("value".into(), AttrValue::Int(0));
        for kind in [
            crate::OpKind::GuardedFieldGet,
            crate::OpKind::GetAttr,
            crate::OpKind::Add,
        ] {
            op.origin = Some(kind);
            assert_eq!(op.plain_typed_slot_load(), None);
        }

        op.origin = Some(OpKind::Load);
        op.operands.push(ValueId(2));
        assert_eq!(op.plain_typed_slot_load(), None);
        op.operands.pop();
        op.results.clear();
        assert_eq!(op.plain_typed_slot_load(), None);
    }
}
