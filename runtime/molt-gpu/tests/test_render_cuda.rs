use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::cuda::CudaRenderer;
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
                spec: None,
    }
}

#[test]
fn test_cuda_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let cuda = CudaRenderer.render(&kernel);
    assert!(cuda.contains("#include <cuda_runtime.h>"));
    assert!(cuda.contains("__global__"));
    assert!(cuda.contains("molt_kernel"));
    assert!(cuda.contains("blockIdx.x * blockDim.x + threadIdx.x"));
    assert!(cuda.contains("buf1[gid] + buf2[gid]"));
}

#[test]
fn test_cuda_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 512);
    let cuda = CudaRenderer.render(&kernel);
    assert!(cuda.contains("buf1[gid] * buf2[gid]"));
}

#[test]
fn test_cuda_full_i64_support() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int64,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int64, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int64, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int64, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None,
    };
    let cuda = CudaRenderer.render(&kernel);
    // CUDA supports full i64 — should NOT narrow to i32
    assert!(cuda.contains("long long"), "CUDA should support i64 as 'long long'");
}

#[test]
fn test_cuda_full_f64_support() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float64,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float64, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float64, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float64, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None,
    };
    let cuda = CudaRenderer.render(&kernel);
    assert!(cuda.contains("double"), "CUDA should support f64 as 'double'");
}

#[test]
fn test_cuda_ternary_operator() {
    // CUDA uses standard C ternary, not select()
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Where,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Bool, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None,
    };
    let cuda = CudaRenderer.render(&kernel);
    assert!(cuda.contains(" ? "), "CUDA should use ternary operator");
}

#[test]
fn test_cuda_all_26_ops_render() {
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
                spec: None,
        };
        let cuda = CudaRenderer.render(&kernel);
        assert!(cuda.contains("molt_kernel"), "op {:?} failed to render CUDA", op);
    }
}
