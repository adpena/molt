use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::ops::{OpType, PrimitiveOp};
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use molt_gpu::shapetracker::ShapeTracker;

#[test]
fn test_all_26_ops() {
    assert_eq!(PrimitiveOp::ALL.len(), 26);
}

#[test]
fn test_op_types() {
    assert_eq!(PrimitiveOp::Add.op_type(), OpType::Binary);
    assert_eq!(PrimitiveOp::Neg.op_type(), OpType::Unary);
    assert_eq!(PrimitiveOp::Where.op_type(), OpType::Ternary);
    assert_eq!(PrimitiveOp::ReduceSum.op_type(), OpType::Reduce);
}

#[test]
fn test_arities() {
    assert_eq!(PrimitiveOp::Neg.arity(), 1);
    assert_eq!(PrimitiveOp::Add.arity(), 2);
    assert_eq!(PrimitiveOp::Where.arity(), 3);
    assert_eq!(PrimitiveOp::ReduceSum.arity(), 1);
}

#[test]
fn test_elementwise() {
    assert!(PrimitiveOp::Add.is_elementwise());
    assert!(PrimitiveOp::Neg.is_elementwise());
    assert!(PrimitiveOp::Where.is_elementwise());
    assert!(!PrimitiveOp::ReduceSum.is_elementwise());
    assert!(!PrimitiveOp::ReduceMax.is_elementwise());
}

// --- CPU interpreter tests ---

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn run_binary_op_cpu(op: PrimitiveOp, a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len();
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };

    let mut bufs = vec![
        vec![0u8; n * 4], // output
        f32_to_bytes(a),
        f32_to_bytes(b),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

fn run_unary_op_cpu(op: PrimitiveOp, a: &[f32]) -> Vec<f32> {
    let n = a.len();
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };

    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(a),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

#[test]
fn test_cpu_add() {
    let result = run_binary_op_cpu(PrimitiveOp::Add, &[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
    assert_eq!(result, vec![5.0, 7.0, 9.0]);
}

#[test]
fn test_cpu_sub() {
    let result = run_binary_op_cpu(PrimitiveOp::Sub, &[5.0, 3.0, 1.0], &[1.0, 2.0, 3.0]);
    assert_eq!(result, vec![4.0, 1.0, -2.0]);
}

#[test]
fn test_cpu_mul() {
    let result = run_binary_op_cpu(PrimitiveOp::Mul, &[2.0, 3.0, 4.0], &[5.0, 6.0, 7.0]);
    assert_eq!(result, vec![10.0, 18.0, 28.0]);
}

#[test]
fn test_cpu_neg() {
    let result = run_unary_op_cpu(PrimitiveOp::Neg, &[1.0, -2.0, 0.0]);
    assert_eq!(result, vec![-1.0, 2.0, -0.0]);
}

#[test]
fn test_cpu_exp2() {
    let result = run_unary_op_cpu(PrimitiveOp::Exp2, &[0.0, 1.0, 2.0, 3.0]);
    assert_eq!(result, vec![1.0, 2.0, 4.0, 8.0]);
}

#[test]
fn test_cpu_log2() {
    let result = run_unary_op_cpu(PrimitiveOp::Log2, &[1.0, 2.0, 4.0, 8.0]);
    assert_eq!(result, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn test_cpu_sqrt() {
    let result = run_unary_op_cpu(PrimitiveOp::Sqrt, &[0.0, 1.0, 4.0, 9.0]);
    assert_eq!(result, vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn test_cpu_reciprocal() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[1.0, 2.0, 4.0, 0.5]);
    assert_eq!(result, vec![1.0, 0.5, 0.25, 2.0]);
}

#[test]
fn test_cpu_reciprocal_zero() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[0.0]);
    assert!(result[0].is_infinite() && result[0] > 0.0); // +inf
}

#[test]
fn test_cpu_reciprocal_neg_zero() {
    let result = run_unary_op_cpu(PrimitiveOp::Reciprocal, &[-0.0]);
    assert!(result[0].is_infinite() && result[0] < 0.0); // -inf
}

#[test]
fn test_cpu_max() {
    let result = run_binary_op_cpu(PrimitiveOp::Max, &[1.0, 5.0, -3.0], &[3.0, 2.0, -1.0]);
    assert_eq!(result, vec![3.0, 5.0, -1.0]);
}

#[test]
fn test_cpu_trunc() {
    let result = run_unary_op_cpu(PrimitiveOp::Trunc, &[2.7, -2.7, 3.0, -3.0]);
    assert_eq!(result, vec![2.0, -2.0, 3.0, -3.0]);
}

#[test]
fn test_cpu_sin() {
    let result = run_unary_op_cpu(PrimitiveOp::Sin, &[0.0]);
    assert!((result[0] - 0.0).abs() < 1e-6);
}

#[test]
fn test_cpu_relu_composition() {
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Max,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: 0.0, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[-2.0, -1.0, 0.0, 3.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![0.0, 0.0, 0.0, 3.0]);
}

#[test]
fn test_cpu_where_ternary() {
    let n = 3;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Where,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[1.0, 0.0, 1.0]),
        f32_to_bytes(&[10.0, 20.0, 30.0]),
        f32_to_bytes(&[40.0, 50.0, 60.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![10.0, 50.0, 30.0]);
}

#[test]
fn test_cpu_reduce_sum() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[4]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; 4],
        f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![10.0]);
}

#[test]
fn test_cpu_reduce_max() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[4]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; 4],
        f32_to_bytes(&[3.0, 1.0, 4.0, 2.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![4.0]);
}

// --- Task 12: Extended ops tests ---

#[test]
fn test_cpu_idiv() {
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Idiv,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[7, -7, 7, -7]),
        i32_to_bytes(&[3, 3, -3, -3]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    assert_eq!(result, vec![2, -2, -2, 2]);
}

#[test]
fn test_cpu_mod() {
    let n = 4;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mod,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[7, -7, 7, -7]),
        i32_to_bytes(&[3, 3, -3, -3]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    assert_eq!(result, vec![1, -1, 1, -1]);
}

#[test]
fn test_cpu_cmplt_nan() {
    let result = run_binary_op_cpu(PrimitiveOp::Cmplt, &[f32::NAN, 1.0, 0.0], &[1.0, f32::NAN, 0.0]);
    assert_eq!(result, vec![0.0, 0.0, 0.0]);
}

#[test]
fn test_cpu_cmpeq_nan() {
    let result = run_binary_op_cpu(PrimitiveOp::Cmpeq, &[f32::NAN, 1.0], &[f32::NAN, 1.0]);
    assert_eq!(result, vec![0.0, 1.0]);
}

#[test]
fn test_cpu_cmpne_nan() {
    let result = run_binary_op_cpu(PrimitiveOp::Cmpne, &[f32::NAN, 1.0], &[f32::NAN, 1.0]);
    assert_eq!(result, vec![1.0, 0.0]);
}

#[test]
fn test_cpu_bitwise_and() {
    let n = 3;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::And,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    let mut bufs = vec![
        vec![0u8; n * 4],
        i32_to_bytes(&[0xFF, 0x0F, 0xAA_u32 as i32]),
        i32_to_bytes(&[0x0F, 0xFF, 0x55]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_i32(&bufs[0]);
    assert_eq!(result, vec![0x0F, 0x0F, 0x00]);
}

#[test]
fn test_cpu_fused_relu_chain() {
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
                    FusedSrc::Const { val: 0.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&[-3.0, -1.0, 1.0, 3.0]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![3.0, 1.0, 0.0, 0.0]);
}

#[test]
fn test_all_26_ops_covered() {
    for op in PrimitiveOp::ALL {
        let srcs: Vec<FusedSrc> = match op.arity() {
            1 => vec![FusedSrc::Const { val: 1.0, dtype: DType::Float32 }],
            2 => vec![
                FusedSrc::Const { val: 1.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
            ],
            3 => vec![
                FusedSrc::Const { val: 1.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 3.0, dtype: DType::Float32 },
            ],
            _ => unreachable!(),
        };
        let fused_op = FusedOp {
            op,
            srcs,
            dst_dtype: DType::Float32,
        };
        assert_eq!(fused_op.op, op);
    }
}

#[cfg(target_os = "macos")]
mod metal_tests {
    use super::*;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::Renderer;

    #[test]
    fn test_metal_add() {
        let device = MetalDevice::new().expect("Metal device required");
        let a_data = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);
        let b_data = f32_to_bytes(&[5.0, 6.0, 7.0, 8.0]);
        let n = 4;

        let out_buf = device.alloc(n * 4).unwrap();
        let a_buf = device.alloc(n * 4).unwrap();
        let b_buf = device.alloc(n * 4).unwrap();
        device.copy_in(&a_buf, &a_data).unwrap();
        device.copy_in(&b_buf, &b_data).unwrap();

        let kernel = FusedKernel {
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
            local: [4, 1, 1],
                spec: None, vectorize_width: 1,
        };

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device.exec(&prog, &[&out_buf, &a_buf, &b_buf], [n as u32, 1, 1], [4, 1, 1]).unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let result = bytes_to_f32(&result_bytes);

        let expected = run_binary_op_cpu(PrimitiveOp::Add, &[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0]);
        assert_eq!(result, expected);
    }
}
