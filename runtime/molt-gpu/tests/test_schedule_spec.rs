use std::sync::Arc;

use molt_gpu::dce;
use molt_gpu::dtype::DType;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::schedule::{schedule, specialize_shapes};
use molt_gpu::shapetracker::ShapeTracker;

// --- Shape specialization tests ---

fn make_kernel_with_shape(shape: &[usize]) -> FusedKernel {
    let n: usize = shape.iter().product();
    FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
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
        spec: None, vectorize_width: 1,
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
        buf: DeviceBufferRef { id: 0, size_bytes: 1024 },
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
    Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef { id, size_bytes: size * 4 },
        st: ShapeTracker::contiguous(&[size]),
        dtype: DType::Float32,
    })
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
    assert_eq!(dce::count_reachable(&[Arc::clone(&neg1), Arc::clone(&neg2)]), 3);
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
    let all_nodes = vec![
        Arc::clone(&buf_a),
        Arc::clone(&buf_b),
        Arc::clone(&live),
    ];
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
