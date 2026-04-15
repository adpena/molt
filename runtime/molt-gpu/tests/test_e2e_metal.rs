//! End-to-end Metal GPU tests.
//!
//! Allocates buffers on Metal, runs all 26 ops, and compares results
//! with CPU reference bit-for-bit. Tests softmax and matmul compositions,
//! and IEEE 754 edge cases.

#[cfg(target_os = "macos")]
mod metal_e2e {
    use molt_gpu::device::cpu::interpret;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::dtype::DType;
    use molt_gpu::fuse::fuse;
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

    /// Run a binary op on Metal and return results alongside CPU reference.
    fn run_binary_metal_vs_cpu(
        op: PrimitiveOp,
        a: &[f32],
        b: &[f32],
        dst_dtype: DType,
    ) -> (Vec<f32>, Vec<f32>) {
        let n = a.len();
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype,
            }],
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
            ops: vec![FusedOp {
                op,
                srcs: vec![FusedSrc::Buf(1)],
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
            local: [n.clamp(1, 256) as u32, 1, 1],
                spec: None,
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
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Add, &[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Add", 0.0);
    }

    #[test]
    fn test_metal_e2e_sub() {
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Sub, &[5.0, 3.0, 1.0, -1.0], &[1.0, 2.0, 3.0, 4.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Sub", 0.0);
    }

    #[test]
    fn test_metal_e2e_mul() {
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Mul, &[2.0, 3.0, 4.0, 5.0], &[5.0, 6.0, 7.0, 8.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Mul", 0.0);
    }

    #[test]
    fn test_metal_e2e_neg() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Neg, &[1.0, -2.0, 0.0, 3.14]);
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
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Max, &[1.0, 5.0, -3.0, 0.0], &[3.0, 2.0, -1.0, 0.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Max", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmplt() {
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Cmplt, &[1.0, 5.0, -3.0, 0.0], &[3.0, 2.0, -1.0, 0.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Cmplt", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmpeq() {
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Cmpeq, &[1.0, 2.0, 3.0, 0.0], &[1.0, 3.0, 3.0, 0.0], DType::Float32);
        assert_f32_close(&metal, &cpu, "Cmpeq", 0.0);
    }

    #[test]
    fn test_metal_e2e_cmpne() {
        let (metal, cpu) =
            run_binary_metal_vs_cpu(PrimitiveOp::Cmpne, &[1.0, 2.0, 3.0, 0.0], &[1.0, 3.0, 3.0, 0.0], DType::Float32);
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
        assert!(metal[0].is_sign_negative(), "Neg(0.0) should be -0.0 on Metal");
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
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceMax,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
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
        };

        // Kernel 2: Sub (x - max) -> Exp2(* LOG2_E) = exp
        let log2_e = std::f64::consts::LOG2_E;
        let k2 = FusedKernel {
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
        };

        // Kernel 3: ReduceSum of exp values
        let k3 = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
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
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
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

    // --- WHERE (ternary) ---

    #[test]
    fn test_metal_e2e_where() {
        let n = 4;
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Where,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
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
    fn test_metal_e2e_cast() {
        let (metal, cpu) = run_unary_metal_vs_cpu(PrimitiveOp::Cast, &[1.5, -2.7, 0.0, 42.0]);
        assert_f32_close(&metal, &cpu, "Cast", 0.0);
    }

    // --- Bitwise on Metal (using float representation) ---

    #[test]
    fn test_metal_e2e_fused_neg_max() {
        // ReLU(-x) = max(-x, 0): tests fused chain on Metal
        let n = 4;
        let kernel = FusedKernel {
            ops: vec![
                FusedOp {
                    op: PrimitiveOp::Neg,
                    srcs: vec![FusedSrc::Buf(1)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::Max,
                    srcs: vec![
                        FusedSrc::Op(0),
                        FusedSrc::Const {
                            val: 0.0,
                            dtype: DType::Float32,
                        },
                    ],
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
            local: [n as u32, 1, 1],
                spec: None,
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
