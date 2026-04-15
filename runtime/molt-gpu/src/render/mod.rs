//! Renderer trait + FusedKernel IR.
//!
//! The FusedKernel is the post-fusion IR passed to renderers. It contains
//! an ordered chain of ops (elementwise prefix -> optional reduce ->
//! elementwise suffix) plus buffer bindings and work distribution.

pub mod msl;
#[cfg(feature = "metal4")]
pub mod msl4;
pub mod wgsl;
pub mod cuda;
pub mod hip;
pub mod glsl;
pub mod opencl;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::shapetracker::ShapeTracker;

/// Metadata from the shape specialization pass.
///
/// When all dimensions of a kernel's output shape are statically known (no
/// dynamic dims), the scheduler can compute optimal work distribution and
/// determine whether bounds checks are eliminable.
#[derive(Debug, Clone, Default)]
pub struct ShapeSpecialization {
    /// `true` when total element count is exactly divisible by the local
    /// workgroup size. When set, renderers may omit the `if (gid < N)` guard.
    pub bounds_check_elim: bool,
    /// Total number of elements processed by this kernel, precomputed from
    /// the static shape.
    pub total_elements: u64,
    /// Optimal local workgroup size selected by the specialization pass.
    /// May differ from the `local` field when the specializer picks a
    /// workgroup size that divides `total_elements` evenly.
    pub optimal_local: [u32; 3],
    /// `true` when all dimensions are statically known (no dynamic dims).
    pub all_static: bool,
}

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
    /// Shape specialization metadata. `None` before the specialization pass
    /// runs; `Some` after.
    pub spec: Option<ShapeSpecialization>,
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
