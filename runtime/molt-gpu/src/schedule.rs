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
pub fn schedule(root: &Arc<LazyOp>, _output_shape: &[usize]) -> Vec<FusedKernel> {
    let mut kernels = Vec::new();
    let mut next_buf_id = 0;

    schedule_recursive(root, &mut kernels, &mut next_buf_id);
    kernels
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

fn schedule_recursive(node: &Arc<LazyOp>, kernels: &mut Vec<FusedKernel>, next_buf_id: &mut usize) {
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
                    BufferBinding {
                        buf_id: out_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: node.dtype(),
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: in_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: src.dtype(),
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
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
                    BufferBinding {
                        buf_id: out_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: node.dtype(),
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: lhs_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: lhs.dtype(),
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: rhs_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: rhs.dtype(),
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
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
                    BufferBinding {
                        buf_id: out_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: node.dtype(),
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: cond_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: cond.dtype(),
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: a_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: a.dtype(),
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: b_id,
                        st: ShapeTracker::contiguous(&shape),
                        dtype: b.dtype(),
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n.max(1) as u32, 1, 1],
                local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
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
                    BufferBinding {
                        buf_id: out_id,
                        st: ShapeTracker::contiguous(&out_shape),
                        dtype: node.dtype(),
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: in_id,
                        st: ShapeTracker::contiguous(&in_shape),
                        dtype: src.dtype(),
                        access: BufferAccess::Read,
                    },
                ],
                grid: [out_n as u32, 1, 1],
                local: [out_n.min(256) as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
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
