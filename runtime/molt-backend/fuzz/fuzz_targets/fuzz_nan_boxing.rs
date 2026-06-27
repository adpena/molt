#![no_main]
use libfuzzer_sys::fuzz_target;
use molt_codegen_abi::{
    INT_MAX_INLINE, INT_MIN_INLINE, POINTER_MASK, QNAN, TAG_BOOL, TAG_INT, TAG_MASK, TAG_NONE,
    TAG_PENDING, TAG_PTR, box_bool_bits, box_int_bits, box_none_bits, box_ptr_bits,
    fits_inline_int, is_bool_bits, is_float_bits, is_int_bits, is_ptr_bits, ptr_payload_bits,
    tag_bits, unbox_bool_bits, unbox_inline_int_bits,
};

// --------------------------------------------------------------------------
// Encoding / decoding helpers (mirrors the runtime WASM NaN-boxing scheme)
// --------------------------------------------------------------------------

fn nan_box_int(value: i64) -> Option<u64> {
    if !fits_inline_int(value) {
        return None;
    }
    Some(box_int_bits(value) as u64)
}

fn nan_unbox_int(bits: u64) -> Option<i64> {
    if !is_int_bits(bits) {
        return None;
    }
    // Mirror the WASM codegen's approach: (val << 17) >> 17 (arithmetic shift)
    // This correctly sign-extends from bit 46 and discards any stray upper bits.
    let value = unbox_inline_int_bits(bits);
    if value < INT_MIN_INLINE || value > INT_MAX_INLINE {
        return None; // invalid payload — bits above the 47-bit field are set
    }
    Some(value)
}

fn nan_box_bool(b: bool) -> u64 {
    box_bool_bits(i64::from(b)) as u64
}

fn nan_unbox_bool(bits: u64) -> Option<bool> {
    if !is_bool_bits(bits) {
        return None;
    }
    Some(unbox_bool_bits(bits) != 0)
}

fn nan_box_none() -> u64 {
    box_none_bits() as u64
}

fn nan_box_ptr(addr: u64) -> Option<u64> {
    if addr & !POINTER_MASK != 0 {
        return None; // address too wide
    }
    Some(box_ptr_bits(addr) as u64)
}

fn nan_unbox_ptr(bits: u64) -> Option<u64> {
    if !is_ptr_bits(bits) {
        return None;
    }
    Some(ptr_payload_bits(bits))
}

fn is_tagged(bits: u64, tag: u64) -> bool {
    tag_bits(bits) == (QNAN | tag)
}

fn tag_of(bits: u64) -> Option<u64> {
    if is_float_bits(bits) {
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
