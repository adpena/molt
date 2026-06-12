use std::sync::Arc;

use molt_gpu::dtype::DType;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedOpDomain, FusedSrc, KernelBody,
};
use molt_gpu::schedule::{deduplicate_kernels, schedule, specialize_shapes};
use molt_gpu::shapetracker::ShapeTracker;
use molt_gpu::{dce, fuse};

// --- Shape specialization tests ---

fn make_kernel_with_shape(shape: &[usize]) -> FusedKernel {
    let n: usize = shape.iter().product();
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(shape),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(shape),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(shape),
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
fn test_specialize_exact_divisible_256() {
    // 1024 elements, local=256, 1024 % 256 == 0 → bounds check eliminated.
    let mut kernels = vec![make_kernel_with_shape(&[1024])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.all_static);
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 1024);
    assert_eq!(spec.optimal_local, [256, 1, 1]);
    assert_eq!(kernels[0].grid, [4, 1, 1]); // 1024 / 256 = 4 workgroups
    assert_eq!(kernels[0].local, [256, 1, 1]);
}

#[test]
fn test_specialize_exact_divisible_128() {
    // 384 elements: 384 % 256 != 0, but 384 % 128 == 0 → picks 128.
    let mut kernels = vec![make_kernel_with_shape(&[384])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 384);
    assert_eq!(spec.optimal_local, [128, 1, 1]);
    assert_eq!(kernels[0].grid, [3, 1, 1]); // 384 / 128 = 3
}

#[test]
fn test_specialize_odd_count() {
    // 257 elements: not divisible by any preferred size except 1.
    let mut kernels = vec![make_kernel_with_shape(&[257])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.bounds_check_elim); // 257 % 1 == 0, always divides
    assert_eq!(spec.total_elements, 257);
    assert_eq!(spec.optimal_local, [1, 1, 1]);
    assert_eq!(kernels[0].grid, [257, 1, 1]);
}

#[test]
fn test_specialize_multidim() {
    // Shape [16, 64] = 1024 total elements.
    let mut kernels = vec![make_kernel_with_shape(&[16, 64])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.all_static);
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 1024);
    assert_eq!(spec.optimal_local, [256, 1, 1]);
}

#[test]
fn test_specialize_power_of_two() {
    // 64 elements → local=64, grid=1.
    let mut kernels = vec![make_kernel_with_shape(&[64])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 64);
    assert_eq!(spec.optimal_local, [64, 1, 1]);
    assert_eq!(kernels[0].grid, [1, 1, 1]);
}

#[test]
fn test_specialize_single_element() {
    let mut kernels = vec![make_kernel_with_shape(&[1])];
    specialize_shapes(&mut kernels);

    let spec = kernels[0].spec.as_ref().expect("spec should be set");
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 1);
    assert_eq!(spec.optimal_local, [1, 1, 1]);
    assert_eq!(kernels[0].grid, [1, 1, 1]);
}

#[test]
fn test_specialize_multiple_kernels() {
    let mut kernels = vec![
        make_kernel_with_shape(&[256]),
        make_kernel_with_shape(&[384]),
        make_kernel_with_shape(&[7]),
    ];
    specialize_shapes(&mut kernels);

    assert!(kernels[0].spec.as_ref().unwrap().bounds_check_elim);
    assert_eq!(kernels[0].spec.as_ref().unwrap().optimal_local, [256, 1, 1]);

    assert!(kernels[1].spec.as_ref().unwrap().bounds_check_elim);
    assert_eq!(kernels[1].spec.as_ref().unwrap().optimal_local, [128, 1, 1]);

    // 7 is only divisible by 1
    assert!(kernels[2].spec.as_ref().unwrap().bounds_check_elim);
    assert_eq!(kernels[2].spec.as_ref().unwrap().optimal_local, [1, 1, 1]);
}

#[test]
fn test_specialize_preserves_no_spec_when_empty() {
    let mut kernels: Vec<FusedKernel> = Vec::new();
    specialize_shapes(&mut kernels);
    assert_eq!(kernels.len(), 0);
}

#[test]
fn test_schedule_then_specialize() {
    // Build a LazyOp DAG and schedule, then specialize.
    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: 1024,
        },
        st: ShapeTracker::contiguous(&[256]),
        dtype: DType::Float32,
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&buf),
    });
    let mut kernels = schedule(&neg, &[256]);
    assert_eq!(kernels.len(), 1);
    assert!(kernels[0].spec.is_none());

    specialize_shapes(&mut kernels);
    let spec = kernels[0].spec.as_ref().expect("spec after specialization");
    assert!(spec.all_static);
    assert!(spec.bounds_check_elim);
    assert_eq!(spec.total_elements, 256);
}

// --- DCE tests ---

fn make_buffer(id: usize, size: usize) -> Arc<LazyOp> {
    make_buffer_shape(id, &[size])
}

fn make_buffer_shape(id: usize, shape: &[usize]) -> Arc<LazyOp> {
    let size = shape.iter().product::<usize>();
    Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id,
            size_bytes: size * 4,
        },
        st: ShapeTracker::contiguous(shape),
        dtype: DType::Float32,
    })
}

#[test]
fn test_schedule_cast_preserves_explicit_target_dtype() {
    let buf = make_buffer(0, 16);
    let cast = Arc::new(LazyOp::Cast {
        op: PrimitiveOp::Cast,
        src: Arc::clone(&buf),
        dst_dtype: DType::Int32,
    });
    let kernels = schedule(&cast, &[16]);

    assert_eq!(cast.dtype(), DType::Int32);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs[0].dtype, DType::Int32);
    assert_eq!(kernel.bufs[1].dtype, DType::Float32);
    assert_eq!(kernel.ops[0].op(), PrimitiveOp::Cast);
    assert_eq!(kernel.ops[0].dst_dtype(), DType::Int32);
    assert_buf_src(&kernel.ops[0].srcs()[0], 1);
}

#[test]
#[should_panic(expected = "Cast/Bitcast LazyOps must use LazyOp::Cast with explicit dst_dtype")]
fn test_untyped_unary_cast_is_rejected() {
    let buf = make_buffer(0, 16);
    let cast = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Cast,
        src: buf,
    });

    let _ = cast.dtype();
}

#[test]
fn test_dce_single_root_all_reachable() {
    let buf_a = make_buffer(0, 64);
    let buf_b = make_buffer(1, 64);
    let add = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&buf_a),
        rhs: Arc::clone(&buf_b),
    });

    // All 3 nodes are reachable from the root.
    assert_eq!(dce::count_reachable(&[Arc::clone(&add)]), 3);
    assert_eq!(dce::count_nodes(&add), 3);
}

#[test]
fn test_dce_multi_root_shared_subtree() {
    let buf = make_buffer(0, 64);
    let neg1 = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&buf),
    });
    let neg2 = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Exp2,
        src: Arc::clone(&buf),
    });

    // Both roots share `buf`. Total unique reachable = 3.
    assert_eq!(
        dce::count_reachable(&[Arc::clone(&neg1), Arc::clone(&neg2)]),
        3
    );
}

#[test]
fn test_dce_dead_node_eliminated() {
    let buf_a = make_buffer(0, 64);
    let buf_b = make_buffer(1, 64);
    let live = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&buf_a),
    });
    // buf_b is dead — not reachable from `live`.
    let all_nodes = vec![Arc::clone(&buf_a), Arc::clone(&buf_b), Arc::clone(&live)];
    let roots = vec![Arc::clone(&live)];
    let surviving = dce::eliminate_dead_nodes(&roots, &all_nodes);

    // Only buf_a and live should survive; buf_b is dead.
    assert_eq!(surviving.len(), 2);
}

#[test]
fn test_dce_empty_roots() {
    let result = dce::eliminate_dead_code(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_dce_deeply_nested() {
    let buf = make_buffer(0, 64);
    let mut current = Arc::clone(&buf);
    for _ in 0..10 {
        current = Arc::new(LazyOp::Unary {
            op: PrimitiveOp::Neg,
            src: current,
        });
    }
    // 11 nodes: 1 buffer + 10 unary ops.
    assert_eq!(dce::count_nodes(&current), 11);
    assert_eq!(dce::count_reachable(&[current]), 11);
}

#[test]
fn test_dce_ternary() {
    let cond = make_buffer(0, 64);
    let a = make_buffer(1, 64);
    let b = make_buffer(2, 64);
    let where_op = Arc::new(LazyOp::Ternary {
        op: PrimitiveOp::Where,
        cond: Arc::clone(&cond),
        a: Arc::clone(&a),
        b: Arc::clone(&b),
    });
    assert_eq!(dce::count_nodes(&where_op), 4);
}

#[test]
fn test_dce_reduce() {
    let buf = make_buffer(0, 64);
    let reduce = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: Arc::clone(&buf),
        axis: 0,
    });
    assert_eq!(dce::count_nodes(&reduce), 2);
}

#[test]
fn test_schedule_reduce_preserves_axis0_domain() {
    let buf = make_buffer_shape(0, &[2, 3]);
    let reduce = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: Arc::clone(&buf),
        axis: 0,
    });
    let kernels = schedule(&reduce, &[3]);

    assert_eq!(kernels.len(), 1);
    assert_eq!(kernels[0].bufs[0].st.shape(), &[3]);
    let domain = match kernels[0].ops[0].domain() {
        FusedOpDomain::Reduction(domain) => domain,
        FusedOpDomain::Elementwise => panic!("reduce op must carry reduction domain"),
    };
    assert_eq!(domain.input_shape, vec![2, 3]);
    assert_eq!(domain.output_shape, vec![3]);
    assert_eq!(domain.axes, vec![0]);
    assert_eq!(domain.kept_axes, vec![1]);
    assert_eq!(domain.reduce_shape, vec![2]);
    assert_eq!(domain.reduce_size, 2);
    assert_eq!(domain.input_linear_index(0, 0), 0);
    assert_eq!(domain.input_linear_index(0, 1), 3);
}

#[test]
fn test_schedule_reduce_preserves_axis1_domain() {
    let buf = make_buffer_shape(0, &[2, 3]);
    let reduce = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: Arc::clone(&buf),
        axis: 1,
    });
    let kernels = schedule(&reduce, &[2]);

    assert_eq!(kernels.len(), 1);
    assert_eq!(kernels[0].bufs[0].st.shape(), &[2]);
    let domain = match kernels[0].ops[0].domain() {
        FusedOpDomain::Reduction(domain) => domain,
        FusedOpDomain::Elementwise => panic!("reduce op must carry reduction domain"),
    };
    assert_eq!(domain.input_shape, vec![2, 3]);
    assert_eq!(domain.output_shape, vec![2]);
    assert_eq!(domain.axes, vec![1]);
    assert_eq!(domain.kept_axes, vec![0]);
    assert_eq!(domain.reduce_shape, vec![3]);
    assert_eq!(domain.reduce_size, 3);
    assert_eq!(domain.input_linear_index(1, 0), 3);
    assert_eq!(domain.input_linear_index(1, 2), 5);
}

#[test]
fn test_kernel_dedup_distinguishes_reduce_axis() {
    let buf = make_buffer_shape(0, &[2, 2]);
    let axis0 = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: Arc::clone(&buf),
        axis: 0,
    });
    let axis1 = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: Arc::clone(&buf),
        axis: 1,
    });

    let k0 = schedule(&axis0, &[2]).remove(0);
    let k1 = schedule(&axis1, &[2]).remove(0);
    let (_, dedup_count) = deduplicate_kernels(&[k0, k1]);
    assert_eq!(dedup_count, 0);
}

#[test]
fn test_dce_movement_and_contiguous() {
    let buf = make_buffer(0, 64);
    let mov = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: ShapeTracker::contiguous(&[8, 8]),
    });
    let contig = Arc::new(LazyOp::Contiguous {
        src: Arc::clone(&mov),
    });
    assert_eq!(dce::count_nodes(&contig), 3);
}

// --- Movement operand scheduling and materialization guards ---
//
// Movement nodes are zero-copy views of source storage. Contiguous still needs
// an explicit materialization kernel; masked/padded Movement views are carried
// into the executable renderers as guarded ShapeTracker reads.

#[test]
fn test_schedule_binds_movement_operand_to_source_storage_with_view() {
    let buf = make_buffer(0, 64);
    let movement_st = ShapeTracker::contiguous(&[64]).flip(0);
    let mov = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: movement_st.clone(),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&mov),
    });
    let kernels = schedule(&neg, &[64]);

    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs.len(), 2);
    assert_eq!(kernel.bufs[1].buf_id, 0);
    assert_eq!(kernel.bufs[1].st, movement_st);
    assert_buf_src(&kernel.ops[0].srcs()[0], 1);
}

#[test]
fn test_schedule_materializes_contiguous_operand_with_fresh_storage() {
    let buf = make_buffer(0, 64);
    let flipped_st = ShapeTracker::contiguous(&[64]).flip(0);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: flipped_st.clone(),
    });
    let contig = Arc::new(LazyOp::Contiguous {
        src: Arc::clone(&flipped),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&contig),
    });
    let kernels = schedule(&neg, &[64]);

    assert_eq!(kernels.len(), 2);

    let copy = &kernels[0];
    assert_eq!(copy.body, KernelBody::MaterializeCopy);
    assert!(copy.ops.is_empty());
    assert_eq!(copy.bufs.len(), 2);
    assert_ne!(copy.bufs[0].buf_id, 0);
    assert_eq!(copy.bufs[0].st, ShapeTracker::contiguous(&[64]));
    assert_eq!(copy.bufs[1].buf_id, 0);
    assert_eq!(copy.bufs[1].st, flipped_st);

    let unary = &kernels[1];
    assert_eq!(unary.body, KernelBody::Compute);
    assert_eq!(unary.bufs[1].buf_id, copy.bufs[0].buf_id);
    assert_eq!(unary.bufs[1].st, ShapeTracker::contiguous(&[64]));
    assert_buf_src(&unary.ops[0].srcs()[0], 1);
}

#[test]
fn test_schedule_shared_contiguous_node_emits_one_copy_kernel() {
    let buf = make_buffer(7, 4);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: ShapeTracker::contiguous(&[4]).flip(0),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: flipped });
    let add = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&contig),
        rhs: Arc::clone(&contig),
    });
    let kernels = schedule(&add, &[4]);

    assert_eq!(kernels.len(), 2);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert_eq!(kernels[1].body, KernelBody::Compute);
    assert_eq!(kernels[1].bufs.len(), 2);
    assert_eq!(kernels[1].bufs[1].buf_id, kernels[0].bufs[0].buf_id);
    assert_eq!(kernels[1].ops[0].srcs().len(), 2);
    assert_buf_src(&kernels[1].ops[0].srcs()[0], 1);
    assert_buf_src(&kernels[1].ops[0].srcs()[1], 1);
}

#[test]
fn test_schedule_contiguous_root_emits_materialize_copy() {
    let buf = make_buffer(0, 64);
    let contig = Arc::new(LazyOp::Contiguous {
        src: Arc::clone(&buf),
    });
    let kernels = schedule(&contig, &[64]);

    assert_eq!(kernels.len(), 1);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert!(kernels[0].ops.is_empty());
    assert_eq!(kernels[0].bufs[0].st, ShapeTracker::contiguous(&[64]));
    assert_eq!(kernels[0].bufs[1].buf_id, 0);
}

#[test]
fn test_schedule_movement_root_emits_materialize_copy() {
    let buf = make_buffer_shape(0, &[2, 1]);
    let expanded_st = ShapeTracker::contiguous(&[2, 1]).expand(&[2, 3]);
    let expanded = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: expanded_st.clone(),
    });
    let kernels = schedule(&expanded, &[2, 3]);

    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);
    assert!(kernel.ops.is_empty());
    assert_eq!(kernel.bufs.len(), 2);
    assert_ne!(kernel.bufs[0].buf_id, 0);
    assert_eq!(kernel.bufs[0].st, ShapeTracker::contiguous(&[2, 3]));
    assert_eq!(kernel.bufs[1].buf_id, 0);
    assert_eq!(kernel.bufs[1].st, expanded_st);
}

#[test]
fn test_schedule_distinct_contiguous_nodes_get_distinct_copy_storage() {
    let buf = make_buffer(0, 64);
    let lhs = Arc::new(LazyOp::Contiguous {
        src: Arc::clone(&buf),
    });
    let rhs = Arc::new(LazyOp::Contiguous {
        src: Arc::clone(&buf),
    });
    let add = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&lhs),
        rhs: Arc::clone(&rhs),
    });
    let kernels = schedule(&add, &[64]);

    assert_eq!(kernels.len(), 3);
    assert_eq!(kernels[0].body, KernelBody::MaterializeCopy);
    assert_eq!(kernels[1].body, KernelBody::MaterializeCopy);
    assert_ne!(kernels[0].bufs[0].buf_id, kernels[1].bufs[0].buf_id);
    assert_eq!(kernels[2].body, KernelBody::Compute);
    assert_eq!(kernels[2].bufs.len(), 3);
    assert_eq!(kernels[2].bufs[1].buf_id, kernels[0].bufs[0].buf_id);
    assert_eq!(kernels[2].bufs[2].buf_id, kernels[1].bufs[0].buf_id);
}

#[test]
fn test_schedule_binds_masked_movement_operand_to_source_storage_with_view() {
    let buf = make_buffer(0, 64);
    let movement_st = ShapeTracker::contiguous(&[64]).pad(&[(1, 1)]);
    let mov = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: movement_st.clone(),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&mov),
    });
    let kernels = schedule(&neg, &[66]);

    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs.len(), 2);
    assert_eq!(kernel.bufs[1].buf_id, 0);
    assert_eq!(kernel.bufs[1].st, movement_st);
    assert_buf_src(&kernel.ops[0].srcs()[0], 1);
}

#[test]
fn test_schedule_keeps_same_storage_distinct_views_as_distinct_slots() {
    let buf = make_buffer(7, 4);
    let flipped_st = ShapeTracker::contiguous(&[4]).flip(0);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&buf),
        st: flipped_st.clone(),
    });
    let add = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&flipped),
        rhs: Arc::clone(&buf),
    });
    let kernels = schedule(&add, &[4]);

    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs.len(), 3);
    assert_eq!(kernel.bufs[1].buf_id, 7);
    assert_eq!(kernel.bufs[2].buf_id, 7);
    assert_eq!(kernel.bufs[1].st, flipped_st);
    assert_eq!(kernel.bufs[2].st, ShapeTracker::contiguous(&[4]));
    assert_buf_src(&kernel.ops[0].srcs()[0], 1);
    assert_buf_src(&kernel.ops[0].srcs()[1], 2);
}

#[test]
fn test_fuse_preserves_same_storage_distinct_view_external_bindings() {
    let st = ShapeTracker::contiguous(&[4]);
    let flipped_st = st.flip(0);
    let first = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 10,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 7,
                st: flipped_st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 7,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let second = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 11,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 10,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 7,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let fused = fuse::fuse(vec![first, second]);
    assert_eq!(fused.len(), 1);
    let kernel = &fused[0];
    assert_eq!(kernel.bufs.len(), 3);
    assert_eq!(kernel.bufs[1].buf_id, 7);
    assert_eq!(kernel.bufs[1].st, flipped_st);
    assert_eq!(kernel.bufs[2].buf_id, 7);
    assert_eq!(kernel.bufs[2].st, st);
    assert_buf_src(&kernel.ops[0].srcs()[0], 1);
    assert_buf_src(&kernel.ops[0].srcs()[1], 2);
    assert_op_src(&kernel.ops[1].srcs()[0], 0);
    assert_buf_src(&kernel.ops[1].srcs()[1], 2);
}

#[test]
fn test_dce_eliminate_dead_nodes_preserves_order() {
    let buf_a = make_buffer(0, 64);
    let buf_b = make_buffer(1, 64);
    let buf_c = make_buffer(2, 64);
    let live_op = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&buf_a),
        rhs: Arc::clone(&buf_c),
    });

    let all_nodes = vec![
        Arc::clone(&buf_a),
        Arc::clone(&buf_b),
        Arc::clone(&buf_c),
        Arc::clone(&live_op),
    ];
    let surviving = dce::eliminate_dead_nodes(&[live_op], &all_nodes);

    // buf_b is dead, so 3 survive.
    assert_eq!(surviving.len(), 3);
}

fn assert_buf_src(src: &FusedSrc, expected: usize) {
    match src {
        FusedSrc::Buf(actual) => assert_eq!(*actual, expected),
        other => panic!("expected FusedSrc::Buf({expected}), got {other:?}"),
    }
}

fn assert_op_src(src: &FusedSrc, expected: usize) {
    match src {
        FusedSrc::Op(actual) => assert_eq!(*actual, expected),
        other => panic!("expected FusedSrc::Op({expected}), got {other:?}"),
    }
}
