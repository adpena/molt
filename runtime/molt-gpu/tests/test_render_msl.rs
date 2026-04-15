use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::msl::MslRenderer;
use molt_gpu::shapetracker::ShapeTracker;

fn make_simple_binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
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
        ops: vec![FusedOp {
            op: PrimitiveOp::Neg,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
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
                spec: None, vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("(-buf1[gid])"));
}

#[test]
fn test_render_fused_chain() {
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(2), FusedSrc::Buf(3)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("v0"));
    assert!(msl.contains("v1"));
    assert!(msl.contains("buf0[gid] = v1"));
}

#[test]
fn test_render_relu_with_const() {
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
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("max(buf1[gid], 0.0f)"));
}

#[test]
fn test_render_comparison_bool_output() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[128]), dtype: DType::Bool, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[128]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [128, 1, 1],
        local: [128, 1, 1],
                spec: None, vectorize_width: 1,
    };
    let msl = MslRenderer.render(&kernel);
    assert!(msl.contains("bool v0"));
    assert!(msl.contains("buf1[gid] < buf2[gid]"));
}

#[test]
fn test_all_26_ops_have_render_patterns() {
    let elementwise_ops = PrimitiveOp::ALL.iter()
        .filter(|op| op.is_elementwise())
        .collect::<Vec<_>>();

    for &&op in &elementwise_ops {
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
            ops: vec![FusedOp {
                op,
                srcs,
                dst_dtype: if matches!(op, PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne) {
                    DType::Bool
                } else {
                    DType::Float32
                },
            }],
            bufs,
            grid: [64, 1, 1],
            local: [64, 1, 1],
                spec: None, vectorize_width: 1,
        };
        let msl = MslRenderer.render(&kernel);
        assert!(msl.contains("molt_kernel"), "op {:?} failed to render", op);
    }
}
