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
    Unary { op: PrimitiveOp, src: Arc<LazyOp> },
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
                    // For Cast/Bitcast, the target dtype must be stored elsewhere
                    // (in the FusedOp layer). At the LazyOp level, we propagate
                    // the source dtype as a placeholder. The scheduler resolves this.
                    src.dtype()
                } else {
                    src.dtype()
                }
            }
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
