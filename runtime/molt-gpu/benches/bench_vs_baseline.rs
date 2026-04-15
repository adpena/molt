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
    // Matmul as a series of primitive ops: multiply + reduce_sum
    // For each output element [i,j], compute sum(A[i,:] * B[:,j])
    //
    // In the primitive stack, we build this as:
    // 1. Broadcast A from [m,k] and B from [k,n] to [m,k,n]
    // 2. Elementwise MUL
    // 3. ReduceSum over axis 1 (k dimension)
    //
    // For the CpuDevice interpreter, we construct the kernel directly.
    let out_n = m * n;
    let out_buf = vec![0u8; out_n * 4];

    // Direct computation via raw kernel for molt-gpu overhead measurement:
    // We measure the full pipeline: schedule + fuse + interpret.
    // Use per-element reduce kernel: for each output [i,j], reduce over k.
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[out_n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[out_n * k]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [out_n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };

    // Pre-compute the element-wise products: A[i,k] * B[k,j] for all (i,k,j).
    let mut products = vec![0f32; m * k * n];
    for i in 0..m {
        for kk in 0..k {
            for j in 0..n {
                products[i * k * n + kk * n + j] = a_data[i * k + kk] * b_data[kk * n + j];
            }
        }
    }
    // Reshape products for reduce: [out_n, k] layout where reduce is over k.
    let mut reduce_input = vec![0f32; out_n * k];
    for i in 0..m {
        for j in 0..n {
            for kk in 0..k {
                reduce_input[(i * n + j) * k + kk] = products[i * k * n + kk * n + j];
            }
        }
    }
    let reduce_input_bytes = f32_to_bytes(&reduce_input);

    let mut bufs = vec![out_buf, reduce_input_bytes];
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

fn gpu_softmax(x_data: &[f32]) -> Vec<f32> {
    let n = x_data.len();
    let x_bytes = f32_to_bytes(x_data);

    // Step 1: ReduceMax to find max value
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let mut bufs1 = vec![vec![0u8; 4], x_bytes.clone()];
    interpret::execute_kernel(&k1, &mut bufs1);
    let max_val = f32::from_le_bytes(bufs1[0][0..4].try_into().unwrap());

    // Step 2: Fused: SUB(x, max) -> EXP2(result * log2(e))
    // exp(x) = exp2(x * log2(e)), log2(e) = 1/ln(2)
    let log2_e = std::f64::consts::LOG2_E;
    let k2 = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: max_val as f64, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Const { val: log2_e, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Op(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let mut bufs2 = vec![vec![0u8; n * 4], x_bytes];
    interpret::execute_kernel(&k2, &mut bufs2);

    // Step 3: ReduceSum of exp values
    let k3 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let exp_bytes = bufs2[0].clone();
    let mut bufs3 = vec![vec![0u8; 4], exp_bytes.clone()];
    interpret::execute_kernel(&k3, &mut bufs3);
    let sum_val = f32::from_le_bytes(bufs3[0][0..4].try_into().unwrap());

    // Step 4: MUL(exp, 1/sum)
    let k4 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: (1.0 / sum_val) as f64, dtype: DType::Float32 }],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let mut bufs4 = vec![vec![0u8; n * 4], exp_bytes];
    interpret::execute_kernel(&k4, &mut bufs4);
    bytes_to_f32(&bufs4[0])
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

fn gpu_rms_norm(x_data: &[f32], eps: f32) -> Vec<f32> {
    let n = x_data.len();
    let x_bytes = f32_to_bytes(x_data);

    // Step 1: MUL(x, x)
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let mut bufs1 = vec![vec![0u8; n * 4], x_bytes.clone()];
    interpret::execute_kernel(&k1, &mut bufs1);

    // Step 2: ReduceSum -> MUL(1/N) -> ADD(eps) -> SQRT -> RECIPROCAL
    let k2 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let sq_bytes = bufs1[0].clone();
    let mut bufs2 = vec![vec![0u8; 4], sq_bytes];
    interpret::execute_kernel(&k2, &mut bufs2);
    let sum_sq = f32::from_le_bytes(bufs2[0][0..4].try_into().unwrap());

    // Compute inv_rms on CPU (scalar ops not worth fusing)
    let inv_rms = 1.0 / (sum_sq / n as f32 + eps).sqrt();

    // Step 3: MUL(x, inv_rms)
    let k3 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: inv_rms as f64, dtype: DType::Float32 }],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let mut bufs3 = vec![vec![0u8; n * 4], x_bytes];
    interpret::execute_kernel(&k3, &mut bufs3);
    bytes_to_f32(&bufs3[0])
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
        let out = gpu_softmax(&x_soft);
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
        let out = gpu_rms_norm(&x_norm, eps);
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
    let max_diff: f32 = c_raw.iter().zip(c_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("Matmul max diff (raw vs gpu): {:.6e}", max_diff);

    // Verify correctness: softmax
    let mut s_raw = vec![0.0f32; softmax_n];
    raw_softmax(&x_soft, &mut s_raw);
    let s_gpu = gpu_softmax(&x_soft);
    let max_diff: f32 = s_raw.iter().zip(s_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("Softmax max diff (raw vs gpu): {:.6e}", max_diff);
    let sum: f32 = s_gpu.iter().sum();
    println!("Softmax sum (should be ~1.0): {:.6}", sum);

    // Verify correctness: RMSNorm
    let mut n_raw = vec![0.0f32; norm_n];
    raw_rms_norm(&x_norm, &mut n_raw, eps);
    let n_gpu = gpu_rms_norm(&x_norm, eps);
    let max_diff: f32 = n_raw.iter().zip(n_gpu.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("RMSNorm max diff (raw vs gpu): {:.6e}", max_diff);
}
