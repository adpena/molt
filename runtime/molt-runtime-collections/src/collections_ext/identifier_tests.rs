use super::is_valid_identifier;
use molt_obj_model::{
    python_version_generated::SUPPORTED_TARGET_PYTHON_VERSIONS, unicode::UnicodeData,
};

#[test]
fn namedtuple_identifiers_follow_target_xid_and_surrogate_domains() {
    // Public CPython namedtuple/str.isidentifier observations, independent of
    // Rust Alphabetic/Alphanumeric and of the generated table parser.
    for version in SUPPORTED_TARGET_PYTHON_VERSIONS {
        let minor = version.split_once('.').unwrap().1.parse::<i64>().unwrap();
        let data = UnicodeData::for_version((major, minor)).unwrap();
        for (name, expected) in [
            ("a²", false),
            ("℘", true),
            ("a\u{301}", true),
            ("\u{301}", false),
            ("", false),
        ] {
            assert_eq!(
                is_valid_identifier(data, name.as_bytes()),
                expected,
                "{version} {name:?}"
            );
        }
        assert!(!is_valid_identifier(data, b"a\xed\xa0\x80"));
        assert_eq!(
            is_valid_identifier(data, "\u{1c89}".as_bytes()),
            minor >= 14
        );
    }
}
