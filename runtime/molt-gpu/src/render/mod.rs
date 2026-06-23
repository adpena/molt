//! Renderer trait + FusedKernel IR.
//!
//! The FusedKernel is the post-fusion IR passed to renderers. It contains
//! an ordered chain of ops (elementwise prefix -> optional reduce ->
//! elementwise suffix) plus buffer bindings and work distribution.

pub mod cuda;
pub mod glsl;
pub mod hip;
pub(crate) mod indexing;
pub mod mil;
pub mod msl;
#[cfg(feature = "metal4")]
pub mod msl4;
pub mod opencl;
pub mod wgsl;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::shapetracker::ShapeTracker;

mod fused_op;
pub use fused_op::{FusedOp, FusedOpDomain, ReductionDomain};

/// Executable body carried by a scheduled kernel.
///
/// Compute kernels evaluate the ordered [`FusedOp`] chain. Materialization
/// kernels copy one input binding through its ShapeTracker view into fresh
/// contiguous output storage. Keeping this distinction in the IR prevents
/// `Contiguous` from becoming either a fake primitive op or a silent passthrough.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum KernelBody {
    #[default]
    Compute,
    MaterializeCopy,
}

/// Metadata from the shape specialization pass.
///
/// When all dimensions of a kernel's output shape are statically known (no
/// dynamic dims), the scheduler can compute optimal work distribution and
/// determine whether bounds checks are eliminable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
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
    /// The executable body shape. `Compute` consumes [`FusedKernel::ops`];
    /// `MaterializeCopy` requires `ops.is_empty()` and exactly one input
    /// binding at `bufs[1]`.
    pub body: KernelBody,
    /// Ordered chain of ops: elementwise prefix -> optional reduce -> elementwise suffix.
    pub ops: Vec<FusedOp>,
    /// Buffer bindings. Convention: bufs[0] is ALWAYS the output (Write access).
    /// bufs[1..] are inputs (Read access). ReadWrite is used for in-place ops.
    ///
    /// Binding identity is the slot index in this vector. Runtime storage
    /// identity is [`BufferBinding::buf_id`]. Renderers must name shader
    /// parameters by slot (`buf0`, `buf1`, ...) so one physical storage id can
    /// appear multiple times with distinct ShapeTracker views.
    pub bufs: Vec<BufferBinding>,
    /// Work distribution. Computed by the scheduler, NOT the renderer.
    pub grid: [u32; 3],
    pub local: [u32; 3],
    /// Shape specialization metadata. `None` before the specialization pass
    /// runs; `Some` after.
    pub spec: Option<ShapeSpecialization>,
    /// SIMD vectorization width for elementwise kernels.
    /// When set to 4, renderers emit vectorized memory access patterns
    /// (float4/vec4<f32>/vload4) instead of scalar per-element access.
    /// Default is 1 (scalar). Set to 4 when `total_elements % 4 == 0`
    /// and all buffers are contiguous and 16-byte aligned.
    pub vectorize_width: u32,
}

impl FusedKernel {
    pub(crate) fn materialize_copy_contract(&self) -> (&BufferBinding, &BufferBinding, usize) {
        assert_eq!(self.body, KernelBody::MaterializeCopy);
        assert!(self.ops.is_empty());
        assert_eq!(self.bufs.len(), 2);
        assert_eq!(self.bufs[0].access, BufferAccess::Write);
        assert_eq!(self.bufs[1].access, BufferAccess::Read);
        assert_eq!(self.bufs[0].dtype, self.bufs[1].dtype);
        assert_eq!(self.bufs[0].st.numel(), self.bufs[1].st.numel());
        assert!(
            self.bufs[0].st.views.len() == 1 && self.bufs[0].st.view().is_contiguous(),
            "MaterializeCopy output must be a single contiguous view"
        );
        (&self.bufs[0], &self.bufs[1], self.bufs[0].st.numel())
    }

    pub(crate) fn compute_body_contract(&self) {
        assert_eq!(self.body, KernelBody::Compute);
        assert!(
            !self.ops.is_empty(),
            "Compute kernels must carry at least one op"
        );
        assert!(
            !self.bufs.is_empty(),
            "Compute kernels must carry an output binding"
        );
        let output_numel = self.bufs[0].st.numel();
        let reduce_count = self
            .ops
            .iter()
            .filter(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
            .count();
        assert!(
            reduce_count <= 1,
            "Compute kernels may carry at most one reduce op"
        );
        for op in &self.ops {
            match (
                op.domain(),
                matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax),
            ) {
                (FusedOpDomain::Reduction(domain), true) => {
                    assert_eq!(
                        domain.output_numel(),
                        output_numel,
                        "Reduction domain output shape must match kernel output"
                    );
                }
                (FusedOpDomain::Elementwise, false) => {}
                (FusedOpDomain::Reduction(_), false) => {
                    panic!(
                        "Elementwise op {:?} must not carry a reduction domain",
                        op.op()
                    )
                }
                (FusedOpDomain::Elementwise, true) => {
                    panic!("Reduce op {:?} must carry a reduction domain", op.op())
                }
            }
        }
    }

    pub(crate) fn assert_no_mxfp_dtypes(&self, context: &str) {
        for (binding_idx, binding) in self.bufs.iter().enumerate() {
            assert!(
                !binding.dtype.is_mxfp(),
                "molt-gpu {context}: MXFP requires explicit block/exponent storage lowering before binding {binding_idx} can use {:?}",
                binding.dtype
            );
        }

        for (op_idx, op) in self.ops.iter().enumerate() {
            assert!(
                !op.dst_dtype().is_mxfp(),
                "molt-gpu {context}: MXFP requires explicit block/exponent storage lowering before op {op_idx} can produce {:?}",
                op.dst_dtype()
            );
            for src in op.srcs() {
                if let FusedSrc::Const { dtype, .. } = src {
                    assert!(
                        !dtype.is_mxfp(),
                        "molt-gpu {context}: MXFP requires explicit block/exponent storage lowering before op {op_idx} can consume {:?} constants",
                        dtype
                    );
                }
            }
        }
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Runtime storage handle identity.
    ///
    /// This is the id the executor uses to resolve bytes/handles. It is not a
    /// renderer-local parameter name; the binding slot in `FusedKernel.bufs` is
    /// the renderer-local identity.
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
