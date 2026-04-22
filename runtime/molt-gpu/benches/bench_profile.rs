//! Full pipeline profiler: LazyOp -> schedule -> fuse -> CpuDevice interpret -> readback.
//!
//! Breaks down time spent in each pipeline stage for three key operations:
//!   - Softmax (N=1024)
//!   - Matmul (M=64, K=64, N=64)
//!   - RMSNorm (N=1024)
//!
//! Reports a percentage breakdown table and identifies the top 3 hotspots.

use std::time::Instant;

use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::fuse::fuse;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use molt_gpu::shapetracker::ShapeTracker;

const WARMUP_ITERS: usize = 5;
const MEASURE_ITERS: usize = 100;

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn _bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// Per-stage timing for one pipeline run.
#[derive(Debug, Clone)]
struct StageTimings {
    dag_construction_us: f64,
    scheduling_us: f64,
    fusion_us: f64,
    interpretation_us: f64,
    memory_alloc_copy_us: f64,
    total_us: f64,
}

impl StageTimings {
    fn zero() -> Self {
        Self {
            dag_construction_us: 0.0,
            scheduling_us: 0.0,
            fusion_us: 0.0,
            interpretation_us: 0.0,
            memory_alloc_copy_us: 0.0,
            total_us: 0.0,
        }
    }

    fn add(&mut self, other: &StageTimings) {
        self.dag_construction_us += other.dag_construction_us;
        self.scheduling_us += other.scheduling_us;
        self.fusion_us += other.fusion_us;
        self.interpretation_us += other.interpretation_us;
        self.memory_alloc_copy_us += other.memory_alloc_copy_us;
        self.total_us += other.total_us;
    }

    fn scale(&mut self, factor: f64) {
        self.dag_construction_us *= factor;
        self.scheduling_us *= factor;
        self.fusion_us *= factor;
        self.interpretation_us *= factor;
        self.memory_alloc_copy_us *= factor;
        self.total_us *= factor;
    }
}

/// Measure a closure's duration.
#[allow(dead_code)]
fn time_us<F: FnMut()>(mut f: F) -> f64 {
    let start = Instant::now();
    f();
    start.elapsed().as_secs_f64() * 1e6
}

// ============================================================================
// Softmax pipeline
// ============================================================================

fn profile_softmax(n: usize) -> StageTimings {
    // Input data
    let x_data: Vec<f32> = (0..n).map(|i| (i as f32) * 0.01 - 5.0).collect();

    let mut avg = StageTimings::zero();

    // Warmup
    for _ in 0..WARMUP_ITERS {
        run_softmax_pipeline(&x_data);
    }

    // Measure
    for _ in 0..MEASURE_ITERS {
        let t = run_softmax_pipeline(&x_data);
        avg.add(&t);
    }

    let scale = 1.0 / MEASURE_ITERS as f64;
    avg.scale(scale);
    avg
}

fn run_softmax_pipeline(x_data: &[f32]) -> StageTimings {
    let n = x_data.len();
    let total_start = Instant::now();

    // Stage 1: DAG construction (build FusedKernel chain)
    let dag_start = Instant::now();

    let k_reduce_max = FusedKernel {
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
        spec: None,
        vectorize_width: 1,
    };

    let log2_e = std::f64::consts::LOG2_E;
    let k_exp = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![
                    FusedSrc::Op(0),
                    FusedSrc::Const {
                        val: log2_e,
                        dtype: DType::Float32,
                    },
                ],
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
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let k_reduce_sum = FusedKernel {
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
        spec: None,
        vectorize_width: 1,
    };

    let k_normalize = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
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
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let kernels = vec![k_reduce_max, k_exp, k_reduce_sum, k_normalize];
    let dag_us = dag_start.elapsed().as_secs_f64() * 1e6;

    // Stage 2: Scheduling (already pre-built, but measure fuse pass)
    // Since we already built FusedKernels directly, scheduling = 0.
    // We measure fusion explicitly.
    let sched_us = 0.0;

    // Stage 3: Fusion pass
    let fuse_start = Instant::now();
    let _fused = fuse(kernels.clone());
    let fuse_us = fuse_start.elapsed().as_secs_f64() * 1e6;

    // Stage 4: Memory allocation + data copy
    let mem_start = Instant::now();
    let x_bytes = f32_to_bytes(x_data);
    let mut max_buf = vec![0u8; 4];
    let mut exp_buf = vec![0u8; n * 4];
    let mut sum_buf = vec![0u8; 4];
    let out_buf = vec![0u8; n * 4];
    let mem_us = mem_start.elapsed().as_secs_f64() * 1e6;

    // Stage 5: Kernel interpretation
    let interp_start = Instant::now();

    // Execute the 4-kernel softmax pipeline
    let mut bufs1 = vec![max_buf.clone(), x_bytes.clone()];
    interpret::execute_kernel(&kernels[0], &mut bufs1);
    max_buf = bufs1[0].clone();

    let mut bufs2 = vec![exp_buf.clone(), x_bytes.clone(), max_buf.clone()];
    interpret::execute_kernel(&kernels[1], &mut bufs2);
    exp_buf = bufs2[0].clone();

    let mut bufs3 = vec![sum_buf.clone(), exp_buf.clone()];
    interpret::execute_kernel(&kernels[2], &mut bufs3);
    sum_buf = bufs3[0].clone();

    // Compute 1/sum for normalization
    let sum_val = f32::from_le_bytes(sum_buf[0..4].try_into().unwrap());
    let inv_sum = 1.0 / sum_val;
    let _inv_sum_bytes = inv_sum.to_le_bytes().to_vec();

    let k_final = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: inv_sum as f64,
                    dtype: DType::Float32,
                },
            ],
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
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs4 = vec![out_buf, exp_buf];
    interpret::execute_kernel(&k_final, &mut bufs4);

    let interp_us = interp_start.elapsed().as_secs_f64() * 1e6;
    let total_us = total_start.elapsed().as_secs_f64() * 1e6;

    StageTimings {
        dag_construction_us: dag_us,
        scheduling_us: sched_us,
        fusion_us: fuse_us,
        interpretation_us: interp_us,
        memory_alloc_copy_us: mem_us,
        total_us,
    }
}

// ============================================================================
// Matmul pipeline (M=64, K=64, N=64)
// ============================================================================

fn profile_matmul(m: usize, k: usize, n: usize) -> StageTimings {
    let a_data: Vec<f32> = (0..m * k).map(|i| (i as f32) * 0.001).collect();
    let b_data: Vec<f32> = (0..k * n).map(|i| (i as f32) * 0.001).collect();

    let mut avg = StageTimings::zero();

    for _ in 0..WARMUP_ITERS {
        run_matmul_pipeline(&a_data, &b_data, m, k, n);
    }

    for _ in 0..MEASURE_ITERS {
        let t = run_matmul_pipeline(&a_data, &b_data, m, k, n);
        avg.add(&t);
    }

    let scale = 1.0 / MEASURE_ITERS as f64;
    avg.scale(scale);
    avg
}

fn run_matmul_pipeline(
    a_data: &[f32],
    b_data: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> StageTimings {
    let out_n = m * n;
    let total_start = Instant::now();

    // DAG construction — fused matmul needs no kernel DAG
    let dag_start = Instant::now();
    let dag_us = dag_start.elapsed().as_secs_f64() * 1e6;

    // Fusion — fused matmul bypasses the fusion pass
    let fuse_us = 0.0;

    // Memory allocation — only output buffer, no intermediate product tensor
    let mem_start = Instant::now();
    let a_bytes = f32_to_bytes(a_data);
    let b_bytes = f32_to_bytes(b_data);
    let mut out_buf = vec![0u8; out_n * 4];
    let mem_us = mem_start.elapsed().as_secs_f64() * 1e6;

    // Interpretation — direct fused matmul
    let interp_start = Instant::now();
    interpret::fused_matmul(&a_bytes, &b_bytes, &mut out_buf, m, k, n);
    let interp_us = interp_start.elapsed().as_secs_f64() * 1e6;

    let total_us = total_start.elapsed().as_secs_f64() * 1e6;

    StageTimings {
        dag_construction_us: dag_us,
        scheduling_us: 0.0,
        fusion_us: fuse_us,
        interpretation_us: interp_us,
        memory_alloc_copy_us: mem_us,
        total_us,
    }
}

// ============================================================================
// RMSNorm pipeline: x * rsqrt(mean(x^2) + eps)
// ============================================================================

fn profile_rmsnorm(n: usize) -> StageTimings {
    let x_data: Vec<f32> = (0..n).map(|i| (i as f32) * 0.01 - 5.0).collect();

    let mut avg = StageTimings::zero();

    for _ in 0..WARMUP_ITERS {
        run_rmsnorm_pipeline(&x_data);
    }

    for _ in 0..MEASURE_ITERS {
        let t = run_rmsnorm_pipeline(&x_data);
        avg.add(&t);
    }

    let scale = 1.0 / MEASURE_ITERS as f64;
    avg.scale(scale);
    avg
}

fn run_rmsnorm_pipeline(x_data: &[f32]) -> StageTimings {
    let n = x_data.len();
    let eps = 1e-6_f64;
    let total_start = Instant::now();

    // DAG construction: x*x -> reduce_sum -> mul(1/n) -> add(eps) -> sqrt -> reciprocal -> mul(x, result)
    let dag_start = Instant::now();

    let k_sq = FusedKernel {
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
        spec: None,
        vectorize_width: 1,
    };

    let k_reduce = FusedKernel {
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
        spec: None,
        vectorize_width: 1,
    };

    let kernels = vec![k_sq.clone(), k_reduce.clone()];
    let dag_us = dag_start.elapsed().as_secs_f64() * 1e6;

    // Fusion
    let fuse_start = Instant::now();
    let _fused = fuse(kernels);
    let fuse_us = fuse_start.elapsed().as_secs_f64() * 1e6;

    // Memory
    let mem_start = Instant::now();
    let x_bytes = f32_to_bytes(x_data);
    let sq_buf = vec![0u8; n * 4];
    let sum_buf = vec![0u8; 4];
    let out_buf = vec![0u8; n * 4];
    let mem_us = mem_start.elapsed().as_secs_f64() * 1e6;

    // Interpretation
    let interp_start = Instant::now();

    // x^2
    let mut bufs1 = vec![sq_buf, x_bytes.clone()];
    interpret::execute_kernel(&k_sq, &mut bufs1);

    // sum(x^2)
    let mut bufs2 = vec![sum_buf, bufs1[0].clone()];
    interpret::execute_kernel(&k_reduce, &mut bufs2);

    let sum_val = f32::from_le_bytes(bufs2[0][0..4].try_into().unwrap());
    let mean_val = sum_val / n as f32;
    let rsqrt_val = 1.0 / (mean_val + eps as f32).sqrt();

    // x * rsqrt(mean(x^2) + eps)
    let k_scale = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: rsqrt_val as f64,
                    dtype: DType::Float32,
                },
            ],
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
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs3 = vec![out_buf, x_bytes];
    interpret::execute_kernel(&k_scale, &mut bufs3);

    let interp_us = interp_start.elapsed().as_secs_f64() * 1e6;
    let total_us = total_start.elapsed().as_secs_f64() * 1e6;

    StageTimings {
        dag_construction_us: dag_us,
        scheduling_us: 0.0,
        fusion_us: fuse_us,
        interpretation_us: interp_us,
        memory_alloc_copy_us: mem_us,
        total_us,
    }
}

// ============================================================================
// Output
// ============================================================================

fn print_breakdown(_name: &str, t: &StageTimings) {
    let total = t.total_us.max(0.001); // prevent division by zero
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "DAG construction",
        t.dag_construction_us,
        t.dag_construction_us / total * 100.0
    );
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "Scheduling",
        t.scheduling_us,
        t.scheduling_us / total * 100.0
    );
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "Fusion",
        t.fusion_us,
        t.fusion_us / total * 100.0
    );
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "Kernel interpretation",
        t.interpretation_us,
        t.interpretation_us / total * 100.0
    );
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "Memory alloc/copy",
        t.memory_alloc_copy_us,
        t.memory_alloc_copy_us / total * 100.0
    );
    println!(
        "| {:<25} | {:>10.2} | {:>6.1}% |",
        "TOTAL", t.total_us, 100.0
    );
}

fn main() {
    println!("# molt-gpu Pipeline Profiler\n");
    println!(
        "Warmup: {} iters, Measurement: {} iters\n",
        WARMUP_ITERS, MEASURE_ITERS
    );

    // Softmax N=1024
    let softmax_t = profile_softmax(1024);
    println!("## Softmax (N=1024)\n");
    println!(
        "| {:<25} | {:>10} | {:>7} |",
        "Stage", "Avg (us)", "% Total"
    );
    println!("|{:-<27}|{:-<12}|{:-<9}|", "", "", "");
    print_breakdown("Softmax", &softmax_t);

    // Matmul 64x64
    let matmul_t = profile_matmul(64, 64, 64);
    println!("\n## Matmul (64x64x64)\n");
    println!(
        "| {:<25} | {:>10} | {:>7} |",
        "Stage", "Avg (us)", "% Total"
    );
    println!("|{:-<27}|{:-<12}|{:-<9}|", "", "", "");
    print_breakdown("Matmul", &matmul_t);

    // RMSNorm N=1024
    let rmsnorm_t = profile_rmsnorm(1024);
    println!("\n## RMSNorm (N=1024)\n");
    println!(
        "| {:<25} | {:>10} | {:>7} |",
        "Stage", "Avg (us)", "% Total"
    );
    println!("|{:-<27}|{:-<12}|{:-<9}|", "", "", "");
    print_breakdown("RMSNorm", &rmsnorm_t);

    // Top 3 hotspots across all operations
    println!("\n## Top 3 Hotspots\n");
    let mut hotspots = vec![
        ("Softmax: DAG construction", softmax_t.dag_construction_us),
        ("Softmax: Fusion", softmax_t.fusion_us),
        ("Softmax: Interpretation", softmax_t.interpretation_us),
        ("Softmax: Memory", softmax_t.memory_alloc_copy_us),
        ("Matmul: DAG construction", matmul_t.dag_construction_us),
        ("Matmul: Fusion", matmul_t.fusion_us),
        ("Matmul: Interpretation", matmul_t.interpretation_us),
        ("Matmul: Memory", matmul_t.memory_alloc_copy_us),
        ("RMSNorm: DAG construction", rmsnorm_t.dag_construction_us),
        ("RMSNorm: Fusion", rmsnorm_t.fusion_us),
        ("RMSNorm: Interpretation", rmsnorm_t.interpretation_us),
        ("RMSNorm: Memory", rmsnorm_t.memory_alloc_copy_us),
    ];
    hotspots.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    for (i, (name, us)) in hotspots.iter().take(3).enumerate() {
        println!("{}. **{}**: {:.2} us", i + 1, name, us);
    }
}
