//! ShapeTracker + View — zero-copy view system for movement ops.
//!
//! All movement ops (reshape, permute, expand, pad, shrink, flip) are
//! O(1) modifications to the view. No GPU kernel, no memory copy.
//!
//! Performance optimizations:
//! - `is_contiguous` is cached at construction time (not recomputed per call).
//! - `expr_idx` has a fast path for contiguous views (linear_idx == buffer offset).
//! - Specialized `expr_idx_1d`, `expr_idx_2d`, `expr_idx_3d` avoid the general loop.

/// A single view into a contiguous buffer.
///
/// Describes how to access a region of a flat buffer via shape, strides,
/// offset, and optional validity mask. Movement ops modify views rather
/// than copying data.
#[derive(Debug, Clone, PartialEq)]
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
        let ndim = shape.len();
        let mut strides = vec![0i64; ndim];
        if ndim > 0 {
            strides[ndim - 1] = 1;
            for i in (0..ndim - 1).rev() {
                strides[i] = strides[i + 1] * shape[i + 1] as i64;
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
            expected_stride *= shape[i] as i64;
        }
        true
    }

    /// Total number of logical elements.
    #[inline(always)]
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
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
            let idx_i64 = idx as i64;
            if idx_i64 < mask[0].0 || idx_i64 >= mask[0].1 {
                return None;
            }
        }

        let buf_offset = self.offset + idx as i64 * self.strides[0];
        if buf_offset < 0 {
            return None;
        }
        Some(buf_offset as usize)
    }

    /// Specialized 2D index computation.
    #[inline(always)]
    fn expr_idx_2d(&self, linear_idx: usize) -> Option<usize> {
        let i1 = linear_idx % self.shape[1];
        let i0 = linear_idx / self.shape[1];

        if let Some(ref mask) = self.mask {
            let i0_i64 = i0 as i64;
            let i1_i64 = i1 as i64;
            if i0_i64 < mask[0].0
                || i0_i64 >= mask[0].1
                || i1_i64 < mask[1].0
                || i1_i64 >= mask[1].1
            {
                return None;
            }
        }

        let buf_offset = self.offset + i0 as i64 * self.strides[0] + i1 as i64 * self.strides[1];
        if buf_offset < 0 {
            return None;
        }
        Some(buf_offset as usize)
    }

    /// Specialized 3D index computation.
    #[inline(always)]
    fn expr_idx_3d(&self, linear_idx: usize) -> Option<usize> {
        let dim2_size = self.shape[2];
        let dim12_size = self.shape[1] * dim2_size;

        let i2 = linear_idx % dim2_size;
        let i1 = (linear_idx / dim2_size) % self.shape[1];
        let i0 = linear_idx / dim12_size;

        if let Some(ref mask) = self.mask {
            let i0_i64 = i0 as i64;
            let i1_i64 = i1 as i64;
            let i2_i64 = i2 as i64;
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

        let buf_offset = self.offset
            + i0 as i64 * self.strides[0]
            + i1 as i64 * self.strides[1]
            + i2 as i64 * self.strides[2];
        if buf_offset < 0 {
            return None;
        }
        Some(buf_offset as usize)
    }

    /// General N-dimensional index computation (4D+).
    #[cold]
    fn expr_idx_general(&self, linear_idx: usize) -> Option<usize> {
        let indices = self.linear_to_indices(linear_idx);

        // Check mask validity
        if let Some(ref mask) = self.mask {
            for (dim, &(lo, hi)) in mask.iter().enumerate() {
                let idx = indices[dim] as i64;
                if idx < lo || idx >= hi {
                    return None;
                }
            }
        }

        // Compute buffer offset: offset + sum(idx[i] * strides[i])
        let mut buf_offset = self.offset;
        for (dim, &idx) in indices.iter().enumerate() {
            buf_offset += idx as i64 * self.strides[dim];
        }

        // Buffer offset must be non-negative for valid elements
        if buf_offset < 0 {
            return None;
        }
        Some(buf_offset as usize)
    }
}

/// A stack of views that tracks how a contiguous buffer is accessed.
#[derive(Debug, Clone, PartialEq)]
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
    pub fn numel(&self) -> usize {
        self.view().numel()
    }

    /// Reshape to a new shape. Same number of elements required.
    /// For Phase 1: only works on contiguous views; inserts contiguous()
    /// fallback otherwise.
    pub fn reshape(&self, new_shape: &[usize]) -> Self {
        let current = self.view();
        assert_eq!(
            current.numel(),
            new_shape.iter().product::<usize>(),
            "reshape: element count mismatch ({} vs {})",
            current.numel(),
            new_shape.iter().product::<usize>()
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
            .map(|(&s, &(before, after))| s + before + after)
            .collect();

        // Adjust offset for padding
        let mut new_offset = current.offset;
        for (i, &(before, _)) in padding.iter().enumerate() {
            new_offset -= before as i64 * current.strides[i];
        }

        // Build mask: valid region is [before, before + original_size)
        let new_mask: Vec<(i64, i64)> = current
            .shape
            .iter()
            .zip(padding.iter())
            .map(|(&s, &(before, _))| (before as i64, (before + s) as i64))
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

        let new_shape: Vec<usize> = bounds.iter().map(|&(s, e)| e - s).collect();

        // Adjust offset for shrink start
        let mut new_offset = current.offset;
        for (i, &(start, _)) in bounds.iter().enumerate() {
            new_offset += start as i64 * current.strides[i];
        }

        Self {
            views: vec![View::new(
                new_shape,
                current.strides.clone(),
                new_offset,
                current.mask.clone(),
            )],
        }
    }

    /// Flip a dimension (reverse element order along that axis).
    pub fn flip(&self, axis: usize) -> Self {
        let current = self.view();
        assert!(axis < current.shape.len(), "flip: axis out of bounds");

        let mut new_strides = current.strides.clone();
        new_strides[axis] = -new_strides[axis];

        // Adjust offset: flip moves the start pointer to the last element
        let new_offset = current.offset + (current.shape[axis] as i64 - 1) * current.strides[axis];

        Self {
            views: vec![View::new(
                current.shape.clone(),
                new_strides,
                new_offset,
                current.mask.clone(),
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
