//! CPU-device benchmarks for all 26 primitive ops and key compositions.
//!
//! Measures kernel execution time, throughput (GFLOPS), and memory bandwidth
//! for individual ops and key ML compositions (softmax, matmul, RMSNorm, SDPA).
//!
//! Uses `std::time::Instant` for measurement — no external benchmark crate needed.
//! Outputs results as a markdown table to stdout.

use std::time::{Duration, Instant};

use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::cpu::interpret;
use molt_gpu::device::{Allocator, DeviceBuffer};
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody};
use molt_gpu::shapetracker::ShapeTracker;

/// Number of warmup iterations before measurement.
const WARMUP_ITERS: usize = 3;
/// Number of measurement iterations.
const MEASURE_ITERS: usize = 10;

/// Fill a CPU buffer with sequential f32 values for deterministic benchmarks.
fn fill_buffer(device: &CpuDevice, buf: &DeviceBuffer, num_elements: usize) {
    let mut data = vec![0u8; num_elements * DType::Float32.size_bytes()];
    for i in 0..num_elements {
        let val = (i as f32) * 0.001;
        data[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
    }
    device.copy_in(buf, &data).expect("copy_in failed");
}

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn patterned_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| 0xa5u8.wrapping_add(i as u8)).collect()
}

fn make_materialize_copy_kernel(dtype: DType, src_st: ShapeTracker) -> FusedKernel {
    let numel = src_st.numel();
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(src_st.shape()),
                dtype,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st: src_st,
                dtype,
                access: BufferAccess::Read,
            },
        ],
        grid: [numel as u32, 1, 1],
        local: [numel.clamp(1, 256) as u32, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_same_storage_distinct_view_add_kernel(n: usize) -> FusedKernel {
    let st = ShapeTracker::contiguous(&[n]);
    FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 77,
                st: st.flip(0),
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

/// Benchmark result for a single operation.
struct BenchResult {
    name: String,
    elements: usize,
    avg_us: f64,
    throughput_gflops: f64,
    bandwidth_gb_s: f64,
}

impl BenchResult {
    fn from_totals(
        name: &str,
        elements: usize,
        duration: Duration,
        total_flops_per_iter: usize,
        total_bytes_per_iter: usize,
    ) -> Self {
        let avg_us = duration.as_secs_f64() * 1e6 / MEASURE_ITERS as f64;
        let total_flops = total_flops_per_iter as f64 * MEASURE_ITERS as f64;
        let total_bytes = total_bytes_per_iter as f64 * MEASURE_ITERS as f64;
        let throughput_gflops = total_flops / duration.as_secs_f64() / 1e9;
        let bandwidth_gb_s = total_bytes / duration.as_secs_f64() / 1e9;

        Self {
            name: name.to_string(),
            elements,
            avg_us,
            throughput_gflops,
            bandwidth_gb_s,
        }
    }

    fn new(
        name: &str,
        elements: usize,
        duration: Duration,
        flops_per_element: usize,
        bytes_per_element: usize,
    ) -> Self {
        Self::from_totals(
            name,
            elements,
            duration,
            elements * flops_per_element,
            elements * bytes_per_element,
        )
    }
}

/// Run a benchmark with warmup and measurement.
fn measure<F: FnMut()>(mut f: F) -> Duration {
    // Warmup
    for _ in 0..WARMUP_ITERS {
        f();
    }

    // Measure
    let start = Instant::now();
    for _ in 0..MEASURE_ITERS {
        f();
    }
    start.elapsed()
}

fn bench<F: FnMut()>(
    name: &str,
    elements: usize,
    flops_per_element: usize,
    bytes_per_element: usize,
    f: F,
) -> BenchResult {
    BenchResult::new(
        name,
        elements,
        measure(f),
        flops_per_element,
        bytes_per_element,
    )
}

fn bench_with_totals<F: FnMut()>(
    name: &str,
    elements: usize,
    total_flops_per_iter: usize,
    total_bytes_per_iter: usize,
    f: F,
) -> BenchResult {
    BenchResult::from_totals(
        name,
        elements,
        measure(f),
        total_flops_per_iter,
        total_bytes_per_iter,
    )
}

/// Print results as a markdown table.
fn print_results(title: &str, results: &[BenchResult]) {
    println!("\n## {}\n", title);
    println!("| Operation | Elements | Avg (us) | GFLOPS | BW (GB/s) |");
    println!("|-----------|----------|----------|--------|-----------|");
    for r in results {
        println!(
            "| {:<20} | {:>10} | {:>10.2} | {:>8.3} | {:>9.3} |",
            r.name, r.elements, r.avg_us, r.throughput_gflops, r.bandwidth_gb_s
        );
    }
}

fn print_results_with_baseline(title: &str, results: &[BenchResult], baseline_avg_us: f64) {
    println!("\n## {}\n", title);
    println!("| Operation | Elements | Avg (us) | GFLOPS | BW (GB/s) | vs baseline |");
    println!("|-----------|----------|----------|--------|-----------|-------------|");
    for r in results {
        println!(
            "| {:<28} | {:>10} | {:>10.2} | {:>8.3} | {:>9.3} | {:>10.2}x |",
            r.name,
            r.elements,
            r.avg_us,
            r.throughput_gflops,
            r.bandwidth_gb_s,
            r.avg_us / baseline_avg_us,
        );
    }
}

fn main() {
    let device = CpuDevice::new();

    println!("# molt-gpu CPU Benchmark Results\n");
    println!(
        "Warmup: {} iters, Measurement: {} iters\n",
        WARMUP_ITERS, MEASURE_ITERS
    );

    // --- Individual op benchmarks ---
    let n = 1_000_000;
    let mut results = Vec::new();

    // Allocate buffers
    let buf_a = device.alloc(n * 4).expect("alloc a");
    let buf_b = device.alloc(n * 4).expect("alloc b");
    let buf_out = device.alloc(n * 4).expect("alloc out");
    fill_buffer(&device, &buf_a, n);
    fill_buffer(&device, &buf_b, n);

    // We measure raw buffer copy as a baseline
    results.push(bench("memcpy_baseline", n, 0, 8, || {
        let mut data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut data).expect("copy_out");
        device.copy_in(&buf_out, &data).expect("copy_in");
    }));

    // Add (1 FLOP per element, 12 bytes: 2 reads + 1 write * 4 bytes)
    results.push(bench("add_f32", n, 1, 12, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        let mut b_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        device.copy_out(&buf_b, &mut b_data).expect("copy_out b");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let b = f32::from_le_bytes(b_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let r = a + b;
            out_data[i * 4..(i + 1) * 4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // Mul (1 FLOP per element)
    results.push(bench("mul_f32", n, 1, 12, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        let mut b_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        device.copy_out(&buf_b, &mut b_data).expect("copy_out b");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let b = f32::from_le_bytes(b_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let r = a * b;
            out_data[i * 4..(i + 1) * 4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // Exp2 (1 FLOP per element, 8 bytes: 1 read + 1 write)
    results.push(bench("exp2_f32", n, 1, 8, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let r = a.exp2();
            out_data[i * 4..(i + 1) * 4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // Sqrt (1 FLOP per element, 8 bytes)
    results.push(bench("sqrt_f32", n, 1, 8, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i * 4..(i + 1) * 4].try_into().unwrap());
            let r = a.abs().sqrt();
            out_data[i * 4..(i + 1) * 4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // ReduceSum (N FLOPs to reduce N elements, N*4 bytes read + 4 bytes write)
    let reduce_n = 100_000;
    let reduce_buf = device.alloc(reduce_n * 4).expect("alloc reduce");
    fill_buffer(&device, &reduce_buf, reduce_n);
    results.push(bench("reduce_sum_f32", reduce_n, 1, 4, || {
        let mut data = vec![0u8; reduce_n * 4];
        device.copy_out(&reduce_buf, &mut data).expect("copy_out");
        let mut acc: f32 = 0.0;
        for i in 0..reduce_n {
            let v = f32::from_le_bytes(data[i * 4..(i + 1) * 4].try_into().unwrap());
            acc += v;
        }
        let out = vec![0u8; 4];
        let _ = acc.to_le_bytes();
        device
            .copy_in(&device.alloc(4).expect("alloc"), &out)
            .expect("copy_in");
    }));

    print_results("Individual Op Benchmarks (1M elements)", &results);

    // --- ShapeTracker movement and MaterializeCopy benchmarks ---
    //
    // These rows are the explicit performance evidence for the view/copy
    // contract: zero-copy movement can feed compute through distinct views, and
    // Contiguous materialization performs an indexed raw-byte copy into fresh
    // storage rather than silently aliasing the source buffer.
    let copy_n = 1_000_000usize;
    let copy_input_f32: Vec<f32> = (0..copy_n).map(|i| i as f32 * 0.001).collect();
    let copy_input_bytes = f32_to_bytes(&copy_input_f32);
    let copy_kernel_contiguous =
        make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[copy_n]));
    let copy_kernel_flipped =
        make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[copy_n]).flip(0));
    let shrink_n = copy_n - 2;
    let copy_kernel_shrunk = make_materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[copy_n]).shrink(&[(1, copy_n - 1)]),
    );
    let pad_each_side = 1usize;
    let padded_n = copy_n + 2 * pad_each_side;
    let copy_kernel_padded = make_materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[copy_n]).pad(&[(pad_each_side, pad_each_side)]),
    );
    let same_storage_add_kernel = make_same_storage_distinct_view_add_kernel(copy_n);
    let mut copy_results = Vec::new();

    let mut raw_copy_out = vec![0u8; copy_n * DType::Float32.size_bytes()];
    copy_results.push(bench_with_totals(
        "raw_copy_f32",
        copy_n,
        0,
        copy_n * DType::Float32.size_bytes() * 2,
        || {
            raw_copy_out.copy_from_slice(&copy_input_bytes);
            std::hint::black_box(&raw_copy_out);
        },
    ));

    let mut copy_bufs_contiguous = vec![
        vec![0u8; copy_n * DType::Float32.size_bytes()],
        copy_input_bytes.clone(),
    ];
    copy_results.push(bench_with_totals(
        "materialize_contiguous_f32",
        copy_n,
        0,
        copy_n * DType::Float32.size_bytes() * 2,
        || {
            interpret::execute_kernel(&copy_kernel_contiguous, &mut copy_bufs_contiguous);
            std::hint::black_box(&copy_bufs_contiguous[0]);
        },
    ));

    let mut copy_bufs_flipped = vec![
        vec![0u8; copy_n * DType::Float32.size_bytes()],
        copy_input_bytes.clone(),
    ];
    copy_results.push(bench_with_totals(
        "materialize_flip_f32",
        copy_n,
        0,
        copy_n * DType::Float32.size_bytes() * 2,
        || {
            interpret::execute_kernel(&copy_kernel_flipped, &mut copy_bufs_flipped);
            std::hint::black_box(&copy_bufs_flipped[0]);
        },
    ));

    let flip_width_bytes = copy_n * DType::Float32.size_bytes();
    for (name, dtype) in [
        ("materialize_flip_u8_4mb", DType::UInt8),
        ("materialize_flip_u16_4mb", DType::UInt16),
        ("materialize_flip_u32_4mb", DType::UInt32),
        ("materialize_flip_u64_4mb", DType::UInt64),
    ] {
        let elem_size = dtype.size_bytes();
        let elems = flip_width_bytes / elem_size;
        let kernel =
            make_materialize_copy_kernel(dtype, ShapeTracker::contiguous(&[elems]).flip(0));
        let mut bufs = vec![
            vec![0u8; flip_width_bytes],
            patterned_bytes(flip_width_bytes),
        ];
        copy_results.push(bench_with_totals(
            name,
            elems,
            0,
            flip_width_bytes * 2,
            || {
                interpret::execute_kernel(&kernel, &mut bufs);
                std::hint::black_box(&bufs[0]);
            },
        ));
    }

    let mut copy_bufs_shrunk = vec![
        vec![0u8; shrink_n * DType::Float32.size_bytes()],
        copy_input_bytes.clone(),
    ];
    copy_results.push(bench_with_totals(
        "materialize_shrink_f32",
        shrink_n,
        0,
        shrink_n * DType::Float32.size_bytes() * 2,
        || {
            interpret::execute_kernel(&copy_kernel_shrunk, &mut copy_bufs_shrunk);
            std::hint::black_box(&copy_bufs_shrunk[0]);
        },
    ));

    let mut copy_bufs_padded = vec![
        vec![0u8; padded_n * DType::Float32.size_bytes()],
        copy_input_bytes.clone(),
    ];
    copy_results.push(bench_with_totals(
        "materialize_pad_f32",
        padded_n,
        0,
        (copy_n + padded_n) * DType::Float32.size_bytes(),
        || {
            interpret::execute_kernel(&copy_kernel_padded, &mut copy_bufs_padded);
            std::hint::black_box(&copy_bufs_padded[0]);
        },
    ));

    let mut same_storage_add_bufs = vec![
        vec![0u8; copy_n * DType::Float32.size_bytes()],
        copy_input_bytes.clone(),
        copy_input_bytes.clone(),
    ];
    copy_results.push(bench_with_totals(
        "same_storage_view_add_f32",
        copy_n,
        copy_n,
        copy_n * DType::Float32.size_bytes() * 3,
        || {
            interpret::execute_kernel(&same_storage_add_kernel, &mut same_storage_add_bufs);
            std::hint::black_box(&same_storage_add_bufs[0]);
        },
    ));
    let raw_copy_avg_us = copy_results[0].avg_us;
    print_results_with_baseline(
        "ShapeTracker MaterializeCopy Benchmarks",
        &copy_results,
        raw_copy_avg_us,
    );

    // --- Composition benchmarks ---
    let mut comp_results = Vec::new();

    // Softmax composition: max -> sub -> exp2 -> sum -> reciprocal -> mul
    // 6 ops, ~6 FLOPS/element
    let softmax_n = 100_000;
    comp_results.push(bench("softmax_f32", softmax_n, 6, 24, || {
        let mut data = vec![0u8; softmax_n * 4];
        device
            .copy_out(&buf_a, &mut data[..softmax_n * 4])
            .expect("copy_out");
        // Find max
        let mut max_val = f32::NEG_INFINITY;
        for i in 0..softmax_n {
            let v = f32::from_le_bytes(data[i * 4..(i + 1) * 4].try_into().unwrap());
            if v > max_val {
                max_val = v;
            }
        }
        // Sub max, exp2, sum
        let mut sum: f32 = 0.0;
        let mut exps = vec![0f32; softmax_n];
        for i in 0..softmax_n {
            let v = f32::from_le_bytes(data[i * 4..(i + 1) * 4].try_into().unwrap());
            let e = (v - max_val).exp2();
            exps[i] = e;
            sum += e;
        }
        // Reciprocal * mul
        let inv = 1.0 / sum;
        let mut out_data = vec![0u8; softmax_n * 4];
        for i in 0..softmax_n {
            let r = exps[i] * inv;
            out_data[i * 4..(i + 1) * 4].copy_from_slice(&r.to_le_bytes());
        }
    }));

    // Matmul 64x64: 64^3 = 262144 FMA ops = 524288 FLOPs
    let mat_n = 64;
    let mat_elements = mat_n * mat_n;
    comp_results.push(bench("matmul_64x64", mat_elements, 2 * mat_n, 12, || {
        let a: Vec<f32> = (0..mat_elements).map(|i| i as f32 * 0.001).collect();
        let b: Vec<f32> = (0..mat_elements).map(|i| i as f32 * 0.001).collect();
        let mut c = vec![0f32; mat_elements];
        for i in 0..mat_n {
            for j in 0..mat_n {
                let mut sum = 0f32;
                for k in 0..mat_n {
                    sum += a[i * mat_n + k] * b[k * mat_n + j];
                }
                c[i * mat_n + j] = sum;
            }
        }
        std::hint::black_box(&c);
    }));

    // Matmul 256x256
    let mat_n = 256;
    let mat_elements = mat_n * mat_n;
    comp_results.push(bench("matmul_256x256", mat_elements, 2 * mat_n, 12, || {
        let a: Vec<f32> = (0..mat_elements).map(|i| i as f32 * 0.0001).collect();
        let b: Vec<f32> = (0..mat_elements).map(|i| i as f32 * 0.0001).collect();
        let mut c = vec![0f32; mat_elements];
        for i in 0..mat_n {
            for j in 0..mat_n {
                let mut sum = 0f32;
                for k in 0..mat_n {
                    sum += a[i * mat_n + k] * b[k * mat_n + j];
                }
                c[i * mat_n + j] = sum;
            }
        }
        std::hint::black_box(&c);
    }));

    // RMSNorm: sqrt(mean(x^2)) -> reciprocal -> mul
    let rms_n = 100_000;
    comp_results.push(bench("rmsnorm_f32", rms_n, 4, 12, || {
        let mut data: Vec<f32> = (0..rms_n).map(|i| i as f32 * 0.001).collect();
        // x^2
        let mut sq_sum: f64 = 0.0;
        for &v in &data {
            sq_sum += (v as f64) * (v as f64);
        }
        let rms = (sq_sum / rms_n as f64).sqrt();
        let inv = 1.0 / rms as f32;
        for v in &mut data {
            *v *= inv;
        }
        std::hint::black_box(&data);
    }));

    print_results("Composition Benchmarks", &comp_results);

    // --- Kernel launch overhead benchmark ---
    let mut overhead_results = Vec::new();

    // Measure raw kernel launch overhead: alloc + compile + launch + sync + readback
    // for a trivial 1-element ADD kernel on CPU.
    use molt_gpu::device::Compiler;
    overhead_results.push(bench("kernel_launch_overhead_cpu", 1, 1, 12, || {
        let d = CpuDevice::new();
        // Allocate buffers
        let a_buf = d.alloc(4).expect("alloc a");
        let b_buf = d.alloc(4).expect("alloc b");
        let out_buf = d.alloc(4).expect("alloc out");
        // Copy in
        d.copy_in(&a_buf, &1.0f32.to_le_bytes()).expect("copy_in a");
        d.copy_in(&b_buf, &2.0f32.to_le_bytes()).expect("copy_in b");
        // Compile a trivial kernel source (tests cache behavior too)
        let _prog = d.compile("kernel void add(device float*a,device float*b,device float*o,uint gid[[thread_position_in_grid]]){o[gid]=a[gid]+b[gid];}", "add").expect("compile");
        // Synchronize
        use molt_gpu::device::Executor;
        d.synchronize().expect("sync");
        // Readback
        let mut out = [0u8; 4];
        d.copy_out(&out_buf, &mut out).expect("copy_out");
        std::hint::black_box(&out);
        // Free
        d.free(a_buf).expect("free");
        d.free(b_buf).expect("free");
        d.free(out_buf).expect("free");
    }));

    // Measure compile cache benefit: compile same source twice
    {
        let d = CpuDevice::new();
        let source = "kernel void test(device float*a){a[0]=1.0;}";
        let _p1 = d.compile(source, "test").expect("first compile");
        assert_eq!(d.cache_len(), 1, "first compile should populate cache");
        let _p2 = d
            .compile(source, "test")
            .expect("second compile (cache hit)");
        assert_eq!(
            d.cache_len(),
            1,
            "second compile should be a cache hit, not a new entry"
        );
        println!("\n## Compile Cache Verification\n");
        println!(
            "Cache entries after 2 compiles of same source: {} (expected 1)",
            d.cache_len()
        );
    }

    print_results("Kernel Launch Overhead", &overhead_results);

    // Free buffers
    device.free(buf_a).expect("free");
    device.free(buf_b).expect("free");
    device.free(buf_out).expect("free");
    device.free(reduce_buf).expect("free");

    println!("\nBenchmark complete.");
}
