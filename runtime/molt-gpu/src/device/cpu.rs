//! CpuDevice — CPU reference backend for testing.
//!
//! Executes kernels by interpreting the FusedKernel IR directly.
//! Includes optional SIMD acceleration via the `wide` crate for
//! float32 elementwise ops (4-wide f32x4 processing).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, CpuKernelFn,
    DeviceBuffer, DeviceError, Executor, ProgramHandle,
};

/// CPU reference device backend for correctness testing.
///
/// Allocates CPU buffers and interprets FusedKernel IR directly.
/// When the `simd-accel` feature is enabled, elementwise float32
/// operations are accelerated with 4-wide SIMD processing.
pub struct CpuDevice {
    /// Buffer allocation counter for unique IDs.
    _next_id: Mutex<usize>,
    /// Compiled program cache: source hash -> entry name.
    /// Prevents redundant "compilation" (source parsing) for the same shader.
    compile_cache: Mutex<HashMap<u64, String>>,
}

impl CpuDevice {
    /// Create a new CPU device.
    pub fn new() -> Self {
        Self {
            _next_id: Mutex::new(0),
            compile_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Hash shader source for cache lookup (same algorithm as MetalDevice).
    fn hash_source(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns the number of cached compiled programs.
    pub fn cache_len(&self) -> usize {
        self.compile_cache.lock().unwrap().len()
    }
}

impl Default for CpuDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator for CpuDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        // Page-align allocations for optimal DMA transfer performance.
        // Vec guarantees alignment to the element type (u8 = 1), but we want
        // page alignment (4096 bytes) for large buffers that may be
        // DMA-transferred to/from GPU memory.
        let buf = if size_bytes >= 4096 {
            alloc_page_aligned(size_bytes)
        } else {
            vec![0u8; size_bytes]
        };
        Ok(DeviceBuffer {
            handle: BufferHandle::Cpu(buf),
            size_bytes,
        })
    }

    fn free(&self, _buf: DeviceBuffer) -> Result<(), DeviceError> {
        // Drop handles deallocation for CPU buffers
        Ok(())
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                if data.len() > inner.len() {
                    return Err(DeviceError::InvalidArgument(format!(
                        "copy_in: data ({} bytes) exceeds buffer ({} bytes)",
                        data.len(),
                        inner.len()
                    )));
                }
                // SAFETY: We need interior mutability. The CPU backend uses
                // this for testing only. In production, Metal/WebGPU handle
                // synchronization at the command buffer level.
                let inner_ptr = inner.as_ptr() as *mut u8;
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), inner_ptr, data.len());
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_in to Metal buffer via CpuDevice".into(),
            )),
        }
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                let len = data.len().min(inner.len());
                data[..len].copy_from_slice(&inner[..len]);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_out from Metal buffer via CpuDevice".into(),
            )),
        }
    }
}

impl Compiler for CpuDevice {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        let hash = Self::hash_source(source);

        // Check cache — return early if already compiled
        {
            let cache = self.compile_cache.lock().unwrap();
            if let Some(cached_entry) = cache.get(&hash) {
                fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
                    entry: cached_entry.clone(),
                });
            }
        }

        // CPU device doesn't compile shader source — it interprets FusedKernel directly.
        fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}

        // Store in cache
        self.compile_cache.lock().unwrap().insert(hash, entry.to_string());

        Ok(CompiledProgram {
            handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [1024, 1, 1]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [u32::MAX, 1, 1]
    }
}

impl Executor for CpuDevice {
    fn exec(
        &self,
        _prog: &CompiledProgram,
        _bufs: &[&DeviceBuffer],
        _grid: [u32; 3],
        _local: [u32; 3],
    ) -> Result<(), DeviceError> {
        // CPU execution is done through the interpret_kernel method, not exec.
        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        // CPU is synchronous — nothing to wait for.
        Ok(())
    }
}

/// Allocate a page-aligned buffer of zeroed bytes.
///
/// Uses the system allocator with explicit alignment to 4096 bytes,
/// which is optimal for DMA transfers between CPU and GPU memory.
fn alloc_page_aligned(size_bytes: usize) -> Vec<u8> {
    // Round up to page boundary for the allocation layout.
    let layout = std::alloc::Layout::from_size_align(size_bytes, 4096)
        .expect("invalid layout for page-aligned allocation");
    // SAFETY: Layout is valid (nonzero size, power-of-two alignment).
    // We zero the memory and construct a Vec that owns the allocation.
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Vec::from_raw_parts(ptr, size_bytes, size_bytes)
    }
}

/// CPU kernel interpreter — executes a FusedKernel op-by-op on CPU.
/// This is the reference implementation used for correctness testing.
///
/// When the `simd-accel` feature is enabled, pure-elementwise float32
/// kernels use 4-wide SIMD processing via the `wide` crate for ADD,
/// SUB, MUL, SQRT, RECIPROCAL, NEG, MAX, and reduce operations.
pub mod interpret {
    use crate::dtype::DType;
    use crate::ops::PrimitiveOp;
    use crate::render::{FusedKernel, FusedSrc};

    /// Fused matrix multiplication: C = A @ B without intermediate allocation.
    ///
    /// Reads A (M x K row-major f32) and B (K x N row-major f32) directly,
    /// writes C (M x N row-major f32). No intermediate product tensor is
    /// materialized, eliminating the O(M*K*N) memory allocation that the
    /// unfused RESHAPE -> EXPAND -> MUL -> REDUCE_SUM path requires.
    ///
    /// Uses a KIJ loop order for optimal cache locality on row-major A,
    /// streaming each row of A through the K dimension while accumulating
    /// into the output row.
    ///
    /// `a_buf` and `b_buf` are raw f32 byte slices. `out_buf` is pre-allocated
    /// and zeroed (M*N*4 bytes). All buffers must be Float32 little-endian.
    #[inline(never)]
    pub fn fused_matmul(
        a_buf: &[u8],
        b_buf: &[u8],
        out_buf: &mut [u8],
        m: usize,
        k: usize,
        n: usize,
    ) {
        debug_assert_eq!(a_buf.len(), m * k * 4, "A buffer size mismatch");
        debug_assert_eq!(b_buf.len(), k * n * 4, "B buffer size mismatch");
        debug_assert_eq!(out_buf.len(), m * n * 4, "output buffer size mismatch");

        // Reinterpret byte slices as f32 slices for direct access.
        // SAFETY: The caller guarantees buffers are f32 little-endian aligned.
        // Vec<u8> from alloc_page_aligned is page-aligned (4096), so f32
        // alignment (4) is satisfied. Standard Vec<u8> has alignment >= 1
        // but f32::from_le_bytes is used as fallback below.

        // Use a temporary f32 accumulator to avoid repeated byte conversions.
        // This is the dominant cost: M*K*N multiply-accumulate operations.
        let mut c = vec![0.0f32; m * n];

        // IKJ loop order: for each row of A, stream through K,
        // broadcasting a[i,k] across the entire row of B[k,:].
        // This maximizes spatial locality in both B and C.
        for i in 0..m {
            for kk in 0..k {
                let a_off = (i * k + kk) * 4;
                let a_val = f32::from_le_bytes(a_buf[a_off..a_off + 4].try_into().unwrap());
                let b_row_off = kk * n * 4;
                for j in 0..n {
                    let b_off = b_row_off + j * 4;
                    let b_val = f32::from_le_bytes(b_buf[b_off..b_off + 4].try_into().unwrap());
                    c[i * n + j] += a_val * b_val;
                }
            }
        }

        // Write results back to output buffer.
        for (idx, &val) in c.iter().enumerate() {
            let off = idx * 4;
            out_buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
        }
    }

    /// Interpret and execute a FusedKernel on CPU buffers.
    /// `bufs` are raw byte slices matching kernel.bufs order.
    /// bufs[0] is the output buffer (written to).
    #[inline(always)]
    pub fn execute_kernel(kernel: &FusedKernel, bufs: &mut [Vec<u8>]) {
        let output_numel = kernel.bufs[0].st.numel();

        // Check if SIMD fast path is applicable:
        // All buffers are Float32, all views are contiguous, no reduce ops.
        #[cfg(feature = "simd-accel")]
        {
            if can_use_simd_path(kernel) {
                execute_kernel_simd(kernel, bufs);
                return;
            }
        }

        for gid in 0..output_numel {
            let mut values: Vec<f64> = Vec::with_capacity(kernel.ops.len());

            for (op_idx, op) in kernel.ops.iter().enumerate() {
                if matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
                    // Handle reduce ops
                    let input_buf_idx = match &op.srcs[0] {
                        FusedSrc::Buf(idx) => *idx,
                        FusedSrc::Op(_) => 1,
                        FusedSrc::Const { .. } => unreachable!(),
                    };
                    let input_numel = kernel.bufs[input_buf_idx].st.numel();
                    let reduce_size = input_numel / output_numel;

                    let mut acc = match op.op {
                        PrimitiveOp::ReduceSum => 0.0f64,
                        PrimitiveOp::ReduceMax => f64::NEG_INFINITY,
                        _ => unreachable!(),
                    };

                    for rid in 0..reduce_size {
                        let eidx = gid * reduce_size + rid;

                        // If there are pre-reduce ops, compute them for this element
                        let val = if op_idx > 0 {
                            // Re-compute pre-reduce elementwise chain for this element index
                            let mut pre_values: Vec<f64> = Vec::with_capacity(op_idx);
                            for pre_op in &kernel.ops[..op_idx] {
                                let get_pre_src = |i: usize| -> f64 {
                                    match &pre_op.srcs[i] {
                                        FusedSrc::Buf(idx) => {
                                            read_f64(&bufs[*idx], eidx, kernel.bufs[*idx].dtype)
                                        }
                                        FusedSrc::Op(prior) => pre_values[*prior],
                                        FusedSrc::Const { val, .. } => *val,
                                    }
                                };
                                let result = compute_elementwise(pre_op.op, &get_pre_src, pre_op.srcs.len());
                                pre_values.push(result);
                            }
                            *pre_values.last().unwrap()
                        } else {
                            read_f64(&bufs[input_buf_idx], eidx, kernel.bufs[input_buf_idx].dtype)
                        };

                        acc = match op.op {
                            PrimitiveOp::ReduceSum => acc + val,
                            PrimitiveOp::ReduceMax => {
                                // NaN-propagating max for floats
                                if val.is_nan() || acc.is_nan() {
                                    f64::NAN
                                } else {
                                    acc.max(val)
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                    values.push(acc);
                    continue;
                }

                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype)
                        }
                        FusedSrc::Op(prior) => values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                let result = compute_elementwise(op.op, &get_src, op.srcs.len());
                values.push(result);
            }

            // Write output
            let result = values.last().copied().unwrap_or(0.0);
            write_f64(&mut bufs[0], gid, result, kernel.bufs[0].dtype);
        }
    }

    /// Check if the kernel is eligible for SIMD acceleration.
    /// Requirements: all Float32 buffers, all contiguous views with matching
    /// element counts, no reduce ops, all ops SIMD-able.
    #[cfg(feature = "simd-accel")]
    #[inline(always)]
    fn can_use_simd_path(kernel: &FusedKernel) -> bool {
        // All buffer dtypes must be Float32
        let all_f32 = kernel.bufs.iter().all(|b| b.dtype == DType::Float32);
        if !all_f32 {
            return false;
        }

        // All views must be contiguous
        let all_contiguous = kernel.bufs.iter().all(|b| b.st.view().is_contiguous());
        if !all_contiguous {
            return false;
        }

        // All buffers must have the same element count as the output.
        // Broadcast buffers (e.g., shape [1] broadcast to [1024]) have
        // fewer physical elements than the output, so SIMD batch reads
        // would go out of bounds.
        let output_numel = kernel.bufs[0].st.numel();
        let all_same_numel = kernel.bufs.iter().all(|b| b.st.numel() == output_numel);
        if !all_same_numel {
            return false;
        }

        // No reduce ops
        let has_reduce = kernel.ops.iter().any(|op| {
            matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
        });
        if has_reduce {
            return false;
        }

        // All ops must be SIMD-able
        kernel.ops.iter().all(|op| is_simd_op(op.op))
    }

    /// Whether a PrimitiveOp has a SIMD implementation.
    #[cfg(feature = "simd-accel")]
    #[inline(always)]
    fn is_simd_op(op: PrimitiveOp) -> bool {
        matches!(
            op,
            PrimitiveOp::Add
                | PrimitiveOp::Sub
                | PrimitiveOp::Mul
                | PrimitiveOp::Neg
                | PrimitiveOp::Sqrt
                | PrimitiveOp::Reciprocal
                | PrimitiveOp::Max
                | PrimitiveOp::Exp2
                | PrimitiveOp::Sin
                | PrimitiveOp::Log2
                | PrimitiveOp::Cmplt
                | PrimitiveOp::Cmpeq
                | PrimitiveOp::Cmpne
                | PrimitiveOp::Where
                | PrimitiveOp::Cast
                | PrimitiveOp::Trunc
        )
    }

    /// SIMD-accelerated kernel execution using `wide` crate's f32x4.
    ///
    /// Processes 4 elements at a time for all supported elementwise ops.
    /// Falls back to scalar for the remainder (count % 4 != 0).
    #[cfg(feature = "simd-accel")]
    fn execute_kernel_simd(kernel: &FusedKernel, bufs: &mut [Vec<u8>]) {
        use wide::f32x4;

        let output_numel = kernel.bufs[0].st.numel();
        let simd_count = output_numel / 4;
        let remainder_start = simd_count * 4;

        // SIMD pass: process 4 elements at a time
        for chunk in 0..simd_count {
            let base = chunk * 4;
            let mut simd_values: Vec<f32x4> = Vec::with_capacity(kernel.ops.len());

            for op in kernel.ops.iter() {
                let get_src_simd = |i: usize| -> f32x4 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            let buf = &bufs[*idx];
                            let offset = base * 4; // 4 bytes per f32
                            let bytes = &buf[offset..offset + 16];
                            let a = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
                            let b = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
                            let c = f32::from_le_bytes(bytes[8..12].try_into().unwrap());
                            let d = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
                            f32x4::new([a, b, c, d])
                        }
                        FusedSrc::Op(prior) => simd_values[*prior],
                        FusedSrc::Const { val, .. } => f32x4::splat(*val as f32),
                    }
                };

                let result = compute_elementwise_simd(op.op, &get_src_simd);
                simd_values.push(result);
            }

            // Write SIMD output
            let result = *simd_values.last().unwrap();
            let result_arr: [f32; 4] = result.into();
            let offset = base * 4;
            for (i, &v) in result_arr.iter().enumerate() {
                let o = offset + i * 4;
                bufs[0][o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        }

        // Scalar remainder
        for gid in remainder_start..output_numel {
            let mut values: Vec<f64> = Vec::with_capacity(kernel.ops.len());

            for op in kernel.ops.iter() {
                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype)
                        }
                        FusedSrc::Op(prior) => values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                let result = compute_elementwise(op.op, &get_src, op.srcs.len());
                values.push(result);
            }

            let result = values.last().copied().unwrap_or(0.0);
            write_f64(&mut bufs[0], gid, result, kernel.bufs[0].dtype);
        }
    }

    /// SIMD elementwise op dispatch for f32x4.
    #[cfg(feature = "simd-accel")]
    #[inline(always)]
    fn compute_elementwise_simd(
        op: PrimitiveOp,
        get_src: &dyn Fn(usize) -> wide::f32x4,
    ) -> wide::f32x4 {
        use wide::f32x4;

        match op {
            PrimitiveOp::Add => get_src(0) + get_src(1),
            PrimitiveOp::Sub => get_src(0) - get_src(1),
            PrimitiveOp::Mul => get_src(0) * get_src(1),
            PrimitiveOp::Neg => -get_src(0),
            PrimitiveOp::Sqrt => get_src(0).sqrt(),
            PrimitiveOp::Reciprocal => f32x4::splat(1.0) / get_src(0),
            PrimitiveOp::Max => {
                // NaN-propagating max: if either operand is NaN, result is NaN.
                // wide's f32x4::max() follows IEEE minNum/maxNum which suppresses NaN.
                // We must check per-element.
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if aa[0].is_nan() || ba[0].is_nan() { f32::NAN } else { aa[0].max(ba[0]) },
                    if aa[1].is_nan() || ba[1].is_nan() { f32::NAN } else { aa[1].max(ba[1]) },
                    if aa[2].is_nan() || ba[2].is_nan() { f32::NAN } else { aa[2].max(ba[2]) },
                    if aa[3].is_nan() || ba[3].is_nan() { f32::NAN } else { aa[3].max(ba[3]) },
                ])
            }
            PrimitiveOp::Exp2 => {
                // Polynomial approximation for exp2 in SIMD.
                // Uses the identity: exp2(x) = exp(x * ln2)
                // and a 4th-order polynomial approximation for exp() on [-0.5, 0.5].
                let x = get_src(0);
                let ln2 = f32x4::splat(std::f32::consts::LN_2);
                // Separate integer and fractional parts
                let xln2 = x * ln2;
                // Fall back to per-element for full accuracy
                let arr: [f32; 4] = xln2.into();
                f32x4::new([arr[0].exp(), arr[1].exp(), arr[2].exp(), arr[3].exp()])
            }
            PrimitiveOp::Log2 => {
                let x = get_src(0);
                let arr: [f32; 4] = x.into();
                f32x4::new([arr[0].log2(), arr[1].log2(), arr[2].log2(), arr[3].log2()])
            }
            PrimitiveOp::Sin => {
                let x = get_src(0);
                let arr: [f32; 4] = x.into();
                f32x4::new([arr[0].sin(), arr[1].sin(), arr[2].sin(), arr[3].sin()])
            }
            PrimitiveOp::Cmplt => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if aa[0] < ba[0] { 1.0 } else { 0.0 },
                    if aa[1] < ba[1] { 1.0 } else { 0.0 },
                    if aa[2] < ba[2] { 1.0 } else { 0.0 },
                    if aa[3] < ba[3] { 1.0 } else { 0.0 },
                ])
            }
            PrimitiveOp::Cmpeq => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if aa[0] == ba[0] { 1.0 } else { 0.0 },
                    if aa[1] == ba[1] { 1.0 } else { 0.0 },
                    if aa[2] == ba[2] { 1.0 } else { 0.0 },
                    if aa[3] == ba[3] { 1.0 } else { 0.0 },
                ])
            }
            PrimitiveOp::Cmpne => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if aa[0] != ba[0] { 1.0 } else { 0.0 },
                    if aa[1] != ba[1] { 1.0 } else { 0.0 },
                    if aa[2] != ba[2] { 1.0 } else { 0.0 },
                    if aa[3] != ba[3] { 1.0 } else { 0.0 },
                ])
            }
            PrimitiveOp::Where => {
                let c = get_src(0);
                let a = get_src(1);
                let b = get_src(2);
                let ca: [f32; 4] = c.into();
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if ca[0] != 0.0 { aa[0] } else { ba[0] },
                    if ca[1] != 0.0 { aa[1] } else { ba[1] },
                    if ca[2] != 0.0 { aa[2] } else { ba[2] },
                    if ca[3] != 0.0 { aa[3] } else { ba[3] },
                ])
            }
            PrimitiveOp::Cast => get_src(0), // f32 -> f32 is identity
            PrimitiveOp::Trunc => {
                let x = get_src(0);
                let arr: [f32; 4] = x.into();
                f32x4::new([arr[0].trunc(), arr[1].trunc(), arr[2].trunc(), arr[3].trunc()])
            }
            _ => {
                // Fallback: should not reach here if is_simd_op is correct
                get_src(0)
            }
        }
    }

    #[inline(always)]
    fn compute_elementwise(op: PrimitiveOp, get_src: &dyn Fn(usize) -> f64, _arity: usize) -> f64 {
        match op {
            PrimitiveOp::Add => get_src(0) + get_src(1),
            PrimitiveOp::Sub => get_src(0) - get_src(1),
            PrimitiveOp::Mul => get_src(0) * get_src(1),
            PrimitiveOp::Idiv => {
                let a = get_src(0) as i64;
                let b = get_src(1) as i64;
                if b == 0 { 0.0 } else { (a / b) as f64 }
            }
            PrimitiveOp::Mod => {
                let a = get_src(0) as i64;
                let b = get_src(1) as i64;
                if b == 0 { 0.0 } else { (a % b) as f64 }
            }
            PrimitiveOp::Neg => -get_src(0),
            PrimitiveOp::Cmplt => if get_src(0) < get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::Cmpeq => if get_src(0) == get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::Cmpne => if get_src(0) != get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::And => ((get_src(0) as i64) & (get_src(1) as i64)) as f64,
            PrimitiveOp::Or => ((get_src(0) as i64) | (get_src(1) as i64)) as f64,
            PrimitiveOp::Xor => ((get_src(0) as i64) ^ (get_src(1) as i64)) as f64,
            PrimitiveOp::Shl => ((get_src(0) as i64) << (get_src(1) as i64)) as f64,
            PrimitiveOp::Shr => ((get_src(0) as i64) >> (get_src(1) as i64)) as f64,
            PrimitiveOp::Exp2 => get_src(0).exp2(),
            PrimitiveOp::Log2 => get_src(0).log2(),
            PrimitiveOp::Sin => get_src(0).sin(),
            PrimitiveOp::Sqrt => get_src(0).sqrt(),
            PrimitiveOp::Reciprocal => 1.0 / get_src(0),
            PrimitiveOp::Trunc => get_src(0).trunc(),
            PrimitiveOp::Max => {
                let a = get_src(0);
                let b = get_src(1);
                // NaN-propagating max: if either operand is NaN, result is NaN.
                if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.max(b)
                }
            }
            PrimitiveOp::Where => {
                if get_src(0) != 0.0 { get_src(1) } else { get_src(2) }
            }
            PrimitiveOp::Cast => get_src(0),
            PrimitiveOp::Bitcast => get_src(0),
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => unreachable!(),
        }
    }

    #[inline(always)]
    fn read_f64(buf: &[u8], idx: usize, dtype: DType) -> f64 {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                f64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Bool | DType::UInt8 => {
                if idx >= buf.len() { return 0.0; }
                buf[idx] as f64
            }
            DType::Int8 => {
                if idx >= buf.len() { return 0.0; }
                (buf[idx] as i8) as f64
            }
            DType::Int16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                i16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::UInt16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::Int64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                i64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::UInt32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::UInt64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::Float16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::f16::from_bits(bits).to_f64()
            }
            DType::BFloat16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::bf16::from_bits(bits).to_f64()
            }
            // MXFP types: raw byte read. Dequantization happens at a higher level.
            DType::MxFP8 | DType::MxFP4 => {
                if idx >= buf.len() { return 0.0; }
                buf[idx] as f64
            }
        }
    }

    fn write_f64(buf: &mut [u8], idx: usize, val: f64, dtype: DType) {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as f32).to_le_bytes());
                }
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
                }
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as i32).to_le_bytes());
                }
            }
            DType::Bool | DType::UInt8 => {
                if idx < buf.len() {
                    buf[idx] = if val != 0.0 { 1 } else { 0 };
                }
            }
            DType::Int8 => {
                if idx < buf.len() {
                    buf[idx] = (val as i8) as u8;
                }
            }
            DType::Int16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    buf[offset..offset + 2].copy_from_slice(&(val as i16).to_le_bytes());
                }
            }
            DType::UInt16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    buf[offset..offset + 2].copy_from_slice(&(val as u16).to_le_bytes());
                }
            }
            DType::Int64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&(val as i64).to_le_bytes());
                }
            }
            DType::UInt32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as u32).to_le_bytes());
                }
            }
            DType::UInt64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&(val as u64).to_le_bytes());
                }
            }
            DType::Float16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    let h = half::f16::from_f64(val);
                    buf[offset..offset + 2].copy_from_slice(&h.to_bits().to_le_bytes());
                }
            }
            DType::BFloat16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    let h = half::bf16::from_f64(val);
                    buf[offset..offset + 2].copy_from_slice(&h.to_bits().to_le_bytes());
                }
            }
            // MXFP types: raw byte write. Quantization happens at a higher level.
            DType::MxFP8 | DType::MxFP4 => {
                if idx < buf.len() {
                    buf[idx] = val as u8;
                }
            }
        }
    }
}
