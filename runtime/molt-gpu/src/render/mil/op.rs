use std::fmt::Write;

use crate::ops::PrimitiveOp;
use crate::render::{FusedKernel, FusedOp, FusedSrc};

use super::value::format_axes;
use super::{MilRenderer, MilValue};

impl MilRenderer {
    /// Render a source reference as a MIL value.
    fn render_src(
        src: &FusedSrc,
        kernel: &FusedKernel,
        input_values: &[Option<MilValue>],
        op_values: &[MilValue],
    ) -> MilValue {
        match src {
            FusedSrc::Buf(buf_idx) => {
                debug_assert!(
                    *buf_idx < kernel.bufs.len(),
                    "FusedSrc::Buf index must name a binding slot"
                );
                input_values
                    .get(*buf_idx)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| {
                        let binding = &kernel.bufs[*buf_idx];
                        MilValue::new(
                            format!("input_{}", buf_idx),
                            binding.st.shape(),
                            binding.dtype,
                        )
                    })
            }
            FusedSrc::Op(prior_idx) => op_values[*prior_idx].clone(),
            FusedSrc::Const { val, dtype } => MilValue::scalar(
                format!(
                    "const(val={}, dtype={})",
                    Self::format_const(*val, *dtype),
                    Self::mil_type(*dtype),
                ),
                *dtype,
            ),
        }
    }

    /// Render a single op as a MIL operation assignment.
    pub(in crate::render::mil) fn render_op(
        out: &mut String,
        op: &FusedOp,
        op_idx: usize,
        kernel: &FusedKernel,
        input_values: &[Option<MilValue>],
        op_values: &[MilValue],
        result_shape: &[usize],
    ) -> MilValue {
        let required_src_shape = match op.op() {
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                op.require_reduction_domain().input_shape.as_slice()
            }
            _ => result_shape,
        };
        let src_values = op
            .srcs()
            .iter()
            .enumerate()
            .map(|(idx, src)| {
                let value = Self::render_src(src, kernel, input_values, op_values);
                Self::ensure_shape(
                    out,
                    value,
                    required_src_shape,
                    &format!("v{}_src{}_shape", op_idx, idx),
                )
            })
            .collect::<Vec<_>>();
        let src = |i: usize| -> &str { src_values[i].name.as_str() };
        let dst_type = Self::mil_type(op.dst_dtype());
        let var = format!("v{}", op_idx);

        let rendered = match op.op() {
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
                    var,
                    dst_type,
                    src(0),
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
                    var,
                    dst_type,
                    src(0),
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
                    var,
                    src(0),
                    src(1),
                    src(2),
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
                let axes = format_axes(op.require_reduction_domain().axes.as_slice());
                format!(
                    "{} = reduce_sum(x={}, axes={}, keep_dims=false)",
                    var,
                    src(0),
                    axes,
                )
            }
            PrimitiveOp::ReduceMax => {
                let axes = format_axes(op.require_reduction_domain().axes.as_slice());
                format!(
                    "{} = reduce_max(x={}, axes={}, keep_dims=false)",
                    var,
                    src(0),
                    axes,
                )
            }
        };
        writeln!(out, "    {}", rendered).unwrap();
        MilValue::new(var, result_shape, op.dst_dtype())
    }
}
