# Tinygrad-Conformant GPU Primitive Stack

**Date:** 2026-04-14
**Status:** Draft
**Scope:** Full GPU compute stack — primitives, ShapeTracker, Tensor API, kernel fusion, TurboQuant, DFlash+DDTree

---

## 1. Problem Statement

Molt's current GPU subsystem is a bespoke pipeline (13 files, ~13,000 LOC) that generates per-kernel MSL/WGSL/CUDA/HIP source from ad-hoc `GpuKernel` structs. It works for trivial vector_add-class kernels but cannot express the operations needed for real ML inference (Falcon-OCR, DFlash speculative decoding, TurboQuant quantization). The API surface (`tensor_linear`, `tensor_softmax_last_axis`, etc.) is a collection of one-off free functions that don't compose and don't match any established framework.

**Goal:** Replace the entire GPU subsystem with a tinygrad-conformant primitive stack that:
1. Compiles unmodified tinygrad Python code through molt
2. Implements all of deep learning with **3 OpTypes** and **26 compute primitives**
3. Generates fused kernels for Metal, WebGPU (WGSL), CUDA, and HIP
4. Enables TurboQuant, DFlash, and DDTree as pure compositions of those primitives
5. Runs on native, WASM, and browser WebGPU targets

## 2. Architecture: The Tinygrad Way

### 2.1 The 3 OpTypes

All computation reduces to three categories (George Hotz, tinygrad.org):

**ElementwiseOps** — operate on 1-3 tensors, run elementwise, fuse freely:
- UnaryOps: `NEG`, `RECIPROCAL`, `SQRT`, `EXP2`, `LOG2`, `SIN`, `TRUNC`, `CAST`, `BITCAST`
- BinaryOps: `ADD`, `SUB`, `MUL`, `IDIV`, `MOD`, `MAX`, `CMPLT`, `CMPEQ`, `CMPNE`, `AND`, `OR`, `XOR`, `SHL`, `SHR`
- TernaryOps: `WHERE`

**ReduceOps** — operate on one tensor, return a smaller tensor:
- `SUM`, `MAX`

**MovementOps** — virtual ops, zero-cost, no kernel generated:
- `RESHAPE`, `PERMUTE`, `EXPAND`, `PAD`, `SHRINK`, `FLIP`

### 2.2 The Compute Primitives (1:1 with tinygrad's renderer `code_for_op`)

Cross-referenced against tinygrad's `CStyleLanguage.code_for_op` (primary source of truth for what a backend must implement). Every op in tinygrad's renderer is a primitive here. No fewer, no more.

**Arithmetic (6 ops):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 1 | `ADD` | Binary | `a + b` |
| 2 | `SUB` | Binary | `a - b` |
| 3 | `MUL` | Binary | `a * b` |
| 4 | `IDIV` | Binary | `a / b` (integer, C semantics: truncates toward zero) |
| 5 | `MOD` | Binary | `a % b` (C semantics: result has sign of dividend. `(-7) % 3 = -1`, not `2`) |
| 6 | `NEG` | Unary | `-a` (NOT `a * -1` — different for `-0.0`, NaN) |

**Comparison (3 ops):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 7 | `CMPLT` | Binary | `a < b ? 1 : 0` → output dtype is always `dtypes.bool`. NaN comparisons follow IEEE 754: `NaN < x = false`. |
| 8 | `CMPEQ` | Binary | `a == b ? 1 : 0` → output dtype is always `dtypes.bool`. `NaN == NaN = false` (IEEE 754). |
| 9 | `CMPNE` | Binary | `a != b ? 1 : 0` → output dtype is always `dtypes.bool`. `NaN != NaN = true` (IEEE 754). |

**Bitwise (5 ops):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 10 | `AND` | Binary | `a & b` |
| 11 | `OR` | Binary | `a \| b` |
| 12 | `XOR` | Binary | `a ^ b` |
| 13 | `SHL` | Binary | `a << b` (logical left shift) |
| 14 | `SHR` | Binary | `a >> b` (arithmetic right shift for signed types: sign-extending. Logical right shift for unsigned types: zero-filling) |

**Math (5 ops):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 15 | `EXP2` | Unary | `exp2(a)` |
| 16 | `LOG2` | Unary | `log2(a)` |
| 17 | `SIN` | Unary | `sin(a)` |
| 18 | `SQRT` | Unary | `sqrt(a)` |
| 19 | `RECIPROCAL` | Unary | `1.0 / a` (float-only. `RECIPROCAL(0.0) = +inf`, `RECIPROCAL(-0.0) = -inf` per IEEE 754. Not valid for integer types — use `IDIV(1, a)` instead.) |

**Other (4 ops):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 20 | `TRUNC` | Unary | `trunc(a)` (needed for floor/ceil/round) |
| 21 | `MAX` | Binary | `max(a, b)` (IEEE 754: NaN-propagating — if either operand is NaN, result is NaN. Maps to `fmax` in MSL, `max` in CUDA/HIP. For integers, standard comparison.) |
| 22 | `WHERE` | Ternary | `cond ? a : b` |
| 23 | `CAST` | Unary | `(target_type)a` (type conversion) |

**Plus handled via specialized patterns (not in `code_for_op`):**

| # | Primitive | Type | Kernel Pattern |
|---|-----------|------|---------------|
| 24 | `BITCAST` | Unary | Reinterpret bits as different type |
| 25 | `REDUCE_SUM` | Reduce | `Σ a[i] over axis` |
| 26 | `REDUCE_MAX` | Reduce | `max(a[i]) over axis` |

**Total: 26 primitive ops.** This matches tinygrad's `CStyleLanguage.code_for_op` backend contract.

**MULACC (fused multiply-accumulate):** tinygrad has `MULACC(a, b, c) = a * b + c` as an optional ternary op for backends with native FMA. Molt does NOT include MULACC as a primitive — it is composed as `ADD(MUL(a, b), c)`. The fusion engine will fuse this into a single kernel. Molt's tinygrad compatibility targets the Python `Tensor` API (method signatures and semantics), not tinygrad's internal linearized IR. Molt generates its own kernel IR from Tensor method calls independently.

**CONTIGUOUS is not a compute op.** It is a scheduling annotation (present as a `LazyOp` variant for DAG construction, but generates no `PrimitiveOp` — the scheduler handles it by inserting a copy kernel directly).

Everything else is composition:
- `DIV(a, b)` = `MUL(a, RECIPROCAL(b))` (float division — IDIV is the primitive for integer division)
- `SUB(a, b)` is a primitive (not `ADD(a, NEG(b))`) — semantically distinct for `-0.0`
- `exp(x)` = `EXP2(MUL(x, LOG2_E))`
- `log(x)` = `MUL(LOG2(x), LN_2)`
- `relu(x)` = `MAX(x, 0)`
- `sigmoid(x)` = `RECIPROCAL(ADD(1, EXP2(MUL(NEG(x), LOG2_E))))`
- `softmax(x)` = `m = REDUCE_MAX(x); e = EXP2(MUL(SUB(x, EXPAND(m)), LOG2_E)); MUL(e, RECIPROCAL(EXPAND(REDUCE_SUM(e))))`
- `floor(x)` = `WHERE(CMPEQ(x, TRUNC(x)), x, SUB(TRUNC(x), WHERE(CMPLT(x, 0), 1, 0)))`
- `matmul(A, B)` = `REDUCE_SUM(MUL(EXPAND(RESHAPE(A)), EXPAND(RESHAPE(B))), axis=-1)` — reshapes and expands are free
- `conv2d` = same pattern with different reshapes

### 2.3 ShapeTracker (Zero-Copy View System)

All MovementOps are handled by ShapeTracker — no GPU kernel, no memory copy:

```rust
/// A stack of views that tracks how a contiguous buffer is accessed.
/// All movement operations (reshape, permute, expand, pad, shrink, flip)
/// are O(1) modifications to the view stack.
pub struct ShapeTracker {
    pub views: Vec<View>,
}

/// A single view into a contiguous buffer.
pub struct View {
    /// Logical shape of this view.
    pub shape: Vec<usize>,
    /// Stride per dimension. 0 = broadcast, negative = flipped.
    pub strides: Vec<i64>,
    /// Offset into the underlying buffer (in elements).
    pub offset: i64,
    /// Optional validity mask per dimension: element is valid iff
    /// mask[dim].0 <= idx[dim] < mask[dim].1. Values can be negative
    /// (required for causal attention masking and negative padding).
    /// Elements outside the mask return the padding value (typically 0).
    pub mask: Option<Vec<(i64, i64)>>,
}
```

### 2.3.1 Supported Types (`DType`)

```rust
pub enum DType {
    Bool,                    // 1-bit logical (stored as u8)
    Int8, Int16, Int32, Int64,
    UInt8, UInt16, UInt32, UInt64,
    Float16, BFloat16, Float32, Float64,
}
```

Backend narrowing: WebGPU lacks `Float64` and `Int64` (narrowed to `Float32`/`Int32` by `WgslRenderer`). Metal lacks `Float64` (narrowed to `Float32` by `MslRenderer`). CUDA and HIP support all types. `CAST` and `BITCAST` are defined for all valid (src, dst) pairs within a backend's supported types.

### 2.3.2 Multi-View ShapeTracker

`ShapeTracker.views` has one entry for simple cases. A new view is pushed when a movement op is applied to a non-contiguous view and cannot be absorbed into the current view (e.g., `pad(permute(x))` where the permute made strides non-standard). For `expr_idx` with multiple views, views are composed from outer to inner: the outermost view maps logical index → intermediate index, then the next view maps intermediate → next, and the innermost view maps to the actual buffer offset. If any view's mask excludes the index, `expr_idx` returns `None` (padding). In Phase 1, the implementation focuses on single-view correctness and falls back to `contiguous()` insertion when a movement would require a second view that cannot be merged.

Key method: `expr_idx(linear_idx) -> Option<usize>` — computes the actual memory offset from a logical index using strides. Returns `None` for elements outside the mask (padding region), which the kernel renders as the padding constant (0). The expression `offset + Σ(idx[i] * strides[i])` is compiled directly into kernel index code, with a bounds check for masked dimensions.

### 2.4 Tensor API (tinygrad-compatible)

The `Tensor` class matches tinygrad's method signatures so tinygrad code compiles directly:

```python
class Tensor:
    _buf: Buffer          # GPU/CPU buffer handle
    _st: ShapeTracker     # view stack
    _dtype: DType         # element type
    
    # --- Creation ---
    @staticmethod
    def zeros(*shape, dtype=dtypes.float32) -> Tensor: ...
    @staticmethod
    def ones(*shape, dtype=dtypes.float32) -> Tensor: ...
    @staticmethod
    def rand(*shape, dtype=dtypes.float32) -> Tensor: ...
    @staticmethod
    def eye(n, dtype=dtypes.float32) -> Tensor: ...
    
    # --- Unary (dispatch to primitives) ---
    def exp(self) -> Tensor: ...     # EXP2(self * LOG2_E)
    def log(self) -> Tensor: ...     # LOG2(self) * LN_2
    def sqrt(self) -> Tensor: ...    # SQRT
    def sin(self) -> Tensor: ...     # SIN
    def cos(self) -> Tensor: ...     # SIN(self + PI/2) — tinygrad-conformant; precision loss for |x| > 2^23 is acceptable for ML workloads
    def reciprocal(self) -> Tensor: ...  # RECIPROCAL
    def neg(self) -> Tensor: ...     # NEG primitive
    def relu(self) -> Tensor: ...    # MAX(self, 0)
    def sigmoid(self) -> Tensor: ... # composed
    def tanh(self) -> Tensor: ...    # composed
    def gelu(self) -> Tensor: ...    # composed
    
    # --- Binary ---
    def __add__(self, other) -> Tensor: ...  # ADD
    def __mul__(self, other) -> Tensor: ...  # MUL
    def __sub__(self, other) -> Tensor: ...  # SUB primitive
    def __truediv__(self, other) -> Tensor: ...  # MUL(self, RECIPROCAL(other))
    def maximum(self, other) -> Tensor: ...  # MAX
    def __floordiv__(self, other) -> Tensor: ...  # IDIV (integer) or TRUNC(MUL(self, RECIPROCAL(other))) (float)
    def __mod__(self, other) -> Tensor: ...  # MOD
    def __and__(self, other) -> Tensor: ...   # AND
    def __or__(self, other) -> Tensor: ...    # OR
    def __xor__(self, other) -> Tensor: ...   # XOR
    def __lshift__(self, other) -> Tensor: ... # SHL
    def __rshift__(self, other) -> Tensor: ... # SHR
    
    # --- Reduce ---
    def sum(self, axis=None) -> Tensor: ...     # REDUCE_SUM
    def max(self, axis=None) -> Tensor: ...     # REDUCE_MAX
    def mean(self, axis=None) -> Tensor: ...    # REDUCE_SUM / count
    def argmax(self, axis=-1) -> Tensor: ...    # comparison reduction
    def topk(self, k, axis=-1) -> Tensor: ...   # iterative argmax + mask
    def softmax(self, axis=-1) -> Tensor: ...   # composed
    def log_softmax(self, axis=-1) -> Tensor: ... # composed
    
    # --- Movement (zero-cost via ShapeTracker) ---
    def reshape(self, *shape) -> Tensor: ...
    def permute(self, *order) -> Tensor: ...
    def expand(self, *shape) -> Tensor: ...
    def pad(self, padding, value=0.0) -> Tensor: ...
    def shrink(self, arg) -> Tensor: ...
    def flip(self, axis) -> Tensor: ...
    def contiguous(self) -> Tensor: ...   # only op that generates kernel
    def T(self) -> Tensor: ...            # permute
    def flatten(self, start=0) -> Tensor: ...  # reshape
    def unsqueeze(self, dim) -> Tensor: ...    # reshape
    def squeeze(self, dim=None) -> Tensor: ... # reshape
    
    # --- Matrix ops (composed from primitives) ---
    def dot(self, other) -> Tensor: ...     # RESHAPE + EXPAND + MUL + REDUCE_SUM
    def matmul(self, other) -> Tensor: ...  # same as dot
    
    @staticmethod
    def cat(*tensors, dim=0) -> Tensor: ...  # PAD each tensor to aligned shape, then ADD
    @staticmethod
    def stack(*tensors, dim=0) -> Tensor: ... # unsqueeze each, then cat
    
    # --- Indexing ---
    # NOTE: gather/scatter are NOT free compositions. They use masked select
    # over expanded index comparison: O(n * vocab_size) memory, not O(n).
    # For performance-critical paths (KV cache compaction), measure and
    # consider promoting to a 14th primitive if the expansion is too expensive.
    def gather(self, idx, axis) -> Tensor: ...
    def scatter(self, idx, src, axis) -> Tensor: ...
    def __getitem__(self, key) -> Tensor: ...  # shrink/stride/reshape
    
    # --- Specialized (composed, not primitives) ---
    def scaled_dot_product_attention(self, key, value,
                                      attn_mask=None, is_causal=False) -> Tensor: ...
    def layernorm(self, weight, bias=None, eps=1e-5) -> Tensor: ...
    def conv2d(self, weight, bias=None, stride=1, padding=0) -> Tensor: ...
```

### 2.5 Lazy Evaluation Model

Every `Tensor` method returns a new `Tensor` backed by a `LazyOp` node. No kernel is executed until `realize()` is called. This is how tinygrad achieves kernel fusion — by seeing the full computation graph before generating any code.

```rust
/// A node in the lazy computation DAG.
pub enum LazyOp {
    /// Leaf: a realized buffer with known contents.
    Buffer { buf: DeviceBuffer, st: ShapeTracker, dtype: DType },
    /// Elementwise unary op (NEG, RECIPROCAL, SQRT, EXP2, LOG2, SIN, TRUNC, CAST, BITCAST).
    Unary { op: PrimitiveOp, src: Arc<LazyOp> },
    /// Elementwise binary op (ADD, SUB, MUL, IDIV, MOD, MAX, CMPLT, CMPEQ, CMPNE, AND, OR, XOR, SHL, SHR).
    Binary { op: PrimitiveOp, lhs: Arc<LazyOp>, rhs: Arc<LazyOp> },
    /// Ternary op (WHERE).
    Ternary { op: PrimitiveOp, cond: Arc<LazyOp>, a: Arc<LazyOp>, b: Arc<LazyOp> },
    /// Reduce op (REDUCE_SUM, REDUCE_MAX) over a single axis.
    /// The axis dimension is REMOVED from the output shape (no keepdim).
    /// Callers needing keepdim=True compose with a subsequent EXPAND
    /// (which is free via ShapeTracker — stride=0 broadcast).
    /// Multi-axis reduces are chained: reduce(axis=1) then reduce(axis=0).
    Reduce { op: PrimitiveOp, src: Arc<LazyOp>, axis: usize },
    /// Movement op (free — just modifies ShapeTracker).
    /// The `st` here is the RESULT view, not an incremental delta.
    /// When generating index expressions, this `st` is used directly —
    /// the source's `st` is irrelevant because the Movement node
    /// represents the composed view (e.g., reshape(permute(x)) has a
    /// single Movement node whose `st` is the composed reshape+permute view).
    Movement { src: Arc<LazyOp>, st: ShapeTracker },
    /// Scheduling annotation: force materialization here.
    /// NOT a compute op — the scheduler handles this by inserting a copy kernel
    /// that reads from the source buffer using the source's ShapeTracker index
    /// expression and writes to a new contiguous buffer with identity strides.
    /// This copy kernel uses no PrimitiveOp — it is a special-case kernel that
    /// the scheduler generates directly (not through the fusion engine).
    Contiguous { src: Arc<LazyOp> },
}
```

**Execution flow:**
1. Python `Tensor` methods build a `LazyOp` DAG (no GPU work)
2. `Tensor.realize()` calls `schedule(dag)` → topologically sorted `Vec<FusedKernel>`
3. `schedule()` applies fusion rules to merge chains into minimal kernels
4. Each `FusedKernel` is rendered to shader source, compiled, and executed
5. Output buffers become `LazyOp::Buffer` leaves for subsequent ops

**Materialization triggers:**
- Explicit `realize()` call
- Reading data back to CPU (`.numpy()`, `.tolist()`)
- Cross-device transfer
- `Contiguous` scheduling annotation

### 2.6 Kernel Fusion

At compile time, molt fuses chains of elementwise ops into single GPU kernels:

**Fusion rule (same as tinygrad):**
```
[Buffer leaves + MovementOps] → ElementwiseOps → ReduceOps → ElementwiseOps
```

This entire chain becomes ONE kernel. The elementwise stages may read from **any number of input Buffer leaves** — the chain pattern describes the op topology (elementwise → reduce → elementwise), not the input count. For example, softmax's Kernel 2 reads both `x` (original input) and `EXPAND(m)` (Kernel 1's output) in its pre-reduce elementwise section — this is a single fused kernel with two input buffers.

Fusion boundaries are:
- ReduceOp output must materialize before the next reduce
- `contiguous()` forces materialization
- Cross-device transfers

Example — `softmax(x)` fuses into **2 kernels** (not 7 individual ops, not 3):
```
Kernel 1: REDUCE_MAX(x, axis)                                              → m
Kernel 2: pre-reduce elementwise: EXP2(MUL(SUB(x, EXPAND(m)), LOG2_E))
          reduce: REDUCE_SUM(_, axis)                                       → s
          post-reduce elementwise: MUL(_, RECIPROCAL(EXPAND(s)))            → out
```
The entire second kernel is one fused chain: elementwise → reduce → elementwise.
EXPAND of `m` and `s` back to original shape is free via ShapeTracker (stride=0 broadcast).
This matches tinygrad's actual kernel count for softmax.

### 2.7 Device Abstraction (Replaces `GpuDevice` Trait)

The current `GpuDevice` trait is replaced by tinygrad's three-component model — three separate traits with distinct ownership and lifetime semantics:

```rust
/// Memory management. Owns buffer lifetimes.
/// SAFETY CONTRACT: `free()` internally synchronizes — it waits for all outstanding
/// kernels referencing this buffer to complete before releasing GPU memory.
/// Callers do NOT need to call `synchronize()` before `free()`.
pub trait Allocator: Send + Sync {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError>;
    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError>;
    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError>;
    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError>;
}

/// Kernel compilation. Owns compiled program cache internally.
pub trait Compiler: Send + Sync {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError>;
    // Cache is internal — repeated compile() with same source returns cached program.
    
    /// Backend limits for the scheduler to compute grid/local work distribution.
    fn max_local_size(&self) -> [u32; 3];  // e.g., Metal: [1024,1024,1024], WebGPU: [256,256,64]
    fn max_grid_size(&self) -> [u32; 3];   // e.g., Metal: [2^31,2^31,2^31]
}

/// Kernel execution. References programs and buffers it does not own.
pub trait Executor: Send + Sync {
    fn exec(&self, prog: &CompiledProgram, bufs: &[&DeviceBuffer],
            grid: [u32; 3], local: [u32; 3]) -> Result<(), DeviceError>;
    fn synchronize(&self) -> Result<(), DeviceError>;
}
```

Each backend (MetalDevice, WebGpuDevice, etc.) is a concrete struct that implements all three traits. The device pool is also internal to the backend — no separate `device_pool.rs` or `kernel_cache.rs` files. The cache lives inside the `Compiler` impl.

### Target Parity: Every Backend Tinygrad Supports, Plus More

Tinygrad supports 17 backends. Molt must support all of them plus WASM/browser targets tinygrad lacks:

**Phase 1 backends (this spec):**
- `MetalDevice` — Apple GPUs (macOS). Dev machine, first backend.
- `WebGpuDevice` — WebGPU via wgpu (native + browser via JS FFI). Browser WebGPU is a host-dispatch boundary on the same device, not a separate backend.
- `CudaDevice` — NVIDIA GPUs via CUDA runtime.
- `HipDevice` — AMD GPUs via HIP/ROCm.
- `CpuDevice` — CPU fallback via Cranelift (testing, correctness reference).

**Phase 2 backends (after Phase 1 is solid):**
- `OpenClDevice` — Broadest GPU compatibility (Intel, ARM Mali, etc.)
- `CloudflareDevice` — Cloudflare Workers (WebGPU in Workers environment)

**Deferred (require HCQ-level driver work, ~100x implementation cost):**
- `NvDevice` — NVIDIA direct driver (HCQ, no CUDA runtime)
- `AmdDevice` — AMD direct driver (HCQ, no ROCm runtime)
- `QcomDevice` — Qualcomm Adreno (mobile/embedded)
- `DspDevice`, `RdmaDevice`, `DiskDevice` — specialized backends

### 2.8 Codegen Renderers

Each backend gets a renderer that converts fused primitive chains to shader source:

| Backend | Renderer | Output | Notes |
|---------|----------|--------|-------|
| Metal | `MslRenderer` | MSL source | f64→f32 narrowing, `thread_position_in_grid` |
| WebGPU | `WgslRenderer` | WGSL source | i64→i32/f64→f32 narrowing, `global_invocation_id` |
| CUDA | `CudaRenderer` | CUDA C source | Full i64/f64, `blockIdx.x * blockDim.x + threadIdx.x` |
| HIP | `HipRenderer` | HIP C++ source | Same as CUDA with HIP intrinsics |

The post-fusion IR passed to renderers:

```rust
/// A single fused kernel ready for codegen.
pub struct FusedKernel {
    /// Ordered chain of ops: elementwise prefix → optional reduce → elementwise suffix.
    pub ops: Vec<FusedOp>,
    /// Buffer bindings. Convention: bufs[0] is ALWAYS the output (Write access).
    /// bufs[1..] are inputs (Read access). ReadWrite is used for in-place ops.
    pub bufs: Vec<BufferBinding>,
    /// Work distribution. Computed by the scheduler, NOT the renderer.
    /// The scheduler queries backend limits (max threads/group, max grid dims)
    /// via the Compiler trait before setting these values.
    /// The renderer takes these as-is and embeds them in the shader source.
    pub grid: [u32; 3],
    pub local: [u32; 3],
}

/// A single op in a fused chain.
pub struct FusedOp {
    pub op: PrimitiveOp,          // ADD, MUL, EXP2, REDUCE_SUM, etc.
    pub srcs: Vec<FusedSrc>,      // input sources (explicit, not ambiguous indices)
    pub dst_dtype: DType,         // output dtype (always dtypes.bool for CMP* ops)
}

/// Source reference for a fused op.
pub enum FusedSrc {
    /// Index into FusedKernel.bufs (an input/output buffer).
    Buf(usize),
    /// Index into FusedKernel.ops (a prior op's result, must be < current op index).
    Op(usize),
    /// Scalar constant. Broadcast to all elements via stride=0.
    /// Used for: relu's 0, sigmoid's 1, softmax's LOG2_E, etc.
    Const { val: f64, dtype: DType },
}

/// Buffer access mode.
pub enum BufferAccess {
    Read,       // input buffer (const device T* in MSL)
    Write,      // output buffer (device T* in MSL)
    ReadWrite,  // in-place mutation
}

/// A buffer binding with its view into memory.
pub struct BufferBinding {
    pub buf_id: usize,            // runtime buffer handle index
    pub st: ShapeTracker,         // how this buffer is accessed
    pub dtype: DType,             // element type
    pub access: BufferAccess,
}
```

Each renderer implements:
```rust
pub trait Renderer {
    /// Render a fused kernel into shader source code.
    fn render(&self, kernel: &FusedKernel) -> String;
}
```

## 3. TurboQuant Integration

TurboQuant decomposes entirely to the 26 primitives — no new ops needed.

### 3.1 PolarQuant (Stage 1)

Convert vectors from Cartesian to polar coordinates for quantization:

```python
# Radius: sqrt(x² + y²)  — SQRT(ADD(MUL(x,x), MUL(y,y)))
radius = (x * x + y * y).sqrt()

# Angle: atan2(y, x) — composed via polynomial atan + domain reduction + quadrant fix.
#
# Step 1: Domain reduction. atan(r) is only accurate for |r| <= 1.
#   r = MUL(y, RECIPROCAL(x))
#   use_recip = CMPLT(1.0, abs(r))   # |r| > 1 → use atan(1/r) identity
#   r_reduced = WHERE(use_recip, RECIPROCAL(r), r)
#
# Step 2: Polynomial approximation of atan(r) on [-1, 1].
#   Use Remez/minimax coefficients (NOT Taylor — Taylor converges slowly at |r|=1).
#   7th-order minimax on [-1,1] achieves ~2e-7 max error:
#   atan(r) ≈ r * (c1 + r² * (c3 + r² * (c5 + r² * c7)))
#   where c1 ≈ 0.99997726, c3 ≈ -0.33262347, c5 ≈ 0.19354346, c7 ≈ -0.11643287
#   (exact Remez coefficients must be computed or sourced from a reference impl)
#
# Step 3: Fix |r| > 1 case: atan(r) = sign(r) * π/2 - atan(1/r)
#   result = WHERE(use_recip, SUB(MUL(sign_r, PI_OVER_2), poly), poly)
#
# Step 4: Quadrant correction based on signs of x, y:
#   x < 0 and y >= 0 → result + π
#   x < 0 and y < 0  → result - π
#   Composed via CMPLT(x, 0), CMPLT(y, 0), WHERE, ADD
#
# All GPUs have native atan2. If polynomial perf is unacceptable,
# promote to a 27th primitive. Measure first.
angle = _atan2_composed(y, x)

# Fixed-grid quantization
q_radius = (radius * grid_size).cast(dtypes.int32)
q_angle = (angle * grid_size / (2 * math.pi)).cast(dtypes.int32)
```

### 3.2 QJL Error Correction (Stage 2)

Random projection to sign bits:

```python
# Random matrix (persistent seed for reproducibility)
R = Tensor.rand(d, k)  # projection matrix

# Project and sign-extract
projected = x.dot(R)                          # matmul (composed)
signs = projected.cmplt(Tensor.zeros(k))      # CMPLT primitive
```

### 3.3 Quantized Attention Kernel

4-bit attention logit computation:

```python
# Dequantize keys
keys_f32 = (q_keys.cast(dtypes.float32) - zero_point) * scale  # CAST + ADD + MUL

# Standard attention with dequantized keys
attn = query.dot(keys_f32.T) * (1.0 / math.sqrt(d_k))  # composed
attn = attn.softmax(axis=-1)                              # composed
out = attn.dot(values)                                     # composed
```

## 4. DFlash + DDTree Integration

### 4.1 DFlash (Block Diffusion Drafter)

The drafter is a standard transformer that produces per-position token distributions in one forward pass. Its forward pass is entirely composed from the 26 primitives:
- Linear layers: `dot` (composed)
- Attention: `scaled_dot_product_attention` (composed)
- RMSNorm: `REDUCE_SUM` + `SQRT` + `RECIPROCAL` + `MUL`
- Softmax: composed
- RoPE: `SIN` + `MUL` + `ADD`

### 4.2 DDTree (Draft Tree Construction)

**Algorithm 1** runs on CPU — it's a heap algorithm, not a GPU kernel:

```python
def build_ddtree(marginals: list[Tensor], budget: int, top_k: int) -> DraftTree:
    """
    Build optimal draft tree from per-position marginal distributions.
    
    Args:
        marginals: L tensors of shape [vocab_size], one per future position
        budget: maximum number of nodes B
        top_k: K most probable tokens per position
    
    Returns:
        DraftTree with B nodes, maximizing expected acceptance length
    """
    # Extract top-K tokens and log-probs per position (GPU)
    top_tokens = [m.topk(top_k) for m in marginals]  # composed from primitives
    log_probs = [m.log_softmax(axis=-1) for m in marginals]  # composed
    
    # Best-first heap construction (CPU, O(B log B))
    # Score σ(ρ) = Σ log q_i^(ρ_i) — additive over positions.
    # Each heap entry stores the FULL path score for correct sibling computation.
    heap = MaxHeap()
    heap.push((log_probs[0][0], (0,)))  # rank tuple (1,) at depth 1
    
    tree = DraftTree()
    while len(tree) < budget and not heap.empty():
        score, ranks = heap.pop()
        tree.add(ranks)
        
        d = len(ranks)
        rho_d = ranks[-1]
        
        # Push sibling: replace last rank's contribution.
        # σ(sibling) = σ(parent_prefix) + log q_d^(ρ_d+1)
        # = score - log q_d^(ρ_d) + log q_d^(ρ_d+1)
        # This is correct because score = Σ_{i=1}^{d} log q_i^(ρ_i)
        # and we are replacing only the d-th term.
        if rho_d + 1 < top_k:
            sibling_ranks = ranks[:-1] + (rho_d + 1,)
            sibling_score = score - log_probs[d-1][rho_d] + log_probs[d-1][rho_d + 1]
            heap.push((sibling_score, sibling_ranks))
        
        # Push first child: extend path by best token at next position.
        # σ(child) = score + log q_{d+1}^(1)
        if d < len(marginals):
            child_ranks = ranks + (0,)
            child_score = score + log_probs[d][0]
            heap.push((child_score, child_ranks))
    
    return tree
```

### 4.3 Tree Attention Mask (GPU)

Ancestor-only attention mask for verifier:

```python
# Compose mask from primitives
def build_tree_attention_mask(tree: DraftTree) -> Tensor:
    n = len(tree)
    # is_ancestor[i][j] = 1 if node j is ancestor of node i
    mask = Tensor.zeros(n, n)
    for i, node in enumerate(tree.nodes):
        for ancestor_idx in node.ancestor_indices:
            mask = mask.scatter(...)  # composed from primitives
    # Convert to attention mask: 0 for attend, -inf for don't
    return mask.where(Tensor.zeros(n, n), Tensor.ones(n, n) * float('-inf'))
```

### 4.4 KV Cache Compaction

After tree-walk verification, compact KV cache to accepted path:

```python
accepted_indices = tree.walk(target_logits)  # CPU
# GPU gather on accepted indices
new_keys = keys.gather(accepted_indices, axis=1)    # composed
new_values = values.gather(accepted_indices, axis=1) # composed
```

## 5. Legacy Code Migration

**No backward compatibility. Delete and replace.**

### 5.1 Files Deleted (current GPU subsystem)

| File | Lines | Reason |
|------|-------|--------|
| `tir/gpu.rs` | 155 | Replaced by `PrimitiveOp` enum |
| `tir/gpu_pipeline.rs` | 429 | Replaced by fused kernel pipeline |
| `tir/gpu_dispatch.rs` | 194 | Replaced by `MoltDevice` dispatch |
| `tir/gpu_msl.rs` | 430 | Replaced by `MslRenderer` |
| `tir/gpu_wgsl.rs` | 554 | Replaced by `WgslRenderer` |
| `tir/gpu_cuda.rs` | 412 | Replaced by `CudaRenderer` |
| `tir/gpu_hip.rs` | 428 | Replaced by `HipRenderer` |
| `tir/gpu_metal.rs` | 572 | Replaced by `MetalDevice` (new) |
| `tir/gpu_webgpu.rs` | 457 | Replaced by `WebGpuDevice` (new) |
| `tir/gpu_cuda_runtime.rs` | 71 | Replaced by `CudaDevice` |
| `tir/gpu_mlx.rs` | 145 | Removed (MLX via Metal primitives) |
| `tir/gpu_runtime.rs` | 191 | Replaced by `MoltDevice` trait |
| `molt-runtime/src/builtins/gpu.rs` | 8990 | Replaced by `Tensor` + `MoltDevice` |

**Total deleted:** ~13,028 lines of ad-hoc GPU code

### 5.2 Enjoice Migration

The `tensor_linear`, `tensor_softmax_last_axis`, etc. free-function API in enjoice's `main_molt.py` gets migrated to `Tensor` method calls:

| Old (enjoice) | New (tinygrad-conformant) |
|--------------|--------------------------|
| `tensor_linear(x, w)` | `x.dot(w)` |
| `tensor_softmax_last_axis(x)` | `x.softmax(axis=-1)` |
| `tensor_scaled_dot_product_attention(q, k, v)` | `q.scaled_dot_product_attention(k, v)` |
| `tensor_permute_dims(x, order)` | `x.permute(*order)` |
| `tensor_reshape_view(x, shape)` | `x.reshape(*shape)` |
| `tensor_take_rows(x, idx)` | `x[idx]` or `x.gather(idx, 0)` |
| `tensor_scatter_rows(x, idx, src)` | `x.scatter(idx, src, 0)` |
| `tensor_concat_first_dim(a, b)` | `Tensor.cat(a, b, dim=0)` |
| `zeros(shape)` | `Tensor.zeros(*shape)` |
| `ones(shape)` | `Tensor.ones(*shape)` |

## 6. Module Structure

```
runtime/molt-gpu/                     # NEW CRATE — primitives only, no application code
├── Cargo.toml
├── src/
│   ├── lib.rs                        # public API
│   ├── ops.rs                        # PrimitiveOp enum (26 ops, 1:1 tinygrad)
│   ├── dtype.rs                      # DType (float32, float16, int32, etc.)
│   ├── shapetracker.rs               # ShapeTracker + View
│   ├── lazy.rs                       # LazyOp DAG nodes (deferred computation)
│   ├── schedule.rs                   # DAG → topological kernel schedule
│   ├── fuse.rs                       # kernel fusion (elementwise → reduce → elementwise)
│   ├── render/
│   │   ├── mod.rs                    # Renderer trait + FusedKernel IR
│   │   ├── msl.rs                    # Metal Shading Language
│   │   ├── wgsl.rs                   # WebGPU Shading Language
│   │   ├── cuda.rs                   # CUDA C
│   │   └── hip.rs                    # HIP C++
│   └── device/
│       ├── mod.rs                    # Allocator + Compiler + Executor traits
│       ├── metal.rs                  # Metal backend (device pool + kernel cache internal)
│       ├── webgpu.rs                 # WebGPU backend (native + WASM)
│       ├── cuda.rs                   # CUDA backend
│       ├── hip.rs                    # HIP backend
│       └── cpu.rs                    # CPU fallback (for testing)
```

**No `quantize/`, no `speculative/`, no `tensor.rs` in the Rust crate.**

TurboQuant, DFlash, DDTree, and all higher-level operations are pure Python compositions of the 26 primitives via the Tensor API. They live in application code, not the GPU crate. The Tensor class itself is Python — it records `LazyOp` nodes and calls into Rust only for `realize()`.

Application-level Python files live in the project that uses them:
```
# In molt stdlib or application code, NOT in molt-gpu
stdlib/molt/gpu/tensor.py             # Tensor class (lazy ops, calls molt-gpu for realize)
stdlib/molt/gpu/nn.py                 # layernorm, conv2d, sdpa (compositions)
# In falcon-ocr / enjoice / application
turbo_quant.py                        # PolarQuant + QJL (pure Tensor API)
dflash.py                             # block diffusion drafter (pure Tensor API)
ddtree.py                             # draft tree construction (CPU heap + Tensor.topk)
```

## 7. Testing Strategy

### 7.1 Primitive Op Tests

Each of the 26 primitives gets a test per backend:

```
test_add_{metal,webgpu,cuda,hip}
test_mul_{metal,webgpu,cuda,hip}
test_reduce_sum_{metal,webgpu,cuda,hip}
...
```

Test pattern: generate random inputs on CPU, run on GPU, compare bit-for-bit with CPU reference.

### 7.2 ShapeTracker Tests

Verify that all movement ops produce correct `expr_idx` expressions:
- reshape preserves element order
- permute reorders strides correctly
- expand broadcasts with stride=0
- pad/shrink adjust bounds correctly
- flip negates stride

### 7.3 Tensor API Tests

Run tinygrad's own test suite (adapted) — tests for every Tensor method with known inputs/outputs.

### 7.4 Fusion Tests

Verify that elementwise chains fuse into single kernels:
- `a + b * c` → 1 kernel (not 2)
- `softmax(x)` → 2 kernels (not 7)
- `x.reshape().permute().add(y)` → 1 kernel (movements are free)

### 7.5 End-to-End Tests

- Falcon-OCR inference: same output as tinygrad reference
- DFlash speculative decoding: correct token generation
- TurboQuant: quantize → dequantize roundtrip within tolerance

### 7.6 Performance Tests

- Sieve of Eratosthenes benchmark (existing): no regression
- MatMul throughput vs tinygrad reference
- Softmax throughput vs tinygrad reference
- Attention throughput vs tinygrad reference

## 8. Performance Requirements

| Metric | Target |
|--------|--------|
| Primitive kernel launch overhead | < 5 μs (with device pool + kernel cache) |
| MatMul (1024×1024, f32, Metal) | Within 1.5x of tinygrad Metal |
| Softmax (batch=32, seq=2048, Metal) | Within 1.5x of tinygrad Metal |
| SDPA (batch=1, heads=8, seq=512, Metal) | Within 2x of tinygrad Metal |
| Binary size (WASM, GPU primitives only) | < 500 KB gzipped |
| Cold start (first kernel launch) | < 50 ms |
| Kernel cache hit | < 1 μs lookup |

## 9. Non-Goals

- Autograd / training support (inference only for now)
- LLVM backend (Cranelift is molt's CPU backend)
- Custom hand-written kernels (everything composes from 26 primitives)
- Backward compatibility with old `tensor_linear` / `GpuKernel` API
- MLX as a separate backend (Metal primitives cover Apple Silicon)

## 10. MLIR Integration

### 10.1 MLIR From the Beginning

Every `FusedKernel` is serialized to MLIR textual IR as the canonical intermediate representation. The 26 primitives map 1:1 to MLIR dialects:

| Primitive | MLIR Op | Dialect |
|-----------|---------|---------|
| `ADD`, `SUB`, `MUL` | `arith.addf/addi`, `arith.subf/subi`, `arith.mulf/muli` | `arith` |
| `IDIV` | `arith.divsi` (signed) / `arith.divui` (unsigned) | `arith` |
| `MOD` | `arith.remsi` (signed) / `arith.remui` (unsigned) | `arith` |
| `NEG` | `arith.negf` (float) / `arith.subi(0, x)` (integer) | `arith` |
| `MAX` | `arith.maximumf` (float, NaN-propagating) / `arith.maxsi` (signed int) / `arith.maxui` (unsigned int) | `arith` |
| `CMPLT`, `CMPEQ`, `CMPNE` | `arith.cmpf "olt"/"oeq"/"une"` | `arith` |
| `AND`, `OR`, `XOR`, `SHL` | `arith.andi`, `arith.ori`, `arith.xori`, `arith.shli` | `arith` |
| `SHR` | `arith.shrsi` (signed) / `arith.shrui` (unsigned) | `arith` |
| `WHERE` | `arith.select` | `arith` |
| `EXP2`, `LOG2`, `SIN`, `SQRT` | `math.exp2`, `math.log2`, `math.sin`, `math.sqrt` | `math` |
| `RECIPROCAL` | `arith.divf(1.0, x)` | `arith` |
| `TRUNC` | `math.trunc` | `math` |
| `CAST` | `arith.extf`, `arith.truncf`, `arith.sitofp`, `arith.fptosi`, `arith.uitofp`, `arith.fptoui`, `arith.extsi`, `arith.extui`, `arith.trunci` (selected by src/dst dtype pair) | `arith` |
| `BITCAST` | `arith.bitcast` | `arith` |
| `REDUCE_SUM` | `linalg.reduce { arith.addf }` (float) or `linalg.reduce { arith.addi }` (int) | `linalg` |
| `REDUCE_MAX` | `linalg.reduce { arith.maximumf }` (float, IEEE 754 NaN-propagating: if any element is NaN, result is NaN) or `linalg.reduce { arith.maxsi }` (signed int) / `linalg.reduce { arith.maxui }` (unsigned int) | `linalg` |
| ShapeTracker `expr_idx` | `affine.apply` with affine map | `affine` |
| Movement ops | `tensor.collapse_shape`, `tensor.expand_shape`, `tensor.pad`, `tensor.extract_slice` | `tensor` |

### 10.2 Dual-Path Rendering

Every fused kernel gets rendered through TWO paths:

1. **Direct renderer** (MSL/WGSL/CUDA/HIP) — generates shader source directly from `FusedKernel`. Fast, no external deps, works on WASM. This is the execution path.

2. **MLIR serializer** — generates MLIR textual IR from the same `FusedKernel`. This is used for:
   - Validation: verify the direct renderer's output matches MLIR semantics
   - Debugging: human-readable IR for inspection
   - Future: plug into MLIR toolchain for tiling/vectorization/lowering when the C++ dependency is acceptable for a given target

The MLIR serializer updates the existing `mlir_compat.rs` to handle the 26 GPU primitives alongside general TIR serialization.

### 10.3 MLIR Toolchain Integration (When C++ Deps Are Acceptable)

For native targets (not WASM), the MLIR toolchain can replace direct renderers:
```
FusedKernel → MLIR text → mlir-opt (tiling, fusion, vectorization)
            → gpu.launch_func → target lowering
            ├── nvvm → PTX (NVIDIA)
            ├── rocdl → GCN (AMD)
            └── spirv → SPIR-V (Vulkan/OpenCL)
```

For WASM/browser targets, direct renderers remain the only path (MLIR cannot compile to WASM). This is not a workaround — it is the correct architecture: WASM targets use WGSL which is a simple enough language that direct rendering is optimal.

### 10.4 No LLVM Dependency

MLIR textual IR serialization requires zero external dependencies — it is string generation. The MLIR *toolchain* (mlir-opt, passes) is only invoked when available and when C++ deps are acceptable. The direct renderers are always available as the primary execution path.

## 11. Risks

| Risk | Mitigation |
|------|-----------|
| WGSL lacks f64/i64 | Type narrowing in WgslRenderer (same as current) |
| Metal lacks f64 | Type narrowing in MslRenderer (same as current) |
| Reduce kernel perf on WebGPU | Workgroup-level reduction with shared memory |
| Kernel fusion complexity | Start with single-op kernels, add fusion incrementally |
| tinygrad API surface is large | Implement methods as needed by Falcon-OCR, DFlash, TurboQuant — not the full API |
