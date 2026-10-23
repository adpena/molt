use super::{CaseOperation, UnicodeData};

#[test]
fn target_facts_are_independent_and_preserve_unicode_domains() {
    for (minor, version, new_symbol, new_upper, old_lower) in [
        (15, "17.0.0", true, true, false),
        (12, "15.0.0", false, false, true),
        (14, "16.0.0", true, true, true),
        (13, "15.1.0", true, false, true),
    ] {
        let data = UnicodeData::for_version((3, minor)).unwrap();
        assert_eq!(data.version, version);
        assert_eq!(data.is_printable(0x2ffc), new_symbol);
        assert_eq!(data.is_upper(0x1c89), new_upper);
        assert_eq!(data.is_lower(0x0295), old_lower);
        assert_eq!(data.category(0xd800), "Cs");
        assert!(!data.is_printable(0xd800));
        assert_eq!(data.numeric(0x2167), Some(8.0));
        assert_eq!(data.digit(0xb2), Some(2));
        assert_eq!(data.decimal(0xb2), None);
        assert_eq!(data.combining(0x301), 230);
        assert!(data.mirrored(0x28));
    }
    assert!(UnicodeData::for_version((3, 11)).is_none());
    assert!(UnicodeData::for_version((4, 12)).is_none());
    assert!(UnicodeData::for_version((3, i64::MAX)).is_none());
    assert!(UnicodeData::for_version((3, -1)).is_none());
}

#[test]
fn full_case_conversion_keeps_context_expansion_and_surrogate_boundaries() {
    // Public CPython oracle; these contextual strings distinguish a scalar
    // case-map loop from Python's complete string operation.
    for version in crate::python_version_generated::SUPPORTED_TARGET_PYTHON_VERSIONS {
        let (major, minor) = version.split_once('.').unwrap();
        let (major, minor) = (major.parse::<i64>().unwrap(), minor.parse::<i64>().unwrap());
        let data = UnicodeData::for_version((major, minor)).unwrap();
        for (text, operation, expected) in [
            ("AΣ", CaseOperation::Lower, "aς"),
            ("AΣ'A", CaseOperation::Lower, "aσ'a"),
            ("A\u{301}Σ", CaseOperation::Lower, "a\u{301}ς"),
            ("İßﬃ", CaseOperation::Lower, "i\u{307}ßﬃ"),
            ("İßﬃ", CaseOperation::Upper, "İSSFFI"),
            ("İßﬃ", CaseOperation::Fold, "i\u{307}ssffi"),
            ("aΣ'ß", CaseOperation::Title, "Aσ'Ss"),
            ("ßΣ", CaseOperation::Capitalize, "Ssς"),
            ("ǅaΣ", CaseOperation::Swapcase, "ǅAς"),
        ] {
            assert_eq!(
                data.transform(text.as_bytes(), operation),
                expected.as_bytes()
            );
        }
        assert_eq!(
            data.transform(b"A\xce\xa3\xed\xa0\x80A\xce\xa3", CaseOperation::Lower),
            b"a\xcf\x82\xed\xa0\x80a\xcf\x82"
        );
        assert_eq!(
            data.transform(b"A\xed\xa0\x80\xce\xa3A", CaseOperation::Lower),
            b"a\xed\xa0\x80\xcf\x83a"
        );
    }
}

#[test]
fn numeric_whitespace_keeps_bytes_and_string_domains_separate() {
    use super::numeric_whitespace;
    let data = UnicodeData::ascii();
    for data in [None, Some(data)] {
        assert!(numeric_whitespace(data, ' '));
        assert!(numeric_whitespace(data, '\t'));
        assert!(!numeric_whitespace(data, '\u{1c}'));
        assert!(!numeric_whitespace(data, '\u{1f}'));
    }
    assert!(!numeric_whitespace(None, '\u{3000}'));
    assert!(numeric_whitespace(Some(data), '\u{3000}'));
}

#[test]
fn admitted_tables_share_ascii_facts() {
    for version in crate::python_version_generated::SUPPORTED_TARGET_PYTHON_VERSIONS {
        let (major, minor) = version.split_once('.').unwrap();
        let (major, minor) = (major.parse::<i64>().unwrap(), minor.parse::<i64>().unwrap());
        let data = UnicodeData::for_version((major, minor)).unwrap();
        for code in 0..128 {
            assert_eq!(
                data.record(code).flags,
                UnicodeData::ascii().record(code).flags
            );
        }
    }
}
