//! Cross-renderer conformance tests.
//!
//! Renders a set of reference FusedKernels (softmax, matmul-like, RMSNorm,
//! attention-like) with ALL 6 renderers (MSL, WGSL, GLSL, CUDA, HIP, OpenCL)
//! and verifies:
//! 1. All 6 outputs contain the expected mathematical operations
//! 2. No renderer produces obviously invalid syntax for its language
//! 3. Type narrowing is applied correctly per renderer
//! 4. Structural consistency across renderers

use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::cuda::CudaRenderer;
use molt_gpu::render::glsl::GlslRenderer;
use molt_gpu::render::hip::HipRenderer;
use molt_gpu::render::msl::MslRenderer;
#[cfg(feature = "metal4")]
use molt_gpu::render::msl4::{Metal4Support, Msl4Renderer};
use molt_gpu::render::opencl::OpenClRenderer;
use molt_gpu::render::wgsl::WgslRenderer;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
    Renderer,
};
use molt_gpu::shapetracker::ShapeTracker;

/// All 6 renderers with their names.
fn all_renderers() -> Vec<(&'static str, Box<dyn Renderer>)> {
    vec![
        ("MSL", Box::new(MslRenderer) as Box<dyn Renderer>),
        ("WGSL", Box::new(WgslRenderer::new()) as Box<dyn Renderer>),
        ("GLSL", Box::new(GlslRenderer) as Box<dyn Renderer>),
        ("CUDA", Box::new(CudaRenderer) as Box<dyn Renderer>),
        ("HIP", Box::new(HipRenderer) as Box<dyn Renderer>),
        (
            "OpenCL",
            Box::new(OpenClRenderer { has_fp64: false }) as Box<dyn Renderer>,
        ),
    ]
}

/// Softmax-like kernel: exp(x - max) / sum(exp(x - max))
/// Simplified as: reduce_sum(exp2(x - reduce_max(x)))
fn make_reduce_sum_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[n, reduce_size], 1),
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
                st: ShapeTracker::contiguous(&[n * reduce_size]),
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

/// Reduce-max kernel (used in softmax denominator computation).
fn make_reduce_max_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[n, reduce_size], 1),
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
                st: ShapeTracker::contiguous(&[n * reduce_size]),
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

/// RMSNorm-like: x * rsqrt(mean(x^2) + eps)
/// Simplified as fused: mul(x, reciprocal(sqrt(reduce_sum(mul(x, x)))))
fn make_elementwise_chain_kernel(n: usize) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![
            // v0 = buf1 * buf2 (element-wise multiply)
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                DType::Float32,
            ),
            // v1 = sqrt(v0)
            FusedOp::elementwise(PrimitiveOp::Sqrt, vec![FusedSrc::Op(0)], DType::Float32),
            // v2 = reciprocal(v1)
            FusedOp::elementwise(
                PrimitiveOp::Reciprocal,
                vec![FusedSrc::Op(1)],
                DType::Float32,
            ),
        ],
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

/// Attention score kernel: exp2(x) * y (used in softmax @ V).
fn make_exp2_mul_kernel(n: usize) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Buf(1)], DType::Float32),
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![FusedSrc::Op(0), FusedSrc::Buf(2)],
                DType::Float32,
            ),
        ],
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

fn make_mxfp_storage_kernel() -> FusedKernel {
    FusedKernel {
        body: KernelBody::Compute,
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::MxFP8,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::MxFP8,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::MxFP8,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::MxFP8,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

/// Fused reduce with pre-reduce elementwise: reduce_sum(exp2(x - const))
fn make_fused_softmax_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        body: Default::default(),
        ops: vec![
            // v0 = buf1 - 5.0 (subtract max)
            FusedOp::elementwise(
                PrimitiveOp::Sub,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: 5.0,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            // v1 = exp2(v0)
            FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(0)], DType::Float32),
            // v2 = reduce_sum(v1)
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[n, reduce_size], 1),
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n * reduce_size]),
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

fn make_attention_core_masked_sdpa_row_chain_kernels() -> Vec<(&'static str, FusedKernel)> {
    let n_keys = 4;
    let head_dim = 2;
    let scale = 1.0 / (head_dim as f64).sqrt();

    vec![
        (
            "qk_reduce_sum_scale",
            FusedKernel {
                body: Default::default(),
                ops: vec![
                    FusedOp::elementwise(
                        PrimitiveOp::Mul,
                        vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                        DType::Float32,
                    ),
                    FusedOp::reduction(
                        PrimitiveOp::ReduceSum,
                        vec![FusedSrc::Op(0)],
                        DType::Float32,
                        ReductionDomain::from_axis(&[n_keys, head_dim], 1),
                    ),
                    FusedOp::elementwise(
                        PrimitiveOp::Mul,
                        vec![
                            FusedSrc::Op(1),
                            FusedSrc::Const {
                                val: scale,
                                dtype: DType::Float32,
                            },
                        ],
                        DType::Float32,
                    ),
                ],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: ShapeTracker::contiguous(&[n_keys, head_dim]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: 2,
                        st: ShapeTracker::contiguous(&[n_keys, head_dim]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n_keys as u32, 1, 1],
                local: [n_keys as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "mask_where",
            FusedKernel {
                body: Default::default(),
                ops: vec![FusedOp::elementwise(
                    PrimitiveOp::Where,
                    vec![
                        FusedSrc::Buf(1),
                        FusedSrc::Buf(2),
                        FusedSrc::Const {
                            val: -1.0e9,
                            dtype: DType::Float32,
                        },
                    ],
                    DType::Float32,
                )],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Bool,
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: 2,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n_keys as u32, 1, 1],
                local: [n_keys as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "row_reduce_max",
            FusedKernel {
                body: Default::default(),
                ops: vec![FusedOp::reduction(
                    PrimitiveOp::ReduceMax,
                    vec![FusedSrc::Buf(1)],
                    DType::Float32,
                    ReductionDomain::from_axis(&[n_keys], 0),
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
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [1, 1, 1],
                local: [1, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "subtract_log2e_exp2",
            FusedKernel {
                body: Default::default(),
                ops: vec![
                    FusedOp::elementwise(
                        PrimitiveOp::Sub,
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
                                val: std::f64::consts::LOG2_E,
                                dtype: DType::Float32,
                            },
                        ],
                        DType::Float32,
                    ),
                    FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(1)], DType::Float32),
                ],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n_keys as u32, 1, 1],
                local: [n_keys as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "row_reduce_sum",
            FusedKernel {
                body: Default::default(),
                ops: vec![FusedOp::reduction(
                    PrimitiveOp::ReduceSum,
                    vec![FusedSrc::Buf(1)],
                    DType::Float32,
                    ReductionDomain::from_axis(&[n_keys], 0),
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
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [1, 1, 1],
                local: [1, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "reciprocal_probs",
            FusedKernel {
                body: Default::default(),
                ops: vec![
                    FusedOp::elementwise(
                        PrimitiveOp::Reciprocal,
                        vec![FusedSrc::Const {
                            val: 3.0,
                            dtype: DType::Float32,
                        }],
                        DType::Float32,
                    ),
                    FusedOp::elementwise(
                        PrimitiveOp::Mul,
                        vec![FusedSrc::Buf(1), FusedSrc::Op(0)],
                        DType::Float32,
                    ),
                ],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [n_keys as u32, 1, 1],
                local: [n_keys as u32, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
        (
            "value_projection_reduce_sum",
            FusedKernel {
                body: Default::default(),
                ops: vec![
                    FusedOp::elementwise(
                        PrimitiveOp::Mul,
                        vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                        DType::Float32,
                    ),
                    FusedOp::reduction(
                        PrimitiveOp::ReduceSum,
                        vec![FusedSrc::Op(0)],
                        DType::Float32,
                        ReductionDomain::from_axis(&[n_keys], 0),
                    ),
                ],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[1]),
                        dtype: DType::Float32,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                    BufferBinding {
                        buf_id: 2,
                        st: ShapeTracker::contiguous(&[n_keys]),
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [1, 1, 1],
                local: [1, 1, 1],
                spec: None,
                vectorize_width: 1,
            },
        ),
    ]
}

fn make_reduce_uses_nonlast_prefix_source_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        body: KernelBody::Compute,
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
            FusedOp::elementwise(
                PrimitiveOp::Add,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: 1.0,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(0)],
                DType::Float32,
                ReductionDomain::from_axis(&[n, reduce_size], 1),
            ),
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n * reduce_size]),
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

fn make_same_storage_distinct_view_kernel() -> FusedKernel {
    let st = ShapeTracker::contiguous(&[4]);
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
    }
}

#[test]
fn test_reduction_accumulates_declared_source_not_last_prefix_op() {
    let kernel = make_reduce_uses_nonlast_prefix_source_kernel(4, 8);

    for (name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        let (expected, forbidden) = match name {
            "WGSL" | "GLSL" => ("acc = acc + v0", "acc = acc + v1"),
            _ => ("acc += v0", "acc += v1"),
        };
        assert!(
            source.contains(expected),
            "{} must accumulate the reduce op's declared source v0\n{}",
            name,
            source
        );
        assert!(
            !source.contains(forbidden),
            "{} must not reduce the last pre-reduce op v1\n{}",
            name,
            source
        );
    }
}

fn make_masked_padded_view_kernel() -> FusedKernel {
    let st = ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]);
    FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Neg,
            vec![FusedSrc::Buf(1)],
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
                buf_id: 77,
                st,
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [5, 1, 1],
        local: [5, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_materialize_flip_copy_kernel() -> FusedKernel {
    let st = ShapeTracker::contiguous(&[4]).flip(0);
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(&[4]),
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
    }
}

fn make_materialize_u32_flip_copy_kernel() -> FusedKernel {
    let st = ShapeTracker::contiguous(&[4]).flip(0);
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::UInt32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 77,
                st,
                dtype: DType::UInt32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

fn make_materialize_padded_copy_kernel() -> FusedKernel {
    let st = ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]);
    FusedKernel {
        body: KernelBody::MaterializeCopy,
        ops: Vec::new(),
        bufs: vec![
            BufferBinding {
                buf_id: 100,
                st: ShapeTracker::contiguous(&[5]),
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
        grid: [5, 1, 1],
        local: [5, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

// ---- Cross-renderer conformance tests ----

#[test]
fn test_cross_all_renderers_produce_output() {
    let kernels: Vec<(&str, FusedKernel)> = vec![
        ("reduce_sum", make_reduce_sum_kernel(32, 8)),
        ("reduce_max", make_reduce_max_kernel(32, 8)),
        ("elementwise_chain", make_elementwise_chain_kernel(256)),
        ("exp2_mul", make_exp2_mul_kernel(128)),
        ("fused_softmax", make_fused_softmax_kernel(16, 16)),
    ];

    for (kernel_name, kernel) in &kernels {
        for (renderer_name, renderer) in all_renderers() {
            let source = renderer.render(kernel);
            assert!(
                !source.is_empty(),
                "{} renderer produced empty output for {} kernel",
                renderer_name,
                kernel_name,
            );
            assert!(
                source.len() > 50,
                "{} renderer produced suspiciously short output ({} bytes) for {} kernel",
                renderer_name,
                source.len(),
                kernel_name,
            );
        }
    }
}

#[test]
fn test_cross_renderers_render_attention_core_masked_sdpa_row_chain() {
    let kernels = make_attention_core_masked_sdpa_row_chain_kernels();

    for (stage_name, kernel) in &kernels {
        for (renderer_name, renderer) in all_renderers() {
            let source = renderer.render(kernel);
            assert!(
                !source.is_empty(),
                "{} renderer produced empty output for {} stage",
                renderer_name,
                stage_name,
            );

            match *stage_name {
                "qk_reduce_sum_scale" => {
                    assert!(
                        source.contains("acc") && source.contains("v0") && source.contains('*'),
                        "{} must render QK Mul -> ReduceSum(axis=1) for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                    assert!(
                        source.contains("0.707106"),
                        "{} must render the post-QK scale multiply for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "mask_where" => {
                    let has_where = if renderer_name == "WGSL" {
                        source.contains("select(")
                    } else {
                        source.contains(" ? ")
                    };
                    assert!(
                        has_where && source.contains("-1000000000"),
                        "{} must render Where(mask, scaled, -1e9) for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "row_reduce_max" => {
                    assert!(
                        source.contains("acc") && source.contains("max"),
                        "{} must render ReduceMax(axis=0) for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "subtract_log2e_exp2" => {
                    let exp2_token = match renderer_name {
                        "CUDA" | "HIP" => "exp2f(",
                        _ => "exp2(",
                    };
                    assert!(
                        source.contains('-')
                            && source.contains("1.442695")
                            && source.contains(exp2_token),
                        "{} must render Sub -> Mul(LOG2_E) -> Exp2 for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "row_reduce_sum" => {
                    assert!(
                        source.contains("acc")
                            && (source.contains("acc +") || source.contains("acc +=")),
                        "{} must render ReduceSum(axis=0) for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "reciprocal_probs" => {
                    assert!(
                        (source.contains("1.0")
                            || source.contains("1.0f")
                            || source.contains("f32(1.0)"))
                            && source.contains('*'),
                        "{} must render Reciprocal -> Mul probabilities for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                "value_projection_reduce_sum" => {
                    assert!(
                        source.contains("acc") && source.contains("v0") && source.contains('*'),
                        "{} must render probs/value Mul -> ReduceSum(axis=0) for {}:\n{}",
                        renderer_name,
                        stage_name,
                        source,
                    );
                }
                _ => unreachable!("unexpected SDPA stage {stage_name}"),
            }
        }
    }
}

macro_rules! assert_renderer_rejects_mxfp_storage {
    ($test_name:ident, $renderer:expr) => {
        #[test]
        #[should_panic(expected = "MXFP requires explicit block/exponent storage lowering")]
        fn $test_name() {
            let kernel = make_mxfp_storage_kernel();
            let renderer = $renderer;
            renderer.render(&kernel);
        }
    };
}

assert_renderer_rejects_mxfp_storage!(
    test_msl_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    MslRenderer
);
#[cfg(feature = "metal4")]
assert_renderer_rejects_mxfp_storage!(
    test_msl4_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    Msl4Renderer::with_support(Metal4Support::None)
);
assert_renderer_rejects_mxfp_storage!(
    test_wgsl_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    WgslRenderer::new()
);
assert_renderer_rejects_mxfp_storage!(
    test_glsl_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    GlslRenderer
);
assert_renderer_rejects_mxfp_storage!(
    test_cuda_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    CudaRenderer
);
assert_renderer_rejects_mxfp_storage!(
    test_hip_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    HipRenderer
);
assert_renderer_rejects_mxfp_storage!(
    test_opencl_renderer_rejects_mxfp_until_block_storage_lowering_exists,
    OpenClRenderer { has_fp64: false }
);

#[test]
fn test_cross_renderers_name_parameters_by_binding_slot_not_storage_id() {
    let kernel = make_same_storage_distinct_view_kernel();

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("buf1[u32((3i - i32(gid)))] + buf2[gid]"),
                    "WGSL must read the flipped and contiguous views through distinct slots:\n{}",
                    source,
                );
                assert!(
                    !source.contains("buf77"),
                    "WGSL leaked storage id into parameter names:\n{}",
                    source,
                );
            }
            "GLSL" => {
                assert!(
                    source.contains("u_tex1") && source.contains("u_tex2"),
                    "GLSL must expose both same-storage views as distinct texture slots:\n{}",
                    source,
                );
                assert!(
                    source.contains("(3 - int(gid))"),
                    "GLSL must render the flipped view index for the first slot:\n{}",
                    source,
                );
                assert!(
                    !source.contains("u_tex77"),
                    "GLSL leaked storage id into texture names:\n{}",
                    source,
                );
            }
            "MSL" | "CUDA" | "HIP" | "OpenCL" => {
                assert!(
                    source.contains("buf1[(3 - ((long)(gid)))] + buf2[gid]"),
                    "{} must read the flipped and contiguous views through distinct slots:\n{}",
                    renderer_name,
                    source,
                );
                assert!(
                    !source.contains("buf77"),
                    "{} leaked storage id into parameter names:\n{}",
                    renderer_name,
                    source,
                );
            }
            _ => unreachable!("unexpected renderer {renderer_name}"),
        }
    }
}

#[test]
fn test_cross_renderers_guard_masked_padded_view_reads() {
    let kernel = make_masked_padded_view_kernel();

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("select(0, buf1[u32(select(0i"),
                    "WGSL must guard padded reads through a safe index:\n{}",
                    source,
                );
                assert!(
                    source.contains("i32(gid) >= 1i && i32(gid) < 4i"),
                    "WGSL must emit the pad mask predicate:\n{}",
                    source,
                );
            }
            "GLSL" => {
                assert!(
                    source.contains("? texelFetch(u_tex1"),
                    "GLSL must guard padded texture reads:\n{}",
                    source,
                );
                assert!(
                    source.contains("int(gid) >= 1 && int(gid) < 4"),
                    "GLSL must emit the pad mask predicate:\n{}",
                    source,
                );
            }
            "MSL" | "CUDA" | "HIP" | "OpenCL" => {
                assert!(
                    source.contains("? buf1["),
                    "{} must guard padded buffer reads:\n{}",
                    renderer_name,
                    source,
                );
                assert!(
                    source.contains("((long)(gid)) >= 1 && ((long)(gid)) < 4"),
                    "{} must emit the pad mask predicate:\n{}",
                    renderer_name,
                    source,
                );
            }
            _ => unreachable!("unexpected renderer {renderer_name}"),
        }
    }
}

#[test]
fn test_cross_renderers_emit_materialize_copy_from_flipped_source() {
    let kernel = make_materialize_flip_copy_kernel();

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("buf0[gid] = buf1[u32((3i - i32(gid)))]"),
                    "WGSL must copy from the flipped source view into contiguous output:\n{}",
                    source,
                );
            }
            "GLSL" => {
                assert!(
                    source.contains("result[comp] = float(texelFetch(u_tex1")
                        && source.contains("(3 - int(gid))"),
                    "GLSL must copy flipped source texels into contiguous packed output:\n{}",
                    source,
                );
            }
            "MSL" | "CUDA" | "HIP" | "OpenCL" => {
                assert!(
                    source.contains("buf0[gid] = buf1[(3 - ((long)(gid)))]"),
                    "{} must copy from the flipped source view into contiguous output:\n{}",
                    renderer_name,
                    source,
                );
            }
            _ => unreachable!("unexpected renderer {renderer_name}"),
        }
    }
}

#[test]
fn test_cross_renderers_emit_uint32_materialize_copy_body() {
    let kernel = make_materialize_u32_flip_copy_kernel();

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("array<u32>"),
                    "WGSL UInt32 MaterializeCopy must use u32 storage:\n{}",
                    source,
                );
                assert!(
                    source.contains("buf0[gid] = buf1[u32((3i - i32(gid)))]"),
                    "WGSL UInt32 MaterializeCopy must copy from the flipped view:\n{}",
                    source,
                );
            }
            "MSL" => {
                assert!(
                    source.contains("device uint* buf0")
                        && source.contains("const device uint* buf1")
                        && source.contains("buf0[gid] = buf1[(3 - ((long)(gid)))]"),
                    "MSL UInt32 MaterializeCopy must use uint buffers and flipped indexing:\n{}",
                    source,
                );
            }
            "CUDA" | "HIP" => {
                assert!(
                    source.contains("unsigned int* buf0")
                        && source.contains("const unsigned int* buf1")
                        && source.contains("buf0[gid] = buf1[(3 - ((long)(gid)))]"),
                    "{} UInt32 MaterializeCopy must use unsigned int buffers and flipped indexing:\n{}",
                    renderer_name,
                    source,
                );
            }
            "OpenCL" => {
                assert!(
                    source.contains("__global uint * restrict buf0")
                        && source.contains("__global const uint * restrict buf1")
                        && source.contains("buf0[gid] = buf1[(3 - ((long)(gid)))]"),
                    "OpenCL UInt32 MaterializeCopy must use uint buffers and flipped indexing:\n{}",
                    source,
                );
            }
            "GLSL" => {
                assert!(
                    source.contains("result[comp] = float(texelFetch(u_tex1")
                        && source.contains("(3 - int(gid))"),
                    "GLSL UInt32 MaterializeCopy must copy the flipped texture source:\n{}",
                    source,
                );
            }
            _ => unreachable!("unexpected renderer {renderer_name}"),
        }
        assert!(
            !source.contains(" v0 = ") && !source.contains("var v0"),
            "{} UInt32 MaterializeCopy must not render a compute op chain:\n{}",
            renderer_name,
            source,
        );
    }
}

#[test]
fn test_cross_renderers_emit_materialize_copy_from_padded_source() {
    let kernel = make_materialize_padded_copy_kernel();

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("buf0[gid] = select(0, buf1[u32(select(0i"),
                    "WGSL must emit a guarded MaterializeCopy store:\n{}",
                    source,
                );
                assert!(
                    !source.contains("var v0"),
                    "WGSL MaterializeCopy must not render a compute op chain:\n{}",
                    source,
                );
            }
            "GLSL" => {
                assert!(
                    source.contains("result[comp] = float((")
                        && source.contains("? texelFetch(u_tex1"),
                    "GLSL must emit a guarded MaterializeCopy texture read:\n{}",
                    source,
                );
                assert!(
                    !source.contains(" v0 = "),
                    "GLSL MaterializeCopy must not render a compute op chain:\n{}",
                    source,
                );
            }
            "MSL" | "CUDA" | "HIP" | "OpenCL" => {
                assert!(
                    source.contains("buf0[gid] =") && source.contains("? buf1["),
                    "{} must emit a guarded MaterializeCopy store:\n{}",
                    renderer_name,
                    source,
                );
                assert!(
                    !source.contains(" v0 = "),
                    "{} MaterializeCopy must not render a compute op chain:\n{}",
                    renderer_name,
                    source,
                );
            }
            _ => unreachable!("unexpected renderer {renderer_name}"),
        }
    }
}

#[test]
fn test_cross_all_renderers_contain_entry_point() {
    let kernel = make_elementwise_chain_kernel(64);

    let expected_entries: &[(&str, &str)] = &[
        ("MSL", "molt_kernel"),
        ("WGSL", "molt_kernel"),
        ("GLSL", "void main()"),
        ("CUDA", "molt_kernel"),
        ("HIP", "molt_kernel"),
        ("OpenCL", "molt_kernel"),
    ];

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        let entry = expected_entries
            .iter()
            .find(|(name, _)| *name == renderer_name)
            .map(|(_, entry)| *entry)
            .unwrap();
        assert!(
            source.contains(entry),
            "{} renderer missing entry point '{}' in:\n{}",
            renderer_name,
            entry,
            source,
        );
    }
}

#[test]
fn test_cross_all_renderers_have_bounds_check() {
    let kernel = make_elementwise_chain_kernel(64);

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        // Every renderer should have a bounds check against the output size.
        // GLSL uses gl_FragCoord so the bounds check is implicit in the texture size.
        if renderer_name != "GLSL" {
            assert!(
                source.contains("64") || source.contains("gid"),
                "{} renderer missing bounds check or element count in:\n{}",
                renderer_name,
                source,
            );
        }
    }
}

#[test]
fn test_cross_reduce_renderers_contain_accumulator() {
    let kernel = make_reduce_sum_kernel(32, 8);

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        // All renderers should have an accumulator variable for reductions.
        // GLSL uses a different approach (loop in fragment shader).
        if renderer_name != "GLSL" {
            assert!(
                source.contains("acc"),
                "{} renderer missing accumulator 'acc' for reduce kernel in:\n{}",
                renderer_name,
                source,
            );
        }
    }
}

#[test]
fn test_cross_renderers_correct_math_ops() {
    let kernel = make_exp2_mul_kernel(128);

    let exp2_patterns: &[(&str, &str)] = &[
        ("MSL", "exp2("),
        ("WGSL", "exp2("),
        ("GLSL", "exp2("),
        ("CUDA", "exp2f("),
        ("HIP", "exp2f("),
        ("OpenCL", "exp2("),
    ];

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        let pattern = exp2_patterns
            .iter()
            .find(|(name, _)| *name == renderer_name)
            .map(|(_, p)| *p)
            .unwrap();
        assert!(
            source.contains(pattern),
            "{} renderer missing exp2 pattern '{}' in:\n{}",
            renderer_name,
            pattern,
            source,
        );
    }
}

#[test]
fn test_cross_renderers_type_narrowing() {
    // Use Float64 dtype — should be narrowed for WebGPU/GLSL/Metal
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float64,
        )],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                // WGSL narrows f64 to f32
                assert!(source.contains("f32"), "WGSL should narrow Float64 to f32");
                assert!(!source.contains("f64"), "WGSL should not contain f64");
            }
            "GLSL" => {
                // GLSL narrows f64 to float (f32)
                assert!(
                    source.contains("float"),
                    "GLSL should narrow Float64 to float"
                );
            }
            "MSL" => {
                // Metal narrows f64 to float
                assert!(
                    source.contains("float"),
                    "MSL should narrow Float64 to float"
                );
            }
            "CUDA" | "HIP" => {
                // CUDA/HIP support f64 natively
                assert!(
                    source.contains("double"),
                    "{} should use double for Float64",
                    renderer_name
                );
            }
            "OpenCL" => {
                // OpenCL with has_fp64=false narrows to float
                assert!(
                    source.contains("float"),
                    "OpenCL (no fp64) should narrow to float"
                );
            }
            _ => {}
        }
    }
}

#[test]
fn test_cross_renderers_no_syntax_errors_basic() {
    // Basic syntax validation: check for balanced braces/parens.
    let kernels: Vec<FusedKernel> = vec![
        make_reduce_sum_kernel(16, 4),
        make_elementwise_chain_kernel(64),
        make_exp2_mul_kernel(32),
    ];

    for kernel in &kernels {
        for (renderer_name, renderer) in all_renderers() {
            let source = renderer.render(kernel);

            // Count braces
            let open_braces = source.chars().filter(|c| *c == '{').count();
            let close_braces = source.chars().filter(|c| *c == '}').count();
            assert_eq!(
                open_braces, close_braces,
                "{} renderer has unbalanced braces ({} open, {} close) in:\n{}",
                renderer_name, open_braces, close_braces, source,
            );

            // Count parens
            let open_parens = source.chars().filter(|c| *c == '(').count();
            let close_parens = source.chars().filter(|c| *c == ')').count();
            assert_eq!(
                open_parens, close_parens,
                "{} renderer has unbalanced parens ({} open, {} close) in:\n{}",
                renderer_name, open_parens, close_parens, source,
            );
        }
    }
}

#[test]
fn test_cross_renderers_language_specific_headers() {
    let kernel = make_elementwise_chain_kernel(64);

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "MSL" => {
                assert!(
                    source.contains("#include <metal_stdlib>"),
                    "MSL missing metal_stdlib"
                );
                assert!(
                    source.contains("using namespace metal"),
                    "MSL missing namespace"
                );
            }
            "WGSL" => {
                assert!(source.contains("@compute"), "WGSL missing @compute");
                assert!(
                    source.contains("@workgroup_size"),
                    "WGSL missing @workgroup_size"
                );
            }
            "GLSL" => {
                assert!(source.contains("#version 300 es"), "GLSL missing version");
                assert!(
                    source.contains("precision highp float"),
                    "GLSL missing precision"
                );
            }
            "CUDA" => {
                assert!(source.contains("__global__"), "CUDA missing __global__");
            }
            "HIP" => {
                assert!(source.contains("__global__"), "HIP missing __global__");
            }
            "OpenCL" => {
                assert!(source.contains("__kernel"), "OpenCL missing __kernel");
            }
            _ => {}
        }
    }
}

#[test]
fn test_cross_renderers_where_op_syntax() {
    // WGSL uses select(), others use ternary
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Where,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
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
                dtype: DType::Bool,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
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

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(
                    source.contains("select("),
                    "WGSL must use select() for Where"
                );
                assert!(!source.contains(" ? "), "WGSL must not use ternary");
            }
            "MSL" | "CUDA" | "HIP" | "GLSL" => {
                assert!(
                    source.contains(" ? "),
                    "{} should use ternary for Where",
                    renderer_name
                );
            }
            "OpenCL" => {
                // OpenCL uses select() or ternary depending on implementation
                assert!(
                    source.contains("select(") || source.contains(" ? "),
                    "OpenCL should have Where implementation",
                );
            }
            _ => {}
        }
    }
}

#[test]
fn test_cross_renderers_fused_reduce_pre_reduce_ops() {
    let kernel = make_fused_softmax_kernel(16, 16);

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        // All renderers should emit the sub and exp2 as pre-reduce ops,
        // followed by a reduction accumulation loop.
        // GLSL handles this differently (texture-based).
        if renderer_name != "GLSL" {
            assert!(
                source.contains("acc"),
                "{} renderer missing accumulator in fused reduce kernel",
                renderer_name,
            );
        }
    }
}

#[test]
fn test_cross_renderers_reciprocal_syntax() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::elementwise(
            PrimitiveOp::Reciprocal,
            vec![FusedSrc::Buf(1)],
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
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        // All renderers express reciprocal as 1.0 / x or native_recip
        assert!(
            source.contains("1.0") || source.contains("1.0f") || source.contains("f32(1.0)"),
            "{} renderer missing reciprocal expression in:\n{}",
            renderer_name,
            source,
        );
    }
}
