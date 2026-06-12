use molt_gpu::dtype::DType;
use molt_gpu::fuse::{constant_fold, fuse};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
};
use molt_gpu::shapetracker::ShapeTracker;

fn make_elementwise_kernel(op: PrimitiveOp, buf_ids: (usize, usize, usize)) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            op,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: buf_ids.0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: buf_ids.1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: buf_ids.2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_reduce_kernel(op: PrimitiveOp, in_size: usize, out_size: usize) -> FusedKernel {
    let input_shape = if out_size == 1 {
        vec![in_size]
    } else {
        assert_eq!(in_size % out_size, 0);
        vec![out_size, in_size / out_size]
    };
    let reduce_axis = input_shape.len() - 1;
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            op,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&input_shape, reduce_axis),
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(&[out_size]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 101,
                st: ShapeTracker::contiguous(&[in_size]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [out_size as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_elementwise_kernel_with_shape(
    op: PrimitiveOp,
    buf_ids: (usize, usize, usize),
    shape: &[usize],
) -> FusedKernel {
    FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            op,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: buf_ids.0,
                st: ShapeTracker::contiguous(shape),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: buf_ids.1,
                st: ShapeTracker::contiguous(shape),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: buf_ids.2,
                st: ShapeTracker::contiguous(shape),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [shape.iter().product::<usize>() as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_copy_kernel(out_id: usize, in_id: usize, st: ShapeTracker) -> FusedKernel {
    let n = st.numel();
    let out_st = ShapeTracker::contiguous(st.shape());
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: out_id,
                st: out_st,
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: in_id,
                st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n.max(1) as u32, 1, 1],
        local: [n.clamp(1, 256) as u32, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_fuse_two_elementwise() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_elementwise_kernel(PrimitiveOp::Mul, (20, 10, 3)),
    ];
    let fused = fuse(kernels);
    assert_eq!(
        fused.len(),
        1,
        "two elementwise ops should fuse into 1 kernel"
    );
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
    assert_eq!(
        fused.len(),
        1,
        "three elementwise ops should fuse into 1 kernel"
    );
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
fn test_reduce_then_same_shape_elementwise_fuses() {
    let kernels = vec![
        make_reduce_kernel(PrimitiveOp::ReduceSum, 6, 3),
        make_elementwise_kernel_with_shape(PrimitiveOp::Add, (200, 100, 3), &[3]),
    ];
    let fused = fuse(kernels);

    assert_eq!(
        fused.len(),
        1,
        "reduce -> same-shape elementwise should fuse"
    );
    assert_eq!(fused[0].ops.len(), 2);
}

#[test]
fn test_reduce_then_shape_expansion_with_same_numel_is_boundary() {
    let kernels = vec![
        make_reduce_kernel(PrimitiveOp::ReduceSum, 6, 3),
        make_elementwise_kernel_with_shape(PrimitiveOp::Add, (200, 100, 3), &[1, 3]),
    ];
    let fused = fuse(kernels);

    assert_eq!(
        fused.len(),
        2,
        "post-reduce shape expansion needs explicit reshape/broadcast IR before fusion"
    );
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

#[test]
fn test_materialize_copy_is_hard_fusion_barrier() {
    let kernels = vec![
        make_elementwise_kernel(PrimitiveOp::Add, (10, 1, 2)),
        make_copy_kernel(20, 10, ShapeTracker::contiguous(&[64]).flip(0)),
        make_elementwise_kernel(PrimitiveOp::Mul, (30, 20, 3)),
    ];

    let fused = fuse(kernels);

    assert_eq!(fused.len(), 3);
    assert_eq!(fused[0].body, KernelBody::Compute);
    assert_eq!(fused[1].body, KernelBody::MaterializeCopy);
    assert!(fused[1].ops.is_empty());
    assert_eq!(fused[2].body, KernelBody::Compute);
    assert_eq!(fused[2].bufs[1].buf_id, 20);
}

#[test]
fn test_constant_fold_leaves_materialize_copy_untouched() {
    let mut kernels = vec![make_copy_kernel(
        20,
        1,
        ShapeTracker::contiguous(&[64]).pad(&[(1, 1)]),
    )];

    assert_eq!(constant_fold(&mut kernels), 0);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert!(kernels[0].ops.is_empty());
    assert_eq!(kernels[0].bufs[1].buf_id, 1);
}
