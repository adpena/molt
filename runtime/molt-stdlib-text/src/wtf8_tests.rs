use super::{from_validated_bytes, push_wtf8_codepoint};

#[test]
fn validated_view_preserves_adjacent_surrogate_codepoints() {
    let points = [0xD83D, 0xDE00, 0x1F600, 0xD800, 0xDFFF];
    let expected_bytes = [
        0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, 0xF0, 0x9F, 0x98, 0x80, 0xED, 0xA0, 0x80, 0xED, 0xBF,
        0xBF,
    ];
    let mut bytes = Vec::new();
    for point in points {
        push_wtf8_codepoint(&mut bytes, point);
    }
    assert_eq!(bytes, expected_bytes);

    // SAFETY: the fixed oracle vector is composed of valid UTF-8 scalar
    // encodings and Molt's three-byte surrogate encodings.
    let decoded: Vec<u32> = unsafe { from_validated_bytes(&expected_bytes) }
        .code_points()
        .map(|point| point.to_u32())
        .collect();
    assert_eq!(decoded, points);
}
