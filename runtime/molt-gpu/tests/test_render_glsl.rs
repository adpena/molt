use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer,
};
use molt_gpu::render::glsl::GlslRenderer;
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
    }
}

fn make_simple_unary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
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
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n as u32, 1, 1],
        local: [256, 1, 1],
    }
}

// ============================================================
// Header tests
// ============================================================

#[test]
fn test_glsl_version_300_es_header() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.starts_with("#version 300 es\n"),
        "GLSL ES 3.0 header must be present: {}",
        &glsl[..80]
    );
}

#[test]
fn test_glsl_precision_highp_float() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("precision highp float;"),
        "Must declare precision highp float for ML accuracy"
    );
}

#[test]
fn test_glsl_precision_highp_int() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("precision highp int;"),
        "Must declare precision highp int"
    );
}

// ============================================================
// Sampler2D input binding tests
// ============================================================

#[test]
fn test_glsl_sampler2d_inputs() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 128);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("uniform sampler2D u_tex1;"), "Input buf 1 should be sampler2D");
    assert!(glsl.contains("uniform sampler2D u_tex2;"), "Input buf 2 should be sampler2D");
    // Output buf (id=0) should NOT be a sampler2D
    assert!(!glsl.contains("uniform sampler2D u_tex0;"), "Output buf should not be sampler2D");
}

#[test]
fn test_glsl_texture_width_uniform() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("uniform int u_tex_width;"), "Must have texture width uniform");
}

#[test]
fn test_glsl_num_elements_uniform() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("uniform int u_num_elements;"), "Must have num_elements uniform");
}

// ============================================================
// Fragment shader output
// ============================================================

#[test]
fn test_glsl_frag_color_output() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("out vec4 frag_color;"), "Must declare frag_color output");
    assert!(glsl.contains("frag_color = result;"), "Must write to frag_color");
}

#[test]
fn test_glsl_gl_fragcoord_index() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("gl_FragCoord"),
        "Must use gl_FragCoord for texel position"
    );
}

// ============================================================
// No disallowed types in output
// ============================================================

#[test]
fn test_glsl_no_f64_types() {
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
    };
    let glsl = GlslRenderer.render(&kernel);
    // f64 and double should not appear in GLSL output
    assert!(!glsl.contains("double"), "GLSL should not contain 'double'");
    assert!(!glsl.contains("f64"), "GLSL should not contain 'f64'");
}

#[test]
fn test_glsl_no_i64_u64_types() {
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
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(!glsl.contains("i64"), "GLSL should not contain 'i64'");
    assert!(!glsl.contains("u64"), "GLSL should not contain 'u64'");
    assert!(!glsl.contains("long"), "GLSL should not contain 'long'");
}

// ============================================================
// Arithmetic op tests
// ============================================================

#[test]
fn test_glsl_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("void main()"));
    assert!(glsl.contains("texelFetch(u_tex1"));
    assert!(glsl.contains("texelFetch(u_tex2"));
    // The add expression
    assert!(glsl.contains("+"));
}

#[test]
fn test_glsl_render_sub() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Sub, 512);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" - "));
}

#[test]
fn test_glsl_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 256);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" * "));
}

#[test]
fn test_glsl_render_idiv() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Idiv, 256);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" / "));
}

#[test]
fn test_glsl_render_mod_c_semantics() {
    // Mod must use C semantics: a - b * (a / b)
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mod, 256);
    let glsl = GlslRenderer.render(&kernel);
    // Check for the C-semantics formula pattern (not %)
    assert!(
        glsl.contains(" - ") && glsl.contains(" * ") && glsl.contains(" / "),
        "Mod should use a - b * (a / b) pattern for C semantics"
    );
}

#[test]
fn test_glsl_render_neg() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Neg, 256);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("(-"));
}

// ============================================================
// Comparison op tests
// ============================================================

#[test]
fn test_glsl_render_cmplt() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" < "), "Cmplt should use < operator");
    assert!(glsl.contains("1.0") && glsl.contains("0.0"), "Comparison should output 1.0/0.0");
}

#[test]
fn test_glsl_render_cmpeq() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmpeq,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" == "), "Cmpeq should use == operator");
}

#[test]
fn test_glsl_render_cmpne() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmpne,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" != "), "Cmpne should use != operator");
}

// ============================================================
// Bitwise op tests
// ============================================================

#[test]
fn test_glsl_render_bitwise_and() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::And, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" & "), "And should use & operator");
    assert!(glsl.contains("int("), "Bitwise ops need int cast in GLSL");
}

#[test]
fn test_glsl_render_bitwise_or() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Or, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" | "), "Or should use | operator");
}

#[test]
fn test_glsl_render_bitwise_xor() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Xor, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" ^ "), "Xor should use ^ operator");
}

#[test]
fn test_glsl_render_shl() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Shl, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" << "), "Shl should use << operator");
}

#[test]
fn test_glsl_render_shr() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Shr, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains(" >> "), "Shr should use >> operator");
}

// ============================================================
// Math op tests
// ============================================================

#[test]
fn test_glsl_render_exp2() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Exp2, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("exp2("), "Exp2 should use GLSL exp2()");
}

#[test]
fn test_glsl_render_log2() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Log2, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("log2("), "Log2 should use GLSL log2()");
}

#[test]
fn test_glsl_render_sin() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Sin, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("sin("), "Sin should use GLSL sin()");
}

#[test]
fn test_glsl_render_sqrt() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Sqrt, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("sqrt("), "Sqrt should use GLSL sqrt()");
}

#[test]
fn test_glsl_render_reciprocal() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Reciprocal, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("1.0 / "), "Reciprocal should use 1.0/x");
}

// ============================================================
// Other op tests
// ============================================================

#[test]
fn test_glsl_render_trunc() {
    let kernel = make_simple_unary_kernel(PrimitiveOp::Trunc, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("trunc("), "Trunc should use GLSL trunc()");
}

#[test]
fn test_glsl_render_max() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Max, 64);
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("max("), "Max should use GLSL max()");
}

#[test]
fn test_glsl_render_where_ternary() {
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
    };
    let glsl = GlslRenderer.render(&kernel);
    // GLSL supports ternary, so should use ? : (unlike WGSL's select())
    assert!(glsl.contains(" ? "), "GLSL Where should use ternary operator");
    assert!(!glsl.contains("select("), "GLSL should not use select()");
}

#[test]
fn test_glsl_render_cast() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cast,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("int("), "Cast to int should use int()");
}

#[test]
fn test_glsl_render_bitcast_to_float() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Bitcast,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("intBitsToFloat("),
        "Bitcast to float must use intBitsToFloat()"
    );
}

#[test]
fn test_glsl_render_bitcast_to_int() {
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
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("floatBitsToInt("),
        "Bitcast to int must use floatBitsToInt()"
    );
}

#[test]
fn test_glsl_render_bitcast_to_uint() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Bitcast,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::UInt32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::UInt32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("floatBitsToUint("),
        "Bitcast to uint must use floatBitsToUint()"
    );
}

// ============================================================
// Reduce op tests
// ============================================================

#[test]
fn test_glsl_render_reduce_sum() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[1024]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("#version 300 es"), "Must have GLSL ES 3.0 header");
    assert!(glsl.contains("acc"), "Reduce must use accumulator");
    assert!(glsl.contains("for (int rid"), "Reduce must have reduction loop");
    assert!(glsl.contains("acc + "), "ReduceSum must accumulate with +");
}

#[test]
fn test_glsl_render_reduce_max() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[1024]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("for (int rid"), "Reduce must have reduction loop");
    assert!(glsl.contains("max(acc,"), "ReduceMax must use max()");
    // -infinity init via intBitsToFloat
    assert!(
        glsl.contains("intBitsToFloat(int(0xff800000u))"),
        "ReduceMax must init to -infinity"
    );
}

#[test]
fn test_glsl_render_fused_elementwise_then_reduce() {
    // Fused chain: Mul -> ReduceSum
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
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[4]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[1024]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[1024]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [4, 1, 1],
        local: [4, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(glsl.contains("for (int rid"), "Must have reduce loop");
    // Mul should appear inside the loop
    assert!(glsl.contains(" * "), "Mul should appear in fused pre-reduce");
    assert!(glsl.contains("acc = acc + "), "ReduceSum accumulation");
}

// ============================================================
// Fused elementwise chain tests
// ============================================================

#[test]
fn test_glsl_render_fused_elementwise_chain() {
    // Fused chain: Add -> Mul -> Neg
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Const { val: 2.0, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Neg,
                srcs: vec![FusedSrc::Op(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [256, 1, 1],
        local: [256, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    // Check chain references: v1 uses v0, v2 uses v1
    assert!(glsl.contains("v0"), "First op result as v0");
    assert!(glsl.contains("v1"), "Second op result as v1");
    assert!(glsl.contains("v2"), "Third op result as v2");
    assert!(glsl.contains("result[comp] = float(v2)"), "Final output should be v2");
}

// ============================================================
// All 26 ops coverage test
// ============================================================

#[test]
fn test_glsl_all_26_ops_have_render_patterns() {
    let elementwise_ops: Vec<_> = PrimitiveOp::ALL.iter()
        .filter(|op| op.is_elementwise())
        .collect();

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
        };
        let glsl = GlslRenderer.render(&kernel);
        assert!(
            glsl.contains("#version 300 es"),
            "op {:?} failed to render valid GLSL header",
            op
        );
        assert!(
            glsl.contains("void main()"),
            "op {:?} failed to render GLSL main()",
            op
        );
        // Verify no disallowed types leak through
        assert!(
            !glsl.contains("double"),
            "op {:?} has disallowed 'double' type in GLSL output",
            op
        );
    }

    // Also check reduce ops render via dedicated kernels
    for reduce_op in [PrimitiveOp::ReduceSum, PrimitiveOp::ReduceMax] {
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: reduce_op,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
        };
        let glsl = GlslRenderer.render(&kernel);
        assert!(
            glsl.contains("#version 300 es") && glsl.contains("for (int rid"),
            "reduce op {:?} failed to render valid GLSL with reduction loop",
            reduce_op
        );
    }
}

// ============================================================
// DType narrowing for WebGL2
// ============================================================

#[test]
fn test_dtype_narrow_webgl2() {
    assert_eq!(DType::Float64.narrow_webgl2(), DType::Float32);
    assert_eq!(DType::Int64.narrow_webgl2(), DType::Int32);
    assert_eq!(DType::UInt64.narrow_webgl2(), DType::UInt32);
    assert_eq!(DType::BFloat16.narrow_webgl2(), DType::Float32);
    assert_eq!(DType::Float16.narrow_webgl2(), DType::Float32);
    assert_eq!(DType::Int8.narrow_webgl2(), DType::Int32);
    assert_eq!(DType::Int16.narrow_webgl2(), DType::Int32);
    assert_eq!(DType::UInt8.narrow_webgl2(), DType::UInt32);
    assert_eq!(DType::UInt16.narrow_webgl2(), DType::UInt32);
    // Types that stay the same
    assert_eq!(DType::Float32.narrow_webgl2(), DType::Float32);
    assert_eq!(DType::Int32.narrow_webgl2(), DType::Int32);
    assert_eq!(DType::UInt32.narrow_webgl2(), DType::UInt32);
    assert_eq!(DType::Bool.narrow_webgl2(), DType::Bool);
}

#[test]
fn test_dtype_glsl_type() {
    assert_eq!(DType::Float32.glsl_type(), "float");
    assert_eq!(DType::Float64.glsl_type(), "float");
    assert_eq!(DType::Int32.glsl_type(), "int");
    assert_eq!(DType::Int64.glsl_type(), "int");
    assert_eq!(DType::UInt32.glsl_type(), "uint");
    assert_eq!(DType::UInt64.glsl_type(), "uint");
    assert_eq!(DType::Bool.glsl_type(), "bool");
    assert_eq!(DType::Float16.glsl_type(), "float");
    assert_eq!(DType::BFloat16.glsl_type(), "float");
    assert_eq!(DType::Int8.glsl_type(), "int");
    assert_eq!(DType::Int16.glsl_type(), "int");
    assert_eq!(DType::UInt8.glsl_type(), "uint");
    assert_eq!(DType::UInt16.glsl_type(), "uint");
}

// ============================================================
// Constants test
// ============================================================

#[test]
fn test_glsl_const_infinity() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: f64::INFINITY, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("intBitsToFloat(0x7f800000)"),
        "Infinity should use intBitsToFloat pattern in GLSL"
    );
}

#[test]
fn test_glsl_const_neg_infinity() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: f64::NEG_INFINITY, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("intBitsToFloat(int(0xff800000u))"),
        "Neg infinity should use intBitsToFloat pattern in GLSL"
    );
}

#[test]
fn test_glsl_const_nan() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const { val: f64::NAN, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let glsl = GlslRenderer.render(&kernel);
    assert!(
        glsl.contains("intBitsToFloat(0x7fc00000)"),
        "NaN should use intBitsToFloat pattern in GLSL"
    );
}
