//! DAG -> topological kernel schedule.
//!
//! Walks the LazyOp DAG, identifies fusion boundaries, and produces
//! an ordered list of FusedKernels ready for rendering and execution.

use std::sync::Arc;

use crate::lazy::LazyOp;
use crate::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use crate::shapetracker::ShapeTracker;

/// Schedule a LazyOp DAG into a list of FusedKernels.
///
/// Phase 1: single-op kernels (no fusion). The fusion engine (fuse.rs)
/// will merge these in a subsequent pass.
pub fn schedule(root: &Arc<LazyOp>, _output_shape: &[usize]) -> Vec<FusedKernel> {
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
                local: [n.clamp(1, 256) as u32, 1, 1],
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
                local: [n.clamp(1, 256) as u32, 1, 1],
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
                local: [n.clamp(1, 256) as u32, 1, 1],
            });
        }
        LazyOp::Reduce { op, src, axis: _ } => {
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
        LazyOp::Movement { src, st: _ } => {
            // Movement ops are free — just modify the ShapeTracker.
            schedule_recursive(src, kernels, next_buf_id);
        }
        LazyOp::Contiguous { src } => {
            // Force materialization — insert a copy kernel.
            schedule_recursive(src, kernels, next_buf_id);
        }
    }
}
