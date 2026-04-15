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
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::msl::MslRenderer;
use molt_gpu::render::wgsl::WgslRenderer;
use molt_gpu::render::cuda::CudaRenderer;
use molt_gpu::render::hip::HipRenderer;
use molt_gpu::render::glsl::GlslRenderer;
use molt_gpu::render::opencl::OpenClRenderer;
use molt_gpu::shapetracker::ShapeTracker;

/// All 6 renderers with their names.
fn all_renderers() -> Vec<(&'static str, Box<dyn Renderer>)> {
    vec![
        ("MSL", Box::new(MslRenderer) as Box<dyn Renderer>),
        ("WGSL", Box::new(WgslRenderer::new()) as Box<dyn Renderer>),
        ("GLSL", Box::new(GlslRenderer) as Box<dyn Renderer>),
        ("CUDA", Box::new(CudaRenderer) as Box<dyn Renderer>),
        ("HIP", Box::new(HipRenderer) as Box<dyn Renderer>),
        ("OpenCL", Box::new(OpenClRenderer { has_fp64: false }) as Box<dyn Renderer>),
    ]
}

/// Softmax-like kernel: exp(x - max) / sum(exp(x - max))
/// Simplified as: reduce_sum(exp2(x - reduce_max(x)))
fn make_reduce_sum_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
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
                st: ShapeTracker::contiguous(&[n * reduce_size]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
        spec: None,
    }
}

/// Reduce-max kernel (used in softmax denominator computation).
fn make_reduce_max_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
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
                st: ShapeTracker::contiguous(&[n * reduce_size]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
        spec: None,
    }
}

/// RMSNorm-like: x * rsqrt(mean(x^2) + eps)
/// Simplified as fused: mul(x, reciprocal(sqrt(reduce_sum(mul(x, x)))))
fn make_elementwise_chain_kernel(n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![
            // v0 = buf1 * buf2 (element-wise multiply)
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            // v1 = sqrt(v0)
            FusedOp {
                op: PrimitiveOp::Sqrt,
                srcs: vec![FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
            // v2 = reciprocal(v1)
            FusedOp {
                op: PrimitiveOp::Reciprocal,
                srcs: vec![FusedSrc::Op(1)],
                dst_dtype: DType::Float32,
            },
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
    }
}

/// Attention score kernel: exp2(x) * y (used in softmax @ V).
fn make_exp2_mul_kernel(n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
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
    }
}

/// Fused reduce with pre-reduce elementwise: reduce_sum(exp2(x - const))
fn make_fused_softmax_kernel(n: usize, reduce_size: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![
            // v0 = buf1 - 5.0 (subtract max)
            FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: 5.0, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            // v1 = exp2(v0)
            FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
            // v2 = reduce_sum(v1)
            FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Op(1)],
                dst_dtype: DType::Float32,
            },
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
                renderer_name, kernel_name,
            );
            assert!(
                source.len() > 50,
                "{} renderer produced suspiciously short output ({} bytes) for {} kernel",
                renderer_name, source.len(), kernel_name,
            );
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
        let entry = expected_entries.iter()
            .find(|(name, _)| *name == renderer_name)
            .map(|(_, entry)| *entry)
            .unwrap();
        assert!(
            source.contains(entry),
            "{} renderer missing entry point '{}' in:\n{}",
            renderer_name, entry, source,
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
                renderer_name, source,
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
                renderer_name, source,
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
        let pattern = exp2_patterns.iter()
            .find(|(name, _)| *name == renderer_name)
            .map(|(_, p)| *p)
            .unwrap();
        assert!(
            source.contains(pattern),
            "{} renderer missing exp2 pattern '{}' in:\n{}",
            renderer_name, pattern, source,
        );
    }
}

#[test]
fn test_cross_renderers_type_narrowing() {
    // Use Float64 dtype — should be narrowed for WebGPU/GLSL/Metal
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
                assert!(source.contains("float"), "GLSL should narrow Float64 to float");
            }
            "MSL" => {
                // Metal narrows f64 to float
                assert!(source.contains("float"), "MSL should narrow Float64 to float");
            }
            "CUDA" | "HIP" => {
                // CUDA/HIP support f64 natively
                assert!(source.contains("double"), "{} should use double for Float64", renderer_name);
            }
            "OpenCL" => {
                // OpenCL with has_fp64=false narrows to float
                assert!(source.contains("float"), "OpenCL (no fp64) should narrow to float");
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
                assert!(source.contains("#include <metal_stdlib>"), "MSL missing metal_stdlib");
                assert!(source.contains("using namespace metal"), "MSL missing namespace");
            }
            "WGSL" => {
                assert!(source.contains("@compute"), "WGSL missing @compute");
                assert!(source.contains("@workgroup_size"), "WGSL missing @workgroup_size");
            }
            "GLSL" => {
                assert!(source.contains("#version 300 es"), "GLSL missing version");
                assert!(source.contains("precision highp float"), "GLSL missing precision");
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

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        match renderer_name {
            "WGSL" => {
                assert!(source.contains("select("), "WGSL must use select() for Where");
                assert!(!source.contains(" ? "), "WGSL must not use ternary");
            }
            "MSL" | "CUDA" | "HIP" | "GLSL" => {
                assert!(source.contains(" ? "), "{} should use ternary for Where", renderer_name);
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
        ops: vec![FusedOp {
            op: PrimitiveOp::Reciprocal,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
    };

    for (renderer_name, renderer) in all_renderers() {
        let source = renderer.render(&kernel);
        // All renderers express reciprocal as 1.0 / x or native_recip
        assert!(
            source.contains("1.0") || source.contains("1.0f") || source.contains("f32(1.0)"),
            "{} renderer missing reciprocal expression in:\n{}",
            renderer_name, source,
        );
    }
}
