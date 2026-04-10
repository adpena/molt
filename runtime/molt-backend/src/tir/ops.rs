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
    // Memory
    Alloc,
    StackAlloc,
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
