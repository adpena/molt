use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::ops::{OpType, PrimitiveOp};
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, ReductionDomain,
};
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

fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(4)
        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn u32_to_bytes(vals: &[u32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_u32(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn run_unary_typed_raw(
    op: PrimitiveOp,
    src_dtype: DType,
    dst_dtype: DType,
    input: Vec<u8>,
    n: usize,
) -> Vec<u8> {
    let kernel = FusedKernel {
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let mut bufs = vec![vec![0u8; n * dst_dtype.size_bytes()], input];
    interpret::execute_kernel(&kernel, &mut bufs);
    bufs.remove(0)
}

fn run_binary_op_cpu(op: PrimitiveOp, a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len();
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            op,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
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
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(a)];
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
fn test_cpu_cast_float32_to_int32_writes_integer_values() {
    let out = run_unary_typed_raw(
        PrimitiveOp::Cast,
        DType::Float32,
        DType::Int32,
        f32_to_bytes(&[1.25, -2.75, 0.0, 7.0]),
        4,
    );

    assert_eq!(bytes_to_i32(&out), vec![1, -2, 0, 7]);
}

#[test]
fn test_cpu_fused_intermediate_cast_uses_converted_value() {
    let n = 2;
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(PrimitiveOp::Cast, vec![FusedSrc::Buf(1)], DType::Int32),
            FusedOp::elementwise(PrimitiveOp::Cast, vec![FusedSrc::Buf(2)], DType::Int32),
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Op(0), FusedSrc::Op(1)],
                DType::Int32,
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![
        vec![0u8; n * DType::Int32.size_bytes()],
        f32_to_bytes(&[1.25, -2.75]),
        f32_to_bytes(&[2.75, -3.25]),
    ];

    interpret::execute_kernel(&kernel, &mut bufs);

    assert_eq!(bytes_to_i32(&bufs[0]), vec![3, -5]);
}

#[test]
fn test_cpu_reduce_sum_uses_intermediate_cast_values() {
    let n = 4;
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(PrimitiveOp::Cast, vec![FusedSrc::Buf(1)], DType::Int32),
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(0)],
                DType::Int32,
                ReductionDomain::from_axis(&[n], 0),
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Int32,
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
    let mut bufs = vec![
        vec![0u8; DType::Int32.size_bytes()],
        f32_to_bytes(&[1.9, 1.9, 1.9, 1.9]),
    ];

    interpret::execute_kernel(&kernel, &mut bufs);

    assert_eq!(bytes_to_i32(&bufs[0]), vec![4]);
}

#[test]
fn test_cpu_cast_float32_to_bool_treats_nan_as_true() {
    let out = run_unary_typed_raw(
        PrimitiveOp::Cast,
        DType::Float32,
        DType::Bool,
        f32_to_bytes(&[0.0, -0.0, f32::NAN, 2.0]),
        4,
    );

    assert_eq!(out, vec![0, 0, 1, 1]);
}

#[test]
fn test_cpu_cast_float32_to_uint8_preserves_numeric_byte() {
    let out = run_unary_typed_raw(
        PrimitiveOp::Cast,
        DType::Float32,
        DType::UInt8,
        f32_to_bytes(&[0.0, 1.0, 2.0, 255.0]),
        4,
    );

    assert_eq!(out, vec![0, 1, 2, 255]);
}

#[test]
fn test_cpu_bitcast_float32_to_uint32_preserves_raw_bits() {
    let values = [1.0f32, -0.0, f32::NAN, f32::NEG_INFINITY];
    let out = run_unary_typed_raw(
        PrimitiveOp::Bitcast,
        DType::Float32,
        DType::UInt32,
        f32_to_bytes(&values),
        values.len(),
    );

    assert_eq!(
        bytes_to_u32(&out),
        values.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn test_cpu_bitcast_uint32_to_float32_preserves_raw_bits() {
    let bits = [
        1.0f32.to_bits(),
        (-0.0f32).to_bits(),
        0x7fc0_0000,
        0xff80_0000,
    ];
    let out = run_unary_typed_raw(
        PrimitiveOp::Bitcast,
        DType::UInt32,
        DType::Float32,
        u32_to_bytes(&bits),
        bits.len(),
    );

    assert_eq!(
        bytes_to_f32(&out)
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        bits
    );
}

#[test]
#[should_panic(expected = "CPU interpreter Bitcast requires equal-width")]
fn test_cpu_bitcast_rejects_width_change() {
    let _ = run_unary_typed_raw(
        PrimitiveOp::Bitcast,
        DType::Float32,
        DType::UInt16,
        f32_to_bytes(&[1.0, 2.0]),
        2,
    );
}

#[test]
fn test_cpu_relu_composition() {
    let n = 4;
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Max,
            vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: 0.0,
                    dtype: DType::Float32,
                },
            ],
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&[-2.0, -1.0, 0.0, 3.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![0.0, 0.0, 0.0, 3.0]);
}

#[test]
fn test_cpu_where_ternary() {
    let n = 3;
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
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
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[4], 0),
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
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&[1.0, 2.0, 3.0, 4.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![10.0]);
}

#[test]
fn test_cpu_reduce_max() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[4], 0),
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
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&[3.0, 1.0, 4.0, 2.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![4.0]);
}

fn run_reduce_2d(op: PrimitiveOp, axis: usize, input: &[f32]) -> Vec<f32> {
    let domain = ReductionDomain::from_axis(&[2, 3], axis);
    let out_shape = domain.output_shape.clone();
    let out_n = domain.output_numel();
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            op,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            domain,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&out_shape),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[2, 3]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [out_n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![vec![0u8; out_n * 4], f32_to_bytes(input)];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

#[test]
fn test_cpu_reduce_sum_axis0_2d() {
    let out = run_reduce_2d(PrimitiveOp::ReduceSum, 0, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(out, vec![5.0, 7.0, 9.0]);
}

#[test]
fn test_cpu_reduce_sum_axis1_2d() {
    let out = run_reduce_2d(PrimitiveOp::ReduceSum, 1, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(out, vec![6.0, 15.0]);
}

#[test]
fn test_cpu_reduce_max_axis0_2d() {
    let out = run_reduce_2d(PrimitiveOp::ReduceMax, 0, &[1.0, 8.0, 3.0, 4.0, 5.0, 9.0]);
    assert_eq!(out, vec![4.0, 8.0, 9.0]);
}

// --- Task 12: Extended ops tests ---

#[test]
fn test_cpu_idiv() {
    let n = 4;
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Idiv,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Int32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(4)
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
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Mod,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Int32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(4)
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
    let result = run_binary_op_cpu(
        PrimitiveOp::Cmplt,
        &[f32::NAN, 1.0, 0.0],
        &[1.0, f32::NAN, 0.0],
    );
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
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::And,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Int32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    fn i32_to_bytes(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(4)
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
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&[-3.0, -1.0, 1.0, 3.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![3.0, 1.0, 0.0, 0.0]);
}

#[test]
fn test_all_26_ops_covered() {
    for op in PrimitiveOp::ALL {
        let srcs: Vec<FusedSrc> = match op.arity() {
            1 => vec![FusedSrc::Const {
                val: 1.0,
                dtype: DType::Float32,
            }],
            2 => vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 2.0,
                    dtype: DType::Float32,
                },
            ],
            3 => vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 2.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 3.0,
                    dtype: DType::Float32,
                },
            ],
            _ => unreachable!(),
        };
        let fused_op = if matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
            FusedOp::reduction(
                op,
                srcs,
                DType::Float32,
                ReductionDomain::from_axis(&[4], 0),
            )
        } else {
            FusedOp::elementwise(op, srcs, DType::Float32)
        };
        assert_eq!(fused_op.op(), op);
    }
}

#[cfg(target_os = "macos")]
mod metal_tests {
    use super::*;
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::{Allocator, Compiler, Executor};
    use molt_gpu::render::Renderer;
    use molt_gpu::render::msl::MslRenderer;

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
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
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
            ],
            grid: [n as u32, 1, 1],
            local: [4, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let msl = MslRenderer.render(&kernel);
        let prog = device.compile(&msl, "molt_kernel").unwrap();
        device
            .exec(
                &prog,
                &[&out_buf, &a_buf, &b_buf],
                [n as u32, 1, 1],
                [4, 1, 1],
            )
            .unwrap();
        device.synchronize().unwrap();

        let mut result_bytes = vec![0u8; n * 4];
        device.copy_out(&out_buf, &mut result_bytes).unwrap();
        let result = bytes_to_f32(&result_bytes);

        let expected = run_binary_op_cpu(
            PrimitiveOp::Add,
            &[1.0, 2.0, 3.0, 4.0],
            &[5.0, 6.0, 7.0, 8.0],
        );
        assert_eq!(result, expected);
    }
}
