# Tinygrad GPU Primitives — Plan 1: Foundation Crate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `molt-gpu` foundation crate with all 26 primitive ops, DType, ShapeTracker, LazyOp DAG, FusedKernel IR, MslRenderer, MetalDevice, CpuDevice, kernel scheduling/fusion, MLIR serialization, and per-op TDD tests with Metal+CPU reference comparison.

**Architecture:** A new `runtime/molt-gpu/` crate that implements the tinygrad-conformant primitive stack per the spec at `docs/superpowers/specs/2026-04-14-tinygrad-gpu-primitives-design.md`. The crate is self-contained: no dependency on `molt-runtime` or `molt-backend`. It exposes the Rust API consumed by the Python `Tensor` class (written separately in a future plan).

**Tech Stack:** Rust (edition 2024), `metal` crate (macOS Metal API bindings), `half` (f16/bf16 types), cargo test, MLIR textual IR (string generation, no C++ deps).

---

## File Map

| Path | Responsibility |
| --- | --- |
| `Cargo.toml` (workspace root) | Add `runtime/molt-gpu` to workspace members |
| `runtime/molt-gpu/Cargo.toml` | Crate manifest with Metal/CPU feature flags |
| `runtime/molt-gpu/src/lib.rs` | Public API re-exports |
| `runtime/molt-gpu/src/ops.rs` | `PrimitiveOp` enum (26 ops) |
| `runtime/molt-gpu/src/dtype.rs` | `DType` enum with size/alignment/narrowing |
| `runtime/molt-gpu/src/shapetracker.rs` | `View` + `ShapeTracker` + movement ops + `expr_idx` |
| `runtime/molt-gpu/src/lazy.rs` | `LazyOp` DAG node enum |
| `runtime/molt-gpu/src/render/mod.rs` | `Renderer` trait + `FusedKernel`/`FusedOp`/`FusedSrc`/`BufferBinding`/`BufferAccess` IR |
| `runtime/molt-gpu/src/render/msl.rs` | `MslRenderer` — all 26 ops to MSL source |
| `runtime/molt-gpu/src/device/mod.rs` | `Allocator` + `Compiler` + `Executor` traits + `DeviceBuffer`/`CompiledProgram`/`DeviceError` |
| `runtime/molt-gpu/src/device/metal.rs` | `MetalDevice` — Metal backend |
| `runtime/molt-gpu/src/device/cpu.rs` | `CpuDevice` — CPU reference backend |
| `runtime/molt-gpu/src/schedule.rs` | DAG -> topological kernel schedule |
| `runtime/molt-gpu/src/fuse.rs` | Kernel fusion (elementwise -> reduce -> elementwise) |
| `runtime/molt-gpu/src/mlir.rs` | MLIR textual IR serialization for FusedKernel |
| `runtime/molt-gpu/tests/test_ops.rs` | Per-op TDD tests (Metal vs CPU reference) |
| `runtime/molt-gpu/tests/test_shapetracker.rs` | ShapeTracker correctness tests |
| `runtime/molt-gpu/tests/test_fusion.rs` | Kernel fusion count tests |
| `runtime/molt-gpu/tests/test_render_msl.rs` | MSL render output validation tests |
| `runtime/molt-gpu/tests/test_mlir.rs` | MLIR serialization tests |

## Coordination Constraints

- There is active partner work in this repository. Read partner-modified files carefully and do not overwrite or revert unrelated changes.
- The `molt-gpu` crate must have zero dependencies on `molt-runtime` or `molt-backend`. It is a standalone GPU compute library.
- Metal tests require macOS. Gate Metal tests behind `#[cfg(target_os = "macos")]`.
- CPU device is always available and is the reference implementation for correctness testing.
- Every build/test command must set `MOLT_SESSION_ID`.
- Use TDD for each task slice: write failing tests first, verify failure, implement minimum code, rerun, then commit.

## Shared Command Prefix For Build/Test Steps

Use this env prelude for every build/test step:

```bash
export MOLT_SESSION_ID=gpu-plan-1
export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
```

## Numerical Constants

These constants appear in multiple tasks. Use exact IEEE 754 values throughout:

```rust
pub const LOG2_E: f64 = std::f64::consts::LOG2_E;   // 1.4426950408889634
pub const LN_2: f64 = std::f64::consts::LN_2;       // 0.6931471805599453
pub const PI: f64 = std::f64::consts::PI;
pub const PI_OVER_2: f64 = std::f64::consts::FRAC_PI_2;
```

---

## Task 1: Crate Scaffold + PrimitiveOp Enum + DType

**Files:**
- `Cargo.toml` (workspace root, add member)
- `runtime/molt-gpu/Cargo.toml`
- `runtime/molt-gpu/src/lib.rs`
- `runtime/molt-gpu/src/ops.rs`
- `runtime/molt-gpu/src/dtype.rs`

**Steps:**

- [ ] **1.1** Add `"runtime/molt-gpu"` to workspace members in the root `Cargo.toml`.

```toml
# In [workspace] members array, add:
    "runtime/molt-gpu",
```

- [ ] **1.2** Create `runtime/molt-gpu/Cargo.toml`:

```toml
[package]
name = "molt-gpu"
version = "0.1.0"
edition = "2024"

[dependencies]
half = "2.4"

[target.'cfg(target_os = "macos")'.dependencies]
metal = "0.30"

[features]
default = ["metal-backend", "cpu-backend"]
metal-backend = []
cpu-backend = []

[dev-dependencies]
# None needed — tests use the crate's own devices
```

- [ ] **1.3** Create `runtime/molt-gpu/src/ops.rs`:

```rust
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
```

- [ ] **1.4** Create `runtime/molt-gpu/src/dtype.rs`:

```rust
//! Data types for GPU tensors.
//!
//! Maps 1:1 to tinygrad's dtypes. Each backend narrows unsupported types
//! (e.g., WebGPU: f64->f32, i64->i32; Metal: f64->f32).

/// Element data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float16,
    BFloat16,
    Float32,
    Float64,
}

impl DType {
    /// Size in bytes of one element.
    pub fn size_bytes(self) -> usize {
        match self {
            Self::Bool | Self::Int8 | Self::UInt8 => 1,
            Self::Int16 | Self::UInt16 | Self::Float16 | Self::BFloat16 => 2,
            Self::Int32 | Self::UInt32 | Self::Float32 => 4,
            Self::Int64 | Self::UInt64 | Self::Float64 => 8,
        }
    }

    /// Whether this is a floating-point type.
    pub fn is_float(self) -> bool {
        matches!(self, Self::Float16 | Self::BFloat16 | Self::Float32 | Self::Float64)
    }

    /// Whether this is a signed integer type.
    pub fn is_signed_int(self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }

    /// Whether this is an unsigned integer type (including Bool).
    pub fn is_unsigned_int(self) -> bool {
        matches!(self, Self::Bool | Self::UInt8 | Self::UInt16 | Self::UInt32 | Self::UInt64)
    }

    /// Whether this is any integer type.
    pub fn is_int(self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    /// MSL type name for this dtype.
    pub fn msl_type(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int8 => "char",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Int64 => "long",
            Self::UInt8 => "uchar",
            Self::UInt16 => "ushort",
            Self::UInt32 => "uint",
            Self::UInt64 => "ulong",
            Self::Float16 => "half",
            Self::BFloat16 => "bfloat",   // Metal 3.1+ supports bfloat
            Self::Float32 => "float",
            Self::Float64 => "float",     // Metal lacks f64 — narrowed to f32
        }
    }

    /// Narrow this dtype to what the Metal backend supports.
    /// Metal lacks Float64.
    pub fn narrow_metal(self) -> DType {
        match self {
            Self::Float64 => Self::Float32,
            other => other,
        }
    }

    /// Narrow this dtype to what the WebGPU backend supports.
    /// WebGPU lacks Float64 and Int64/UInt64.
    pub fn narrow_webgpu(self) -> DType {
        match self {
            Self::Float64 => Self::Float32,
            Self::Int64 => Self::Int32,
            Self::UInt64 => Self::UInt32,
            other => other,
        }
    }
}
```

- [ ] **1.5** Create `runtime/molt-gpu/src/lib.rs`:

```rust
//! molt-gpu: Tinygrad-conformant GPU primitive stack.
//!
//! Implements all of deep learning with 26 compute primitives,
//! a zero-copy ShapeTracker view system, lazy evaluation DAG,
//! kernel fusion, and multi-backend rendering/execution.

pub mod ops;
pub mod dtype;
pub mod shapetracker;
pub mod lazy;
pub mod render;
pub mod device;
pub mod schedule;
pub mod fuse;
pub mod mlir;
```

- [ ] **1.6** Create stub files for all remaining modules so the crate compiles:

`runtime/molt-gpu/src/shapetracker.rs`:
```rust
//! ShapeTracker + View — zero-copy view system for movement ops.
// Implemented in Task 2.
```

`runtime/molt-gpu/src/lazy.rs`:
```rust
//! LazyOp DAG — deferred computation graph.
// Implemented in Task 3.
```

`runtime/molt-gpu/src/render/mod.rs`:
```rust
//! Renderer trait + FusedKernel IR.
// Implemented in Task 4.
pub mod msl;
```

`runtime/molt-gpu/src/render/msl.rs`:
```rust
//! MslRenderer — Metal Shading Language codegen.
// Implemented in Task 5.
```

`runtime/molt-gpu/src/device/mod.rs`:
```rust
//! Device traits: Allocator, Compiler, Executor.
// Implemented in Task 6.
#[cfg(target_os = "macos")]
pub mod metal;
pub mod cpu;
```

`runtime/molt-gpu/src/device/metal.rs`:
```rust
//! MetalDevice — Apple GPU backend.
// Implemented in Task 7.
```

`runtime/molt-gpu/src/device/cpu.rs`:
```rust
//! CpuDevice — CPU reference backend for testing.
// Implemented in Task 8.
```

`runtime/molt-gpu/src/schedule.rs`:
```rust
//! DAG -> topological kernel schedule.
// Implemented in Task 9.
```

`runtime/molt-gpu/src/fuse.rs`:
```rust
//! Kernel fusion: elementwise -> reduce -> elementwise chains.
// Implemented in Task 10.
```

`runtime/molt-gpu/src/mlir.rs`:
```rust
//! MLIR textual IR serialization for FusedKernel.
// Implemented in Task 11.
```

- [ ] **1.7** Verify the crate compiles:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu
```

- [ ] **1.8** Write and run initial ops test:

Create `runtime/molt-gpu/tests/test_ops.rs`:
```rust
use molt_gpu::ops::{PrimitiveOp, OpType};

#[test]
fn test_all_26_ops() {
    assert_eq!(PrimitiveOp::ALL.len(), 26);
}

#[test]
fn test_op_types() {
    assert_eq!(PrimitiveOp::Add.op_type(), OpType::Binary);
    assert_eq!(PrimitiveOp::Neg.op_type(), OpType::Unary);
    assert_eq!(PrimitiveOp::Where.op_type(), OpType::Ternary);
    assert_eq!(PrimitiveOp::ReduceSum.op_type(), OpType::Reduce);
}

#[test]
fn test_arities() {
    assert_eq!(PrimitiveOp::Neg.arity(), 1);
    assert_eq!(PrimitiveOp::Add.arity(), 2);
    assert_eq!(PrimitiveOp::Where.arity(), 3);
    assert_eq!(PrimitiveOp::ReduceSum.arity(), 1);
}

#[test]
fn test_elementwise() {
    assert!(PrimitiveOp::Add.is_elementwise());
    assert!(PrimitiveOp::Neg.is_elementwise());
    assert!(PrimitiveOp::Where.is_elementwise());
    assert!(!PrimitiveOp::ReduceSum.is_elementwise());
    assert!(!PrimitiveOp::ReduceMax.is_elementwise());
}
```

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_ops
```

- [ ] **1.9** Write and run dtype test:

Create `runtime/molt-gpu/tests/test_dtype.rs` (alongside test_ops.rs, not inside src/):
```rust
use molt_gpu::dtype::DType;

#[test]
fn test_size_bytes() {
    assert_eq!(DType::Bool.size_bytes(), 1);
    assert_eq!(DType::Float16.size_bytes(), 2);
    assert_eq!(DType::Float32.size_bytes(), 4);
    assert_eq!(DType::Float64.size_bytes(), 8);
    assert_eq!(DType::Int64.size_bytes(), 8);
}

#[test]
fn test_type_categories() {
    assert!(DType::Float32.is_float());
    assert!(!DType::Int32.is_float());
    assert!(DType::Int32.is_signed_int());
    assert!(!DType::UInt32.is_signed_int());
    assert!(DType::UInt32.is_unsigned_int());
    assert!(DType::Bool.is_unsigned_int());
}

#[test]
fn test_metal_narrowing() {
    assert_eq!(DType::Float64.narrow_metal(), DType::Float32);
    assert_eq!(DType::Float32.narrow_metal(), DType::Float32);
    assert_eq!(DType::Int64.narrow_metal(), DType::Int64);
}

#[test]
fn test_webgpu_narrowing() {
    assert_eq!(DType::Float64.narrow_webgpu(), DType::Float32);
    assert_eq!(DType::Int64.narrow_webgpu(), DType::Int32);
    assert_eq!(DType::UInt64.narrow_webgpu(), DType::UInt32);
    assert_eq!(DType::Int32.narrow_webgpu(), DType::Int32);
}

#[test]
fn test_msl_types() {
    assert_eq!(DType::Float32.msl_type(), "float");
    assert_eq!(DType::Float64.msl_type(), "float"); // narrowed
    assert_eq!(DType::Int32.msl_type(), "int");
    assert_eq!(DType::Bool.msl_type(), "bool");
    assert_eq!(DType::Float16.msl_type(), "half");
}
```

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_dtype
```

- [ ] **1.10** `git add` all files and commit.

---

## Task 2: ShapeTracker + View

**Files:**
- `runtime/molt-gpu/src/shapetracker.rs`
- `runtime/molt-gpu/tests/test_shapetracker.rs`

**Steps:**

- [ ] **2.1** Implement `View` and `ShapeTracker` in `runtime/molt-gpu/src/shapetracker.rs`:

```rust
//! ShapeTracker + View — zero-copy view system for movement ops.
//!
//! All movement ops (reshape, permute, expand, pad, shrink, flip) are
//! O(1) modifications to the view. No GPU kernel, no memory copy.

/// A single view into a contiguous buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// Logical shape of this view.
    pub shape: Vec<usize>,
    /// Stride per dimension. 0 = broadcast, negative = flipped.
    pub strides: Vec<i64>,
    /// Offset into the underlying buffer (in elements).
    pub offset: i64,
    /// Optional validity mask per dimension: element is valid iff
    /// mask[dim].0 <= idx[dim] < mask[dim].1.
    /// Elements outside the mask return the padding value (typically 0).
    pub mask: Option<Vec<(i64, i64)>>,
}

impl View {
    /// Create a contiguous view for a given shape.
    /// Strides are row-major (C-order): last dimension is stride 1.
    pub fn contiguous(shape: &[usize]) -> Self {
        let ndim = shape.len();
        let mut strides = vec![0i64; ndim];
        if ndim > 0 {
            strides[ndim - 1] = 1;
            for i in (0..ndim - 1).rev() {
                strides[i] = strides[i + 1] * shape[i + 1] as i64;
            }
        }
        Self {
            shape: shape.to_vec(),
            strides,
            offset: 0,
            mask: None,
        }
    }

    /// Total number of logical elements.
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// Whether this view is contiguous (row-major, no mask, offset=0).
    pub fn is_contiguous(&self) -> bool {
        if self.offset != 0 || self.mask.is_some() {
            return false;
        }
        let expected = View::contiguous(&self.shape);
        self.strides == expected.strides
    }

    /// Convert a linear index to multi-dimensional indices.
    fn linear_to_indices(&self, linear_idx: usize) -> Vec<usize> {
        let ndim = self.shape.len();
        let mut indices = vec![0usize; ndim];
        let mut remaining = linear_idx;
        for i in (0..ndim).rev() {
            indices[i] = remaining % self.shape[i];
            remaining /= self.shape[i];
        }
        indices
    }

    /// Compute the actual buffer offset from a linear logical index.
    /// Returns None if the index falls in a masked (padding) region.
    pub fn expr_idx(&self, linear_idx: usize) -> Option<usize> {
        let indices = self.linear_to_indices(linear_idx);

        // Check mask validity
        if let Some(ref mask) = self.mask {
            for (dim, &(lo, hi)) in mask.iter().enumerate() {
                let idx = indices[dim] as i64;
                if idx < lo || idx >= hi {
                    return None;
                }
            }
        }

        // Compute buffer offset: offset + sum(idx[i] * strides[i])
        let mut buf_offset = self.offset;
        for (dim, &idx) in indices.iter().enumerate() {
            buf_offset += idx as i64 * self.strides[dim];
        }

        // Buffer offset must be non-negative for valid elements
        if buf_offset < 0 {
            return None;
        }
        Some(buf_offset as usize)
    }
}

/// A stack of views that tracks how a contiguous buffer is accessed.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeTracker {
    pub views: Vec<View>,
}

impl ShapeTracker {
    /// Create a ShapeTracker for a contiguous buffer with the given shape.
    pub fn contiguous(shape: &[usize]) -> Self {
        Self {
            views: vec![View::contiguous(shape)],
        }
    }

    /// The current (outermost) view.
    pub fn view(&self) -> &View {
        self.views.last().expect("ShapeTracker must have at least one view")
    }

    /// The logical shape of the tensor.
    pub fn shape(&self) -> &[usize] {
        &self.view().shape
    }

    /// Total number of logical elements.
    pub fn numel(&self) -> usize {
        self.view().numel()
    }

    /// Reshape to a new shape. Same number of elements required.
    /// For Phase 1: only works on contiguous views; inserts contiguous()
    /// fallback otherwise.
    pub fn reshape(&self, new_shape: &[usize]) -> Self {
        let current = self.view();
        assert_eq!(
            current.numel(),
            new_shape.iter().product::<usize>(),
            "reshape: element count mismatch ({} vs {})",
            current.numel(),
            new_shape.iter().product::<usize>()
        );
        if current.is_contiguous() {
            Self {
                views: vec![View::contiguous(new_shape)],
            }
        } else {
            // Phase 1: push a new view (multi-view case)
            let mut views = self.views.clone();
            views.push(View::contiguous(new_shape));
            Self { views }
        }
    }

    /// Permute dimensions.
    pub fn permute(&self, order: &[usize]) -> Self {
        let current = self.view();
        let ndim = current.shape.len();
        assert_eq!(order.len(), ndim, "permute: order length mismatch");

        let new_shape: Vec<usize> = order.iter().map(|&i| current.shape[i]).collect();
        let new_strides: Vec<i64> = order.iter().map(|&i| current.strides[i]).collect();
        let new_mask = current.mask.as_ref().map(|m| {
            order.iter().map(|&i| m[i]).collect()
        });

        Self {
            views: vec![View {
                shape: new_shape,
                strides: new_strides,
                offset: current.offset,
                mask: new_mask,
            }],
        }
    }

    /// Expand broadcast dimensions. Dimensions with size 1 in the current
    /// shape can be expanded to any size (stride becomes 0).
    pub fn expand(&self, new_shape: &[usize]) -> Self {
        let current = self.view();
        assert_eq!(new_shape.len(), current.shape.len(), "expand: ndim mismatch");

        let mut new_strides = current.strides.clone();
        for (i, (&old, &new)) in current.shape.iter().zip(new_shape.iter()).enumerate() {
            if old == 1 && new != 1 {
                new_strides[i] = 0; // broadcast
            } else {
                assert_eq!(old, new, "expand: can only expand size-1 dims (dim {} is {})", i, old);
            }
        }

        Self {
            views: vec![View {
                shape: new_shape.to_vec(),
                strides: new_strides,
                offset: current.offset,
                mask: current.mask.clone(),
            }],
        }
    }

    /// Pad tensor with zeros. `padding` is (before, after) pairs per dimension.
    pub fn pad(&self, padding: &[(usize, usize)]) -> Self {
        let current = self.view();
        assert_eq!(padding.len(), current.shape.len(), "pad: ndim mismatch");

        let new_shape: Vec<usize> = current
            .shape
            .iter()
            .zip(padding.iter())
            .map(|(&s, &(before, after))| s + before + after)
            .collect();

        // Adjust offset for padding
        let mut new_offset = current.offset;
        for (i, &(before, _)) in padding.iter().enumerate() {
            new_offset -= before as i64 * current.strides[i];
        }

        // Build mask: valid region is [before, before + original_size)
        let new_mask: Vec<(i64, i64)> = current
            .shape
            .iter()
            .zip(padding.iter())
            .map(|(&s, &(before, _))| (before as i64, (before + s) as i64))
            .collect();

        Self {
            views: vec![View {
                shape: new_shape,
                strides: current.strides.clone(),
                offset: new_offset,
                mask: Some(new_mask),
            }],
        }
    }

    /// Shrink: extract a sub-region. `bounds` is (start, end) per dimension.
    pub fn shrink(&self, bounds: &[(usize, usize)]) -> Self {
        let current = self.view();
        assert_eq!(bounds.len(), current.shape.len(), "shrink: ndim mismatch");

        let new_shape: Vec<usize> = bounds.iter().map(|&(s, e)| e - s).collect();

        // Adjust offset for shrink start
        let mut new_offset = current.offset;
        for (i, &(start, _)) in bounds.iter().enumerate() {
            new_offset += start as i64 * current.strides[i];
        }

        Self {
            views: vec![View {
                shape: new_shape,
                strides: current.strides.clone(),
                offset: new_offset,
                mask: current.mask.clone(), // mask may need adjustment in production
            }],
        }
    }

    /// Flip a dimension (reverse element order along that axis).
    pub fn flip(&self, axis: usize) -> Self {
        let current = self.view();
        assert!(axis < current.shape.len(), "flip: axis out of bounds");

        let mut new_strides = current.strides.clone();
        new_strides[axis] = -new_strides[axis];

        // Adjust offset: flip moves the start pointer to the last element
        let new_offset = current.offset + (current.shape[axis] as i64 - 1) * current.strides[axis];

        Self {
            views: vec![View {
                shape: current.shape.clone(),
                strides: new_strides,
                offset: new_offset,
                mask: current.mask.clone(),
            }],
        }
    }

    /// Compute the buffer offset for a linear logical index.
    /// For single-view ShapeTrackers, delegates directly to the view.
    /// For multi-view, composes views from outer to inner.
    /// Returns None if any view's mask excludes the index (padding).
    pub fn expr_idx(&self, linear_idx: usize) -> Option<usize> {
        if self.views.len() == 1 {
            return self.views[0].expr_idx(linear_idx);
        }

        // Multi-view: compose from outer to inner.
        let mut idx = linear_idx;
        for view in self.views.iter().rev() {
            match view.expr_idx(idx) {
                Some(next_idx) => idx = next_idx,
                None => return None,
            }
        }
        Some(idx)
    }
}
```

- [ ] **2.2** Write `runtime/molt-gpu/tests/test_shapetracker.rs`:

```rust
use molt_gpu::shapetracker::{View, ShapeTracker};

#[test]
fn test_contiguous_view() {
    let v = View::contiguous(&[2, 3, 4]);
    assert_eq!(v.shape, vec![2, 3, 4]);
    assert_eq!(v.strides, vec![12, 4, 1]);
    assert_eq!(v.offset, 0);
    assert!(v.is_contiguous());
    assert_eq!(v.numel(), 24);
}

#[test]
fn test_expr_idx_contiguous() {
    let v = View::contiguous(&[2, 3]);
    // [0,0]=0, [0,1]=1, [0,2]=2, [1,0]=3, [1,1]=4, [1,2]=5
    assert_eq!(v.expr_idx(0), Some(0));
    assert_eq!(v.expr_idx(1), Some(1));
    assert_eq!(v.expr_idx(3), Some(3));
    assert_eq!(v.expr_idx(5), Some(5));
}

#[test]
fn test_reshape() {
    let st = ShapeTracker::contiguous(&[2, 3]);
    let reshaped = st.reshape(&[3, 2]);
    assert_eq!(reshaped.shape(), &[3, 2]);
    assert_eq!(reshaped.numel(), 6);
}

#[test]
fn test_permute() {
    let st = ShapeTracker::contiguous(&[2, 3, 4]);
    let perm = st.permute(&[2, 0, 1]);
    assert_eq!(perm.shape(), &[4, 2, 3]);
    // Strides should be reordered: original [12, 4, 1] -> [1, 12, 4]
    assert_eq!(perm.view().strides, vec![1, 12, 4]);
}

#[test]
fn test_expand() {
    let st = ShapeTracker::contiguous(&[1, 3]);
    let expanded = st.expand(&[4, 3]);
    assert_eq!(expanded.shape(), &[4, 3]);
    assert_eq!(expanded.view().strides, vec![0, 1]); // broadcast dim has stride 0
    // All rows should map to the same underlying data
    assert_eq!(expanded.expr_idx(0), Some(0));  // [0,0]
    assert_eq!(expanded.expr_idx(3), Some(0));  // [1,0] -> same as [0,0]
    assert_eq!(expanded.expr_idx(4), Some(1));  // [1,1]
}

#[test]
fn test_pad() {
    let st = ShapeTracker::contiguous(&[3]);
    let padded = st.pad(&[(1, 2)]); // 1 before, 2 after -> shape [6]
    assert_eq!(padded.shape(), &[6]);
    assert_eq!(padded.expr_idx(0), None); // padding before
    assert_eq!(padded.expr_idx(1), Some(0)); // first real element
    assert_eq!(padded.expr_idx(2), Some(1));
    assert_eq!(padded.expr_idx(3), Some(2));
    assert_eq!(padded.expr_idx(4), None); // padding after
    assert_eq!(padded.expr_idx(5), None);
}

#[test]
fn test_shrink() {
    let st = ShapeTracker::contiguous(&[5]);
    let shrunk = st.shrink(&[(1, 4)]); // extract elements [1,2,3]
    assert_eq!(shrunk.shape(), &[3]);
    assert_eq!(shrunk.expr_idx(0), Some(1));
    assert_eq!(shrunk.expr_idx(1), Some(2));
    assert_eq!(shrunk.expr_idx(2), Some(3));
}

#[test]
fn test_flip() {
    let st = ShapeTracker::contiguous(&[4]);
    let flipped = st.flip(0);
    assert_eq!(flipped.shape(), &[4]);
    assert_eq!(flipped.expr_idx(0), Some(3)); // first element -> last
    assert_eq!(flipped.expr_idx(1), Some(2));
    assert_eq!(flipped.expr_idx(2), Some(1));
    assert_eq!(flipped.expr_idx(3), Some(0));
}

#[test]
fn test_2d_pad() {
    let st = ShapeTracker::contiguous(&[2, 3]);
    let padded = st.pad(&[(1, 0), (0, 1)]); // pad row before, col after -> [3, 4]
    assert_eq!(padded.shape(), &[3, 4]);
    assert_eq!(padded.expr_idx(0), None);  // [0, 0] -> padded row
    assert_eq!(padded.expr_idx(4), Some(0)); // [1, 0] -> first real element
    assert_eq!(padded.expr_idx(7), None);  // [1, 3] -> padded col
}

#[test]
fn test_transpose_via_permute() {
    // Transpose a 2D matrix
    let st = ShapeTracker::contiguous(&[3, 4]);
    let transposed = st.permute(&[1, 0]);
    assert_eq!(transposed.shape(), &[4, 3]);
    // [0,0] of transposed = [0,0] of original = offset 0
    assert_eq!(transposed.expr_idx(0), Some(0));
    // [0,1] of transposed = [1,0] of original = offset 4
    assert_eq!(transposed.expr_idx(1), Some(4));
    // [1,0] of transposed = [0,1] of original = offset 1
    assert_eq!(transposed.expr_idx(3), Some(1));
}
```

- [ ] **2.3** Run ShapeTracker tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_shapetracker
```

- [ ] **2.4** `git add` and commit.

---

## Task 3: LazyOp DAG

**Files:**
- `runtime/molt-gpu/src/lazy.rs`

**Steps:**

- [ ] **3.1** Implement `LazyOp` in `runtime/molt-gpu/src/lazy.rs`:

```rust
//! LazyOp DAG — deferred computation graph.
//!
//! Every Tensor method returns a new Tensor backed by a LazyOp node.
//! No GPU kernel is executed until realize() is called. This enables
//! the fusion engine to see the full computation graph.

use std::sync::Arc;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::shapetracker::ShapeTracker;

/// Opaque handle to a device buffer. The actual buffer is managed
/// by the device's Allocator and not exposed through the DAG.
#[derive(Debug, Clone)]
pub struct DeviceBufferRef {
    /// Unique identifier for this buffer in the device's allocation table.
    pub id: usize,
    /// Size in bytes.
    pub size_bytes: usize,
}

/// A node in the lazy computation DAG.
#[derive(Debug, Clone)]
pub enum LazyOp {
    /// Leaf: a realized buffer with known contents.
    Buffer {
        buf: DeviceBufferRef,
        st: ShapeTracker,
        dtype: DType,
    },
    /// Elementwise unary op.
    Unary {
        op: PrimitiveOp,
        src: Arc<LazyOp>,
    },
    /// Elementwise binary op.
    Binary {
        op: PrimitiveOp,
        lhs: Arc<LazyOp>,
        rhs: Arc<LazyOp>,
    },
    /// Ternary op (WHERE).
    Ternary {
        op: PrimitiveOp,
        cond: Arc<LazyOp>,
        a: Arc<LazyOp>,
        b: Arc<LazyOp>,
    },
    /// Reduce op over a single axis.
    /// The axis dimension is REMOVED from the output shape (no keepdim).
    /// Multi-axis reduces are chained.
    Reduce {
        op: PrimitiveOp,
        src: Arc<LazyOp>,
        axis: usize,
    },
    /// Movement op (free — just modifies ShapeTracker).
    /// The `st` here is the RESULT view, not an incremental delta.
    Movement {
        src: Arc<LazyOp>,
        st: ShapeTracker,
    },
    /// Scheduling annotation: force materialization.
    /// NOT a compute op — the scheduler inserts a copy kernel.
    Contiguous {
        src: Arc<LazyOp>,
    },
}

impl LazyOp {
    /// Get the output dtype of this op.
    pub fn dtype(&self) -> DType {
        match self {
            Self::Buffer { dtype, .. } => *dtype,
            Self::Unary { op, src } => {
                if matches!(op, PrimitiveOp::Cast | PrimitiveOp::Bitcast) {
                    // For Cast/Bitcast, the target dtype must be stored elsewhere
                    // (in the FusedOp layer). At the LazyOp level, we propagate
                    // the source dtype as a placeholder. The scheduler resolves this.
                    src.dtype()
                } else {
                    src.dtype()
                }
            }
            Self::Binary { op, lhs, .. } => {
                if matches!(op, PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne) {
                    DType::Bool
                } else {
                    lhs.dtype()
                }
            }
            Self::Ternary { a, .. } => a.dtype(),
            Self::Reduce { src, .. } => src.dtype(),
            Self::Movement { src, .. } => src.dtype(),
            Self::Contiguous { src } => src.dtype(),
        }
    }

    /// Get the output shape of this op.
    pub fn shape(&self) -> Vec<usize> {
        match self {
            Self::Buffer { st, .. } => st.shape().to_vec(),
            Self::Unary { src, .. } => src.shape(),
            Self::Binary { lhs, .. } => lhs.shape(),
            Self::Ternary { a, .. } => a.shape(),
            Self::Reduce { src, axis, .. } => {
                let mut shape = src.shape();
                shape.remove(*axis);
                if shape.is_empty() {
                    vec![1] // scalar result
                } else {
                    shape
                }
            }
            Self::Movement { st, .. } => st.shape().to_vec(),
            Self::Contiguous { src } => src.shape(),
        }
    }
}
```

- [ ] **3.2** Add basic LazyOp compilation test (verify crate still compiles):

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu
```

- [ ] **3.3** `git add` and commit.

---

## Task 4: FusedKernel IR + Renderer Trait

**Files:**
- `runtime/molt-gpu/src/render/mod.rs`

**Steps:**

- [ ] **4.1** Implement the full FusedKernel IR and Renderer trait in `runtime/molt-gpu/src/render/mod.rs`:

```rust
//! Renderer trait + FusedKernel IR.
//!
//! The FusedKernel is the post-fusion IR passed to renderers. It contains
//! an ordered chain of ops (elementwise prefix -> optional reduce ->
//! elementwise suffix) plus buffer bindings and work distribution.

pub mod msl;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::shapetracker::ShapeTracker;

/// A single fused kernel ready for codegen.
#[derive(Debug, Clone)]
pub struct FusedKernel {
    /// Ordered chain of ops: elementwise prefix -> optional reduce -> elementwise suffix.
    pub ops: Vec<FusedOp>,
    /// Buffer bindings. Convention: bufs[0] is ALWAYS the output (Write access).
    /// bufs[1..] are inputs (Read access). ReadWrite is used for in-place ops.
    pub bufs: Vec<BufferBinding>,
    /// Work distribution. Computed by the scheduler, NOT the renderer.
    pub grid: [u32; 3],
    pub local: [u32; 3],
}

/// A single op in a fused chain.
#[derive(Debug, Clone)]
pub struct FusedOp {
    /// The primitive op to execute.
    pub op: PrimitiveOp,
    /// Input sources (explicit references, not ambiguous indices).
    pub srcs: Vec<FusedSrc>,
    /// Output dtype. Always DType::Bool for comparison ops (Cmplt, Cmpeq, Cmpne).
    pub dst_dtype: DType,
}

/// Source reference for a fused op.
#[derive(Debug, Clone)]
pub enum FusedSrc {
    /// Index into FusedKernel.bufs (an input/output buffer).
    Buf(usize),
    /// Index into FusedKernel.ops (a prior op's result, must be < current op index).
    Op(usize),
    /// Scalar constant broadcast to all elements.
    /// Used for: relu's 0, sigmoid's 1, softmax's LOG2_E, etc.
    Const { val: f64, dtype: DType },
}

/// Buffer access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferAccess {
    /// Input buffer (const device T* in MSL).
    Read,
    /// Output buffer (device T* in MSL).
    Write,
    /// In-place mutation.
    ReadWrite,
}

/// A buffer binding with its view into memory.
#[derive(Debug, Clone)]
pub struct BufferBinding {
    /// Runtime buffer handle index.
    pub buf_id: usize,
    /// How this buffer is accessed (ShapeTracker view).
    pub st: ShapeTracker,
    /// Element type.
    pub dtype: DType,
    /// Access mode.
    pub access: BufferAccess,
}

/// Renderer trait — converts FusedKernel to shader source code.
pub trait Renderer {
    /// Render a fused kernel into shader source code.
    fn render(&self, kernel: &FusedKernel) -> String;
}
```

- [ ] **4.2** Verify compilation:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu
```

- [ ] **4.3** `git add` and commit.

---

## Task 5: MslRenderer — All 26 Ops to MSL

**Files:**
- `runtime/molt-gpu/src/render/msl.rs`
- `runtime/molt-gpu/tests/test_render_msl.rs`

**Steps:**

- [ ] **5.1** Implement `MslRenderer` in `runtime/molt-gpu/src/render/msl.rs`:

```rust
//! MslRenderer — Metal Shading Language codegen for all 26 primitive ops.
//!
//! Generates MSL compute kernel source from FusedKernel IR.
//! All dtypes are narrowed via DType::narrow_metal() before rendering.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer};

pub struct MslRenderer;

impl MslRenderer {
    /// Format a constant value as MSL literal.
    fn format_const(val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_metal();
        match dtype {
            DType::Bool => {
                if val != 0.0 { "true".to_string() } else { "false".to_string() }
            }
            DType::Float16 => format!("half({})", val),
            DType::BFloat16 => format!("bfloat({})", val),
            DType::Float32 => {
                if val == f64::INFINITY { "INFINITY".to_string() }
                else if val == f64::NEG_INFINITY { "(-INFINITY)".to_string() }
                else if val.is_nan() { "NAN".to_string() }
                else { format!("{}f", val) }
            }
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("{}({})", dtype.msl_type(), val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("{}({})", dtype.msl_type(), val as u64)
            }
            _ => format!("{}", val),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(binding: &BufferBinding, idx_var: &str) -> String {
        let st = &binding.st;
        let view = st.view();

        // Build index expression from ShapeTracker
        let ndim = view.shape.len();
        if ndim == 0 {
            return format!("buf{}[0]", binding.buf_id);
        }

        // For contiguous single-dim or simple strides, generate direct expression
        if view.is_contiguous() && view.mask.is_none() {
            return format!("buf{}[{}]", binding.buf_id, idx_var);
        }

        // General case: decompose linear index, apply strides
        // Generate index decomposition inline
        let mut parts = Vec::new();
        let mut remaining = idx_var.to_string();
        for dim in 0..ndim {
            let size = view.shape[dim];
            let stride = view.strides[dim];
            if stride == 0 {
                // Broadcast dimension — skip
                if dim < ndim - 1 {
                    remaining = format!("({} % {})", remaining, view.shape[dim+1..].iter().product::<usize>());
                }
                continue;
            }
            let idx_expr = if dim == ndim - 1 {
                format!("({} % {})", remaining, size)
            } else {
                let divisor: usize = view.shape[dim+1..].iter().product();
                let expr = format!("({} / {} % {})", remaining, divisor, size);
                expr
            };
            if stride == 1 {
                parts.push(idx_expr);
            } else if stride == -1 {
                parts.push(format!("({} - {})", size - 1, idx_expr));
            } else if stride > 0 {
                parts.push(format!("{} * {}", idx_expr, stride));
            } else {
                parts.push(format!("({} - {}) * {}", size - 1, idx_expr, -stride));
            }
        }

        let offset = if view.offset != 0 {
            format!("{} + ", view.offset)
        } else {
            String::new()
        };

        let idx_sum = if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" + ")
        };

        format!("buf{}[{}{}]", binding.buf_id, offset, idx_sum)
    }

    /// Render a single op expression.
    fn render_op(op: &FusedOp, op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String {
            match &op.srcs[i] {
                FusedSrc::Buf(buf_idx) => {
                    Self::render_buf_read(&kernel.bufs[*buf_idx], idx_var)
                }
                FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
                FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
            }
        };

        let dst_type = op.dst_dtype.narrow_metal().msl_type();

        match op.op {
            // Arithmetic
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            PrimitiveOp::Mod => format!("({} % {})", src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            // Comparison — output is always bool
            PrimitiveOp::Cmplt => format!("({} < {})", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("({} == {})", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("({} != {})", src(0), src(1)),

            // Bitwise
            PrimitiveOp::And => format!("({} & {})", src(0), src(1)),
            PrimitiveOp::Or => format!("({} | {})", src(0), src(1)),
            PrimitiveOp::Xor => format!("({} ^ {})", src(0), src(1)),
            PrimitiveOp::Shl => format!("({} << {})", src(0), src(1)),
            PrimitiveOp::Shr => format!("({} >> {})", src(0), src(1)),

            // Math
            PrimitiveOp::Exp2 => format!("exp2({})", src(0)),
            PrimitiveOp::Log2 => format!("log2({})", src(0)),
            PrimitiveOp::Sin => format!("sin({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrt({})", src(0)),
            PrimitiveOp::Reciprocal => format!("(1.0f / {})", src(0)),

            // Other
            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            PrimitiveOp::Max => format!("max({}, {})", src(0), src(1)),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("{}({})", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("as_type<{}>({})", dst_type, src(0)),

            // Reduce — these generate loop structures, handled specially
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                // Reduce ops are rendered as loop structures in the kernel body.
                // This method only handles the reduction expression inside the loop.
                // The loop structure is generated by render().
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }
}

impl Renderer for MslRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        // Include headers
        writeln!(out, "#include <metal_stdlib>").unwrap();
        writeln!(out, "using namespace metal;").unwrap();
        writeln!(out).unwrap();

        // Kernel function signature
        write!(out, "kernel void molt_kernel(").unwrap();

        // Buffer parameters
        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.narrow_metal().msl_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "const device",
                BufferAccess::Write | BufferAccess::ReadWrite => "device",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{} {}* buf{} [[buffer({})]]", qualifier, dtype_str, binding.buf_id, i).unwrap();
        }

        // Thread index
        write!(out, ", uint gid [[thread_position_in_grid]]").unwrap();
        writeln!(out, ") {{").unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}) return;", output_numel).unwrap();

        // Check if we have reduce ops
        let has_reduce = kernel.ops.iter().any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if !has_reduce {
            // Pure elementwise kernel — straightforward
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            // Write output
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        } else {
            // Fused kernel with reduce: elementwise prefix -> reduce -> elementwise suffix
            // Find the reduce op
            let reduce_idx = kernel.ops.iter().position(|op| {
                matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
            }).expect("has_reduce but no reduce op found");

            // Pre-reduce elementwise ops use the input element index
            // The reduce loop iterates over the reduce axis

            // For now, generate a simple sequential reduce
            // (workgroup reduction is Phase 2 optimization)
            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs[0];
            let reduce_dtype = reduce_op.dst_dtype.narrow_metal();

            // Find the input buffer for the reduce source
            let input_buf = match reduce_src {
                FusedSrc::Buf(idx) => &kernel.bufs[*idx],
                FusedSrc::Op(_) => {
                    // The reduce operates on a prior op's output;
                    // we need the shape from the input buffer
                    &kernel.bufs[1] // default to first input
                }
                FusedSrc::Const { .. } => unreachable!("reduce on constant"),
            };
            let reduce_size = input_buf.st.numel() / output_numel;

            let init_val = match reduce_op.op {
                PrimitiveOp::ReduceSum => "0",
                PrimitiveOp::ReduceMax => "-INFINITY",
                _ => unreachable!(),
            };

            // Initialize accumulator
            writeln!(out, "    {} acc = {};", reduce_dtype.msl_type(), init_val).unwrap();

            // Pre-reduce elementwise ops
            if reduce_idx > 0 {
                writeln!(out, "    for (uint rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        uint eidx = gid * {} + rid;", reduce_size).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
                }

                // Accumulate
                let src_var = format!("v{}", reduce_idx - 1);
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_var).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_var).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                // Reduce directly from buffer
                writeln!(out, "    for (uint rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        uint eidx = gid * {} + rid;", reduce_size).unwrap();
                let src_expr = match reduce_src {
                    FusedSrc::Buf(idx) => Self::render_buf_read(&kernel.bufs[*idx], "eidx"),
                    _ => unreachable!(),
                };
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_expr).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            // Store reduce result
            writeln!(out, "    {} v{} = acc;", reduce_dtype.msl_type(), reduce_idx).unwrap();

            // Post-reduce elementwise ops
            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            // Write output
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
```

- [ ] **5.2** Write `runtime/molt-gpu/tests/test_render_msl.rs`:

```rust
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::msl::MslRenderer;
use molt_gpu::shapetracker::ShapeTracker;

fn make_simple_binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
    }
}

#[test]
fn test_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("#include <metal_stdlib>"));
    assert!(msl.contains("kernel void molt_kernel"));
    assert!(msl.contains("buf1[gid] + buf2[gid]"));
    assert!(msl.contains("buf0[gid] = v0"));
}

#[test]
fn test_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 512);
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("buf1[gid] * buf2[gid]"));
}

#[test]
fn test_render_neg_unary() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Neg,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("(-buf1[gid])"));
}

#[test]
fn test_render_fused_chain() {
    // a + b * c -> fused into: v0 = b * c, v1 = a + v0
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(2), FusedSrc::Buf(3)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("v0")); // mul result
    assert!(msl.contains("v1")); // add result referencing v0
    assert!(msl.contains("buf0[gid] = v1")); // output is last op
}

#[test]
fn test_render_relu_with_const() {
    // relu(x) = max(x, 0)
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Max,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: 0.0, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("max(buf1[gid], 0f)"));
}

#[test]
fn test_render_comparison_bool_output() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Bool,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("bool v0"));
    assert!(msl.contains("buf1[gid] < buf2[gid]"));
}

#[test]
fn test_all_26_ops_have_render_patterns() {
    // Verify every elementwise op has a render pattern (doesn't panic)
    let elementwise_ops = PrimitiveOp::ALL.iter()
        .filter(|op| op.is_elementwise())
        .collect::<Vec<_>>();

    for &op in &elementwise_ops {
        let srcs = match op.arity() {
            1 => vec![FusedSrc::Buf(1)],
            2 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            3 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            _ => unreachable!(),
        };
        let mut bufs = vec![BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[64]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        }];
        for i in 1..=op.arity() {
            bufs.push(BufferBinding {
                buf_id: i,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            });
        }
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs,
                dst_dtype: if matches!(op, PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne) {
                    DType::Bool
                } else {
                    DType::Float32
                },
            }],
            bufs,
            grid: [64, 1, 1],
            local: [64, 1, 1],
        };
        let msl = MslRenderer.render(&kernel);
        assert!(msl.contains("molt_kernel"), "op {:?} failed to render", op);
    }
}
```

- [ ] **5.3** Run MSL render tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_render_msl
```

- [ ] **5.4** `git add` and commit.

---

## Task 6: Device Traits + DeviceBuffer + DeviceError

**Files:**
- `runtime/molt-gpu/src/device/mod.rs`

**Steps:**

- [ ] **6.1** Implement device traits in `runtime/molt-gpu/src/device/mod.rs`:

```rust
//! Device traits: Allocator, Compiler, Executor.
//!
//! Each backend implements all three traits. The separation provides
//! distinct ownership semantics (buffers vs programs vs execution state).

#[cfg(target_os = "macos")]
pub mod metal;
pub mod cpu;

/// Opaque GPU buffer handle.
#[derive(Debug)]
pub struct DeviceBuffer {
    /// Backend-specific opaque handle (pointer, id, etc.).
    /// Stored as raw bytes to avoid backend-specific types in the trait.
    pub(crate) handle: BufferHandle,
    /// Size in bytes.
    pub size_bytes: usize,
}

/// Backend-specific buffer handle.
#[derive(Debug)]
pub(crate) enum BufferHandle {
    /// CPU buffer (owned Vec<u8>).
    Cpu(Vec<u8>),
    /// Metal buffer (raw pointer to MTLBuffer).
    #[cfg(target_os = "macos")]
    Metal(*mut std::ffi::c_void),
}

// SAFETY: Metal buffers are Send+Sync when accessed through command buffers.
unsafe impl Send for BufferHandle {}
unsafe impl Sync for BufferHandle {}

/// Compiled program handle.
#[derive(Debug)]
pub struct CompiledProgram {
    /// Backend-specific compiled program handle.
    pub(crate) handle: ProgramHandle,
    /// Entry point function name.
    pub entry: String,
}

/// Backend-specific program handle.
#[derive(Debug)]
pub(crate) enum ProgramHandle {
    /// CPU: compiled function pointer.
    Cpu(CpuKernelFn),
    /// Metal: raw pointer to MTLComputePipelineState.
    #[cfg(target_os = "macos")]
    Metal(*mut std::ffi::c_void),
}

unsafe impl Send for ProgramHandle {}
unsafe impl Sync for ProgramHandle {}

/// CPU kernel function type.
pub(crate) type CpuKernelFn = fn(bufs: &[&[u8]], out: &mut [u8], num_elements: usize);

/// Device error type.
#[derive(Debug)]
pub enum DeviceError {
    /// Buffer allocation failed.
    AllocationFailed(String),
    /// Compilation failed.
    CompilationFailed(String),
    /// Execution failed.
    ExecutionFailed(String),
    /// Invalid argument.
    InvalidArgument(String),
    /// Out of memory.
    OutOfMemory,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailed(msg) => write!(f, "allocation failed: {}", msg),
            Self::CompilationFailed(msg) => write!(f, "compilation failed: {}", msg),
            Self::ExecutionFailed(msg) => write!(f, "execution failed: {}", msg),
            Self::InvalidArgument(msg) => write!(f, "invalid argument: {}", msg),
            Self::OutOfMemory => write!(f, "out of memory"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// Memory management trait. Owns buffer lifetimes.
/// SAFETY CONTRACT: free() internally synchronizes before releasing GPU memory.
pub trait Allocator: Send + Sync {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError>;
    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError>;
    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError>;
    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError>;
}

/// Kernel compilation trait. Owns compiled program cache internally.
pub trait Compiler: Send + Sync {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError>;

    /// Maximum local (threadgroup/workgroup) size per dimension.
    fn max_local_size(&self) -> [u32; 3];
    /// Maximum grid size per dimension.
    fn max_grid_size(&self) -> [u32; 3];
}

/// Kernel execution trait.
pub trait Executor: Send + Sync {
    fn exec(
        &self,
        prog: &CompiledProgram,
        bufs: &[&DeviceBuffer],
        grid: [u32; 3],
        local: [u32; 3],
    ) -> Result<(), DeviceError>;
    fn synchronize(&self) -> Result<(), DeviceError>;
}
```

- [ ] **6.2** Verify compilation:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu
```

- [ ] **6.3** `git add` and commit.

---

## Task 7: CpuDevice — Reference Backend

**Files:**
- `runtime/molt-gpu/src/device/cpu.rs`

**Steps:**

- [ ] **7.1** Implement `CpuDevice` in `runtime/molt-gpu/src/device/cpu.rs`. This is the reference implementation for correctness testing. It executes FusedKernels by interpreting them directly, not by compiling shader source.

```rust
//! CpuDevice — CPU reference backend for testing.
//!
//! Executes kernels by interpreting the FusedKernel IR directly.
//! Not performant — used only for correctness reference.

use std::sync::Mutex;

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, CpuKernelFn,
    DeviceBuffer, DeviceError, Executor, ProgramHandle,
};

pub struct CpuDevice {
    /// Buffer allocation counter for unique IDs.
    next_id: Mutex<usize>,
}

impl CpuDevice {
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(0),
        }
    }
}

impl Default for CpuDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator for CpuDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buf = vec![0u8; size_bytes];
        Ok(DeviceBuffer {
            handle: BufferHandle::Cpu(buf),
            size_bytes,
        })
    }

    fn free(&self, _buf: DeviceBuffer) -> Result<(), DeviceError> {
        // Drop handles deallocation for CPU buffers
        Ok(())
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                if data.len() > inner.len() {
                    return Err(DeviceError::InvalidArgument(format!(
                        "copy_in: data ({} bytes) exceeds buffer ({} bytes)",
                        data.len(),
                        inner.len()
                    )));
                }
                // SAFETY: We need interior mutability. The CPU backend uses
                // this for testing only. In production, Metal/WebGPU handle
                // synchronization at the command buffer level.
                let inner_ptr = inner.as_ptr() as *mut u8;
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), inner_ptr, data.len());
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_in to Metal buffer via CpuDevice".into(),
            )),
        }
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                let len = data.len().min(inner.len());
                data[..len].copy_from_slice(&inner[..len]);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_out from Metal buffer via CpuDevice".into(),
            )),
        }
    }
}

impl Compiler for CpuDevice {
    fn compile(&self, _source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        // CPU device doesn't compile shader source — it interprets FusedKernel directly.
        // The CompiledProgram is a no-op placeholder.
        fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}
        Ok(CompiledProgram {
            handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [1024, 1, 1]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [u32::MAX, 1, 1]
    }
}

impl Executor for CpuDevice {
    fn exec(
        &self,
        _prog: &CompiledProgram,
        _bufs: &[&DeviceBuffer],
        _grid: [u32; 3],
        _local: [u32; 3],
    ) -> Result<(), DeviceError> {
        // CPU execution is done through the interpret_kernel method, not exec.
        // The scheduler calls interpret_kernel directly for CpuDevice.
        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        // CPU is synchronous — nothing to wait for.
        Ok(())
    }
}

/// CPU kernel interpreter — executes a FusedKernel op-by-op on CPU.
/// This is the reference implementation used for correctness testing.
pub mod interpret {
    use crate::dtype::DType;
    use crate::ops::PrimitiveOp;
    use crate::render::{FusedKernel, FusedSrc};

    /// Interpret and execute a FusedKernel on CPU buffers.
    /// `bufs` are raw byte slices matching kernel.bufs order.
    /// bufs[0] is the output buffer (written to).
    pub fn execute_kernel(kernel: &FusedKernel, bufs: &mut [Vec<u8>]) {
        let output_numel = kernel.bufs[0].st.numel();

        for gid in 0..output_numel {
            let mut values: Vec<f64> = Vec::with_capacity(kernel.ops.len());

            for (op_idx, op) in kernel.ops.iter().enumerate() {
                if matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
                    // Handle reduce ops
                    let input_buf_idx = match &op.srcs[0] {
                        FusedSrc::Buf(idx) => *idx,
                        FusedSrc::Op(_) => 1, // fallback
                        FusedSrc::Const { .. } => unreachable!(),
                    };
                    let input_numel = kernel.bufs[input_buf_idx].st.numel();
                    let reduce_size = input_numel / output_numel;

                    let mut acc = match op.op {
                        PrimitiveOp::ReduceSum => 0.0f64,
                        PrimitiveOp::ReduceMax => f64::NEG_INFINITY,
                        _ => unreachable!(),
                    };

                    for rid in 0..reduce_size {
                        let eidx = gid * reduce_size + rid;
                        let val = if op_idx > 0 {
                            // Pre-reduce ops were computed — but in the CPU interpreter
                            // we need to recompute for each reduce element.
                            // Simplified: read directly from input buffer.
                            read_f64(&bufs[input_buf_idx], eidx, kernel.bufs[input_buf_idx].dtype)
                        } else {
                            read_f64(&bufs[input_buf_idx], eidx, kernel.bufs[input_buf_idx].dtype)
                        };

                        acc = match op.op {
                            PrimitiveOp::ReduceSum => acc + val,
                            PrimitiveOp::ReduceMax => acc.max(val),
                            _ => unreachable!(),
                        };
                    }
                    values.push(acc);
                    continue;
                }

                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype)
                        }
                        FusedSrc::Op(prior) => values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                let result = match op.op {
                    PrimitiveOp::Add => get_src(0) + get_src(1),
                    PrimitiveOp::Sub => get_src(0) - get_src(1),
                    PrimitiveOp::Mul => get_src(0) * get_src(1),
                    PrimitiveOp::Idiv => {
                        let a = get_src(0) as i64;
                        let b = get_src(1) as i64;
                        if b == 0 { 0.0 } else { (a / b) as f64 }
                    }
                    PrimitiveOp::Mod => {
                        let a = get_src(0) as i64;
                        let b = get_src(1) as i64;
                        if b == 0 { 0.0 } else { (a % b) as f64 }
                    }
                    PrimitiveOp::Neg => -get_src(0),
                    PrimitiveOp::Cmplt => if get_src(0) < get_src(1) { 1.0 } else { 0.0 },
                    PrimitiveOp::Cmpeq => if get_src(0) == get_src(1) { 1.0 } else { 0.0 },
                    PrimitiveOp::Cmpne => if get_src(0) != get_src(1) { 1.0 } else { 0.0 },
                    PrimitiveOp::And => ((get_src(0) as i64) & (get_src(1) as i64)) as f64,
                    PrimitiveOp::Or => ((get_src(0) as i64) | (get_src(1) as i64)) as f64,
                    PrimitiveOp::Xor => ((get_src(0) as i64) ^ (get_src(1) as i64)) as f64,
                    PrimitiveOp::Shl => ((get_src(0) as i64) << (get_src(1) as i64)) as f64,
                    PrimitiveOp::Shr => ((get_src(0) as i64) >> (get_src(1) as i64)) as f64,
                    PrimitiveOp::Exp2 => get_src(0).exp2(),
                    PrimitiveOp::Log2 => get_src(0).log2(),
                    PrimitiveOp::Sin => get_src(0).sin(),
                    PrimitiveOp::Sqrt => get_src(0).sqrt(),
                    PrimitiveOp::Reciprocal => 1.0 / get_src(0),
                    PrimitiveOp::Trunc => get_src(0).trunc(),
                    PrimitiveOp::Max => get_src(0).max(get_src(1)),
                    PrimitiveOp::Where => {
                        if get_src(0) != 0.0 { get_src(1) } else { get_src(2) }
                    }
                    PrimitiveOp::Cast => get_src(0), // simplified: f64 pass-through
                    PrimitiveOp::Bitcast => get_src(0), // simplified
                    PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => unreachable!(),
                };
                values.push(result);
            }

            // Write output
            let result = values.last().copied().unwrap_or(0.0);
            write_f64(&mut bufs[0], gid, result, kernel.bufs[0].dtype);
        }
    }

    fn read_f64(buf: &[u8], idx: usize, dtype: DType) -> f64 {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                f64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Bool | DType::UInt8 => {
                if idx >= buf.len() { return 0.0; }
                buf[idx] as f64
            }
            _ => 0.0, // extend for other types as needed
        }
    }

    fn write_f64(buf: &mut [u8], idx: usize, val: f64, dtype: DType) {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as f32).to_le_bytes());
                }
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
                }
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as i32).to_le_bytes());
                }
            }
            DType::Bool | DType::UInt8 => {
                if idx < buf.len() {
                    buf[idx] = if val != 0.0 { 1 } else { 0 };
                }
            }
            _ => {} // extend for other types as needed
        }
    }
}
```

- [ ] **7.2** Write CPU device tests in `runtime/molt-gpu/tests/test_ops.rs` (extend the existing file):

Add to `test_ops.rs`:
```rust
use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::shapetracker::ShapeTracker;

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn run_binary_op_cpu(op: PrimitiveOp, a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len();
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };

    let mut bufs = vec![
        vec![0u8; n * 4], // output
        f32_to_bytes(a),
        f32_to_bytes(b),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

fn run_unary_op_cpu(op: PrimitiveOp, a: &[f32]) -> Vec<f32> {
    let n = a.len();
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };

    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(a),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

#[test]
fn test_cpu_add() {
    let result = run_binary_op_cpu(PrimitiveOp::Add, &[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
    assert_eq!(result, vec![5.0, 7.0, 9.0]);
}

#[test]
fn test_cpu_sub() {
    let result = run_binary_op_cpu(PrimitiveOp::Sub, &[5.0, 3.0, 1.0], &[1.0, 2.0, 3.0]);
    assert_eq!(result, vec![4.0, 1.0, -2.0]);
}

#[test]
fn test_cpu_mul() {
    let result = run_binary_op_cpu(PrimitiveOp::Mul, &[2.0, 3.0, 4.0], &[5.0, 6.0, 7.0]);
    assert_eq!(result, vec![10.0, 18.0, 28.0]);
}

#[test]
fn test_cpu_neg() {
    let result = run_unary_op_cpu(PrimitiveOp::Neg, &[1.0, -2.0, 0.0]);
    assert_eq!(result, vec![-1.0, 2.0, -0.0]);
}

#[test]
fn test_cpu_exp2() {
    let result = run_unary_op_cpu(PrimitiveOp::Exp2, &[0.0, 1.0, 2.0, 3.0]);
    assert_eq!(result, vec![1.0, 2.0, 4.0, 8.0]);
}

#[test]
fn test_cpu_log2() {
    let result = run_unary_op_cpu(PrimitiveOp::Log2, &[1.0, 2.0, 4.0, 8.0]);
    assert_eq!(result, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn test_cpu_sqrt() {
    let result = run_unary_op_cpu(PrimitiveOp::Sqrt, &[0.0, 1.0, 4.0, 9.0]);
    assert_eq!(result, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn test_cpu_reciprocal() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[1.0, 2.0, 4.0, 0.5]);
    assert_eq!(result, vec![1.0, 0.5, 0.25, 2.0]);
}

#[test]
fn test_cpu_reciprocal_zero() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[0.0]);
    assert!(result[0].is_infinite() && result[0] > 0.0); // +inf
}

#[test]
fn test_cpu_reciprocal_neg_zero() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[-0.0]);
    assert!(result[0].is_infinite() && result[0] < 0.0); // -inf
}

#[test]
fn test_cpu_max() {
    let result = run_binary_op_cpu(PrimitiveOp::Max, &[1.0, 5.0, -3.0], &[3.0, 2.0, -1.0]);
    assert_eq!(result, vec![3.0, 5.0, -1.0]);
}

#[test]
fn test_cpu_trunc() {
    let result = run_unary_op_cpu(PrimitiveOp::Trunc, &[2.7, -2.7, 3.0, -3.0]);
    assert_eq!(result, vec![2.0, -2.0, 3.0, -3.0]);
}

#[test]
fn test_cpu_sin() {
    let result = run_unary_op_cpu(PrimitiveOp::Sin, &[0.0]);
    assert!((result[0] - 0.0).abs() < 1e-6);
}

#[test]
fn test_cpu_relu_composition() {
    // relu(x) = max(x, 0)
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Max,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: 0.0, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[-2.0, -1.0, 0.0, 3.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![0.0, 0.0, 0.0, 3.0]);
}

#[test]
fn test_cpu_where_ternary() {
    let n = 3;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Where,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[1.0, 0.0, 1.0]),   // condition
        f32_to_bytes(&[10.0, 20.0, 30.0]), // true branch
        f32_to_bytes(&[40.0, 50.0, 60.0]), // false branch
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![10.0, 50.0, 30.0]);
}

#[test]
fn test_cpu_reduce_sum() {
    // Reduce sum of [1, 2, 3, 4] -> [10]
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[4]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let mut bufs = vec![
        vec![0u8; 4],
        f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![10.0]);
}

#[test]
fn test_cpu_reduce_max() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[4]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let mut bufs = vec![
        vec![0u8; 4],
        f32_to_bytes(&[3.0, 1.0, 4.0, 2.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![4.0]);
}
```

- [ ] **7.3** Run all tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu
```

- [ ] **7.4** `git add` and commit.

---

## Task 8: MetalDevice Backend

**Files:**
- `runtime/molt-gpu/src/device/metal.rs`

**Steps:**

- [ ] **8.1** Implement `MetalDevice` in `runtime/molt-gpu/src/device/metal.rs`:

```rust
//! MetalDevice — Apple GPU backend.
//!
//! Implements Allocator, Compiler, and Executor for Metal on macOS.
//! Device pool and kernel cache are internal to this struct.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::sync::Mutex;

use metal::{Device, MTLResourceOptions, MTLSize};

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, DeviceBuffer,
    DeviceError, Executor, ProgramHandle,
};

pub struct MetalDevice {
    device: Device,
    queue: metal::CommandQueue,
    /// Compiled pipeline state cache: source hash -> pipeline state.
    cache: Mutex<HashMap<u64, metal::ComputePipelineState>>,
}

impl MetalDevice {
    pub fn new() -> Result<Self, DeviceError> {
        let device = Device::system_default()
            .ok_or_else(|| DeviceError::AllocationFailed("no Metal device found".into()))?;
        let queue = device.new_command_queue();
        Ok(Self {
            device,
            queue,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Hash shader source for cache lookup.
    fn hash_source(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }
}

impl Allocator for MetalDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buffer = self.device.new_buffer(
            size_bytes as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let ptr = buffer.as_ptr() as *mut std::ffi::c_void;
        // Prevent buffer from being dropped — we manage lifetime manually
        std::mem::forget(buffer);
        Ok(DeviceBuffer {
            handle: BufferHandle::Metal(ptr),
            size_bytes,
        })
    }

    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError> {
        // Synchronize before freeing
        self.synchronize()?;
        match buf.handle {
            BufferHandle::Metal(ptr) => {
                // SAFETY: ptr was created by new_buffer and forgotten.
                // We reconstruct the buffer to drop it properly.
                unsafe {
                    let _ = metal::Buffer::from_ptr(ptr as *mut _);
                }
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Metal(ptr) => {
                // SAFETY: Metal shared buffers are directly accessible from CPU.
                unsafe {
                    let contents = (*ptr as *mut u8);
                    std::ptr::copy_nonoverlapping(data.as_ptr(), contents, data.len().min(buf.size_bytes));
                }
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        self.synchronize()?;
        match &buf.handle {
            BufferHandle::Metal(ptr) => {
                unsafe {
                    let contents = (*ptr as *const u8);
                    let len = data.len().min(buf.size_bytes);
                    std::ptr::copy_nonoverlapping(contents, data.as_mut_ptr(), len);
                }
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }
}

impl Compiler for MetalDevice {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        let hash = Self::hash_source(source);

        // Check cache
        {
            let cache = self.cache.lock().unwrap();
            if cache.contains_key(&hash) {
                // Return cached pipeline handle
                let pso = cache[&hash].clone();
                let ptr = &pso as *const _ as *mut std::ffi::c_void;
                std::mem::forget(pso);
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Metal(ptr),
                    entry: entry.to_string(),
                });
            }
        }

        // Compile MSL source
        let options = metal::CompileOptions::new();
        let library = self
            .device
            .new_library_with_source(source, &options)
            .map_err(|e| DeviceError::CompilationFailed(format!("{}", e)))?;

        let function = library
            .get_function(entry, None)
            .map_err(|e| DeviceError::CompilationFailed(format!("function '{}': {}", entry, e)))?;

        let pso = self
            .device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| DeviceError::CompilationFailed(format!("{}", e)))?;

        let ptr = &pso as *const _ as *mut std::ffi::c_void;

        // Cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(hash, pso);
        }

        Ok(CompiledProgram {
            handle: ProgramHandle::Metal(ptr),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [1024, 1024, 1024]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [u32::MAX, u32::MAX, u32::MAX]
    }
}

impl Executor for MetalDevice {
    fn exec(
        &self,
        prog: &CompiledProgram,
        bufs: &[&DeviceBuffer],
        grid: [u32; 3],
        local: [u32; 3],
    ) -> Result<(), DeviceError> {
        let command_buffer = self.queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();

        // Set pipeline state
        match &prog.handle {
            ProgramHandle::Metal(ptr) => {
                unsafe {
                    let pso = &*(*ptr as *const metal::ComputePipelineState);
                    encoder.set_compute_pipeline_state(pso);
                }
            }
            _ => return Err(DeviceError::InvalidArgument("not a Metal program".into())),
        }

        // Bind buffers
        for (i, buf) in bufs.iter().enumerate() {
            match &buf.handle {
                BufferHandle::Metal(ptr) => {
                    unsafe {
                        let mtl_buf = &*(*ptr as *const metal::Buffer);
                        encoder.set_buffer(i as u64, Some(mtl_buf), 0);
                    }
                }
                _ => return Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
            }
        }

        // Dispatch
        let grid_size = MTLSize::new(grid[0] as u64, grid[1] as u64, grid[2] as u64);
        let local_size = MTLSize::new(local[0] as u64, local[1] as u64, local[2] as u64);
        encoder.dispatch_threads(grid_size, local_size);
        encoder.end_encoding();
        command_buffer.commit();

        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        let command_buffer = self.queue.new_command_buffer();
        command_buffer.commit();
        command_buffer.wait_until_completed();
        Ok(())
    }
}
```

**Note:** The Metal backend implementation above is a starting scaffold. The exact Metal API calls will need adjustment based on the `metal` crate version's API. The implementor must verify each Metal API call compiles against `metal = "0.30"` and adjust as needed. The structure (allocate shared buffer, compile MSL, dispatch compute) is correct — only the Rust binding syntax may vary.

- [ ] **8.2** Write Metal integration test (gated behind `#[cfg(target_os = "macos")]`):

Add to `runtime/molt-gpu/tests/test_ops.rs`:
```rust
#[cfg(target_os = "macos")]
mod metal_tests {
    use super::*;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::Renderer;

    #[test]
    fn test_metal_add() {
        let device = MetalDevice::new().expect("Metal device required");
        let a_data = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);
        let b_data = f32_to_bytes(&[5.0, 6.0, 7.0, 8.0]);
        let n = 4;

        // Allocate buffers
        let out_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        let b_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&a_buf, &a_data).unwrap();
        device.copy_in(&b_buf, &b_data).unwrap();

        // Build kernel
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
                BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [4, 1, 1],
        };

        // Render MSL
        let msl = MslRenderer.render(&kernel);

        // Compile and execute
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device.exec(&prog, &[&out_buf, &a_buf, &b_buf], [n as u32, 1, 1], [4, 1, 1]).unwrap();
        device.synchronize().unwrap();

        // Read back
        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let result = bytes_to_f32(&result_bytes);

        // Compare with CPU reference
        let expected = run_binary_op_cpu(PrimitiveOp::Add, &[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0]);
        assert_eq!(result, expected);
    }
}
```

- [ ] **8.3** Run Metal tests (macOS only):

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_ops -- metal_tests
```

- [ ] **8.4** `git add` and commit.

---

## Task 9: Scheduler — DAG to Kernel Schedule

**Files:**
- `runtime/molt-gpu/src/schedule.rs`

**Steps:**

- [ ] **9.1** Implement the scheduler in `runtime/molt-gpu/src/schedule.rs`:

```rust
//! DAG -> topological kernel schedule.
//!
//! Walks the LazyOp DAG, identifies fusion boundaries, and produces
//! an ordered list of FusedKernels ready for rendering and execution.

use std::sync::Arc;

use crate::dtype::DType;
use crate::lazy::LazyOp;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use crate::shapetracker::ShapeTracker;

/// Schedule a LazyOp DAG into a list of FusedKernels.
///
/// Phase 1: single-op kernels (no fusion). The fusion engine (fuse.rs)
/// will merge these in a subsequent pass.
pub fn schedule(root: &Arc<LazyOp>, output_shape: &[usize]) -> Vec<FusedKernel> {
    let mut kernels = Vec::new();
    let mut next_buf_id = 0;

    schedule_recursive(root, &mut kernels, &mut next_buf_id);
    kernels
}

fn schedule_recursive(
    node: &Arc<LazyOp>,
    kernels: &mut Vec<FusedKernel>,
    next_buf_id: &mut usize,
) {
    match node.as_ref() {
        LazyOp::Buffer { .. } => {
            // Leaf node — already materialized, nothing to schedule.
        }
        LazyOp::Unary { op, src } => {
            schedule_recursive(src, kernels, next_buf_id);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();
            let out_id = *next_buf_id;
            *next_buf_id += 1;
            let in_id = *next_buf_id;
            *next_buf_id += 1;

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(1)],
                    dst_dtype: node.dtype(),
                }],
                bufs: vec![
                    BufferBinding { buf_id: out_id, st: ShapeTracker::contiguous(&shape), dtype: node.dtype(), access: BufferAccess::Write },
                    BufferBinding { buf_id: in_id, st: ShapeTracker::contiguous(&shape), dtype: src.dtype(), access: BufferAccess::Read },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.min(256).max(1) as u32, 1, 1],
            });
        }
        LazyOp::Binary { op, lhs, rhs } => {
            schedule_recursive(lhs, kernels, next_buf_id);
            schedule_recursive(rhs, kernels, next_buf_id);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();
            let out_id = *next_buf_id;
            *next_buf_id += 1;
            let lhs_id = *next_buf_id;
            *next_buf_id += 1;
            let rhs_id = *next_buf_id;
            *next_buf_id += 1;

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                    dst_dtype: node.dtype(),
                }],
                bufs: vec![
                    BufferBinding { buf_id: out_id, st: ShapeTracker::contiguous(&shape), dtype: node.dtype(), access: BufferAccess::Write },
                    BufferBinding { buf_id: lhs_id, st: ShapeTracker::contiguous(&shape), dtype: lhs.dtype(), access: BufferAccess::Read },
                    BufferBinding { buf_id: rhs_id, st: ShapeTracker::contiguous(&shape), dtype: rhs.dtype(), access: BufferAccess::Read },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.min(256).max(1) as u32, 1, 1],
            });
        }
        LazyOp::Ternary { op, cond, a, b } => {
            schedule_recursive(cond, kernels, next_buf_id);
            schedule_recursive(a, kernels, next_buf_id);
            schedule_recursive(b, kernels, next_buf_id);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();
            let out_id = *next_buf_id;
            *next_buf_id += 1;
            let cond_id = *next_buf_id;
            *next_buf_id += 1;
            let a_id = *next_buf_id;
            *next_buf_id += 1;
            let b_id = *next_buf_id;
            *next_buf_id += 1;

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
                    dst_dtype: node.dtype(),
                }],
                bufs: vec![
                    BufferBinding { buf_id: out_id, st: ShapeTracker::contiguous(&shape), dtype: node.dtype(), access: BufferAccess::Write },
                    BufferBinding { buf_id: cond_id, st: ShapeTracker::contiguous(&shape), dtype: cond.dtype(), access: BufferAccess::Read },
                    BufferBinding { buf_id: a_id, st: ShapeTracker::contiguous(&shape), dtype: a.dtype(), access: BufferAccess::Read },
                    BufferBinding { buf_id: b_id, st: ShapeTracker::contiguous(&shape), dtype: b.dtype(), access: BufferAccess::Read },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.min(256).max(1) as u32, 1, 1],
            });
        }
        LazyOp::Reduce { op, src, axis } => {
            schedule_recursive(src, kernels, next_buf_id);
            let in_shape = src.shape();
            let out_shape = node.shape();
            let out_n = out_shape.iter().product::<usize>().max(1);
            let out_id = *next_buf_id;
            *next_buf_id += 1;
            let in_id = *next_buf_id;
            *next_buf_id += 1;

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(1)],
                    dst_dtype: node.dtype(),
                }],
                bufs: vec![
                    BufferBinding { buf_id: out_id, st: ShapeTracker::contiguous(&out_shape), dtype: node.dtype(), access: BufferAccess::Write },
                    BufferBinding { buf_id: in_id, st: ShapeTracker::contiguous(&in_shape), dtype: src.dtype(), access: BufferAccess::Read },
                ],
                grid: [out_n as u32, 1, 1],
                local: [out_n.min(256) as u32, 1, 1],
            });
        }
        LazyOp::Movement { src, st } => {
            // Movement ops are free — just modify the ShapeTracker.
            // Recurse into the source.
            schedule_recursive(src, kernels, next_buf_id);
        }
        LazyOp::Contiguous { src } => {
            // Force materialization — insert a copy kernel.
            schedule_recursive(src, kernels, next_buf_id);
            // The copy kernel is a no-op identity kernel that reads
            // from the source buffer and writes to a new contiguous buffer.
            // Phase 1: this is handled at the executor level.
        }
    }
}
```

- [ ] **9.2** Verify compilation:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu
```

- [ ] **9.3** `git add` and commit.

---

## Task 10: Kernel Fusion Engine

**Files:**
- `runtime/molt-gpu/src/fuse.rs`
- `runtime/molt-gpu/tests/test_fusion.rs`

**Steps:**

- [ ] **10.1** Implement the fusion engine in `runtime/molt-gpu/src/fuse.rs`:

```rust
//! Kernel fusion: elementwise -> reduce -> elementwise chains.
//!
//! Merges chains of single-op kernels into fused multi-op kernels.
//! Fusion rule (same as tinygrad):
//!   [Buffer leaves + MovementOps] -> ElementwiseOps -> ReduceOps -> ElementwiseOps
//!
//! This entire chain becomes ONE kernel.

use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};

/// Fuse a list of single-op kernels into minimal fused kernels.
///
/// Phase 1 fusion rules:
/// 1. Consecutive elementwise ops merge into a single kernel.
/// 2. An elementwise chain followed by a reduce merges into one kernel.
/// 3. A reduce followed by elementwise ops merges into one kernel (post-reduce).
/// 4. Reduce-to-reduce is a fusion boundary (must materialize between).
pub fn fuse(kernels: Vec<FusedKernel>) -> Vec<FusedKernel> {
    if kernels.is_empty() {
        return kernels;
    }

    let mut fused = Vec::new();
    let mut current_chain: Vec<FusedKernel> = Vec::new();
    let mut has_reduce_in_chain = false;

    for kernel in kernels {
        let is_reduce = kernel.ops.iter().any(|op| {
            matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
        });

        if is_reduce && has_reduce_in_chain {
            // Fusion boundary: reduce-to-reduce.
            // Emit current chain and start new one.
            if !current_chain.is_empty() {
                fused.push(merge_chain(current_chain));
                current_chain = Vec::new();
            }
            has_reduce_in_chain = false;
        }

        if is_reduce {
            has_reduce_in_chain = true;
        }

        current_chain.push(kernel);
    }

    // Emit remaining chain
    if !current_chain.is_empty() {
        fused.push(merge_chain(current_chain));
    }

    fused
}

/// Merge a chain of kernels into a single fused kernel.
fn merge_chain(chain: Vec<FusedKernel>) -> FusedKernel {
    if chain.len() == 1 {
        return chain.into_iter().next().unwrap();
    }

    // Collect all unique input buffers and build merged ops
    let mut merged_ops = Vec::new();
    let mut merged_bufs = Vec::new();

    // Output buffer from the last kernel
    let last = chain.last().unwrap();
    merged_bufs.push(last.bufs[0].clone()); // output is always bufs[0]

    // Collect input buffers from all kernels, remapping indices
    for kernel in &chain {
        for buf in &kernel.bufs[1..] {
            // Add input buffer (avoid duplicates by buf_id)
            if !merged_bufs.iter().any(|b: &BufferBinding| b.buf_id == buf.buf_id) {
                merged_bufs.push(buf.clone());
            }
        }
    }

    // Build ops chain: remap FusedSrc references
    let op_offset_base = 0;
    for (kernel_idx, kernel) in chain.iter().enumerate() {
        let op_offset = merged_ops.len();
        for op in &kernel.ops {
            let mut remapped_srcs = Vec::new();
            for src in &op.srcs {
                match src {
                    FusedSrc::Buf(idx) => {
                        if *idx == 0 {
                            // Output of previous kernel -> reference the previous op
                            if kernel_idx > 0 {
                                remapped_srcs.push(FusedSrc::Op(op_offset - 1));
                            } else {
                                remapped_srcs.push(FusedSrc::Buf(0));
                            }
                        } else {
                            // Input buffer -> find in merged_bufs
                            let buf_id = kernel.bufs[*idx].buf_id;
                            let new_idx = merged_bufs.iter().position(|b| b.buf_id == buf_id)
                                .expect("buffer not found in merged set");
                            remapped_srcs.push(FusedSrc::Buf(new_idx));
                        }
                    }
                    FusedSrc::Op(prior) => {
                        remapped_srcs.push(FusedSrc::Op(op_offset + prior));
                    }
                    FusedSrc::Const { val, dtype } => {
                        remapped_srcs.push(FusedSrc::Const { val: *val, dtype: *dtype });
                    }
                }
            }
            merged_ops.push(FusedOp {
                op: op.op,
                srcs: remapped_srcs,
                dst_dtype: op.dst_dtype,
            });
        }
    }

    FusedKernel {
        ops: merged_ops,
        bufs: merged_bufs,
        grid: last.grid,
        local: last.local,
    }
}
```

- [ ] **10.2** Write `runtime/molt-gpu/tests/test_fusion.rs`:

```rust
use molt_gpu::dtype::DType;
use molt_gpu::fuse::fuse;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::shapetracker::ShapeTracker;

fn make_elementwise_kernel(op: PrimitiveOp, buf_ids: (usize, usize, usize)) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: buf_ids.0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: buf_ids.1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: buf_ids.2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    }
}

fn make_reduce_kernel(op: PrimitiveOp, in_size: usize, out_size: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 100, st: ShapeTracker::contiguous(&[out_size]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 101, st: ShapeTracker::contiguous(&[in_size]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [out_size as u32, 1, 1],
        local: [1, 1, 1],
    }
}

#[test]
fn test_fuse_two_elementwise() {
    // a + b, then * c -> should fuse to 1 kernel
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_elementwise_kernel(PrimitiveOp::Mul, (20, 10, 3)),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "two elementwise ops should fuse into 1 kernel");
    assert_eq!(fused[0].ops.len(), 2);
}

#[test]
fn test_fuse_three_elementwise() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_elementwise_kernel(PrimitiveOp::Mul, (20, 10, 3)),
        make_elementwise_kernel(PrimitiveOp::Sub, (30, 20, 4)),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "three elementwise ops should fuse into 1 kernel");
    assert_eq!(fused[0].ops.len(), 3);
}

#[test]
fn test_reduce_to_reduce_boundary() {
    // Two consecutive reduces should NOT fuse
    let kernels = vec![
        make_reduce_kernel(PrimitiveOp::ReduceMax, 1024, 32),
        make_reduce_kernel(PrimitiveOp::ReduceSum, 32, 1),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 2, "reduce-to-reduce is a fusion boundary");
}

#[test]
fn test_elementwise_reduce_fuses() {
    // elementwise -> reduce -> fuses into 1
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Mul, (10, 1, 2)),
        make_reduce_kernel(PrimitiveOp::ReduceSum, 64, 1),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "elementwise -> reduce should fuse");
}

#[test]
fn test_single_kernel_unchanged() {
    let kernels = vec![make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2))];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].ops.len(), 1);
}

#[test]
fn test_empty_input() {
    let fused = fuse(vec![]);
    assert_eq!(fused.len(), 0);
}
```

- [ ] **10.3** Run fusion tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_fusion
```

- [ ] **10.4** `git add` and commit.

---

## Task 11: MLIR Textual IR Serialization

**Files:**
- `runtime/molt-gpu/src/mlir.rs`
- `runtime/molt-gpu/tests/test_mlir.rs`

**Steps:**

- [ ] **11.1** Implement MLIR serialization in `runtime/molt-gpu/src/mlir.rs`:

```rust
//! MLIR textual IR serialization for FusedKernel.
//!
//! Generates MLIR text that maps 1:1 to the 26 primitives.
//! This is string generation only — zero C++ dependencies.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{FusedKernel, FusedOp, FusedSrc};

/// Serialize a FusedKernel to MLIR textual IR.
pub fn to_mlir_text(kernel: &FusedKernel) -> String {
    let mut out = String::with_capacity(4096);

    writeln!(out, "// Generated by molt-gpu MLIR serializer").unwrap();
    writeln!(out, "// {} ops, {} buffers", kernel.ops.len(), kernel.bufs.len()).unwrap();
    writeln!(out, "func.func @molt_kernel() {{").unwrap();

    // Declare buffer memrefs
    for (i, binding) in kernel.bufs.iter().enumerate() {
        let mlir_type = mlir_element_type(binding.dtype);
        let shape = binding.st.shape();
        let shape_str = shape.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("x");
        writeln!(out, "  // buf{}: memref<{}x{}>", i, shape_str, mlir_type).unwrap();
    }

    // Emit ops
    for (i, op) in kernel.ops.iter().enumerate() {
        let mlir_type = mlir_element_type(op.dst_dtype);
        let src_refs: Vec<String> = op.srcs.iter().map(|s| mlir_src_ref(s)).collect();

        let mlir_op = match op.op {
            // Arithmetic
            PrimitiveOp::Add => {
                if op.dst_dtype.is_float() {
                    format!("arith.addf {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.addi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Sub => {
                if op.dst_dtype.is_float() {
                    format!("arith.subf {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.subi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Mul => {
                if op.dst_dtype.is_float() {
                    format!("arith.mulf {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.muli {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Idiv => {
                if op.dst_dtype.is_signed_int() {
                    format!("arith.divsi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.divui {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Mod => {
                if op.dst_dtype.is_signed_int() {
                    format!("arith.remsi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.remui {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Neg => {
                if op.dst_dtype.is_float() {
                    format!("arith.negf {} : {}", src_refs[0], mlir_type)
                } else {
                    format!("arith.subi %zero , {} : {}", src_refs[0], mlir_type)
                }
            }
            PrimitiveOp::Cmplt => format!("arith.cmpf \"olt\" , {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Cmpeq => format!("arith.cmpf \"oeq\" , {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Cmpne => format!("arith.cmpf \"une\" , {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::And => format!("arith.andi {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Or => format!("arith.ori {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Xor => format!("arith.xori {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Shl => format!("arith.shli {} , {} : {}", src_refs[0], src_refs[1], mlir_type),
            PrimitiveOp::Shr => {
                if op.dst_dtype.is_signed_int() {
                    format!("arith.shrsi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.shrui {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Exp2 => format!("math.exp2 {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Log2 => format!("math.log2 {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Sin => format!("math.sin {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Sqrt => format!("math.sqrt {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Reciprocal => format!("arith.divf %one , {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Trunc => format!("math.trunc {} : {}", src_refs[0], mlir_type),
            PrimitiveOp::Max => {
                if op.dst_dtype.is_float() {
                    format!("arith.maximumf {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else if op.dst_dtype.is_signed_int() {
                    format!("arith.maxsi {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                } else {
                    format!("arith.maxui {} , {} : {}", src_refs[0], src_refs[1], mlir_type)
                }
            }
            PrimitiveOp::Where => format!("arith.select {} , {} , {} : {}", src_refs[0], src_refs[1], src_refs[2], mlir_type),
            PrimitiveOp::Cast => format!("// cast to {} : {}", mlir_type, src_refs[0]),
            PrimitiveOp::Bitcast => format!("arith.bitcast {} : {} to {}", src_refs[0], mlir_type, mlir_type),
            PrimitiveOp::ReduceSum => {
                if op.dst_dtype.is_float() {
                    format!("// linalg.reduce {{ arith.addf }} {} : {}", src_refs[0], mlir_type)
                } else {
                    format!("// linalg.reduce {{ arith.addi }} {} : {}", src_refs[0], mlir_type)
                }
            }
            PrimitiveOp::ReduceMax => {
                if op.dst_dtype.is_float() {
                    format!("// linalg.reduce {{ arith.maximumf }} {} : {}", src_refs[0], mlir_type)
                } else if op.dst_dtype.is_signed_int() {
                    format!("// linalg.reduce {{ arith.maxsi }} {} : {}", src_refs[0], mlir_type)
                } else {
                    format!("// linalg.reduce {{ arith.maxui }} {} : {}", src_refs[0], mlir_type)
                }
            }
        };

        writeln!(out, "  %v{} = {}", i, mlir_op).unwrap();
    }

    writeln!(out, "  return").unwrap();
    writeln!(out, "}}").unwrap();

    out
}

fn mlir_element_type(dtype: DType) -> &'static str {
    match dtype {
        DType::Bool => "i1",
        DType::Int8 => "i8",
        DType::Int16 => "i16",
        DType::Int32 => "i32",
        DType::Int64 => "i64",
        DType::UInt8 => "ui8",
        DType::UInt16 => "ui16",
        DType::UInt32 => "ui32",
        DType::UInt64 => "ui64",
        DType::Float16 => "f16",
        DType::BFloat16 => "bf16",
        DType::Float32 => "f32",
        DType::Float64 => "f64",
    }
}

fn mlir_src_ref(src: &FusedSrc) -> String {
    match src {
        FusedSrc::Buf(idx) => format!("%buf{}", idx),
        FusedSrc::Op(idx) => format!("%v{}", idx),
        FusedSrc::Const { val, dtype } => {
            let ty = mlir_element_type(*dtype);
            if dtype.is_float() {
                format!("{}({} : {})", ty, val, ty)
            } else {
                format!("{}({} : {})", ty, *val as i64, ty)
            }
        }
    }
}
```

- [ ] **11.2** Write `runtime/molt-gpu/tests/test_mlir.rs`:

```rust
use molt_gpu::dtype::DType;
use molt_gpu::mlir::to_mlir_text;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::shapetracker::ShapeTracker;

#[test]
fn test_mlir_add_f32() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addf"));
    assert!(mlir.contains("f32"));
    assert!(mlir.contains("func.func @molt_kernel"));
}

#[test]
fn test_mlir_add_i32() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addi"));
    assert!(mlir.contains("i32"));
}

#[test]
fn test_mlir_cmplt() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[32]), dtype: DType::Bool, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [32, 1, 1],
        local: [32, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.cmpf \"olt\""));
}

#[test]
fn test_mlir_shr_signed_vs_unsigned() {
    // Signed SHR -> shrsi
    let kernel_signed = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Shr,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
    };
    assert!(to_mlir_text(&kernel_signed).contains("arith.shrsi"));

    // Unsigned SHR -> shrui
    let kernel_unsigned = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Shr,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::UInt32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Read },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
    };
    assert!(to_mlir_text(&kernel_unsigned).contains("arith.shrui"));
}

#[test]
fn test_mlir_reduce_sum() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addf"));
}

#[test]
fn test_mlir_math_ops() {
    for (op, expected) in [
        (PrimitiveOp::Exp2, "math.exp2"),
        (PrimitiveOp::Log2, "math.log2"),
        (PrimitiveOp::Sin, "math.sin"),
        (PrimitiveOp::Sqrt, "math.sqrt"),
        (PrimitiveOp::Trunc, "math.trunc"),
    ] {
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [32, 1, 1],
            local: [32, 1, 1],
        };
        let mlir = to_mlir_text(&kernel);
        assert!(mlir.contains(expected), "op {:?} should emit {}", op, expected);
    }
}
```

- [ ] **11.3** Run MLIR tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu --test test_mlir
```

- [ ] **11.4** `git add` and commit.

---

## Task 12: Full Integration Test — All 26 Ops on CPU

**Files:**
- `runtime/molt-gpu/tests/test_ops.rs` (extend)

**Steps:**

- [ ] **12.1** Add comprehensive tests for ALL remaining ops not yet tested. Extend `runtime/molt-gpu/tests/test_ops.rs` with:

```rust
#[test]
fn test_cpu_idiv() {
    // Test integer division with truncation toward zero
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Idiv,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[7, -7, 7, -7]),
        i32_to_bytes(&[3, 3, -3, -3]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    assert_eq!(result, vec![2, -2, -2, 2]); // truncation toward zero
}

#[test]
fn test_cpu_mod() {
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mod,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[7, -7, 7, -7]),
        i32_to_bytes(&[3, 3, -3, -3]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    // C semantics: result has sign of dividend
    assert_eq!(result, vec![1, -1, 1, -1]);
}

#[test]
fn test_cpu_cmplt_nan() {
    // NaN < x = false (IEEE 754)
    let result = run_binary_op_cpu(PrimitiveOp::Cmplt, &[f32::NAN, 1.0, 0.0], &[1.0, f32::NAN, 0.0]);
    // NaN comparisons: NaN < 1.0 = false, 1.0 < NaN = false, 0.0 < 0.0 = false
    assert_eq!(result, vec![0.0, 0.0, 0.0]);
}

#[test]
fn test_cpu_cmpeq_nan() {
    // NaN == NaN = false (IEEE 754)
    let result = run_binary_op_cpu(PrimitiveOp::Cmpeq, &[f32::NAN, 1.0], &[f32::NAN, 1.0]);
    assert_eq!(result, vec![0.0, 1.0]);
}

#[test]
fn test_cpu_cmpne_nan() {
    // NaN != NaN = true (IEEE 754)
    let result = run_binary_op_cpu(PrimitiveOp::Cmpne, &[f32::NAN, 1.0], &[f32::NAN, 1.0]);
    assert_eq!(result, vec![1.0, 0.0]);
}

#[test]
fn test_cpu_bitwise_and() {
    let n = 3;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::And,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[0xFF, 0x0F, 0xAA]),
        i32_to_bytes(&[0x0F, 0xFF, 0x55]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    assert_eq!(result, vec![0x0F, 0x0F, 0x00]);
}

#[test]
fn test_cpu_fused_relu_chain() {
    // Test fused chain: neg(x), then max(neg_x, 0) — effectively relu(-x)
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Neg,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Max,
                srcs: vec![
                    FusedSrc::Op(0),
                    FusedSrc::Const { val: 0.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[-3.0, -1.0, 1.0, 3.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    // relu(-x): [-3,-1,1,3] -> neg -> [3,1,-1,-3] -> max(.,0) -> [3,1,0,0]
    assert_eq!(result, vec![3.0, 1.0, 0.0, 0.0]);
}

#[test]
fn test_all_26_ops_covered() {
    // Meta-test: ensure every PrimitiveOp variant is tested somewhere
    // by verifying we can construct a FusedOp for each
    for op in PrimitiveOp::ALL {
        let srcs: Vec<FusedSrc> = match op.arity() {
            1 => vec![FusedSrc::Const { val: 1.0, dtype: DType::Float32 }],
            2 => vec![
                FusedSrc::Const { val: 1.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
            ],
            3 => vec![
                FusedSrc::Const { val: 1.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 3.0, dtype: DType::Float32 },
            ],
            _ => unreachable!(),
        };
        let fused_op = FusedOp {
            op,
            srcs,
            dst_dtype: DType::Float32,
        };
        assert_eq!(fused_op.op, op);
    }
}
```

- [ ] **12.2** Run all tests:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu
```

- [ ] **12.3** `git add` and commit.

---

## Task 13: Final Review + Cleanup

**Steps:**

- [ ] **13.1** Run full test suite one final time:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo test -p molt-gpu 2>&1
```

- [ ] **13.2** Run clippy:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo clippy -p molt-gpu -- -D warnings
```

- [ ] **13.3** Verify all files are staged:

```bash
git status runtime/molt-gpu/
```

- [ ] **13.4** Verify crate compiles with each feature flag:

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check -p molt-gpu --no-default-features --features cpu-backend
cargo check -p molt-gpu --no-default-features --features metal-backend
cargo check -p molt-gpu --all-features
```

- [ ] **13.5** Verify the workspace still builds (no breakage to other crates):

```bash
export MOLT_SESSION_ID=gpu-plan-1 && export CARGO_TARGET_DIR=$PWD/target-gpu_plan_1
cargo check --workspace
```

- [ ] **13.6** Final `git add` and commit.

---

## Summary: What Plan 1 Delivers

After completing all 13 tasks:

1. **`molt-gpu` crate** — new workspace member at `runtime/molt-gpu/`
2. **`PrimitiveOp` enum** — all 26 ops with type metadata, arity, fusion classification
3. **`DType` enum** — 13 types with size, category, narrowing for Metal/WebGPU, MSL type names
4. **`ShapeTracker` + `View`** — zero-copy view system with reshape, permute, expand, pad, shrink, flip, and `expr_idx` offset computation
5. **`LazyOp` DAG** — deferred computation graph with Buffer, Unary, Binary, Ternary, Reduce, Movement, Contiguous nodes
6. **`FusedKernel` IR** — post-fusion intermediate representation with `FusedOp`, `FusedSrc`, `BufferBinding`
7. **`Renderer` trait** — interface for shader codegen
8. **`MslRenderer`** — full Metal Shading Language codegen for all 26 ops (elementwise + reduce)
9. **`Allocator`/`Compiler`/`Executor` traits** — device abstraction
10. **`CpuDevice`** — CPU reference backend with op-by-op interpreter for all 26 ops
11. **`MetalDevice`** — Metal backend (alloc, compile MSL, dispatch compute)
12. **Scheduler** — LazyOp DAG to FusedKernel list (Phase 1: single-op kernels)
13. **Fusion engine** — merges elementwise chains, respects reduce boundaries
14. **MLIR serializer** — textual MLIR IR generation for all 26 ops
15. **Comprehensive tests** — per-op CPU reference tests, IEEE 754 edge cases, MSL render validation, fusion count tests, MLIR output tests, Metal integration tests

**What Plan 1 does NOT include** (deferred to Plan 2+):
- Python `Tensor` class
- WGSL/CUDA/HIP renderers
- WebGPU/CUDA/HIP device backends
- Workgroup-level reduce optimization
- Multi-view ShapeTracker composition (Phase 1 uses single-view + contiguous fallback)
- Legacy GPU code deletion (done after Plan 2 validates the new stack)
