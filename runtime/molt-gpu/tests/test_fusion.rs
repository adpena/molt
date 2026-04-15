use molt_gpu::dtype::DType;
use molt_gpu::fuse::fuse;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::shapetracker::ShapeTracker;

fn make_elementwise_kernel(op: PrimitiveOp, buf_ids: (usize, usize, usize)) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: buf_ids.0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: buf_ids.1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: buf_ids.2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None, vectorize_width: 1,
    }
}

fn make_reduce_kernel(op: PrimitiveOp, in_size: usize, out_size: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 100, st: ShapeTracker::contiguous(&[out_size]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 101, st: ShapeTracker::contiguous(&[in_size]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [out_size as u32, 1, 1],
        local: [1, 1, 1],
                spec: None, vectorize_width: 1,
    }
}

#[test]
fn test_fuse_two_elementwise() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_elementwise_kernel(PrimitiveOp::Mul, (20, 10, 3)),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "two elementwise ops should fuse into 1 kernel");
    assert_eq!(fused[0].ops.len(), 2);
}

#[test]
fn test_fuse_three_elementwise() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_elementwise_kernel(PrimitiveOp::Mul, (20, 10, 3)),
        make_elementwise_kernel(PrimitiveOp::Sub, (30, 20, 4)),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "three elementwise ops should fuse into 1 kernel");
    assert_eq!(fused[0].ops.len(), 3);
}

#[test]
fn test_reduce_to_reduce_boundary() {
    let kernels = vec![
        make_reduce_kernel(PrimitiveOp::ReduceMax, 1024, 32),
        make_reduce_kernel(PrimitiveOp::ReduceSum, 32, 1),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 2, "reduce-to-reduce is a fusion boundary");
}

#[test]
fn test_elementwise_reduce_fuses() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Mul, (10, 1, 2)),
        make_reduce_kernel(PrimitiveOp::ReduceSum, 64, 1),
    ];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1, "elementwise -> reduce should fuse");
}

#[test]
fn test_single_kernel_unchanged() {
    let kernels = vec![make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2))];
    let fused = fuse(kernels);
    assert_eq!(fused.len(), 1);
    assert_eq!(fused[0].ops.len(), 1);
}

#[test]
fn test_empty_input() {
    let fused = fuse(vec![]);
    assert_eq!(fused.len(), 0);
}
