//! DAG -> topological kernel schedule.
//!
//! Walks the LazyOp DAG, identifies fusion boundaries, and produces
//! an ordered list of FusedKernels ready for rendering and execution.
//!
//! Includes a shape specialization pass that, for kernels with fully
//! static shapes, computes optimal grid/local sizes and determines
//! whether bounds checks can be eliminated.

use std::collections::HashMap;
use std::sync::Arc;

use crate::dtype::DType;
use crate::lazy::LazyOp;
use crate::ops::PrimitiveOp;
use crate::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, ShapeSpecialization,
};
use crate::shapetracker::ShapeTracker;

/// Common workgroup sizes to try, in descending order of preference.
/// We pick the largest that evenly divides the total element count,
/// falling back to 1 (which always divides).
///
/// Default preference: 256 first (optimal for Vulkan/NVIDIA per Maczan 2026).
/// Use `preferred_local_sizes_for_backend()` for backend-adaptive selection.
const PREFERRED_LOCAL_SIZES: [u32; 9] = [256, 128, 64, 32, 16, 8, 4, 2, 1];

/// Backend-adaptive workgroup size preferences.
///
/// Per Maczan 2026 (arXiv 2604.02344), backend choice is the dominant factor
/// for dispatch overhead, and within Metal alone there is 2.2x variance
/// between implementations. Workgroup size selection should account for this.
///
/// Returns the preferred local size array for the given backend.
#[cfg(feature = "webgpu-backend")]
pub fn preferred_local_sizes_for_backend(
    backend: crate::device::webgpu::WebGpuBackendKind,
) -> &'static [u32] {
    use crate::device::webgpu::WebGpuBackendKind;
    match backend {
        // Vulkan (NVIDIA, AMD): 256 is optimal, matches CUDA warp scheduling.
        WebGpuBackendKind::Vulkan => &PREFERRED_LOCAL_SIZES,
        // Metal (Apple): Prefer 128 over 256 to reduce register pressure on
        // Apple GPU's TBDR architecture. Maczan shows Metal-specific regressions
        // with aggressive fusion; conservative workgroup sizes help.
        WebGpuBackendKind::Metal => &[128, 64, 256, 32, 16, 8, 4, 2, 1],
        // D3D12 (Windows/NVIDIA): 256 works well, but 128 is safer fallback
        // for Intel integrated GPUs with limited EU count.
        WebGpuBackendKind::Dx12 => &[256, 128, 64, 32, 16, 8, 4, 2, 1],
        // GL/Unknown: conservative default.
        WebGpuBackendKind::Gl | WebGpuBackendKind::Unknown => &[64, 128, 32, 256, 16, 8, 4, 2, 1],
    }
}

/// Schedule a LazyOp DAG into a list of FusedKernels.
///
/// Phase 1: single-op kernels (no fusion). The fusion engine (fuse.rs)
/// will merge these in a subsequent pass.
///
/// ## Buffer identity contract
///
/// Each [`BufferBinding::buf_id`] is the *identity of the DAG node that
/// produces that buffer's data*, NOT a fresh per-binding counter:
///
/// - A **leaf** ([`LazyOp::Buffer`]) contributes its own [`DeviceBufferRef::id`]
///   as the binding id. That id is the key the runtime uses to fetch the leaf's
///   realized bytes, so the schedule→execute bridge (which looks data up by
///   binding id) resolves to the correct input buffer. Two ops reading the same
///   leaf therefore share a binding id, and a leaf read twice within one kernel
///   (e.g. `x - x`) collapses to a single binding.
/// - An **intermediate** (the result of a non-leaf op that another kernel
///   consumes) gets a fresh globally-unique id from [`alloc_buffer_id`],
///   memoized per node so the producing kernel's output binding and every
///   consuming kernel's input binding agree. This is what `fuse::merge_chain`'s
///   buffer-dedup and the executor's kernel-chaining rely on.
///
/// Because leaf ids and intermediate ids are both drawn from the single global
/// [`alloc_buffer_id`] counter, the two id spaces never overlap.
pub fn schedule(root: &Arc<LazyOp>, _output_shape: &[usize]) -> Vec<FusedKernel> {
    let mut kernels = Vec::new();
    let mut ctx = ScheduleCtx::default();

    schedule_recursive(root, &mut kernels, &mut ctx);
    kernels
}

/// Scheduler state: maps each DAG node to its stable buffer identity.
///
/// Keyed by the node's `Arc<LazyOp>` pointer address (stable for the lifetime of
/// the DAG, which the root keeps alive across scheduling). The same node always
/// resolves to the same id; distinct nodes resolve to distinct ids.
#[derive(Default)]
struct ScheduleCtx {
    /// `Arc<LazyOp>` pointer (as usize) -> assigned buffer id.
    node_ids: HashMap<usize, usize>,
}

impl ScheduleCtx {
    /// The stable buffer id of the buffer produced by `node`.
    ///
    /// Leaves contribute their concrete [`DeviceBufferRef::id`]; non-leaves are
    /// assigned a fresh globally-unique id on first encounter and memoized so all
    /// later references to the same node observe the same id.
    fn buf_id_for(&mut self, node: &Arc<LazyOp>) -> usize {
        if let LazyOp::Buffer { buf, .. } = node.as_ref() {
            return buf.id;
        }
        let key = Arc::as_ptr(node) as usize;
        *self.node_ids.entry(key).or_insert_with(crate::lazy::alloc_buffer_id)
    }
}

/// Build the input bindings for a kernel from its ordered operand nodes,
/// deduplicating operands that refer to the same DAG node.
///
/// Returns `(input_bindings, operand_slots)` where `input_bindings` are the
/// distinct input [`BufferBinding`]s (to be appended after the output at
/// `bufs[0]`) and `operand_slots[i]` is the `bufs` index that operand `i`
/// resolves to — i.e. the value to wrap in `FusedSrc::Buf`.
///
/// Deduplication is required for two reasons that the historical
/// fresh-id-per-operand scheme violated:
/// 1. **Codegen correctness**: renderers emit `buf{buf_id}` as a unique kernel
///    parameter name; two bindings with the same id (the same source) would
///    declare the parameter twice. Collapsing repeats keeps names unique.
/// 2. **Data routing**: the same source must map to one physical input buffer
///    (e.g. `x - x` reads one buffer, used for both operands), mirroring the
///    `srcs: [Buf(1), Buf(1)]` convention the interpreter and Metal paths expect.
///
/// `binding_st` is the [`ShapeTracker`] view every input binding of this kernel
/// shares: the output view for elementwise ops, the source view for a reduce
/// (whose input element count differs from its output).
fn build_input_bindings(
    ctx: &mut ScheduleCtx,
    binding_st: &ShapeTracker,
    operands: &[&Arc<LazyOp>],
) -> (Vec<BufferBinding>, Vec<usize>) {
    let mut input_bindings: Vec<BufferBinding> = Vec::with_capacity(operands.len());
    let mut operand_slots: Vec<usize> = Vec::with_capacity(operands.len());

    for operand in operands {
        let id = ctx.buf_id_for(operand);
        // bufs[0] is the output; inputs begin at slot 1. Reuse an existing slot
        // if this operand's source is already bound (same buffer id).
        if let Some(pos) = input_bindings.iter().position(|b| b.buf_id == id) {
            operand_slots.push(pos + 1);
        } else {
            let slot = input_bindings.len() + 1;
            input_bindings.push(BufferBinding {
                buf_id: id,
                st: binding_st.clone(),
                dtype: operand.dtype(),
                access: BufferAccess::Read,
            });
            operand_slots.push(slot);
        }
    }

    (input_bindings, operand_slots)
}

/// Run shape specialization on a list of kernels.
///
/// For each kernel whose output shape is fully static (all dimensions
/// known at schedule time, no zero dimensions indicating dynamic sizes):
///
/// 1. Computes the total element count from the output buffer's shape.
/// 2. Selects the largest preferred workgroup size that evenly divides
///    the total element count.
/// 3. Sets `bounds_check_elim = true` when the total is exactly
///    divisible by the local workgroup size, allowing renderers to
///    omit `if (gid < N)` guards.
/// 4. Updates the kernel's `grid` and `local` fields to the optimized
///    values.
/// 5. Stores the specialization metadata in `kernel.spec`.
pub fn specialize_shapes(kernels: &mut [FusedKernel]) {
    for kernel in kernels.iter_mut() {
        // The output buffer is always bufs[0].
        let out_shape = kernel.bufs[0].st.shape();

        // Check that all dimensions are static (nonzero).
        let all_static = !out_shape.is_empty() && out_shape.iter().all(|&d| d > 0);
        if !all_static {
            continue;
        }

        let total: u64 = out_shape.iter().map(|&d| d as u64).product();
        if total == 0 {
            continue;
        }

        // Find the largest preferred local size that evenly divides total.
        let optimal_local_x = PREFERRED_LOCAL_SIZES
            .iter()
            .copied()
            .find(|&ls| total.is_multiple_of(u64::from(ls)))
            .unwrap_or(1); // 1 always divides

        let bounds_check_elim = total.is_multiple_of(u64::from(optimal_local_x));

        // Compute grid: number of workgroups = ceil(total / local).
        // When bounds_check_elim is true, this is exact (no remainder).
        let grid_x = total.div_ceil(u64::from(optimal_local_x)) as u32;

        let spec = ShapeSpecialization {
            bounds_check_elim,
            total_elements: total,
            optimal_local: [optimal_local_x, 1, 1],
            all_static: true,
        };

        // Update the kernel's work distribution to the optimized values.
        kernel.grid = [grid_x, 1, 1];
        kernel.local = [optimal_local_x, 1, 1];
        kernel.spec = Some(spec);

        // Auto-vectorize: set vectorize_width to 4 when conditions are met:
        // 1. Total elements divisible by 4 (for aligned SIMD access)
        // 2. All buffers are Float32 (SIMD float4 loads/stores)
        // 3. All buffer views are contiguous (no stride gaps)
        // 4. No reduce ops (vectorized reduce needs different codegen)
        let can_vectorize = total.is_multiple_of(4)
            && kernel.bufs.iter().all(|b| b.dtype == DType::Float32)
            && kernel.bufs.iter().all(|b| b.st.view().is_contiguous())
            && !kernel
                .ops
                .iter()
                .any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));
        if can_vectorize {
            kernel.vectorize_width = 4;
        }
    }
}

/// Deduplicate kernels that have identical op chains and buffer shapes.
///
/// Two kernels are "structurally identical" if they have the same ops
/// (same PrimitiveOp sequence with same FusedSrc structure), same buffer
/// shapes and dtypes, and same grid/local sizes — differing only in
/// buffer IDs. This is common in multi-head attention where each head
/// runs the same computation on different data.
///
/// Returns `(deduplicated_kernels, dedup_count)`:
/// - `deduplicated_kernels`: the kernel list with duplicates replaced by
///   canonical references (same compiled shader, different buffer bindings).
/// - `dedup_count`: number of kernels that were deduplicated.
pub fn deduplicate_kernels(kernels: &[FusedKernel]) -> (Vec<FusedKernel>, usize) {
    if kernels.len() <= 1 {
        return (kernels.to_vec(), 0);
    }

    let mut canonical: HashMap<u64, usize> = HashMap::new();
    let mut result = Vec::with_capacity(kernels.len());
    let mut dedup_count = 0;

    for kernel in kernels {
        let sig = kernel_structural_hash(kernel);
        if let std::collections::hash_map::Entry::Vacant(e) = canonical.entry(sig) {
            e.insert(result.len());
        } else {
            dedup_count += 1;
        }
        result.push(kernel.clone());
    }

    (result, dedup_count)
}

/// Compute a structural hash of a kernel that ignores buffer IDs.
/// Two kernels with the same hash are structurally identical (modulo
/// buffer IDs) and can share the same compiled shader.
fn kernel_structural_hash(kernel: &FusedKernel) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    // Hash op chain structure
    for op in &kernel.ops {
        std::mem::discriminant(&op.op).hash(&mut hasher);
        op.dst_dtype.hash(&mut hasher);
        for src in &op.srcs {
            match src {
                FusedSrc::Buf(_) => 0u8.hash(&mut hasher),
                FusedSrc::Op(idx) => {
                    1u8.hash(&mut hasher);
                    idx.hash(&mut hasher);
                }
                FusedSrc::Const { val, dtype } => {
                    2u8.hash(&mut hasher);
                    val.to_bits().hash(&mut hasher);
                    dtype.hash(&mut hasher);
                }
            }
        }
    }

    // Hash buffer shapes and dtypes (not IDs)
    for buf in &kernel.bufs {
        buf.st.shape().hash(&mut hasher);
        buf.dtype.hash(&mut hasher);
        buf.access.hash(&mut hasher);
    }

    // Hash grid/local sizes
    kernel.grid.hash(&mut hasher);
    kernel.local.hash(&mut hasher);

    hasher.finish()
}

fn schedule_recursive(node: &Arc<LazyOp>, kernels: &mut Vec<FusedKernel>, ctx: &mut ScheduleCtx) {
    match node.as_ref() {
        LazyOp::Buffer { .. } => {
            // Leaf node — already materialized, nothing to schedule. Its buffer
            // id is its own `DeviceBufferRef::id`, resolved on demand by
            // `ScheduleCtx::buf_id_for` when a parent op binds it.
        }
        LazyOp::Unary { op, src } => {
            schedule_recursive(src, kernels, ctx);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();

            let out_id = ctx.buf_id_for(node);
            let st = ShapeTracker::contiguous(&shape);
            let (inputs, slots) =
                build_input_bindings(ctx, &st, &[src]);

            let mut bufs = Vec::with_capacity(1 + inputs.len());
            bufs.push(BufferBinding {
                buf_id: out_id,
                st: st.clone(),
                dtype: node.dtype(),
                access: BufferAccess::Write,
            });
            bufs.extend(inputs);

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(slots[0])],
                    dst_dtype: node.dtype(),
                }],
                bufs,
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            });
        }
        LazyOp::Binary { op, lhs, rhs } => {
            schedule_recursive(lhs, kernels, ctx);
            schedule_recursive(rhs, kernels, ctx);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();

            let out_id = ctx.buf_id_for(node);
            let st = ShapeTracker::contiguous(&shape);
            let (inputs, slots) =
                build_input_bindings(ctx, &st, &[lhs, rhs]);

            let mut bufs = Vec::with_capacity(1 + inputs.len());
            bufs.push(BufferBinding {
                buf_id: out_id,
                st: st.clone(),
                dtype: node.dtype(),
                access: BufferAccess::Write,
            });
            bufs.extend(inputs);

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(slots[0]), FusedSrc::Buf(slots[1])],
                    dst_dtype: node.dtype(),
                }],
                bufs,
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            });
        }
        LazyOp::Ternary { op, cond, a, b } => {
            schedule_recursive(cond, kernels, ctx);
            schedule_recursive(a, kernels, ctx);
            schedule_recursive(b, kernels, ctx);
            let shape = node.shape();
            let n = shape.iter().product::<usize>();

            let out_id = ctx.buf_id_for(node);
            let st = ShapeTracker::contiguous(&shape);
            let (inputs, slots) =
                build_input_bindings(ctx, &st, &[cond, a, b]);

            let mut bufs = Vec::with_capacity(1 + inputs.len());
            bufs.push(BufferBinding {
                buf_id: out_id,
                st: st.clone(),
                dtype: node.dtype(),
                access: BufferAccess::Write,
            });
            bufs.extend(inputs);

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![
                        FusedSrc::Buf(slots[0]),
                        FusedSrc::Buf(slots[1]),
                        FusedSrc::Buf(slots[2]),
                    ],
                    dst_dtype: node.dtype(),
                }],
                bufs,
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            });
        }
        LazyOp::Reduce { op, src, axis: _ } => {
            schedule_recursive(src, kernels, ctx);
            let in_shape = src.shape();
            let out_shape = node.shape();
            let out_n = out_shape.iter().product::<usize>().max(1);

            let out_id = ctx.buf_id_for(node);
            // The reduce input keeps the SOURCE shape (which differs from the
            // output), so the binding ShapeTracker is the input shape, not the
            // output shape.
            let in_st = ShapeTracker::contiguous(&in_shape);
            let (inputs, slots) =
                build_input_bindings(ctx, &in_st, &[src]);

            let mut bufs = Vec::with_capacity(1 + inputs.len());
            bufs.push(BufferBinding {
                buf_id: out_id,
                st: ShapeTracker::contiguous(&out_shape),
                dtype: node.dtype(),
                access: BufferAccess::Write,
            });
            bufs.extend(inputs);

            kernels.push(FusedKernel {
                ops: vec![FusedOp {
                    op: *op,
                    srcs: vec![FusedSrc::Buf(slots[0])],
                    dst_dtype: node.dtype(),
                }],
                bufs,
                grid: [out_n as u32, 1, 1],
                local: [out_n.min(256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            });
        }
        LazyOp::Movement { src, st: _ } => {
            // Movement ops are free — just modify the ShapeTracker.
            schedule_recursive(src, kernels, ctx);
        }
        LazyOp::Contiguous { src } => {
            // Force materialization — insert a copy kernel.
            schedule_recursive(src, kernels, ctx);
        }
    }
}
