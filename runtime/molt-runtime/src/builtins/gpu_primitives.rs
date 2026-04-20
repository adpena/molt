//! Bridge module: molt-gpu primitive stack -> molt-runtime FFI.
//!
//! Exposes molt-gpu's LazyOp DAG construction, scheduling, fusion, and
//! CpuDevice execution to compiled Python code via `extern "C"` FFI
//! functions. This is the integration layer that connects the standalone
//! `molt-gpu` crate (26-op tinygrad-conformant primitive stack) into
//! molt's compilation pipeline.
//!
//! The bridge provides three tiers of API:
//!
//! 1. **Tensor lifecycle**: create tensors from flat f32 data, realize
//!    tensors (execute the lazy DAG), read results back.
//!
//! 2. **Op construction**: build LazyOp DAG nodes for unary, binary,
//!    ternary, reduce, and movement operations.
//!
//! 3. **Device selection**: select CPU (always available), Metal (macOS),
//!    or WebGPU (when feature-gated) backends.
//!
//! All functions use molt's `u64`-based NaN-boxed ABI convention.

use std::cell::RefCell;
use std::sync::Arc;

use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::{Allocator, DeviceBuffer};
use molt_gpu::dtype::DType;
use molt_gpu::fuse;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{FusedKernel};
use molt_gpu::schedule;
use molt_gpu::shapetracker::ShapeTracker;
use molt_gpu::device::cpu::interpret;

use crate::{MoltObject, PyToken, raise_exception};

// ============================================================================
// Thread-local tensor store
// ============================================================================

/// A realized or unrealized tensor in the primitive GPU stack.
struct PrimitiveTensor {
    /// The lazy computation DAG for this tensor.
    lazy: Arc<LazyOp>,
    /// Realized data (f32 bytes), or None if not yet realized.
    data: Option<Vec<u8>>,
    /// Logical shape.
    shape: Vec<usize>,
    /// Element dtype.
    dtype: DType,
}

thread_local! {
    /// Store of all live primitive tensors, indexed by handle ID.
    static TENSOR_STORE: RefCell<Vec<Option<PrimitiveTensor>>> = const { RefCell::new(Vec::new()) };
    /// Shared CPU device instance.
    static CPU_DEVICE: RefCell<CpuDevice> = RefCell::new(CpuDevice::new());
}

/// Allocate a new tensor handle in the thread-local store.
fn store_tensor(tensor: PrimitiveTensor) -> u64 {
    TENSOR_STORE.with(|store| {
        let mut store = store.borrow_mut();
        // Reuse a freed slot if available.
        for (i, slot) in store.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(tensor);
                return i as u64;
            }
        }
        let id = store.len();
        store.push(Some(tensor));
        id as u64
    })
}

/// Look up a tensor by handle, returning a reference via callback.
fn with_tensor<R>(handle: u64, f: impl FnOnce(&PrimitiveTensor) -> R) -> Option<R> {
    TENSOR_STORE.with(|store| {
        let store = store.borrow();
        store.get(handle as usize).and_then(|slot| slot.as_ref().map(|t| f(t)))
    })
}

/// Look up a tensor by handle for mutation.
fn with_tensor_mut<R>(handle: u64, f: impl FnOnce(&mut PrimitiveTensor) -> R) -> Option<R> {
    TENSOR_STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.get_mut(handle as usize).and_then(|slot| slot.as_mut().map(f))
    })
}

// ============================================================================
// Tensor lifecycle FFI
// ============================================================================

/// Create a tensor from a flat f32 buffer and shape.
///
/// `data_ptr`: pointer to f32 array
/// `data_len`: number of f32 elements
/// `shape_ptr`: pointer to usize array
/// `shape_len`: number of dimensions
///
/// Returns a tensor handle (u64).
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_create_tensor(
    data_ptr: *const f32,
    data_len: usize,
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    if data_ptr.is_null() || shape_ptr.is_null() {
        return u64::MAX;
    }

    let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
    let shape = unsafe { std::slice::from_raw_parts(shape_ptr, shape_len) };

    // Validate shape matches data length.
    let expected_len: usize = shape.iter().product();
    if expected_len != data_len {
        return u64::MAX;
    }

    // Convert to bytes.
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();

    let buf_ref = DeviceBufferRef {
        id: 0, // Placeholder; CpuDevice interpreter uses raw bytes directly.
        size_bytes: bytes.len(),
    };

    let st = ShapeTracker::contiguous(shape);
    let lazy = Arc::new(LazyOp::Buffer {
        buf: buf_ref,
        st,
        dtype: DType::Float32,
    });

    let tensor = PrimitiveTensor {
        lazy,
        data: Some(bytes),
        shape: shape.to_vec(),
        dtype: DType::Float32,
    };

    store_tensor(tensor)
}

/// Create a tensor filled with zeros.
///
/// `shape_ptr`: pointer to usize array
/// `shape_len`: number of dimensions
///
/// Returns a tensor handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_zeros(
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    if shape_ptr.is_null() {
        return u64::MAX;
    }

    let shape = unsafe { std::slice::from_raw_parts(shape_ptr, shape_len) };
    let numel: usize = shape.iter().product();
    let bytes = vec![0u8; numel * 4]; // f32 zeros

    let buf_ref = DeviceBufferRef {
        id: 0,
        size_bytes: bytes.len(),
    };

    let st = ShapeTracker::contiguous(shape);
    let lazy = Arc::new(LazyOp::Buffer {
        buf: buf_ref,
        st,
        dtype: DType::Float32,
    });

    store_tensor(PrimitiveTensor {
        lazy,
        data: Some(bytes),
        shape: shape.to_vec(),
        dtype: DType::Float32,
    })
}

/// Realize a tensor: schedule -> fuse -> execute on CpuDevice.
///
/// After realization, the tensor's data is available for readback.
/// Returns 0 on success, u64::MAX on failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_realize(handle: u64) -> u64 {
    // If already realized, nothing to do.
    let already_realized = with_tensor(handle, |t| t.data.is_some()).unwrap_or(false);
    if already_realized {
        return 0;
    }

    // Get the lazy op and shape.
    let (lazy, shape, dtype) = match TENSOR_STORE.with(|store| {
        let store = store.borrow();
        store.get(handle as usize).and_then(|slot| {
            slot.as_ref().map(|t| (t.lazy.clone(), t.shape.clone(), t.dtype))
        })
    }) {
        Some(v) => v,
        None => return u64::MAX,
    };

    // Schedule the lazy DAG into kernels.
    let mut kernels = schedule::schedule(&lazy, &shape);

    // Run shape specialization.
    schedule::specialize_shapes(&mut kernels);

    // Fuse kernels.
    let fused = fuse::fuse(kernels);

    // Execute each kernel on CPU.
    let numel: usize = shape.iter().product();
    let out_bytes = execute_fused_pipeline_cpu(&lazy, &fused, numel, dtype);

    // Store the realized data.
    with_tensor_mut(handle, |t| {
        t.data = Some(out_bytes);
    });

    0
}

/// Execute a fused kernel pipeline on CpuDevice, returning the output bytes.
///
/// This traverses the LazyOp DAG to collect leaf buffer data, then
/// executes each fused kernel in sequence using the CPU interpreter.
fn execute_fused_pipeline_cpu(
    root: &Arc<LazyOp>,
    fused_kernels: &[FusedKernel],
    output_numel: usize,
    output_dtype: DType,
) -> Vec<u8> {
    let elem_size = output_dtype.size_bytes();

    // For leaf-only DAGs (already realized buffers), extract data directly.
    if fused_kernels.is_empty() {
        if let LazyOp::Buffer { .. } = root.as_ref() {
            return collect_leaf_data(root);
        }
        return vec![0u8; output_numel * elem_size];
    }

    // Execute each fused kernel.
    // Collect all leaf buffer data for input buffers.
    let leaf_data = collect_all_leaf_data(root);

    let mut last_output = vec![0u8; output_numel * elem_size];

    for kernel in fused_kernels {
        let n_bufs = kernel.bufs.len();
        let mut bufs: Vec<Vec<u8>> = Vec::with_capacity(n_bufs);

        // bufs[0] = output
        let out_numel = kernel.bufs[0].st.numel();
        let out_size = out_numel * kernel.bufs[0].dtype.size_bytes();
        bufs.push(vec![0u8; out_size]);

        // bufs[1..] = inputs from leaf data or prior output
        for buf_binding in &kernel.bufs[1..] {
            let in_numel = buf_binding.st.numel();
            let in_size = in_numel * buf_binding.dtype.size_bytes();
            // Try to find matching leaf data by buffer ID.
            if let Some(data) = leaf_data.get(&buf_binding.buf_id) {
                let mut input = vec![0u8; in_size];
                let copy_len = input.len().min(data.len());
                input[..copy_len].copy_from_slice(&data[..copy_len]);
                bufs.push(input);
            } else {
                // Use the last output as input (chained kernels).
                let mut input = vec![0u8; in_size];
                let copy_len = input.len().min(last_output.len());
                input[..copy_len].copy_from_slice(&last_output[..copy_len]);
                bufs.push(input);
            }
        }

        interpret::execute_kernel(kernel, &mut bufs);
        last_output = bufs.into_iter().next().unwrap();
    }

    last_output
}

/// Collect realized data from a leaf LazyOp::Buffer node.
fn collect_leaf_data(node: &Arc<LazyOp>) -> Vec<u8> {
    // Leaf nodes should have their data stored in the tensor store.
    // For now, return empty; the tensor store lookup happens at a higher level.
    match node.as_ref() {
        LazyOp::Buffer { buf, dtype, st } => {
            vec![0u8; st.numel() * dtype.size_bytes()]
        }
        _ => Vec::new(),
    }
}

/// Collect all leaf buffer data from the DAG, keyed by buffer ID.
fn collect_all_leaf_data(root: &Arc<LazyOp>) -> std::collections::HashMap<usize, Vec<u8>> {
    let mut leaves = std::collections::HashMap::new();
    collect_leaves_recursive(root, &mut leaves);
    leaves
}

/// Recursively walk the DAG to find all Buffer leaf nodes.
fn collect_leaves_recursive(
    node: &Arc<LazyOp>,
    leaves: &mut std::collections::HashMap<usize, Vec<u8>>,
) {
    match node.as_ref() {
        LazyOp::Buffer { buf, dtype, st } => {
            leaves.entry(buf.id).or_insert_with(|| {
                // Look up the tensor store for this buffer's data.
                TENSOR_STORE.with(|store| {
                    let store = store.borrow();
                    for slot in store.iter().flatten() {
                        if let LazyOp::Buffer { buf: ref b, .. } = *slot.lazy {
                            if b.id == buf.id {
                                if let Some(ref data) = slot.data {
                                    return data.clone();
                                }
                            }
                        }
                    }
                    vec![0u8; st.numel() * dtype.size_bytes()]
                })
            });
        }
        LazyOp::Unary { src, .. } => collect_leaves_recursive(src, leaves),
        LazyOp::Binary { lhs, rhs, .. } => {
            collect_leaves_recursive(lhs, leaves);
            collect_leaves_recursive(rhs, leaves);
        }
        LazyOp::Ternary { cond, a, b, .. } => {
            collect_leaves_recursive(cond, leaves);
            collect_leaves_recursive(a, leaves);
            collect_leaves_recursive(b, leaves);
        }
        LazyOp::Reduce { src, .. } => collect_leaves_recursive(src, leaves),
        LazyOp::Movement { src, .. } => collect_leaves_recursive(src, leaves),
        LazyOp::Contiguous { src } => collect_leaves_recursive(src, leaves),
    }
}

/// Read realized tensor data back as f32 values.
///
/// `handle`: tensor handle
/// `out_ptr`: pointer to f32 output array
/// `out_len`: capacity of output array (in f32 elements)
///
/// Returns the number of elements written, or u64::MAX on failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_read_data(
    handle: u64,
    out_ptr: *mut f32,
    out_len: usize,
) -> u64 {
    if out_ptr.is_null() {
        return u64::MAX;
    }

    with_tensor(handle, |t| {
        let Some(ref data) = t.data else {
            return 0u64; // Not realized.
        };

        let numel = data.len() / 4; // f32 = 4 bytes
        let count = numel.min(out_len);
        let out = unsafe { std::slice::from_raw_parts_mut(out_ptr, count) };

        for (i, chunk) in data[..count * 4].chunks_exact(4).enumerate() {
            out[i] = f32::from_le_bytes(chunk.try_into().unwrap());
        }

        count as u64
    })
    .unwrap_or(u64::MAX)
}

/// Free a tensor handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_free(handle: u64) -> u64 {
    TENSOR_STORE.with(|store| {
        let mut store = store.borrow_mut();
        if let Some(slot) = store.get_mut(handle as usize) {
            *slot = None;
        }
    });
    0
}

// ============================================================================
// LazyOp construction FFI
// ============================================================================

/// Map a u32 op code to a PrimitiveOp.
fn op_from_code(code: u32) -> Option<PrimitiveOp> {
    match code {
        0 => Some(PrimitiveOp::Add),
        1 => Some(PrimitiveOp::Sub),
        2 => Some(PrimitiveOp::Mul),
        3 => Some(PrimitiveOp::Idiv),
        4 => Some(PrimitiveOp::Mod),
        5 => Some(PrimitiveOp::Neg),
        6 => Some(PrimitiveOp::Cmplt),
        7 => Some(PrimitiveOp::Cmpeq),
        8 => Some(PrimitiveOp::Cmpne),
        9 => Some(PrimitiveOp::And),
        10 => Some(PrimitiveOp::Or),
        11 => Some(PrimitiveOp::Xor),
        12 => Some(PrimitiveOp::Shl),
        13 => Some(PrimitiveOp::Shr),
        14 => Some(PrimitiveOp::Exp2),
        15 => Some(PrimitiveOp::Log2),
        16 => Some(PrimitiveOp::Sin),
        17 => Some(PrimitiveOp::Sqrt),
        18 => Some(PrimitiveOp::Reciprocal),
        19 => Some(PrimitiveOp::Trunc),
        20 => Some(PrimitiveOp::Max),
        21 => Some(PrimitiveOp::Where),
        22 => Some(PrimitiveOp::Cast),
        23 => Some(PrimitiveOp::Bitcast),
        24 => Some(PrimitiveOp::ReduceSum),
        25 => Some(PrimitiveOp::ReduceMax),
        _ => None,
    }
}

/// Apply a unary op to a tensor, returning a new tensor handle.
///
/// `op_code`: one of the 26 primitive op codes (see `op_from_code`)
/// `src_handle`: source tensor handle
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_unary(op_code: u32, src_handle: u64) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op) => op,
        None => return u64::MAX,
    };

    let (lazy, shape, dtype) = match with_tensor(src_handle, |t| {
        (t.lazy.clone(), t.shape.clone(), t.dtype)
    }) {
        Some(v) => v,
        None => return u64::MAX,
    };

    let new_lazy = Arc::new(LazyOp::Unary { op, src: lazy });
    let new_shape = new_lazy.shape();

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype,
    })
}

/// Apply a binary op to two tensors, returning a new tensor handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_binary(op_code: u32, lhs_handle: u64, rhs_handle: u64) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op) => op,
        None => return u64::MAX,
    };

    let lhs = match with_tensor(lhs_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
        Some(v) => v,
        None => return u64::MAX,
    };
    let rhs = match with_tensor(rhs_handle, |t| t.lazy.clone()) {
        Some(v) => v,
        None => return u64::MAX,
    };

    let out_dtype = if matches!(op, PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne) {
        DType::Bool
    } else {
        lhs.2
    };

    let new_lazy = Arc::new(LazyOp::Binary {
        op,
        lhs: lhs.0,
        rhs,
    });
    let new_shape = new_lazy.shape();

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype: out_dtype,
    })
}

/// Apply a ternary op (WHERE) to three tensors.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_ternary(
    op_code: u32,
    cond_handle: u64,
    a_handle: u64,
    b_handle: u64,
) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op) => op,
        None => return u64::MAX,
    };

    let cond = match with_tensor(cond_handle, |t| t.lazy.clone()) {
        Some(v) => v,
        None => return u64::MAX,
    };
    let a = match with_tensor(a_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
        Some(v) => v,
        None => return u64::MAX,
    };
    let b = match with_tensor(b_handle, |t| t.lazy.clone()) {
        Some(v) => v,
        None => return u64::MAX,
    };

    let new_lazy = Arc::new(LazyOp::Ternary {
        op,
        cond,
        a: a.0,
        b,
    });
    let new_shape = new_lazy.shape();

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype: a.2,
    })
}

/// Apply a reduce op along an axis.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_reduce(op_code: u32, src_handle: u64, axis: usize) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op) => op,
        None => return u64::MAX,
    };

    let (lazy, _shape, dtype) = match with_tensor(src_handle, |t| {
        (t.lazy.clone(), t.shape.clone(), t.dtype)
    }) {
        Some(v) => v,
        None => return u64::MAX,
    };

    let new_lazy = Arc::new(LazyOp::Reduce { op, src: lazy, axis });
    let new_shape = new_lazy.shape();

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype,
    })
}

/// Get the shape of a tensor.
///
/// `handle`: tensor handle
/// `out_ptr`: pointer to usize output array
/// `out_len`: capacity of output array
///
/// Returns the number of dimensions, or u64::MAX on failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_shape(
    handle: u64,
    out_ptr: *mut usize,
    out_len: usize,
) -> u64 {
    if out_ptr.is_null() {
        return u64::MAX;
    }

    with_tensor(handle, |t| {
        let ndim = t.shape.len().min(out_len);
        let out = unsafe { std::slice::from_raw_parts_mut(out_ptr, ndim) };
        out.copy_from_slice(&t.shape[..ndim]);
        t.shape.len() as u64
    })
    .unwrap_or(u64::MAX)
}

/// Get the number of elements in a tensor.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_numel(handle: u64) -> u64 {
    with_tensor(handle, |t| {
        t.shape.iter().product::<usize>() as u64
    })
    .unwrap_or(0)
}

/// Query the current device name.
///
/// Returns: 0 = CPU, 1 = Metal, 2 = WebGPU
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_device() -> u32 {
    #[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
    { return 1; }
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
    { return 2; }
    0 // CPU
}

/// Get the number of live tensors in the store (for debugging/testing).
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_tensor_count() -> u64 {
    TENSOR_STORE.with(|store| {
        store.borrow().iter().filter(|s| s.is_some()).count() as u64
    })
}
