use molt_gpu::dtype::DType;
use molt_gpu::mlir::to_mlir_text;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
};
use molt_gpu::shapetracker::ShapeTracker;

fn materialize_copy_kernel(dtype: DType, src_st: ShapeTracker) -> FusedKernel {
    let numel = src_st.numel();
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 10,
                st: ShapeTracker::contiguous(src_st.shape()),
                dtype,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: src_st,
                dtype,
                access: BufferAccess::Read,
            },
        ],
        grid: [numel as u32, 1, 1],
        local: [numel.clamp(1, 256) as u32, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn cast_kernel(src_dtype: DType, dst_dtype: DType) -> FusedKernel {
    FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Cast,
            vec![FusedSrc::Buf(1)],
            dst_dtype,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
                dtype: dst_dtype,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: src_dtype,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn intermediate_mxfp_cast_kernel(dtype: DType) -> FusedKernel {
    FusedKernel {
        body: KernelBody::Compute,
        ops: vec![
            FusedOp::elementwise(PrimitiveOp::Cast, vec![FusedSrc::Buf(1)], dtype),
            FusedOp::elementwise(PrimitiveOp::Cast, vec![FusedSrc::Op(0)], DType::Float32),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
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
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_mlir_add_f32() {
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
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains(
        "func.func @molt_kernel(%buf0: memref<?xf32>, %buf1: memref<?xf32>, %buf2: memref<?xf32>)"
    ));
    assert!(mlir.contains("scf.for %gid = %c0 to %c_numel step %c1"));
    assert!(mlir.contains("memref.load %buf1[%gid] : memref<?xf32>"));
    assert!(mlir.contains("memref.load %buf2[%gid] : memref<?xf32>"));
    assert!(mlir.contains("arith.addf"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xf32>"));
    assert!(mlir.contains("f32"));
}

#[test]
fn test_mlir_add_i32() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Int32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addi"));
    assert!(mlir.contains("i32"));
}

#[test]
fn test_mlir_cmplt() {
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
                st: ShapeTracker::contiguous(&[32]),
                dtype: DType::Bool,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[32]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[32]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [32, 1, 1],
        local: [32, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.cmpf \"olt\""));
}

#[test]
fn test_mlir_shr_signed_vs_unsigned() {
    let kernel_signed = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Shr,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Int32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    assert!(to_mlir_text(&kernel_signed).contains("arith.shrsi"));

    let kernel_unsigned = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Shr,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::UInt32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::UInt32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::UInt32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[16]),
                dtype: DType::UInt32,
                access: BufferAccess::Read,
            },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    assert!(to_mlir_text(&kernel_unsigned).contains("arith.shrui"));
}

#[test]
fn test_mlir_lowers_reduce_sum_axis0_loop() {
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
                st: ShapeTracker::contiguous(&[2, 3]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [3, 1, 1],
        local: [3, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("%c_numel = arith.constant 3 : index"));
    assert!(mlir.contains("%c_reduce = arith.constant 2 : index"));
    assert!(mlir.contains("scf.for %rid = %c0 to %c_reduce step %c1 iter_args(%acc"));
    assert!(mlir.contains("memref.load %buf1[%reduce_idx"));
    assert!(mlir.contains("arith.addf %acc"));
    assert!(mlir.contains("scf.yield"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_lowers_reduce_max_axis1_loop() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[2, 3], 1),
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[2]),
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
        grid: [2, 1, 1],
        local: [2, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("%c_numel = arith.constant 2 : index"));
    assert!(mlir.contains("%c_reduce = arith.constant 3 : index"));
    assert!(mlir.contains("arith.constant -inf : f32"));
    assert!(mlir.contains("arith.maximumf %acc"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_lowers_reduce_with_prefix_and_suffix() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: 2.0,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(0)],
                DType::Float32,
                ReductionDomain::from_axis(&[2, 3], 1),
            ),
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Op(1), FusedSrc::Buf(2)],
                DType::Float32,
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[2]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[2, 3]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[2]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [2, 1, 1],
        local: [2, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("%v0 = arith.mulf"));
    assert!(mlir.contains("%v1 = scf.for %rid = %c0 to %c_reduce step %c1"));
    assert!(mlir.contains("%v2 = arith.addf %v1"));
    assert!(mlir.contains("memref.store %v2, %buf0[%gid] : memref<?xf32>"));
}

#[test]
#[should_panic(expected = "post-reduce op 2 cannot reference pre-reduce op 0")]
fn test_mlir_rejects_post_reduce_reference_to_pre_reduce_value() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: 2.0,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(0)],
                DType::Float32,
                ReductionDomain::from_axis(&[2, 3], 1),
            ),
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![FusedSrc::Op(1), FusedSrc::Op(0)],
                DType::Float32,
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[2]),
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
        grid: [2, 1, 1],
        local: [2, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let _ = to_mlir_text(&kernel);
}

#[test]
fn test_mlir_compute_loads_flipped_view() {
    let st = ShapeTracker::contiguous(&[4]);
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: st.flip(0),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains(
        "func.func @molt_kernel(%buf0: memref<?xf32>, %buf1: memref<?xf32>, %buf2: memref<?xf32>)"
    ));
    assert!(mlir.contains("arith.constant 3 : index"));
    assert!(mlir.contains("arith.subi"));
    assert!(mlir.contains("memref.load %buf1[%buf1_idx"));
    assert!(mlir.contains("memref.load %buf2[%gid] : memref<?xf32>"));
    assert!(mlir.contains("arith.addf"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_compute_zero_fills_masked_view_before_load() {
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[5]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[5]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [5, 1, 1],
        local: [5, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("arith.constant -1 : index"));
    assert!(mlir.contains("arith.cmpi sge"));
    assert!(mlir.contains("arith.cmpi slt"));
    assert!(mlir.contains("arith.andi"));
    assert!(mlir.contains("scf.if"));
    assert!(mlir.contains("scf.yield %buf1_zero"));
    assert!(mlir.contains("arith.addf"));
    let if_pos = mlir.find("scf.if").unwrap();
    let load_pos = mlir.find("memref.load %buf1[").unwrap();
    let add_pos = mlir.find("arith.addf").unwrap();
    assert!(if_pos < load_pos);
    assert!(load_pos < add_pos);
}

#[test]
fn test_mlir_compute_names_same_storage_distinct_views_by_binding_slot() {
    let st = ShapeTracker::contiguous(&[4]);
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 77,
                st: st.flip(0),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains(
        "func.func @molt_kernel(%buf0: memref<?xf32>, %buf1: memref<?xf32>, %buf2: memref<?xf32>)"
    ));
    assert!(mlir.contains("memref.load %buf1[%gid] : memref<?xf32>"));
    assert!(mlir.contains("memref.load %buf2[%buf2_idx"));
    assert!(mlir.contains("arith.addf"));
    assert!(!mlir.contains("%buf77"));
}

#[test]
fn test_mlir_compute_lowers_composed_permuted_view_indices() {
    let src_st = ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]);
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[2, 2]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: src_st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[2, 2]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("arith.divui"));
    assert!(mlir.contains("arith.remui"));
    assert!(mlir.contains("arith.subi"));
    assert!(mlir.contains("memref.load %buf1[%buf1_idx"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_compute_uses_integer_comparison_for_integer_sources() {
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Cmplt,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Bool,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[8]),
                dtype: DType::Bool,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[8]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[8]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [8, 1, 1],
        local: [8, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("arith.cmpi slt"));
    assert!(mlir.contains("memref.store %v0, %buf0[%gid] : memref<?xi1>"));
}

#[test]
fn test_mlir_compute_resolves_constants_and_prior_ops() {
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![
                    FusedSrc::Buf(1),
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
                        val: 3.0,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
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
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("arith.constant 2e0 : f32"));
    assert!(mlir.contains("arith.constant 3e0 : f32"));
    assert!(mlir.contains("%v0 = arith.addf"));
    assert!(mlir.contains("%v1 = arith.mulf %v0"));
    assert!(mlir.contains("memref.store %v1, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_cast_lowers_float_width_conversions() {
    let widened = to_mlir_text(&cast_kernel(DType::Float32, DType::Float64));
    assert!(widened.contains("arith.extf %buf1_load_0 : f32 to f64"));
    assert!(widened.contains("memref.store %v0, %buf0[%gid] : memref<?xf64>"));

    let narrowed = to_mlir_text(&cast_kernel(DType::Float64, DType::Float32));
    assert!(narrowed.contains("arith.truncf %buf1_load_0 : f64 to f32"));

    let same_width = to_mlir_text(&cast_kernel(DType::Float16, DType::BFloat16));
    assert!(same_width.contains("arith.convertf %buf1_load_0 : f16 to bf16"));
}

#[test]
fn test_mlir_cast_lowers_float_integer_conversions() {
    let float_to_signed = to_mlir_text(&cast_kernel(DType::Float32, DType::Int32));
    assert!(float_to_signed.contains("arith.fptosi %buf1_load_0 : f32 to i32"));

    let float_to_unsigned = to_mlir_text(&cast_kernel(DType::Float32, DType::UInt32));
    assert!(float_to_unsigned.contains("arith.fptoui %buf1_load_0 : f32 to ui32"));

    let signed_to_float = to_mlir_text(&cast_kernel(DType::Int32, DType::Float32));
    assert!(signed_to_float.contains("arith.sitofp %buf1_load_0 : i32 to f32"));

    let unsigned_to_float = to_mlir_text(&cast_kernel(DType::UInt32, DType::Float32));
    assert!(unsigned_to_float.contains("arith.uitofp %buf1_load_0 : ui32 to f32"));
}

#[test]
fn test_mlir_cast_lowers_integer_width_and_signedness_conversions() {
    let signed_widen = to_mlir_text(&cast_kernel(DType::Int16, DType::Int32));
    assert!(signed_widen.contains("arith.extsi %buf1_load_0 : i16 to i32"));

    let unsigned_widen = to_mlir_text(&cast_kernel(DType::UInt16, DType::UInt32));
    assert!(unsigned_widen.contains("arith.extui %buf1_load_0 : ui16 to ui32"));

    let truncate = to_mlir_text(&cast_kernel(DType::Int32, DType::Int16));
    assert!(truncate.contains("arith.trunci %buf1_load_0 : i32 to i16"));

    let same_width_signedness = to_mlir_text(&cast_kernel(DType::Int32, DType::UInt32));
    assert!(same_width_signedness.contains("arith.bitcast %buf1_load_0 : i32 to ui32"));
}

#[test]
fn test_mlir_cast_lowers_bool_truth_and_value_conversions() {
    let int_to_bool = to_mlir_text(&cast_kernel(DType::Int32, DType::Bool));
    assert!(int_to_bool.contains("%op0_zero_1 = arith.constant 0 : i32"));
    assert!(int_to_bool.contains("arith.cmpi ne, %buf1_load_0, %op0_zero_1 : i32"));

    let float_to_bool = to_mlir_text(&cast_kernel(DType::Float32, DType::Bool));
    assert!(float_to_bool.contains("%op0_zero_1 = arith.constant 0.000000e+00 : f32"));
    assert!(float_to_bool.contains("arith.cmpf \"une\", %buf1_load_0, %op0_zero_1 : f32"));

    let bool_to_int = to_mlir_text(&cast_kernel(DType::Bool, DType::Int32));
    assert!(bool_to_int.contains("arith.extui %buf1_load_0 : i1 to i32"));

    let bool_to_float = to_mlir_text(&cast_kernel(DType::Bool, DType::Float32));
    assert!(bool_to_float.contains("arith.uitofp %buf1_load_0 : i1 to f32"));
}

#[test]
fn test_mlir_cast_same_dtype_aliases_source_value() {
    let mlir = to_mlir_text(&cast_kernel(DType::Float32, DType::Float32));

    assert!(!mlir.contains("%v0 ="));
    assert!(mlir.contains("memref.store %buf1_load_0, %buf0[%gid] : memref<?xf32>"));
}

#[test]
#[should_panic(expected = "MLIR buffer storage for MXFP requires explicit block/exponent")]
fn test_mlir_compute_rejects_mxfp_source_buffer_until_storage_lowering_exists() {
    let _ = to_mlir_text(&cast_kernel(DType::MxFP8, DType::Float32));
}

#[test]
#[should_panic(expected = "MLIR buffer storage for MXFP requires explicit block/exponent")]
fn test_mlir_compute_rejects_mxfp_destination_buffer_until_storage_lowering_exists() {
    let _ = to_mlir_text(&cast_kernel(DType::Float32, DType::MxFP8));
}

#[test]
#[should_panic(expected = "MLIR buffer storage for MXFP requires explicit block/exponent")]
fn test_mlir_compute_rejects_same_dtype_mxfp4_copy_before_raw_i8_signature() {
    let _ = to_mlir_text(&cast_kernel(DType::MxFP4, DType::MxFP4));
}

#[test]
#[should_panic(expected = "Cast involving MXFP requires explicit quantized conversion lowering")]
fn test_mlir_cast_rejects_intermediate_mxfp_until_quantized_lowering_exists() {
    let _ = to_mlir_text(&intermediate_mxfp_cast_kernel(DType::MxFP8));
}

#[test]
#[should_panic(expected = "compute output must be a single contiguous view")]
fn test_mlir_rejects_non_contiguous_compute_output_view() {
    let kernel = FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]).flip(0),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let _ = to_mlir_text(&kernel);
}

#[test]
fn test_mlir_materialize_copy_contiguous_uses_flat_memrefs() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[4]),
    ));

    assert!(mlir.contains("func.func @molt_kernel(%buf0: memref<?xf32>, %buf1: memref<?xf32>)"));
    assert!(mlir.contains("scf.for %gid = %c0 to %c_numel step %c1"));
    assert!(mlir.contains("memref.load %buf1[%gid] : memref<?xf32>"));
    assert!(mlir.contains("memref.store %copy_value, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_materialize_copy_from_flipped_view() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[4]).flip(0),
    ));

    assert!(mlir.contains("arith.constant 3 : index"));
    assert!(mlir.contains("arith.subi"));
    assert!(mlir.contains("memref.load %buf1[%src"));
    assert!(!mlir.contains("scf.if"));
}

#[test]
fn test_mlir_materialize_copy_from_shrunk_view() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[5]).shrink(&[(1, 4)]),
    ));

    assert!(mlir.contains("%c_numel = arith.constant 3 : index"));
    assert!(mlir.contains("arith.constant 1 : index"));
    assert!(mlir.contains("arith.addi"));
    assert!(mlir.contains("memref.store %copy_value, %buf0[%gid] : memref<?xf32>"));
}

#[test]
fn test_mlir_materialize_copy_from_padded_view_zero_fills() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    ));

    assert!(mlir.contains("%c_numel = arith.constant 5 : index"));
    assert!(mlir.contains("arith.constant -1 : index"));
    assert!(mlir.contains("arith.cmpi sge"));
    assert!(mlir.contains("arith.cmpi slt"));
    assert!(mlir.contains("arith.andi"));
    assert!(mlir.contains("scf.if"));
    assert!(mlir.contains("scf.yield %copy_zero : f32"));
}

#[test]
fn test_mlir_materialize_copy_from_permuted_2d_view() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[2, 3]).permute(&[1, 0]),
    ));

    assert!(mlir.contains("%c_numel = arith.constant 6 : index"));
    assert!(mlir.contains("arith.divui"));
    assert!(mlir.contains("arith.remui"));
    assert!(mlir.contains("arith.muli"));
    assert!(mlir.contains("memref.load %buf1[%src"));
}

#[test]
fn test_mlir_materialize_copy_composes_multiple_views() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]),
    ));

    assert!(mlir.contains("%c_numel = arith.constant 4 : index"));
    assert!(mlir.contains("arith.divui"));
    assert!(mlir.contains("arith.remui"));
    assert!(mlir.contains("arith.subi"));
    assert!(mlir.contains("memref.load %buf1[%src"));
}

#[test]
fn test_mlir_materialize_copy_from_expanded_zero_stride_view() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[1]).expand(&[4]),
    ));

    assert!(mlir.contains("%c_numel = arith.constant 4 : index"));
    assert!(mlir.contains("memref.load %buf1[%src_v0_0] : memref<?xf32>"));
    assert!(!mlir.contains("scf.if"));
}

#[test]
fn test_mlir_materialize_copy_preserves_uint32_storage_type() {
    let mlir = to_mlir_text(&materialize_copy_kernel(
        DType::UInt32,
        ShapeTracker::contiguous(&[4]),
    ));

    assert!(mlir.contains("memref<?xui32>"));
    assert!(mlir.contains("%copy_zero = arith.constant 0 : ui32"));
    assert!(mlir.contains("memref.load %buf1[%gid] : memref<?xui32>"));
}

#[test]
fn test_mlir_materialize_copy_zero_fill_literals_by_dtype() {
    for (dtype, expected) in [
        (DType::Bool, "%copy_zero = arith.constant false : i1"),
        (
            DType::Float32,
            "%copy_zero = arith.constant 0.000000e+00 : f32",
        ),
        (
            DType::Float64,
            "%copy_zero = arith.constant 0.000000e+00 : f64",
        ),
    ] {
        let mlir = to_mlir_text(&materialize_copy_kernel(
            dtype,
            ShapeTracker::contiguous(&[4]),
        ));
        assert!(
            mlir.contains(expected),
            "missing zero literal {expected} for {dtype:?}\n{mlir}"
        );
    }
}

#[test]
#[should_panic(
    expected = "MaterializeCopy for MXFP requires explicit block/exponent storage lowering"
)]
fn test_mlir_materialize_copy_rejects_mxfp8_until_storage_lowering_exists() {
    let _ = to_mlir_text(&materialize_copy_kernel(
        DType::MxFP8,
        ShapeTracker::contiguous(&[4]),
    ));
}

#[test]
#[should_panic(
    expected = "MaterializeCopy for MXFP requires explicit block/exponent storage lowering"
)]
fn test_mlir_materialize_copy_rejects_mxfp4_until_storage_lowering_exists() {
    let _ = to_mlir_text(&materialize_copy_kernel(
        DType::MxFP4,
        ShapeTracker::contiguous(&[4]),
    ));
}

#[test]
fn test_mlir_materialize_copy_names_args_by_binding_slot_not_storage_id() {
    let st = ShapeTracker::contiguous(&[4]);
    let kernel = FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 77,
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let mlir = to_mlir_text(&kernel);

    assert!(mlir.contains("func.func @molt_kernel(%buf0: memref<?xf32>, %buf1: memref<?xf32>)"));
    assert!(mlir.contains("memref.load %buf1[%gid]"));
    assert!(mlir.contains("memref.store %copy_value, %buf0[%gid]"));
    assert!(!mlir.contains("%buf77"));
}

#[test]
fn test_mlir_math_ops() {
    for (op, expected) in [
        (PrimitiveOp::Exp2, "math.exp2"),
        (PrimitiveOp::Log2, "math.log2"),
        (PrimitiveOp::Sin, "math.sin"),
        (PrimitiveOp::Sqrt, "math.sqrt"),
        (PrimitiveOp::Trunc, "math.trunc"),
    ] {
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
                    st: ShapeTracker::contiguous(&[32]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[32]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [32, 1, 1],
            local: [32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let mlir = to_mlir_text(&kernel);
        assert!(
            mlir.contains(expected),
            "op {:?} should emit {}",
            op,
            expected
        );
    }
}
