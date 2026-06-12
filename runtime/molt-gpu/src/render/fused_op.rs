use crate::dtype::DType;
use crate::ops::PrimitiveOp;

use super::FusedSrc;

/// Logical reduction domain owned by a reduce [`FusedOp`].
///
/// Renderers and interpreters consume this instead of inferring a flat
/// `input_numel / output_numel` segment. That keeps non-last-axis reductions
/// and future multi-axis reductions tied to one shared row-major mapping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReductionDomain {
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
    pub axes: Vec<usize>,
    pub kept_axes: Vec<usize>,
    pub reduce_shape: Vec<usize>,
    pub reduce_size: usize,
    pub input_strides: Vec<usize>,
}

impl ReductionDomain {
    pub fn for_single_axis(input_shape: &[usize], axis: usize) -> Self {
        Self::from_axis(input_shape, axis)
    }

    pub fn from_axis(input_shape: &[usize], axis: usize) -> Self {
        Self::from_axes(input_shape, &[axis])
    }

    pub fn from_axes(input_shape: &[usize], axes: &[usize]) -> Self {
        assert!(
            !input_shape.is_empty(),
            "Reduction input shape must be non-empty"
        );
        assert!(!axes.is_empty(), "Reduction must specify at least one axis");

        let mut axes = axes.to_vec();
        axes.sort_unstable();
        let original_len = axes.len();
        axes.dedup();
        assert_eq!(axes.len(), original_len, "Reduction axes must be unique");
        for &axis in &axes {
            assert!(
                axis < input_shape.len(),
                "Reduction axis {} out of range for rank {}",
                axis,
                input_shape.len()
            );
        }

        let mut output_shape = Vec::with_capacity(input_shape.len().saturating_sub(axes.len()));
        let mut kept_axes = Vec::with_capacity(output_shape.capacity());
        let mut reduce_shape = Vec::with_capacity(axes.len());
        for (dim, &extent) in input_shape.iter().enumerate() {
            if axes.binary_search(&dim).is_ok() {
                reduce_shape.push(extent);
            } else {
                kept_axes.push(dim);
                output_shape.push(extent);
            }
        }
        if output_shape.is_empty() {
            output_shape.push(1);
        }

        let reduce_size = reduce_shape.iter().product();
        let input_strides = row_major_strides(input_shape);
        Self {
            input_shape: input_shape.to_vec(),
            output_shape,
            axes,
            kept_axes,
            reduce_shape,
            reduce_size,
            input_strides,
        }
    }

    pub fn input_linear_index(&self, output_linear_idx: usize, reduce_linear_idx: usize) -> usize {
        assert!(output_linear_idx < self.output_numel());
        assert!(reduce_linear_idx < self.reduce_size);

        let mut coords = vec![0usize; self.input_shape.len()];
        let mut out_tmp = output_linear_idx;
        let mut red_tmp = reduce_linear_idx;

        for dim in (0..self.input_shape.len()).rev() {
            if let Some(axis_pos) = self.axes.iter().position(|&axis| axis == dim) {
                let extent = self.reduce_shape[axis_pos];
                coords[dim] = red_tmp % extent;
                red_tmp /= extent;
            } else {
                let extent = self.input_shape[dim];
                coords[dim] = out_tmp % extent;
                out_tmp /= extent;
            }
        }

        coords
            .iter()
            .zip(self.input_strides.iter())
            .map(|(coord, stride)| coord * stride)
            .sum()
    }

    pub fn output_numel(&self) -> usize {
        self.output_shape.iter().product()
    }

    pub fn is_trailing_contiguous(&self) -> bool {
        if self.axes.is_empty() {
            return false;
        }
        let first_axis = self.input_shape.len() - self.axes.len();
        self.axes
            .iter()
            .copied()
            .enumerate()
            .all(|(idx, axis)| axis == first_axis + idx)
    }
}

fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1; shape.len()];
    let mut stride = 1usize;
    for dim in (0..shape.len()).rev() {
        strides[dim] = stride;
        stride = stride
            .checked_mul(shape[dim])
            .expect("row-major stride overflow");
    }
    strides
}

/// Semantic domain for a fused op.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FusedOpDomain {
    Elementwise,
    Reduction(ReductionDomain),
}

/// A single op in a fused chain.
#[derive(Debug, Clone)]
pub struct FusedOp {
    /// The primitive op to execute.
    op: PrimitiveOp,
    /// Input sources (explicit references, not ambiguous indices).
    srcs: Vec<FusedSrc>,
    /// Output dtype. Always DType::Bool for comparison ops (Cmplt, Cmpeq, Cmpne).
    dst_dtype: DType,
    /// Per-op semantic domain.
    domain: FusedOpDomain,
}

impl FusedOp {
    pub fn elementwise(op: PrimitiveOp, srcs: Vec<FusedSrc>, dst_dtype: DType) -> Self {
        assert!(
            !matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax),
            "Reduce op {:?} must use FusedOp::reduction",
            op
        );
        Self {
            op,
            srcs,
            dst_dtype,
            domain: FusedOpDomain::Elementwise,
        }
    }

    pub fn reduction(
        op: PrimitiveOp,
        srcs: Vec<FusedSrc>,
        dst_dtype: DType,
        domain: ReductionDomain,
    ) -> Self {
        assert!(
            matches!(op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax),
            "Non-reduce op {:?} must use FusedOp::elementwise",
            op
        );
        Self {
            op,
            srcs,
            dst_dtype,
            domain: FusedOpDomain::Reduction(domain),
        }
    }

    pub fn op(&self) -> PrimitiveOp {
        self.op
    }

    pub fn srcs(&self) -> &[FusedSrc] {
        &self.srcs
    }

    pub fn dst_dtype(&self) -> DType {
        self.dst_dtype
    }

    pub fn require_reduction_domain(&self) -> &ReductionDomain {
        match &self.domain {
            FusedOpDomain::Reduction(domain) => domain,
            FusedOpDomain::Elementwise => {
                panic!(
                    "Reduce op {:?} is missing reduction-domain metadata",
                    self.op
                )
            }
        }
    }

    pub fn domain(&self) -> &FusedOpDomain {
        &self.domain
    }

    pub(crate) fn clone_with_srcs(&self, srcs: Vec<FusedSrc>) -> Self {
        Self {
            op: self.op,
            srcs,
            dst_dtype: self.dst_dtype,
            domain: self.domain.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "Reduce op ReduceSum must use FusedOp::reduction")]
    fn elementwise_rejects_reduce_ops() {
        let _ = FusedOp::elementwise(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
        );
    }

    #[test]
    #[should_panic(expected = "Non-reduce op Add must use FusedOp::elementwise")]
    fn reduction_rejects_non_reduce_ops() {
        let _ = FusedOp::reduction(
            PrimitiveOp::Add,
            vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            DType::Float32,
            ReductionDomain::from_axis(&[2, 3], 1),
        );
    }

    #[test]
    fn reduction_constructor_preserves_ranked_domain() {
        let op = FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[2, 3], 0),
        );

        match op.domain() {
            FusedOpDomain::Reduction(domain) => {
                assert_eq!(domain.input_shape, vec![2, 3]);
                assert_eq!(domain.output_shape, vec![3]);
                assert_eq!(domain.axes, vec![0]);
            }
            FusedOpDomain::Elementwise => panic!("reduce constructor must set reduction domain"),
        }
    }
}
