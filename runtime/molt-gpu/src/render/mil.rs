//! MilRenderer — Apple MIL (Machine Learning Intermediate Language) codegen.
//!
//! Maps all 26 tinygrad primitive ops to MIL IR operations for execution on
//! the Apple Neural Engine (ANE) via Core ML.
//!
//! MIL is Apple's internal graph IR used by the Core ML compiler. A MIL
//! program consists of typed operations on tensor values, compiled to
//! ANE microcode or Metal compute shaders depending on op support.
//!
//! # MIL Op Mapping
//!
//! The 26 tinygrad primitives map to MIL as follows:
//!
//! | Tinygrad Op   | MIL Op(s)                              |
//! |---------------|----------------------------------------|
//! | Add           | `add(x, y)`                            |
//! | Sub           | `sub(x, y)`                            |
//! | Mul           | `mul(x, y)`                            |
//! | Idiv          | `floor_div(x, y)`                      |
//! | Mod           | `mod(x, y)`                            |
//! | Neg           | `mul(x, const(-1))`                    |
//! | Cmplt         | `less(x, y)`                           |
//! | Cmpeq         | `equal(x, y)`                          |
//! | Cmpne         | `not_equal(x, y)`                      |
//! | And           | `logical_and(x, y)`                    |
//! | Or            | `logical_or(x, y)`                     |
//! | Xor           | `logical_xor(x, y)`                    |
//! | Shl           | `mul(x, pow(const(2), y))`             |
//! | Shr           | `floor_div(x, pow(const(2), y))`       |
//! | Exp2          | `pow(const(2), x)`                     |
//! | Log2          | `log(x)` / `log(const(2))`             |
//! | Sin           | `sin(x)`                               |
//! | Sqrt          | `sqrt(x)`                              |
//! | Reciprocal    | `real_div(const(1), x)`                |
//! | Trunc         | `cast(cast(x, int32), fp16)`           |
//! | Max           | `maximum(x, y)`                        |
//! | Where         | `select(cond, a, b)`                   |
//! | Cast          | `cast(x, dtype)`                       |
//! | Bitcast       | `cast(x, dtype)` (reinterpret)         |
//! | ReduceSum     | `reduce_sum(x, axes)`                  |
//! | ReduceMax     | `reduce_max(x, axes)`                  |
//!
//! # Output Format
//!
//! The renderer produces MIL text format (`.mil`), which is the human-readable
//! serialization of MIL programs. In production, this would be serialized to
//! the binary protobuf format consumed by the Core ML compiler.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, Renderer};

/// Apple MIL IR renderer for all 26 primitive ops.
pub struct MilRenderer;

impl MilRenderer {
    /// Map a DType to MIL type string.
    fn mil_type(dtype: DType) -> &'static str {
        match dtype {
            DType::Bool => "bool",
            DType::Int8 => "int8",
            DType::Int16 => "int16",
            DType::Int32 => "int32",
            DType::Int64 => "int64",
            DType::UInt8 => "uint8",
            DType::UInt16 => "uint16",
            DType::UInt32 => "uint32",
            DType::UInt64 => "uint64",
            DType::Float16 | DType::BFloat16 => "fp16",
            DType::Float32 => "fp32",
            DType::Float64 => "fp64",
            DType::MxFP8 => "fp8",
            DType::MxFP4 => "fp4",
        }
    }

    /// Format a constant value as a MIL literal.
    fn format_const(val: f64, dtype: DType) -> String {
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("{}", val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("{}", val as u64)
            }
            _ => {
                if val == f64::INFINITY {
                    "inf".to_string()
                } else if val == f64::NEG_INFINITY {
                    "-inf".to_string()
                } else if val.is_nan() {
                    "nan".to_string()
                } else {
                    format!("{}", val)
                }
            }
        }
    }

    /// Render a source reference as a MIL variable name.
    fn render_src(src: &FusedSrc, kernel: &FusedKernel) -> String {
        match src {
            FusedSrc::Buf(buf_idx) => format!("input_{}", kernel.bufs[*buf_idx].buf_id),
            FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
            FusedSrc::Const { val, dtype } => {
                format!(
                    "const(val={}, dtype={})",
                    Self::format_const(*val, *dtype),
                    Self::mil_type(*dtype),
                )
            }
        }
    }

    /// Render a single op as a MIL operation assignment.
    fn render_op(op: &FusedOp, op_idx: usize, kernel: &FusedKernel) -> String {
        let src = |i: usize| -> String { Self::render_src(&op.srcs[i], kernel) };
        let dst_type = Self::mil_type(op.dst_dtype);
        let var = format!("v{}", op_idx);

        match op.op {
            // Arithmetic
            PrimitiveOp::Add => {
                format!("{} = add(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Sub => {
                format!("{} = sub(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Mul => {
                format!("{} = mul(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Idiv => {
                format!("{} = floor_div(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Mod => {
                format!("{} = mod(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Neg => {
                // MIL has no unary neg; express as mul(x, -1).
                format!(
                    "{} = mul(x={}, y=const(val=-1, dtype={}))",
                    var,
                    src(0),
                    dst_type,
                )
            }

            // Comparison
            PrimitiveOp::Cmplt => {
                format!("{} = less(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Cmpeq => {
                format!("{} = equal(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Cmpne => {
                format!("{} = not_equal(x={}, y={})", var, src(0), src(1))
            }

            // Bitwise — MIL uses logical ops on boolean tensors.
            // For integer bitwise, these map to the MIL bitwise_ variants.
            PrimitiveOp::And => {
                format!("{} = logical_and(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Or => {
                format!("{} = logical_or(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Xor => {
                format!("{} = logical_xor(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Shl => {
                // MIL has no shift ops. Decompose: x << y = x * pow(2, y).
                format!(
                    "{tmp} = pow(x=const(val=2, dtype={dt}), y={y})\n    \
                     {var} = mul(x={x}, y={tmp})",
                    tmp = format!("v{}_shl_pow", op_idx),
                    dt = dst_type,
                    y = src(1),
                    var = var,
                    x = src(0),
                )
            }
            PrimitiveOp::Shr => {
                // x >> y = floor_div(x, pow(2, y)).
                format!(
                    "{tmp} = pow(x=const(val=2, dtype={dt}), y={y})\n    \
                     {var} = floor_div(x={x}, y={tmp})",
                    tmp = format!("v{}_shr_pow", op_idx),
                    dt = dst_type,
                    y = src(1),
                    var = var,
                    x = src(0),
                )
            }

            // Math
            PrimitiveOp::Exp2 => {
                // exp2(x) = pow(2, x)
                format!(
                    "{} = pow(x=const(val=2, dtype={}), y={})",
                    var, dst_type, src(0),
                )
            }
            PrimitiveOp::Log2 => {
                // log2(x) = log(x) / log(2)
                // MIL has no native log2; decompose as real_div(log(x), log(2)).
                format!(
                    "{tmp} = log(x={x})\n    \
                     {var} = real_div(x={tmp}, y=const(val=0.6931471805599453, dtype={dt}))",
                    tmp = format!("v{}_ln", op_idx),
                    x = src(0),
                    var = var,
                    dt = dst_type,
                )
            }
            PrimitiveOp::Sin => {
                format!("{} = sin(x={})", var, src(0))
            }
            PrimitiveOp::Sqrt => {
                format!("{} = sqrt(x={})", var, src(0))
            }
            PrimitiveOp::Reciprocal => {
                format!(
                    "{} = real_div(x=const(val=1, dtype={}), y={})",
                    var, dst_type, src(0),
                )
            }

            // Other
            PrimitiveOp::Trunc => {
                // MIL has no trunc; cast to int32 then back to float.
                format!(
                    "{tmp} = cast(x={x}, dtype=\"int32\")\n    \
                     {var} = cast(x={tmp}, dtype=\"{dt}\")",
                    tmp = format!("v{}_trunc_int", op_idx),
                    x = src(0),
                    var = var,
                    dt = dst_type,
                )
            }
            PrimitiveOp::Max => {
                format!("{} = maximum(x={}, y={})", var, src(0), src(1))
            }
            PrimitiveOp::Where => {
                format!(
                    "{} = select(cond={}, a={}, b={})",
                    var, src(0), src(1), src(2),
                )
            }
            PrimitiveOp::Cast => {
                format!("{} = cast(x={}, dtype=\"{}\")", var, src(0), dst_type)
            }
            PrimitiveOp::Bitcast => {
                // MIL does not have a true bitcast. This is a best-effort cast.
                // ANE-targeted models should avoid bitcast where possible.
                format!("{} = cast(x={}, dtype=\"{}\")", var, src(0), dst_type)
            }

            // Reduce
            PrimitiveOp::ReduceSum => {
                format!(
                    "{} = reduce_sum(x={}, axes=[0], keep_dims=false)",
                    var, src(0),
                )
            }
            PrimitiveOp::ReduceMax => {
                format!(
                    "{} = reduce_max(x={}, axes=[0], keep_dims=false)",
                    var, src(0),
                )
            }
        }
    }
}

impl Renderer for MilRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        // MIL program header
        writeln!(out, "mil_program {{").unwrap();
        writeln!(out, "  func main(").unwrap();

        // Input parameters
        for binding in &kernel.bufs {
            let dtype_str = Self::mil_type(binding.dtype);
            let numel = binding.st.numel();
            match binding.access {
                BufferAccess::Read => {
                    writeln!(
                        out,
                        "    input_{}: tensor<[{}], {}>,",
                        binding.buf_id, numel, dtype_str,
                    )
                    .unwrap();
                }
                BufferAccess::Write | BufferAccess::ReadWrite => {
                    // Output declared in return type, not as parameter
                }
            }
        }
        writeln!(out, "  ) {{").unwrap();

        // Emit ops
        for (i, op) in kernel.ops.iter().enumerate() {
            let rendered = Self::render_op(op, i, kernel);
            writeln!(out, "    {}", rendered).unwrap();
        }

        // Return the last op result, written to the output buffer
        let last_op = kernel.ops.len().saturating_sub(1);
        let out_dtype = Self::mil_type(kernel.bufs[0].dtype);
        let out_numel = kernel.bufs[0].st.numel();
        writeln!(
            out,
            "    return v{}: tensor<[{}], {}>",
            last_op, out_numel, out_dtype,
        )
        .unwrap();
        writeln!(out, "  }}").unwrap();
        writeln!(out, "}}").unwrap();

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{BufferBinding, FusedKernel, FusedOp, FusedSrc, BufferAccess};
    use crate::shapetracker::ShapeTracker;

    fn make_elementwise_kernel(op: PrimitiveOp, dst_dtype: DType) -> FusedKernel {
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
            ops: vec![FusedOp { op, srcs, dst_dtype }],
            bufs,
            grid: [1024, 1, 1],
            local: [1, 1, 1],
            spec: None,
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
            ops: vec![FusedOp {
                op: PrimitiveOp::ReduceSum,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
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
        };
        let renderer = MilRenderer;
        let source = renderer.render(&kernel);
        assert!(source.contains("reduce_sum(x=input_1, axes=[0], keep_dims=false)"));
    }

    #[test]
    fn test_mil_render_all_26_ops() {
        // Verify that every primitive op can be rendered without panic.
        let renderer = MilRenderer;
        for op in PrimitiveOp::ALL {
            let dst_dtype = if matches!(op, PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne) {
                DType::Bool
            } else {
                DType::Float32
            };
            let kernel = make_elementwise_kernel(op, dst_dtype);
            let source = renderer.render(&kernel);
            assert!(!source.is_empty(), "Empty render for op {:?}", op);
            assert!(source.contains("mil_program"), "Missing header for op {:?}", op);
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
        assert_eq!(MilRenderer::format_const(f64::INFINITY, DType::Float32), "inf");
        assert_eq!(MilRenderer::format_const(f64::NEG_INFINITY, DType::Float32), "-inf");
    }
}
