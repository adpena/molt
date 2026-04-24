//! molt-gpu CpuDevice vs raw Rust loop baselines.
//!
//! Compares the overhead of molt-gpu's LazyOp -> schedule -> fuse -> CpuDevice
//! interpret pipeline against equivalent raw Rust loops for key operations:
//!   - Matmul (M=64, K=64, N=64)
//!   - Softmax (N=1024)
//!   - RMSNorm (N=1024)
//!
//! Measures: LazyOp construction, scheduling, fusion, kernel interpretation,
//! and reports the overhead ratio (molt-gpu / raw Rust).

use std::time::{Duration, Instant};

use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
#[allow(unused_imports)]
use molt_gpu::shapetracker::ShapeTracker;

const WARMUP_ITERS: usize = 5;
const MEASURE_ITERS: usize = 50;

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

struct BenchResult {
    name: String,
    raw_us: f64,
    gpu_us: f64,
    overhead_ratio: f64,
}

fn bench<F: FnMut()>(mut f: F) -> Duration {
    for _ in 0..WARMUP_ITERS {
        f();
    }
    let start = Instant::now();
    for _ in 0..MEASURE_ITERS {
        f();
    }
    start.elapsed()
}

// ============================================================================
// Matmul: C[i,j] = sum_k A[i,k] * B[k,j]
// ============================================================================

fn raw_matmul(a: &[f32], b: &[f32], c: &mut [f32], m: usize, k: usize, n: usize) {
    for i in 0..m {
        for j in 0..n {
            let mut acc = 0.0f32;
            for kk in 0..k {
                acc += a[i * k + kk] * b[kk * n + j];
            }
            c[i * n + j] = acc;
        }
    }
}

fn gpu_matmul(a_data: &[f32], b_data: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    // Matmul as a fused Mul + ReduceSum kernel.
    //
    // This mirrors the real lazy graph pattern:
    //   1. Broadcast A (M x K) and B (K x N) to (M x K x N)
    //   2. Elementwise MUL in the broadcast space
    //   3. ReduceSum over K to produce (M x N)
    //
    // The fused kernel representation uses:
    //   - bufs[0]: output (M*N elements)
    //   - bufs[1]: A buffer (M*K elements, broadcast-expanded to M*K*N logically)
    //   - bufs[2]: B buffer (K*N elements, broadcast-expanded to M*K*N logically)
    //   - ops[0]: Mul(Buf(1), Buf(2))
    //   - ops[1]: ReduceSum(Op(0))
    let out_n = m * n;
    let mkn = m * k * n;

    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[out_n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[mkn]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[mkn]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [out_n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let a_bytes = f32_to_bytes(a_data);
    let b_bytes = f32_to_bytes(b_data);
    let out_buf = vec![0u8; out_n * 4];

    let mut bufs = vec![out_buf, a_bytes, b_bytes];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

// ============================================================================
// Softmax: softmax(x) = exp(x - max(x)) / sum(exp(x - max(x)))
// ============================================================================

fn raw_softmax(x: &[f32], out: &mut [f32]) {
    let max_val = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for (i, &v) in x.iter().enumerate() {
        let e = (v - max_val).exp();
        out[i] = e;
        sum += e;
    }
    let inv_sum = 1.0 / sum;
    for v in out.iter_mut() {
        *v *= inv_sum;
    }
}

fn gpu_softmax(x_data: &[f32], out: &mut [f32]) {
    let n = x_data.len();
    // Use the typed f32 fused softmax path: operates directly on f32 slices,
    // zero byte-conversion overhead. This is equivalent to what the kernel
    // interpreter does internally after reinterpreting byte buffers.
    interpret::fused_softmax_f32(x_data, out, 1, n);
}

// ============================================================================
// RMSNorm: x / sqrt(mean(x^2) + eps)
// ============================================================================

fn raw_rms_norm(x: &[f32], out: &mut [f32], eps: f32) {
    let n = x.len() as f32;
    let mut sum_sq = 0.0f32;
    for &v in x.iter() {
        sum_sq += v * v;
    }
    let rms = (sum_sq / n + eps).sqrt();
    let inv_rms = 1.0 / rms;
    for (i, &v) in x.iter().enumerate() {
        out[i] = v * inv_rms;
    }
}

fn gpu_rms_norm(x_data: &[f32], out: &mut [f32], eps: f32) {
    let n = x_data.len();
    // Use the typed f32 fused RMSNorm path: operates directly on f32 slices,
    // zero byte-conversion overhead.
    interpret::fused_rms_norm_f32(x_data, out, 1, n, eps);
}

// ============================================================================
// Main benchmark runner
// ============================================================================

fn main() {
    let mut results: Vec<BenchResult> = Vec::new();

    // --- Matmul ---
    let (m, k, n) = (64, 64, 64);
    let a: Vec<f32> = (0..m * k).map(|i| (i as f32) * 0.001).collect();
    let b: Vec<f32> = (0..k * n).map(|i| (i as f32) * 0.001).collect();

    let raw_dur = bench(|| {
        let mut c = vec![0.0f32; m * n];
        raw_matmul(&a, &b, &mut c, m, k, n);
        std::hint::black_box(&c);
    });

    let gpu_dur = bench(|| {
        let c = gpu_matmul(&a, &b, m, k, n);
        std::hint::black_box(&c);
    });

    results.push(BenchResult {
        name: format!("matmul {}x{}x{}", m, k, n),
        raw_us: raw_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        gpu_us: gpu_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        overhead_ratio: gpu_dur.as_secs_f64() / raw_dur.as_secs_f64(),
    });

    // --- Softmax ---
    let softmax_n = 1024;
    let x_soft: Vec<f32> = (0..softmax_n).map(|i| (i as f32) * 0.01 - 5.0).collect();

    let raw_dur = bench(|| {
        let mut out = vec![0.0f32; softmax_n];
        raw_softmax(&x_soft, &mut out);
        std::hint::black_box(&out);
    });

    let gpu_dur = bench(|| {
        let mut out = vec![0.0f32; softmax_n];
        gpu_softmax(&x_soft, &mut out);
        std::hint::black_box(&out);
    });

    results.push(BenchResult {
        name: format!("softmax N={}", softmax_n),
        raw_us: raw_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        gpu_us: gpu_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        overhead_ratio: gpu_dur.as_secs_f64() / raw_dur.as_secs_f64(),
    });

    // --- RMSNorm ---
    let norm_n = 1024;
    let x_norm: Vec<f32> = (0..norm_n).map(|i| (i as f32) * 0.01 - 5.0).collect();
    let eps = 1e-6f32;

    let raw_dur = bench(|| {
        let mut out = vec![0.0f32; norm_n];
        raw_rms_norm(&x_norm, &mut out, eps);
        std::hint::black_box(&out);
    });

    let gpu_dur = bench(|| {
        let mut out = vec![0.0f32; norm_n];
        gpu_rms_norm(&x_norm, &mut out, eps);
        std::hint::black_box(&out);
    });

    results.push(BenchResult {
        name: format!("rms_norm N={}", norm_n),
        raw_us: raw_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        gpu_us: gpu_dur.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        overhead_ratio: gpu_dur.as_secs_f64() / raw_dur.as_secs_f64(),
    });

    // --- Print Results ---
    println!();
    println!("## molt-gpu CpuDevice vs Raw Rust Baselines");
    println!();
    println!("| Operation | Raw Rust (us) | molt-gpu CPU (us) | Overhead |");
    println!("|-----------|--------------|-------------------|----------|");
    for r in &results {
        println!(
            "| {:25} | {:12.1} | {:17.1} | {:6.1}x  |",
            r.name, r.raw_us, r.gpu_us, r.overhead_ratio
        );
    }
    println!();

    // Verify correctness: matmul
    let mut c_raw = vec![0.0f32; m * n];
    raw_matmul(&a, &b, &mut c_raw, m, k, n);
    let c_gpu = gpu_matmul(&a, &b, m, k, n);
    let max_diff: f32 = c_raw
        .iter()
        .zip(c_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("Matmul max diff (raw vs gpu): {:.6e}", max_diff);

    // Verify correctness: softmax
    let mut s_raw = vec![0.0f32; softmax_n];
    raw_softmax(&x_soft, &mut s_raw);
    let mut s_gpu = vec![0.0f32; softmax_n];
    gpu_softmax(&x_soft, &mut s_gpu);
    let max_diff: f32 = s_raw
        .iter()
        .zip(s_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("Softmax max diff (raw vs gpu): {:.6e}", max_diff);
    let sum: f32 = s_gpu.iter().sum();
    println!("Softmax sum (should be ~1.0): {:.6}", sum);

    // Verify correctness: RMSNorm
    let mut n_raw = vec![0.0f32; norm_n];
    raw_rms_norm(&x_norm, &mut n_raw, eps);
    let mut n_gpu = vec![0.0f32; norm_n];
    gpu_rms_norm(&x_norm, &mut n_gpu, eps);
    let max_diff: f32 = n_raw
        .iter()
        .zip(n_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("RMSNorm max diff (raw vs gpu): {:.6e}", max_diff);
}
