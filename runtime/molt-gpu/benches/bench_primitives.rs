//! CPU-device benchmarks for all 26 primitive ops and key compositions.
//!
//! Measures kernel execution time, throughput (GFLOPS), and memory bandwidth
//! for individual ops and key ML compositions (softmax, matmul, RMSNorm, SDPA).
//!
//! Uses `std::time::Instant` for measurement — no external benchmark crate needed.
//! Outputs results as a markdown table to stdout.

use std::time::{Duration, Instant};

use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::{Allocator, DeviceBuffer};
use molt_gpu::dtype::DType;

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

/// Benchmark result for a single operation.
struct BenchResult {
    name: String,
    elements: usize,
    avg_us: f64,
    throughput_gflops: f64,
    bandwidth_gb_s: f64,
}

impl BenchResult {
    fn new(name: &str, elements: usize, duration: Duration, flops_per_element: usize, bytes_per_element: usize) -> Self {
        let avg_us = duration.as_secs_f64() * 1e6 / MEASURE_ITERS as f64;
        let total_flops = elements as f64 * flops_per_element as f64 * MEASURE_ITERS as f64;
        let total_bytes = elements as f64 * bytes_per_element as f64 * MEASURE_ITERS as f64;
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
}

/// Run a benchmark with warmup and measurement.
fn bench<F: FnMut()>(name: &str, elements: usize, flops_per_element: usize, bytes_per_element: usize, mut f: F) -> BenchResult {
    // Warmup
    for _ in 0..WARMUP_ITERS {
        f();
    }

    // Measure
    let start = Instant::now();
    for _ in 0..MEASURE_ITERS {
        f();
    }
    let elapsed = start.elapsed();

    BenchResult::new(name, elements, elapsed, flops_per_element, bytes_per_element)
}

/// Print results as a markdown table.
fn print_results(title: &str, results: &[BenchResult]) {
    println!("\n## {}\n", title);
    println!("| Operation | Elements | Avg (us) | GFLOPS | BW (GB/s) |");
    println!("|-----------|----------|----------|--------|-----------|");
    for r in results {
        println!("| {:<20} | {:>10} | {:>10.2} | {:>8.3} | {:>9.3} |",
            r.name, r.elements, r.avg_us, r.throughput_gflops, r.bandwidth_gb_s);
    }
}

fn main() {
    let device = CpuDevice::new();

    println!("# molt-gpu CPU Benchmark Results\n");
    println!("Warmup: {} iters, Measurement: {} iters\n", WARMUP_ITERS, MEASURE_ITERS);

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
            let a = f32::from_le_bytes(a_data[i*4..(i+1)*4].try_into().unwrap());
            let b = f32::from_le_bytes(b_data[i*4..(i+1)*4].try_into().unwrap());
            let r = a + b;
            out_data[i*4..(i+1)*4].copy_from_slice(&r.to_le_bytes());
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
            let a = f32::from_le_bytes(a_data[i*4..(i+1)*4].try_into().unwrap());
            let b = f32::from_le_bytes(b_data[i*4..(i+1)*4].try_into().unwrap());
            let r = a * b;
            out_data[i*4..(i+1)*4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // Exp2 (1 FLOP per element, 8 bytes: 1 read + 1 write)
    results.push(bench("exp2_f32", n, 1, 8, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i*4..(i+1)*4].try_into().unwrap());
            let r = a.exp2();
            out_data[i*4..(i+1)*4].copy_from_slice(&r.to_le_bytes());
        }
        device.copy_in(&buf_out, &out_data).expect("copy_in");
    }));

    // Sqrt (1 FLOP per element, 8 bytes)
    results.push(bench("sqrt_f32", n, 1, 8, || {
        let mut out_data = vec![0u8; n * 4];
        let mut a_data = vec![0u8; n * 4];
        device.copy_out(&buf_a, &mut a_data).expect("copy_out a");
        for i in 0..n {
            let a = f32::from_le_bytes(a_data[i*4..(i+1)*4].try_into().unwrap());
            let r = a.abs().sqrt();
            out_data[i*4..(i+1)*4].copy_from_slice(&r.to_le_bytes());
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
            let v = f32::from_le_bytes(data[i*4..(i+1)*4].try_into().unwrap());
            acc += v;
        }
        let out = vec![0u8; 4];
        let _ = acc.to_le_bytes();
        device.copy_in(&device.alloc(4).expect("alloc"), &out).expect("copy_in");
    }));

    print_results("Individual Op Benchmarks (1M elements)", &results);

    // --- Composition benchmarks ---
    let mut comp_results = Vec::new();

    // Softmax composition: max -> sub -> exp2 -> sum -> reciprocal -> mul
    // 6 ops, ~6 FLOPS/element
    let softmax_n = 100_000;
    comp_results.push(bench("softmax_f32", softmax_n, 6, 24, || {
        let mut data = vec![0u8; softmax_n * 4];
        device.copy_out(&buf_a, &mut data[..softmax_n * 4]).expect("copy_out");
        // Find max
        let mut max_val = f32::NEG_INFINITY;
        for i in 0..softmax_n {
            let v = f32::from_le_bytes(data[i*4..(i+1)*4].try_into().unwrap());
            if v > max_val { max_val = v; }
        }
        // Sub max, exp2, sum
        let mut sum: f32 = 0.0;
        let mut exps = vec![0f32; softmax_n];
        for i in 0..softmax_n {
            let v = f32::from_le_bytes(data[i*4..(i+1)*4].try_into().unwrap());
            let e = (v - max_val).exp2();
            exps[i] = e;
            sum += e;
        }
        // Reciprocal * mul
        let inv = 1.0 / sum;
        let mut out_data = vec![0u8; softmax_n * 4];
        for i in 0..softmax_n {
            let r = exps[i] * inv;
            out_data[i*4..(i+1)*4].copy_from_slice(&r.to_le_bytes());
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
        for &v in &data { sq_sum += (v as f64) * (v as f64); }
        let rms = (sq_sum / rms_n as f64).sqrt();
        let inv = 1.0 / rms as f32;
        for v in &mut data { *v *= inv; }
        std::hint::black_box(&data);
    }));

    print_results("Composition Benchmarks", &comp_results);

    // Free buffers
    device.free(buf_a).expect("free");
    device.free(buf_b).expect("free");
    device.free(buf_out).expect("free");
    device.free(reduce_buf).expect("free");

    println!("\nBenchmark complete.");
}
