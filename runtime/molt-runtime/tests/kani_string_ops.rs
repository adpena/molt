//! Kani bounded-verification harnesses for string/byte-slice helper logic.
//!
//! The Molt runtime uses raw byte slices (`&[u8]`) for string payloads with
//! UTF-8 encoding.  These harnesses verify safety invariants of pure helper
//! functions that can be modeled without the full runtime (GIL, allocator, etc).
//!
//! Run with: `cd runtime/molt-runtime && cargo kani --tests`

#[cfg(kani)]
mod string_ops_proofs {
    // ---------------------------------------------------------------
    // Model of byte-level find (mirrors memchr-style single-byte search
    // used in molt_string_find when the needle is a single byte).
    // ---------------------------------------------------------------

    /// Find the first occurrence of `needle` in `haystack[start..end]`.
    /// Returns Some(index) relative to start of haystack, or None.
    fn byte_find(haystack: &[u8], needle: u8, start: usize, end: usize) -> Option<usize> {
        if start > haystack.len() || end > haystack.len() || start > end {
            return None;
        }
        let mut i = start;
        while i < end {
            if haystack[i] == needle {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    // ---------------------------------------------------------------
    // Model of slice bounds clamping (mirrors Python-style clamping for
    // string[start:stop] where negative indices are resolved upstream).
    // ---------------------------------------------------------------

    /// Clamp a start..stop range to [0, len].
    fn clamp_range(start: i64, stop: i64, len: usize) -> (usize, usize) {
        let len_i = len as i64;
        let s = start.max(0).min(len_i) as usize;
        let e = stop.max(0).min(len_i) as usize;
        if s > e { (s, s) } else { (s, e) }
    }

    // ---------------------------------------------------------------
    // Model of UTF-8 char boundary check (mirrors the standard library
    // `is_char_boundary` invariant that Molt relies on).
    // ---------------------------------------------------------------

    /// A byte is a UTF-8 char boundary if it is NOT a continuation byte
    /// (i.e. not in the range 0x80..0xBF, or equivalently top 2 bits != 10).
    fn is_utf8_char_boundary(b: u8) -> bool {
        // This matches std::str::is_char_boundary logic for a single byte.
        (b as i8) >= -0x40 // equivalent to b < 128 || b >= 192
    }

    // ===============================================================
    // 1. byte_find INVARIANTS
    // ===============================================================

    /// byte_find returns None when start == end (empty search window).
    #[kani::proof]
    #[kani::unwind(1)]
    fn byte_find_empty_window() {
        let buf: [u8; 4] = [kani::any(), kani::any(), kani::any(), kani::any()];
        let needle: u8 = kani::any();
        let start: usize = kani::any();
        kani::assume(start <= 4);
        assert_eq!(byte_find(&buf, needle, start, start), None);
    }

    /// If byte_find returns Some(idx), then haystack[idx] == needle.
    #[kani::proof]
    #[kani::unwind(5)]
    fn byte_find_result_is_correct() {
        let buf: [u8; 4] = [kani::any(), kani::any(), kani::any(), kani::any()];
        let needle: u8 = kani::any();
        let start: usize = kani::any();
        let end: usize = kani::any();
        kani::assume(start <= 4 && end <= 4);
        if let Some(idx) = byte_find(&buf, needle, start, end) {
            assert_eq!(buf[idx], needle);
            assert!(idx >= start && idx < end);
        }
    }

    /// If byte_find returns Some(idx), there is no earlier match in [start..idx).
    #[kani::proof]
    #[kani::unwind(5)]
    fn byte_find_returns_first_match() {
        let buf: [u8; 4] = [kani::any(), kani::any(), kani::any(), kani::any()];
        let needle: u8 = kani::any();
        let start: usize = kani::any();
        let end: usize = kani::any();
        kani::assume(start <= 4 && end <= 4);
        if let Some(idx) = byte_find(&buf, needle, start, end) {
            let mut k = start;
            while k < idx {
                assert_ne!(buf[k], needle);
                k += 1;
            }
        }
    }

    /// byte_find with out-of-bounds indices returns None (no panic).
    #[kani::proof]
    #[kani::unwind(1)]
    fn byte_find_oob_returns_none() {
        let buf: [u8; 4] = [0; 4];
        let needle: u8 = kani::any();
        // start > end
        assert_eq!(byte_find(&buf, needle, 3, 1), None);
        // start > len
        assert_eq!(byte_find(&buf, needle, 5, 6), None);
    }

    // ===============================================================
    // 2. clamp_range INVARIANTS
    // ===============================================================

    /// Clamped range always satisfies 0 <= start <= stop <= len.
    #[kani::proof]
    #[kani::unwind(1)]
    fn clamp_range_bounded() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        let len: usize = kani::any();
        // Bound len to keep state space small.
        kani::assume(len <= 256);
        kani::assume(start >= -256 && start <= 256);
        kani::assume(stop >= -256 && stop <= 256);

        let (s, e) = clamp_range(start, stop, len);
        assert!(s <= e);
        assert!(e <= len);
    }

    /// Negative start clamps to 0.
    #[kani::proof]
    #[kani::unwind(1)]
    fn clamp_range_negative_start() {
        let start: i64 = kani::any();
        kani::assume(start < 0);
        let stop: i64 = kani::any();
        kani::assume(stop >= 0 && stop <= 100);

        let (s, _e) = clamp_range(start, stop, 100);
        assert_eq!(s, 0);
    }

    /// Stop beyond len clamps to len.
    #[kani::proof]
    #[kani::unwind(1)]
    fn clamp_range_stop_beyond_len() {
        let len: usize = kani::any();
        kani::assume(len > 0 && len <= 256);
        let stop = (len as i64) + 10;

        let (_s, e) = clamp_range(0, stop, len);
        assert_eq!(e, len);
    }

    /// When start > stop, the result range is empty (s == e).
    #[kani::proof]
    #[kani::unwind(1)]
    fn clamp_range_inverted_is_empty() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        kani::assume(start >= 0 && start <= 100);
        kani::assume(stop >= 0 && stop <= 100);
        kani::assume(start > stop);

        let (s, e) = clamp_range(start, stop, 100);
        assert_eq!(s, e);
    }

    // ===============================================================
    // 3. UTF-8 BOUNDARY CHECK
    // ===============================================================

    /// ASCII bytes (0x00..0x7F) are always char boundaries.
    #[kani::proof]
    #[kani::unwind(1)]
    fn ascii_is_char_boundary() {
        let b: u8 = kani::any();
        kani::assume(b < 0x80);
        assert!(is_utf8_char_boundary(b));
    }

    /// Continuation bytes (0x80..0xBF) are NOT char boundaries.
    #[kani::proof]
    #[kani::unwind(1)]
    fn continuation_byte_not_boundary() {
        let b: u8 = kani::any();
        kani::assume(b >= 0x80 && b <= 0xBF);
        assert!(!is_utf8_char_boundary(b));
    }

    /// Leading multi-byte bytes (0xC0..0xFF) are char boundaries.
    #[kani::proof]
    #[kani::unwind(1)]
    fn leading_byte_is_boundary() {
        let b: u8 = kani::any();
        kani::assume(b >= 0xC0);
        assert!(is_utf8_char_boundary(b));
    }

    /// is_utf8_char_boundary agrees with the standard library on all bytes.
    #[kani::proof]
    #[kani::unwind(1)]
    fn char_boundary_matches_std() {
        let b: u8 = kani::any();
        // std::str uses: b < 128 || b >= 192 (i.e. not a continuation byte).
        let std_result = b < 128 || b >= 192;
        assert_eq!(is_utf8_char_boundary(b), std_result);
    }

    // ===============================================================
    // 4. UTF-8 VALIDITY PRESERVATION
    // ===============================================================

    /// Slicing valid UTF-8 at char boundaries preserves UTF-8 validity.
    /// We test with a small 4-byte buffer containing a known-valid ASCII string.
    #[kani::proof]
    #[kani::unwind(5)]
    fn ascii_slice_preserves_utf8() {
        // All ASCII bytes form valid UTF-8.
        let b0: u8 = kani::any();
        let b1: u8 = kani::any();
        let b2: u8 = kani::any();
        let b3: u8 = kani::any();
        kani::assume(b0 < 0x80 && b1 < 0x80 && b2 < 0x80 && b3 < 0x80);

        let buf = [b0, b1, b2, b3];
        let start: usize = kani::any();
        let end: usize = kani::any();
        kani::assume(start <= end && end <= 4);

        // For all-ASCII bytes, every index is a char boundary, so any
        // sub-slice is valid UTF-8.
        let slice = &buf[start..end];
        assert!(std::str::from_utf8(slice).is_ok());
    }
}
