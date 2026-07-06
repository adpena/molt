use super::*;
use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
};
use crate::shapetracker::ShapeTracker;

fn make_elementwise_kernel(op: PrimitiveOp, dst_dtype: DType) -> FusedKernel {
    if matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
        let input_st = ShapeTracker::contiguous(&[1024]);
        return FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                op,
                vec![FusedSrc::Buf(1)],
                dst_dtype,
                ReductionDomain::from_axis(&[1024], 0),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[1]),
                    dtype: dst_dtype,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: input_st,
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
    }

    let st = ShapeTracker::contiguous(&[1024]);
    let srcs = match op.arity() {
        1 => vec![FusedSrc::Buf(1)],
        2 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
        3 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
        _ => unreachable!(),
    };

    let n_inputs = op.arity();
    let mut bufs = vec![BufferBinding {
        buf_id: 0,
        st: st.clone(),
        dtype: dst_dtype,
        access: BufferAccess::Write,
    }];
    for i in 0..n_inputs {
        bufs.push(BufferBinding {
            buf_id: i + 1,
            st: st.clone(),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        });
    }

    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(op, srcs, dst_dtype)],
        bufs,
        grid: [1024, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_materialize_copy_kernel(dtype: DType, src_st: ShapeTracker) -> FusedKernel {
    let numel = src_st.numel();
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(src_st.shape()),
                dtype,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
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

fn make_reduce_kernel(op: PrimitiveOp, input_st: ShapeTracker, axis: usize) -> FusedKernel {
    let domain = ReductionDomain::from_axis(input_st.shape(), axis);
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            op,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            domain.clone(),
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&domain.output_shape),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: input_st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [domain.output_numel() as u32, 1, 1],
        local: [domain.output_numel().clamp(1, 256) as u32, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_mil_render_add() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Add, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("add(x=input_1, y=input_2)"));
    assert!(source.contains("mil_program"));
}

#[test]
fn test_mil_names_inputs_by_binding_slot_not_storage_id() {
    let st = ShapeTracker::contiguous(&[4]);
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
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[4], fp32>"));
    assert!(source.contains("input_2: tensor<[4], fp32>"));
    assert!(source.contains("add(x=input_1, y=input_2)"));
    assert!(!source.contains("input_77"));
}

#[test]
fn test_mil_compute_materializes_same_storage_distinct_views() {
    let st = ShapeTracker::contiguous(&[4]);
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
                st: st.clone(),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st: st.flip(0),
                dtype: DType::Float32,
                access: BufferAccess::Read,
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

    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], fp32>"));
    assert!(source.contains("input_2: tensor<[4], fp32>"));
    assert!(source.contains("gather(x=input_1"));
    assert!(source.contains("add(x=raw_input_1, y=input_2)"));
    assert!(!source.contains("input_77"));
}

#[test]
fn test_mil_materialize_copy_from_flipped_view() {
    let kernel =
        make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[4]).flip(0));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], fp32>"));
    assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
    assert!(source.contains("const(val=3, dtype=int32)"));
    assert!(source.contains("sub(x="));
    assert!(source.contains("gather(x=input_1"));
    assert!(source.contains("return raw_input_1: tensor<[4], fp32>"));
    assert!(!source.contains("input_77"));
}

#[test]
fn test_mil_materialize_copy_contiguous_returns_input_slot() {
    let kernel = make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[4]));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[4], fp32>"));
    assert!(source.contains("return input_1: tensor<[4], fp32>"));
    assert!(!source.contains("gather("));
    assert!(!source.contains("idx_1 = range_1d"));
    assert!(!source.contains("input_77"));
}

#[test]
fn test_mil_materialize_copy_from_padded_view_zero_fills() {
    let kernel = make_materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    );
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], fp32>"));
    assert!(source.contains("idx_1 = range_1d(start=0, end=5, step=1, dtype=\"int32\")"));
    assert!(source.contains("less(x=idx_1, y=const(val=1, dtype=int32))"));
    assert!(source.contains("logical_not"));
    assert!(source.contains("logical_and"));
    assert!(source.contains("select(cond="));
    assert!(source.contains("gather(x=input_1"));
    assert!(source.contains("view_input_1 = select"));
    assert!(source.contains("b=const(val=0, dtype=fp32)"));
    assert!(source.contains("return view_input_1: tensor<[5], fp32>"));
}

#[test]
fn test_mil_materialize_copy_padded_safe_index_feeds_gather_before_zero_fill() {
    let kernel = make_materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    );
    let source = MilRenderer.render(&kernel);

    let safe_index_pos = source
        .find("view1_safe")
        .expect("padded MIL materialization must emit a safe gather index");
    let gather_pos = source
        .find("raw_input_1 = gather")
        .expect("padded MIL materialization must gather from the safe index");
    let zero_fill_pos = source
        .find("view_input_1 = select")
        .expect("padded MIL materialization must zero-fill after gather");

    assert!(safe_index_pos < gather_pos);
    assert!(gather_pos < zero_fill_pos);
    assert!(source.contains("indices=view1_safe"));
    assert!(source.contains("b=const(val=0, dtype=fp32)"));
}

#[test]
fn test_mil_materialize_copy_composes_multiple_views() {
    let kernel = make_materialize_copy_kernel(
        DType::Float32,
        ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]),
    );
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
    assert!(source.contains("floor_div"));
    assert!(source.contains("mod"));
    assert!(source.contains("const(val=3, dtype=int32)"));
    assert!(source.contains("sub(x="));
    assert!(source.contains("gather(x=input_1"));
}

#[test]
fn test_mil_materialize_copy_from_expanded_zero_stride_view() {
    let kernel =
        make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[1]).expand(&[4]));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
    assert!(source.contains("mul(x=idx_1, y=const(val=0, dtype=int32))"));
    assert!(source.contains("gather(x=input_1"));
    assert!(!source.contains("view_input_1 = select"));
}

#[test]
fn test_mil_materialize_copy_uint32_from_flipped_view() {
    let kernel =
        make_materialize_copy_kernel(DType::UInt32, ShapeTracker::contiguous(&[4]).flip(0));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], uint32>"));
    assert!(source.contains("gather(x=input_1"));
    assert!(source.contains("return raw_input_1: tensor<[4], uint32>"));
}

#[test]
fn test_mil_materialize_copy_int16_padded_zero_fills_with_int16_zero() {
    let kernel =
        make_materialize_copy_kernel(DType::Int16, ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], int16>"));
    assert!(source.contains("view_input_1 = select"));
    assert!(source.contains("b=const(val=0, dtype=int16)"));
    assert!(source.contains("return view_input_1: tensor<[5], int16>"));
}

#[test]
fn test_mil_materialize_copy_bool_padded_zero_fills_with_false() {
    let kernel =
        make_materialize_copy_kernel(DType::Bool, ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]));
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[*], bool>"));
    assert!(source.contains("view_input_1 = select"));
    assert!(source.contains("b=const(val=false, dtype=bool)"));
    assert!(source.contains("return view_input_1: tensor<[5], bool>"));
}

#[test]
fn test_mil_materialize_copy_supported_integer_zero_literals_by_dtype() {
    for (dtype, expected) in [
        (DType::Int8, "b=const(val=0, dtype=int8)"),
        (DType::UInt8, "b=const(val=0, dtype=uint8)"),
        (DType::Int16, "b=const(val=0, dtype=int16)"),
        (DType::UInt16, "b=const(val=0, dtype=uint16)"),
        (DType::Int32, "b=const(val=0, dtype=int32)"),
        (DType::UInt32, "b=const(val=0, dtype=uint32)"),
    ] {
        let kernel =
            make_materialize_copy_kernel(dtype, ShapeTracker::contiguous(&[2]).pad(&[(1, 1)]));
        let source = MilRenderer.render(&kernel);
        assert!(
            source.contains(expected),
            "missing zero literal {expected} for {dtype:?}\n{source}"
        );
    }
}

#[test]
fn test_mil_materialize_copy_rejects_unverified_storage_dtypes() {
    for (dtype, expected) in [
        (
            DType::BFloat16,
            "BFloat16 requires a distinct bf16 storage proof",
        ),
        (DType::Int64, "64-bit dtypes requires MIL compile"),
        (DType::UInt64, "64-bit dtypes requires MIL compile"),
        (DType::Float64, "64-bit dtypes requires MIL compile"),
        (
            DType::MxFP8,
            "MXFP requires explicit block/exponent storage lowering",
        ),
        (
            DType::MxFP4,
            "MXFP requires explicit block/exponent storage lowering",
        ),
    ] {
        let kernel = make_materialize_copy_kernel(dtype, ShapeTracker::contiguous(&[4]).flip(0));
        let panic = std::panic::catch_unwind(|| {
            let _ = MilRenderer.render(&kernel);
        })
        .expect_err("unsupported MIL materialize dtype should panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic>");
        assert!(
            message.contains(expected),
            "panic for {dtype:?} should contain {expected:?}, got {message:?}"
        );
    }
}

#[test]
#[should_panic(expected = "requires 1..=i32::MAX elements")]
fn test_mil_materialize_copy_rejects_zero_numel() {
    let kernel = make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[0]));

    let _ = MilRenderer.render(&kernel);
}

#[test]
#[should_panic(expected = "requires 1..=i32::MAX elements")]
fn test_mil_materialize_copy_rejects_too_large_numel() {
    let too_large = i32::MAX as usize + 1;
    let kernel =
        make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[too_large]));

    let _ = MilRenderer.render(&kernel);
}

#[test]
#[should_panic(expected = "offset value")]
fn test_mil_materialize_copy_rejects_out_of_range_offset_constant() {
    let huge = i32::MAX as usize + 2;
    let st = ShapeTracker::contiguous(&[huge]).shrink(&[(huge - 1, huge)]);
    let kernel = make_materialize_copy_kernel(DType::Float32, st);

    let _ = MilRenderer.render(&kernel);
}

#[test]
#[should_panic(expected = "stride value")]
fn test_mil_materialize_copy_rejects_out_of_range_stride_constant() {
    let huge = i32::MAX as usize + 1;
    let st = ShapeTracker::contiguous(&[2, huge])
        .permute(&[1, 0])
        .shrink(&[(0, 1), (0, 1)]);
    let kernel = make_materialize_copy_kernel(DType::Float32, st);

    let _ = MilRenderer.render(&kernel);
}

#[test]
#[should_panic(expected = "physical offset value")]
fn test_mil_materialize_copy_rejects_out_of_range_physical_offset() {
    let stride_fits_i32 = i32::MAX as usize;
    let st = ShapeTracker::contiguous(&[3, stride_fits_i32]).shrink(&[(0, 3), (0, 1)]);
    let kernel = make_materialize_copy_kernel(DType::Float32, st);

    let _ = MilRenderer.render(&kernel);
}

#[test]
fn test_mil_render_mul() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Mul, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("mul(x=input_1, y=input_2)"));
}

#[test]
fn test_mil_render_exp2() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Exp2, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("pow(x=const(val=2, dtype=fp32), y=input_1)"));
}

#[test]
fn test_mil_render_reciprocal() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Reciprocal, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("real_div(x=const(val=1, dtype=fp32), y=input_1)"));
}

#[test]
fn test_mil_render_where() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Where, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("select(cond=input_1, a=input_2, b=input_3)"));
}

#[test]
fn test_mil_render_cmplt() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Cmplt, DType::Bool);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("less(x=input_1, y=input_2)"));
}

#[test]
fn test_mil_render_neg() {
    let kernel = make_elementwise_kernel(PrimitiveOp::Neg, DType::Float32);
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("mul(x=input_1, y=const(val=-1, dtype=fp32))"));
}

#[test]
fn test_mil_render_reduce_sum() {
    let st = ShapeTracker::contiguous(&[1024]);
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[1024], 0),
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
                st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let renderer = MilRenderer;
    let source = renderer.render(&kernel);
    assert!(source.contains("reduce_sum(x=input_1, axes=[0], keep_dims=false)"));
}

#[test]
fn test_mil_render_reduce_sum_axis0_keeps_ranked_input_shape() {
    let kernel = make_reduce_kernel(PrimitiveOp::ReduceSum, ShapeTracker::contiguous(&[2, 3]), 0);
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[2, 3], fp32>"));
    assert!(source.contains("reduce_sum(x=input_1, axes=[0], keep_dims=false)"));
    assert!(source.contains("return v0: tensor<[3], fp32>"));
    assert!(!source.contains("input_1: tensor<[6], fp32>"));
    assert!(!source.contains("reduce_sum(x=v0_src0_shape"));
}

#[test]
fn test_mil_render_reduce_max_axis1_keeps_ranked_input_shape() {
    let kernel = make_reduce_kernel(PrimitiveOp::ReduceMax, ShapeTracker::contiguous(&[2, 3]), 1);
    let source = MilRenderer.render(&kernel);

    assert!(source.contains("input_1: tensor<[2, 3], fp32>"));
    assert!(source.contains("reduce_max(x=input_1, axes=[1], keep_dims=false)"));
    assert!(source.contains("return v0: tensor<[2], fp32>"));
}

#[test]
fn test_mil_render_noncontiguous_reduce_reshapes_gather_before_axis_reduce() {
    let input_st = ShapeTracker::contiguous(&[6]).flip(0).reshape(&[2, 3]);
    let kernel = make_reduce_kernel(PrimitiveOp::ReduceSum, input_st, 0);
    let source = MilRenderer.render(&kernel);

    let gather_pos = source
        .find("raw_input_1 = gather")
        .expect("non-contiguous reduction input must gather physical storage");
    let reshape_pos = source
        .find("logical_input_1 = reshape(x=raw_input_1, shape=[2, 3])")
        .expect("gathered flat view must be restored to the logical reduction rank");
    let reduce_pos = source
        .find("reduce_sum(x=logical_input_1, axes=[0], keep_dims=false)")
        .expect("axis reduction must consume the ranked logical view");

    assert!(gather_pos < reshape_pos);
    assert!(reshape_pos < reduce_pos);
    assert!(source.contains("input_1: tensor<[*], fp32>"));
    assert!(source.contains("return v0: tensor<[3], fp32>"));
}

#[test]
fn test_mil_render_masked_reduce_zero_fills_then_reshapes_before_axis_reduce() {
    let input_st = ShapeTracker::contiguous(&[1, 3]).pad(&[(1, 0), (0, 0)]);
    let kernel = make_reduce_kernel(PrimitiveOp::ReduceSum, input_st, 0);
    let source = MilRenderer.render(&kernel);

    let safe_index_pos = source
        .find("view1_safe")
        .expect("masked reduction input must select a safe gather index");
    let gather_pos = source
        .find("raw_input_1 = gather")
        .expect("masked reduction input must gather physical storage");
    let zero_fill_pos = source
        .find("view_input_1 = select")
        .expect("masked reduction input must zero-fill invalid lanes");
    let reshape_pos = source
        .find("logical_input_1 = reshape(x=view_input_1, shape=[2, 3])")
        .expect("zero-filled flat view must be restored to the logical reduction rank");
    let reduce_pos = source
        .find("reduce_sum(x=logical_input_1, axes=[0], keep_dims=false)")
        .expect("axis reduction must consume the ranked zero-filled view");

    assert!(safe_index_pos < gather_pos);
    assert!(gather_pos < zero_fill_pos);
    assert!(zero_fill_pos < reshape_pos);
    assert!(reshape_pos < reduce_pos);
    assert!(source.contains("b=const(val=0, dtype=fp32)"));
}

#[test]
fn test_mil_render_all_26_ops() {
    // Verify that every primitive op can be rendered without panic.
    let renderer = MilRenderer;
    for op in PrimitiveOp::ALL {
        let dst_dtype = if matches!(
            op,
            PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
        ) {
            DType::Bool
        } else {
            DType::Float32
        };
        let kernel = make_elementwise_kernel(op, dst_dtype);
        let source = renderer.render(&kernel);
        assert!(!source.is_empty(), "Empty render for op {:?}", op);
        assert!(
            source.contains("mil_program"),
            "Missing header for op {:?}",
            op
        );
    }
}

#[test]
fn test_mil_type_mapping() {
    assert_eq!(MilRenderer::mil_type(DType::Float16), "fp16");
    assert_eq!(MilRenderer::mil_type(DType::Float32), "fp32");
    assert_eq!(MilRenderer::mil_type(DType::Int32), "int32");
    assert_eq!(MilRenderer::mil_type(DType::Bool), "bool");
}

#[test]
fn test_mil_format_const() {
    assert_eq!(MilRenderer::format_const(1.0, DType::Bool), "true");
    assert_eq!(MilRenderer::format_const(0.0, DType::Bool), "false");
    assert_eq!(MilRenderer::format_const(42.0, DType::Int32), "42");
    assert_eq!(
        MilRenderer::format_const(f64::INFINITY, DType::Float32),
        "inf"
    );
    assert_eq!(
        MilRenderer::format_const(f64::NEG_INFINITY, DType::Float32),
        "-inf"
    );
}
