//! ShapeTracker + View — zero-copy view system for movement ops.
//!
//! All movement ops (reshape, permute, expand, pad, shrink, flip) are
//! O(1) modifications to the view. No GPU kernel, no memory copy.
//!
//! Performance optimizations:
//! - `is_contiguous` is cached at construction time (not recomputed per call).
//! - `expr_idx` has a fast path for contiguous views (linear_idx == buffer offset).
//! - Specialized `expr_idx_1d`, `expr_idx_2d`, `expr_idx_3d` avoid the general loop.

#[inline(always)]
fn checked_shape_numel(shape: &[usize]) -> Option<usize> {
    shape
        .iter()
        .try_fold(1usize, |numel, &dim| numel.checked_mul(dim))
}

#[inline(always)]
fn usize_to_i64(value: usize, context: &str) -> i64 {
    i64::try_from(value).unwrap_or_else(|_| panic!("{context} exceeds i64 capacity: {value}"))
}

#[inline(always)]
fn checked_shape_add(left: usize, right: usize, context: &str) -> usize {
    left.checked_add(right)
        .unwrap_or_else(|| panic!("{context} overflows usize: {left} + {right}"))
}

#[inline(always)]
fn checked_shape_sub(left: usize, right: usize, context: &str) -> usize {
    left.checked_sub(right)
        .unwrap_or_else(|| panic!("{context} underflows usize: {left} - {right}"))
}

#[inline(always)]
fn validate_shape_i64(shape: &[usize], context: &str) {
    for &dim in shape {
        let _ = usize_to_i64(dim, context);
    }
}

#[inline(always)]
fn checked_stride_term(index: usize, stride: i64, context: &str) -> i64 {
    usize_to_i64(index, context)
        .checked_mul(stride)
        .unwrap_or_else(|| panic!("{context} stride term overflows i64: {index} * {stride}"))
}

#[inline(always)]
fn checked_offset_add(offset: i64, term: i64, context: &str) -> i64 {
    offset
        .checked_add(term)
        .unwrap_or_else(|| panic!("{context} offset overflows i64: {offset} + {term}"))
}

#[inline(always)]
fn checked_offset_sub(offset: i64, term: i64, context: &str) -> i64 {
    offset
        .checked_sub(term)
        .unwrap_or_else(|| panic!("{context} offset overflows i64: {offset} - {term}"))
}

#[inline(always)]
fn nonnegative_offset_to_usize(offset: i64, context: &str) -> Option<usize> {
    if offset < 0 {
        return None;
    }
    Some(
        usize::try_from(offset)
            .unwrap_or_else(|_| panic!("{context} exceeds usize capacity: {offset}")),
    )
}

/// A single view into a contiguous buffer.
///
/// Describes how to access a region of a flat buffer via shape, strides,
/// offset, and optional validity mask. Movement ops modify views rather
/// than copying data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct View {
    /// Logical shape of this view.
    pub shape: Vec<usize>,
    /// Stride per dimension. 0 = broadcast, negative = flipped.
    pub strides: Vec<i64>,
    /// Offset into the underlying buffer (in elements).
    pub offset: i64,
    /// Optional validity mask per dimension: element is valid iff
    /// mask[dim].0 <= idx[dim] < mask[dim].1.
    /// Elements outside the mask return the padding value (typically 0).
    pub mask: Option<Vec<(i64, i64)>>,
    /// Cached result of contiguity check. Set at construction time.
    /// When true, `expr_idx(i)` == `Some(i)` for all valid indices.
    is_contiguous_cache: bool,
}

impl View {
    /// Create a contiguous view for a given shape.
    /// Strides are row-major (C-order): last dimension is stride 1.
    pub fn contiguous(shape: &[usize]) -> Self {
        validate_shape_i64(shape, "contiguous shape dimension");
        let ndim = shape.len();
        let mut strides = vec![0i64; ndim];
        if ndim > 0 {
            strides[ndim - 1] = 1;
            for i in (0..ndim - 1).rev() {
                let dim = usize_to_i64(shape[i + 1], "row-major shape dimension");
                strides[i] = strides[i + 1]
                    .checked_mul(dim)
                    .expect("row-major stride overflows i64");
            }
        }
        Self {
            shape: shape.to_vec(),
            strides,
            offset: 0,
            mask: None,
            is_contiguous_cache: true, // contiguous by construction
        }
    }

    /// Create a view with explicit fields and compute the contiguity cache.
    fn new(
        shape: Vec<usize>,
        strides: Vec<i64>,
        offset: i64,
        mask: Option<Vec<(i64, i64)>>,
    ) -> Self {
        validate_shape_i64(&shape, "view shape dimension");
        let is_contiguous_cache = Self::compute_is_contiguous(&shape, &strides, offset, &mask);
        Self {
            shape,
            strides,
            offset,
            mask,
            is_contiguous_cache,
        }
    }

    /// Compute contiguity without constructing a View (used during construction).
    fn compute_is_contiguous(
        shape: &[usize],
        strides: &[i64],
        offset: i64,
        mask: &Option<Vec<(i64, i64)>>,
    ) -> bool {
        if offset != 0 || mask.is_some() {
            return false;
        }
        let ndim = shape.len();
        if ndim == 0 {
            return true;
        }
        // Check row-major strides: last dim stride=1, each preceding dim = product of subsequent dims
        let mut expected_stride: i64 = 1;
        for i in (0..ndim).rev() {
            if strides[i] != expected_stride {
                return false;
            }
            let dim = match i64::try_from(shape[i]) {
                Ok(dim) => dim,
                Err(_) => return false,
            };
            expected_stride = match expected_stride.checked_mul(dim) {
                Some(stride) => stride,
                None => return false,
            };
        }
        true
    }

    /// Checked total number of logical elements.
    #[inline(always)]
    pub fn checked_numel(&self) -> Option<usize> {
        checked_shape_numel(&self.shape)
    }

    /// Total number of logical elements.
    #[inline(always)]
    pub fn numel(&self) -> usize {
        self.checked_numel()
            .expect("shape logical element count overflows usize")
    }

    /// Whether this view is contiguous (row-major, no mask, offset=0).
    /// O(1): returns the cached result computed at construction time.
    #[inline(always)]
    pub fn is_contiguous(&self) -> bool {
        self.is_contiguous_cache
    }

    /// Convert a linear index to multi-dimensional indices.
    #[inline(always)]
    fn linear_to_indices(&self, linear_idx: usize) -> Vec<usize> {
        let ndim = self.shape.len();
        let mut indices = vec![0usize; ndim];
        let mut remaining = linear_idx;
        for i in (0..ndim).rev() {
            indices[i] = remaining % self.shape[i];
            remaining /= self.shape[i];
        }
        indices
    }

    /// Compute the actual buffer offset from a linear logical index.
    /// Returns None if the index falls in a masked (padding) region.
    ///
    /// Fast path: for contiguous views, returns `Some(linear_idx)` directly.
    /// Specialized paths for 1D, 2D, 3D avoid the general loop.
    #[inline(always)]
    pub fn expr_idx(&self, linear_idx: usize) -> Option<usize> {
        // Fast path: contiguous view — buffer offset IS the linear index.
        if self.is_contiguous_cache {
            return Some(linear_idx);
        }

        let ndim = self.shape.len();

        // Specialized paths for common dimensionalities.
        match ndim {
            0 => return Some(0),
            1 => return self.expr_idx_1d(linear_idx),
            2 => return self.expr_idx_2d(linear_idx),
            3 => return self.expr_idx_3d(linear_idx),
            _ => {}
        }

        // General path for 4D+
        self.expr_idx_general(linear_idx)
    }

    /// Specialized 1D index computation.
    #[inline(always)]
    fn expr_idx_1d(&self, linear_idx: usize) -> Option<usize> {
        let idx = linear_idx;

        if let Some(ref mask) = self.mask {
            let idx_i64 = usize_to_i64(idx, "1D logical index");
            if idx_i64 < mask[0].0 || idx_i64 >= mask[0].1 {
                return None;
            }
        }

        let buf_offset = checked_offset_add(
            self.offset,
            checked_stride_term(idx, self.strides[0], "1D buffer offset"),
            "1D buffer offset",
        );
        nonnegative_offset_to_usize(buf_offset, "1D buffer offset")
    }

    /// Specialized 2D index computation.
    #[inline(always)]
    fn expr_idx_2d(&self, linear_idx: usize) -> Option<usize> {
        let i1 = linear_idx % self.shape[1];
        let i0 = linear_idx / self.shape[1];

        if let Some(ref mask) = self.mask {
            let i0_i64 = usize_to_i64(i0, "2D logical index");
            let i1_i64 = usize_to_i64(i1, "2D logical index");
            if i0_i64 < mask[0].0
                || i0_i64 >= mask[0].1
                || i1_i64 < mask[1].0
                || i1_i64 >= mask[1].1
            {
                return None;
            }
        }

        let buf_offset = checked_offset_add(
            checked_offset_add(
                self.offset,
                checked_stride_term(i0, self.strides[0], "2D buffer offset"),
                "2D buffer offset",
            ),
            checked_stride_term(i1, self.strides[1], "2D buffer offset"),
            "2D buffer offset",
        );
        nonnegative_offset_to_usize(buf_offset, "2D buffer offset")
    }

    /// Specialized 3D index computation.
    #[inline(always)]
    fn expr_idx_3d(&self, linear_idx: usize) -> Option<usize> {
        let dim2_size = self.shape[2];
        let dim12_size = self.shape[1]
            .checked_mul(dim2_size)
            .expect("3D linear index divisor overflows usize");

        let i2 = linear_idx % dim2_size;
        let i1 = (linear_idx / dim2_size) % self.shape[1];
        let i0 = linear_idx / dim12_size;

        if let Some(ref mask) = self.mask {
            let i0_i64 = usize_to_i64(i0, "3D logical index");
            let i1_i64 = usize_to_i64(i1, "3D logical index");
            let i2_i64 = usize_to_i64(i2, "3D logical index");
            if i0_i64 < mask[0].0
                || i0_i64 >= mask[0].1
                || i1_i64 < mask[1].0
                || i1_i64 >= mask[1].1
                || i2_i64 < mask[2].0
                || i2_i64 >= mask[2].1
            {
                return None;
            }
        }

        let buf_offset = checked_offset_add(
            checked_offset_add(
                checked_offset_add(
                    self.offset,
                    checked_stride_term(i0, self.strides[0], "3D buffer offset"),
                    "3D buffer offset",
                ),
                checked_stride_term(i1, self.strides[1], "3D buffer offset"),
                "3D buffer offset",
            ),
            checked_stride_term(i2, self.strides[2], "3D buffer offset"),
            "3D buffer offset",
        );
        nonnegative_offset_to_usize(buf_offset, "3D buffer offset")
    }

    /// General N-dimensional index computation (4D+).
    #[cold]
    fn expr_idx_general(&self, linear_idx: usize) -> Option<usize> {
        let indices = self.linear_to_indices(linear_idx);

        // Check mask validity
        if let Some(ref mask) = self.mask {
            for (dim, &(lo, hi)) in mask.iter().enumerate() {
                let idx = usize_to_i64(indices[dim], "N-D logical index");
                if idx < lo || idx >= hi {
                    return None;
                }
            }
        }

        // Compute buffer offset: offset + sum(idx[i] * strides[i])
        let mut buf_offset = self.offset;
        for (dim, &idx) in indices.iter().enumerate() {
            buf_offset = checked_offset_add(
                buf_offset,
                checked_stride_term(idx, self.strides[dim], "N-D buffer offset"),
                "N-D buffer offset",
            );
        }

        nonnegative_offset_to_usize(buf_offset, "N-D buffer offset")
    }
}

/// A stack of views that tracks how a contiguous buffer is accessed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShapeTracker {
    pub views: Vec<View>,
}

impl ShapeTracker {
    /// Create a ShapeTracker for a contiguous buffer with the given shape.
    pub fn contiguous(shape: &[usize]) -> Self {
        Self {
            views: vec![View::contiguous(shape)],
        }
    }

    /// The current (outermost) view.
    #[inline(always)]
    pub fn view(&self) -> &View {
        self.views
            .last()
            .expect("ShapeTracker must have at least one view")
    }

    /// The logical shape of the tensor.
    #[inline(always)]
    pub fn shape(&self) -> &[usize] {
        &self.view().shape
    }

    /// Total number of logical elements.
    #[inline(always)]
    pub fn checked_numel(&self) -> Option<usize> {
        self.view().checked_numel()
    }

    /// Total number of logical elements.
    #[inline(always)]
    pub fn numel(&self) -> usize {
        self.view().numel()
    }

    /// Reshape to a new shape. Same number of elements required.
    /// For Phase 1: only works on contiguous views; inserts contiguous()
    /// fallback otherwise.
    pub fn reshape(&self, new_shape: &[usize]) -> Self {
        let current = self.view();
        let current_numel = current.numel();
        let new_numel =
            checked_shape_numel(new_shape).expect("reshape target element count overflows usize");
        assert_eq!(
            current_numel, new_numel,
            "reshape: element count mismatch ({} vs {})",
            current_numel, new_numel
        );
        if current.is_contiguous() {
            Self {
                views: vec![View::contiguous(new_shape)],
            }
        } else {
            // Phase 1: push a new view (multi-view case)
            let mut views = self.views.clone();
            views.push(View::contiguous(new_shape));
            Self { views }
        }
    }

    /// Permute dimensions.
    pub fn permute(&self, order: &[usize]) -> Self {
        let current = self.view();
        let ndim = current.shape.len();
        assert_eq!(order.len(), ndim, "permute: order length mismatch");

        let new_shape: Vec<usize> = order.iter().map(|&i| current.shape[i]).collect();
        let new_strides: Vec<i64> = order.iter().map(|&i| current.strides[i]).collect();
        let new_mask = current
            .mask
            .as_ref()
            .map(|m| order.iter().map(|&i| m[i]).collect());

        Self {
            views: vec![View::new(new_shape, new_strides, current.offset, new_mask)],
        }
    }

    /// Expand broadcast dimensions. Dimensions with size 1 in the current
    /// shape can be expanded to any size (stride becomes 0).
    pub fn expand(&self, new_shape: &[usize]) -> Self {
        let current = self.view();
        assert_eq!(
            new_shape.len(),
            current.shape.len(),
            "expand: ndim mismatch"
        );

        let mut new_strides = current.strides.clone();
        for (i, (&old, &new)) in current.shape.iter().zip(new_shape.iter()).enumerate() {
            if old == 1 && new != 1 {
                new_strides[i] = 0; // broadcast
            } else {
                assert_eq!(
                    old, new,
                    "expand: can only expand size-1 dims (dim {} is {})",
                    i, old
                );
            }
        }

        Self {
            views: vec![View::new(
                new_shape.to_vec(),
                new_strides,
                current.offset,
                current.mask.clone(),
            )],
        }
    }

    /// Pad tensor with zeros. `padding` is (before, after) pairs per dimension.
    pub fn pad(&self, padding: &[(usize, usize)]) -> Self {
        let current = self.view();
        assert_eq!(padding.len(), current.shape.len(), "pad: ndim mismatch");

        let new_shape: Vec<usize> = current
            .shape
            .iter()
            .zip(padding.iter())
            .map(|(&s, &(before, after))| {
                checked_shape_add(
                    checked_shape_add(s, before, "pad shape"),
                    after,
                    "pad shape",
                )
            })
            .collect();

        // Adjust offset for padding
        let mut new_offset = current.offset;
        for (i, &(before, _)) in padding.iter().enumerate() {
            new_offset = checked_offset_sub(
                new_offset,
                checked_stride_term(before, current.strides[i], "pad offset"),
                "pad offset",
            );
        }

        // Build mask: valid region is [before, before + original_size)
        let new_mask: Vec<(i64, i64)> = current
            .shape
            .iter()
            .zip(padding.iter())
            .map(|(&s, &(before, _))| {
                (
                    usize_to_i64(before, "pad mask"),
                    usize_to_i64(checked_shape_add(before, s, "pad mask"), "pad mask"),
                )
            })
            .collect();

        Self {
            views: vec![View::new(
                new_shape,
                current.strides.clone(),
                new_offset,
                Some(new_mask),
            )],
        }
    }

    /// Shrink: extract a sub-region. `bounds` is (start, end) per dimension.
    pub fn shrink(&self, bounds: &[(usize, usize)]) -> Self {
        let current = self.view();
        assert_eq!(bounds.len(), current.shape.len(), "shrink: ndim mismatch");

        let new_shape: Vec<usize> = current
            .shape
            .iter()
            .zip(bounds.iter())
            .enumerate()
            .map(|(axis, (&dim, &(start, end)))| {
                assert!(
                    end <= dim,
                    "shrink bound end exceeds dimension on axis {axis}: {end} > {dim}"
                );
                checked_shape_sub(end, start, "shrink shape")
            })
            .collect();

        // Adjust offset for shrink start
        let mut new_offset = current.offset;
        for (i, &(start, _)) in bounds.iter().enumerate() {
            new_offset = checked_offset_add(
                new_offset,
                checked_stride_term(start, current.strides[i], "shrink offset"),
                "shrink offset",
            );
        }
        let new_mask = current.mask.as_ref().map(|mask| {
            mask.iter()
                .zip(bounds.iter())
                .zip(new_shape.iter())
                .map(|((&(lo, hi), &(start, _)), &len)| {
                    let start = usize_to_i64(start, "shrink mask");
                    let len = usize_to_i64(len, "shrink mask");
                    (
                        checked_offset_sub(lo, start, "shrink mask").max(0),
                        checked_offset_sub(hi, start, "shrink mask").min(len),
                    )
                })
                .collect()
        });

        Self {
            views: vec![View::new(
                new_shape,
                current.strides.clone(),
                new_offset,
                new_mask,
            )],
        }
    }

    /// Flip a dimension (reverse element order along that axis).
    pub fn flip(&self, axis: usize) -> Self {
        let current = self.view();
        assert!(axis < current.shape.len(), "flip: axis out of bounds");

        let mut new_strides = current.strides.clone();
        new_strides[axis] = new_strides[axis]
            .checked_neg()
            .expect("flip stride negation overflows i64");

        // Adjust offset: flip moves the start pointer to the last element
        let axis_len = usize_to_i64(current.shape[axis], "flip axis length");
        let new_offset = if current.shape[axis] == 0 {
            current.offset
        } else {
            let last_axis_index = checked_shape_sub(current.shape[axis], 1, "flip axis length");
            checked_offset_add(
                current.offset,
                checked_stride_term(last_axis_index, current.strides[axis], "flip offset"),
                "flip offset",
            )
        };
        let new_mask = current.mask.as_ref().map(|mask| {
            let mut flipped_mask = mask.clone();
            let (lo, hi) = flipped_mask[axis];
            flipped_mask[axis] = (
                checked_offset_sub(axis_len, hi, "flip mask"),
                checked_offset_sub(axis_len, lo, "flip mask"),
            );
            flipped_mask
        });

        Self {
            views: vec![View::new(
                current.shape.clone(),
                new_strides,
                new_offset,
                new_mask,
            )],
        }
    }

    /// Compute the buffer offset for a linear logical index.
    /// For single-view ShapeTrackers, delegates directly to the view.
    /// For multi-view, composes views from outer to inner.
    /// Returns None if any view's mask excludes the index (padding).
    #[inline(always)]
    pub fn expr_idx(&self, linear_idx: usize) -> Option<usize> {
        if self.views.len() == 1 {
            return self.views[0].expr_idx(linear_idx);
        }

        // Multi-view: compose from outer to inner.
        let mut idx = linear_idx;
        for view in self.views.iter().rev() {
            match view.expr_idx(idx) {
                Some(next_idx) => idx = next_idx,
                None => return None,
            }
        }
        Some(idx)
    }
}
