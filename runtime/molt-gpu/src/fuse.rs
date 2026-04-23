//! Kernel fusion: elementwise -> reduce -> elementwise chains.
//!
//! Merges chains of single-op kernels into fused multi-op kernels.
//! Fusion rule (same as tinygrad):
//!   [Buffer leaves + MovementOps] -> ElementwiseOps -> ReduceOps -> ElementwiseOps
//!
//! This entire chain becomes ONE kernel.
//!
//! Also includes a constant folding pass that evaluates subtrees
//! with only Const inputs at compile time.

use crate::dtype::DType;
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
        let is_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

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
            if !merged_bufs
                .iter()
                .any(|b: &BufferBinding| b.buf_id == buf.buf_id)
            {
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
                            let new_idx = merged_bufs
                                .iter()
                                .position(|b| b.buf_id == buf_id)
                                .expect("buffer not found in merged set");
                            remapped_srcs.push(FusedSrc::Buf(new_idx));
                        }
                    }
                    FusedSrc::Op(prior) => {
                        remapped_srcs.push(FusedSrc::Op(op_offset + prior));
                    }
                    FusedSrc::Const { val, dtype } => {
                        remapped_srcs.push(FusedSrc::Const {
                            val: *val,
                            dtype: *dtype,
                        });
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
        spec: None,
        vectorize_width: 1,
    }
}

/// Constant folding pass for fused kernels.
///
/// Walks each kernel's ops list and, for any op whose sources are ALL
/// `FusedSrc::Const` values, evaluates the op at compile time and replaces
/// it with a single `FusedSrc::Const` in all downstream references.
///
/// This eliminates runtime computation for static sub-expressions like
/// `MUL(Const(2.0), Const(3.0))` → the op is removed and downstream
/// ops that referenced it now reference `Const(6.0)` directly.
///
/// Returns the number of ops folded across all kernels.
pub fn constant_fold(kernels: &mut [FusedKernel]) -> usize {
    let mut total_folded = 0;
    for kernel in kernels.iter_mut() {
        total_folded += constant_fold_kernel(kernel);
    }
    total_folded
}

/// Constant-fold a single kernel's ops.
///
/// Returns the number of ops that were folded (removed).
fn constant_fold_kernel(kernel: &mut FusedKernel) -> usize {
    // Phase 1: Evaluate all ops that have only Const sources.
    // Store the computed constant value for each foldable op.
    let n_ops = kernel.ops.len();
    let mut folded_values: Vec<Option<(f64, DType)>> = vec![None; n_ops];

    for i in 0..n_ops {
        let op = &kernel.ops[i];

        // Resolve each source to a constant value if possible.
        let const_srcs: Vec<Option<(f64, DType)>> = op
            .srcs
            .iter()
            .map(|src| match src {
                FusedSrc::Const { val, dtype } => Some((*val, *dtype)),
                FusedSrc::Op(prior_idx) => folded_values[*prior_idx],
                FusedSrc::Buf(_) => None,
            })
            .collect();

        // If all sources are constants, evaluate the op.
        if const_srcs.iter().all(|s| s.is_some()) {
            let vals: Vec<f64> = const_srcs.iter().map(|s| s.unwrap().0).collect();
            let dst_dtype = op.dst_dtype;

            if let Some(result) = evaluate_const_op(op.op, &vals) {
                folded_values[i] = Some((result, dst_dtype));
            }
        }
    }

    // Phase 2: Rebuild the ops list, replacing folded ops with nothing
    // and remapping references.
    //
    // Build a mapping: old op index -> new op index (or None if folded).
    let mut old_to_new: Vec<Option<usize>> = vec![None; n_ops];
    let mut new_ops: Vec<FusedOp> = Vec::new();

    for i in 0..n_ops {
        if folded_values[i].is_some() {
            // This op was folded — skip it.
            continue;
        }
        old_to_new[i] = Some(new_ops.len());

        // Remap sources: replace references to folded ops with Const values.
        let remapped_srcs: Vec<FusedSrc> = kernel.ops[i]
            .srcs
            .iter()
            .map(|src| match src {
                FusedSrc::Op(prior_idx) => {
                    if let Some((val, dtype)) = folded_values[*prior_idx] {
                        FusedSrc::Const { val, dtype }
                    } else {
                        FusedSrc::Op(
                            old_to_new[*prior_idx].expect("non-folded op must have a new index"),
                        )
                    }
                }
                other => other.clone(),
            })
            .collect();

        new_ops.push(FusedOp {
            op: kernel.ops[i].op,
            srcs: remapped_srcs,
            dst_dtype: kernel.ops[i].dst_dtype,
        });
    }

    let folded_count = n_ops - new_ops.len();
    kernel.ops = new_ops;
    folded_count
}

/// Evaluate a primitive op on constant f64 inputs.
///
/// Returns `Some(result)` if the op can be evaluated at compile time,
/// or `None` if it cannot (e.g., reduce ops need runtime context).
fn evaluate_const_op(op: PrimitiveOp, vals: &[f64]) -> Option<f64> {
    match op {
        // Unary ops (1 input)
        PrimitiveOp::Neg => Some(-vals[0]),
        PrimitiveOp::Exp2 => Some(vals[0].exp2()),
        PrimitiveOp::Log2 => Some(vals[0].log2()),
        PrimitiveOp::Sin => Some(vals[0].sin()),
        PrimitiveOp::Sqrt => Some(vals[0].sqrt()),
        PrimitiveOp::Reciprocal => Some(1.0 / vals[0]),
        PrimitiveOp::Trunc => Some(vals[0].trunc()),

        // Binary ops (2 inputs)
        PrimitiveOp::Add => Some(vals[0] + vals[1]),
        PrimitiveOp::Sub => Some(vals[0] - vals[1]),
        PrimitiveOp::Mul => Some(vals[0] * vals[1]),
        PrimitiveOp::Idiv => {
            if vals[1] == 0.0 {
                None // Division by zero — cannot fold.
            } else {
                Some((vals[0] / vals[1]).trunc())
            }
        }
        PrimitiveOp::Mod => {
            if vals[1] == 0.0 {
                None
            } else {
                Some(vals[0] % vals[1])
            }
        }
        PrimitiveOp::Max => Some(vals[0].max(vals[1])),
        PrimitiveOp::Cmplt => Some(if vals[0] < vals[1] { 1.0 } else { 0.0 }),
        PrimitiveOp::Cmpeq => Some(if vals[0] == vals[1] { 1.0 } else { 0.0 }),
        PrimitiveOp::Cmpne => Some(if vals[0] != vals[1] { 1.0 } else { 0.0 }),
        PrimitiveOp::And => Some(f64::from_bits(vals[0].to_bits() & vals[1].to_bits())),
        PrimitiveOp::Or => Some(f64::from_bits(vals[0].to_bits() | vals[1].to_bits())),
        PrimitiveOp::Xor => Some(f64::from_bits(vals[0].to_bits() ^ vals[1].to_bits())),
        PrimitiveOp::Shl => {
            let a = vals[0] as i64;
            let b = vals[1] as u32;
            Some((a << b) as f64)
        }
        PrimitiveOp::Shr => {
            let a = vals[0] as i64;
            let b = vals[1] as u32;
            Some((a >> b) as f64)
        }

        // Ternary ops
        PrimitiveOp::Where => Some(if vals[0] != 0.0 { vals[1] } else { vals[2] }),

        // Cast/Bitcast cannot be folded without target type context
        // at this level (the FusedOp knows dst_dtype but the value
        // is already f64, so a simple pass-through is correct for
        // constant folding purposes).
        PrimitiveOp::Cast => Some(vals[0]),
        PrimitiveOp::Bitcast => None, // Bit reinterpretation needs type info

        // Reduce ops cannot be folded at the FusedOp level — they
        // operate over buffer ranges, not scalar constants.
        PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => None,
    }
}
