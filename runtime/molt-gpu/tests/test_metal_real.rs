//! Real Metal GPU execution tests.
//!
//! Compiles and runs kernels on the Metal GPU, comparing results against
//! CPU reference for correctness. Tests vector add, softmax composition,
//! and reduce_sum.
//!
//! Gated behind `#[cfg(target_os = "macos")]` — these tests are no-ops on
//! non-macOS platforms.

#[cfg(target_os = "macos")]
mod metal_real {
    use molt_gpu::device::cpu::interpret;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::dtype::DType;
    use molt_gpu::ops::PrimitiveOp;
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::{
        BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
    };
    use molt_gpu::shapetracker::ShapeTracker;

    fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    // ========================================================================
    // Vector Add (1M elements)
    // ========================================================================

    #[test]
    fn test_vector_add_1m() {
        let n = 1_000_000;
        let a: Vec<f32> = (0..n).map(|i| (i as f32) * 0.001).collect();
        let b: Vec<f32> = (0..n).map(|i| (n as f32 - i as f32) * 0.001).collect();

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
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
            spec: None, vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        let b_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&a_buf, &f32_to_bytes(&a)).unwrap();
        device.copy_in(&b_buf, &f32_to_bytes(&b)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &a_buf, &b_buf],
                [n as u32, 1, 1],
                [256, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        // Compare: should be exact match for addition
        let mut max_diff = 0.0f32;
        for i in 0..n {
            let diff = (metal_result[i] - cpu_result[i]).abs();
            max_diff = max_diff.max(diff);
        }
        assert!(
            max_diff < 1e-5,
            "Vector add 1M: max diff = {} (expected < 1e-5)",
            max_diff
        );
        println!("Vector add 1M: PASS (max diff = {:.2e})", max_diff);
    }

    // ========================================================================
    // Softmax composition on Metal
    // ========================================================================

    #[test]
    fn test_softmax_metal_vs_cpu() {
        let n = 1024;
        let x: Vec<f32> = (0..n).map(|i| (i as f32) * 0.01 - 5.0).collect();
        let x_bytes = f32_to_bytes(&x);

        // --- CPU reference softmax ---
        let cpu_result = cpu_softmax(&x);

        // --- Metal softmax ---
        let device = MetalDevice::new().expect("Metal device required");

        // Step 1: ReduceMax
        let k1 = FusedKernel {
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
            local: [1, 1, 1],
            spec: None, vectorize_width: 1,
        };

        let max_buf = device.alloc(4).unwrap();
        let x_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&x_buf, &x_bytes).unwrap();

        let msl1 = MslRenderer.render(&k1);
        let prog1 = device.compile(&msl1, "molt_kernel").unwrap();
        device.exec(&prog1, &[&max_buf, &x_buf], [1, 1, 1], [1, 1, 1]).unwrap();
        device.synchronize().unwrap();

        let mut max_bytes = [0u8; 4];
        device.copy_out(&max_buf, &mut max_bytes).unwrap();
        let max_val = f32::from_le_bytes(max_bytes);

        // Step 2: exp(x - max) via fused sub + mul(log2e) + exp2
        let log2_e = std::f64::consts::LOG2_E;
        let k2 = FusedKernel {
            ops: vec![
                FusedOp { op: PrimitiveOp::Sub, srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: max_val as f64, dtype: DType::Float32 }], dst_dtype: DType::Float32 },
                FusedOp { op: PrimitiveOp::Mul, srcs: vec![FusedSrc::Op(0), FusedSrc::Const { val: log2_e, dtype: DType::Float32 }], dst_dtype: DType::Float32 },
                FusedOp { op: PrimitiveOp::Exp2, srcs: vec![FusedSrc::Op(1)], dst_dtype: DType::Float32 },
            ],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
            spec: None, vectorize_width: 1,
        };

        let exp_buf = device.alloc(n * 4).unwrap();
        let msl2 = MslRenderer.render(&k2);
        let prog2 = device.compile(&msl2, "molt_kernel").unwrap();
        device.exec(&prog2, &[&exp_buf, &x_buf], [n as u32, 1, 1], [256, 1, 1]).unwrap();
        device.synchronize().unwrap();

        // Step 3: ReduceSum
        let k3 = FusedKernel {
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
            local: [1, 1, 1],
            spec: None, vectorize_width: 1,
        };

        let sum_buf = device.alloc(4).unwrap();
        let msl3 = MslRenderer.render(&k3);
        let prog3 = device.compile(&msl3, "molt_kernel").unwrap();
        device.exec(&prog3, &[&sum_buf, &exp_buf], [1, 1, 1], [1, 1, 1]).unwrap();
        device.synchronize().unwrap();

        let mut sum_bytes = [0u8; 4];
        device.copy_out(&sum_buf, &mut sum_bytes).unwrap();
        let sum_val = f32::from_le_bytes(sum_bytes);
        let inv_sum = 1.0 / sum_val;

        // Step 4: Normalize
        let k4 = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: inv_sum as f64, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [n as u32, 1, 1],
            local: [256, 1, 1],
            spec: None, vectorize_width: 1,
        };

        let out_buf = device.alloc(n * 4).unwrap();
        let msl4 = MslRenderer.render(&k4);
        let prog4 = device.compile(&msl4, "molt_kernel").unwrap();
        device.exec(&prog4, &[&out_buf, &exp_buf], [n as u32, 1, 1], [256, 1, 1]).unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        // Compare with tolerance
        let mut max_diff = 0.0f32;
        for i in 0..n {
            let diff = (metal_result[i] - cpu_result[i]).abs();
            max_diff = max_diff.max(diff);
        }
        assert!(
            max_diff < 1e-4,
            "Softmax Metal vs CPU: max diff = {} (expected < 1e-4)",
            max_diff
        );
        println!("Softmax Metal vs CPU (N={}): PASS (max diff = {:.2e})", n, max_diff);
    }

    // ========================================================================
    // ReduceSum on Metal
    // ========================================================================

    #[test]
    fn test_reduce_sum_metal_vs_cpu() {
        let n = 4096;
        let x: Vec<f32> = (0..n).map(|i| (i as f32) * 0.001).collect();

        let kernel = FusedKernel {
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
            local: [1, 1, 1],
            spec: None, vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; 4], f32_to_bytes(&x)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_sum = f32::from_le_bytes(cpu_bufs[0][0..4].try_into().unwrap());

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(4).unwrap();
        let x_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&x_buf, &f32_to_bytes(&x)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device.exec(&prog, &[&out_buf, &x_buf], [1, 1, 1], [1, 1, 1]).unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = [0u8; 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_sum = f32::from_le_bytes(result_bytes);

        let rel_err = ((metal_sum - cpu_sum) / cpu_sum).abs();
        assert!(
            rel_err < 1e-4,
            "ReduceSum Metal vs CPU: metal={} cpu={} rel_err={} (expected < 1e-4)",
            metal_sum, cpu_sum, rel_err
        );
        println!(
            "ReduceSum Metal vs CPU (N={}): PASS (metal={:.4}, cpu={:.4}, rel_err={:.2e})",
            n, metal_sum, cpu_sum, rel_err
        );
    }

    /// CPU reference softmax for comparison.
    fn cpu_softmax(x: &[f32]) -> Vec<f32> {
        let max_val = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exp_vals: Vec<f32> = x.iter().map(|&v| (v - max_val).exp()).collect();
        let sum: f32 = exp_vals.iter().sum();
        exp_vals.iter().map(|&v| v / sum).collect()
    }
}
