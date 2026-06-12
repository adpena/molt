use molt_gpu::dtype::DType;
use molt_gpu::fuse::{constant_fold, identity_fold};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
};
use molt_gpu::shapetracker::ShapeTracker;

fn make_kernel(ops: Vec<FusedOp>, bufs: Vec<BufferBinding>) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops,
        bufs,
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_materialize_copy_kernel() -> FusedKernel {
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![out_buf(), in_buf(1)],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn out_buf() -> BufferBinding {
    BufferBinding {
        buf_id: 0,
        st: ShapeTracker::contiguous(&[64]),
        dtype: DType::Float32,
        access: BufferAccess::Write,
    }
}

fn in_buf(id: usize) -> BufferBinding {
    BufferBinding {
        buf_id: id,
        st: ShapeTracker::contiguous(&[64]),
        dtype: DType::Float32,
        access: BufferAccess::Read,
    }
}

#[test]
fn test_fold_mul_two_consts() {
    // MUL(Const(2.0), Const(3.0)) → should fold to Const(6.0)
    // Then ADD(Buf(1), folded_const) stays as one op with Const(6.0) source.
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Const {
                    val: 2.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 3.0,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![
                FusedSrc::Buf(1),
                FusedSrc::Op(0), // references the MUL result
            ],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1, "one op should have been folded");
    assert_eq!(kernels[0].ops.len(), 1, "only ADD remains");

    // The ADD's second source should now be Const(6.0)
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 6.0).abs() < 1e-10, "expected 6.0, got {}", val);
        }
        other => panic!("expected Const, got {:?}", other),
    }
}

#[test]
fn test_fold_chain_of_consts() {
    // Op 0: ADD(Const(1.0), Const(2.0)) → Const(3.0)
    // Op 1: MUL(Op(0), Const(4.0)) → Const(12.0)
    // Op 2: SUB(Buf(1), Op(1)) → stays, with Const(12.0)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 2.0,
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
                    val: 4.0,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Sub,
            vec![FusedSrc::Buf(1), FusedSrc::Op(1)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 2, "two ops should have been folded");
    assert_eq!(kernels[0].ops.len(), 1, "only SUB remains");

    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 12.0).abs() < 1e-10, "expected 12.0, got {}", val);
        }
        other => panic!("expected Const(12.0), got {:?}", other),
    }
}

#[test]
fn test_no_fold_when_buffer_involved() {
    // ADD(Buf(1), Const(1.0)) → cannot fold (has buffer input).
    let ops = vec![FusedOp::elementwise(
        PrimitiveOp::Add,
        vec![
            FusedSrc::Buf(1),
            FusedSrc::Const {
                val: 1.0,
                dtype: DType::Float32,
            },
        ],
        DType::Float32,
    )];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 0);
    assert_eq!(kernels[0].ops.len(), 1);
}

#[test]
fn test_fold_unary_const() {
    // NEG(Const(5.0)) → Const(-5.0), then ADD(Buf(1), folded).
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Neg,
            vec![FusedSrc::Const {
                val: 5.0,
                dtype: DType::Float32,
            }],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - (-5.0)).abs() < 1e-10);
        }
        other => panic!("expected Const(-5.0), got {:?}", other),
    }
}

#[test]
fn test_fold_exp2_const() {
    // EXP2(Const(3.0)) → Const(8.0)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Exp2,
            vec![FusedSrc::Const {
                val: 3.0,
                dtype: DType::Float32,
            }],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 8.0).abs() < 1e-10);
        }
        other => panic!("expected Const(8.0), got {:?}", other),
    }
}

#[test]
fn test_fold_comparison_const() {
    // CMPLT(Const(1.0), Const(2.0)) → Const(1.0) (true)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Cmplt,
            vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 2.0,
                    dtype: DType::Float32,
                },
            ],
            DType::Bool,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 1.0).abs() < 1e-10, "1 < 2 should be true (1.0)");
        }
        other => panic!("expected Const(1.0), got {:?}", other),
    }
}

#[test]
fn test_fold_where_const() {
    // WHERE(Const(1.0), Const(10.0), Const(20.0)) → Const(10.0) (cond is true)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Where,
            vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Bool,
                },
                FusedSrc::Const {
                    val: 10.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 20.0,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 10.0).abs() < 1e-10);
        }
        other => panic!("expected Const(10.0), got {:?}", other),
    }
}

#[test]
fn test_fold_empty_kernel() {
    let mut kernels: Vec<FusedKernel> = Vec::new();
    let folded = constant_fold(&mut kernels);
    assert_eq!(folded, 0);
}

#[test]
fn test_fold_all_const_kernel() {
    // Entire kernel is constant: MUL(Const(2.0), Const(3.0)).
    // The single op gets folded, leaving an empty ops list.
    let ops = vec![FusedOp::elementwise(
        PrimitiveOp::Mul,
        vec![
            FusedSrc::Const {
                val: 2.0,
                dtype: DType::Float32,
            },
            FusedSrc::Const {
                val: 3.0,
                dtype: DType::Float32,
            },
        ],
        DType::Float32,
    )];

    let mut kernels = vec![make_kernel(ops, vec![out_buf()])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    assert_eq!(kernels[0].ops.len(), 0);
}

#[test]
fn test_fold_multiple_kernels() {
    let ops1 = vec![
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Const {
                    val: 2.0,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];
    let ops2 = vec![FusedOp::elementwise(
        PrimitiveOp::Add,
        vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
        DType::Float32,
    )];

    let mut kernels = vec![
        make_kernel(ops1, vec![out_buf(), in_buf(1)]),
        make_kernel(ops2, vec![out_buf(), in_buf(1), in_buf(2)]),
    ];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    assert_eq!(kernels[0].ops.len(), 1);
    assert_eq!(kernels[1].ops.len(), 1); // no folding in kernel 2
}

#[test]
fn test_fold_sqrt_const() {
    // SQRT(Const(16.0)) → Const(4.0)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Sqrt,
            vec![FusedSrc::Const {
                val: 16.0,
                dtype: DType::Float32,
            }],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 4.0).abs() < 1e-10);
        }
        other => panic!("expected Const(4.0), got {:?}", other),
    }
}

#[test]
fn test_fold_reciprocal_const() {
    // RECIPROCAL(Const(4.0)) → Const(0.25)
    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Reciprocal,
            vec![FusedSrc::Const {
                val: 4.0,
                dtype: DType::Float32,
            }],
            DType::Float32,
        ),
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
            DType::Float32,
        ),
    ];

    let mut kernels = vec![make_kernel(ops, vec![out_buf(), in_buf(1)])];
    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 1);
    match &kernels[0].ops[0].srcs()[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 0.25).abs() < 1e-10);
        }
        other => panic!("expected Const(0.25), got {:?}", other),
    }
}

#[test]
fn test_no_fold_reduce_op() {
    // ReduceSum cannot be folded even with const source.
    let ops = vec![FusedOp::reduction(
        PrimitiveOp::ReduceSum,
        vec![FusedSrc::Const {
            val: 1.0,
            dtype: DType::Float32,
        }],
        DType::Float32,
        ReductionDomain::from_axis(&[64, 1], 1),
    )];

    let mut kernels = vec![make_kernel(ops, vec![out_buf()])];
    let folded = constant_fold(&mut kernels);
    assert_eq!(folded, 0);
}

#[test]
fn test_constant_fold_leaves_materialize_copy_unchanged() {
    let mut kernels = vec![make_materialize_copy_kernel()];

    let folded = constant_fold(&mut kernels);

    assert_eq!(folded, 0);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert!(kernels[0].ops.is_empty());
    assert_eq!(kernels[0].bufs.len(), 2);
    assert_eq!(kernels[0].bufs[1].buf_id, 1);
}

#[test]
fn test_identity_fold_leaves_materialize_copy_unchanged() {
    let mut kernels = vec![make_materialize_copy_kernel()];

    let folded = identity_fold(&mut kernels);

    assert_eq!(folded, 0);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert!(kernels[0].ops.is_empty());
    assert_eq!(kernels[0].bufs.len(), 2);
    assert_eq!(kernels[0].bufs[1].buf_id, 1);
}
