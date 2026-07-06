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
use crate::render::{BufferAccess, BufferBinding, FusedKernel, KernelBody, Renderer};

mod index;
mod op;
#[cfg(test)]
mod tests;
mod value;

use value::{MilValue, canonical_mil_shape, format_mil_shape};

/// Apple MIL IR renderer for all 26 primitive ops.
pub struct MilRenderer;

impl MilRenderer {
    fn tensor_shape_for_input(binding: &BufferBinding) -> String {
        if binding.st.views.len() == 1 && binding.st.view().is_contiguous() {
            format_mil_shape(binding.st.shape())
        } else {
            "[*]".to_string()
        }
    }

    pub(in crate::render::mil) fn const_i32(value: i64) -> String {
        format!("const(val={}, dtype=int32)", value)
    }

    fn const_for_dtype(value: f64, dtype: DType) -> String {
        format!(
            "const(val={}, dtype={})",
            Self::format_const(value, dtype),
            Self::mil_type(dtype)
        )
    }

    fn materialize_zero_for_dtype(dtype: DType) -> String {
        let ty = Self::mil_materialize_type(dtype);
        let value = match dtype {
            DType::Bool => "false".to_string(),
            DType::Int8 | DType::Int16 | DType::Int32 => "0".to_string(),
            DType::UInt8 | DType::UInt16 | DType::UInt32 => "0".to_string(),
            DType::Float16 | DType::Float32 => "0".to_string(),
            DType::BFloat16
            | DType::Int64
            | DType::UInt64
            | DType::Float64
            | DType::MxFP8
            | DType::MxFP4 => unreachable!("mil_materialize_type rejects unsupported dtypes"),
        };
        format!("const(val={}, dtype={})", value, ty)
    }

    pub(in crate::render::mil) fn fresh(prefix: &str, next: &mut usize) -> String {
        let name = format!("{}_{}", prefix, *next);
        *next += 1;
        name
    }

    pub(in crate::render::mil) fn emit_named_line(out: &mut String, name: &str, expr: &str) {
        writeln!(out, "    {} = {}", name, expr).unwrap();
    }

    pub(in crate::render::mil) fn ensure_shape(
        out: &mut String,
        value: MilValue,
        required_shape: &[usize],
        name: &str,
    ) -> MilValue {
        if value.is_scalar() {
            return value;
        }

        let required_shape = canonical_mil_shape(required_shape);
        if value.shape == required_shape {
            return value;
        }

        let value_numel = value.shape.iter().product::<usize>();
        let required_numel = required_shape.iter().product::<usize>();
        assert_eq!(
            value_numel, required_numel,
            "molt-gpu MIL renderer: cannot reshape {} from {:?} to {:?}",
            value.name, value.shape, required_shape
        );
        Self::emit_named_line(
            out,
            name,
            &format!(
                "reshape(x={}, shape={})",
                value.name,
                format_mil_shape(&required_shape)
            ),
        );
        MilValue {
            name: name.to_string(),
            shape: required_shape,
            dtype: value.dtype,
        }
    }

    fn render_logical_view_value(
        out: &mut String,
        binding_idx: usize,
        binding: &BufferBinding,
    ) -> MilValue {
        if binding.st.views.len() == 1 && binding.st.view().is_contiguous() {
            return MilValue::new(
                format!("input_{}", binding_idx),
                binding.st.shape(),
                binding.dtype,
            );
        }
        Self::assert_supported_compute_logical_view_binding(binding);
        let value = Self::render_logical_view_value_with_zero(
            out,
            binding_idx,
            binding,
            Self::const_for_dtype(0.0, binding.dtype),
        );
        Self::ensure_shape(
            out,
            value,
            binding.st.shape(),
            &format!("logical_input_{}", binding_idx),
        )
    }

    fn render_materialize_logical_view_value(
        out: &mut String,
        binding_idx: usize,
        binding: &BufferBinding,
    ) -> MilValue {
        if binding.st.views.len() == 1 && binding.st.view().is_contiguous() {
            return MilValue::new(
                format!("input_{}", binding_idx),
                binding.st.shape(),
                binding.dtype,
            );
        }
        Self::assert_supported_materialize_logical_view_binding(binding);
        let value = Self::render_logical_view_value_with_zero(
            out,
            binding_idx,
            binding,
            Self::materialize_zero_for_dtype(binding.dtype),
        );
        Self::ensure_shape(
            out,
            value,
            binding.st.shape(),
            &format!("logical_input_{}", binding_idx),
        )
    }

    fn render_logical_view_value_with_zero(
        out: &mut String,
        binding_idx: usize,
        binding: &BufferBinding,
        zero_literal: String,
    ) -> MilValue {
        let mut next = 0usize;
        let prefix = format!("view{}_idx", binding_idx);
        let idx_name = format!("idx_{}", binding_idx);
        writeln!(
            out,
            "    {} = range_1d(start=0, end={}, step=1, dtype=\"int32\")",
            idx_name,
            binding.st.numel()
        )
        .unwrap();
        let (physical_idx, valid) =
            Self::lower_shapetracker_index(out, &binding.st, &idx_name, &prefix, &mut next);

        let gather_idx = if let Some(valid) = valid.as_ref() {
            Self::emit_index_op(
                out,
                &format!("view{}_safe", binding_idx),
                &mut next,
                "select",
                &format!(
                    "cond={}, a={}, b={}",
                    valid,
                    physical_idx,
                    Self::const_i32(0)
                ),
            )
        } else {
            physical_idx
        };
        let raw_name = format!("raw_input_{}", binding_idx);
        Self::emit_named_line(
            out,
            &raw_name,
            &format!(
                "gather(x=input_{}, indices={}, axis=0)",
                binding_idx, gather_idx
            ),
        );
        if let Some(valid) = valid {
            let view_name = format!("view_input_{}", binding_idx);
            Self::emit_named_line(
                out,
                &view_name,
                &format!("select(cond={}, a={}, b={})", valid, raw_name, zero_literal),
            );
            MilValue::new(view_name, &[binding.st.numel()], binding.dtype)
        } else {
            MilValue::new(raw_name, &[binding.st.numel()], binding.dtype)
        }
    }
}

impl Renderer for MilRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        if kernel.body == KernelBody::MaterializeCopy {
            let (dst, src, _) = kernel.materialize_copy_contract();
            Self::assert_supported_materialize_logical_view_binding(dst);
            Self::assert_supported_materialize_logical_view_binding(src);
            let src_type = Self::mil_materialize_type(src.dtype);
            let dst_type = Self::mil_materialize_type(dst.dtype);

            let mut out = String::with_capacity(4096);
            writeln!(out, "mil_program {{").unwrap();
            writeln!(out, "  func main(").unwrap();
            writeln!(
                out,
                "    input_1: tensor<{}, {}>,",
                Self::tensor_shape_for_input(src),
                src_type,
            )
            .unwrap();
            writeln!(out, "  ) {{").unwrap();
            let value = Self::render_materialize_logical_view_value(&mut out, 1, src);
            writeln!(
                out,
                "    return {}: tensor<{}, {}>",
                value.name,
                format_mil_shape(&value.shape),
                dst_type,
            )
            .unwrap();
            writeln!(out, "  }}").unwrap();
            writeln!(out, "}}").unwrap();
            return out;
        }
        kernel.compute_body_contract();

        let mut out = String::with_capacity(4096);
        let mut input_values = vec![None; kernel.bufs.len()];
        let reduce_domain = kernel
            .ops
            .iter()
            .enumerate()
            .find(|(_, op)| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
            .map(|(idx, op)| (idx, op.require_reduction_domain().clone()));

        // MIL program header
        writeln!(out, "mil_program {{").unwrap();
        writeln!(out, "  func main(").unwrap();

        // Input parameters
        for (binding_idx, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = Self::mil_type(binding.dtype);
            match binding.access {
                BufferAccess::Read => {
                    writeln!(
                        out,
                        "    input_{}: tensor<{}, {}>,",
                        binding_idx,
                        Self::tensor_shape_for_input(binding),
                        dtype_str,
                    )
                    .unwrap();
                }
                BufferAccess::Write | BufferAccess::ReadWrite => {
                    // Output declared in return type, not as parameter
                }
            }
        }
        writeln!(out, "  ) {{").unwrap();

        for (binding_idx, binding) in kernel.bufs.iter().enumerate() {
            if binding.access == BufferAccess::Read {
                input_values[binding_idx] = Some(Self::render_logical_view_value(
                    &mut out,
                    binding_idx,
                    binding,
                ));
            }
        }

        // Emit ops
        let output_shape = canonical_mil_shape(kernel.bufs[0].st.shape());
        let mut op_values = Vec::with_capacity(kernel.ops.len());
        for (i, op) in kernel.ops.iter().enumerate() {
            let result_shape = match &reduce_domain {
                Some((reduce_idx, domain)) if i < *reduce_idx => domain.input_shape.as_slice(),
                Some((_, domain)) => domain.output_shape.as_slice(),
                None => output_shape.as_slice(),
            };
            let value = Self::render_op(
                &mut out,
                op,
                i,
                kernel,
                &input_values,
                &op_values,
                result_shape,
            );
            op_values.push(value);
        }

        // Return the last op result, written to the output buffer
        let out_dtype = Self::mil_type(kernel.bufs[0].dtype);
        let final_value = op_values
            .pop()
            .expect("Compute kernels must carry at least one op");
        let final_value =
            Self::ensure_shape(&mut out, final_value, &output_shape, "return_value_shape");
        writeln!(
            out,
            "    return {}: tensor<{}, {}>",
            final_value.name,
            format_mil_shape(&final_value.shape),
            out_dtype,
        )
        .unwrap();
        writeln!(out, "  }}").unwrap();
        writeln!(out, "}}").unwrap();

        out
    }
}
