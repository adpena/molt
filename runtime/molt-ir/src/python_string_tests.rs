use crate::python_string::PythonString;

#[test]
fn python_text_preserves_compact_bytes_and_distinct_surrogate_code_points() {
    let bytes = [
        b'a', 0, 0xc3, 0xa9, 0xed, 0xa0, 0x80, 0xed, 0xbf, 0xbf, 0xf0, 0x9f, 0x98, 0x80,
    ];
    let text = PythonString::from_utf8_surrogatepass(&bytes).unwrap();
    assert_eq!(text.as_surrogatepass_bytes(), bytes);
    assert_eq!(text.len(), 6);
    assert_eq!(
        text.code_points().collect::<Vec<_>>(),
        [0x61, 0, 0xe9, 0xd800, 0xdfff, 0x1f600]
    );
    assert_eq!(
        text.code_points().rev().collect::<Vec<_>>(),
        [0x1f600, 0xdfff, 0xd800, 0xe9, 0, 0x61]
    );
    assert_eq!(text.to_utf8(), Err(3));
    assert_eq!(text.repr(), "'a\\x00é\\ud800\\udfff😀'");
    assert_eq!(
        PythonString::from("ascii").as_surrogatepass_bytes().len(),
        5
    );
    assert_eq!(PythonString::from("é😀").as_surrogatepass_bytes().len(), 6);
    assert_eq!(
        PythonString::from_code_points(&[0xd800, 0xdfff]).as_surrogatepass_bytes(),
        &[0xed, 0xa0, 0x80, 0xed, 0xbf, 0xbf]
    );
}

#[test]
fn python_text_constructors_reject_malformed_utf8_without_replacement() {
    for bytes in [
        vec![0x80],
        vec![0xc0, 0x80],
        vec![0xe0, 0x80, 0x80],
        vec![0xf0, 0x80, 0x80, 0x80],
        vec![0xf4, 0x90, 0x80, 0x80],
        vec![0xe2, 0x82],
        vec![0xed, b'x', 0x80],
    ] {
        assert_eq!(PythonString::from_utf8_surrogatepass(&bytes), Err(0));
        let op = crate::OpIR {
            kind: "const_str".into(),
            bytes: Some(bytes.clone()),
            out: Some("s".into()),
            ..crate::OpIR::default()
        };
        assert!(crate::literal_payload::validate_simple_literal(&op).is_err());
        let raw = crate::OpIR {
            kind: "const_bytes".into(),
            ..op
        };
        assert!(crate::literal_payload::validate_simple_literal(&raw).is_ok());
    }
}

#[test]
fn python_text_operations_share_code_point_semantics() {
    let high = PythonString::from_code_point(0xd800);
    let low = PythonString::from_code_point(0xdc00);
    let pair = high.concat(&low);
    assert_eq!(pair.len(), 2);
    assert!(pair.contains(&low));
    assert!(pair.contains(&PythonString::new()));
    assert_eq!(pair.repeat(2).len(), 4);
    assert!(high < low);
    assert!(low < PythonString::from_code_point(0x10000));
    assert_eq!(PythonString::from("can't").repr(), "\"can't\"");
    assert_eq!(PythonString::from("\n\\'\"").repr(), "'\\n\\\\\\'\"'");
}
