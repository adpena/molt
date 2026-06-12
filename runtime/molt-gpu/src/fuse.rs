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
use crate::render::{BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody};

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
        if kernel.body == KernelBody::MaterializeCopy {
            if !current_chain.is_empty() {
                fused.push(merge_chain(current_chain));
                current_chain = Vec::new();
                has_reduce_in_chain = false;
            }
            fused.push(kernel);
            continue;
        }

        let is_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));
        if has_reduce_in_chain
            && !current_chain.is_empty()
            && !post_reduce_shapes_compatible(
                current_chain.last().expect("non-empty chain"),
                &kernel,
            )
        {
            fused.push(merge_chain(current_chain));
            current_chain = Vec::new();
            has_reduce_in_chain = false;
        }

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

fn post_reduce_shapes_compatible(producer: &FusedKernel, consumer: &FusedKernel) -> bool {
    producer.bufs[0].st.shape() == consumer.bufs[0].st.shape()
}

/// Merge a chain of kernels into a single fused kernel.
///
/// Inter-kernel data flow is expressed by **buffer identity**: a consuming
/// kernel reads a producing kernel's result via an input binding whose `buf_id`
/// equals the producer's output `buf_id`. When such a producer is fused into the
/// same kernel, its result lives in an SSA value (`FusedSrc::Op`), not a device
/// buffer — so every reference to a produced-in-chain id is rewritten to the
/// producing op, and that id is dropped from the merged buffer list (it is no
/// longer a real input). Only ids that are NOT produced within the chain remain
/// as external input bindings.
///
/// This is the same storage-identity-plus-view contract the scheduler
/// establishes, so fusion composes with it without a second, divergent notion
/// of "which buffer is which".
fn merge_chain(chain: Vec<FusedKernel>) -> FusedKernel {
    if chain.len() == 1 {
        return chain.into_iter().next().unwrap();
    }
    assert!(
        chain
            .iter()
            .all(|kernel| kernel.body == KernelBody::Compute),
        "MaterializeCopy kernels are hard fusion barriers"
    );

    // Output ids produced by kernels in this chain. A binding with one of these
    // ids is an intermediate computed in-chain, not an external input.
    let produced_ids: std::collections::HashSet<usize> =
        chain.iter().map(|k| k.bufs[0].buf_id).collect();

    let last = chain.last().unwrap();

    // Merged inputs: the last kernel's output at slot 0, then every DISTINCT
    // input binding whose id is NOT produced within the chain (i.e. genuinely
    // external leaves / upstream intermediates), deduplicated by storage id,
    // ShapeTracker view, dtype, and access mode. Same storage through different
    // views must remain two binding slots.
    let mut merged_bufs: Vec<BufferBinding> = Vec::new();
    merged_bufs.push(last.bufs[0].clone());
    for kernel in &chain {
        for buf in &kernel.bufs[1..] {
            if produced_ids.contains(&buf.buf_id) {
                continue; // produced in-chain → becomes an Op, not an input.
            }
            if !merged_bufs.iter().any(|b| same_external_binding(b, buf)) {
                merged_bufs.push(buf.clone());
            }
        }
    }

    // Map each in-chain-produced buffer id to the merged-op index that writes it
    // (a kernel's last op). Filled as kernels are appended so later kernels can
    // reference earlier outputs as `Op(..)`.
    let mut produced_at: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    // Build ops chain: remap FusedSrc references by buffer identity.
    let mut merged_ops: Vec<FusedOp> = Vec::new();
    for kernel in &chain {
        let op_offset = merged_ops.len();
        for op in &kernel.ops {
            let mut remapped_srcs = Vec::with_capacity(op.srcs().len());
            for src in op.srcs() {
                match src {
                    FusedSrc::Buf(idx) => {
                        let binding = &kernel.bufs[*idx];
                        let buf_id = binding.buf_id;
                        if let Some(&producer_op) = produced_at.get(&buf_id) {
                            // Produced by an earlier kernel in this chain — read
                            // its SSA result instead of a device buffer.
                            remapped_srcs.push(FusedSrc::Op(producer_op));
                        } else {
                            let new_idx = merged_bufs
                                .iter()
                                .position(|b| same_external_binding(b, binding))
                                .expect("external input must be in the merged buffer set");
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
            merged_ops.push(op.clone_with_srcs(remapped_srcs));
        }
        // This kernel's output is produced by its final merged op.
        produced_at.insert(kernel.bufs[0].buf_id, merged_ops.len() - 1);
    }

    FusedKernel {
        body: KernelBody::Compute,
        ops: merged_ops,
        bufs: merged_bufs,
        grid: last.grid,
        local: last.local,
        spec: None,
        vectorize_width: 1,
    }
}

fn same_external_binding(lhs: &BufferBinding, rhs: &BufferBinding) -> bool {
    lhs.buf_id == rhs.buf_id
        && lhs.st == rhs.st
        && lhs.dtype == rhs.dtype
        && lhs.access == rhs.access
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
    if kernel.body != KernelBody::Compute {
        return 0;
    }

    // Phase 1: Evaluate all ops that have only Const sources.
    // Store the computed constant value for each foldable op.
    let n_ops = kernel.ops.len();
    let mut folded_values: Vec<Option<(f64, DType)>> = vec![None; n_ops];

    for i in 0..n_ops {
        let op = &kernel.ops[i];

        // Resolve each source to a constant value if possible.
        let const_srcs: Vec<Option<(f64, DType)>> = op
            .srcs()
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
            let dst_dtype = op.dst_dtype();

            if let Some(result) = evaluate_const_op(op.op(), &vals) {
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
            .srcs()
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

        new_ops.push(kernel.ops[i].clone_with_srcs(remapped_srcs));
    }

    let folded_count = n_ops - new_ops.len();
    kernel.ops = new_ops;
    folded_count
}

/// Identity operation folding pass.
///
/// Eliminates no-op operations where a constant operand makes the op
/// a pass-through of the other operand:
///   - ADD(x, 0.0)  or ADD(0.0, x) -> x
///   - SUB(x, 0.0)                 -> x
///   - MUL(x, 1.0)  or MUL(1.0, x) -> x
///   - MUL(x, 0.0)  or MUL(0.0, x) -> Const(0.0)
///
/// Replaces the op with a direct reference to the non-identity source,
/// remapping all downstream Op references.
///
/// Returns the number of ops eliminated across all kernels.
pub fn identity_fold(kernels: &mut [FusedKernel]) -> usize {
    let mut total_folded = 0;
    for kernel in kernels.iter_mut() {
        total_folded += identity_fold_kernel(kernel);
    }
    total_folded
}

/// Identity-fold a single kernel's ops.
fn identity_fold_kernel(kernel: &mut FusedKernel) -> usize {
    if kernel.body != KernelBody::Compute {
        return 0;
    }

    let n_ops = kernel.ops.len();
    if n_ops == 0 {
        return 0;
    }

    // Phase 1: Identify ops that are identity operations.
    // For each foldable op, store what it should be replaced by.
    #[derive(Clone)]
    enum Replacement {
        /// Keep the op as-is.
        Keep,
        /// Replace with this source (pass-through the non-identity operand).
        PassThrough(FusedSrc),
        /// Replace with a constant value (e.g., MUL(x, 0) -> 0).
        Const(f64, DType),
    }

    let mut replacements: Vec<Replacement> = vec![Replacement::Keep; n_ops];

    for (op, replacement) in kernel.ops.iter().zip(replacements.iter_mut()) {
        match op.op() {
            PrimitiveOp::Add => {
                // ADD(x, 0) -> x, ADD(0, x) -> x
                if is_const_val(&op.srcs()[1], 0.0) {
                    *replacement = Replacement::PassThrough(op.srcs()[0].clone());
                } else if is_const_val(&op.srcs()[0], 0.0) {
                    *replacement = Replacement::PassThrough(op.srcs()[1].clone());
                }
            }
            PrimitiveOp::Sub => {
                // SUB(x, 0) -> x
                if is_const_val(&op.srcs()[1], 0.0) {
                    *replacement = Replacement::PassThrough(op.srcs()[0].clone());
                }
            }
            PrimitiveOp::Mul => {
                // MUL(x, 1) -> x, MUL(1, x) -> x
                if is_const_val(&op.srcs()[1], 1.0) {
                    *replacement = Replacement::PassThrough(op.srcs()[0].clone());
                } else if is_const_val(&op.srcs()[0], 1.0) {
                    *replacement = Replacement::PassThrough(op.srcs()[1].clone());
                }
                // MUL(x, 0) -> 0, MUL(0, x) -> 0
                else if is_const_val(&op.srcs()[1], 0.0) || is_const_val(&op.srcs()[0], 0.0) {
                    *replacement = Replacement::Const(0.0, op.dst_dtype());
                }
            }
            _ => {}
        }
    }

    // Phase 2: Rebuild ops, skipping replaced ones and remapping references.
    let mut old_to_new: Vec<Option<usize>> = vec![None; n_ops];
    // For replaced ops, store what downstream refs should use.
    let mut replace_with: Vec<Option<FusedSrc>> = vec![None; n_ops];

    let mut new_ops: Vec<FusedOp> = Vec::new();

    for i in 0..n_ops {
        match &replacements[i] {
            Replacement::Keep => {
                old_to_new[i] = Some(new_ops.len());
                // Remap sources
                let remapped_srcs: Vec<FusedSrc> = kernel.ops[i]
                    .srcs()
                    .iter()
                    .map(|src| remap_src(src, &old_to_new, &replace_with))
                    .collect();
                new_ops.push(kernel.ops[i].clone_with_srcs(remapped_srcs));
            }
            Replacement::PassThrough(src) => {
                // This op is eliminated. Store the remapped source for downstream.
                let remapped = remap_src(src, &old_to_new, &replace_with);
                replace_with[i] = Some(remapped);
            }
            Replacement::Const(val, dtype) => {
                replace_with[i] = Some(FusedSrc::Const {
                    val: *val,
                    dtype: *dtype,
                });
            }
        }
    }

    let folded = n_ops - new_ops.len();
    kernel.ops = new_ops;
    folded
}

/// Check if a FusedSrc is a Const with the given value.
fn is_const_val(src: &FusedSrc, val: f64) -> bool {
    matches!(src, FusedSrc::Const { val: v, .. } if *v == val)
}

/// Remap a FusedSrc through the old-to-new index mapping and replacements.
fn remap_src(
    src: &FusedSrc,
    old_to_new: &[Option<usize>],
    replace_with: &[Option<FusedSrc>],
) -> FusedSrc {
    match src {
        FusedSrc::Op(prior_idx) => {
            if let Some(replacement) = &replace_with[*prior_idx] {
                // The referenced op was folded, use its replacement.
                // Need to recursively remap if the replacement is also an Op.
                match replacement {
                    FusedSrc::Op(p) => {
                        if let Some(new_idx) = old_to_new[*p] {
                            FusedSrc::Op(new_idx)
                        } else {
                            replacement.clone()
                        }
                    }
                    other => other.clone(),
                }
            } else {
                FusedSrc::Op(old_to_new[*prior_idx].expect("non-folded op must have new index"))
            }
        }
        other => other.clone(),
    }
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
