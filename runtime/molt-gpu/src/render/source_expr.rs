//! Shared source-expression helpers for text shader renderers.
//!
//! Language renderers own their literal syntax and buffer-index syntax. This
//! module owns the renderer-independent traversal of `FusedSrc` and fused FMA
//! pattern detection so every backend optimizes the same graph shape.

use crate::dtype::DType;
use crate::render::{BufferBinding, FusedKernel, FusedOp, FusedSrc, detect_fma_pattern};

pub(crate) trait SourceExprRenderer {
    fn render_source_buf_read(
        &self,
        binding_idx: usize,
        binding: &BufferBinding,
        idx_var: &str,
    ) -> String;

    fn format_source_const(&self, val: f64, dtype: DType) -> String;
}

pub(crate) fn render_src<R>(
    renderer: &R,
    src: &FusedSrc,
    kernel: &FusedKernel,
    idx_var: &str,
) -> String
where
    R: SourceExprRenderer + ?Sized,
{
    match src {
        FusedSrc::Buf(buf_idx) => {
            renderer.render_source_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
        }
        FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
        FusedSrc::Const { val, dtype } => renderer.format_source_const(*val, *dtype),
    }
}

pub(crate) fn detect_fma<R, F>(
    renderer: &R,
    op: &FusedOp,
    op_idx: usize,
    kernel: &FusedKernel,
    idx_var: &str,
    dst_is_float: F,
) -> Option<(String, String, String)>
where
    R: SourceExprRenderer + ?Sized,
    F: FnOnce(DType) -> bool,
{
    let pattern = detect_fma_pattern(op, op_idx, kernel, dst_is_float(op.dst_dtype()))?;
    let prior_op = &kernel.ops[pattern.mul_op_idx];
    Some((
        render_src(renderer, &prior_op.srcs()[0], kernel, idx_var),
        render_src(renderer, &prior_op.srcs()[1], kernel, idx_var),
        render_src(renderer, &op.srcs()[pattern.add_src_pos], kernel, idx_var),
    ))
}
