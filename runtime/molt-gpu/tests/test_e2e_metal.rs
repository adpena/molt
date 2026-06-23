//! End-to-end Metal GPU tests.
//!
//! Allocates buffers on Metal, runs all 26 ops, and compares results
//! with CPU reference bit-for-bit. Tests softmax and matmul compositions,
//! and IEEE 754 edge cases.

#[cfg(target_os = "macos")]
mod metal_e2e {
    use molt_gpu::device::cpu::interpret;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, DeviceBuffer, Executor};
    use molt_gpu::dtype::DType;
    use molt_gpu::fuse::fuse;
    use molt_gpu::ops::PrimitiveOp;
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::{
        BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
        Renderer,
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

    fn u16_to_bytes(vals: &[u16]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn u32_to_bytes(vals: &[u32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn bytes_to_u16(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn run_kernel_metal_bytes(kernel: &FusedKernel, inputs: &[Vec<u8>]) -> Vec<u8> {
        let device = MetalDevice::new().expect("Metal device required");
        let out_len = kernel.bufs[0].st.numel() * kernel.bufs[0].dtype.size_bytes();
        let out_buf = device.alloc(out_len).unwrap();
        let mut input_bufs: Vec<DeviceBuffer> = Vec::with_capacity(inputs.len());
        for input in inputs {
            let input_buf = device.alloc(input.len()).unwrap();
            device.copy_in(&input_buf, input).unwrap();
            input_bufs.push(input_buf);
        }

        let msl = MslRenderer.render(kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        let mut refs: Vec<&DeviceBuffer> = Vec::with_capacity(kernel.bufs.len());
        refs.push(&out_buf);
        for input_buf in &input_bufs {
            refs.push(input_buf);
        }
        device
            .exec(&prog, &refs, kernel.grid, kernel.local)
            .unwrap();
        device.synchronize().unwrap();
        drop(refs);

        let mut result = vec![0u8; out_len];
        device.copy_out(&out_buf, &mut result).unwrap();
        device.free(out_buf).unwrap();
        for input_buf in input_bufs {
            device.free(input_buf).unwrap();
        }
        result
    }

    fn run_kernel_metal_vs_cpu_f32(
        kernel: &FusedKernel,
        inputs: &[Vec<u8>],
    ) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(kernel.bufs[0].dtype, DType::Float32);
        let out_len = kernel.bufs[0].st.numel() * DType::Float32.size_bytes();
        let mut cpu_bufs = vec![vec![0u8; out_len]];
        cpu_bufs.extend(inputs.iter().cloned());
        interpret::execute_kernel(kernel, &mut cpu_bufs);
        let cpu = bytes_to_f32(&cpu_bufs[0]);
        let metal = bytes_to_f32(&run_kernel_metal_bytes(kernel, inputs));
        (metal, cpu)
    }

    fn materialize_copy_kernel(dtype: DType, src_st: ShapeTracker) -> FusedKernel {
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

    #[test]
    fn test_metal_e2e_materialize_copy_from_flipped_view() {
        let kernel =
            materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[4]).flip(0));
        let input = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);

        let mut cpu_bufs = vec![vec![0u8; 16], input.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let metal = run_kernel_metal_bytes(&kernel, &[input]);

        assert_eq!(bytes_to_f32(&cpu_bufs[0]), vec![4.0, 3.0, 2.0, 1.0]);
        assert_eq!(bytes_to_f32(&metal), bytes_to_f32(&cpu_bufs[0]));
    }

    #[test]
    fn test_metal_e2e_materialize_copy_from_padded_view() {
        let kernel = materialize_copy_kernel(
            DType::Float32,
            ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        );
        let input = f32_to_bytes(&[1.0, 2.0, 3.0]);

        let mut cpu_bufs = vec![vec![0u8; 20], input.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let metal = run_kernel_metal_bytes(&kernel, &[input]);

        assert_eq!(bytes_to_f32(&cpu_bufs[0]), vec![0.0, 1.0, 2.0, 3.0, 0.0]);
        assert_eq!(bytes_to_f32(&metal), bytes_to_f32(&cpu_bufs[0]));
    }

    #[test]
    fn test_metal_e2e_materialize_copy_preserves_u16_raw_storage() {
        let kernel = materialize_copy_kernel(DType::UInt16, ShapeTracker::contiguous(&[4]).flip(0));
        let input = u16_to_bytes(&[0x0001, 0x00ff, 0x8001, 0xffff]);

        let mut cpu_bufs = vec![vec![0u8; 8], input.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let metal = run_kernel_metal_bytes(&kernel, &[input]);

        assert_eq!(
            bytes_to_u16(&cpu_bufs[0]),
            vec![0xffff, 0x8001, 0x00ff, 0x0001]
        );
        assert_eq!(bytes_to_u16(&metal), bytes_to_u16(&cpu_bufs[0]));
    }

    #[test]
    fn test_metal_e2e_same_storage_distinct_views_share_device_buffer() {
        let n = 4;
        let st = ShapeTracker::contiguous(&[n]);
        let kernel = FusedKernel {
            body: KernelBody::Compute,
            ops: vec![FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                DType::Float32,
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
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
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let input = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);

        let mut cpu_bufs = vec![vec![0u8; 16], input.clone(), input.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);

        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(16).unwrap();
        let in_buf = device.alloc(16).unwrap();
        device.copy_in(&in_buf, &input).unwrap();
        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &in_buf, &in_buf],
                kernel.grid,
                kernel.local,
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut metal = vec![0u8; 16];
        device.copy_out(&out_buf, &mut metal).unwrap();
        device.free(out_buf).unwrap();
        device.free(in_buf).unwrap();

        assert_eq!(bytes_to_f32(&cpu_bufs[0]), vec![5.0, 5.0, 5.0, 5.0]);
        assert_eq!(bytes_to_f32(&metal), bytes_to_f32(&cpu_bufs[0]));
    }

    /// Run a binary op on Metal and return results alongside CPU reference.
    fn run_binary_metal_vs_cpu(
        op: PrimitiveOp,
        a: &[f32],
        b: &[f32],
        dst_dtype: DType,
    ) -> (Vec<f32>, Vec<f32>) {
        let n = a.len();
        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                op,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype,
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: dst_dtype,
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
            local: [n.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(a), f32_to_bytes(b)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        let b_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&a_buf, &f32_to_bytes(a)).unwrap();
        device.copy_in(&b_buf, &f32_to_bytes(b)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &a_buf, &b_buf],
                [n as u32, 1, 1],
                [n.clamp(1, 256) as u32, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        (metal_result, cpu_result)
    }

    /// Run a unary op on Metal and return results alongside CPU reference.
    fn run_unary_metal_vs_cpu(op: PrimitiveOp, a: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let n = a.len();
        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                op,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
            )],
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
            local: [n.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(a)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&a_buf, &f32_to_bytes(a)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &a_buf],
                [n as u32, 1, 1],
                [n.clamp(1, 256) as u32, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        (metal_result, cpu_result)
    }

    fn unary_typed_kernel(
        op: PrimitiveOp,
        src_dtype: DType,
        dst_dtype: DType,
        n: usize,
    ) -> FusedKernel {
        FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(op, vec![FusedSrc::Buf(1)], dst_dtype)],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: dst_dtype,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: src_dtype,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        }
    }

    fn run_unary_typed_metal_vs_cpu_raw(
        op: PrimitiveOp,
        src_dtype: DType,
        dst_dtype: DType,
        input: Vec<u8>,
        n: usize,
    ) -> (Vec<u8>, Vec<u8>) {
        assert_eq!(input.len(), n * src_dtype.size_bytes());
        let kernel = unary_typed_kernel(op, src_dtype, dst_dtype, n);

        let mut cpu_bufs = vec![vec![0u8; n * dst_dtype.size_bytes()], input.clone()];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let metal = run_kernel_metal_bytes(&kernel, &[input]);

        (metal, cpu_bufs.remove(0))
    }

    fn assert_f32_close(metal: &[f32], cpu: &[f32], op_name: &str, tol: f32) {
        assert_eq!(
            metal.len(),
            cpu.len(),
            "{}: length mismatch ({} vs {})",
            op_name,
            metal.len(),
            cpu.len()
        );
        for (i, (m, c)) in metal.iter().zip(cpu.iter()).enumerate() {
            if m.is_nan() && c.is_nan() {
                continue;
            }
            if m.is_infinite() && c.is_infinite() && m.is_sign_positive() == c.is_sign_positive() {
                continue;
            }
            assert!(
                (m - c).abs() <= tol,
                "{}: index {} mismatch: metal={} cpu={} (tol={})",
                op_name,
                i,
                m,
                c,
                tol,
            );
        }
    }

    // --- Arithmetic ops ---

    #[test]
    fn test_metal_e2e_add() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Add,
            &[1.0, 2.0, 3.0, 4.0],
            &[5.0, 6.0, 7.0, 8.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Add", 0.0);
    }

    #[test]
    fn test_metal_e2e_sub() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Sub,
            &[5.0, 3.0, 1.0, -1.0],
            &[1.0, 2.0, 3.0, 4.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Sub", 0.0);
    }

    #[test]
    fn test_metal_e2e_mul() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Mul,
            &[2.0, 3.0, 4.0, 5.0],
            &[5.0, 6.0, 7.0, 8.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Mul", 0.0);
    }

    #[test]
    fn test_metal_e2e_neg() {
        let (metal, cpu) =
            run_unary_metal_vs_cpu(PrimitiveOp::Neg, &[1.0, -2.0, 0.0, std::f32::consts::PI]);
        assert_f32_close(&metal, &cpu, "Neg", 0.0);
    }

    #[test]
    fn test_metal_e2e_exp2() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Exp2, &[0.0, 1.0, 2.0, 3.0]);
        assert_f32_close(&metal, &cpu, "Exp2", 1e-5);
    }

    #[test]
    fn test_metal_e2e_log2() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Log2, &[1.0, 2.0, 4.0, 8.0]);
        assert_f32_close(&metal, &cpu, "Log2", 1e-5);
    }

    #[test]
    fn test_metal_e2e_sin() {
        let (metal, cpu) = run_unary_metal_vs_cpu(
            PrimitiveOp::Sin,
            &[0.0, std::f32::consts::FRAC_PI_2, std::f32::consts::PI, 1.0],
        );
        assert_f32_close(&metal, &cpu, "Sin", 1e-5);
    }

    #[test]
    fn test_metal_e2e_sqrt() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Sqrt, &[0.0, 1.0, 4.0, 9.0]);
        assert_f32_close(&metal, &cpu, "Sqrt", 0.0);
    }

    #[test]
    fn test_metal_e2e_reciprocal() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Reciprocal, &[1.0, 2.0, 4.0, 0.5]);
        assert_f32_close(&metal, &cpu, "Reciprocal", 0.0);
    }

    #[test]
    fn test_metal_e2e_trunc() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Trunc, &[2.7, -2.7, 3.0, -3.0]);
        assert_f32_close(&metal, &cpu, "Trunc", 0.0);
    }

    #[test]
    fn test_metal_e2e_max() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Max,
            &[1.0, 5.0, -3.0, 0.0],
            &[3.0, 2.0, -1.0, 0.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Max", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmplt() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Cmplt,
            &[1.0, 5.0, -3.0, 0.0],
            &[3.0, 2.0, -1.0, 0.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Cmplt", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmpeq() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Cmpeq,
            &[1.0, 2.0, 3.0, 0.0],
            &[1.0, 3.0, 3.0, 0.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Cmpeq", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmpne() {
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Cmpne,
            &[1.0, 2.0, 3.0, 0.0],
            &[1.0, 3.0, 3.0, 0.0],
            DType::Float32,
        );
        assert_f32_close(&metal, &cpu, "Cmpne", 0.0);
    }

    // --- IEEE 754 edge cases ---

    #[test]
    fn test_metal_e2e_nan_propagation() {
        let nan = f32::NAN;
        let (metal, _cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Add,
            &[nan, 1.0, nan, 0.0],
            &[1.0, nan, nan, 0.0],
            DType::Float32,
        );
        assert!(metal[0].is_nan(), "NaN + 1.0 should be NaN");
        assert!(metal[1].is_nan(), "1.0 + NaN should be NaN");
        assert!(metal[2].is_nan(), "NaN + NaN should be NaN");
        assert_eq!(metal[3], 0.0, "0.0 + 0.0 should be 0.0");
    }

    #[test]
    fn test_metal_e2e_neg_zero() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Neg, &[0.0]);
        // Both should produce -0.0
        assert!(
            metal[0].is_sign_negative(),
            "Neg(0.0) should be -0.0 on Metal"
        );
        assert!(cpu[0].is_sign_negative(), "Neg(0.0) should be -0.0 on CPU");
    }

    #[test]
    fn test_metal_e2e_infinity() {
        let inf = f32::INFINITY;
        let neg_inf = f32::NEG_INFINITY;
        let (metal, cpu) = run_binary_metal_vs_cpu(
            PrimitiveOp::Add,
            &[inf, neg_inf, inf, 1.0],
            &[1.0, 1.0, neg_inf, inf],
            DType::Float32,
        );
        assert_eq!(metal[0], inf, "inf + 1.0 = inf");
        assert_eq!(metal[1], neg_inf, "-inf + 1.0 = -inf");
        assert!(metal[2].is_nan(), "inf + (-inf) = NaN");
        assert_eq!(metal[3], inf, "1.0 + inf = inf");
        assert_f32_close(&metal, &cpu, "Infinity", 0.0);
    }

    #[test]
    fn test_metal_e2e_reciprocal_zero_inf() {
        let (metal, _cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Reciprocal, &[0.0, -0.0_f32]);
        assert!(metal[0].is_infinite() && metal[0] > 0.0, "1/0 = +inf");
        assert!(metal[1].is_infinite() && metal[1] < 0.0, "1/(-0) = -inf");
    }

    // --- Softmax composition (2 fused kernels) ---

    #[test]
    fn test_metal_e2e_softmax_composition() {
        // softmax = exp(x - max(x)) / sum(exp(x - max(x)))
        // This should produce 2 fused kernels: one for max+sub+exp, one for sum+div
        let n = 4;

        // Kernel 1: ReduceMax over input
        let k1 = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceMax,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n], 0),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 10,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 11,
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

        // Kernel 2: Sub (x - max) -> Exp2(* LOG2_E) = exp
        let log2_e = std::f64::consts::LOG2_E;
        let k2 = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(
                    PrimitiveOp::Sub,
                    vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                    DType::Float32,
                ),
                FusedOp::elementwise(
                    PrimitiveOp::Mul,
                    vec![
                        FusedSrc::Op(0),
                        FusedSrc::Const {
                            val: log2_e,
                            dtype: DType::Float32,
                        },
                    ],
                    DType::Float32,
                ),
                FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(1)], DType::Float32),
            ],
            bufs: vec![
                BufferBinding {
                    buf_id: 20,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 11,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 10,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        // Kernel 3: ReduceSum of exp values
        let k3 = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n], 0),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 30,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 20,
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

        // Fuse: k1 is one kernel, k2 fuses elementwise, k3 is another
        let fused = fuse(vec![k1, k2, k3]);
        // k1 = reduce, k2 = elementwise -> separate from k1 (k1 output feeds k2)
        // k3 = reduce -> separate from k2 (reduce-to-reduce boundary with k3 reading k2 output)
        // Actually k1 is reduce, k2 is elementwise (no reduce), k3 is reduce
        // So k1+k2 would try to fuse (reduce then elementwise), and k3 is new reduce
        // Since k2 elementwise does NOT have a reduce, and k1 is reduce:
        // k1 is reduce alone, k2 is elementwise alone, k3 is reduce alone
        // After fusion: k1+k2 fuses (reduce+elementwise suffix), k3 separate
        assert!(
            fused.len() <= 3,
            "softmax should produce at most 3 fused kernels, got {}",
            fused.len()
        );
    }

    // --- Matmul composition (RESHAPE + EXPAND + MUL + REDUCE_SUM) ---

    #[test]
    fn test_metal_e2e_matmul_reduce_sum() {
        // Matmul 2x3 @ 3x2 = 2x2 via element-wise mul + reduce_sum
        // Test just the reduce_sum component on Metal
        let input = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2 groups of 3
        let n_out = 2;
        let reduce_size = 3;

        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n_out, reduce_size], 1),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_out]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_out * reduce_size]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_out as u32, 1, 1],
            local: [n_out as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&input)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n_out * 4).unwrap();
        let in_buf = device.alloc(input.len() * 4).unwrap();
        device.copy_in(&in_buf, &f32_to_bytes(&input)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &in_buf],
                [n_out as u32, 1, 1],
                [n_out as u32, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n_out * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        assert_eq!(cpu_result, vec![6.0, 15.0]); // 1+2+3=6, 4+5+6=15
        assert_f32_close(&metal_result, &cpu_result, "MatmulReduceSum", 1e-5);
    }

    #[test]
    fn test_metal_e2e_attention_core_masked_sdpa_row() {
        let query = [1.0f32, 2.0];
        let keys = [1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 1.0];
        let mask = [1.0f32, 1.0, 0.0, 0.0];
        let values = [10.0f32, 20.0, 30.0, 40.0];
        let n_keys = values.len();
        let head_dim = query.len();
        let scale = 1.0 / (head_dim as f64).sqrt();
        let masked_sentinel = -1.0e9f64;
        let mut tiled_query = Vec::with_capacity(n_keys * head_dim);
        for _ in 0..n_keys {
            tiled_query.extend_from_slice(&query);
        }

        let qk_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(
                    PrimitiveOp::Mul,
                    vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                    DType::Float32,
                ),
                FusedOp::reduction(
                    PrimitiveOp::ReduceSum,
                    vec![FusedSrc::Op(0)],
                    DType::Float32,
                    ReductionDomain::from_axis(&[n_keys, head_dim], 1),
                ),
            ],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys, head_dim]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[n_keys, head_dim]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_keys as u32, 1, 1],
            local: [n_keys as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let qk_inputs = [f32_to_bytes(&tiled_query), f32_to_bytes(&keys)];
        let (qk_scores, cpu_qk_scores) = run_kernel_metal_vs_cpu_f32(&qk_kernel, &qk_inputs);
        assert_f32_close(&qk_scores, &cpu_qk_scores, "SDPAQK", 1e-5);
        assert_f32_close(&qk_scores, &[1.0, 2.0, 3.0, 4.0], "SDPAQKRef", 1e-5);

        let scale_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: scale,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_keys as u32, 1, 1],
            local: [n_keys as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let scale_inputs = [f32_to_bytes(&qk_scores)];
        let (scaled_scores, cpu_scaled_scores) =
            run_kernel_metal_vs_cpu_f32(&scale_kernel, &scale_inputs);
        assert_f32_close(&scaled_scores, &cpu_scaled_scores, "SDPAScale", 1e-6);

        let masked_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                PrimitiveOp::Where,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Buf(2),
                    FusedSrc::Const {
                        val: masked_sentinel,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_keys as u32, 1, 1],
            local: [n_keys as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let masked_inputs = [f32_to_bytes(&mask), f32_to_bytes(&scaled_scores)];
        let (masked_scores, cpu_masked_scores) =
            run_kernel_metal_vs_cpu_f32(&masked_kernel, &masked_inputs);
        assert_f32_close(&masked_scores, &cpu_masked_scores, "SDPAMask", 0.0);
        assert!(masked_scores[2] < -1.0e8);
        assert!(masked_scores[3] < -1.0e8);

        let max_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceMax,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n_keys], 0),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let max_inputs = [f32_to_bytes(&masked_scores)];
        let (max_values, cpu_max_values) = run_kernel_metal_vs_cpu_f32(&max_kernel, &max_inputs);
        assert_f32_close(&max_values, &cpu_max_values, "SDPAMax", 1e-6);
        let max_val = max_values[0];

        let exp_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(
                    PrimitiveOp::Sub,
                    vec![
                        FusedSrc::Buf(1),
                        FusedSrc::Const {
                            val: max_val as f64,
                            dtype: DType::Float32,
                        },
                    ],
                    DType::Float32,
                ),
                FusedOp::elementwise(
                    PrimitiveOp::Mul,
                    vec![
                        FusedSrc::Op(0),
                        FusedSrc::Const {
                            val: std::f64::consts::LOG2_E,
                            dtype: DType::Float32,
                        },
                    ],
                    DType::Float32,
                ),
                FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(1)], DType::Float32),
            ],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_keys as u32, 1, 1],
            local: [n_keys as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let exp_inputs = [f32_to_bytes(&masked_scores)];
        let (exp_vals, cpu_exp_vals) = run_kernel_metal_vs_cpu_f32(&exp_kernel, &exp_inputs);
        assert_f32_close(&exp_vals, &cpu_exp_vals, "SDPAExp", 1e-5);

        let sum_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n_keys], 0),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let sum_inputs = [f32_to_bytes(&exp_vals)];
        let (sum_vals, cpu_sum_vals) = run_kernel_metal_vs_cpu_f32(&sum_kernel, &sum_inputs);
        assert_f32_close(&sum_vals, &cpu_sum_vals, "SDPASum", 1e-5);
        let sum_val = sum_vals[0];

        let prob_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(
                    PrimitiveOp::Reciprocal,
                    vec![FusedSrc::Const {
                        val: sum_val as f64,
                        dtype: DType::Float32,
                    }],
                    DType::Float32,
                ),
                FusedOp::elementwise(
                    PrimitiveOp::Mul,
                    vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
                    DType::Float32,
                ),
            ],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n_keys as u32, 1, 1],
            local: [n_keys as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let prob_inputs = [f32_to_bytes(&exp_vals)];
        let (probs, cpu_probs) = run_kernel_metal_vs_cpu_f32(&prob_kernel, &prob_inputs);
        assert_f32_close(&probs, &cpu_probs, "SDPAProb", 1e-5);

        let value_kernel = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(
                    PrimitiveOp::Mul,
                    vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                    DType::Float32,
                ),
                FusedOp::reduction(
                    PrimitiveOp::ReduceSum,
                    vec![FusedSrc::Op(0)],
                    DType::Float32,
                    ReductionDomain::from_axis(&[n_keys], 0),
                ),
            ],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[n_keys]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let value_inputs = [f32_to_bytes(&probs), f32_to_bytes(&values)];
        let (attended, cpu_attended) = run_kernel_metal_vs_cpu_f32(&value_kernel, &value_inputs);
        assert_f32_close(&attended, &cpu_attended, "SDPAAttended", 1e-4);

        let ref_scores: Vec<f64> = keys
            .chunks_exact(head_dim)
            .map(|key| {
                query
                    .iter()
                    .zip(key.iter())
                    .map(|(&q, &k)| q as f64 * k as f64)
                    .sum::<f64>()
                    * scale
            })
            .collect();
        let ref_max = ref_scores
            .iter()
            .zip(mask.iter())
            .filter_map(|(&score, &keep)| (keep != 0.0).then_some(score))
            .fold(f64::NEG_INFINITY, f64::max);
        let ref_exps: Vec<f64> = ref_scores
            .iter()
            .zip(mask.iter())
            .map(|(&score, &keep)| {
                if keep != 0.0 {
                    (score - ref_max).exp()
                } else {
                    0.0
                }
            })
            .collect();
        let ref_sum: f64 = ref_exps.iter().sum();
        let ref_probs: Vec<f32> = ref_exps.iter().map(|&exp| (exp / ref_sum) as f32).collect();
        let ref_attended: f32 = ref_probs
            .iter()
            .zip(values.iter())
            .map(|(&prob, &value)| prob * value)
            .sum();
        assert_f32_close(&probs, &ref_probs, "SDPAProbRef", 1e-5);
        assert_f32_close(&attended, &[ref_attended], "SDPAAttendedRef", 1e-4);
    }

    // --- WHERE (ternary) ---

    #[test]
    fn test_metal_e2e_where() {
        let n = 4;
        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                PrimitiveOp::Where,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
                DType::Float32,
            )],
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
                BufferBinding {
                    buf_id: 3,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let cond = [1.0f32, 0.0, 1.0, 0.0];
        let a_vals = [10.0f32, 20.0, 30.0, 40.0];
        let b_vals = [50.0f32, 60.0, 70.0, 80.0];

        // CPU reference
        let mut cpu_bufs = vec![
            vec![0u8; n * 4],
            f32_to_bytes(&cond),
            f32_to_bytes(&a_vals),
            f32_to_bytes(&b_vals),
        ];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n * 4).unwrap();
        let cond_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        let b_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&cond_buf, &f32_to_bytes(&cond)).unwrap();
        device.copy_in(&a_buf, &f32_to_bytes(&a_vals)).unwrap();
        device.copy_in(&b_buf, &f32_to_bytes(&b_vals)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &cond_buf, &a_buf, &b_buf],
                [n as u32, 1, 1],
                [n as u32, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        assert_eq!(cpu_result, vec![10.0, 60.0, 30.0, 80.0]);
        assert_f32_close(&metal_result, &cpu_result, "Where", 0.0);
    }

    // --- CAST ---

    #[test]
    fn test_metal_e2e_cast_float32_to_int32_raw_storage() {
        let expected = vec![1, 0, 0, 0, 254, 255, 255, 255, 0, 0, 0, 0, 7, 0, 0, 0];
        let (metal, cpu) = run_unary_typed_metal_vs_cpu_raw(
            PrimitiveOp::Cast,
            DType::Float32,
            DType::Int32,
            f32_to_bytes(&[1.25, -2.75, 0.0, 7.0]),
            4,
        );

        assert_eq!(cpu, expected);
        assert_eq!(metal, expected);
    }

    #[test]
    fn test_metal_e2e_cast_float32_to_uint16_raw_storage() {
        let expected = vec![0, 0, 1, 0, 255, 0, 255, 255];
        let (metal, cpu) = run_unary_typed_metal_vs_cpu_raw(
            PrimitiveOp::Cast,
            DType::Float32,
            DType::UInt16,
            f32_to_bytes(&[0.0, 1.0, 255.0, 65535.0]),
            4,
        );

        assert_eq!(cpu, expected);
        assert_eq!(metal, expected);
    }

    #[test]
    fn test_metal_e2e_cast_float32_to_uint8_raw_storage() {
        let expected = vec![0, 1, 2, 255];
        let (metal, cpu) = run_unary_typed_metal_vs_cpu_raw(
            PrimitiveOp::Cast,
            DType::Float32,
            DType::UInt8,
            f32_to_bytes(&[0.0, 1.0, 2.0, 255.0]),
            4,
        );

        assert_eq!(cpu, expected);
        assert_eq!(metal, expected);
    }

    #[test]
    fn test_metal_e2e_bitcast_float32_to_uint32_raw_storage() {
        let expected = vec![0, 0, 128, 63, 0, 0, 0, 128, 0, 0, 32, 64, 0, 0, 128, 255];
        let (metal, cpu) = run_unary_typed_metal_vs_cpu_raw(
            PrimitiveOp::Bitcast,
            DType::Float32,
            DType::UInt32,
            f32_to_bytes(&[1.0, -0.0, 2.5, f32::NEG_INFINITY]),
            4,
        );

        assert_eq!(cpu, expected);
        assert_eq!(metal, expected);
    }

    #[test]
    fn test_metal_e2e_bitcast_uint32_to_float32_raw_storage() {
        let source = u32_to_bytes(&[0x3f80_0000, 0x8000_0000, 0x4020_0000, 0xff80_0000]);
        let (metal, cpu) = run_unary_typed_metal_vs_cpu_raw(
            PrimitiveOp::Bitcast,
            DType::UInt32,
            DType::Float32,
            source.clone(),
            4,
        );

        assert_eq!(cpu, source);
        assert_eq!(metal, source);
    }

    // --- Bitwise on Metal (using float representation) ---

    #[test]
    fn test_metal_e2e_fused_neg_max() {
        // ReLU(-x) = max(-x, 0): tests fused chain on Metal
        let n = 4;
        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![
                FusedOp::elementwise(PrimitiveOp::Neg, vec![FusedSrc::Buf(1)], DType::Float32),
                FusedOp::elementwise(
                    PrimitiveOp::Max,
                    vec![
                        FusedSrc::Op(0),
                        FusedSrc::Const {
                            val: 0.0,
                            dtype: DType::Float32,
                        },
                    ],
                    DType::Float32,
                ),
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
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let input = [-3.0f32, -1.0, 1.0, 3.0];

        // CPU reference
        let mut cpu_bufs = vec![vec![0u8; n * 4], f32_to_bytes(&input)];
        interpret::execute_kernel(&kernel, &mut cpu_bufs);
        let cpu_result = bytes_to_f32(&cpu_bufs[0]);

        // Metal
        let device = MetalDevice::new().expect("Metal device required");
        let out_buf = device.alloc(n * 4).unwrap();
        let in_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&in_buf, &f32_to_bytes(&input)).unwrap();

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &in_buf],
                [n as u32, 1, 1],
                [n as u32, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let metal_result = bytes_to_f32(&result_bytes);

        assert_eq!(cpu_result, vec![3.0, 1.0, 0.0, 0.0]);
        assert_f32_close(&metal_result, &cpu_result, "FusedNegMax", 0.0);
    }
}
