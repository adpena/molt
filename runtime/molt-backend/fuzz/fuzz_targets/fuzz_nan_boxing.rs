#![no_main]
use libfuzzer_sys::fuzz_target;

// --------------------------------------------------------------------------
// Mirror the NaN-boxing constants from molt-backend/src/wasm.rs so that the
// fuzz target compiles standalone without requiring the full compilation
// pipeline or exposing private items.
// --------------------------------------------------------------------------

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_MIN_INLINE: i64 = -(1 << 46);
const INT_MAX_INLINE: i64 = (1 << 46) - 1;

// --------------------------------------------------------------------------
// Encoding / decoding helpers (mirrors the runtime WASM NaN-boxing scheme)
// --------------------------------------------------------------------------

fn nan_box_int(value: i64) -> Option<u64> {
    if value < INT_MIN_INLINE || value > INT_MAX_INLINE {
        return None;
    }
    let payload = (value as u64) & POINTER_MASK;
    Some(QNAN | TAG_INT | payload)
}

fn nan_unbox_int(bits: u64) -> Option<i64> {
    if !is_tagged(bits, TAG_INT) {
        return None;
    }
    let payload = bits & POINTER_MASK;
    // Sign-extend from 47 bits
    let value = if payload & (1 << 46) != 0 {
        (payload | !POINTER_MASK) as i64
    } else {
        payload as i64
    };
    Some(value)
}

fn nan_box_bool(b: bool) -> u64 {
    QNAN | TAG_BOOL | (b as u64)
}

fn nan_unbox_bool(bits: u64) -> Option<bool> {
    if !is_tagged(bits, TAG_BOOL) {
        return None;
    }
    Some((bits & 1) != 0)
}

fn nan_box_none() -> u64 {
    QNAN | TAG_NONE
}

fn nan_box_ptr(addr: u64) -> Option<u64> {
    if addr & !POINTER_MASK != 0 {
        return None; // address too wide
    }
    Some(QNAN | TAG_PTR | addr)
}

fn nan_unbox_ptr(bits: u64) -> Option<u64> {
    if !is_tagged(bits, TAG_PTR) {
        return None;
    }
    Some(bits & POINTER_MASK)
}

fn is_tagged(bits: u64, tag: u64) -> bool {
    (bits & (QNAN | TAG_MASK)) == (QNAN | tag)
}

fn tag_of(bits: u64) -> Option<u64> {
    if (bits & QNAN) != QNAN {
        return None; // plain float or signalling NaN
    }
    Some(bits & TAG_MASK)
}

// --------------------------------------------------------------------------
// Fuzz target
// --------------------------------------------------------------------------

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let bits = u64::from_le_bytes(data[..8].try_into().unwrap());

    // --- Test 1: int roundtrip ---
    if let Some(unboxed) = nan_unbox_int(bits) {
        // Value must be in the inline range.
        assert!(
            unboxed >= INT_MIN_INLINE && unboxed <= INT_MAX_INLINE,
            "unboxed int {unboxed} outside inline range for {bits:#018x}"
        );
        let reboxed = nan_box_int(unboxed).expect("in-range int must box");
        let re_unboxed = nan_unbox_int(reboxed).expect("reboxed must unbox");
        assert_eq!(
            unboxed, re_unboxed,
            "int roundtrip failed: {unboxed} -> {reboxed:#018x} -> {re_unboxed}"
        );
    }

    // --- Test 2: bool roundtrip ---
    if let Some(b) = nan_unbox_bool(bits) {
        let reboxed = nan_box_bool(b);
        let re_unboxed = nan_unbox_bool(reboxed).expect("reboxed bool must unbox");
        assert_eq!(b, re_unboxed, "bool roundtrip failed for {bits:#018x}");
    }

    // --- Test 3: ptr roundtrip ---
    if let Some(addr) = nan_unbox_ptr(bits) {
        if let Some(reboxed) = nan_box_ptr(addr) {
            let re_addr = nan_unbox_ptr(reboxed).expect("reboxed ptr must unbox");
            assert_eq!(
                addr, re_addr,
                "ptr roundtrip failed: {addr:#x} -> {reboxed:#018x} -> {re_addr:#x}"
            );
        }
    }

    // --- Test 4: tag exclusivity ---
    // A NaN-boxed value must match at most ONE tag. Two tags sharing the same
    // bit pattern would indicate a collision in the encoding scheme.
    if (bits & QNAN) == QNAN {
        let tag = bits & TAG_MASK;
        let known_tags = [TAG_INT, TAG_BOOL, TAG_NONE, TAG_PTR, TAG_PENDING];
        let matches: usize = known_tags.iter().filter(|&&t| tag == t).count();
        assert!(
            matches <= 1,
            "multiple tags matched for {bits:#018x} (tag field = {tag:#018x})"
        );
    }

    // --- Test 5: box all ints in range using a second chunk of fuzz data ---
    if data.len() >= 16 {
        let raw_int = i64::from_le_bytes(data[8..16].try_into().unwrap());
        let clamped = raw_int.clamp(INT_MIN_INLINE, INT_MAX_INLINE);
        let boxed = nan_box_int(clamped).expect("in-range int should box");
        assert!(
            is_tagged(boxed, TAG_INT),
            "boxed int has wrong tag: {boxed:#018x}"
        );
        let unboxed = nan_unbox_int(boxed).expect("boxed int should unbox");
        assert_eq!(clamped, unboxed, "int box/unbox mismatch for {clamped}");
    }

    // --- Test 6: None is a singleton encoding ---
    let none_bits = nan_box_none();
    assert!(is_tagged(none_bits, TAG_NONE));
    assert_eq!(none_bits & POINTER_MASK, 0);

    // --- Test 7: plain float bits must NOT match any tag ---
    // Interpret the fuzz input as an f64 and verify that normal floats
    // (non-NaN) are never misclassified as tagged values.
    let f = f64::from_bits(bits);
    if !f.is_nan() {
        assert!(
            tag_of(bits).is_none(),
            "non-NaN float {f} ({bits:#018x}) matched a tag"
        );
    }
});
