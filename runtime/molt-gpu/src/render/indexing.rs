//! Shared ShapeTracker indexing codegen for renderers.
//!
//! Renderer parameters are backend-specific, but ShapeTracker semantics are
//! not. This module emits the common linear-index composition:
//!
//! `physical = view.offset + sum(index_dim * view.strides[dim])`
//!
//! across every view in a ShapeTracker, and returns the validity predicate
//! needed for masked/padded views.

use crate::render::ReductionDomain;
use crate::shapetracker::{ShapeTracker, View};

#[derive(Debug, Clone, Copy)]
pub(crate) enum IndexDialect {
    CLike,
    Wgsl,
    Glsl,
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedIndex {
    pub index: String,
    pub valid: Option<String>,
}

pub(crate) fn render_shapetracker_index(
    st: &ShapeTracker,
    linear_idx: &str,
    dialect: IndexDialect,
) -> RenderedIndex {
    if st.views.len() == 1 && st.views[0].is_contiguous() {
        return RenderedIndex {
            index: linear_idx.to_string(),
            valid: None,
        };
    }

    let mut index = cast_linear_idx(linear_idx, dialect);
    let mut valid_terms = Vec::new();

    for view in st.views.iter().rev() {
        let rendered = render_view_index(view, &index, dialect);
        if let Some(valid) = rendered.valid {
            valid_terms.push(valid);
        }
        index = rendered.index;
    }

    RenderedIndex {
        index,
        valid: combine_valid_terms(valid_terms, dialect),
    }
}

pub(crate) fn render_reduction_input_index(
    domain: &ReductionDomain,
    output_linear_idx: &str,
    reduce_linear_idx: &str,
    dialect: IndexDialect,
) -> String {
    let mut terms = Vec::with_capacity(domain.input_shape.len());

    for dim in 0..domain.input_shape.len() {
        let coord = if let Some(axis_pos) = domain.axes.iter().position(|&axis| axis == dim) {
            reduce_coord_expr(domain, axis_pos, reduce_linear_idx, dialect)
        } else {
            output_coord_expr(domain, dim, output_linear_idx, dialect)
        };

        let stride = domain.input_strides[dim];
        if stride == 0 {
            continue;
        }
        terms.push(multiply_reduction_coord_by_stride(coord, stride, dialect));
    }

    if terms.is_empty() {
        literal(0, dialect)
    } else {
        terms
            .into_iter()
            .reduce(|lhs, rhs| format!("({lhs} + {rhs})"))
            .expect("non-empty terms")
    }
}

pub(crate) fn zero_literal_for_dtype(
    dtype: crate::dtype::DType,
    bool_false: &'static str,
) -> &'static str {
    if matches!(dtype, crate::dtype::DType::Bool) {
        bool_false
    } else {
        "0"
    }
}

fn render_view_index(view: &View, linear_idx: &str, dialect: IndexDialect) -> RenderedIndex {
    let ndim = view.shape.len();
    if ndim == 0 {
        return RenderedIndex {
            index: literal(0, dialect),
            valid: None,
        };
    }

    let mut idx_sum = literal(view.offset, dialect);
    let mut valid_terms = Vec::new();
    let mut dim_indices = Vec::with_capacity(ndim);

    for dim in 0..ndim {
        let idx_expr = dim_index_expr(view, linear_idx, dim, dialect);
        dim_indices.push(idx_expr.clone());

        let stride = view.strides[dim];
        if stride != 0 {
            let term = multiply_idx_by_stride(idx_expr, stride.abs(), dialect);
            if stride > 0 {
                idx_sum = add_expr(idx_sum, term, dialect);
            } else {
                idx_sum = sub_expr(idx_sum, term, dialect);
            }
        }
    }

    if let Some(mask) = &view.mask {
        for (dim, &(lo, hi)) in mask.iter().enumerate() {
            valid_terms.push(format!(
                "({idx} >= {lo} && {idx} < {hi})",
                idx = dim_indices[dim],
                lo = literal(lo, dialect),
                hi = literal(hi, dialect)
            ));
        }
    }

    if min_physical_offset(view) < 0 {
        valid_terms.push(format!(
            "({idx} >= {zero})",
            idx = idx_sum,
            zero = literal(0, dialect)
        ));
    }

    RenderedIndex {
        index: idx_sum,
        valid: combine_valid_terms(valid_terms, dialect),
    }
}

fn dim_index_expr(view: &View, linear_idx: &str, dim: usize, dialect: IndexDialect) -> String {
    if view.shape.len() == 1 {
        return linear_idx.to_string();
    }

    let size = literal(view.shape[dim] as i64, dialect);
    if dim == view.shape.len() - 1 {
        format!("({linear_idx} % {size})")
    } else {
        let divisor: usize = view.shape[dim + 1..].iter().product();
        format!(
            "(({linear_idx} / {divisor}) % {size})",
            divisor = literal(divisor as i64, dialect)
        )
    }
}

fn reduce_coord_expr(
    domain: &ReductionDomain,
    axis_pos: usize,
    reduce_linear_idx: &str,
    dialect: IndexDialect,
) -> String {
    let divisor: usize = domain.reduce_shape[axis_pos + 1..].iter().product();
    let size = domain.reduce_shape[axis_pos];
    linear_coord_expr(reduce_linear_idx, divisor, size, dialect)
}

fn output_coord_expr(
    domain: &ReductionDomain,
    dim: usize,
    output_linear_idx: &str,
    dialect: IndexDialect,
) -> String {
    let kept_pos = domain
        .kept_axes
        .iter()
        .position(|&axis| axis == dim)
        .expect("output coord requested for reduced axis");
    let divisor: usize = domain.output_shape[kept_pos + 1..].iter().product();
    let size = domain.output_shape[kept_pos];
    linear_coord_expr(output_linear_idx, divisor, size, dialect)
}

fn linear_coord_expr(
    linear_idx: &str,
    divisor: usize,
    size: usize,
    dialect: IndexDialect,
) -> String {
    let size = reduction_literal(size, dialect);
    if divisor == 1 {
        format!("({linear_idx} % {size})")
    } else {
        format!(
            "(({linear_idx} / {divisor}) % {size})",
            divisor = reduction_literal(divisor, dialect)
        )
    }
}

fn multiply_reduction_coord_by_stride(
    coord_expr: String,
    stride: usize,
    dialect: IndexDialect,
) -> String {
    if stride == 1 {
        coord_expr
    } else {
        format!("({coord_expr} * {})", reduction_literal(stride, dialect))
    }
}

fn reduction_literal(value: usize, dialect: IndexDialect) -> String {
    match dialect {
        IndexDialect::CLike | IndexDialect::Glsl => value.to_string(),
        IndexDialect::Wgsl => format!("{value}u"),
    }
}

fn min_physical_offset(view: &View) -> i64 {
    let mut min_offset = view.offset;
    for (&shape, &stride) in view.shape.iter().zip(view.strides.iter()) {
        if stride < 0 {
            min_offset += (shape as i64 - 1) * stride;
        }
    }
    min_offset
}

fn multiply_idx_by_stride(idx_expr: String, abs_stride: i64, dialect: IndexDialect) -> String {
    if abs_stride == 1 {
        idx_expr
    } else {
        format!("({idx_expr} * {})", literal(abs_stride, dialect))
    }
}

fn add_expr(lhs: String, rhs: String, dialect: IndexDialect) -> String {
    if lhs == literal(0, dialect) {
        rhs
    } else {
        format!("({lhs} + {rhs})")
    }
}

fn sub_expr(lhs: String, rhs: String, _dialect: IndexDialect) -> String {
    format!("({lhs} - {rhs})")
}

fn combine_valid_terms(terms: Vec<String>, _dialect: IndexDialect) -> Option<String> {
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" && "))
    }
}

fn cast_linear_idx(linear_idx: &str, dialect: IndexDialect) -> String {
    match dialect {
        IndexDialect::CLike => format!("((long)({linear_idx}))"),
        IndexDialect::Wgsl => format!("i32({linear_idx})"),
        IndexDialect::Glsl => format!("int({linear_idx})"),
    }
}

fn literal(value: i64, dialect: IndexDialect) -> String {
    match dialect {
        IndexDialect::CLike | IndexDialect::Glsl => value.to_string(),
        IndexDialect::Wgsl => format!("{value}i"),
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexDialect, render_reduction_input_index, render_shapetracker_index};
    use crate::render::ReductionDomain;
    use crate::shapetracker::ShapeTracker;

    #[test]
    fn contiguous_view_is_direct_and_unguarded() {
        let rendered =
            render_shapetracker_index(&ShapeTracker::contiguous(&[4]), "gid", IndexDialect::CLike);

        assert_eq!(rendered.index, "gid");
        assert_eq!(rendered.valid, None);
    }

    #[test]
    fn flip_view_uses_signed_physical_index_without_unneeded_guard() {
        let st = ShapeTracker::contiguous(&[4]).flip(0);
        let rendered = render_shapetracker_index(&st, "gid", IndexDialect::CLike);

        assert_eq!(rendered.index, "(3 - ((long)(gid)))");
        assert_eq!(rendered.valid, None);
    }

    #[test]
    fn padded_view_returns_mask_predicate_and_signed_offset_index() {
        let st = ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]);
        let rendered = render_shapetracker_index(&st, "gid", IndexDialect::CLike);

        assert_eq!(rendered.index, "(-1 + ((long)(gid)))");
        assert_eq!(
            rendered.valid.as_deref(),
            Some("(((long)(gid)) >= 1 && ((long)(gid)) < 4) && ((-1 + ((long)(gid))) >= 0)")
        );
    }

    #[test]
    fn composed_views_walk_outer_to_inner() {
        let st = ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]);
        let rendered = render_shapetracker_index(&st, "gid", IndexDialect::CLike);

        assert_eq!(
            rendered.index,
            "(3 - ((((((long)(gid)) / 2) % 2) * 2) + (((long)(gid)) % 2)))"
        );
        assert_eq!(rendered.valid, None);
    }

    #[test]
    fn reduction_index_for_trailing_axis_matches_row_segment() {
        let domain = ReductionDomain::from_axis(&[2, 3], 1);
        let rendered = render_reduction_input_index(&domain, "gid", "rid", IndexDialect::CLike);

        assert_eq!(rendered, "(((gid % 2) * 3) + (rid % 3))");
    }

    #[test]
    fn reduction_index_for_leading_axis_uses_output_column_coord() {
        let domain = ReductionDomain::from_axis(&[2, 3], 0);
        let rendered = render_reduction_input_index(&domain, "gid", "rid", IndexDialect::CLike);

        assert_eq!(rendered, "(((rid % 2) * 3) + (gid % 3))");
    }

    #[test]
    fn wgsl_reduction_index_uses_unsigned_literals() {
        let domain = ReductionDomain::from_axis(&[2, 3], 0);
        let rendered = render_reduction_input_index(&domain, "gid", "rid", IndexDialect::Wgsl);

        assert_eq!(rendered, "(((rid % 2u) * 3u) + (gid % 3u))");
    }
}
