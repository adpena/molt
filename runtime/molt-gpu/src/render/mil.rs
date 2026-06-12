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
use crate::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, Renderer,
};
use crate::shapetracker::{ShapeTracker, View};

/// Apple MIL IR renderer for all 26 primitive ops.
pub struct MilRenderer;

#[derive(Debug, Clone)]
struct MilValue {
    name: String,
    shape: Vec<usize>,
    dtype: DType,
}

impl MilValue {
    fn new(name: String, shape: &[usize], dtype: DType) -> Self {
        Self {
            name,
            shape: canonical_mil_shape(shape),
            dtype,
        }
    }

    fn scalar(name: String, dtype: DType) -> Self {
        Self {
            name,
            shape: Vec::new(),
            dtype,
        }
    }

    fn is_scalar(&self) -> bool {
        self.shape.is_empty()
    }
}

impl MilRenderer {
    fn supports_logical_view_dtype(dtype: DType) -> bool {
        dtype == DType::Float32
    }

    fn assert_supported_compute_logical_view_binding(binding: &BufferBinding) {
        if !Self::supports_logical_view_dtype(binding.dtype) {
            panic!(
                "molt-gpu MIL renderer: ShapeTracker gather/select lowering is only verified for Float32"
            );
        }
        Self::assert_shape_numel_i32(binding.st.shape());
        Self::assert_shapetracker_i32_indexable(&binding.st);
    }

    fn assert_supported_materialize_logical_view_binding(binding: &BufferBinding) {
        let _ = Self::mil_materialize_type(binding.dtype);
        Self::assert_shape_numel_i32(binding.st.shape());
        Self::assert_shapetracker_i32_indexable(&binding.st);
    }

    fn checked_shape_numel(shape: &[usize]) -> Option<usize> {
        shape
            .iter()
            .try_fold(1usize, |numel, &dim| numel.checked_mul(dim))
    }

    fn assert_shape_numel_i32(shape: &[usize]) -> usize {
        match Self::checked_shape_numel(shape) {
            Some(numel) if numel > 0 && numel <= i32::MAX as usize => numel,
            _ => {
                panic!(
                    "molt-gpu MIL renderer: ShapeTracker gather/select lowering requires 1..=i32::MAX elements"
                );
            }
        }
    }

    fn assert_i32_index_value(value: i64, what: &str) {
        if value < i32::MIN as i64 || value > i32::MAX as i64 {
            panic!(
                "molt-gpu MIL renderer: ShapeTracker {} value {} exceeds int32 index range",
                what, value
            );
        }
    }

    fn assert_usize_i32_index_value(value: usize, what: &str) {
        if value > i32::MAX as usize {
            panic!(
                "molt-gpu MIL renderer: ShapeTracker {} value {} exceeds int32 index range",
                what, value
            );
        }
    }

    fn assert_i128_i32_index_value(value: i128, what: &str) {
        if value < i32::MIN as i128 || value > i32::MAX as i128 {
            panic!(
                "molt-gpu MIL renderer: ShapeTracker {} value {} exceeds int32 index range",
                what, value
            );
        }
    }

    fn physical_offset_bounds(view: &View) -> (i128, i128) {
        let mut min_offset = view.offset as i128;
        let mut max_offset = view.offset as i128;
        for (&shape, &stride) in view.shape.iter().zip(view.strides.iter()) {
            let delta = (shape as i128 - 1) * stride as i128;
            if delta < 0 {
                min_offset += delta;
            } else {
                max_offset += delta;
            }
        }
        (min_offset, max_offset)
    }

    fn assert_shapetracker_i32_indexable(st: &ShapeTracker) {
        for view in &st.views {
            Self::assert_shape_numel_i32(&view.shape);
            Self::assert_i32_index_value(view.offset, "offset");
            for &shape in &view.shape {
                Self::assert_usize_i32_index_value(shape, "shape");
            }
            for &stride in &view.strides {
                Self::assert_i32_index_value(stride, "stride");
            }
            if let Some(mask) = &view.mask {
                for &(lo, hi) in mask {
                    Self::assert_i32_index_value(lo, "mask");
                    Self::assert_i32_index_value(hi, "mask");
                }
            }
            let (min_offset, max_offset) = Self::physical_offset_bounds(view);
            Self::assert_i128_i32_index_value(min_offset, "physical offset");
            Self::assert_i128_i32_index_value(max_offset, "physical offset");
        }
    }

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

    fn mil_materialize_type(dtype: DType) -> &'static str {
        match dtype {
            DType::Bool => "bool",
            DType::Int8 => "int8",
            DType::Int16 => "int16",
            DType::Int32 => "int32",
            DType::UInt8 => "uint8",
            DType::UInt16 => "uint16",
            DType::UInt32 => "uint32",
            DType::Float16 => "fp16",
            DType::Float32 => "fp32",
            DType::BFloat16 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for BFloat16 requires a distinct bf16 storage proof"
            ),
            DType::Int64 | DType::UInt64 | DType::Float64 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for 64-bit dtypes requires MIL compile and byte-roundtrip proof"
            ),
            DType::MxFP8 | DType::MxFP4 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for MXFP requires explicit block/exponent storage lowering"
            ),
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

    fn tensor_shape_for_input(binding: &BufferBinding) -> String {
        if binding.st.views.len() == 1 && binding.st.view().is_contiguous() {
            format_mil_shape(binding.st.shape())
        } else {
            "[*]".to_string()
        }
    }

    fn const_i32(value: i64) -> String {
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

    fn fresh(prefix: &str, next: &mut usize) -> String {
        let name = format!("{}_{}", prefix, *next);
        *next += 1;
        name
    }

    fn emit_named_line(out: &mut String, name: &str, expr: &str) {
        writeln!(out, "    {} = {}", name, expr).unwrap();
    }

    fn ensure_shape(
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

    fn emit_index_op(
        out: &mut String,
        prefix: &str,
        next: &mut usize,
        op: &str,
        args: &str,
    ) -> String {
        let name = Self::fresh(prefix, next);
        Self::emit_named_line(out, &name, &format!("{}({})", op, args));
        name
    }

    fn zero_index_like(
        out: &mut String,
        linear_idx: &str,
        prefix: &str,
        next: &mut usize,
    ) -> String {
        Self::emit_index_op(
            out,
            prefix,
            next,
            "mul",
            &format!("x={}, y={}", linear_idx, Self::const_i32(0)),
        )
    }

    fn add_index(out: &mut String, lhs: &str, rhs: &str, prefix: &str, next: &mut usize) -> String {
        Self::emit_index_op(out, prefix, next, "add", &format!("x={}, y={}", lhs, rhs))
    }

    fn sub_index(out: &mut String, lhs: &str, rhs: &str, prefix: &str, next: &mut usize) -> String {
        Self::emit_index_op(out, prefix, next, "sub", &format!("x={}, y={}", lhs, rhs))
    }

    fn mul_index_by_const(
        out: &mut String,
        idx: &str,
        value: i64,
        prefix: &str,
        next: &mut usize,
    ) -> String {
        if value == 1 {
            idx.to_string()
        } else {
            Self::emit_index_op(
                out,
                prefix,
                next,
                "mul",
                &format!("x={}, y={}", idx, Self::const_i32(value)),
            )
        }
    }

    fn lower_dim_index(
        out: &mut String,
        view: &View,
        linear_idx: &str,
        dim: usize,
        prefix: &str,
        next: &mut usize,
    ) -> String {
        if view.shape.len() == 1 {
            return linear_idx.to_string();
        }

        let base = if dim == view.shape.len() - 1 {
            linear_idx.to_string()
        } else {
            let divisor: usize = view.shape[dim + 1..].iter().product();
            Self::emit_index_op(
                out,
                prefix,
                next,
                "floor_div",
                &format!("x={}, y={}", linear_idx, Self::const_i32(divisor as i64)),
            )
        };
        Self::emit_index_op(
            out,
            prefix,
            next,
            "mod",
            &format!("x={}, y={}", base, Self::const_i32(view.shape[dim] as i64)),
        )
    }

    fn min_physical_offset(view: &View) -> i128 {
        Self::physical_offset_bounds(view).0
    }

    fn combine_valid_terms(
        out: &mut String,
        terms: Vec<String>,
        prefix: &str,
        next: &mut usize,
    ) -> Option<String> {
        let mut iter = terms.into_iter();
        let mut combined = iter.next()?;
        for term in iter {
            combined = Self::emit_index_op(
                out,
                prefix,
                next,
                "logical_and",
                &format!("x={}, y={}", combined, term),
            );
        }
        Some(combined)
    }

    fn lower_view_index(
        out: &mut String,
        view: &View,
        linear_idx: &str,
        prefix: &str,
        next: &mut usize,
    ) -> (String, Option<String>) {
        if view.shape.is_empty() {
            return (Self::const_i32(0), None);
        }

        let mut dim_indices = Vec::with_capacity(view.shape.len());
        for dim in 0..view.shape.len() {
            dim_indices.push(Self::lower_dim_index(
                out, view, linear_idx, dim, prefix, next,
            ));
        }

        let zero = Self::zero_index_like(out, linear_idx, prefix, next);
        let mut idx_sum: Option<String> = if view.offset == 0 {
            None
        } else {
            Some(Self::add_index(
                out,
                &zero,
                &Self::const_i32(view.offset),
                prefix,
                next,
            ))
        };

        for (dim_idx, &stride) in dim_indices.iter().zip(view.strides.iter()) {
            if stride == 0 {
                continue;
            }
            let term = Self::mul_index_by_const(out, dim_idx, stride.abs(), prefix, next);
            idx_sum = Some(match (&idx_sum, stride > 0) {
                (Some(current), true) => Self::add_index(out, current, &term, prefix, next),
                (Some(current), false) => Self::sub_index(out, current, &term, prefix, next),
                (None, true) => term,
                (None, false) => Self::sub_index(out, &zero, &term, prefix, next),
            });
        }
        let idx_sum = idx_sum.unwrap_or(zero);

        let mut valid_terms = Vec::new();
        if let Some(mask) = &view.mask {
            for (dim, &(lo, hi)) in mask.iter().enumerate() {
                let below_lo = Self::emit_index_op(
                    out,
                    prefix,
                    next,
                    "less",
                    &format!("x={}, y={}", dim_indices[dim], Self::const_i32(lo)),
                );
                valid_terms.push(Self::emit_index_op(
                    out,
                    prefix,
                    next,
                    "logical_not",
                    &format!("x={}", below_lo),
                ));
                valid_terms.push(Self::emit_index_op(
                    out,
                    prefix,
                    next,
                    "less",
                    &format!("x={}, y={}", dim_indices[dim], Self::const_i32(hi)),
                ));
            }
        }
        if Self::min_physical_offset(view) < 0 {
            let negative = Self::emit_index_op(
                out,
                prefix,
                next,
                "less",
                &format!("x={}, y={}", idx_sum, Self::const_i32(0)),
            );
            valid_terms.push(Self::emit_index_op(
                out,
                prefix,
                next,
                "logical_not",
                &format!("x={}", negative),
            ));
        }

        (
            idx_sum,
            Self::combine_valid_terms(out, valid_terms, prefix, next),
        )
    }

    fn lower_shapetracker_index(
        out: &mut String,
        st: &ShapeTracker,
        linear_idx: &str,
        prefix: &str,
        next: &mut usize,
    ) -> (String, Option<String>) {
        if st.views.len() == 1 && st.views[0].is_contiguous() {
            return (linear_idx.to_string(), None);
        }

        let mut index = linear_idx.to_string();
        let mut valid_terms = Vec::new();
        for (view_idx, view) in st.views.iter().rev().enumerate() {
            let (next_index, valid) = Self::lower_view_index(
                out,
                view,
                &index,
                &format!("{}_v{}", prefix, view_idx),
                next,
            );
            if let Some(valid) = valid {
                valid_terms.push(valid);
            }
            index = next_index;
        }

        (
            index,
            Self::combine_valid_terms(out, valid_terms, prefix, next),
        )
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
    fn render_op(
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

fn format_axes(axes: &[usize]) -> String {
    let joined = axes
        .iter()
        .map(|axis| axis.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{}]", joined)
}

fn canonical_mil_shape(shape: &[usize]) -> Vec<usize> {
    if shape.is_empty() {
        vec![1]
    } else {
        shape.to_vec()
    }
}

fn format_mil_shape(shape: &[usize]) -> String {
    let joined = canonical_mil_shape(shape)
        .iter()
        .map(|dim| dim.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{}]", joined)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{
        BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
    };
    use crate::shapetracker::ShapeTracker;

    fn make_elementwise_kernel(op: PrimitiveOp, dst_dtype: DType) -> FusedKernel {
        if matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
            let input_st = ShapeTracker::contiguous(&[1024]);
            return FusedKernel {
                body: Default::default(),
                ops: vec![FusedOp::reduction(
                    op,
                    vec![FusedSrc::Buf(1)],
                    dst_dtype,
                    ReductionDomain::from_axis(&[1024], 0),
                )],
                bufs: vec![
                    BufferBinding {
                        buf_id: 0,
                        st: ShapeTracker::contiguous(&[1]),
                        dtype: dst_dtype,
                        access: BufferAccess::Write,
                    },
                    BufferBinding {
                        buf_id: 1,
                        st: input_st,
                        dtype: DType::Float32,
                        access: BufferAccess::Read,
                    },
                ],
                grid: [1, 1, 1],
                local: [1, 1, 1],
                spec: None,
                vectorize_width: 1,
            };
        }

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
            body: Default::default(),
            ops: vec![FusedOp::elementwise(op, srcs, dst_dtype)],
            bufs,
            grid: [1024, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        }
    }

    fn make_materialize_copy_kernel(dtype: DType, src_st: ShapeTracker) -> FusedKernel {
        let numel = src_st.numel();
        FusedKernel {
            body: KernelBody::MaterializeCopy,
            ops: Vec::new(),
            bufs: vec![
                BufferBinding {
                    buf_id: 100,
                    st: ShapeTracker::contiguous(src_st.shape()),
                    dtype,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 77,
                    st: src_st,
                    dtype,
                    access: BufferAccess::Read,
                },
            ],
            grid: [numel as u32, 1, 1],
            local: [numel.clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        }
    }

    fn make_reduce_kernel(op: PrimitiveOp, input_st: ShapeTracker, axis: usize) -> FusedKernel {
        let domain = ReductionDomain::from_axis(input_st.shape(), axis);
        FusedKernel {
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                op,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                domain.clone(),
            )],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&domain.output_shape),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: input_st,
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [domain.output_numel() as u32, 1, 1],
            local: [domain.output_numel().clamp(1, 256) as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
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
    fn test_mil_names_inputs_by_binding_slot_not_storage_id() {
        let st = ShapeTracker::contiguous(&[4]);
        let kernel = FusedKernel {
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
                    st: st.clone(),
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
        };
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[4], fp32>"));
        assert!(source.contains("input_2: tensor<[4], fp32>"));
        assert!(source.contains("add(x=input_1, y=input_2)"));
        assert!(!source.contains("input_77"));
    }

    #[test]
    fn test_mil_compute_materializes_same_storage_distinct_views() {
        let st = ShapeTracker::contiguous(&[4]);
        let kernel = FusedKernel {
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
        };

        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], fp32>"));
        assert!(source.contains("input_2: tensor<[4], fp32>"));
        assert!(source.contains("gather(x=input_1"));
        assert!(source.contains("add(x=raw_input_1, y=input_2)"));
        assert!(!source.contains("input_77"));
    }

    #[test]
    fn test_mil_materialize_copy_from_flipped_view() {
        let kernel =
            make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[4]).flip(0));
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], fp32>"));
        assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
        assert!(source.contains("const(val=3, dtype=int32)"));
        assert!(source.contains("sub(x="));
        assert!(source.contains("gather(x=input_1"));
        assert!(source.contains("return raw_input_1: tensor<[4], fp32>"));
        assert!(!source.contains("input_77"));
    }

    #[test]
    fn test_mil_materialize_copy_contiguous_returns_input_slot() {
        let kernel = make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[4]));
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[4], fp32>"));
        assert!(source.contains("return input_1: tensor<[4], fp32>"));
        assert!(!source.contains("gather("));
        assert!(!source.contains("idx_1 = range_1d"));
        assert!(!source.contains("input_77"));
    }

    #[test]
    fn test_mil_materialize_copy_from_padded_view_zero_fills() {
        let kernel = make_materialize_copy_kernel(
            DType::Float32,
            ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        );
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], fp32>"));
        assert!(source.contains("idx_1 = range_1d(start=0, end=5, step=1, dtype=\"int32\")"));
        assert!(source.contains("less(x=idx_1, y=const(val=1, dtype=int32))"));
        assert!(source.contains("logical_not"));
        assert!(source.contains("logical_and"));
        assert!(source.contains("select(cond="));
        assert!(source.contains("gather(x=input_1"));
        assert!(source.contains("view_input_1 = select"));
        assert!(source.contains("b=const(val=0, dtype=fp32)"));
        assert!(source.contains("return view_input_1: tensor<[5], fp32>"));
    }

    #[test]
    fn test_mil_materialize_copy_padded_safe_index_feeds_gather_before_zero_fill() {
        let kernel = make_materialize_copy_kernel(
            DType::Float32,
            ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        );
        let source = MilRenderer.render(&kernel);

        let safe_index_pos = source
            .find("view1_safe")
            .expect("padded MIL materialization must emit a safe gather index");
        let gather_pos = source
            .find("raw_input_1 = gather")
            .expect("padded MIL materialization must gather from the safe index");
        let zero_fill_pos = source
            .find("view_input_1 = select")
            .expect("padded MIL materialization must zero-fill after gather");

        assert!(safe_index_pos < gather_pos);
        assert!(gather_pos < zero_fill_pos);
        assert!(source.contains("indices=view1_safe"));
        assert!(source.contains("b=const(val=0, dtype=fp32)"));
    }

    #[test]
    fn test_mil_materialize_copy_composes_multiple_views() {
        let kernel = make_materialize_copy_kernel(
            DType::Float32,
            ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]),
        );
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
        assert!(source.contains("floor_div"));
        assert!(source.contains("mod"));
        assert!(source.contains("const(val=3, dtype=int32)"));
        assert!(source.contains("sub(x="));
        assert!(source.contains("gather(x=input_1"));
    }

    #[test]
    fn test_mil_materialize_copy_from_expanded_zero_stride_view() {
        let kernel = make_materialize_copy_kernel(
            DType::Float32,
            ShapeTracker::contiguous(&[1]).expand(&[4]),
        );
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("idx_1 = range_1d(start=0, end=4, step=1, dtype=\"int32\")"));
        assert!(source.contains("mul(x=idx_1, y=const(val=0, dtype=int32))"));
        assert!(source.contains("gather(x=input_1"));
        assert!(!source.contains("view_input_1 = select"));
    }

    #[test]
    fn test_mil_materialize_copy_uint32_from_flipped_view() {
        let kernel =
            make_materialize_copy_kernel(DType::UInt32, ShapeTracker::contiguous(&[4]).flip(0));
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], uint32>"));
        assert!(source.contains("gather(x=input_1"));
        assert!(source.contains("return raw_input_1: tensor<[4], uint32>"));
    }

    #[test]
    fn test_mil_materialize_copy_int16_padded_zero_fills_with_int16_zero() {
        let kernel = make_materialize_copy_kernel(
            DType::Int16,
            ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        );
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], int16>"));
        assert!(source.contains("view_input_1 = select"));
        assert!(source.contains("b=const(val=0, dtype=int16)"));
        assert!(source.contains("return view_input_1: tensor<[5], int16>"));
    }

    #[test]
    fn test_mil_materialize_copy_bool_padded_zero_fills_with_false() {
        let kernel = make_materialize_copy_kernel(
            DType::Bool,
            ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        );
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[*], bool>"));
        assert!(source.contains("view_input_1 = select"));
        assert!(source.contains("b=const(val=false, dtype=bool)"));
        assert!(source.contains("return view_input_1: tensor<[5], bool>"));
    }

    #[test]
    fn test_mil_materialize_copy_supported_integer_zero_literals_by_dtype() {
        for (dtype, expected) in [
            (DType::Int8, "b=const(val=0, dtype=int8)"),
            (DType::UInt8, "b=const(val=0, dtype=uint8)"),
            (DType::Int16, "b=const(val=0, dtype=int16)"),
            (DType::UInt16, "b=const(val=0, dtype=uint16)"),
            (DType::Int32, "b=const(val=0, dtype=int32)"),
            (DType::UInt32, "b=const(val=0, dtype=uint32)"),
        ] {
            let kernel =
                make_materialize_copy_kernel(dtype, ShapeTracker::contiguous(&[2]).pad(&[(1, 1)]));
            let source = MilRenderer.render(&kernel);
            assert!(
                source.contains(expected),
                "missing zero literal {expected} for {dtype:?}\n{source}"
            );
        }
    }

    #[test]
    fn test_mil_materialize_copy_rejects_unverified_storage_dtypes() {
        for (dtype, expected) in [
            (
                DType::BFloat16,
                "BFloat16 requires a distinct bf16 storage proof",
            ),
            (DType::Int64, "64-bit dtypes requires MIL compile"),
            (DType::UInt64, "64-bit dtypes requires MIL compile"),
            (DType::Float64, "64-bit dtypes requires MIL compile"),
            (
                DType::MxFP8,
                "MXFP requires explicit block/exponent storage lowering",
            ),
            (
                DType::MxFP4,
                "MXFP requires explicit block/exponent storage lowering",
            ),
        ] {
            let kernel =
                make_materialize_copy_kernel(dtype, ShapeTracker::contiguous(&[4]).flip(0));
            let panic = std::panic::catch_unwind(|| {
                let _ = MilRenderer.render(&kernel);
            })
            .expect_err("unsupported MIL materialize dtype should panic");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&'static str>().copied())
                .unwrap_or("<non-string panic>");
            assert!(
                message.contains(expected),
                "panic for {dtype:?} should contain {expected:?}, got {message:?}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "requires 1..=i32::MAX elements")]
    fn test_mil_materialize_copy_rejects_zero_numel() {
        let kernel = make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[0]));

        let _ = MilRenderer.render(&kernel);
    }

    #[test]
    #[should_panic(expected = "requires 1..=i32::MAX elements")]
    fn test_mil_materialize_copy_rejects_too_large_numel() {
        let too_large = i32::MAX as usize + 1;
        let kernel =
            make_materialize_copy_kernel(DType::Float32, ShapeTracker::contiguous(&[too_large]));

        let _ = MilRenderer.render(&kernel);
    }

    #[test]
    #[should_panic(expected = "offset value")]
    fn test_mil_materialize_copy_rejects_out_of_range_offset_constant() {
        let huge = i32::MAX as usize + 2;
        let st = ShapeTracker::contiguous(&[huge]).shrink(&[(huge - 1, huge)]);
        let kernel = make_materialize_copy_kernel(DType::Float32, st);

        let _ = MilRenderer.render(&kernel);
    }

    #[test]
    #[should_panic(expected = "stride value")]
    fn test_mil_materialize_copy_rejects_out_of_range_stride_constant() {
        let huge = i32::MAX as usize + 1;
        let st = ShapeTracker::contiguous(&[2, huge])
            .permute(&[1, 0])
            .shrink(&[(0, 1), (0, 1)]);
        let kernel = make_materialize_copy_kernel(DType::Float32, st);

        let _ = MilRenderer.render(&kernel);
    }

    #[test]
    #[should_panic(expected = "physical offset value")]
    fn test_mil_materialize_copy_rejects_out_of_range_physical_offset() {
        let stride_fits_i32 = i32::MAX as usize;
        let st = ShapeTracker::contiguous(&[3, stride_fits_i32]).shrink(&[(0, 3), (0, 1)]);
        let kernel = make_materialize_copy_kernel(DType::Float32, st);

        let _ = MilRenderer.render(&kernel);
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
            body: Default::default(),
            ops: vec![FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Buf(1)],
                DType::Float32,
                ReductionDomain::from_axis(&[1024], 0),
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
                    st,
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [1, 1, 1],
            local: [1, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let renderer = MilRenderer;
        let source = renderer.render(&kernel);
        assert!(source.contains("reduce_sum(x=input_1, axes=[0], keep_dims=false)"));
    }

    #[test]
    fn test_mil_render_reduce_sum_axis0_keeps_ranked_input_shape() {
        let kernel =
            make_reduce_kernel(PrimitiveOp::ReduceSum, ShapeTracker::contiguous(&[2, 3]), 0);
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[2, 3], fp32>"));
        assert!(source.contains("reduce_sum(x=input_1, axes=[0], keep_dims=false)"));
        assert!(source.contains("return v0: tensor<[3], fp32>"));
        assert!(!source.contains("input_1: tensor<[6], fp32>"));
        assert!(!source.contains("reduce_sum(x=v0_src0_shape"));
    }

    #[test]
    fn test_mil_render_reduce_max_axis1_keeps_ranked_input_shape() {
        let kernel =
            make_reduce_kernel(PrimitiveOp::ReduceMax, ShapeTracker::contiguous(&[2, 3]), 1);
        let source = MilRenderer.render(&kernel);

        assert!(source.contains("input_1: tensor<[2, 3], fp32>"));
        assert!(source.contains("reduce_max(x=input_1, axes=[1], keep_dims=false)"));
        assert!(source.contains("return v0: tensor<[2], fp32>"));
    }

    #[test]
    fn test_mil_render_noncontiguous_reduce_reshapes_gather_before_axis_reduce() {
        let input_st = ShapeTracker::contiguous(&[6]).flip(0).reshape(&[2, 3]);
        let kernel = make_reduce_kernel(PrimitiveOp::ReduceSum, input_st, 0);
        let source = MilRenderer.render(&kernel);

        let gather_pos = source
            .find("raw_input_1 = gather")
            .expect("non-contiguous reduction input must gather physical storage");
        let reshape_pos = source
            .find("logical_input_1 = reshape(x=raw_input_1, shape=[2, 3])")
            .expect("gathered flat view must be restored to the logical reduction rank");
        let reduce_pos = source
            .find("reduce_sum(x=logical_input_1, axes=[0], keep_dims=false)")
            .expect("axis reduction must consume the ranked logical view");

        assert!(gather_pos < reshape_pos);
        assert!(reshape_pos < reduce_pos);
        assert!(source.contains("input_1: tensor<[*], fp32>"));
        assert!(source.contains("return v0: tensor<[3], fp32>"));
    }

    #[test]
    fn test_mil_render_masked_reduce_zero_fills_then_reshapes_before_axis_reduce() {
        let input_st = ShapeTracker::contiguous(&[1, 3]).pad(&[(1, 0), (0, 0)]);
        let kernel = make_reduce_kernel(PrimitiveOp::ReduceSum, input_st, 0);
        let source = MilRenderer.render(&kernel);

        let safe_index_pos = source
            .find("view1_safe")
            .expect("masked reduction input must select a safe gather index");
        let gather_pos = source
            .find("raw_input_1 = gather")
            .expect("masked reduction input must gather physical storage");
        let zero_fill_pos = source
            .find("view_input_1 = select")
            .expect("masked reduction input must zero-fill invalid lanes");
        let reshape_pos = source
            .find("logical_input_1 = reshape(x=view_input_1, shape=[2, 3])")
            .expect("zero-filled flat view must be restored to the logical reduction rank");
        let reduce_pos = source
            .find("reduce_sum(x=logical_input_1, axes=[0], keep_dims=false)")
            .expect("axis reduction must consume the ranked zero-filled view");

        assert!(safe_index_pos < gather_pos);
        assert!(gather_pos < zero_fill_pos);
        assert!(zero_fill_pos < reshape_pos);
        assert!(reshape_pos < reduce_pos);
        assert!(source.contains("b=const(val=0, dtype=fp32)"));
    }

    #[test]
    fn test_mil_render_all_26_ops() {
        // Verify that every primitive op can be rendered without panic.
        let renderer = MilRenderer;
        for op in PrimitiveOp::ALL {
            let dst_dtype = if matches!(
                op,
                PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
            ) {
                DType::Bool
            } else {
                DType::Float32
            };
            let kernel = make_elementwise_kernel(op, dst_dtype);
            let source = renderer.render(&kernel);
            assert!(!source.is_empty(), "Empty render for op {:?}", op);
            assert!(
                source.contains("mil_program"),
                "Missing header for op {:?}",
                op
            );
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
        assert_eq!(
            MilRenderer::format_const(f64::INFINITY, DType::Float32),
            "inf"
        );
        assert_eq!(
            MilRenderer::format_const(f64::NEG_INFINITY, DType::Float32),
            "-inf"
        );
    }
}
