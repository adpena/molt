use crate::dtype::DType;
use crate::render::BufferBinding;
use crate::shapetracker::{ShapeTracker, View};

use super::MilRenderer;

impl MilRenderer {
    fn supports_logical_view_dtype(dtype: DType) -> bool {
        dtype == DType::Float32
    }

    pub(in crate::render::mil) fn assert_supported_compute_logical_view_binding(
        binding: &BufferBinding,
    ) {
        if !Self::supports_logical_view_dtype(binding.dtype) {
            panic!(
                "molt-gpu MIL renderer: ShapeTracker gather/select lowering is only verified for Float32"
            );
        }
        Self::assert_shape_numel_i32(binding.st.shape());
        Self::assert_shapetracker_i32_indexable(&binding.st);
    }

    pub(in crate::render::mil) fn assert_supported_materialize_logical_view_binding(
        binding: &BufferBinding,
    ) {
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

    pub(in crate::render::mil) fn emit_index_op(
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

    pub(in crate::render::mil) fn lower_shapetracker_index(
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
}
