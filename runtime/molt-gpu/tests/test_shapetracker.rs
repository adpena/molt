use molt_gpu::shapetracker::{ShapeTracker, View};

#[test]
fn test_contiguous_view() {
    let v = View::contiguous(&[2, 3, 4]);
    assert_eq!(v.shape, vec![2, 3, 4]);
    assert_eq!(v.strides, vec![12, 4, 1]);
    assert_eq!(v.offset, 0);
    assert!(v.is_contiguous());
    assert_eq!(v.checked_numel(), Some(24));
    assert_eq!(v.numel(), 24);
}

#[test]
fn test_checked_numel_reports_overflow() {
    let huge_dim = (u32::MAX as usize) + 1;
    let v = View::contiguous(&[huge_dim, huge_dim]);
    let st = ShapeTracker::contiguous(&[huge_dim, huge_dim]);

    assert_eq!(v.checked_numel(), None);
    assert_eq!(st.checked_numel(), None);
}

#[test]
#[should_panic(expected = "shape logical element count overflows usize")]
fn test_numel_panics_on_overflow() {
    let huge_dim = (u32::MAX as usize) + 1;
    let v = View::contiguous(&[huge_dim, huge_dim]);
    let _ = v.numel();
}

#[test]
#[should_panic(expected = "row-major stride overflows i64")]
fn test_contiguous_rejects_i64_stride_overflow() {
    let dim = 3_037_000_500usize;
    let _ = View::contiguous(&[2, dim, dim]);
}

#[test]
#[should_panic(expected = "contiguous shape dimension exceeds i64 capacity")]
fn test_contiguous_rejects_i64_coordinate_overflow() {
    let _ = View::contiguous(&[usize::MAX]);
}

#[test]
#[should_panic(expected = "reshape target element count overflows usize")]
fn test_reshape_rejects_overflowing_target_numel() {
    let huge_dim = (u32::MAX as usize) + 1;
    let st = ShapeTracker::contiguous(&[1]);
    let _ = st.reshape(&[huge_dim, huge_dim]);
}

#[test]
#[should_panic(expected = "view shape dimension exceeds i64 capacity")]
fn test_expand_rejects_i64_coordinate_overflow() {
    let st = ShapeTracker::contiguous(&[1]);
    let _ = st.expand(&[usize::MAX]);
}

#[test]
#[should_panic(expected = "pad shape overflows usize")]
fn test_pad_rejects_shape_overflow() {
    let st = ShapeTracker::contiguous(&[1]);
    let _ = st.pad(&[(usize::MAX, 1)]);
}

#[test]
#[should_panic(expected = "shrink shape underflows usize")]
fn test_shrink_rejects_inverted_bounds() {
    let st = ShapeTracker::contiguous(&[4]);
    let _ = st.shrink(&[(3, 2)]);
}

#[test]
fn test_expr_idx_contiguous() {
    let v = View::contiguous(&[2, 3]);
    assert_eq!(v.expr_idx(0), Some(0));
    assert_eq!(v.expr_idx(1), Some(1));
    assert_eq!(v.expr_idx(3), Some(3));
    assert_eq!(v.expr_idx(5), Some(5));
}

#[test]
fn test_reshape() {
    let st = ShapeTracker::contiguous(&[2, 3]);
    let reshaped = st.reshape(&[3, 2]);
    assert_eq!(reshaped.shape(), &[3, 2]);
    assert_eq!(reshaped.numel(), 6);
}

#[test]
fn test_permute() {
    let st = ShapeTracker::contiguous(&[2, 3, 4]);
    let perm = st.permute(&[2, 0, 1]);
    assert_eq!(perm.shape(), &[4, 2, 3]);
    assert_eq!(perm.view().strides, vec![1, 12, 4]);
}

#[test]
fn test_expand() {
    let st = ShapeTracker::contiguous(&[1, 3]);
    let expanded = st.expand(&[4, 3]);
    assert_eq!(expanded.shape(), &[4, 3]);
    assert_eq!(expanded.view().strides, vec![0, 1]);
    assert_eq!(expanded.expr_idx(0), Some(0));
    assert_eq!(expanded.expr_idx(3), Some(0));
    assert_eq!(expanded.expr_idx(4), Some(1));
}

#[test]
fn test_expand_inner_axis_reuses_source_offsets() {
    let st = ShapeTracker::contiguous(&[2, 1]);
    let expanded = st.expand(&[2, 3]);
    let offsets: Vec<Option<usize>> = (0..expanded.numel())
        .map(|idx| expanded.expr_idx(idx))
        .collect();

    assert_eq!(expanded.shape(), &[2, 3]);
    assert_eq!(expanded.view().strides, vec![1, 0]);
    assert_eq!(
        offsets,
        vec![Some(0), Some(0), Some(0), Some(1), Some(1), Some(1)]
    );
}

#[test]
fn test_pad() {
    let st = ShapeTracker::contiguous(&[3]);
    let padded = st.pad(&[(1, 2)]);
    assert_eq!(padded.shape(), &[6]);
    assert_eq!(padded.expr_idx(0), None);
    assert_eq!(padded.expr_idx(1), Some(0));
    assert_eq!(padded.expr_idx(2), Some(1));
    assert_eq!(padded.expr_idx(3), Some(2));
    assert_eq!(padded.expr_idx(4), None);
    assert_eq!(padded.expr_idx(5), None);
}

#[test]
fn test_shrink() {
    let st = ShapeTracker::contiguous(&[5]);
    let shrunk = st.shrink(&[(1, 4)]);
    assert_eq!(shrunk.shape(), &[3]);
    assert_eq!(shrunk.expr_idx(0), Some(1));
    assert_eq!(shrunk.expr_idx(1), Some(2));
    assert_eq!(shrunk.expr_idx(2), Some(3));
}

#[test]
#[should_panic(expected = "shrink bound end exceeds dimension")]
fn test_shrink_rejects_end_past_dimension() {
    let st = ShapeTracker::contiguous(&[4]);
    let _ = st.shrink(&[(0, 5)]);
}

#[test]
fn test_flip() {
    let st = ShapeTracker::contiguous(&[4]);
    let flipped = st.flip(0);
    assert_eq!(flipped.shape(), &[4]);
    assert_eq!(flipped.expr_idx(0), Some(3));
    assert_eq!(flipped.expr_idx(1), Some(2));
    assert_eq!(flipped.expr_idx(2), Some(1));
    assert_eq!(flipped.expr_idx(3), Some(0));
}

#[test]
fn test_flip_zero_length_axis_keeps_empty_offset() {
    let st = ShapeTracker::contiguous(&[0]);
    let flipped = st.flip(0);

    assert_eq!(flipped.shape(), &[0]);
    assert_eq!(flipped.view().offset, 0);
    assert_eq!(flipped.view().strides, vec![-1]);
    assert_eq!(flipped.numel(), 0);
}

#[test]
fn test_flip_transforms_pad_mask_coordinates() {
    let st = ShapeTracker::contiguous(&[5]);
    let padded_flipped = st.pad(&[(1, 2)]).flip(0);
    assert_eq!(padded_flipped.shape(), &[8]);
    assert_eq!(padded_flipped.expr_idx(0), None);
    assert_eq!(padded_flipped.expr_idx(1), None);
    assert_eq!(padded_flipped.expr_idx(2), Some(4));
    assert_eq!(padded_flipped.expr_idx(3), Some(3));
    assert_eq!(padded_flipped.expr_idx(4), Some(2));
    assert_eq!(padded_flipped.expr_idx(5), Some(1));
    assert_eq!(padded_flipped.expr_idx(6), Some(0));
    assert_eq!(padded_flipped.expr_idx(7), None);
}

#[test]
fn test_shrink_transforms_pad_mask_coordinates() {
    let st = ShapeTracker::contiguous(&[3]);
    let view = st.pad(&[(1, 2)]).shrink(&[(1, 4)]).flip(0);
    let offsets: Vec<Option<usize>> = (0..view.numel()).map(|idx| view.expr_idx(idx)).collect();

    assert_eq!(view.shape(), &[3]);
    assert_eq!(offsets, vec![Some(2), Some(1), Some(0)]);
}

#[test]
fn test_2d_pad() {
    let st = ShapeTracker::contiguous(&[2, 3]);
    let padded = st.pad(&[(1, 0), (0, 1)]);
    assert_eq!(padded.shape(), &[3, 4]);
    assert_eq!(padded.expr_idx(0), None);
    assert_eq!(padded.expr_idx(4), Some(0));
    assert_eq!(padded.expr_idx(7), None);
}

#[test]
fn test_transpose_via_permute() {
    let st = ShapeTracker::contiguous(&[3, 4]);
    let transposed = st.permute(&[1, 0]);
    assert_eq!(transposed.shape(), &[4, 3]);
    assert_eq!(transposed.expr_idx(0), Some(0));
    assert_eq!(transposed.expr_idx(1), Some(4));
    assert_eq!(transposed.expr_idx(3), Some(1));
}
