#[inline(always)]
pub fn usize_from_bits(bits: u64) -> Option<usize> {
    usize::try_from(bits).ok()
}

#[cfg(test)]
mod tests {
    use super::usize_from_bits;

    #[test]
    fn usize_bits_conversion_preserves_the_target_width() {
        let value = usize::MAX;
        assert_eq!(usize_from_bits(value as u64), Some(value));
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn usize_bits_conversion_rejects_truncation() {
        assert_eq!(usize_from_bits(u64::from(u32::MAX) + 1), None);
    }
}
