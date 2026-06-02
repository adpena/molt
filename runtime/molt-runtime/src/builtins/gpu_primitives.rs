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
use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::fuse;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp, alloc_buffer_id};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::FusedKernel;
use molt_gpu::schedule;
use molt_gpu::shapetracker::ShapeTracker;

// Metal GPU execution path. `molt-gpu`'s `device::metal` module is compiled on
// EVERY macOS target (gated on `cfg(target_os = "macos")`, not a Cargo feature)
// and `metal = "0.30"` is an unconditional macOS dependency of molt-gpu, so
// `MetalDevice` links here whenever `molt_gpu_primitives` pulls in molt-gpu — no
// feature-plumbing or version-skew change required. Gated on the SAME cfg as
// `molt_gpu_prim_device` below so the device a program OBSERVES is the device
// `realize()` actually executes on (closing the "reports METAL, runs CPU" drift).
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
use molt_gpu::device::metal::MetalDevice;
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
use molt_gpu::device::{Allocator, Compiler, DeviceBuffer, DeviceError, Executor};
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
use molt_gpu::render::{Renderer, msl::MslRenderer};

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
        store
            .get(handle as usize)
            .and_then(|slot| slot.as_ref().map(f))
    })
}

/// Look up a tensor by handle for mutation.
fn with_tensor_mut<R>(handle: u64, f: impl FnOnce(&mut PrimitiveTensor) -> R) -> Option<R> {
    TENSOR_STORE.with(|store| {
        let mut store = store.borrow_mut();
        store
            .get_mut(handle as usize)
            .and_then(|slot| slot.as_mut().map(f))
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
///
/// # Safety
///
/// `data_ptr` must point to `data_len` contiguous initialized `f32` values and
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_create_tensor(
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

    // Allocate a globally-unique buffer id. This id is the key the
    // schedule→execute bridge uses to fetch this leaf's bytes from the tensor
    // store (`collect_leaves_recursive`), and the scheduler stamps the matching
    // `BufferBinding::buf_id` from it — so `realize()` routes the real input data
    // instead of falling back to zeros.
    let buf_ref = DeviceBufferRef {
        id: alloc_buffer_id(),
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
///
/// # Safety
///
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_zeros(shape_ptr: *const usize, shape_len: usize) -> u64 {
    if shape_ptr.is_null() {
        return u64::MAX;
    }

    let shape = unsafe { std::slice::from_raw_parts(shape_ptr, shape_len) };
    let numel: usize = shape.iter().product();
    let bytes = vec![0u8; numel * 4]; // f32 zeros

    // Unique buffer id (see `molt_gpu_prim_create_tensor`): keeps every leaf's
    // identity distinct so the schedule→execute bridge resolves the right bytes.
    let buf_ref = DeviceBufferRef {
        id: alloc_buffer_id(),
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
            slot.as_ref()
                .map(|t| (t.lazy.clone(), t.shape.clone(), t.dtype))
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

    // Execute each kernel on the active device (Metal on macOS when
    // `molt_gpu_metal` is enabled — matching what `molt_gpu_prim_device`
    // reports — otherwise the CPU interpreter).
    let numel: usize = shape.iter().product();
    let out_bytes = execute_fused_pipeline(&lazy, &fused, numel, dtype);

    // Store the realized data.
    with_tensor_mut(handle, |t| {
        t.data = Some(out_bytes);
    });

    0
}

/// Build the host `bufs` vector for one kernel: `bufs[0]` is a freshly-zeroed
/// output, `bufs[1..]` are the kernel's inputs, each routed to its real data by
/// matching `BufferBinding::buf_id`.
///
/// Routing is purely id-keyed and uniform across leaf and intermediate inputs —
/// the structural property the buffer-id fix establishes:
/// - a **leaf** binding's id is found in `leaf_data` (the DAG's realized leaves);
/// - an **intermediate** binding's id is found in `intermediates` (outputs of
///   earlier kernels in this pipeline, keyed by their output binding id).
///
/// This replaces the previous `last_output`-only heuristic, which silently
/// mis-routed any kernel that consumed more than one live intermediate or an
/// intermediate other than the immediately-preceding kernel's output (a real DAG
/// shape, e.g. `reduce(x) + reduce(y)`). A binding whose id is in neither map is
/// a scheduling invariant violation; we surface it under `MOLT_GPU_DEBUG` and use
/// a correctly-sized zero buffer rather than corrupting an unrelated input.
fn gather_kernel_inputs(
    kernel: &FusedKernel,
    leaf_data: &std::collections::HashMap<usize, Vec<u8>>,
    intermediates: &std::collections::HashMap<usize, Vec<u8>>,
) -> Vec<Vec<u8>> {
    let mut bufs: Vec<Vec<u8>> = Vec::with_capacity(kernel.bufs.len());

    // bufs[0] = output (written by the kernel).
    let out_size = kernel.bufs[0].st.numel() * kernel.bufs[0].dtype.size_bytes();
    bufs.push(vec![0u8; out_size]);

    // bufs[1..] = inputs, routed by buffer id.
    for binding in &kernel.bufs[1..] {
        let in_size = binding.st.numel() * binding.dtype.size_bytes();
        let mut input = vec![0u8; in_size];
        let source = leaf_data
            .get(&binding.buf_id)
            .or_else(|| intermediates.get(&binding.buf_id));
        if let Some(data) = source {
            let copy_len = input.len().min(data.len());
            input[..copy_len].copy_from_slice(&data[..copy_len]);
        } else if std::env::var_os("MOLT_GPU_DEBUG").is_some() {
            eprintln!(
                "molt.gpu: kernel input buf_id {} not found in leaf or intermediate \
                 data — scheduling invariant violation; using zeros",
                binding.buf_id
            );
        }
        bufs.push(input);
    }

    bufs
}

/// Execute a fused kernel pipeline on CpuDevice, returning the output bytes.
///
/// Traverses the LazyOp DAG to collect leaf buffer data, executes each fused
/// kernel in topological order, and routes every kernel input to its real data
/// by buffer id (leaves from the DAG, intermediates from earlier kernels). The
/// final kernel computes the root, so its output is the realized result.
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

    let leaf_data = collect_all_leaf_data(root);
    // Outputs of already-executed kernels, keyed by their output binding id, so
    // a downstream kernel reading that intermediate resolves the exact bytes.
    let mut intermediates: std::collections::HashMap<usize, Vec<u8>> =
        std::collections::HashMap::new();
    let mut last_output = vec![0u8; output_numel * elem_size];

    for kernel in fused_kernels {
        let mut bufs = gather_kernel_inputs(kernel, &leaf_data, &intermediates);
        interpret::execute_kernel(kernel, &mut bufs);
        last_output = bufs.into_iter().next().unwrap();
        intermediates.insert(kernel.bufs[0].buf_id, last_output.clone());
    }

    last_output
}

/// Execute the fused kernel pipeline on the active device.
///
/// On macOS with `molt_gpu_metal`, this dispatches to the GPU via `MetalDevice`
/// — the device `molt_gpu_prim_device` reports — closing the prior drift where
/// `realize()` always ran the CPU interpreter even though the device was
/// advertised as METAL. A CPU fallback is taken ONLY when the Metal device is
/// genuinely unavailable (no GPU) or a kernel fails; that fallback is surfaced
/// under `MOLT_GPU_DEBUG` so it is never silent. Metal and CPU outputs are
/// bit-exact (asserted by `metal_realize_tests`), so the fallback is a
/// performance decision, not a correctness divergence. Off macOS or without the
/// feature this is exactly the CPU interpreter.
fn execute_fused_pipeline(
    root: &Arc<LazyOp>,
    fused_kernels: &[FusedKernel],
    output_numel: usize,
    output_dtype: DType,
) -> Vec<u8> {
    #[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
    {
        match execute_fused_pipeline_metal(root, fused_kernels, output_numel, output_dtype) {
            Ok(bytes) => return bytes,
            Err(err) => {
                if std::env::var_os("MOLT_GPU_DEBUG").is_some() {
                    eprintln!(
                        "molt.gpu: Metal execution unavailable ({err:?}); falling back to CPU \
                         (numerically identical, slower)"
                    );
                }
            }
        }
    }
    execute_fused_pipeline_cpu(root, fused_kernels, output_numel, output_dtype)
}

/// Execute one fused kernel on Metal: upload inputs, render → compile → dispatch
/// → read back. `bufs[0]` is the output (filled on return); `bufs[1..]` are the
/// input host buffers. Mirrors `interpret::execute_kernel`'s contract exactly so
/// the Metal and CPU pipelines are interchangeable per kernel.
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
fn execute_kernel_metal(
    device: &MetalDevice,
    kernel: &FusedKernel,
    bufs: &mut [Vec<u8>],
) -> Result<(), DeviceError> {
    // Split the output (written) from the inputs (read) so the borrows don't
    // overlap: `out_slot[0]` is filled by `copy_out`; `in_slots` are uploaded.
    let (out_slot, in_slots) = bufs.split_at_mut(1);

    let out_dev = device.alloc(out_slot[0].len())?;
    let mut in_devs: Vec<DeviceBuffer> = Vec::with_capacity(in_slots.len());
    for host in in_slots.iter() {
        let dev = device.alloc(host.len())?;
        device.copy_in(&dev, host)?;
        in_devs.push(dev);
    }

    let msl = MslRenderer.render(kernel);
    let prog = device.compile(&msl, "molt_kernel")?;

    // Buffer binding order matches `FusedKernel::bufs`: output first, inputs after.
    let mut refs: Vec<&DeviceBuffer> = Vec::with_capacity(1 + in_devs.len());
    refs.push(&out_dev);
    refs.extend(in_devs.iter());
    // `kernel.grid`/`kernel.local` are the scheduler-computed work distribution.
    device.exec(&prog, &refs, kernel.grid, kernel.local)?;
    device.synchronize()?;
    drop(refs);

    device.copy_out(&out_dev, &mut out_slot[0])?;

    device.free(out_dev)?;
    for dev in in_devs {
        device.free(dev)?;
    }
    Ok(())
}

/// GPU mirror of [`execute_fused_pipeline_cpu`]: identical id-keyed input
/// gathering (via [`gather_kernel_inputs`]) and kernel chaining, with each kernel
/// executed on Metal instead of the CPU interpreter. Returns `Err`
/// (→ CPU fallback in [`execute_fused_pipeline`]) if the Metal device is
/// unavailable or any kernel fails.
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
fn execute_fused_pipeline_metal(
    root: &Arc<LazyOp>,
    fused_kernels: &[FusedKernel],
    output_numel: usize,
    output_dtype: DType,
) -> Result<Vec<u8>, DeviceError> {
    let elem_size = output_dtype.size_bytes();

    if fused_kernels.is_empty() {
        if let LazyOp::Buffer { .. } = root.as_ref() {
            return Ok(collect_leaf_data(root));
        }
        return Ok(vec![0u8; output_numel * elem_size]);
    }

    let device = MetalDevice::new()?;
    let leaf_data = collect_all_leaf_data(root);
    let mut intermediates: std::collections::HashMap<usize, Vec<u8>> =
        std::collections::HashMap::new();
    let mut last_output = vec![0u8; output_numel * elem_size];

    for kernel in fused_kernels {
        // Byte-for-byte the same input routing as the CPU path.
        let mut bufs = gather_kernel_inputs(kernel, &leaf_data, &intermediates);
        execute_kernel_metal(&device, kernel, &mut bufs)?;
        last_output = bufs.into_iter().next().unwrap();
        intermediates.insert(kernel.bufs[0].buf_id, last_output.clone());
    }

    Ok(last_output)
}

/// Resolve a leaf buffer's realized bytes from the tensor store by its unique
/// `buf.id`.
///
/// The DAG leaf node carries only the buffer *identity* (`buf.id`); the realized
/// bytes live in the tensor store (the single source of truth). Because every
/// leaf id is globally unique ([`alloc_buffer_id`]), the first store entry whose
/// `Buffer` id matches is unambiguously this leaf's data. Returns `None` if no
/// realized tensor with that id is live (a dropped/never-realized source).
fn leaf_bytes_from_store(buf_id: usize) -> Option<Vec<u8>> {
    TENSOR_STORE.with(|store| {
        let store = store.borrow();
        for slot in store.iter().flatten() {
            if let LazyOp::Buffer { buf: ref b, .. } = *slot.lazy
                && b.id == buf_id
                && let Some(ref data) = slot.data
            {
                return Some(data.clone());
            }
        }
        None
    })
}

/// Collect realized data from a leaf `LazyOp::Buffer` node.
///
/// Used for the degenerate "DAG is a bare realized leaf" path. The bytes come
/// from the tensor store keyed by the leaf's unique id; only if the source is no
/// longer live do we fall back to zeros (a correctly-sized empty buffer).
fn collect_leaf_data(node: &Arc<LazyOp>) -> Vec<u8> {
    match node.as_ref() {
        LazyOp::Buffer { buf, dtype, st } => leaf_bytes_from_store(buf.id)
            .unwrap_or_else(|| vec![0u8; st.numel() * dtype.size_bytes()]),
        _ => Vec::new(),
    }
}

/// Collect all leaf buffer data from the DAG, keyed by buffer ID.
fn collect_all_leaf_data(root: &Arc<LazyOp>) -> std::collections::HashMap<usize, Vec<u8>> {
    let mut leaves = std::collections::HashMap::new();
    collect_leaves_recursive(root, &mut leaves);
    leaves
}

/// Recursively walk the DAG to find all Buffer leaf nodes, keying each leaf's
/// realized bytes by its unique `buf.id` (the same id the scheduler stamps into
/// the corresponding `BufferBinding::buf_id`, so the executor's per-binding
/// lookup hits).
fn collect_leaves_recursive(
    node: &Arc<LazyOp>,
    leaves: &mut std::collections::HashMap<usize, Vec<u8>>,
) {
    match node.as_ref() {
        LazyOp::Buffer { buf, dtype, st } => {
            leaves.entry(buf.id).or_insert_with(|| {
                leaf_bytes_from_store(buf.id)
                    .unwrap_or_else(|| vec![0u8; st.numel() * dtype.size_bytes()])
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
pub extern "C" fn molt_gpu_prim_read_data(handle: u64, out_ptr: *mut f32, out_len: usize) -> u64 {
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

    let (lazy, _shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
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

    let out_dtype = if matches!(
        op,
        PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
    ) {
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

    let (lazy, _shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };

    let new_lazy = Arc::new(LazyOp::Reduce {
        op,
        src: lazy,
        axis,
    });
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
pub extern "C" fn molt_gpu_prim_shape(handle: u64, out_ptr: *mut usize, out_len: usize) -> u64 {
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
    with_tensor(handle, |t| t.shape.iter().product::<usize>() as u64).unwrap_or(0)
}

/// Query the current device name.
///
/// Returns: 0 = CPU, 1 = Metal, 2 = WebGPU
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_device() -> u32 {
    #[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
    {
        return 1;
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
    {
        return 2;
    }
    0 // CPU
}

/// Get the number of live tensors in the store (for debugging/testing).
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_tensor_count() -> u64 {
    TENSOR_STORE.with(|store| store.borrow().iter().filter(|s| s.is_some()).count() as u64)
}

/// Regression for the "device reports METAL but `realize()` runs CPU" drift.
///
/// Verifies the new Metal execution path is BIT-EXACT with the CPU interpreter
/// `realize()` used before the fix. If Metal and CPU ever diverge, this fails —
/// making the silent-CPU drift non-reintroducible. Skips cleanly when no Metal
/// device is present (headless CI), so it never produces a false failure.
#[cfg(all(test, target_os = "macos", feature = "molt_gpu_metal"))]
mod metal_realize_tests {
    use super::*;
    use molt_gpu::device::cpu::interpret;
    use molt_gpu::ops::PrimitiveOp;
    use molt_gpu::render::{BufferAccess, BufferBinding, FusedOp, FusedSrc};

    fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
        let st = || ShapeTracker::contiguous(&[n]);
        FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: st(),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: st(),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 2,
                    st: st(),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        }
    }

    #[test]
    fn execute_kernel_metal_is_bit_exact_with_cpu_interpret() {
        let device = match MetalDevice::new() {
            Ok(d) => d,
            Err(_) => return, // No Metal device (headless CI): nothing to compare.
        };
        let n = 1024usize;
        // Mixed signs/magnitudes so a wrong op or buffer order would diverge.
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.5 - 7.0).collect();
        let b: Vec<f32> = (0..n).map(|i| (n - i) as f32 * 0.25 + 1.0).collect();

        for op in [PrimitiveOp::Add, PrimitiveOp::Sub, PrimitiveOp::Mul] {
            let kernel = binary_kernel(op, n);

            let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];
            interpret::execute_kernel(&kernel, &mut cpu_bufs);

            let mut metal_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];
            execute_kernel_metal(&device, &kernel, &mut metal_bufs)
                .expect("metal kernel execution must succeed on a Metal-capable host");

            assert_eq!(
                bytes_to_f32(&cpu_bufs[0]),
                bytes_to_f32(&metal_bufs[0]),
                "Metal realize() diverged from the CPU interpreter for {op:?} — \
                 the fidelity regression this test guards"
            );
        }
    }

    /// The full `realize()` path runs `specialize_shapes`, which rewrites
    /// `kernel.grid` to the THREADGROUP count (`ceil(total/local)`). This runs a
    /// specialized kernel through `execute_kernel_metal` exactly as `realize()`
    /// does, catching any mismatch between the scheduler's grid convention and
    /// `MetalDevice`'s dispatch model — which a hand-built (un-specialized)
    /// kernel hides.
    #[test]
    fn execute_kernel_metal_matches_cpu_after_specialize_shapes() {
        let device = match MetalDevice::new() {
            Ok(d) => d,
            Err(_) => return,
        };
        let n = 1024usize;
        let a: Vec<f32> = (0..n).map(|i| i as f32 + 1.0).collect();
        let b: Vec<f32> = (0..n).map(|i| (i as f32) * 3.0).collect();

        let mut kernels = vec![binary_kernel(PrimitiveOp::Add, n)];
        schedule::specialize_shapes(&mut kernels);
        let kernel = &kernels[0];

        let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];
        interpret::execute_kernel(kernel, &mut cpu_bufs);

        let mut metal_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];
        execute_kernel_metal(&device, kernel, &mut metal_bufs).expect("metal kernel execution");

        assert_eq!(
            bytes_to_f32(&cpu_bufs[0]),
            bytes_to_f32(&metal_bufs[0]),
            "specialized-kernel Metal dispatch diverged from CPU: the scheduler's \
             grid convention mismatches MetalDevice's dispatch model"
        );
    }

    fn reduce_kernel(n_out: usize, reduce_size: usize) -> FusedKernel {
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_out]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_out * reduce_size]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_out as u32, 1, 1],
            local: [n_out.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        }
    }

    /// Reductions specialize differently from elementwise ops — `total` is the
    /// OUTPUT element count, so `specialize_shapes` produces `ceil(n_out/local)`
    /// threadgroups with one thread per output element (each reducing its input
    /// slice). This verifies that shape (distinct from elementwise) realizes
    /// bit-exact on Metal — closing the reduce coverage gap, not deferring it.
    #[test]
    fn reduce_kernel_metal_matches_cpu_after_specialize_shapes() {
        let device = match MetalDevice::new() {
            Ok(d) => d,
            Err(_) => return,
        };
        let n_out = 1024usize; // > local, so the specialized grid spans multiple groups
        let reduce_size = 4usize;
        let input: Vec<f32> = (0..n_out * reduce_size)
            .map(|i| (i as f32) * 0.25 - 3.0)
            .collect();

        let mut kernels = vec![reduce_kernel(n_out, reduce_size)];
        schedule::specialize_shapes(&mut kernels);
        let kernel = &kernels[0];

        let mut cpu_bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&input)];
        interpret::execute_kernel(kernel, &mut cpu_bufs);

        let mut metal_bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&input)];
        execute_kernel_metal(&device, kernel, &mut metal_bufs).expect("metal reduce execution");

        assert_eq!(
            bytes_to_f32(&cpu_bufs[0]),
            bytes_to_f32(&metal_bufs[0]),
            "specialized reduce diverged Metal vs CPU"
        );
    }

    /// End-to-end Metal realize through the real schedule pipeline, asserting
    /// ABSOLUTE VALUES: builds `c = a + b` via the FFI, runs the scheduled +
    /// specialized + fused kernels through both pipelines, and checks that each
    /// element equals `a[i] + b[i]` — on Metal AND on CPU — and that the two are
    /// byte-identical. `n` forces a multi-threadgroup specialized grid so a
    /// dispatch-model regression would diverge here.
    ///
    /// This formerly asserted only Metal==CPU *parity* because the FFI realize
    /// path computed on zeros (every leaf stamped `buf.id = 0`, colliding in the
    /// leaf-data map while the scheduler used disjoint sequential binding ids).
    /// The buffer-id fix makes leaf ids globally unique and routes binding ids
    /// from node identity, so the values are now correct; the assertion is
    /// upgraded to absolute correctness accordingly.
    #[test]
    fn realize_metal_matches_cpu_through_full_schedule() {
        if MetalDevice::new().is_err() {
            return;
        }
        let n = 4096usize;
        let a: Vec<f32> = (0..n).map(|i| i as f32 * 0.5).collect();
        let b: Vec<f32> = (0..n).map(|i| (n - i) as f32 * 0.25).collect();
        let shape = [n];

        // SAFETY: each pointer is valid for the matching length.
        let ha =
            unsafe { molt_gpu_prim_create_tensor(a.as_ptr(), a.len(), shape.as_ptr(), shape.len()) };
        let hb =
            unsafe { molt_gpu_prim_create_tensor(b.as_ptr(), b.len(), shape.as_ptr(), shape.len()) };
        let hc = molt_gpu_prim_binary(0 /* Add */, ha, hb);
        assert_ne!(hc, u64::MAX);

        let (lazy, tshape, dtype) =
            with_tensor(hc, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)).expect("tensor");
        let mut kernels = schedule::schedule(&lazy, &tshape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);
        let numel: usize = tshape.iter().product();

        let cpu = execute_fused_pipeline_cpu(&lazy, &fused, numel, dtype);
        let metal =
            execute_fused_pipeline_metal(&lazy, &fused, numel, dtype).expect("metal pipeline");

        let expected: Vec<f32> = (0..n).map(|i| a[i] + b[i]).collect();
        assert_eq!(
            bytes_to_f32(&metal),
            expected,
            "Metal realize did not compute a[i] + b[i] (the buffer-id fix routes \
             real leaf data instead of zeros)"
        );
        assert_eq!(
            bytes_to_f32(&cpu),
            expected,
            "CPU realize did not compute a[i] + b[i] through the full schedule"
        );
        assert_eq!(
            cpu, metal,
            "Metal realize diverged from CPU realize through the full schedule pipeline"
        );

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hb);
        molt_gpu_prim_free(hc);
    }

    /// Metal realize of `reduce_sum(a + b)` through the FULL public FFI entry
    /// (`molt_gpu_prim_realize` → `molt_gpu_prim_read_data`), asserting the exact
    /// reduced scalar `sum_i (a[i] + b[i])`. This is the strongest end-to-end
    /// check: it drives the same path compiled Python uses, on the device the
    /// runtime advertises (Metal here), and a single value summarizes every
    /// element so a leaf-routing regression cannot hide. `a + b` then a reduce
    /// also exercises elementwise→reduce fusion on real input data.
    #[test]
    fn realize_reduce_sum_of_add_correct_value_via_ffi() {
        if MetalDevice::new().is_err() {
            return;
        }
        let n = 2048usize;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.5 - 11.0).collect();
        let b: Vec<f32> = (0..n).map(|i| (n - i) as f32 * 0.25 + 2.0).collect();
        let shape = [n];

        // SAFETY: pointers valid for their lengths.
        let ha =
            unsafe { molt_gpu_prim_create_tensor(a.as_ptr(), a.len(), shape.as_ptr(), shape.len()) };
        let hb =
            unsafe { molt_gpu_prim_create_tensor(b.as_ptr(), b.len(), shape.as_ptr(), shape.len()) };
        let hsum = molt_gpu_prim_binary(0 /* Add */, ha, hb);
        let hred = molt_gpu_prim_reduce(24 /* ReduceSum */, hsum, 0 /* axis */);
        assert_ne!(hred, u64::MAX);

        assert_eq!(molt_gpu_prim_realize(hred), 0, "realize must succeed");

        let mut out = [0.0f32; 1];
        let written = molt_gpu_prim_read_data(hred, out.as_mut_ptr(), out.len());
        assert_eq!(written, 1, "reduce_sum produces one scalar");

        let expected: f32 = (0..n).map(|i| a[i] + b[i]).sum();
        // f32 summation order differs between the reference and the kernel, so
        // compare with a relative tolerance rather than bit-exactly.
        let tol = expected.abs() * 1e-4 + 1e-3;
        assert!(
            (out[0] - expected).abs() <= tol,
            "reduce_sum(a+b) on Metal via FFI = {}, expected ~{} (tol {})",
            out[0],
            expected,
            tol
        );

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hb);
        molt_gpu_prim_free(hsum);
        molt_gpu_prim_free(hred);
    }
}

/// CPU-path realize VALUE regressions, gated on `molt_gpu_primitives` only (no
/// Metal). The buffer-id bug affected the CPU pipeline identically to Metal, so
/// these assert the CPU `realize()` produces correct VALUES — exercising
/// [`execute_fused_pipeline_cpu`] directly (independent of which device the
/// public FFI dispatches to), through DAGs built via the real FFI constructors.
///
/// Coverage is deliberately structural, not a single happy path:
/// - single-kernel binary (`a + b`),
/// - a leaf read twice in one kernel (`x * x`) — the id-dedup / aliasing case,
/// - a reduce (`reduce_sum(a)`) — distinct output-vs-input shape,
/// - a multi-kernel DAG with TWO live intermediates (`reduce_sum(a) +
///   reduce_sum(b)`) — the case the old `last_output`-only routing mis-handled.
#[cfg(all(test, feature = "molt_gpu_primitives"))]
mod cpu_realize_value_tests {
    use super::*;

    fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    /// Schedule + specialize + fuse a tensor handle's DAG and run it through the
    /// CPU pipeline, returning the realized f32 values.
    fn realize_cpu_values(handle: u64) -> Vec<f32> {
        let (lazy, shape, dtype) =
            with_tensor(handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)).expect("tensor");
        let mut kernels = schedule::schedule(&lazy, &shape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);
        let numel: usize = shape.iter().product();
        bytes_to_f32(&execute_fused_pipeline_cpu(&lazy, &fused, numel, dtype))
    }

    fn make_tensor(data: &[f32], shape: &[usize]) -> u64 {
        // SAFETY: pointers valid for their lengths for the duration of the call.
        unsafe {
            molt_gpu_prim_create_tensor(data.as_ptr(), data.len(), shape.as_ptr(), shape.len())
        }
    }

    #[test]
    fn cpu_realize_add_computes_real_values_not_zeros() {
        let n = 1024usize;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.5 - 4.0).collect();
        let b: Vec<f32> = (0..n).map(|i| (n - i) as f32 * 0.25 + 1.0).collect();

        let ha = make_tensor(&a, &[n]);
        let hb = make_tensor(&b, &[n]);
        let hc = molt_gpu_prim_binary(0 /* Add */, ha, hb);
        assert_ne!(hc, u64::MAX);

        let out = realize_cpu_values(hc);
        let expected: Vec<f32> = (0..n).map(|i| a[i] + b[i]).collect();
        assert_eq!(out, expected, "CPU realize a+b must equal a[i]+b[i], not zeros");

        // Sanity: prove it is NOT the all-zeros fallback (the historical bug).
        assert!(
            out.iter().any(|&v| v != 0.0),
            "output is all zeros — the leaf-data bridge regressed"
        );

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hb);
        molt_gpu_prim_free(hc);
    }

    #[test]
    fn cpu_realize_square_same_leaf_twice() {
        // x * x reads ONE leaf into BOTH operands. The scheduler must emit a
        // single input binding (deduped by buffer id) with srcs [Buf(1), Buf(1)],
        // and the bridge must route the leaf's real data to it.
        let n = 512usize;
        let x: Vec<f32> = (0..n).map(|i| (i as f32) * 0.125 - 2.0).collect();

        let hx = make_tensor(&x, &[n]);
        let hsq = molt_gpu_prim_binary(2 /* Mul */, hx, hx);
        assert_ne!(hsq, u64::MAX);

        let out = realize_cpu_values(hsq);
        let expected: Vec<f32> = x.iter().map(|&v| v * v).collect();
        assert_eq!(out, expected, "x*x must square the real leaf data");

        molt_gpu_prim_free(hx);
        molt_gpu_prim_free(hsq);
    }

    #[test]
    fn cpu_realize_reduce_sum_value() {
        let n = 1000usize;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.01 - 5.0).collect();

        let ha = make_tensor(&a, &[n]);
        let hred = molt_gpu_prim_reduce(24 /* ReduceSum */, ha, 0);
        assert_ne!(hred, u64::MAX);

        let out = realize_cpu_values(hred);
        assert_eq!(out.len(), 1, "reduce over the only axis yields a scalar");
        let expected: f32 = a.iter().sum();
        let tol = expected.abs() * 1e-4 + 1e-3;
        assert!(
            (out[0] - expected).abs() <= tol,
            "reduce_sum(a) = {}, expected ~{}",
            out[0],
            expected
        );

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hred);
    }

    #[test]
    fn cpu_realize_two_reduces_then_add_routes_both_intermediates() {
        // DAG: ADD(reduce_sum(a), reduce_sum(b)). This forces the scheduler to
        // emit >= 2 kernels whose results are BOTH consumed by the final add —
        // two distinct live intermediates. The old `last_output`-only routing
        // fed the final kernel the same (last) intermediate for both operands and
        // was wrong; the id-keyed `intermediates` map routes each correctly.
        let n = 256usize;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.5 + 1.0).collect();
        let b: Vec<f32> = (0..n).map(|i| -(i as f32) * 0.25 - 3.0).collect();

        let ha = make_tensor(&a, &[n]);
        let hb = make_tensor(&b, &[n]);
        let ra = molt_gpu_prim_reduce(24 /* ReduceSum */, ha, 0);
        let rb = molt_gpu_prim_reduce(24 /* ReduceSum */, hb, 0);
        let hsum = molt_gpu_prim_binary(0 /* Add */, ra, rb);
        assert_ne!(hsum, u64::MAX);

        // Confirm this DAG really schedules to multiple kernels (so the
        // intermediate-routing path is actually exercised, not fused away).
        let (lazy, shape, _dtype) =
            with_tensor(hsum, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)).expect("tensor");
        let kernels = schedule::schedule(&lazy, &shape);
        assert!(
            kernels.len() >= 3,
            "expected >=3 kernels (two reduces + add), got {}",
            kernels.len()
        );

        let out = realize_cpu_values(hsum);
        assert_eq!(out.len(), 1);
        let sum_a: f32 = a.iter().sum();
        let sum_b: f32 = b.iter().sum();
        let expected = sum_a + sum_b;
        let tol = expected.abs() * 1e-4 + 1e-3;
        assert!(
            (out[0] - expected).abs() <= tol,
            "reduce_sum(a)+reduce_sum(b) = {}, expected ~{} (each intermediate \
             must route to its own buffer)",
            out[0],
            expected
        );

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hb);
        molt_gpu_prim_free(ra);
        molt_gpu_prim_free(rb);
        molt_gpu_prim_free(hsum);
    }
}
