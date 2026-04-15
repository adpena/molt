//! MslRenderer — Metal Shading Language codegen for all 26 primitive ops.
//!
//! Generates MSL compute kernel source from FusedKernel IR.
//! All dtypes are narrowed via DType::narrow_metal() before rendering.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, Renderer};

/// Metal Shading Language renderer for all 26 primitive ops.
pub struct MslRenderer;

impl MslRenderer {
    /// Format a constant value as MSL literal.
    fn format_const(val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_metal();
        match dtype {
            DType::Bool => {
                if val != 0.0 { "true".to_string() } else { "false".to_string() }
            }
            DType::Float16 => format!("half({})", val),
            DType::BFloat16 => format!("bfloat({})", val),
            DType::Float32 => {
                if val == f64::INFINITY { "INFINITY".to_string() }
                else if val == f64::NEG_INFINITY { "(-INFINITY)".to_string() }
                else if val.is_nan() { "NAN".to_string() }
                else {
                    let s = format!("{}", val);
                    // Ensure the literal always has a decimal point for valid MSL
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        format!("{}f", s)
                    } else {
                        format!("{}.0f", s)
                    }
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("{}({})", dtype.msl_type(), val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("{}({})", dtype.msl_type(), val as u64)
            }
            _ => format!("{}", val),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(binding: &crate::render::BufferBinding, idx_var: &str) -> String {
        let st = &binding.st;
        let view = st.view();

        // Build index expression from ShapeTracker
        let ndim = view.shape.len();
        if ndim == 0 {
            return format!("buf{}[0]", binding.buf_id);
        }

        // For contiguous single-dim or simple strides, generate direct expression
        if view.is_contiguous() && view.mask.is_none() {
            return format!("buf{}[{}]", binding.buf_id, idx_var);
        }

        // General case: decompose linear index, apply strides
        let mut parts = Vec::new();
        for dim in 0..ndim {
            let stride = view.strides[dim];
            if stride == 0 {
                continue;
            }
            let size = view.shape[dim];
            let idx_expr = if dim == ndim - 1 {
                format!("({} % {})", idx_var, size)
            } else {
                let divisor: usize = view.shape[dim + 1..].iter().product();
                format!("({} / {} % {})", idx_var, divisor, size)
            };
            if stride == 1 {
                parts.push(idx_expr);
            } else if stride == -1 {
                parts.push(format!("({} - {})", size - 1, idx_expr));
            } else if stride > 0 {
                parts.push(format!("{} * {}", idx_expr, stride));
            } else {
                parts.push(format!("({} - {}) * {}", size - 1, idx_expr, -stride));
            }
        }

        let offset = if view.offset != 0 {
            format!("{} + ", view.offset)
        } else {
            String::new()
        };

        let idx_sum = if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" + ")
        };

        format!("buf{}[{}{}]", binding.buf_id, offset, idx_sum)
    }

    /// Render a single op expression.
    fn render_op(op: &FusedOp, _op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String {
            match &op.srcs[i] {
                FusedSrc::Buf(buf_idx) => {
                    Self::render_buf_read(&kernel.bufs[*buf_idx], idx_var)
                }
                FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
                FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
            }
        };

        let dst_type = op.dst_dtype.narrow_metal().msl_type();

        match op.op {
            // Arithmetic
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            PrimitiveOp::Mod => format!("({} % {})", src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            // Comparison — output is always bool
            PrimitiveOp::Cmplt => format!("({} < {})", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("({} == {})", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("({} != {})", src(0), src(1)),

            // Bitwise
            PrimitiveOp::And => format!("({} & {})", src(0), src(1)),
            PrimitiveOp::Or => format!("({} | {})", src(0), src(1)),
            PrimitiveOp::Xor => format!("({} ^ {})", src(0), src(1)),
            PrimitiveOp::Shl => format!("({} << {})", src(0), src(1)),
            PrimitiveOp::Shr => format!("({} >> {})", src(0), src(1)),

            // Math
            PrimitiveOp::Exp2 => format!("exp2({})", src(0)),
            PrimitiveOp::Log2 => format!("log2({})", src(0)),
            PrimitiveOp::Sin => format!("sin({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrt({})", src(0)),
            PrimitiveOp::Reciprocal => format!("(1.0f / {})", src(0)),

            // Other
            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            PrimitiveOp::Max => format!("max({}, {})", src(0), src(1)),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("{}({})", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("as_type<{}>({})", dst_type, src(0)),

            // Reduce — these generate loop structures, handled specially
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Detect FMA pattern: ADD(MUL(a, b), c) or ADD(c, MUL(a, b)).
    /// Returns Some((a_expr, b_expr, c_expr)) if the pattern matches.
    fn detect_fma(op: &FusedOp, op_idx: usize, kernel: &FusedKernel) -> Option<(String, String, String)> {
        if op.op != PrimitiveOp::Add {
            return None;
        }
        // Only emit FMA for float types
        if !op.dst_dtype.narrow_metal().is_float() {
            return None;
        }

        // Check if either source is a MUL op result
        for (mul_src_pos, add_src_pos) in [(0, 1), (1, 0)] {
            if let FusedSrc::Op(prior_idx) = &op.srcs[mul_src_pos] {
                if *prior_idx < op_idx {
                    let prior_op = &kernel.ops[*prior_idx];
                    if prior_op.op == PrimitiveOp::Mul {
                        let a = match &prior_op.srcs[0] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let b = match &prior_op.srcs[1] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let c = match &op.srcs[add_src_pos] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        return Some((a, b, c));
                    }
                }
            }
        }
        None
    }
}

impl Renderer for MslRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        // Include headers
        writeln!(out, "#include <metal_stdlib>").unwrap();
        writeln!(out, "using namespace metal;").unwrap();
        writeln!(out).unwrap();

        // Kernel function signature
        write!(out, "kernel void molt_kernel(").unwrap();

        // Buffer parameters
        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.narrow_metal().msl_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "const device",
                BufferAccess::Write | BufferAccess::ReadWrite => "device",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{} {}* buf{} [[buffer({})]]", qualifier, dtype_str, binding.buf_id, i).unwrap();
        }

        // Thread index
        write!(out, ", uint gid [[thread_position_in_grid]]").unwrap();
        writeln!(out, ") {{").unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}) return;", output_numel).unwrap();

        // Check if we have reduce ops
        let has_reduce = kernel.ops.iter().any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if !has_reduce {
            // Pure elementwise kernel — straightforward
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                // Try FMA emission: fma(a, b, c) is faster and more accurate than a*b+c
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel) {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "gid")
                };
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            // Write output
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        } else {
            // Fused kernel with reduce: elementwise prefix -> reduce -> elementwise suffix
            let reduce_idx = kernel.ops.iter().position(|op| {
                matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
            }).expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs[0];
            let reduce_dtype = reduce_op.dst_dtype.narrow_metal();

            // Find the input buffer for the reduce source
            let input_buf = match reduce_src {
                FusedSrc::Buf(idx) => &kernel.bufs[*idx],
                FusedSrc::Op(_) => &kernel.bufs[1],
                FusedSrc::Const { .. } => unreachable!("reduce on constant"),
            };
            let reduce_size = input_buf.st.numel() / output_numel;

            let init_val = match reduce_op.op {
                PrimitiveOp::ReduceSum => "0",
                PrimitiveOp::ReduceMax => "-INFINITY",
                _ => unreachable!(),
            };

            // Initialize accumulator
            writeln!(out, "    {} acc = {};", reduce_dtype.msl_type(), init_val).unwrap();

            // Pre-reduce elementwise ops
            if reduce_idx > 0 {
                // Hint: unroll small reduction loops for better ILP.
                if reduce_size <= 16 {
                    writeln!(out, "    #pragma unroll").unwrap();
                }
                writeln!(out, "    for (uint rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        uint eidx = gid * {} + rid;", reduce_size).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
                }

                // Accumulate
                let src_var = format!("v{}", reduce_idx - 1);
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_var).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_var).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                // Reduce directly from buffer
                if reduce_size <= 16 {
                    writeln!(out, "    #pragma unroll").unwrap();
                }
                writeln!(out, "    for (uint rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        uint eidx = gid * {} + rid;", reduce_size).unwrap();
                let src_expr = match reduce_src {
                    FusedSrc::Buf(idx) => Self::render_buf_read(&kernel.bufs[*idx], "eidx"),
                    _ => unreachable!(),
                };
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_expr).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            // Store reduce result
            writeln!(out, "    {} v{} = acc;", reduce_dtype.msl_type(), reduce_idx).unwrap();

            // Post-reduce elementwise ops
            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype.narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            // Write output
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
