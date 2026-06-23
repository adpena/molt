//! HipRenderer — HIP C codegen for all 26 primitive ops.
//!
//! Generates HIP compute kernel source from FusedKernel IR.
//! Nearly identical to CUDA (HIP is source-compatible) with HIP intrinsics.
//! Thread index: `hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x`
//! Includes: `<hip/hip_runtime.h>`

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::indexing::{
    render_reduction_input_index, render_shapetracker_index, zero_literal_for_dtype, IndexDialect,
};
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, KernelBody, Renderer};

/// HIP C renderer for all 26 primitive ops.
pub struct HipRenderer;

impl HipRenderer {
    /// Format a constant value as HIP C literal.
    fn format_const(val: f64, dtype: DType) -> String {
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
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
                if val == f64::INFINITY {
                    "INFINITY".to_string()
                } else if val == f64::NEG_INFINITY {
                    "(-INFINITY)".to_string()
                } else if val.is_nan() {
                    "NAN".to_string()
                } else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        format!("{}f", s)
                    } else {
                        format!("{}.0f", s)
                    }
                }
            }
            DType::Float64 => {
                if val == f64::INFINITY {
                    "INFINITY".to_string()
                } else if val == f64::NEG_INFINITY {
                    "(-INFINITY)".to_string()
                } else if val.is_nan() {
                    "NAN".to_string()
                } else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        s
                    } else {
                        format!("{}.0", s)
                    }
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 => {
                format!("(({}){})", dtype.hip_type(), val as i64)
            }
            DType::Int64 => format!("{}LL", val as i64),
            DType::UInt8 | DType::UInt16 | DType::UInt32 => {
                format!("(({}){}u)", dtype.hip_type(), val as u64)
            }
            DType::UInt64 => format!("{}ULL", val as u64),
            // MXFP types: constants are stored as unsigned char (raw byte).
            DType::MxFP8 | DType::MxFP4 => format!("((unsigned char){})", val as u8),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(
        binding_idx: usize,
        binding: &crate::render::BufferBinding,
        idx_var: &str,
    ) -> String {
        let idx = render_shapetracker_index(&binding.st, idx_var, IndexDialect::CLike);
        let read = format!("buf{}[{}]", binding_idx, idx.index);
        if let Some(valid) = idx.valid {
            let zero = zero_literal_for_dtype(binding.dtype, "false");
            format!("({} ? {} : {})", valid, read, zero)
        } else {
            read
        }
    }

    fn render_src(src: &FusedSrc, kernel: &FusedKernel, idx_var: &str) -> String {
        match src {
            FusedSrc::Buf(buf_idx) => {
                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
            }
            FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
        }
    }

    /// Render a single op expression.
    fn render_op(op: &FusedOp, _op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String { Self::render_src(&op.srcs()[i], kernel, idx_var) };

        let dst_type = op.dst_dtype().hip_type();

        match op.op() {
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
            // NaN-propagating max: if either operand is NaN, result is NaN.
            // fmaxf is NaN-suppressing (IEEE 754 minNum), so we add an explicit NaN check.
            PrimitiveOp::Max => format!(
                "(isnan({a}) || isnan({b}) ? NAN : fmaxf({a}, {b}))",
                a = src(0),
                b = src(1)
            ),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("(({})({}))", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("*reinterpret_cast<const {}*>(&{})", dst_type, src(0)),

            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Detect FMA pattern: ADD(MUL(a, b), c) or ADD(c, MUL(a, b)).
    fn detect_fma(
        op: &FusedOp,
        op_idx: usize,
        kernel: &FusedKernel,
        idx_var: &str,
    ) -> Option<(String, String, String)> {
        if op.op() != PrimitiveOp::Add {
            return None;
        }
        if !op.dst_dtype().is_float() {
            return None;
        }

        for (mul_src_pos, add_src_pos) in [(0, 1), (1, 0)] {
            if let FusedSrc::Op(prior_idx) = &op.srcs()[mul_src_pos] {
                if *prior_idx < op_idx {
                    let prior_op = &kernel.ops[*prior_idx];
                    if prior_op.op() == PrimitiveOp::Mul {
                        let a = match &prior_op.srcs()[0] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let b = match &prior_op.srcs()[1] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let c = match &op.srcs()[add_src_pos] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
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

impl Renderer for HipRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        kernel.assert_no_mxfp_dtypes("HIP renderer");
        let mut out = String::with_capacity(4096);

        writeln!(out, "#include <hip/hip_runtime.h>").unwrap();
        writeln!(out, "#include <math.h>").unwrap();
        writeln!(out).unwrap();

        // Kernel function signature
        write!(out, "extern \"C\" __global__ void molt_kernel(").unwrap();

        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.hip_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "const ",
                BufferAccess::Write | BufferAccess::ReadWrite => "",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{}{}* buf{}", qualifier, dtype_str, i).unwrap();
        }
        writeln!(out, ") {{").unwrap();

        // Thread index — HIP intrinsics
        writeln!(
            out,
            "    unsigned int gid = hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x;"
        )
        .unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}) return;", output_numel).unwrap();

        if kernel.body == KernelBody::MaterializeCopy {
            let (_, src_binding, copy_numel) = kernel.materialize_copy_contract();
            assert_eq!(copy_numel, output_numel);
            let src = Self::render_buf_read(1, src_binding, "gid");
            writeln!(out, "    buf0[gid] = {};", src).unwrap();
            writeln!(out, "}}").unwrap();
            return out;
        }
        kernel.compute_body_contract();

        let has_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if !has_reduce {
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().hip_type();
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel, "gid") {
                    format!("fmaf({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "gid")
                };
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        } else {
            let reduce_idx = kernel
                .ops
                .iter()
                .position(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
                .expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs()[0];
            let reduce_dtype = reduce_op.dst_dtype();
            let domain = reduce_op.require_reduction_domain();
            assert_eq!(
                domain.output_numel(),
                output_numel,
                "HIP reduction domain output shape must match kernel output"
            );
            let reduce_size = domain.reduce_size;
            let reduce_index =
                render_reduction_input_index(domain, "gid", "rid", IndexDialect::CLike);

            let init_val = match reduce_op.op() {
                PrimitiveOp::ReduceSum => "0",
                PrimitiveOp::ReduceMax => "(-INFINITY)",
                _ => unreachable!(),
            };

            writeln!(out, "    {} acc = {};", reduce_dtype.hip_type(), init_val).unwrap();

            if reduce_idx > 0 {
                if reduce_size <= 16 {
                    writeln!(out, "    #pragma unroll").unwrap();
                }
                writeln!(
                    out,
                    "    for (unsigned int rid = 0; rid < {}; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        unsigned int eidx = {};", reduce_index).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype().hip_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
                }

                let src_expr = Self::render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc += {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => writeln!(
                        out,
                        "        acc = (isnan({v}) || isnan(acc)) ? NAN : fmaxf(acc, {v});",
                        v = src_expr
                    )
                    .unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                if reduce_size <= 16 {
                    writeln!(out, "    #pragma unroll").unwrap();
                }
                writeln!(
                    out,
                    "    for (unsigned int rid = 0; rid < {}; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        unsigned int eidx = {};", reduce_index).unwrap();
                let src_expr = Self::render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc += {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "        {{ float _rv = {}; acc = (isnan(_rv) || isnan(acc)) ? NAN : fmaxf(acc, _rv); }}", src_expr).unwrap();
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            writeln!(
                out,
                "    {} v{} = acc;",
                reduce_dtype.hip_type(),
                reduce_idx
            )
            .unwrap();

            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().hip_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
