//! Multi-representation string types for Project TITAN Phase 0.
//!
//! # Overview
//!
//! Rather than storing every Python string as a heap-allocated UTF-8 `String`,
//! Molt selects the most compact storage based on the string's content and
//! length at creation time:
//!
//! | Variant    | When used                                | Storage        |
//! |------------|------------------------------------------|----------------|
//! | `Inline`   | len ≤ 23 bytes                           | in object body |
//! | `OneByte`  | ASCII-only, len > 23                     | heap `u8[]`    |
//! | `TwoByte`  | BMP-only (U+0000–U+FFFF), len > 23      | heap `u16[]`   |
//! | `General`  | supplementary chars present, len > 23    | heap UTF-8     |
//! | `Interned` | pointer into intern pool (see string_intern.rs) | `&'static str` |
//!
//! `Inline` (also called SSO — Small String Optimisation) covers roughly 80% of
//! Python strings in typical programs. It requires zero heap allocation.

// ---------------------------------------------------------------------------
// StringReprKind
// ---------------------------------------------------------------------------

/// Discriminant for each string storage strategy.
///
/// The explicit `u8` representation keeps the enum 1 byte so it can be packed
/// into tight object headers without padding waste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StringReprKind {
    /// ≤ 23 bytes — stored directly in the object body, zero heap allocation.
    Inline = 0,
    /// ASCII-only content, heap-allocated as a `u8` array (1 byte/char).
    OneByte = 1,
    /// BMP-only content (U+0000–U+FFFF), heap-allocated as a `u16` array.
    TwoByte = 2,
    /// General UTF-8 fallback (supplementary characters present).
    General = 3,
    /// Pointer into the global intern pool (`string_intern::intern`).
    Interned = 4,
}

// ---------------------------------------------------------------------------
// InlineString
// ---------------------------------------------------------------------------

/// A small string stored entirely inline — no heap allocation.
///
/// The struct is exactly **24 bytes**: one byte for the length followed by
/// 23 bytes of character data.  This matches a typical cache-line-friendly
/// 24-byte object slot.
///
/// # Invariants
///
/// * `len as usize <= 23`
/// * `data[..len]` is valid UTF-8.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InlineString {
    /// Number of valid bytes in `data`. Always ≤ 23.
    pub len: u8,
    /// Raw byte storage. Only `data[..len]` is meaningful.
    pub data: [u8; 23],
}

// Compile-time assertion: InlineString must be exactly 24 bytes.
const _: () = assert!(
    std::mem::size_of::<InlineString>() == 24,
    "InlineString must be exactly 24 bytes"
);

impl InlineString {
    /// Try to create an `InlineString` from `s`.
    ///
    /// Returns `None` if `s.len() > 23` (string too long for inline storage).
    /// The string does **not** need to be ASCII; any UTF-8 content whose byte
    /// length fits in 23 bytes is accepted.
    #[inline]
    pub fn try_new(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        let n = bytes.len();
        if n > 23 {
            return None;
        }
        let mut data = [0u8; 23];
        data[..n].copy_from_slice(bytes);
        Some(InlineString {
            len: n as u8,
            data,
        })
    }

    /// Return the stored string as a `&str`.
    ///
    /// # Safety (internal)
    ///
    /// Safe because `try_new` only accepts valid UTF-8 and we only copy
    /// `bytes[..n]` which preserves that invariant.
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: `data[..len]` was copied from a valid UTF-8 `&str`.
        unsafe { std::str::from_utf8_unchecked(&self.data[..self.len as usize]) }
    }

    /// Return the raw bytes of the stored string.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

impl std::fmt::Debug for InlineString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineString")
            .field("len", &self.len)
            .field("str", &self.as_str())
            .finish()
    }
}

impl PartialEq for InlineString {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for InlineString {}

// ---------------------------------------------------------------------------
// classify_string
// ---------------------------------------------------------------------------

/// Choose the best `StringReprKind` for the given string content.
///
/// This function examines **content and byte length** only; it does not check
/// whether the string is interned (callers should check the intern pool
/// separately if desired).
///
/// Decision tree:
/// 1. `len ≤ 23` → [`StringReprKind::Inline`]
/// 2. All code-points are ASCII (U+0000–U+007F) → [`StringReprKind::OneByte`]
/// 3. All code-points are BMP (U+0000–U+FFFF) → [`StringReprKind::TwoByte`]
/// 4. Otherwise → [`StringReprKind::General`]
#[inline]
pub fn classify_string(s: &str) -> StringReprKind {
    // Step 1: SSO — fits inline.
    if s.len() <= 23 {
        return StringReprKind::Inline;
    }

    // Step 2: ASCII-only check (byte-level — fast).
    if s.is_ascii() {
        return StringReprKind::OneByte;
    }

    // Step 3: BMP-only — every code-point fits in a u16.
    if s.chars().all(|c| (c as u32) <= 0xFFFF) {
        return StringReprKind::TwoByte;
    }

    // Step 4: Supplementary characters present.
    StringReprKind::General
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- InlineString tests ---

    #[test]
    fn inline_try_new_short_string_succeeds() {
        let s = "hello";
        let inline = InlineString::try_new(s);
        assert!(inline.is_some(), "short string should fit inline");
    }

    #[test]
    fn inline_try_new_exactly_23_bytes_succeeds() {
        // Exactly 23 ASCII bytes — the maximum that fits.
        let s = "a".repeat(23);
        let inline = InlineString::try_new(&s);
        assert!(inline.is_some(), "23-byte string should fit inline");
    }

    #[test]
    fn inline_try_new_24_byte_string_returns_none() {
        let s = "a".repeat(24);
        let inline = InlineString::try_new(&s);
        assert!(inline.is_none(), "24-byte string must not fit inline");
    }

    #[test]
    fn inline_round_trip() {
        let original = "Hello, World!";
        let inline = InlineString::try_new(original).expect("should fit");
        assert_eq!(inline.as_str(), original, "round-trip must preserve content");
    }

    #[test]
    fn inline_round_trip_unicode() {
        // "café" in UTF-8 is 5 bytes — fits inline.
        let original = "café";
        let inline = InlineString::try_new(original).expect("should fit");
        assert_eq!(inline.as_str(), original);
    }

    #[test]
    fn inline_as_bytes_correct() {
        let s = "abc";
        let inline = InlineString::try_new(s).unwrap();
        assert_eq!(inline.as_bytes(), b"abc");
    }

    #[test]
    fn inline_empty_string() {
        let inline = InlineString::try_new("").expect("empty string should fit");
        assert_eq!(inline.len, 0);
        assert_eq!(inline.as_str(), "");
    }

    // --- classify_string tests ---

    #[test]
    fn classify_empty_string_is_inline() {
        assert_eq!(classify_string(""), StringReprKind::Inline);
    }

    #[test]
    fn classify_short_ascii_is_inline() {
        assert_eq!(classify_string("hello"), StringReprKind::Inline);
    }

    #[test]
    fn classify_exactly_23_bytes_is_inline() {
        let s = "a".repeat(23);
        assert_eq!(classify_string(&s), StringReprKind::Inline);
    }

    #[test]
    fn classify_30_byte_ascii_is_one_byte() {
        // 30 ASCII characters — exceeds SSO threshold, all ASCII.
        let s = "a".repeat(30);
        assert_eq!(classify_string(&s), StringReprKind::OneByte);
    }

    #[test]
    fn classify_long_ascii_path_is_one_byte() {
        let s = "/usr/local/bin/python3.12/site-packages";
        // len = 38, all ASCII
        assert!(s.len() > 23 && s.is_ascii());
        assert_eq!(classify_string(s), StringReprKind::OneByte);
    }

    #[test]
    fn classify_bmp_chars_is_two_byte() {
        // U+03B1 (α) is BMP; repeat enough times to exceed 23 bytes.
        // "α" is 2 bytes in UTF-8, so 12 repetitions = 24 bytes, len > 23.
        let s = "α".repeat(12); // 24 bytes, all BMP
        assert!(s.len() > 23);
        assert!(!s.is_ascii());
        assert!(s.chars().all(|c| (c as u32) <= 0xFFFF));
        assert_eq!(classify_string(&s), StringReprKind::TwoByte);
    }

    #[test]
    fn classify_supplementary_chars_is_general() {
        // U+1F600 (😀) is a supplementary character (> U+FFFF).
        // Each emoji is 4 bytes in UTF-8; 6 repetitions = 24 bytes.
        let s = "😀".repeat(6); // 24 bytes, supplementary chars
        assert!(s.len() > 23);
        assert!(s.chars().any(|c| (c as u32) > 0xFFFF));
        assert_eq!(classify_string(&s), StringReprKind::General);
    }

    #[test]
    fn classify_mixed_bmp_and_supplementary_is_general() {
        // Mix of ASCII and emoji pushes to General once len > 23.
        let s = format!("{}{}", "hello world! ", "😀😀😀");
        assert!(s.len() > 23);
        assert_eq!(classify_string(&s), StringReprKind::General);
    }
}
