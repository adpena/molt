use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::msl::MslRenderer;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, ReductionDomain, Renderer,
};
use molt_gpu::shapetracker::ShapeTracker;

fn make_simple_binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
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
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_simple_unary_typed_kernel(
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
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("#include <metal_stdlib>"));
    assert!(msl.contains("kernel void molt_kernel"));
    assert!(msl.contains("buf1[gid] + buf2[gid]"));
    assert!(msl.contains("buf0[gid] = v0"));
}

#[test]
fn test_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 512);
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("buf1[gid] * buf2[gid]"));
}

#[test]
fn test_render_neg_unary() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Neg,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("(-buf1[gid])"));
}

#[test]
fn test_render_fused_chain() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![FusedSrc::Buf(2), FusedSrc::Buf(3)],
                DType::Float32,
            ),
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
                DType::Float32,
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("v0"));
    assert!(msl.contains("v1"));
    assert!(msl.contains("buf0[gid] = v1"));
}

#[test]
fn test_render_relu_with_const() {
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
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("max(buf1[gid], 0.0f)"));
}

#[test]
fn test_render_comparison_bool_output() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Cmplt,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Bool,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Bool,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("bool v0"));
    assert!(msl.contains("buf1[gid] < buf2[gid]"));
}

#[test]
fn test_render_cast_uses_typed_integer_storage() {
    let cases = [
        (DType::Int32, "device int* buf0", "int(buf1[gid])"),
        (DType::UInt16, "device ushort* buf0", "ushort(buf1[gid])"),
        (DType::UInt8, "device uchar* buf0", "uchar(buf1[gid])"),
    ];

    for (dst_dtype, out_ptr, cast_expr) in cases {
        let kernel =
            make_simple_unary_typed_kernel(PrimitiveOp::Cast, DType::Float32, dst_dtype, 128);
        let msl = MslRenderer.render(&kernel);
        assert!(msl.contains(out_ptr), "missing {out_ptr} in:\n{msl}");
        assert!(msl.contains("const device float* buf1"));
        assert!(msl.contains(cast_expr), "missing {cast_expr} in:\n{msl}");
    }
}

#[test]
fn test_render_bitcast_uses_typed_uint_storage() {
    let kernel =
        make_simple_unary_typed_kernel(PrimitiveOp::Bitcast, DType::Float32, DType::UInt32, 128);
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("device uint* buf0"));
    assert!(msl.contains("const device float* buf1"));
    assert!(msl.contains("as_type<uint>(buf1[gid])"));
}

#[test]
fn test_all_26_ops_have_render_patterns() {
    let elementwise_ops = PrimitiveOp::ALL
        .iter()
        .copied()
        .filter(|op| op.is_elementwise())
        .collect::<Vec<_>>();

    for op in elementwise_ops {
        let srcs = match op.arity() {
            1 => vec![FusedSrc::Buf(1)],
            2 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            3 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            _ => unreachable!(),
        };
        let mut bufs = vec![BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[64]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        }];
        for i in 1..=op.arity() {
            bufs.push(BufferBinding {
                buf_id: i,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            });
        }
        let kernel = FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::elementwise(
                op,
                srcs,
                if matches!(
                    op,
                    PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
                ) {
                    DType::Bool
                } else {
                    DType::Float32
                },
            )],
            bufs,
            grid: [64, 1, 1],
            local: [64, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let msl = MslRenderer.render(&kernel);
        assert!(msl.contains("molt_kernel"), "op {:?} failed to render", op);
    }
}

#[test]
fn test_msl_reduce_axis0_uses_affine_domain_index() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[2, 3], 0),
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[3]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[6]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [3, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("uint eidx = (((rid % 2) * 3) + (gid % 3));"));
}
