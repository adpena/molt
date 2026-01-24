#[inline]
pub(crate) fn usize_from_bits(bits: u64) -> usize {
    debug_assert!(bits <= usize::MAX as u64);
    bits as usize
}
