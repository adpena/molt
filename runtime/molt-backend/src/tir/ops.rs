use std::collections::HashMap;

use super::values::ValueId;

/// Dialect namespace for operations (MLIR-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpCode {
    // Arithmetic
    Add,
    Sub,
    Mul,
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
    // Call
    Call,
    CallMethod,
    CallBuiltin,
    // Box/unbox
    BoxVal,
    UnboxVal,
    TypeGuard,
    // Refcount
    IncRef,
    DecRef,
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
    TryStart,
    TryEnd,
    StateBlockStart,
    StateBlockEnd,
    // Constants
    ConstInt,
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
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// Attribute dictionary attached to an operation.
pub type AttrDict = HashMap<String, AttrValue>;

/// A single operation in the TIR.
#[derive(Debug, Clone)]
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
