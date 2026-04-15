//! The 26 tinygrad-conformant primitive ops.
//!
//! 1:1 with tinygrad's CStyleLanguage.code_for_op backend contract.
//! No fewer, no more.

/// Categorization of ops for fusion analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpType {
    Unary,
    Binary,
    Ternary,
    Reduce,
}

/// The 26 primitive compute ops.
///
/// Every GPU kernel is built from these ops and nothing else.
/// Compositions (exp, log, sigmoid, softmax, matmul, etc.) are
/// expressed as DAGs of these primitives in the LazyOp layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveOp {
    // --- Arithmetic (6) ---
    /// `a + b`
    Add,
    /// `a - b` (primitive, NOT Add(a, Neg(b)) — distinct for -0.0)
    Sub,
    /// `a * b`
    Mul,
    /// `a / b` (integer division, truncates toward zero — C semantics)
    Idiv,
    /// `a % b` (result has sign of dividend — C semantics: (-7) % 3 = -1)
    Mod,
    /// `-a` (NOT a * -1 — different for -0.0, NaN sign bit)
    Neg,

    // --- Comparison (3) ---
    /// `a < b ? 1 : 0` — output dtype is always Bool.
    /// NaN: NaN < x = false (IEEE 754 unordered comparison).
    Cmplt,
    /// `a == b ? 1 : 0` — output dtype is always Bool.
    /// NaN: NaN == NaN = false (IEEE 754).
    Cmpeq,
    /// `a != b ? 1 : 0` — output dtype is always Bool.
    /// NaN: NaN != NaN = true (IEEE 754).
    Cmpne,

    // --- Bitwise (5) ---
    /// `a & b`
    And,
    /// `a | b`
    Or,
    /// `a ^ b`
    Xor,
    /// `a << b` (logical left shift)
    Shl,
    /// `a >> b` (arithmetic right shift for signed: sign-extending.
    /// Logical right shift for unsigned: zero-filling.)
    Shr,

    // --- Math (5) ---
    /// `exp2(a)`
    Exp2,
    /// `log2(a)`
    Log2,
    /// `sin(a)`
    Sin,
    /// `sqrt(a)`
    Sqrt,
    /// `1.0 / a` (float-only. RECIPROCAL(0.0) = +inf, RECIPROCAL(-0.0) = -inf per IEEE 754.
    /// Not valid for integer types — use Idiv(1, a) instead.)
    Reciprocal,

    // --- Other (4) ---
    /// `trunc(a)` — truncate toward zero. Needed for floor/ceil/round compositions.
    Trunc,
    /// `max(a, b)` — IEEE 754: NaN-propagating (if either operand is NaN, result is NaN).
    /// Maps to fmax in MSL. For integers, standard comparison.
    Max,
    /// `cond ? a : b` — ternary select.
    Where,
    /// Type conversion: `(target_type)a`.
    /// Target dtype is stored in FusedOp.dst_dtype.
    Cast,

    // --- Specialized (3) ---
    /// Reinterpret bits as different type (no conversion).
    /// Target dtype is stored in FusedOp.dst_dtype.
    Bitcast,
    /// `sum(a[i]) over axis` — reduce op.
    ReduceSum,
    /// `max(a[i]) over axis` — reduce op. NaN-propagating for floats.
    ReduceMax,
}

impl PrimitiveOp {
    /// Returns the op type category for fusion analysis.
    pub fn op_type(self) -> OpType {
        match self {
            Self::Neg | Self::Exp2 | Self::Log2 | Self::Sin | Self::Sqrt
            | Self::Reciprocal | Self::Trunc | Self::Cast | Self::Bitcast => OpType::Unary,

            Self::Add | Self::Sub | Self::Mul | Self::Idiv | Self::Mod
            | Self::Cmplt | Self::Cmpeq | Self::Cmpne
            | Self::And | Self::Or | Self::Xor | Self::Shl | Self::Shr
            | Self::Max => OpType::Binary,

            Self::Where => OpType::Ternary,

            Self::ReduceSum | Self::ReduceMax => OpType::Reduce,
        }
    }

    /// Number of source operands this op consumes.
    pub fn arity(self) -> usize {
        match self.op_type() {
            OpType::Unary => 1,
            OpType::Binary => 2,
            OpType::Ternary => 3,
            OpType::Reduce => 1,
        }
    }

    /// Whether this op is elementwise (fuses freely with other elementwise ops).
    pub fn is_elementwise(self) -> bool {
        matches!(self.op_type(), OpType::Unary | OpType::Binary | OpType::Ternary)
    }

    /// All 26 primitive ops in canonical order.
    pub const ALL: [PrimitiveOp; 26] = [
        Self::Add, Self::Sub, Self::Mul, Self::Idiv, Self::Mod, Self::Neg,
        Self::Cmplt, Self::Cmpeq, Self::Cmpne,
        Self::And, Self::Or, Self::Xor, Self::Shl, Self::Shr,
        Self::Exp2, Self::Log2, Self::Sin, Self::Sqrt, Self::Reciprocal,
        Self::Trunc, Self::Max, Self::Where, Self::Cast,
        Self::Bitcast, Self::ReduceSum, Self::ReduceMax,
    ];
}
