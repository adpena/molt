//! Fusion benchmarks: fused vs unfused kernel chains.
//!
//! Measures the overhead of the fusion pass itself and compares
//! fused vs unfused render output for key compositions.
//!
//! Uses `std::time::Instant` for measurement — no external benchmark crate needed.

use std::time::{Duration, Instant};

use molt_gpu::dtype::DType;
use molt_gpu::fuse::fuse;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::msl::MslRenderer;
use molt_gpu::shapetracker::ShapeTracker;

/// Number of warmup iterations.
const WARMUP_ITERS: usize = 5;
/// Number of measurement iterations.
const MEASURE_ITERS: usize = 100;

/// Benchmark result.
struct FusionBenchResult {
    name: String,
    unfused_kernels: usize,
    fused_kernels: usize,
    unfused_render_us: f64,
    fused_render_us: f64,
    fusion_time_us: f64,
}

/// Run a timed closure and return average duration.
fn measure<F: FnMut()>(mut f: F) -> Duration {
    for _ in 0..WARMUP_ITERS {
        f();
    }
    let start = Instant::now();
    for _ in 0..MEASURE_ITERS {
        f();
    }
    start.elapsed()
}

fn main() {
    let renderer = MslRenderer;
    let mut results = Vec::new();

    println!("# molt-gpu Fusion Benchmark Results\n");
    println!("Warmup: {} iters, Measurement: {} iters\n", WARMUP_ITERS, MEASURE_ITERS);

    // --- Softmax: unfused (7 individual ops) vs fused (2 kernels) ---
    let n = 1024;
    let softmax_unfused: Vec<FusedKernel> = vec![
        // 1. ReduceMax
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceMax,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // 2. Sub (x - max)
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
                BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // 3. Exp2
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // 4. ReduceSum
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // 5. Reciprocal
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Reciprocal,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // 6. Mul (exp * inv_sum)
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
                BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
    ];

    // Fused softmax: 2 kernels (reduce_max fused with sub+exp2, reduce_sum fused with recip+mul)
    let softmax_fused: Vec<FusedKernel> = vec![
        // Kernel 1: sub -> exp2 -> reduce_sum (with ReduceMax as separate first kernel)
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceMax,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
        // Kernel 2: sub -> exp2 -> reduce_sum -> reciprocal -> mul (fused)
        FusedKernel {
            ops: vec![
                FusedOp {
                    op: PrimitiveOp::Sub,
                    srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::Exp2,
                    srcs: vec![FusedSrc::Op(0)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::ReduceSum,
                    srcs: vec![FusedSrc::Op(1)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::Reciprocal,
                    srcs: vec![FusedSrc::Op(2)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::Mul,
                    srcs: vec![FusedSrc::Op(1), FusedSrc::Op(3)],
                    dst_dtype: DType::Float32,
                },
            ],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
                BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        },
    ];

    // Measure unfused render time
    let unfused_duration = measure(|| {
        for k in &softmax_unfused {
            let _ = renderer.render(k);
        }
    });

    // Measure fused render time
    let fused_duration = measure(|| {
        for k in &softmax_fused {
            let _ = renderer.render(k);
        }
    });

    // Measure fusion pass time
    let fusion_duration = measure(|| {
        let _ = fuse(softmax_unfused.clone());
    });

    results.push(FusionBenchResult {
        name: "softmax_1024".to_string(),
        unfused_kernels: softmax_unfused.len(),
        fused_kernels: softmax_fused.len(),
        unfused_render_us: unfused_duration.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        fused_render_us: fused_duration.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        fusion_time_us: fusion_duration.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
    });

    // --- Elementwise chain: 4 ops unfused vs 1 kernel fused ---
    let chain_unfused: Vec<FusedKernel> = (0..4).map(|_| {
        FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
                BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
                spec: None, vectorize_width: 1,
        }
    }).collect();

    let chain_fused = vec![FusedKernel {
        ops: vec![
            FusedOp { op: PrimitiveOp::Add, srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)], dst_dtype: DType::Float32 },
            FusedOp { op: PrimitiveOp::Add, srcs: vec![FusedSrc::Op(0), FusedSrc::Buf(3)], dst_dtype: DType::Float32 },
            FusedOp { op: PrimitiveOp::Add, srcs: vec![FusedSrc::Op(1), FusedSrc::Buf(4)], dst_dtype: DType::Float32 },
            FusedOp { op: PrimitiveOp::Add, srcs: vec![FusedSrc::Op(2), FusedSrc::Buf(5)], dst_dtype: DType::Float32 },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 4, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 5, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
                spec: None, vectorize_width: 1,
    }];

    let unfused_d2 = measure(|| {
        for k in &chain_unfused {
            let _ = renderer.render(k);
        }
    });
    let fused_d2 = measure(|| {
        for k in &chain_fused {
            let _ = renderer.render(k);
        }
    });
    let fusion_d2 = measure(|| {
        let _ = fuse(chain_unfused.clone());
    });

    results.push(FusionBenchResult {
        name: "elem_chain_4x_add".to_string(),
        unfused_kernels: chain_unfused.len(),
        fused_kernels: chain_fused.len(),
        unfused_render_us: unfused_d2.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        fused_render_us: fused_d2.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
        fusion_time_us: fusion_d2.as_secs_f64() * 1e6 / MEASURE_ITERS as f64,
    });

    // Print results
    println!("## Fusion Benchmarks\n");
    println!("| Composition | Unfused Kernels | Fused Kernels | Unfused Render (us) | Fused Render (us) | Speedup | Fusion Pass (us) |");
    println!("|-------------|-----------------|---------------|--------------------:|------------------:|--------:|-----------------:|");
    for r in &results {
        let speedup = r.unfused_render_us / r.fused_render_us;
        println!("| {:<20} | {:>15} | {:>13} | {:>19.2} | {:>17.2} | {:>7.2}x | {:>16.2} |",
            r.name, r.unfused_kernels, r.fused_kernels,
            r.unfused_render_us, r.fused_render_us, speedup, r.fusion_time_us);
    }

    println!("\nFusion benchmark complete.");
}
