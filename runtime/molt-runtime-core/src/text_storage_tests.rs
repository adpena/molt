use super::{code_points, push_backslash_bytes, push_code_point};

#[test]
fn storage_preserves_adjacent_surrogate_codepoints() {
    // CPython chr/encode('utf-8', 'surrogatepass') oracle, including every
    // encoding-width boundary and surrogates on both sides of a scalar.
    let points = [
        0, 0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xd800, 0xd83d, 0xde00, 0xdfff, 0xe000, 0xffff,
        0x10000, 0x1f600, 0x10ffff,
    ];
    let expected = [
        0x00, 0x7f, 0xc2, 0x80, 0xdf, 0xbf, 0xe0, 0xa0, 0x80, 0xed, 0x9f, 0xbf, 0xed, 0xa0, 0x80,
        0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80, 0xed, 0xbf, 0xbf, 0xee, 0x80, 0x80, 0xef, 0xbf, 0xbf,
        0xf0, 0x90, 0x80, 0x80, 0xf0, 0x9f, 0x98, 0x80, 0xf4, 0x8f, 0xbf, 0xbf,
    ];
    let mut encoded = Vec::new();
    for point in points {
        push_code_point(&mut encoded, point);
    }
    assert_eq!(encoded, expected);
    assert_eq!(code_points(&expected).collect::<Vec<_>>(), points);
    assert!(std::str::from_utf8(&encoded).is_err());
    let mut empty = code_points(&[]);
    assert_eq!(empty.size_hint(), (0, Some(0)));
    assert_eq!(empty.next(), None);
    assert_eq!(empty.next(), None);
}

#[test]
fn every_unicode_scalar_matches_rust_utf8() {
    let mut actual = Vec::with_capacity(4);
    let mut expected = [0; 4];
    for code in 0..=0x10ffff {
        let Some(scalar) = char::from_u32(code) else {
            continue;
        };
        let expected = scalar.encode_utf8(&mut expected).as_bytes();
        actual.clear();
        push_code_point(&mut actual, code);
        assert_eq!(actual, expected);
        let mut points = code_points(expected);
        assert_eq!(points.clone().next(), Some(code));
        assert_eq!(points.next(), Some(code));
        assert_eq!(points.next(), None);
    }
}

#[test]
fn invalid_ordinal_cannot_publish_partial_storage() {
    let mut out = vec![b'A'];
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            push_code_point(&mut out, 0x110000);
        }))
        .is_err()
    );
    assert_eq!(out, b"A");
    out.clear();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            push_code_point(&mut out, u32::MAX);
        }))
        .is_err()
    );
    assert!(out.is_empty());
}

#[test]
fn truncated_storage_never_becomes_an_unchecked_string() {
    for bytes in [&[0xc2][..], &[0xe0, 0xa0], &[0xf4, 0x8f, 0xbf]] {
        assert!(std::panic::catch_unwind(|| code_points(bytes).next()).is_err());
    }
    let mut out = b"prefix".to_vec();
    push_backslash_bytes(&mut out, &[0, 0x0f, 0x80, 0xff]);
    assert_eq!(out, b"prefix\\x00\\x0f\\x80\\xff");
}
