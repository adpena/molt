//! CudaRenderer — CUDA C codegen for all 26 primitive ops.
//!
//! Generates CUDA compute kernel source from FusedKernel IR.
//! Full i64/f64 support (no narrowing needed).
//! Thread index: `blockIdx.x * blockDim.x + threadIdx.x`

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, Renderer};

/// CUDA C renderer for all 26 primitive ops.
pub struct CudaRenderer;

impl CudaRenderer {
    /// Format a constant value as CUDA C literal.
    fn format_const(val: f64, dtype: DType) -> String {
        match dtype {
            DType::Bool => {
                if val != 0.0 { "true".to_string() } else { "false".to_string() }
            }
            DType::Float16 => {
                let s = format!("{}", val);
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    format!("__float2half({}f)", s)
                } else {
                    format!("__float2half({}.0f)", s)
                }
            }
            DType::BFloat16 => {
                let s = format!("{}", val);
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    format!("__float2bfloat16({}f)", s)
                } else {
                    format!("__float2bfloat16({}.0f)", s)
                }
            }
            DType::Float32 => {
                if val == f64::INFINITY { "INFINITY".to_string() }
                else if val == f64::NEG_INFINITY { "(-INFINITY)".to_string() }
                else if val.is_nan() { "NAN".to_string() }
                else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        format!("{}f", s)
                    } else {
                        format!("{}.0f", s)
                    }
                }
            }
            DType::Float64 => {
                if val == f64::INFINITY { "INFINITY".to_string() }
                else if val == f64::NEG_INFINITY { "(-INFINITY)".to_string() }
                else if val.is_nan() { "NAN".to_string() }
                else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        s
                    } else {
                        format!("{}.0", s)
                    }
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 => {
                format!("(({}){})", dtype.cuda_type(), val as i64)
            }
            DType::Int64 => format!("{}LL", val as i64),
            DType::UInt8 | DType::UInt16 | DType::UInt32 => {
                format!("(({}){}u)", dtype.cuda_type(), val as u64)
            }
            DType::UInt64 => format!("{}ULL", val as u64),
            // MXFP types: constants are stored as unsigned char (raw byte).
            DType::MxFP8 | DType::MxFP4 => format!("((unsigned char){})", val as u8),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(binding: &crate::render::BufferBinding, idx_var: &str) -> String {
        let st = &binding.st;
        let view = st.view();

        let ndim = view.shape.len();
        if ndim == 0 {
            return format!("buf{}[0]", binding.buf_id);
        }

        if view.is_contiguous() && view.mask.is_none() {
            return format!("buf{}[{}]", binding.buf_id, idx_var);
        }

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

        let dst_type = op.dst_dtype.cuda_type();

        match op.op {
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            PrimitiveOp::Mod => format!("({} % {})", src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            PrimitiveOp::Cmplt => format!("({} < {})", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("({} == {})", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("({} != {})", src(0), src(1)),

            PrimitiveOp::And => format!("({} & {})", src(0), src(1)),
            PrimitiveOp::Or => format!("({} | {})", src(0), src(1)),
            PrimitiveOp::Xor => format!("({} ^ {})", src(0), src(1)),
            PrimitiveOp::Shl => format!("({} << {})", src(0), src(1)),
            PrimitiveOp::Shr => format!("({} >> {})", src(0), src(1)),

            PrimitiveOp::Exp2 => format!("exp2f({})", src(0)),
            PrimitiveOp::Log2 => format!("log2f({})", src(0)),
            PrimitiveOp::Sin => format!("sinf({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrtf({})", src(0)),
            PrimitiveOp::Reciprocal => format!("(1.0f / {})", src(0)),

            PrimitiveOp::Trunc => format!("truncf({})", src(0)),
            PrimitiveOp::Max => format!("fmaxf({}, {})", src(0), src(1)),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("(({})({}))", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("*reinterpret_cast<const {}*>(&{})", dst_type, src(0)),

            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }
}

impl Renderer for CudaRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        writeln!(out, "#include <cuda_runtime.h>").unwrap();
        writeln!(out, "#include <math.h>").unwrap();
        writeln!(out).unwrap();

        // Kernel function signature
        write!(out, "extern \"C\" __global__ void molt_kernel(").unwrap();

        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.cuda_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "const ",
                BufferAccess::Write | BufferAccess::ReadWrite => "",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{}{}* buf{}", qualifier, dtype_str, binding.buf_id).unwrap();
        }
        writeln!(out, ") {{").unwrap();

        // Thread index
        writeln!(out, "    unsigned int gid = blockIdx.x * blockDim.x + threadIdx.x;").unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}) return;", output_numel).unwrap();

        let has_reduce = kernel.ops.iter().any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if !has_reduce {
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype.cuda_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        } else {
            let reduce_idx = kernel.ops.iter().position(|op| {
                matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
            }).expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs[0];
            let reduce_dtype = reduce_op.dst_dtype;

            let input_buf = match reduce_src {
                FusedSrc::Buf(idx) => &kernel.bufs[*idx],
                FusedSrc::Op(_) => &kernel.bufs[1],
                FusedSrc::Const { .. } => unreachable!("reduce on constant"),
            };
            let reduce_size = input_buf.st.numel() / output_numel;

            let init_val = match reduce_op.op {
                PrimitiveOp::ReduceSum => "0",
                PrimitiveOp::ReduceMax => "(-INFINITY)",
                _ => unreachable!(),
            };

            writeln!(out, "    {} acc = {};", reduce_dtype.cuda_type(), init_val).unwrap();

            if reduce_idx > 0 {
                writeln!(out, "    for (unsigned int rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        unsigned int eidx = gid * {} + rid;", reduce_size).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype.cuda_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
                }

                let src_var = format!("v{}", reduce_idx - 1);
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_var).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = fmaxf(acc, {});", src_var).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                writeln!(out, "    for (unsigned int rid = 0; rid < {}; rid++) {{", reduce_size).unwrap();
                writeln!(out, "        unsigned int eidx = gid * {} + rid;", reduce_size).unwrap();
                let src_expr = match reduce_src {
                    FusedSrc::Buf(idx) => Self::render_buf_read(&kernel.bufs[*idx], "eidx"),
                    _ => unreachable!(),
                };
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = fmaxf(acc, {});", src_expr).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            writeln!(out, "    {} v{} = acc;", reduce_dtype.cuda_type(), reduce_idx).unwrap();

            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype.cuda_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
