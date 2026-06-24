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
//! 1. **Tensor lifecycle**: create tensors from flat f32 data or typed raw
//!    storage bytes, realize tensors (execute the lazy DAG), read results back.
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
    /// Realized storage bytes, or None if not yet realized.
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

fn store_tensor_from_storage_bytes(shape: &[usize], dtype: DType, bytes: Vec<u8>) -> Option<u64> {
    if dtype.is_mxfp() {
        return None;
    }
    if tensor_storage_nbytes(shape, dtype)? != bytes.len() {
        return None;
    }

    let buf_ref = DeviceBufferRef {
        id: alloc_buffer_id(),
        size_bytes: bytes.len(),
    };

    let lazy = Arc::new(LazyOp::Buffer {
        buf: buf_ref,
        st: ShapeTracker::contiguous(shape),
        dtype,
    });

    Some(store_tensor(PrimitiveTensor {
        lazy,
        data: Some(bytes),
        shape: shape.to_vec(),
        dtype,
    }))
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

    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    store_tensor_from_storage_bytes(shape, DType::Float32, bytes).unwrap_or(u64::MAX)
}

/// Create a tensor from exact typed storage bytes and shape.
///
/// `data_ptr`: pointer to initialized storage bytes
/// `data_len`: number of bytes
/// `dtype_code`: dtype code from `dtype_from_code`
/// `shape_ptr`: pointer to usize array
/// `shape_len`: number of dimensions
///
/// MXFP storage is rejected until the block/exponent layout contract is explicit.
/// Returns a tensor handle (u64), or `u64::MAX` on invalid dtype, shape/byte
/// mismatch, unsupported storage layout, or null pointer.
///
/// # Safety
///
/// `data_ptr` must point to `data_len` contiguous initialized bytes and
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_create_tensor_raw(
    data_ptr: *const u8,
    data_len: usize,
    dtype_code: u32,
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    if (data_len > 0 && data_ptr.is_null()) || shape_ptr.is_null() {
        return u64::MAX;
    }

    let dtype = match dtype_from_code(dtype_code) {
        Some(dtype) => dtype,
        None => return u64::MAX,
    };
    let shape = unsafe { std::slice::from_raw_parts(shape_ptr, shape_len) };
    let data = if data_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data_ptr, data_len) }
    };

    store_tensor_from_storage_bytes(shape, dtype, data.to_vec()).unwrap_or(u64::MAX)
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
    let Some(nbytes) = tensor_storage_nbytes(shape, DType::Float32) else {
        return u64::MAX;
    };
    store_tensor_from_storage_bytes(shape, DType::Float32, vec![0u8; nbytes]).unwrap_or(u64::MAX)
}

/// Create a typed zero-filled tensor.
///
/// MXFP storage is rejected until the block/exponent layout contract is explicit.
///
/// # Safety
///
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_zeros_dtype(
    dtype_code: u32,
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    if shape_ptr.is_null() {
        return u64::MAX;
    }

    let dtype = match dtype_from_code(dtype_code) {
        Some(dtype) => dtype,
        None => return u64::MAX,
    };
    let shape = unsafe { std::slice::from_raw_parts(shape_ptr, shape_len) };
    let Some(nbytes) = tensor_storage_nbytes(shape, dtype) else {
        return u64::MAX;
    };

    store_tensor_from_storage_bytes(shape, dtype, vec![0u8; nbytes]).unwrap_or(u64::MAX)
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
/// output, `bufs[1..]` are full source-storage inputs, each routed to its real
/// data by matching `BufferBinding::buf_id`.
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
/// a scheduling invariant violation and fails immediately.
///
/// Input slots must carry full source storage bytes, not `binding.st.numel()`
/// bytes. Shrink/offset/flip views index into the underlying storage via
/// `ShapeTracker::expr_idx`; truncating the host slot to logical view length can
/// turn a valid physical read into an out-of-bounds zero.
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
        let source = leaf_data
            .get(&binding.buf_id)
            .or_else(|| intermediates.get(&binding.buf_id))
            .unwrap_or_else(|| {
                panic!(
                    "molt.gpu: kernel input buf_id {} not found in leaf or intermediate data",
                    binding.buf_id
                )
            });
        bufs.push(source.clone());
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
        panic!(
            "molt.gpu: scheduler emitted no kernels for non-buffer root {} \
             ({} output elements of {:?})",
            lazy_op_kind(root),
            output_numel,
            output_dtype
        );
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
    let mut input_devs: std::collections::HashMap<usize, DeviceBuffer> =
        std::collections::HashMap::new();
    for (slot_offset, host) in in_slots.iter().enumerate() {
        let binding = &kernel.bufs[slot_offset + 1];
        if let std::collections::hash_map::Entry::Vacant(entry) = input_devs.entry(binding.buf_id) {
            let dev = device.alloc(host.len())?;
            device.copy_in(&dev, host)?;
            entry.insert(dev);
        }
    }

    let msl = MslRenderer.render(kernel);
    let prog = device.compile(&msl, "molt_kernel")?;

    // Buffer binding order matches `FusedKernel::bufs`: output first, inputs after.
    let mut refs: Vec<&DeviceBuffer> = Vec::with_capacity(kernel.bufs.len());
    refs.push(&out_dev);
    for binding in &kernel.bufs[1..] {
        refs.push(input_devs.get(&binding.buf_id).unwrap_or_else(|| {
            panic!(
                "molt.gpu: device buffer for input buf_id {} was not allocated",
                binding.buf_id
            )
        }));
    }
    // `kernel.grid`/`kernel.local` are the scheduler-computed work distribution.
    device.exec(&prog, &refs, kernel.grid, kernel.local)?;
    device.synchronize()?;
    drop(refs);

    device.copy_out(&out_dev, &mut out_slot[0])?;

    device.free(out_dev)?;
    for dev in input_devs.into_values() {
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
        return Err(DeviceError::InvalidArgument(format!(
            "scheduler emitted no kernels for non-buffer root {} ({} output elements of {:?})",
            lazy_op_kind(root),
            output_numel,
            output_dtype
        )));
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
/// `Buffer` id matches is unambiguously this leaf's data.
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
/// from the tensor store keyed by the leaf's unique id. Missing realized bytes
/// are a runtime consistency violation and raise immediately.
fn collect_leaf_data(node: &Arc<LazyOp>) -> Vec<u8> {
    match node.as_ref() {
        LazyOp::Buffer { buf, .. } => leaf_bytes_from_store(buf.id).unwrap_or_else(|| {
            panic!(
                "molt.gpu: realized bytes for leaf buf_id {} are missing",
                buf.id
            )
        }),
        _ => Vec::new(),
    }
}

fn lazy_op_kind(node: &Arc<LazyOp>) -> &'static str {
    match node.as_ref() {
        LazyOp::Buffer { .. } => "Buffer",
        LazyOp::Unary { .. } => "Unary",
        LazyOp::Cast { .. } => "Cast",
        LazyOp::Binary { .. } => "Binary",
        LazyOp::Ternary { .. } => "Ternary",
        LazyOp::Reduce { .. } => "Reduce",
        LazyOp::Movement { .. } => "Movement",
        LazyOp::Contiguous { .. } => "Contiguous",
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
                leaf_bytes_from_store(buf.id).unwrap_or_else(|| {
                    panic!(
                        "molt.gpu: realized bytes for leaf buf_id {} ({} elements of {:?}) \
                         are missing",
                        buf.id,
                        st.numel(),
                        dtype
                    )
                })
            });
        }
        LazyOp::Unary { src, .. } => collect_leaves_recursive(src, leaves),
        LazyOp::Cast { src, .. } => collect_leaves_recursive(src, leaves),
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
        if t.dtype != DType::Float32 {
            return u64::MAX;
        }

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

/// Return this tensor's dtype code, or u64::MAX for an invalid handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_dtype(handle: u64) -> u64 {
    with_tensor(handle, |t| dtype_to_code(t.dtype) as u64).unwrap_or(u64::MAX)
}

/// Return this tensor's logical storage byte count, or u64::MAX on overflow.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_nbytes(handle: u64) -> u64 {
    with_tensor(handle, |t| {
        tensor_storage_nbytes(&t.shape, t.dtype)
            .map(|nbytes| nbytes as u64)
            .unwrap_or(u64::MAX)
    })
    .unwrap_or(u64::MAX)
}

/// Copy realized tensor storage bytes into `out_ptr` without dtype reinterpretation.
///
/// The caller must provide the expected dtype code and enough byte capacity for
/// the whole tensor. Partial copies are rejected to avoid silently publishing
/// truncated tensor state.
///
/// # Safety
///
/// `out_ptr` must be valid for writes of at least `out_len` bytes for the
/// duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_read_data_raw(
    handle: u64,
    expected_dtype_code: u32,
    out_ptr: *mut u8,
    out_len: usize,
) -> u64 {
    if out_ptr.is_null() {
        return u64::MAX;
    }
    let expected_dtype = match dtype_from_code(expected_dtype_code) {
        Some(dtype) => dtype,
        None => return u64::MAX,
    };

    with_tensor(handle, |t| {
        if t.dtype != expected_dtype {
            return u64::MAX;
        }

        let Some(ref data) = t.data else {
            return 0u64; // Not realized.
        };

        if tensor_storage_nbytes(&t.shape, t.dtype) != Some(data.len()) || out_len < data.len() {
            return u64::MAX;
        }

        let out = unsafe { std::slice::from_raw_parts_mut(out_ptr, data.len()) };
        out.copy_from_slice(data);
        data.len() as u64
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

fn dtype_from_code(code: u32) -> Option<DType> {
    match code {
        0 => Some(DType::Bool),
        1 => Some(DType::Int8),
        2 => Some(DType::Int16),
        3 => Some(DType::Int32),
        4 => Some(DType::Int64),
        5 => Some(DType::UInt8),
        6 => Some(DType::UInt16),
        7 => Some(DType::UInt32),
        8 => Some(DType::UInt64),
        9 => Some(DType::Float16),
        10 => Some(DType::BFloat16),
        11 => Some(DType::Float32),
        12 => Some(DType::Float64),
        13 => Some(DType::MxFP8),
        14 => Some(DType::MxFP4),
        _ => None,
    }
}

fn dtype_to_code(dtype: DType) -> u32 {
    match dtype {
        DType::Bool => 0,
        DType::Int8 => 1,
        DType::Int16 => 2,
        DType::Int32 => 3,
        DType::Int64 => 4,
        DType::UInt8 => 5,
        DType::UInt16 => 6,
        DType::UInt32 => 7,
        DType::UInt64 => 8,
        DType::Float16 => 9,
        DType::BFloat16 => 10,
        DType::Float32 => 11,
        DType::Float64 => 12,
        DType::MxFP8 => 13,
        DType::MxFP4 => 14,
    }
}

fn tensor_storage_nbytes(shape: &[usize], dtype: DType) -> Option<usize> {
    shape
        .iter()
        .try_fold(1usize, |numel, dim| numel.checked_mul(*dim))
        .and_then(|numel| numel.checked_mul(dtype.size_bytes()))
}

fn tensor_numel(shape: &[usize]) -> Option<usize> {
    shape
        .iter()
        .try_fold(1usize, |numel, dim| numel.checked_mul(*dim))
}

unsafe fn raw_usize_slice<'a>(ptr: *const usize, len: usize) -> Option<&'a [usize]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

fn flat_usize_pairs(values: &[usize], ndim: usize) -> Option<Vec<(usize, usize)>> {
    if values.len() != ndim.checked_mul(2)? {
        return None;
    }
    Some(
        values
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect(),
    )
}

fn is_valid_permutation(order: &[usize], ndim: usize) -> bool {
    if order.len() != ndim {
        return false;
    }
    let mut seen = vec![false; ndim];
    for &axis in order {
        if axis >= ndim || seen[axis] {
            return false;
        }
        seen[axis] = true;
    }
    true
}

fn output_shapetracker(lazy: &LazyOp) -> ShapeTracker {
    match lazy {
        LazyOp::Buffer { st, .. } | LazyOp::Movement { st, .. } => st.clone(),
        LazyOp::Contiguous { src } => ShapeTracker::contiguous(&src.shape()),
        _ => ShapeTracker::contiguous(&lazy.shape()),
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
    if matches!(op, PrimitiveOp::Cast | PrimitiveOp::Bitcast) {
        return u64::MAX;
    }

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

/// Apply a typed cast/bitcast to a tensor, returning a new tensor handle.
///
/// `op_code`: `Cast` or `Bitcast`
/// `dst_dtype_code`: DType code from `dtype_from_code`
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_cast(op_code: u32, src_handle: u64, dst_dtype_code: u32) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op @ (PrimitiveOp::Cast | PrimitiveOp::Bitcast)) => op,
        Some(_) | None => return u64::MAX,
    };
    let dst_dtype = match dtype_from_code(dst_dtype_code) {
        Some(dtype) => dtype,
        None => return u64::MAX,
    };

    let (lazy, _shape) = match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone())) {
        Some(v) => v,
        None => return u64::MAX,
    };

    let new_lazy = Arc::new(LazyOp::Cast {
        op,
        src: lazy,
        dst_dtype,
    });
    let new_shape = new_lazy.shape();

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype: dst_dtype,
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
        Some(PrimitiveOp::Where) => PrimitiveOp::Where,
        Some(_) => return u64::MAX,
        None => return u64::MAX,
    };

    let cond = match with_tensor(cond_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
        Some(v) => v,
        None => return u64::MAX,
    };
    let a = match with_tensor(a_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
        Some(v) => v,
        None => return u64::MAX,
    };
    let b = match with_tensor(b_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
        Some(v) => v,
        None => return u64::MAX,
    };
    if cond.1 != a.1 || cond.2 != DType::Bool || b.1 != a.1 || b.2 != a.2 {
        return u64::MAX;
    }

    let new_lazy = Arc::new(LazyOp::Ternary {
        op,
        cond: cond.0,
        a: a.0,
        b: b.0,
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
        Some(op @ (PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)) => op,
        Some(_) => return u64::MAX,
        None => return u64::MAX,
    };

    let (lazy, shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    let Some((new_lazy, keepdim_shape)) = keepdim_reduce_lazy(op, lazy, shape, axis) else {
        return u64::MAX;
    };

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: keepdim_shape,
        dtype,
    })
}

/// Apply a reduce op across every axis, returning Molt's public scalar shape
/// `(1,)`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_reduce_all(op_code: u32, src_handle: u64) -> u64 {
    let op = match op_from_code(op_code) {
        Some(op @ (PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)) => op,
        Some(_) => return u64::MAX,
        None => return u64::MAX,
    };

    let (mut lazy, mut shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };

    for axis in 0..shape.len() {
        let Some((next_lazy, next_shape)) = keepdim_reduce_lazy(op, lazy, shape, axis) else {
            return u64::MAX;
        };
        lazy = next_lazy;
        shape = next_shape;
    }

    let out_shape = vec![1usize];
    if shape != out_shape {
        if tensor_numel(&shape) != Some(1) {
            return u64::MAX;
        }
        let st = output_shapetracker(&lazy).reshape(&out_shape);
        lazy = Arc::new(LazyOp::Movement { src: lazy, st });
    }

    store_tensor(PrimitiveTensor {
        lazy,
        data: None,
        shape: out_shape,
        dtype,
    })
}

fn keepdim_reduce_lazy(
    op: PrimitiveOp,
    lazy: Arc<LazyOp>,
    shape: Vec<usize>,
    axis: usize,
) -> Option<(Arc<LazyOp>, Vec<usize>)> {
    if !matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) || axis >= shape.len() {
        return None;
    }
    let reduce_lazy = Arc::new(LazyOp::Reduce {
        op,
        src: lazy,
        axis,
    });
    let squeezed_shape = reduce_lazy.shape();
    let mut keepdim_shape = shape;
    keepdim_shape[axis] = 1;
    let st = ShapeTracker::contiguous(&squeezed_shape).reshape(&keepdim_shape);
    Some((
        Arc::new(LazyOp::Movement {
            src: reduce_lazy,
            st,
        }),
        keepdim_shape,
    ))
}

/// Build a zero-copy reshape view over a tensor.
///
/// # Safety
///
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_reshape(
    src_handle: u64,
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    let new_shape = match unsafe { raw_usize_slice(shape_ptr, shape_len) } {
        Some(shape) => shape,
        None => return u64::MAX,
    };

    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    let Some(old_numel) = tensor_numel(&old_shape) else {
        return u64::MAX;
    };
    let Some(new_numel) = tensor_numel(new_shape) else {
        return u64::MAX;
    };
    if old_numel != new_numel {
        return u64::MAX;
    }

    let st = output_shapetracker(&lazy).reshape(new_shape);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape.to_vec(),
        dtype,
    })
}

/// Build a zero-copy broadcast view over a tensor.
///
/// # Safety
///
/// `shape_ptr` must point to `shape_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_expand(
    src_handle: u64,
    shape_ptr: *const usize,
    shape_len: usize,
) -> u64 {
    let new_shape = match unsafe { raw_usize_slice(shape_ptr, shape_len) } {
        Some(shape) => shape,
        None => return u64::MAX,
    };

    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    if old_shape.len() != new_shape.len() {
        return u64::MAX;
    }
    for (&old, &new) in old_shape.iter().zip(new_shape.iter()) {
        if old != new && old != 1 {
            return u64::MAX;
        }
    }

    let st = output_shapetracker(&lazy).expand(new_shape);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape.to_vec(),
        dtype,
    })
}

/// Build a zero-copy permuted view over a tensor.
///
/// # Safety
///
/// `order_ptr` must point to `order_len` contiguous initialized `usize` values
/// for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_permute(
    src_handle: u64,
    order_ptr: *const usize,
    order_len: usize,
) -> u64 {
    let order = match unsafe { raw_usize_slice(order_ptr, order_len) } {
        Some(order) => order,
        None => return u64::MAX,
    };

    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    if !is_valid_permutation(order, old_shape.len()) {
        return u64::MAX;
    }

    let new_shape: Vec<usize> = order.iter().map(|&axis| old_shape[axis]).collect();
    let st = output_shapetracker(&lazy).permute(order);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype,
    })
}

/// Build a zero-copy zero-fill padded view over a tensor.
///
/// `padding_ptr` is a flattened `(before, after)` pair list with
/// `padding_len == 2 * ndim`.
///
/// # Safety
///
/// `padding_ptr` must point to `padding_len` contiguous initialized `usize`
/// values for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_pad(
    src_handle: u64,
    padding_ptr: *const usize,
    padding_len: usize,
) -> u64 {
    let flat_padding = match unsafe { raw_usize_slice(padding_ptr, padding_len) } {
        Some(padding) => padding,
        None => return u64::MAX,
    };

    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    let padding = match flat_usize_pairs(flat_padding, old_shape.len()) {
        Some(padding) => padding,
        None => return u64::MAX,
    };
    let mut new_shape = Vec::with_capacity(old_shape.len());
    for (&old, &(before, after)) in old_shape.iter().zip(&padding) {
        let Some(padded) = old.checked_add(before).and_then(|n| n.checked_add(after)) else {
            return u64::MAX;
        };
        new_shape.push(padded);
    }
    if tensor_numel(&new_shape).is_none() {
        return u64::MAX;
    }

    let st = output_shapetracker(&lazy).pad(&padding);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype,
    })
}

/// Build a zero-copy shrunk view over a tensor.
///
/// `bounds_ptr` is a flattened `(start, end)` pair list with
/// `bounds_len == 2 * ndim`.
///
/// # Safety
///
/// `bounds_ptr` must point to `bounds_len` contiguous initialized `usize`
/// values for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_shrink(
    src_handle: u64,
    bounds_ptr: *const usize,
    bounds_len: usize,
) -> u64 {
    let flat_bounds = match unsafe { raw_usize_slice(bounds_ptr, bounds_len) } {
        Some(bounds) => bounds,
        None => return u64::MAX,
    };

    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    let bounds = match flat_usize_pairs(flat_bounds, old_shape.len()) {
        Some(bounds) => bounds,
        None => return u64::MAX,
    };
    let mut new_shape = Vec::with_capacity(old_shape.len());
    for (&old, &(start, end)) in old_shape.iter().zip(&bounds) {
        if start > end || end > old {
            return u64::MAX;
        }
        new_shape.push(end - start);
    }
    if tensor_numel(&new_shape).is_none() {
        return u64::MAX;
    }

    let st = output_shapetracker(&lazy).shrink(&bounds);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: new_shape,
        dtype,
    })
}

/// Build a zero-copy flipped view over a tensor.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_flip(src_handle: u64, axis: usize) -> u64 {
    let (lazy, old_shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };
    if axis >= old_shape.len() {
        return u64::MAX;
    }

    let st = output_shapetracker(&lazy).flip(axis);
    let new_lazy = Arc::new(LazyOp::Movement { src: lazy, st });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape: old_shape,
        dtype,
    })
}

/// Insert a materializing contiguous barrier for a tensor DAG.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_contiguous(src_handle: u64) -> u64 {
    let (lazy, shape, dtype) =
        match with_tensor(src_handle, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)) {
            Some(v) => v,
            None => return u64::MAX,
        };

    let new_lazy = Arc::new(LazyOp::Contiguous { src: lazy });

    store_tensor(PrimitiveTensor {
        lazy: new_lazy,
        data: None,
        shape,
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
///
/// # Safety
///
/// `out_ptr` must be valid for writes of at least `out_len` contiguous `usize`
/// values for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_gpu_prim_shape(
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
    with_tensor(handle, |t| t.shape.iter().product::<usize>() as u64).unwrap_or(0)
}

/// Query the current device name.
///
/// Returns: 0 = CPU, 1 = Metal, 2 = WebGPU
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_prim_device() -> u32 {
    if cfg!(all(target_os = "macos", feature = "molt_gpu_metal")) {
        1
    } else if cfg!(all(
        not(target_arch = "wasm32"),
        feature = "molt_gpu_webgpu"
    )) {
        2
    } else {
        0
    }
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
    use molt_gpu::render::{BufferAccess, BufferBinding, FusedOp, FusedSrc, ReductionDomain};

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
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                op,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                DType::Float32,
            )],
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
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n_out, reduce_size], 1),
            )],
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
        let ha = unsafe {
            molt_gpu_prim_create_tensor(a.as_ptr(), a.len(), shape.as_ptr(), shape.len())
        };
        let hb = unsafe {
            molt_gpu_prim_create_tensor(b.as_ptr(), b.len(), shape.as_ptr(), shape.len())
        };
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

    #[test]
    fn metal_realize_same_storage_distinct_view_routes_distinct_slots() {
        if MetalDevice::new().is_err() {
            return;
        }
        let data = vec![1.0f32, 2.0, 3.0, 4.0];
        let shape_in = [4usize];

        // SAFETY: pointer and shape slices are valid for the duration of the call.
        let ha = unsafe {
            molt_gpu_prim_create_tensor(
                data.as_ptr(),
                data.len(),
                shape_in.as_ptr(),
                shape_in.len(),
            )
        };
        let (src, dtype) = with_tensor(ha, |t| (t.lazy.clone(), t.dtype)).expect("source tensor");

        let flipped = Arc::new(LazyOp::Movement {
            src: Arc::clone(&src),
            st: ShapeTracker::contiguous(&[4]).flip(0),
        });
        let add = Arc::new(LazyOp::Binary {
            op: PrimitiveOp::Add,
            lhs: Arc::clone(&src),
            rhs: flipped,
        });
        let shape = add.shape();
        let mut kernels = schedule::schedule(&add, &shape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);

        assert_eq!(fused.len(), 1, "same-storage add should stay one kernel");
        let kernel = &fused[0];
        assert_eq!(kernel.bufs.len(), 3);
        assert_eq!(kernel.bufs[1].buf_id, kernel.bufs[2].buf_id);
        assert_ne!(kernel.bufs[1].st, kernel.bufs[2].st);

        let numel: usize = shape.iter().product();
        let cpu = execute_fused_pipeline_cpu(&add, &fused, numel, dtype);
        let metal =
            execute_fused_pipeline_metal(&add, &fused, numel, dtype).expect("metal pipeline");
        let expected = vec![5.0, 5.0, 5.0, 5.0];
        assert_eq!(bytes_to_f32(&cpu), expected);
        assert_eq!(
            bytes_to_f32(&metal),
            expected,
            "Metal bridge must bind one storage allocation through both view slots"
        );
        assert_eq!(cpu, metal);

        molt_gpu_prim_free(ha);
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
        let ha = unsafe {
            molt_gpu_prim_create_tensor(a.as_ptr(), a.len(), shape.as_ptr(), shape.len())
        };
        let hb = unsafe {
            molt_gpu_prim_create_tensor(b.as_ptr(), b.len(), shape.as_ptr(), shape.len())
        };
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
/// - an offset movement view, where input slots must carry full storage,
/// - a multi-kernel DAG with TWO live intermediates (`reduce_sum(a) +
///   reduce_sum(b)`) — the case the old `last_output`-only routing mis-handled.
#[cfg(all(test, feature = "molt_gpu_primitives"))]
mod cpu_realize_value_tests {
    use super::*;
    use molt_gpu::render::{BufferAccess, BufferBinding, KernelBody};

    fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn u16_to_bytes(vals: &[u16]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
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

    fn make_tensor_raw(data: &[u8], dtype_code: u32, shape: &[usize]) -> u64 {
        // SAFETY: pointers valid for their lengths for the duration of the call.
        unsafe {
            molt_gpu_prim_create_tensor_raw(
                data.as_ptr(),
                data.len(),
                dtype_code,
                shape.as_ptr(),
                shape.len(),
            )
        }
    }

    fn read_data_raw(handle: u64, expected_dtype_code: u32, out: &mut [u8]) -> u64 {
        read_data_raw_len(handle, expected_dtype_code, out, out.len())
    }

    fn read_data_raw_len(
        handle: u64,
        expected_dtype_code: u32,
        out: &mut [u8],
        out_len: usize,
    ) -> u64 {
        // SAFETY: `out` supplies writable storage; tests may intentionally pass
        // a shorter logical length to exercise rejection without invalid memory.
        unsafe {
            molt_gpu_prim_read_data_raw(handle, expected_dtype_code, out.as_mut_ptr(), out_len)
        }
    }

    fn read_shape(handle: u64, out: &mut [usize]) -> u64 {
        // SAFETY: `out` supplies writable storage for its full length.
        unsafe { molt_gpu_prim_shape(handle, out.as_mut_ptr(), out.len()) }
    }

    #[test]
    fn cpu_materialize_copy_broadcast_view_reads_source_offsets() {
        let kernel = FusedKernel {
            body: KernelBody::MaterializeCopy,
            ops: Vec::new(),
            bufs: vec![
                BufferBinding {
                    buf_id: 90,
                    st: ShapeTracker::contiguous(&[2, 3]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 7,
                    st: ShapeTracker::contiguous(&[2, 1]).expand(&[2, 3]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [6, 1, 1],
            local: [6, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let input = [5.0f32, 7.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let mut bufs = vec![vec![0u8; 6 * 4], input];

        interpret::execute_kernel(&kernel, &mut bufs);

        assert_eq!(bytes_to_f32(&bufs[0]), vec![5.0, 5.0, 5.0, 7.0, 7.0, 7.0]);
    }

    #[test]
    #[should_panic(expected = "scheduler emitted no kernels for non-buffer root Unary")]
    fn cpu_empty_pipeline_non_buffer_root_panics_instead_of_zero_fill() {
        let src = Arc::new(LazyOp::Buffer {
            buf: DeviceBufferRef {
                id: usize::MAX - 2,
                size_bytes: 4 * 4,
            },
            st: ShapeTracker::contiguous(&[4]),
            dtype: DType::Float32,
        });
        let neg = Arc::new(LazyOp::Unary {
            op: PrimitiveOp::Neg,
            src,
        });

        let _ = execute_fused_pipeline_cpu(&neg, &[], 4, DType::Float32);
    }

    #[test]
    fn cpu_ffi_create_tensor_raw_roundtrips_exact_uint16_storage() {
        let bytes = u16_to_bytes(&[0x1234, 0xabcd]);
        let handle = make_tensor_raw(&bytes, 6 /* UInt16 */, &[2]);
        assert_ne!(handle, u64::MAX);
        assert_eq!(molt_gpu_prim_dtype(handle), 6);
        assert_eq!(molt_gpu_prim_nbytes(handle), 4);

        let mut f32_out = [99.0f32; 2];
        assert_eq!(
            molt_gpu_prim_read_data(handle, f32_out.as_mut_ptr(), f32_out.len()),
            u64::MAX
        );
        assert_eq!(f32_out, [99.0f32; 2]);

        let mut out = [0u8; 4];
        assert_eq!(read_data_raw(handle, 6, &mut out), 4);
        assert_eq!(out.as_slice(), bytes.as_slice());

        molt_gpu_prim_free(handle);
    }

    #[test]
    fn cpu_ffi_create_tensor_raw_rejects_mismatched_invalid_and_mxfp_storage() {
        let shape = [2usize];
        let short = [0u8; 3];
        assert_eq!(
            make_tensor_raw(&short, 6 /* UInt16 */, &shape),
            u64::MAX,
            "UInt16[2] requires exactly four bytes"
        );
        let valid_len = [0u8; 4];
        assert_eq!(
            make_tensor_raw(&valid_len, 99 /* invalid */, &shape),
            u64::MAX
        );
        assert_eq!(
            make_tensor_raw(&[0u8; 2], 13 /* MxFP8 */, &shape),
            u64::MAX,
            "MXFP raw upload needs an explicit block/exponent storage contract"
        );
    }

    #[test]
    fn cpu_ffi_zeros_dtype_creates_exact_typed_zero_storage() {
        let shape = [3usize];
        // SAFETY: shape pointer is valid for the duration of the call.
        let handle = unsafe {
            molt_gpu_prim_zeros_dtype(7 /* UInt32 */, shape.as_ptr(), shape.len())
        };
        assert_ne!(handle, u64::MAX);
        assert_eq!(molt_gpu_prim_dtype(handle), 7);
        assert_eq!(molt_gpu_prim_nbytes(handle), 12);

        let mut out = [0xa5u8; 12];
        assert_eq!(read_data_raw(handle, 7, &mut out), 12);
        assert_eq!(out, [0u8; 12]);

        molt_gpu_prim_free(handle);

        // SAFETY: shape pointer is valid for the duration of the call.
        assert_eq!(
            unsafe {
                molt_gpu_prim_zeros_dtype(14 /* MxFP4 */, shape.as_ptr(), shape.len())
            },
            u64::MAX
        );
    }

    #[test]
    fn cpu_ffi_raw_int32_upload_executes_integer_add_without_f32_smuggling() {
        let a = i32_to_bytes(&[1, -2, 7, i32::MAX - 4]);
        let b = i32_to_bytes(&[4, 5, -8, 3]);
        let ha = make_tensor_raw(&a, 3 /* Int32 */, &[4]);
        let hb = make_tensor_raw(&b, 3 /* Int32 */, &[4]);
        assert_ne!(ha, u64::MAX);
        assert_ne!(hb, u64::MAX);

        let hc = molt_gpu_prim_binary(0 /* Add */, ha, hb);
        assert_ne!(hc, u64::MAX);
        assert_eq!(molt_gpu_prim_dtype(hc), 3);
        assert_eq!(molt_gpu_prim_realize(hc), 0);

        let mut out = [0u8; 16];
        assert_eq!(read_data_raw(hc, 3, &mut out), 16);
        assert_eq!(bytes_to_i32(&out), vec![5, 3, -1, i32::MAX - 1]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hb);
        molt_gpu_prim_free(hc);
    }

    #[test]
    fn cpu_ffi_typed_cast_carries_target_dtype_to_schedule() {
        let data = vec![1.25f32, -2.75, 0.0, 7.0];
        let ha = make_tensor(&data, &[4]);
        let hcast = molt_gpu_prim_cast(22 /* Cast */, ha, 3 /* Int32 */);
        assert_ne!(hcast, u64::MAX);

        let (lazy, shape, dtype) =
            with_tensor(hcast, |t| (t.lazy.clone(), t.shape.clone(), t.dtype)).expect("tensor");
        assert_eq!(dtype, DType::Int32);
        assert_eq!(lazy.dtype(), DType::Int32);
        assert_eq!(shape, vec![4]);

        let kernels = schedule::schedule(&lazy, &shape);
        assert_eq!(kernels.len(), 1);
        assert_eq!(kernels[0].bufs[0].dtype, DType::Int32);
        assert_eq!(kernels[0].bufs[1].dtype, DType::Float32);
        assert_eq!(kernels[0].ops[0].op(), PrimitiveOp::Cast);
        assert_eq!(kernels[0].ops[0].dst_dtype(), DType::Int32);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hcast);
    }

    #[test]
    fn cpu_ffi_read_data_rejects_realized_non_float32_tensor() {
        let data = vec![1.25f32, -2.75, 0.0, 7.0];
        let ha = make_tensor(&data, &[4]);
        let hcast = molt_gpu_prim_cast(22 /* Cast */, ha, 3 /* Int32 */);
        assert_ne!(hcast, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hcast), 0);

        let mut out = [123.0f32; 4];
        assert_eq!(
            molt_gpu_prim_read_data(hcast, out.as_mut_ptr(), out.len()),
            u64::MAX
        );
        assert_eq!(out, [123.0f32; 4]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hcast);
    }

    #[test]
    fn cpu_ffi_read_data_raw_returns_exact_typed_cast_bytes() {
        let data = vec![1.25f32, -2.75, 0.0, 7.0];
        let ha = make_tensor(&data, &[4]);
        let hcast = molt_gpu_prim_cast(22 /* Cast */, ha, 3 /* Int32 */);
        assert_ne!(hcast, u64::MAX);

        assert_eq!(molt_gpu_prim_dtype(hcast), 3);
        assert_eq!(molt_gpu_prim_nbytes(hcast), 16);
        let mut pre_realize = [0u8; 16];
        assert_eq!(read_data_raw(hcast, 3, &mut pre_realize), 0);
        assert_eq!(molt_gpu_prim_realize(hcast), 0);

        let mut out = [0u8; 16];
        assert_eq!(read_data_raw(hcast, 3, &mut out), 16);
        assert_eq!(bytes_to_i32(&out), vec![1, -2, 0, 7]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hcast);
    }

    #[test]
    fn cpu_ffi_movement_views_preserve_handles_and_shape() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let ha = make_tensor(&data, &[2, 3]);

        let reshaped_shape = [3usize, 2];
        // SAFETY: shape pointer is valid for the duration of the call.
        let hreshape =
            unsafe { molt_gpu_prim_reshape(ha, reshaped_shape.as_ptr(), reshaped_shape.len()) };
        assert_ne!(hreshape, u64::MAX);
        assert_eq!(molt_gpu_prim_numel(hreshape), 6);
        let mut shape_out = [0usize; 2];
        assert_eq!(read_shape(hreshape, &mut shape_out), 2);
        assert_eq!(shape_out, reshaped_shape);

        let order = [1usize, 0];
        // SAFETY: order pointer is valid for the duration of the call.
        let hpermute = unsafe { molt_gpu_prim_permute(ha, order.as_ptr(), order.len()) };
        assert_ne!(hpermute, u64::MAX);
        let hcontiguous = molt_gpu_prim_contiguous(hpermute);
        assert_ne!(hcontiguous, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hcontiguous), 0);
        let mut out = [0.0f32; 6];
        assert_eq!(
            molt_gpu_prim_read_data(hcontiguous, out.as_mut_ptr(), out.len()),
            6
        );
        assert_eq!(out, [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hreshape);
        molt_gpu_prim_free(hpermute);
        molt_gpu_prim_free(hcontiguous);
    }

    #[test]
    fn cpu_ffi_expand_view_broadcasts_without_host_upload() {
        let data = vec![5.0f32, 7.0];
        let ha = make_tensor(&data, &[2, 1]);
        let expanded_shape = [2usize, 3];
        // SAFETY: shape pointer is valid for the duration of the call.
        let hexpand =
            unsafe { molt_gpu_prim_expand(ha, expanded_shape.as_ptr(), expanded_shape.len()) };
        assert_ne!(hexpand, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hexpand), 0);

        let mut out = [0.0f32; 6];
        assert_eq!(
            molt_gpu_prim_read_data(hexpand, out.as_mut_ptr(), out.len()),
            6
        );
        assert_eq!(out, [5.0, 5.0, 5.0, 7.0, 7.0, 7.0]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hexpand);
    }

    #[test]
    fn cpu_ffi_pad_shrink_flip_views_compose_without_host_upload() {
        let data = vec![1.0f32, 2.0, 3.0];
        let ha = make_tensor(&data, &[3]);
        let padding = [1usize, 2];
        // SAFETY: flattened padding pointer is valid for the duration of the call.
        let hpad = unsafe { molt_gpu_prim_pad(ha, padding.as_ptr(), padding.len()) };
        assert_ne!(hpad, u64::MAX);
        let bounds = [1usize, 4];
        // SAFETY: flattened bounds pointer is valid for the duration of the call.
        let hshrink = unsafe { molt_gpu_prim_shrink(hpad, bounds.as_ptr(), bounds.len()) };
        assert_ne!(hshrink, u64::MAX);
        let hflip = molt_gpu_prim_flip(hshrink, 0);
        assert_ne!(hflip, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hflip), 0);

        let mut out = [0.0f32; 3];
        assert_eq!(
            molt_gpu_prim_read_data(hflip, out.as_mut_ptr(), out.len()),
            3
        );
        assert_eq!(out, [3.0, 2.0, 1.0]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hpad);
        molt_gpu_prim_free(hshrink);
        molt_gpu_prim_free(hflip);
    }

    #[test]
    fn cpu_ffi_read_data_raw_rejects_wrong_dtype_and_short_buffer() {
        let data = vec![1.25f32, -2.75, 0.0, 7.0];
        let ha = make_tensor(&data, &[4]);
        let hcast = molt_gpu_prim_cast(22 /* Cast */, ha, 3 /* Int32 */);
        assert_ne!(hcast, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hcast), 0);

        let mut out = [0xa5u8; 16];
        assert_eq!(read_data_raw(hcast, 11 /* Float32 */, &mut out), u64::MAX);
        assert_eq!(out, [0xa5u8; 16]);
        assert_eq!(
            read_data_raw_len(hcast, 3 /* Int32 */, &mut out, 15),
            u64::MAX
        );
        assert_eq!(out, [0xa5u8; 16]);
        assert_eq!(read_data_raw(hcast, 99 /* invalid */, &mut out), u64::MAX);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hcast);
    }

    #[test]
    fn cpu_ffi_untyped_unary_rejects_cast_and_bitcast() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0];
        let ha = make_tensor(&data, &[4]);

        assert_eq!(molt_gpu_prim_unary(22 /* Cast */, ha), u64::MAX);
        assert_eq!(molt_gpu_prim_unary(23 /* Bitcast */, ha), u64::MAX);

        molt_gpu_prim_free(ha);
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
        assert_eq!(
            out, expected,
            "CPU realize a+b must equal a[i]+b[i], not zeros"
        );

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
    fn cpu_ffi_reduce_preserves_keepdim_shape_for_runtime_broadcast() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let ha = make_tensor(&data, &[2, 3]);
        let hred = molt_gpu_prim_reduce(24 /* ReduceSum */, ha, 1);
        assert_ne!(hred, u64::MAX);

        let mut shape = [usize::MAX; 2];
        assert_eq!(read_shape(hred, &mut shape), 2);
        assert_eq!(shape, [2, 1]);

        let expanded_shape = [2usize, 3];
        // SAFETY: shape pointer is valid for the duration of the call.
        let hexpand =
            unsafe { molt_gpu_prim_expand(hred, expanded_shape.as_ptr(), expanded_shape.len()) };
        assert_ne!(hexpand, u64::MAX);
        assert_eq!(molt_gpu_prim_realize(hexpand), 0);

        let mut out = [0.0f32; 6];
        assert_eq!(
            molt_gpu_prim_read_data(hexpand, out.as_mut_ptr(), out.len()),
            6
        );
        assert_eq!(out, [6.0, 6.0, 6.0, 15.0, 15.0, 15.0]);

        molt_gpu_prim_free(ha);
        molt_gpu_prim_free(hred);
        molt_gpu_prim_free(hexpand);
    }

    #[test]
    fn cpu_ffi_ternary_where_selects_values_and_rejects_invalid_contracts() {
        let shape = [3usize];
        let cond = make_tensor_raw(&[1u8, 0, 1], 0, &shape);
        let true_values = make_tensor(&[10.0, 20.0, 30.0], &shape);
        let false_values = make_tensor(&[1.0, 2.0, 3.0], &shape);
        let result = molt_gpu_prim_ternary(21 /* Where */, cond, true_values, false_values);
        assert_ne!(result, u64::MAX);
        assert_eq!(realize_cpu_values(result), vec![10.0, 2.0, 30.0]);

        assert_eq!(
            molt_gpu_prim_ternary(
                0, /* Add is not ternary */
                cond,
                true_values,
                false_values
            ),
            u64::MAX
        );

        let mismatched_shape = make_tensor(&[4.0, 5.0], &[2]);
        assert_eq!(
            molt_gpu_prim_ternary(21, cond, true_values, mismatched_shape),
            u64::MAX
        );

        let mismatched_dtype = make_tensor_raw(&i32_to_bytes(&[1, 2, 3]), 3, &shape);
        assert_eq!(
            molt_gpu_prim_ternary(21, cond, true_values, mismatched_dtype),
            u64::MAX
        );

        let non_bool_cond = make_tensor(&[1.0, 0.0, 1.0], &shape);
        assert_eq!(
            molt_gpu_prim_ternary(21, non_bool_cond, true_values, false_values),
            u64::MAX
        );

        molt_gpu_prim_free(cond);
        molt_gpu_prim_free(true_values);
        molt_gpu_prim_free(false_values);
        molt_gpu_prim_free(result);
        molt_gpu_prim_free(mismatched_shape);
        molt_gpu_prim_free(mismatched_dtype);
        molt_gpu_prim_free(non_bool_cond);
    }

    #[test]
    fn cpu_ffi_reduce_all_owns_public_scalar_shape_for_rank2_and_rank3() {
        let rank2 = vec![1.0f32, 2.0, 3.0, 4.0];
        let h2 = make_tensor(&rank2, &[2, 2]);
        let r2 = molt_gpu_prim_reduce_all(24 /* ReduceSum */, h2);
        assert_ne!(r2, u64::MAX);
        let mut shape2 = [usize::MAX; 1];
        assert_eq!(read_shape(r2, &mut shape2), 1);
        assert_eq!(shape2, [1]);
        assert_eq!(realize_cpu_values(r2), vec![10.0]);

        let rank3 = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let h3 = make_tensor(&rank3, &[1, 2, 3]);
        let r3 = molt_gpu_prim_reduce_all(25 /* ReduceMax */, h3);
        assert_ne!(r3, u64::MAX);
        let mut shape3 = [usize::MAX; 1];
        assert_eq!(read_shape(r3, &mut shape3), 1);
        assert_eq!(shape3, [1]);
        assert_eq!(realize_cpu_values(r3), vec![6.0]);

        assert_eq!(
            molt_gpu_prim_reduce_all(0 /* Add is not a reduce op */, h2),
            u64::MAX
        );
        assert_eq!(
            molt_gpu_prim_reduce(0 /* Add is not a reduce op */, h2, 0),
            u64::MAX
        );

        molt_gpu_prim_free(h2);
        molt_gpu_prim_free(r2);
        molt_gpu_prim_free(h3);
        molt_gpu_prim_free(r3);
    }

    #[test]
    fn cpu_realize_shrink_movement_reads_full_source_storage() {
        // The shrunk logical view has 3 elements, but its physical indexes are
        // 1, 2, and 3 into the 4-element source storage. The bridge must pass
        // all 4 source elements to the kernel input slot; sizing the slot by the
        // view length truncates index 3 and historically produced a zero.
        let data = vec![10.0f32, 20.0, 30.0, 40.0];
        let ha = make_tensor(&data, &[4]);
        let (src, dtype) = with_tensor(ha, |t| (t.lazy.clone(), t.dtype)).expect("source tensor");

        let shrunk = Arc::new(LazyOp::Movement {
            src,
            st: ShapeTracker::contiguous(&[4]).shrink(&[(1, 4)]),
        });
        let neg = Arc::new(LazyOp::Unary {
            op: PrimitiveOp::Neg,
            src: shrunk,
        });
        let shape = neg.shape();
        let mut kernels = schedule::schedule(&neg, &shape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);

        let out = bytes_to_f32(&execute_fused_pipeline_cpu(
            &neg,
            &fused,
            shape.iter().product(),
            dtype,
        ));
        assert_eq!(out, vec![-20.0, -30.0, -40.0]);

        molt_gpu_prim_free(ha);
    }

    #[test]
    fn cpu_realize_same_storage_distinct_view_routes_distinct_slots() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0];
        let ha = make_tensor(&data, &[4]);
        let (src, dtype) = with_tensor(ha, |t| (t.lazy.clone(), t.dtype)).expect("source tensor");

        let flipped = Arc::new(LazyOp::Movement {
            src: Arc::clone(&src),
            st: ShapeTracker::contiguous(&[4]).flip(0),
        });
        let add = Arc::new(LazyOp::Binary {
            op: PrimitiveOp::Add,
            lhs: Arc::clone(&src),
            rhs: flipped,
        });
        let shape = add.shape();
        let mut kernels = schedule::schedule(&add, &shape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);

        assert_eq!(fused.len(), 1, "same-storage add should stay one kernel");
        let kernel = &fused[0];
        assert_eq!(kernel.bufs.len(), 3);
        assert_eq!(kernel.bufs[1].buf_id, kernel.bufs[2].buf_id);
        assert_ne!(kernel.bufs[1].st, kernel.bufs[2].st);

        let out = bytes_to_f32(&execute_fused_pipeline_cpu(
            &add,
            &fused,
            shape.iter().product(),
            dtype,
        ));
        assert_eq!(
            out,
            vec![5.0, 5.0, 5.0, 5.0],
            "add(x, flip(x)) must route one storage id through two distinct views"
        );

        molt_gpu_prim_free(ha);
    }

    #[test]
    #[should_panic(expected = "realized bytes for leaf buf_id")]
    fn cpu_realize_missing_leaf_storage_panics_instead_of_zero_fallback() {
        let missing = Arc::new(LazyOp::Buffer {
            buf: DeviceBufferRef {
                id: usize::MAX - 1,
                size_bytes: 16,
            },
            st: ShapeTracker::contiguous(&[4]),
            dtype: DType::Float32,
        });
        let neg = Arc::new(LazyOp::Unary {
            op: PrimitiveOp::Neg,
            src: missing,
        });
        let shape = neg.shape();
        let mut kernels = schedule::schedule(&neg, &shape);
        schedule::specialize_shapes(&mut kernels);
        let fused = fuse::fuse(kernels);

        let _ = execute_fused_pipeline_cpu(&neg, &fused, shape.iter().product(), DType::Float32);
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
