use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::opencl::OpenClRenderer;
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

fn make_unary_kernel(op: PrimitiveOp, n: usize, dtype: DType) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: dtype,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
                spec: None,
    }
}

// --- Basic rendering tests ---

#[test]
fn test_opencl_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("__kernel"), "must have __kernel qualifier");
    assert!(ocl.contains("molt_kernel"), "must have kernel name");
    assert!(ocl.contains("get_global_id(0)"), "must use get_global_id(0)");
    assert!(ocl.contains("__global"), "must have __global buffer qualifiers");
    assert!(ocl.contains("restrict"), "must have restrict qualifier");
    assert!(ocl.contains("buf1[gid] + buf2[gid]"));
}

#[test]
fn test_opencl_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 512);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("buf1[gid] * buf2[gid]"));
}

#[test]
fn test_opencl_render_sub() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Sub, 256);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("buf1[gid] - buf2[gid]"));
}

#[test]
fn test_opencl_kernel_qualifier() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("__kernel void molt_kernel("));
}

#[test]
fn test_opencl_thread_indexing() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("unsigned int gid = get_global_id(0);"));
}

#[test]
fn test_opencl_global_buffer_qualifiers() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    // Output buffer: __global without const
    assert!(ocl.contains("__global float * restrict buf0"));
    // Input buffers: __global const
    assert!(ocl.contains("__global const float * restrict buf1"));
    assert!(ocl.contains("__global const float * restrict buf2"));
}

// --- DType narrowing tests ---

#[test]
fn test_opencl_f64_with_extension() {
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
    let ocl = OpenClRenderer::new(true).render(&kernel);
    assert!(ocl.contains("#pragma OPENCL EXTENSION cl_khr_fp64 : enable"), "must enable fp64 pragma");
    assert!(ocl.contains("double"), "must use double type when fp64 available");
}

#[test]
fn test_opencl_f64_without_extension() {
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
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(!ocl.contains("#pragma OPENCL EXTENSION cl_khr_fp64"), "no fp64 pragma when not supported");
    assert!(!ocl.contains("double"), "f64 should be narrowed to f32 when no fp64");
    assert!(ocl.contains("float"), "should use float instead of double");
}

#[test]
fn test_opencl_i64_native_support() {
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
    let ocl = OpenClRenderer::new(false).render(&kernel);
    // OpenCL supports i64 natively
    assert!(ocl.contains("long"), "OpenCL should support i64 as 'long'");
}

#[test]
fn test_opencl_bool_as_int() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Bool, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None,
    };
    let ocl = OpenClRenderer::new(false).render(&kernel);
    // OpenCL kernels use int for bool
    assert!(ocl.contains("int v0"), "bool should be rendered as int in OpenCL");
}

// --- Op rendering tests ---

#[test]
fn test_opencl_ternary_where() {
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
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains(" ? "), "OpenCL should use ternary operator for Where");
}

#[test]
fn test_opencl_bitcast_as_type() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Bitcast,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
                spec: None,
    };
    let ocl = OpenClRenderer::new(false).render(&kernel);
    // OpenCL uses as_type() for bitcasts
    assert!(ocl.contains("as_int("), "OpenCL should use as_type for bitcast");
}

#[test]
fn test_opencl_math_ops() {
    // exp2
    let kernel = make_unary_kernel(PrimitiveOp::Exp2, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("exp2("));

    // log2
    let kernel = make_unary_kernel(PrimitiveOp::Log2, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("log2("));

    // sin
    let kernel = make_unary_kernel(PrimitiveOp::Sin, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("sin("));

    // sqrt
    let kernel = make_unary_kernel(PrimitiveOp::Sqrt, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("sqrt("));

    // trunc
    let kernel = make_unary_kernel(PrimitiveOp::Trunc, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("trunc("));
}

#[test]
fn test_opencl_reciprocal() {
    let kernel = make_unary_kernel(PrimitiveOp::Reciprocal, 64, DType::Float32);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("1.0f /"));
}

#[test]
fn test_opencl_max_uses_fmax() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Max, 64);
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("fmax("), "OpenCL should use fmax for Max op");
}

// --- Reduce tests ---

#[test]
fn test_opencl_reduce_sum_with_barrier() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [256, 1, 1],
                spec: None,
    };
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("barrier(CLK_LOCAL_MEM_FENCE)"), "reduce must use barrier");
    assert!(ocl.contains("__local"), "reduce must use __local shared memory");
    assert!(ocl.contains("sdata["), "reduce must use shared memory array");
    assert!(ocl.contains("get_local_id(0)"), "reduce must use get_local_id");
    assert!(ocl.contains("get_local_size(0)"), "reduce must use get_local_size");
}

#[test]
fn test_opencl_reduce_max_with_barrier() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [256, 1, 1],
                spec: None,
    };
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("barrier(CLK_LOCAL_MEM_FENCE)"), "reduce must use barrier");
    assert!(ocl.contains("__local"), "reduce must use __local shared memory");
    assert!(ocl.contains("fmax("), "reduce max must use fmax");
    assert!(ocl.contains("-INFINITY"), "reduce max must init with -INFINITY");
}

#[test]
fn test_opencl_reduce_with_prefix_ops() {
    // Test fused: elementwise prefix -> reduce
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Op(0)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [256, 1, 1],
                spec: None,
    };
    let ocl = OpenClRenderer::new(false).render(&kernel);
    assert!(ocl.contains("barrier(CLK_LOCAL_MEM_FENCE)"));
    assert!(ocl.contains("buf1[eidx] * buf2[eidx]"), "prefix mul should be inside reduce loop");
}

// --- All 26 ops test ---

#[test]
fn test_opencl_all_26_ops_render() {
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
        let ocl = OpenClRenderer::new(false).render(&kernel);
        assert!(ocl.contains("__kernel void molt_kernel("), "op {:?} failed to render OpenCL", op);
        assert!(ocl.contains("get_global_id(0)"), "op {:?} missing thread index", op);
        assert!(ocl.contains("__global"), "op {:?} missing buffer qualifiers", op);
    }

    // Test reduce ops
    for reduce_op in [PrimitiveOp::ReduceSum, PrimitiveOp::ReduceMax] {
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: reduce_op,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [64, 1, 1],
                spec: None,
        };
        let ocl = OpenClRenderer::new(false).render(&kernel);
        assert!(ocl.contains("__kernel void molt_kernel("), "reduce op {:?} failed to render OpenCL", reduce_op);
        assert!(ocl.contains("barrier(CLK_LOCAL_MEM_FENCE)"), "reduce op {:?} missing barrier", reduce_op);
    }
}

// --- DType narrowing unit tests ---

#[test]
fn test_narrow_opencl_bf16_always_narrowed() {
    assert_eq!(DType::BFloat16.narrow_opencl(true), DType::Float32);
    assert_eq!(DType::BFloat16.narrow_opencl(false), DType::Float32);
}

#[test]
fn test_narrow_opencl_f64_conditional() {
    assert_eq!(DType::Float64.narrow_opencl(true), DType::Float64);
    assert_eq!(DType::Float64.narrow_opencl(false), DType::Float32);
}

#[test]
fn test_narrow_opencl_passthrough() {
    // All other types pass through unchanged
    for dtype in [DType::Bool, DType::Int8, DType::Int16, DType::Int32, DType::Int64,
                  DType::UInt8, DType::UInt16, DType::UInt32, DType::UInt64,
                  DType::Float16, DType::Float32] {
        assert_eq!(dtype.narrow_opencl(false), dtype, "{:?} should not be narrowed", dtype);
        assert_eq!(dtype.narrow_opencl(true), dtype, "{:?} should not be narrowed", dtype);
    }
}

#[test]
fn test_opencl_type_names() {
    assert_eq!(DType::Bool.opencl_type(), "int");
    assert_eq!(DType::Int8.opencl_type(), "char");
    assert_eq!(DType::Int16.opencl_type(), "short");
    assert_eq!(DType::Int32.opencl_type(), "int");
    assert_eq!(DType::Int64.opencl_type(), "long");
    assert_eq!(DType::UInt8.opencl_type(), "uchar");
    assert_eq!(DType::UInt16.opencl_type(), "ushort");
    assert_eq!(DType::UInt32.opencl_type(), "uint");
    assert_eq!(DType::UInt64.opencl_type(), "ulong");
    assert_eq!(DType::Float16.opencl_type(), "half");
    assert_eq!(DType::Float32.opencl_type(), "float");
    assert_eq!(DType::Float64.opencl_type(), "double");
}

// --- No fp64 pragma when not needed ---

#[test]
fn test_opencl_no_fp64_pragma_for_f32_kernel() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let ocl = OpenClRenderer::new(true).render(&kernel);
    assert!(!ocl.contains("#pragma OPENCL EXTENSION cl_khr_fp64"), "should not emit fp64 pragma for f32 kernel");
}
