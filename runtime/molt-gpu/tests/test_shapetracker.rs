use molt_gpu::shapetracker::{ShapeTracker, View};

#[test]
fn test_contiguous_view() {
    let v = View::contiguous(&[2, 3, 4]);
    assert_eq!(v.shape, vec![2, 3, 4]);
    assert_eq!(v.strides, vec![12, 4, 1]);
    assert_eq!(v.offset, 0);
    assert!(v.is_contiguous());
    assert_eq!(v.numel(), 24);
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
