//! Kernel fusion: elementwise -> reduce -> elementwise chains.
//!
//! Merges chains of single-op kernels into fused multi-op kernels.
//! Fusion rule (same as tinygrad):
//!   [Buffer leaves + MovementOps] -> ElementwiseOps -> ReduceOps -> ElementwiseOps
//!
//! This entire chain becomes ONE kernel.

use crate::ops::PrimitiveOp;
use crate::render::{BufferBinding, FusedKernel, FusedOp, FusedSrc};

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
            if !merged_bufs.iter().any(|b: &BufferBinding| b.buf_id == buf.buf_id) {
                merged_bufs.push(buf.clone());
            }
        }
    }

    // Build ops chain: remap FusedSrc references
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
