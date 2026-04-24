//! CpuDevice — CPU reference backend for testing.
//!
//! Executes kernels by interpreting the FusedKernel IR directly.
//! Includes optional SIMD acceleration via the `wide` crate for
//! float32 elementwise ops (4-wide f32x4 processing).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::device::{
    Allocator, BufferHandle, CompiledProgram, Compiler, CpuKernelFn, DeviceBuffer, DeviceError,
    Executor, ProgramHandle,
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
        // All allocations are at least 16-byte aligned for SIMD (f32x4 loads).
        // Large buffers (>= 4096 bytes) are page-aligned (4096 bytes) for
        // optimal DMA transfer performance between CPU and GPU memory.
        let buf = if size_bytes >= 4096 {
            alloc_page_aligned(size_bytes)
        } else {
            alloc_simd_aligned(size_bytes)
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
        self.compile_cache
            .lock()
            .unwrap()
            .insert(hash, entry.to_string());

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

/// Allocate a 16-byte-aligned buffer of zeroed bytes.
///
/// Uses the system allocator with explicit 16-byte alignment for SIMD
/// operations (f32x4 loads require 16-byte alignment for optimal
/// performance on all architectures).
fn alloc_simd_aligned(size_bytes: usize) -> Vec<u8> {
    if size_bytes == 0 {
        return Vec::new();
    }
    let layout = std::alloc::Layout::from_size_align(size_bytes, 16)
        .expect("invalid layout for SIMD-aligned allocation");
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Vec::from_raw_parts(ptr, size_bytes, size_bytes)
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

        // Cast byte buffers to f32 slices once, eliminating per-element
        // from_le_bytes in the hot inner loop. On little-endian platforms
        // (x86_64, aarch64), this is a zero-cost reinterpret.
        let a = as_f32_slice(a_buf);
        let b = as_f32_slice(b_buf);
        let mut c = vec![0.0f32; m * n];

        // IKJ loop order: for each row of A, stream through K,
        // broadcasting a[i,k] across the entire row of B[k,:].
        // This maximizes spatial locality in both B and C.
        for i in 0..m {
            for kk in 0..k {
                let a_val = a[i * k + kk];
                let b_row = kk * n;
                for j in 0..n {
                    c[i * n + j] = a_val.mul_add(b[b_row + j], c[i * n + j]);
                }
            }
        }

        // Write results back to output buffer via direct f32 slice.
        let out_f32 = as_f32_slice_mut(out_buf);
        out_f32[..m * n].copy_from_slice(&c);
    }

    /// Detect and execute a fused softmax pattern:
    /// REDUCE_MAX -> SUB -> EXP2 -> REDUCE_SUM -> RECIPROCAL -> MUL
    ///
    /// This is numerically equivalent to: softmax(x) = exp2(x - max(x)) / sum(exp2(x - max(x)))
    /// Executes as a single pass per output row, avoiding 6 separate kernel
    /// interpretations and intermediate buffer allocations.
    ///
    /// `input_buf` is the raw f32 input, `output_buf` is pre-allocated.
    /// `n` is the number of output rows, `reduce_size` is elements per row.
    #[inline(never)]
    pub fn fused_softmax(input_buf: &[u8], output_buf: &mut [u8], n: usize, reduce_size: usize) {
        let input = as_f32_slice(input_buf);
        let output = as_f32_slice_mut(output_buf);

        // Pre-allocate exp buffer once, reuse across rows.
        let mut exp_vals = vec![0.0f64; reduce_size];

        for row in 0..n {
            let row_start = row * reduce_size;
            let row_end = row_start + reduce_size;

            if row_end > input.len() {
                break;
            }

            let row_slice = &input[row_start..row_end];

            // Pass 1: find max for numerical stability
            let mut max_val = f64::NEG_INFINITY;
            for &v in row_slice {
                let val = v as f64;
                if val > max_val {
                    max_val = val;
                }
            }

            // Pass 2: compute exp2(x - max) and sum
            let mut sum = 0.0f64;
            for (j, &v) in row_slice.iter().enumerate() {
                let e = (v as f64 - max_val).exp2();
                exp_vals[j] = e;
                sum += e;
            }

            // Pass 3: normalize
            let inv_sum = 1.0 / sum;
            let out_len = output.len();
            let out_row = &mut output[row_start..row_end.min(out_len)];
            for (j, out_v) in out_row.iter_mut().enumerate() {
                *out_v = (exp_vals[j] * inv_sum) as f32;
            }
        }
    }

    /// Detect and execute a fused RMSNorm pattern:
    /// MUL(x,x) -> REDUCE_SUM -> ADD(eps) -> SQRT -> RECIPROCAL -> MUL(x, ...)
    ///
    /// Equivalent to: rmsnorm(x) = x / sqrt(mean(x^2) + eps)
    /// (Note: mean = sum/N, so the caller must account for the division by N
    /// in the eps or post-processing.)
    ///
    /// Executes as a single pass per output row instead of 6 separate kernels.
    ///
    /// `input_buf` is the raw f32 input, `output_buf` is pre-allocated.
    /// `n` is the number of rows, `dim` is elements per row, `eps` is the
    /// normalization epsilon.
    #[inline(never)]
    pub fn fused_rms_norm(input_buf: &[u8], output_buf: &mut [u8], n: usize, dim: usize, eps: f64) {
        let input = as_f32_slice(input_buf);
        let output = as_f32_slice_mut(output_buf);

        for row in 0..n {
            let row_start = row * dim;
            let row_end = row_start + dim;

            if row_end > input.len() {
                break;
            }

            let row_slice = &input[row_start..row_end];

            // Pass 1: compute sum of squares
            let mut sum_sq = 0.0f64;
            for &v in row_slice {
                let val = v as f64;
                sum_sq += val * val;
            }

            // Compute 1/sqrt(mean(x^2) + eps)
            let mean_sq = sum_sq / dim as f64;
            let inv_rms = 1.0 / (mean_sq + eps).sqrt();

            // Pass 2: scale each element
            let out_len = output.len();
            let out_row = &mut output[row_start..row_end.min(out_len)];
            for (j, out_v) in out_row.iter_mut().enumerate() {
                *out_v = (row_slice[j] as f64 * inv_rms) as f32;
            }
        }
    }

    /// Detected matmul pattern metadata.
    /// Captures the dimensions and buffer indices for a pure matmul kernel.
    struct MatmulPattern {
        /// Index of A buffer in kernel.bufs (M x K).
        a_buf_idx: usize,
        /// Index of B buffer in kernel.bufs (K x N).
        b_buf_idx: usize,
        /// Optional bias buffer index in kernel.bufs (M x N or broadcast).
        bias_buf_idx: Option<usize>,
        /// M dimension (rows of A / rows of C).
        m: usize,
        /// K dimension (cols of A / rows of B, the reduce axis).
        k: usize,
        /// N dimension (cols of B / cols of C).
        n: usize,
        /// Element dtype (Float32 or Float64).
        dtype: DType,
    }

    /// Detect if a FusedKernel represents a pure matmul (optionally with bias).
    ///
    /// Matches these patterns:
    ///   Pattern 1: Mul(Buf(a), Buf(b)) -> ReduceSum(Op(0))
    ///   Pattern 2: Mul(Buf(a), Buf(b)) -> ReduceSum(Op(0)) -> Add(Op(1), Buf(bias))
    ///
    /// Requirements:
    /// - All buffers must be contiguous
    /// - All buffers must share the same dtype (Float32 or Float64)
    /// - Output buffer has M*N elements
    /// - Physical A buffer has M*K elements, physical B buffer has K*N elements
    /// - ReduceSum reduces over the K dimension
    fn detect_matmul_pattern(kernel: &FusedKernel, bufs: &[Vec<u8>]) -> Option<MatmulPattern> {
        let n_ops = kernel.ops.len();

        // Must have 2 ops (Mul + ReduceSum) or 3 ops (Mul + ReduceSum + Add for bias)
        if n_ops < 2 || n_ops > 3 {
            return None;
        }

        // Op 0 must be Mul(Buf(a), Buf(b)) where a != b
        let mul_op = &kernel.ops[0];
        if mul_op.op != PrimitiveOp::Mul {
            return None;
        }
        let (a_idx, b_idx) = match (&mul_op.srcs[0], &mul_op.srcs[1]) {
            (FusedSrc::Buf(a), FusedSrc::Buf(b)) if a != b => (*a, *b),
            _ => return None,
        };

        // Op 1 must be ReduceSum(Op(0))
        let reduce_op = &kernel.ops[1];
        if reduce_op.op != PrimitiveOp::ReduceSum {
            return None;
        }
        match &reduce_op.srcs[0] {
            FusedSrc::Op(0) => {}
            _ => return None,
        }

        // Check for optional bias add as op 2
        let bias_buf_idx = if n_ops == 3 {
            let add_op = &kernel.ops[2];
            if add_op.op != PrimitiveOp::Add {
                return None;
            }
            // One source must be Op(1) (the reduce result), other must be Buf
            match (&add_op.srcs[0], &add_op.srcs[1]) {
                (FusedSrc::Op(1), FusedSrc::Buf(bias)) => Some(*bias),
                (FusedSrc::Buf(bias), FusedSrc::Op(1)) => Some(*bias),
                _ => return None,
            }
        } else {
            None
        };

        // All buffers must be contiguous
        for buf in &kernel.bufs {
            if !buf.st.view().is_contiguous() {
                return None;
            }
        }

        // All buffers must share the same dtype, and it must be Float32 or Float64
        let dtype = kernel.bufs[0].dtype;
        if dtype != DType::Float32 && dtype != DType::Float64 {
            return None;
        }
        for buf in &kernel.bufs {
            if buf.dtype != dtype {
                return None;
            }
        }

        // Extract dimensions from buffer shapes and physical sizes.
        //
        // The ShapeTracker numels represent the LOGICAL (broadcast-expanded)
        // element counts: both Mul inputs have M*K*N logical elements.
        // The PHYSICAL buffer sizes are what matters for the fast path:
        //   A physical: M*K elements
        //   B physical: K*N elements
        //   Output physical: M*N elements
        //
        // K = logical_numel / output_numel (the reduce dimension)
        let output_numel = kernel.bufs[0].st.numel();
        let logical_input_numel = kernel.bufs[a_idx].st.numel();

        // Both Mul inputs must have the same logical element count
        if kernel.bufs[b_idx].st.numel() != logical_input_numel {
            return None;
        }

        // K = logical_input_numel / output_numel
        if output_numel == 0 || logical_input_numel % output_numel != 0 {
            return None;
        }
        let k = logical_input_numel / output_numel;
        if k == 0 {
            return None;
        }

        // Get physical element counts from actual buffer byte sizes.
        let elem_size = match dtype {
            DType::Float32 => 4,
            DType::Float64 => 8,
            _ => return None,
        };

        let a_phys_elems = bufs[a_idx].len() / elem_size;
        let b_phys_elems = bufs[b_idx].len() / elem_size;

        // Derive M and N from physical sizes:
        //   a_phys_elems == M * K  =>  M = a_phys_elems / K
        //   b_phys_elems == K * N  =>  N = b_phys_elems / K
        if a_phys_elems % k != 0 || b_phys_elems % k != 0 {
            return None;
        }
        let m = a_phys_elems / k;
        let n = b_phys_elems / k;

        if m * n != output_numel {
            return None;
        }
        if m == 0 || n == 0 {
            return None;
        }

        // If bias is present, it must have M*N elements (or be broadcastable,
        // but we only fast-path exact match).
        if let Some(bi) = bias_buf_idx {
            let bias_numel = kernel.bufs[bi].st.numel();
            if bias_numel != output_numel {
                return None;
            }
        }

        Some(MatmulPattern {
            a_buf_idx: a_idx,
            b_buf_idx: b_idx,
            bias_buf_idx,
            m,
            k,
            n,
            dtype,
        })
    }

    /// Execute a detected matmul pattern directly, bypassing the per-element
    /// interpreter. Uses 32x32 tiled IKJ loop order for cache friendliness.
    ///
    /// C[i,j] = sum_k A[i,k] * B[k,j] (+ bias[i,j] if present)
    #[inline(never)]
    fn execute_matmul_fast(pattern: &MatmulPattern, bufs: &mut [Vec<u8>]) {
        let MatmulPattern {
            a_buf_idx,
            b_buf_idx,
            bias_buf_idx,
            m,
            k,
            n,
            dtype,
        } = *pattern;

        match dtype {
            DType::Float32 => execute_matmul_f32(bufs, a_buf_idx, b_buf_idx, bias_buf_idx, m, k, n),
            DType::Float64 => execute_matmul_f64(bufs, a_buf_idx, b_buf_idx, bias_buf_idx, m, k, n),
            _ => unreachable!("detect_matmul_pattern only matches Float32/Float64"),
        }
    }

    /// Reinterpret a byte slice as an f32 slice. The buffer must be f32-aligned
    /// (4-byte alignment minimum). Our alloc functions guarantee 16-byte or
    /// 4096-byte alignment, so this is always safe for molt-gpu buffers.
    #[inline(always)]
    fn as_f32_slice(buf: &[u8]) -> &[f32] {
        debug_assert_eq!(
            buf.as_ptr() as usize % 4,
            0,
            "buffer not 4-byte aligned for f32 cast"
        );
        // SAFETY: Buffers are allocated with alloc_simd_aligned (16-byte) or
        // alloc_page_aligned (4096-byte), both satisfying f32 alignment (4-byte).
        // Length is always a multiple of 4 for Float32 buffers.
        unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const f32, buf.len() / 4) }
    }

    /// Reinterpret a mutable byte slice as a mutable f32 slice.
    #[inline(always)]
    fn as_f32_slice_mut(buf: &mut [u8]) -> &mut [f32] {
        debug_assert_eq!(
            buf.as_ptr() as usize % 4,
            0,
            "buffer not 4-byte aligned for f32 cast"
        );
        // SAFETY: Same alignment guarantees as as_f32_slice.
        unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut f32, buf.len() / 4) }
    }

    /// Tiled f32 matmul: C = A @ B (+ optional bias).
    /// 32x32 tiles, IKJ loop order for cache locality on row-major data.
    /// Uses direct f32 slice access instead of per-element from_le_bytes.
    #[inline(never)]
    fn execute_matmul_f32(
        bufs: &mut [Vec<u8>],
        a_idx: usize,
        b_idx: usize,
        bias_idx: Option<usize>,
        m: usize,
        k: usize,
        n: usize,
    ) {
        const TILE: usize = 32;

        // Accumulate in a contiguous f32 buffer to avoid byte conversion overhead
        // in the inner loop.
        let mut c = vec![0.0f32; m * n];

        // Cast byte buffers to f32 slices ONCE, eliminating per-element
        // from_le_bytes overhead in the hot inner loop. On little-endian
        // platforms (x86_64, aarch64), this is a zero-cost reinterpret.
        let a = as_f32_slice(&bufs[a_idx]);
        let b = as_f32_slice(&bufs[b_idx]);

        // Tiled IKJ: iterate over tiles of (i, k) in A and (k, j) in B.
        // Within each tile, the IKJ order broadcasts a[i,k] across B's row.
        let mut ii = 0;
        while ii < m {
            let i_end = (ii + TILE).min(m);
            let mut kk = 0;
            while kk < k {
                let k_end = (kk + TILE).min(k);
                let mut jj = 0;
                while jj < n {
                    let j_end = (jj + TILE).min(n);

                    // Micro-kernel: process tile [ii..i_end, kk..k_end] x [kk..k_end, jj..j_end]
                    for i in ii..i_end {
                        for ki in kk..k_end {
                            let a_val = a[i * k + ki];
                            let b_row = ki * n;
                            for j in jj..j_end {
                                c[i * n + j] = a_val.mul_add(b[b_row + j], c[i * n + j]);
                            }
                        }
                    }

                    jj += TILE;
                }
                kk += TILE;
            }
            ii += TILE;
        }

        // Add bias if present
        if let Some(bi) = bias_idx {
            let bias = as_f32_slice(&bufs[bi]);
            for idx in 0..m * n {
                c[idx] += bias[idx];
            }
        }

        // Write results to output buffer via direct f32 slice.
        let out_f32 = as_f32_slice_mut(&mut bufs[0]);
        out_f32[..m * n].copy_from_slice(&c);
    }

    /// Reinterpret a byte slice as an f64 slice.
    #[inline(always)]
    fn as_f64_slice(buf: &[u8]) -> &[f64] {
        debug_assert_eq!(
            buf.as_ptr() as usize % 8,
            0,
            "buffer not 8-byte aligned for f64 cast"
        );
        // SAFETY: Buffers are allocated with >= 16-byte alignment.
        unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const f64, buf.len() / 8) }
    }

    /// Reinterpret a mutable byte slice as a mutable f64 slice.
    #[inline(always)]
    fn as_f64_slice_mut(buf: &mut [u8]) -> &mut [f64] {
        debug_assert_eq!(
            buf.as_ptr() as usize % 8,
            0,
            "buffer not 8-byte aligned for f64 cast"
        );
        // SAFETY: Same alignment guarantees as as_f64_slice.
        unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut f64, buf.len() / 8) }
    }

    /// Tiled f64 matmul: C = A @ B (+ optional bias).
    /// 32x32 tiles, IKJ loop order for cache locality on row-major data.
    /// Uses direct f64 slice access instead of per-element from_le_bytes.
    #[inline(never)]
    fn execute_matmul_f64(
        bufs: &mut [Vec<u8>],
        a_idx: usize,
        b_idx: usize,
        bias_idx: Option<usize>,
        m: usize,
        k: usize,
        n: usize,
    ) {
        const TILE: usize = 32;

        let mut c = vec![0.0f64; m * n];

        let a = as_f64_slice(&bufs[a_idx]);
        let b = as_f64_slice(&bufs[b_idx]);

        let mut ii = 0;
        while ii < m {
            let i_end = (ii + TILE).min(m);
            let mut kk = 0;
            while kk < k {
                let k_end = (kk + TILE).min(k);
                let mut jj = 0;
                while jj < n {
                    let j_end = (jj + TILE).min(n);

                    for i in ii..i_end {
                        for ki in kk..k_end {
                            let a_val = a[i * k + ki];
                            let b_row = ki * n;
                            for j in jj..j_end {
                                c[i * n + j] = a_val.mul_add(b[b_row + j], c[i * n + j]);
                            }
                        }
                    }

                    jj += TILE;
                }
                kk += TILE;
            }
            ii += TILE;
        }

        if let Some(bi) = bias_idx {
            let bias = as_f64_slice(&bufs[bi]);
            for idx in 0..m * n {
                c[idx] += bias[idx];
            }
        }

        let out_f64 = as_f64_slice_mut(&mut bufs[0]);
        out_f64[..m * n].copy_from_slice(&c);
    }

    /// Detect a softmax-like reduction pattern in a fused kernel.
    ///
    /// Matches any kernel that contains both a ReduceMax and a ReduceSum
    /// (the hallmark of softmax: max-stabilize, exponentiate, sum-normalize).
    /// All buffers must be Float32 and contiguous.
    ///
    /// Returns `Some((input_buf_idx, reduce_size))` if the pattern matches.
    fn detect_softmax_pattern(kernel: &FusedKernel) -> Option<(usize, usize)> {
        // Need at least ReduceMax + something + ReduceSum + something = 4 ops minimum
        if kernel.ops.len() < 4 {
            return None;
        }

        // All buffers must be Float32 and contiguous
        if !kernel
            .bufs
            .iter()
            .all(|b| b.dtype == DType::Float32 && b.st.view().is_contiguous())
        {
            return None;
        }

        let has_reduce_max = kernel
            .ops
            .iter()
            .any(|op| op.op == PrimitiveOp::ReduceMax);
        let has_reduce_sum = kernel
            .ops
            .iter()
            .any(|op| op.op == PrimitiveOp::ReduceSum);

        if !has_reduce_max || !has_reduce_sum {
            return None;
        }

        // Find the input buffer for the ReduceMax (should be the raw input)
        let reduce_max_op = kernel
            .ops
            .iter()
            .find(|op| op.op == PrimitiveOp::ReduceMax)?;
        let input_buf_idx = match &reduce_max_op.srcs[0] {
            FusedSrc::Buf(idx) => *idx,
            _ => return None,
        };

        let output_numel = kernel.bufs[0].st.numel();
        let input_numel = kernel.bufs[input_buf_idx].st.numel();
        if output_numel == 0 {
            return None;
        }
        let reduce_size = input_numel / output_numel;
        if reduce_size == 0 || input_numel % output_numel != 0 {
            return None;
        }

        Some((input_buf_idx, reduce_size))
    }

    /// Detect an RMSNorm-like pattern in a fused kernel.
    ///
    /// Matches kernels that: square input -> ReduceSum -> scale by rsqrt.
    /// Specifically: first op is Mul(Buf(x), Buf(x)) (self-multiply = square),
    /// followed by ReduceSum. All buffers must be Float32 and contiguous.
    ///
    /// Returns `Some((input_buf_idx, dim, eps))` if the pattern matches.
    fn detect_rms_norm_pattern(kernel: &FusedKernel) -> Option<(usize, usize, f64)> {
        // Need at least Mul(x,x) + ReduceSum = 2 ops
        if kernel.ops.len() < 2 {
            return None;
        }

        // All buffers must be Float32 and contiguous
        if !kernel
            .bufs
            .iter()
            .all(|b| b.dtype == DType::Float32 && b.st.view().is_contiguous())
        {
            return None;
        }

        // First op must be Mul(Buf(x), Buf(x)) — self-multiply (squaring)
        let mul_op = &kernel.ops[0];
        if mul_op.op != PrimitiveOp::Mul {
            return None;
        }
        let input_buf_idx = match (&mul_op.srcs[0], &mul_op.srcs[1]) {
            (FusedSrc::Buf(a), FusedSrc::Buf(b)) if a == b => *a,
            _ => return None,
        };

        // Second op must be ReduceSum
        if kernel.ops[1].op != PrimitiveOp::ReduceSum {
            return None;
        }

        let output_numel = kernel.bufs[0].st.numel();
        let input_numel = kernel.bufs[input_buf_idx].st.numel();
        if output_numel == 0 {
            return None;
        }

        // For single-row RMSNorm: output_numel == input_numel, dim == input_numel
        // For multi-row RMSNorm: dim == input_numel / output_numel
        // The detect path only fires for the square+reduce subkernel which
        // reduces to a scalar. The full RMSNorm is: Mul(x,x) -> ReduceSum.
        // output_numel for the reduce kernel is 1 (scalar), dim == input_numel.
        let dim = input_numel;
        let n = if output_numel > 0 {
            output_numel
        } else {
            return None;
        };

        // Use standard epsilon for numerical stability
        let eps = 1e-6_f64;

        // Only match when output is the same size as input (the full rmsnorm
        // pattern where we write back x * rsqrt). For partial patterns (just
        // the reduce), the output is a scalar and we should fall through.
        if kernel.bufs[0].st.numel() != input_numel {
            return None;
        }

        Some((input_buf_idx, dim / n, eps))
    }

    /// Interpret and execute a FusedKernel on CPU buffers.
    /// `bufs` are raw byte slices matching kernel.bufs order.
    /// bufs[0] is the output buffer (written to).
    #[inline(always)]
    pub fn execute_kernel(kernel: &FusedKernel, bufs: &mut [Vec<u8>]) {
        let output_numel = kernel.bufs[0].st.numel();

        // Fast path: detect matmul pattern (Mul + ReduceSum, optionally + Add bias).
        // Must be checked BEFORE the SIMD path since matmul contains a reduce op.
        if let Some(pattern) = detect_matmul_pattern(kernel, bufs) {
            execute_matmul_fast(&pattern, bufs);
            return;
        }

        // Fast path: detect softmax pattern.
        // ReduceMax -> Sub -> Mul(log2e) -> Exp2 -> ReduceSum -> Reciprocal -> Mul
        // Or simpler: any kernel with ReduceMax followed by Sub/Exp2/ReduceSum/Mul.
        if let Some(pattern) = detect_softmax_pattern(kernel) {
            let (input_idx, reduce_size) = pattern;
            let input = bufs[input_idx].clone();
            fused_softmax(&input, &mut bufs[0], output_numel, reduce_size);
            return;
        }

        // Fast path: detect RMSNorm pattern.
        // Mul(x,x) -> ReduceSum -> ... -> Mul(x, rsqrt)
        if let Some(pattern) = detect_rms_norm_pattern(kernel) {
            let (input_idx, dim, eps) = pattern;
            let input = bufs[input_idx].clone();
            fused_rms_norm(&input, &mut bufs[0], output_numel, dim, eps);
            return;
        }

        // Check if SIMD fast path is applicable:
        // All buffers are Float32, all views are contiguous, no reduce ops.
        #[cfg(feature = "simd-accel")]
        {
            if can_use_simd_path(kernel) {
                execute_kernel_simd(kernel, bufs);
                return;
            }
        }

        // Pre-allocate values buffer outside the hot loop to avoid per-element
        // heap allocation. This is the single biggest optimization for small kernels.
        let mut values: Vec<f64> = vec![0.0; kernel.ops.len()];

        for gid in 0..output_numel {
            // Pre-allocate pre_values ONCE outside the reduce inner loop.
            // This eliminates O(reduce_size) heap allocations per output element.
            let mut pre_values: Vec<f64> = Vec::new();

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

                    // Reuse pre_values across reduce iterations instead of
                    // re-allocating on every iteration of the inner loop.
                    if op_idx > 0 {
                        pre_values.resize(op_idx, 0.0);
                    }

                    for rid in 0..reduce_size {
                        let eidx = gid * reduce_size + rid;

                        // If there are pre-reduce ops, compute them for this element
                        let val = if op_idx > 0 {
                            // Re-compute pre-reduce elementwise chain for this element index.
                            // pre_values is reused across iterations (no allocation).
                            for (pre_idx, pre_op) in kernel.ops[..op_idx].iter().enumerate() {
                                let get_pre_src = |i: usize| -> f64 {
                                    match &pre_op.srcs[i] {
                                        FusedSrc::Buf(idx) => {
                                            read_f64(&bufs[*idx], eidx, kernel.bufs[*idx].dtype)
                                        }
                                        FusedSrc::Op(prior) => pre_values[*prior],
                                        FusedSrc::Const { val, .. } => *val,
                                    }
                                };
                                pre_values[pre_idx] =
                                    compute_elementwise(pre_op.op, &get_pre_src, pre_op.srcs.len());
                            }
                            pre_values[op_idx - 1]
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
                    values[op_idx] = acc;
                    continue;
                }

                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype),
                        FusedSrc::Op(prior) => values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                values[op_idx] = compute_elementwise(op.op, &get_src, op.srcs.len());
            }

            // Write output
            let result = values[kernel.ops.len() - 1];
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
        let has_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));
        if has_reduce {
            return false;
        }

        // All ops must be SIMD-able
        kernel.ops.iter().all(|op| is_simd_op(op.op))
    }

    /// Whether a PrimitiveOp has a SIMD implementation.
    /// All 26 ops are covered: 6 arithmetic, 3 comparison, 5 bitwise,
    /// 6 math, 2 reduce, 2 conversion, 1 ternary, 1 bitcast.
    #[cfg(feature = "simd-accel")]
    #[inline(always)]
    fn is_simd_op(op: PrimitiveOp) -> bool {
        matches!(
            op,
            PrimitiveOp::Add
                | PrimitiveOp::Sub
                | PrimitiveOp::Mul
                | PrimitiveOp::Idiv
                | PrimitiveOp::Mod
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
                | PrimitiveOp::And
                | PrimitiveOp::Or
                | PrimitiveOp::Xor
                | PrimitiveOp::Shl
                | PrimitiveOp::Shr
                | PrimitiveOp::Bitcast
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

        // Pre-allocate values buffer OUTSIDE the hot loop — avoids heap
        // allocation per chunk which dominates small-kernel execution time.
        let mut simd_values: Vec<f32x4> = vec![f32x4::splat(0.0); kernel.ops.len()];

        // SIMD pass: process 4 elements at a time
        for chunk in 0..simd_count {
            let base = chunk * 4;

            for (op_idx, op) in kernel.ops.iter().enumerate() {
                let get_src_simd = |i: usize| -> f32x4 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            let buf = &bufs[*idx];
                            let offset = base * 4; // 4 bytes per f32
                            // Load 4 contiguous f32 values in one shot via pointer cast.
                            // Buffers are allocated with 16-byte alignment (alloc_simd_aligned),
                            // and base is always a multiple of 4 (chunk * 4), so offset is
                            // always 16-byte aligned. Use ptr::read for aligned access.
                            let ptr = buf[offset..].as_ptr() as *const [f32; 4];
                            // SAFETY: Buffer is 16-byte aligned (alloc_simd_aligned/alloc_page_aligned),
                            // offset is 16-byte aligned (base = chunk * 4, so offset = chunk * 16),
                            // and we verified offset + 16 <= buf.len() via simd_count calculation.
                            let arr = unsafe { std::ptr::read(ptr) };
                            f32x4::new(arr)
                        }
                        FusedSrc::Op(prior) => simd_values[*prior],
                        FusedSrc::Const { val, .. } => f32x4::splat(*val as f32),
                    }
                };

                simd_values[op_idx] = compute_elementwise_simd(op.op, &get_src_simd);
            }

            // Write SIMD output: store 4 contiguous f32 values in one shot.
            let result: [f32; 4] = simd_values[kernel.ops.len() - 1].into();
            let offset = base * 4;
            // SAFETY: Same alignment guarantees as the load path above.
            // Output buffer bufs[0] is 16-byte aligned and offset is 16-byte aligned.
            let out_ptr = bufs[0][offset..].as_mut_ptr() as *mut [f32; 4];
            unsafe {
                std::ptr::write(out_ptr, result);
            }
        }

        // Scalar remainder — reuse pre-allocated buffer
        let mut scalar_values: Vec<f64> = vec![0.0; kernel.ops.len()];
        for gid in remainder_start..output_numel {
            for (op_idx, op) in kernel.ops.iter().enumerate() {
                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype),
                        FusedSrc::Op(prior) => scalar_values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                scalar_values[op_idx] = compute_elementwise(op.op, &get_src, op.srcs.len());
            }

            let result = scalar_values[kernel.ops.len() - 1];
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
                    if aa[0].is_nan() || ba[0].is_nan() {
                        f32::NAN
                    } else {
                        aa[0].max(ba[0])
                    },
                    if aa[1].is_nan() || ba[1].is_nan() {
                        f32::NAN
                    } else {
                        aa[1].max(ba[1])
                    },
                    if aa[2].is_nan() || ba[2].is_nan() {
                        f32::NAN
                    } else {
                        aa[2].max(ba[2])
                    },
                    if aa[3].is_nan() || ba[3].is_nan() {
                        f32::NAN
                    } else {
                        aa[3].max(ba[3])
                    },
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
            PrimitiveOp::Idiv => {
                // Integer division: truncate both operands to i32, divide, convert back.
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if ba[0] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[0] as i32) / (ba[0] as i32)) as f32
                    },
                    if ba[1] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[1] as i32) / (ba[1] as i32)) as f32
                    },
                    if ba[2] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[2] as i32) / (ba[2] as i32)) as f32
                    },
                    if ba[3] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[3] as i32) / (ba[3] as i32)) as f32
                    },
                ])
            }
            PrimitiveOp::Mod => {
                // Integer modulo: truncate both operands to i32, modulo, convert back.
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    if ba[0] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[0] as i32) % (ba[0] as i32)) as f32
                    },
                    if ba[1] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[1] as i32) % (ba[1] as i32)) as f32
                    },
                    if ba[2] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[2] as i32) % (ba[2] as i32)) as f32
                    },
                    if ba[3] as i32 == 0 {
                        0.0
                    } else {
                        ((aa[3] as i32) % (ba[3] as i32)) as f32
                    },
                ])
            }
            PrimitiveOp::And => {
                // Bitwise AND on i32 reinterpretation of the f32 lanes.
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    ((aa[0] as i32) & (ba[0] as i32)) as f32,
                    ((aa[1] as i32) & (ba[1] as i32)) as f32,
                    ((aa[2] as i32) & (ba[2] as i32)) as f32,
                    ((aa[3] as i32) & (ba[3] as i32)) as f32,
                ])
            }
            PrimitiveOp::Or => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    ((aa[0] as i32) | (ba[0] as i32)) as f32,
                    ((aa[1] as i32) | (ba[1] as i32)) as f32,
                    ((aa[2] as i32) | (ba[2] as i32)) as f32,
                    ((aa[3] as i32) | (ba[3] as i32)) as f32,
                ])
            }
            PrimitiveOp::Xor => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    ((aa[0] as i32) ^ (ba[0] as i32)) as f32,
                    ((aa[1] as i32) ^ (ba[1] as i32)) as f32,
                    ((aa[2] as i32) ^ (ba[2] as i32)) as f32,
                    ((aa[3] as i32) ^ (ba[3] as i32)) as f32,
                ])
            }
            PrimitiveOp::Shl => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    ((aa[0] as i32) << (ba[0] as i32)) as f32,
                    ((aa[1] as i32) << (ba[1] as i32)) as f32,
                    ((aa[2] as i32) << (ba[2] as i32)) as f32,
                    ((aa[3] as i32) << (ba[3] as i32)) as f32,
                ])
            }
            PrimitiveOp::Shr => {
                let a = get_src(0);
                let b = get_src(1);
                let aa: [f32; 4] = a.into();
                let ba: [f32; 4] = b.into();
                f32x4::new([
                    ((aa[0] as i32) >> (ba[0] as i32)) as f32,
                    ((aa[1] as i32) >> (ba[1] as i32)) as f32,
                    ((aa[2] as i32) >> (ba[2] as i32)) as f32,
                    ((aa[3] as i32) >> (ba[3] as i32)) as f32,
                ])
            }
            PrimitiveOp::Bitcast => get_src(0), // f32 -> f32 is identity in SIMD
            PrimitiveOp::Cast => get_src(0),    // f32 -> f32 is identity
            PrimitiveOp::Trunc => {
                let x = get_src(0);
                let arr: [f32; 4] = x.into();
                f32x4::new([
                    arr[0].trunc(),
                    arr[1].trunc(),
                    arr[2].trunc(),
                    arr[3].trunc(),
                ])
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
                if b == 0 {
                    0.0
                } else {
                    (a / b) as f64
                }
            }
            PrimitiveOp::Mod => {
                let a = get_src(0) as i64;
                let b = get_src(1) as i64;
                if b == 0 {
                    0.0
                } else {
                    (a % b) as f64
                }
            }
            PrimitiveOp::Neg => -get_src(0),
            PrimitiveOp::Cmplt => {
                if get_src(0) < get_src(1) {
                    1.0
                } else {
                    0.0
                }
            }
            PrimitiveOp::Cmpeq => {
                if get_src(0) == get_src(1) {
                    1.0
                } else {
                    0.0
                }
            }
            PrimitiveOp::Cmpne => {
                if get_src(0) != get_src(1) {
                    1.0
                } else {
                    0.0
                }
            }
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
                if get_src(0) != 0.0 {
                    get_src(1)
                } else {
                    get_src(2)
                }
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
                if offset + 4 > buf.len() {
                    return 0.0;
                }
                f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() {
                    return 0.0;
                }
                f64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() {
                    return 0.0;
                }
                i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Bool | DType::UInt8 => {
                if idx >= buf.len() {
                    return 0.0;
                }
                buf[idx] as f64
            }
            DType::Int8 => {
                if idx >= buf.len() {
                    return 0.0;
                }
                (buf[idx] as i8) as f64
            }
            DType::Int16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() {
                    return 0.0;
                }
                i16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::UInt16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() {
                    return 0.0;
                }
                u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::Int64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() {
                    return 0.0;
                }
                i64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::UInt32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() {
                    return 0.0;
                }
                u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::UInt64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() {
                    return 0.0;
                }
                u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::Float16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() {
                    return 0.0;
                }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::f16::from_bits(bits).to_f64()
            }
            DType::BFloat16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() {
                    return 0.0;
                }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::bf16::from_bits(bits).to_f64()
            }
            // MXFP types: raw byte read. Dequantization happens at a higher level.
            DType::MxFP8 | DType::MxFP4 => {
                if idx >= buf.len() {
                    return 0.0;
                }
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
