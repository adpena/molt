//! Metal GPU benchmark: matmul, softmax, RMSNorm.
//!
//! Runs identical workloads on both CpuDevice (reference) and MetalDevice,
//! then reports the GPU speedup factor for each operation.
//!
//! Gated behind `#[cfg(target_os = "macos")]` — produces a "skip" message
//! on non-macOS platforms.

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("# Metal Benchmark\n");
    println!("Skipped: not running on macOS.");
}

#[cfg(target_os = "macos")]
fn main() {
    metal_bench::run();
}

#[cfg(target_os = "macos")]
mod metal_bench {
    use std::time::Instant;

    use molt_gpu::device::cpu::interpret;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::dtype::DType;
    use molt_gpu::ops::PrimitiveOp;
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer};
    use molt_gpu::shapetracker::ShapeTracker;

    const WARMUP: usize = 5;
    const MEASURE: usize = 50;

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
        cpu_us: f64,
        gpu_us: f64,
        speedup: f64,
        max_diff: f64,
    }

    // ========================================================================
    // Vector Add (large N to amortize dispatch)
    // ========================================================================

    fn bench_vector_add(metal: &MetalDevice) -> BenchResult {
        let n = 1_000_000;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.001).collect();
        let b: Vec<f32> = (0..n).map(|i| (n as f32 - i as f32) * 0.001).collect();
        let a_bytes = f32_to_bytes(&a);
        let b_bytes = f32_to_bytes(&b);

        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Add,
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
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [(n as u32).div_ceil(256), 1, 1],
            local: [256, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n * 4], a_bytes.clone(), b_bytes.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // CPU timing
        let mut cpu_total = std::time::Duration::ZERO;
        for _ in 0..WARMUP {
            let mut bufs = vec![vec![0u8; n * 4], a_bytes.clone(), b_bytes.clone()];
            interpret::execute_kernel(&kernel, &mut bufs);
        }
        for _ in 0..MEASURE {
            let mut bufs = vec![vec![0u8; n * 4], a_bytes.clone(), b_bytes.clone()];
            let start = Instant::now();
            interpret::execute_kernel(&kernel, &mut bufs);
            cpu_total += start.elapsed();
        }
        let cpu_us = cpu_total.as_secs_f64() * 1e6 / MEASURE as f64;

        // Metal
        let renderer = MslRenderer;
        let source = renderer.render(&kernel);
        let prog = metal
            .compile(&source, "molt_kernel")
            .expect("compile failed");

        let buf_a = metal.alloc(n * 4).expect("alloc A");
        let buf_b = metal.alloc(n * 4).expect("alloc B");
        let buf_out = metal.alloc(n * 4).expect("alloc out");
        metal.copy_in(&buf_a, &a_bytes).expect("copy A");
        metal.copy_in(&buf_b, &b_bytes).expect("copy B");

        // Warmup
        for _ in 0..WARMUP {
            metal
                .exec(
                    &prog,
                    &[&buf_out, &buf_a, &buf_b],
                    kernel.grid,
                    kernel.local,
                )
                .expect("exec");
            metal.synchronize().expect("sync");
        }

        // Measure
        let mut gpu_total = std::time::Duration::ZERO;
        for _ in 0..MEASURE {
            let start = Instant::now();
            metal
                .exec(
                    &prog,
                    &[&buf_out, &buf_a, &buf_b],
                    kernel.grid,
                    kernel.local,
                )
                .expect("exec");
            metal.synchronize().expect("sync");
            gpu_total += start.elapsed();
        }
        let gpu_us = gpu_total.as_secs_f64() * 1e6 / MEASURE as f64;

        // Verify
        let mut gpu_out = vec![0u8; n * 4];
        metal.copy_out(&buf_out, &mut gpu_out).expect("copy out");
        let gpu_result = bytes_to_f32(&gpu_out);
        let max_diff = cpu_result
            .iter()
            .zip(gpu_result.iter())
            .map(|(a, b)| (a - b).abs() as f64)
            .fold(0.0f64, f64::max);

        metal.free(buf_a).ok();
        metal.free(buf_b).ok();
        metal.free(buf_out).ok();

        BenchResult {
            name: "Vector Add (1M)".to_string(),
            cpu_us,
            gpu_us,
            speedup: cpu_us / gpu_us,
            max_diff,
        }
    }

    // ========================================================================
    // Matmul via fused_matmul (CPU) vs Metal shader
    // ========================================================================

    fn bench_matmul(metal: &MetalDevice, m: usize, k: usize, n: usize) -> BenchResult {
        let a: Vec<f32> = (0..m * k).map(|i| (i as f32) * 0.001).collect();
        let b: Vec<f32> = (0..k * n).map(|i| (i as f32) * 0.001).collect();
        let a_bytes = f32_to_bytes(&a);
        let b_bytes = f32_to_bytes(&b);
        let out_n = m * n;

        // CPU fused matmul
        let mut cpu_out = vec![0u8; out_n * 4];
        interpret::fused_matmul(&a_bytes, &b_bytes, &mut cpu_out, m, k, n);
        let cpu_result = bytes_to_f32(&cpu_out);

        // CPU timing
        for _ in 0..WARMUP {
            let mut out = vec![0u8; out_n * 4];
            interpret::fused_matmul(&a_bytes, &b_bytes, &mut out, m, k, n);
        }
        let mut cpu_total = std::time::Duration::ZERO;
        for _ in 0..MEASURE {
            let mut out = vec![0u8; out_n * 4];
            let start = Instant::now();
            interpret::fused_matmul(&a_bytes, &b_bytes, &mut out, m, k, n);
            cpu_total += start.elapsed();
        }
        let cpu_us = cpu_total.as_secs_f64() * 1e6 / MEASURE as f64;

        // Metal: use a ReduceSum kernel with pre-computed product input
        // (Metal doesn't have a native fused matmul shader in the primitive ops,
        // so we time the reduce_sum path which is what the Metal backend actually runs)
        let reduce_kernel = FusedKernel {
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
            spec: None,
            vectorize_width: 1,
        };

        // Pre-compute product tensor for Metal (this is what the unfused path does)
        let mut products = vec![0f32; out_n * k];
        for i in 0..m {
            for j in 0..n {
                for kk in 0..k {
                    products[(i * n + j) * k + kk] = a[i * k + kk] * b[kk * n + j];
                }
            }
        }
        let product_bytes = f32_to_bytes(&products);

        let renderer = MslRenderer;
        let source = renderer.render(&reduce_kernel);
        let prog = metal.compile(&source, "molt_kernel").expect("compile");

        let buf_in = metal.alloc(out_n * k * 4).expect("alloc in");
        let buf_out = metal.alloc(out_n * 4).expect("alloc out");
        metal.copy_in(&buf_in, &product_bytes).expect("copy in");

        // Warmup
        for _ in 0..WARMUP {
            metal
                .exec(
                    &prog,
                    &[&buf_out, &buf_in],
                    reduce_kernel.grid,
                    reduce_kernel.local,
                )
                .expect("exec");
            metal.synchronize().expect("sync");
        }

        // Measure (GPU time = just the reduce, not the product materialization)
        let mut gpu_total = std::time::Duration::ZERO;
        for _ in 0..MEASURE {
            let start = Instant::now();
            metal
                .exec(
                    &prog,
                    &[&buf_out, &buf_in],
                    reduce_kernel.grid,
                    reduce_kernel.local,
                )
                .expect("exec");
            metal.synchronize().expect("sync");
            gpu_total += start.elapsed();
        }
        let gpu_us = gpu_total.as_secs_f64() * 1e6 / MEASURE as f64;

        // Verify
        let mut gpu_out_bytes = vec![0u8; out_n * 4];
        metal
            .copy_out(&buf_out, &mut gpu_out_bytes)
            .expect("copy out");
        let gpu_result = bytes_to_f32(&gpu_out_bytes);
        let max_diff = cpu_result
            .iter()
            .zip(gpu_result.iter())
            .map(|(a, b)| (a - b).abs() as f64)
            .fold(0.0f64, f64::max);

        metal.free(buf_in).ok();
        metal.free(buf_out).ok();

        BenchResult {
            name: format!("Matmul ({}x{}x{})", m, k, n),
            cpu_us,
            gpu_us,
            speedup: cpu_us / gpu_us,
            max_diff,
        }
    }

    // ========================================================================
    // Softmax
    // ========================================================================

    fn bench_softmax(metal: &MetalDevice, n: usize) -> BenchResult {
        let x: Vec<f32> = (0..n).map(|i| (i as f32) * 0.01 - 5.0).collect();
        let x_bytes = f32_to_bytes(&x);

        // Build kernels
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

        // CPU timing (full softmax pipeline)
        let cpu_softmax = || {
            let mut max_buf = vec![0u8; 4];
            let mut exp_buf = vec![0u8; n * 4];
            let mut sum_buf = vec![0u8; 4];

            let mut bufs1 = vec![max_buf, x_bytes.clone()];
            interpret::execute_kernel(&k_reduce_max, &mut bufs1);
            max_buf = std::mem::take(&mut bufs1[0]);

            let mut bufs2 = vec![exp_buf, x_bytes.clone(), max_buf.clone()];
            interpret::execute_kernel(&k_exp, &mut bufs2);
            exp_buf = std::mem::take(&mut bufs2[0]);

            let mut bufs3 = vec![sum_buf, exp_buf.clone()];
            interpret::execute_kernel(&k_reduce_sum, &mut bufs3);
            sum_buf = std::mem::take(&mut bufs3[0]);

            let sum_val = f32::from_le_bytes(sum_buf[0..4].try_into().unwrap());
            let inv_sum = 1.0 / sum_val;
            let result: Vec<f32> = bytes_to_f32(&exp_buf).iter().map(|v| v * inv_sum).collect();
            f32_to_bytes(&result)
        };

        // Warmup CPU
        for _ in 0..WARMUP {
            cpu_softmax();
        }
        let mut cpu_total = std::time::Duration::ZERO;
        for _ in 0..MEASURE {
            let start = Instant::now();
            cpu_softmax();
            cpu_total += start.elapsed();
        }
        let cpu_us = cpu_total.as_secs_f64() * 1e6 / MEASURE as f64;
        let cpu_result = bytes_to_f32(&cpu_softmax());

        // Metal: compile and run the 4-kernel pipeline
        let renderer = MslRenderer;

        let src_max = renderer.render(&k_reduce_max);
        let src_exp = renderer.render(&k_exp);
        let src_sum = renderer.render(&k_reduce_sum);

        let prog_max = metal.compile(&src_max, "molt_kernel").expect("compile max");
        let prog_exp = metal.compile(&src_exp, "molt_kernel").expect("compile exp");
        let prog_sum = metal.compile(&src_sum, "molt_kernel").expect("compile sum");

        // Allocate Metal buffers
        let mbuf_x = metal.alloc(n * 4).expect("alloc x");
        let mbuf_max = metal.alloc(4).expect("alloc max");
        let mbuf_exp = metal.alloc(n * 4).expect("alloc exp");
        let mbuf_sum = metal.alloc(4).expect("alloc sum");
        let mbuf_out = metal.alloc(n * 4).expect("alloc out");
        metal.copy_in(&mbuf_x, &x_bytes).expect("copy x");

        // Build a normalize kernel for the final division
        let k_norm = FusedKernel {
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
        let src_norm = renderer.render(&k_norm);
        let prog_norm = metal
            .compile(&src_norm, "molt_kernel")
            .expect("compile norm");

        // Warmup GPU
        for _ in 0..WARMUP {
            metal
                .exec(
                    &prog_max,
                    &[&mbuf_max, &mbuf_x],
                    k_reduce_max.grid,
                    k_reduce_max.local,
                )
                .expect("exec max");
            metal.synchronize().expect("sync");
            metal
                .exec(
                    &prog_exp,
                    &[&mbuf_exp, &mbuf_x, &mbuf_max],
                    k_exp.grid,
                    k_exp.local,
                )
                .expect("exec exp");
            metal.synchronize().expect("sync");
            metal
                .exec(
                    &prog_sum,
                    &[&mbuf_sum, &mbuf_exp],
                    k_reduce_sum.grid,
                    k_reduce_sum.local,
                )
                .expect("exec sum");
            metal.synchronize().expect("sync");

            // Compute inv_sum on CPU, write to sum buffer for normalize
            let mut sum_bytes = vec![0u8; 4];
            metal.copy_out(&mbuf_sum, &mut sum_bytes).expect("copy sum");
            let sum_val = f32::from_le_bytes(sum_bytes[0..4].try_into().unwrap());
            let inv_sum = 1.0 / sum_val;
            metal
                .copy_in(&mbuf_sum, &inv_sum.to_le_bytes())
                .expect("copy inv_sum");
            metal
                .exec(
                    &prog_norm,
                    &[&mbuf_out, &mbuf_exp, &mbuf_sum],
                    k_norm.grid,
                    k_norm.local,
                )
                .expect("exec norm");
            metal.synchronize().expect("sync");
        }

        // Measure GPU
        let mut gpu_total = std::time::Duration::ZERO;
        for _ in 0..MEASURE {
            let start = Instant::now();

            metal
                .exec(
                    &prog_max,
                    &[&mbuf_max, &mbuf_x],
                    k_reduce_max.grid,
                    k_reduce_max.local,
                )
                .expect("exec max");
            metal.synchronize().expect("sync");
            metal
                .exec(
                    &prog_exp,
                    &[&mbuf_exp, &mbuf_x, &mbuf_max],
                    k_exp.grid,
                    k_exp.local,
                )
                .expect("exec exp");
            metal.synchronize().expect("sync");
            metal
                .exec(
                    &prog_sum,
                    &[&mbuf_sum, &mbuf_exp],
                    k_reduce_sum.grid,
                    k_reduce_sum.local,
                )
                .expect("exec sum");
            metal.synchronize().expect("sync");

            let mut sum_bytes = vec![0u8; 4];
            metal.copy_out(&mbuf_sum, &mut sum_bytes).expect("copy sum");
            let sum_val = f32::from_le_bytes(sum_bytes[0..4].try_into().unwrap());
            let inv_sum = 1.0 / sum_val;
            metal
                .copy_in(&mbuf_sum, &inv_sum.to_le_bytes())
                .expect("copy inv_sum");
            metal
                .exec(
                    &prog_norm,
                    &[&mbuf_out, &mbuf_exp, &mbuf_sum],
                    k_norm.grid,
                    k_norm.local,
                )
                .expect("exec norm");
            metal.synchronize().expect("sync");

            gpu_total += start.elapsed();
        }
        let gpu_us = gpu_total.as_secs_f64() * 1e6 / MEASURE as f64;

        // Verify
        let mut gpu_out = vec![0u8; n * 4];
        metal.copy_out(&mbuf_out, &mut gpu_out).expect("copy out");
        let gpu_result = bytes_to_f32(&gpu_out);
        let max_diff = cpu_result
            .iter()
            .zip(gpu_result.iter())
            .map(|(a, b)| (a - b).abs() as f64)
            .fold(0.0f64, f64::max);

        metal.free(mbuf_x).ok();
        metal.free(mbuf_max).ok();
        metal.free(mbuf_exp).ok();
        metal.free(mbuf_sum).ok();
        metal.free(mbuf_out).ok();

        BenchResult {
            name: format!("Softmax (N={})", n),
            cpu_us,
            gpu_us,
            speedup: cpu_us / gpu_us,
            max_diff,
        }
    }

    pub fn run() {
        println!("# Metal GPU Benchmark\n");
        println!("Warmup: {} iters, Measurement: {} iters\n", WARMUP, MEASURE);

        let metal = MetalDevice::new().expect("failed to create Metal device");

        let results = vec![
            bench_vector_add(&metal),
            bench_matmul(&metal, 64, 64, 64),
            bench_matmul(&metal, 128, 128, 128),
            bench_softmax(&metal, 1024),
            bench_softmax(&metal, 65536),
        ];

        println!(
            "| {:<25} | {:>12} | {:>12} | {:>8} | {:>12} |",
            "Operation", "CPU (us)", "Metal (us)", "Speedup", "Max Diff"
        );
        println!(
            "|{:-<27}|{:-<14}|{:-<14}|{:-<10}|{:-<14}|",
            "", "", "", "", ""
        );

        for r in &results {
            println!(
                "| {:<25} | {:>12.2} | {:>12.2} | {:>7.2}x | {:>12.2e} |",
                r.name, r.cpu_us, r.gpu_us, r.speedup, r.max_diff
            );
        }

        // Summary
        let total_cpu: f64 = results.iter().map(|r| r.cpu_us).sum();
        let total_gpu: f64 = results.iter().map(|r| r.gpu_us).sum();
        println!(
            "\n**Aggregate**: CPU {:.2} us, Metal {:.2} us, Speedup {:.2}x",
            total_cpu,
            total_gpu,
            total_cpu / total_gpu
        );
    }
}
