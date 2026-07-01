//! LazyOp DAG — deferred computation graph.
//!
//! Every Tensor method returns a new Tensor backed by a LazyOp node.
//! No GPU kernel is executed until realize() is called. This enables
//! the fusion engine to see the full computation graph.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::ReductionDomain;
use crate::shapetracker::ShapeTracker;

/// Process-global monotonic allocator for buffer identities.
///
/// Every concrete buffer (a realized leaf tensor) AND every scheduler-materialized
/// intermediate draws its `buf_id` from this single counter, so all buffer
/// identities in the system are globally unique by construction. This is the
/// invariant that makes the schedule/execute bridge correct:
///
/// - A leaf's [`DeviceBufferRef::id`] is the key the runtime uses to look its
///   realized bytes up in the tensor store. It must be stable and unique per
///   distinct leaf so two different leaves never collide in that store, and so
///   the *same* leaf referenced by multiple ops resolves to the *same* data.
/// - The scheduler assigns each `BufferBinding::buf_id` from the identity of the
///   DAG node that produces that buffer (a leaf's own id, or a fresh id for an
///   intermediate). Because leaf ids and intermediate ids are drawn from this
///   one counter, the two id spaces can never overlap. Codegen uses per-kernel
///   binding slots as parameter names, so one storage id can be bound through
///   multiple ShapeTracker views without naming collisions.
///
/// Starts at 1 so `0` is never a live buffer id — making a stale/placeholder `0`
/// (the historical "realize computes on zeros" bug) impossible to mistake for a
/// real buffer.
static NEXT_BUFFER_ID: AtomicUsize = AtomicUsize::new(1);

/// Allocate a fresh, process-globally-unique buffer identity.
///
/// Used both by runtime tensor constructors (for realized leaves) and by the
/// scheduler (for materialized intermediates), guaranteeing the two never
/// collide. `Relaxed` ordering is sufficient: we only require uniqueness of the
/// returned values, not ordering relative to other memory operations.
pub fn alloc_buffer_id() -> usize {
    NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed)
}

/// Opaque handle to a device buffer. The actual buffer is managed
/// by the device's Allocator and not exposed through the DAG.
#[derive(Debug, Clone)]
pub struct DeviceBufferRef {
    /// Globally-unique identifier for this buffer, drawn from
    /// [`alloc_buffer_id`]. Never `0` for a live buffer.
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
    Unary { op: PrimitiveOp, src: Arc<LazyOp> },
    /// Elementwise typed cast or bitcast.
    Cast {
        op: PrimitiveOp,
        src: Arc<LazyOp>,
        dst_dtype: DType,
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
    Movement { src: Arc<LazyOp>, st: ShapeTracker },
    /// Scheduling annotation: force materialization.
    /// NOT a compute op — the scheduler inserts a copy kernel.
    Contiguous { src: Arc<LazyOp> },
}

impl LazyOp {
    /// Get the output dtype of this op.
    pub fn dtype(&self) -> DType {
        match self {
            Self::Buffer { dtype, .. } => *dtype,
            Self::Unary { op, src } => {
                if matches!(op, PrimitiveOp::Cast | PrimitiveOp::Bitcast) {
                    panic!("Cast/Bitcast LazyOps must use LazyOp::Cast with explicit dst_dtype")
                } else {
                    src.dtype()
                }
            }
            Self::Cast { dst_dtype, .. } => *dst_dtype,
            Self::Binary { op, lhs, .. } => {
                if matches!(
                    op,
                    PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
                ) {
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
            Self::Cast { src, .. } => src.shape(),
            Self::Binary { lhs, .. } => lhs.shape(),
            Self::Ternary { a, .. } => a.shape(),
            Self::Reduce { src, axis, .. } => {
                ReductionDomain::from_axis(&src.shape(), *axis).output_shape
            }
            Self::Movement { st, .. } => st.shape().to_vec(),
            Self::Contiguous { src } => src.shape(),
        }
    }
}
