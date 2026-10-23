//! String descriptor predicates over Python code points, including surrogates.
//!
//! Admission is shared with strip/split. Scalar properties come from the selected
//! interpreter target's pinned Unicode tables; Rust UTF-8/Unicode properties are
//! not a second semantic authority. Classification never allocates or calls a
//! receiver protocol, and ASCII paths keep their existing SIMD kernels.

use super::*;
use molt_obj_model::text_storage::code_points;

fn all_nonempty(bytes: &[u8], predicate: impl FnMut(u32) -> bool) -> bool {
    !bytes.is_empty() && code_points(bytes).all(predicate)
}

pub(crate) fn is_identifier_bytes(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    let data = unicode_data(py);
    let mut codes = code_points(bytes);
    let Some(first) = codes.next() else {
        return false;
    };
    data.is_identifier_start(first) && codes.all(|code| data.is_identifier_continue(code))
}

fn isdigit(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_digit(bytes)
    } else {
        {
            let data = unicode_data(py);
            all_nonempty(bytes, |code| data.is_digit(code))
        }
    }
}

fn isdecimal(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_digit(bytes)
    } else {
        {
            let data = unicode_data(py);
            all_nonempty(bytes, |code| data.is_decimal(code))
        }
    }
}

fn isnumeric(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_digit(bytes)
    } else {
        {
            let data = unicode_data(py);
            all_nonempty(bytes, |code| data.is_numeric(code))
        }
    }
}

fn isspace(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_text_whitespace(bytes)
    } else {
        {
            let data = unicode_data(py);
            all_nonempty(bytes, |code| data.is_space(code))
        }
    }
}

fn isalpha(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_alpha(bytes)
    } else {
        {
            let data = unicode_data(py);
            all_nonempty(bytes, |code| data.is_alpha(code))
        }
    }
}

fn isalnum(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_alnum(bytes)
    } else {
        let data = unicode_data(py);
        all_nonempty(bytes, |code| data.is_alpha(code) || data.is_numeric(code))
    }
}

fn islower(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        return simd_has_any_ascii_lower(bytes) && !simd_has_any_ascii_upper(bytes);
    }
    let data = unicode_data(py);
    let mut seen_lower = false;
    for code in code_points(bytes) {
        if data.is_lower(code) {
            seen_lower = true;
        } else if data.is_upper(code) || data.is_title(code) {
            return false;
        }
    }
    seen_lower
}

fn isupper(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        return simd_has_any_ascii_upper(bytes) && !simd_has_any_ascii_lower(bytes);
    }
    let data = unicode_data(py);
    let mut seen_upper = false;
    for code in code_points(bytes) {
        if data.is_upper(code) {
            seen_upper = true;
        } else if data.is_lower(code) || data.is_title(code) {
            return false;
        }
    }
    seen_upper
}

fn isascii(_py: &PyToken<'_>, bytes: &[u8]) -> bool {
    bytes.is_ascii()
}

fn istitle(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    let data = unicode_data(py);
    let mut seen_cased = false;
    let mut previous_cased = false;
    for code in code_points(bytes) {
        if data.is_upper(code) || data.is_title(code) {
            if previous_cased {
                return false;
            }
            seen_cased = true;
            previous_cased = true;
        } else if data.is_lower(code) {
            if !previous_cased {
                return false;
            }
            seen_cased = true;
            previous_cased = true;
        } else {
            // An uncased code point, including a lone surrogate, is a word
            // boundary rather than an invalid encoding of the whole string.
            previous_cased = false;
        }
    }
    seen_cased
}

fn isprintable(py: &PyToken<'_>, bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        simd_is_all_ascii_printable(bytes)
    } else {
        {
            let data = unicode_data(py);
            code_points(bytes).all(|code| data.is_printable(code))
        }
    }
}

fn string_predicate(
    py: &PyToken<'_>,
    hay_bits: u64,
    method: &str,
    classify: impl FnOnce(&PyToken<'_>, &[u8]) -> bool,
) -> u64 {
    let Some(hay_ptr) = validate_string_receiver(py, hay_bits, method) else {
        return MoltObject::none().bits();
    };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr)) };
    MoltObject::from_bool(classify(py, bytes)).bits()
}

// Keep concrete exported signatures visible to runtime symbol/ABI discovery.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isidentifier(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        string_predicate(py, hay_bits, "isidentifier", is_identifier_bytes)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isdigit(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isdigit", isdigit) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isdecimal(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        string_predicate(py, hay_bits, "isdecimal", isdecimal)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isnumeric(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        string_predicate(py, hay_bits, "isnumeric", isnumeric)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isspace(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isspace", isspace) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isalpha(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isalpha", isalpha) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isalnum(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isalnum", isalnum) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_islower(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "islower", islower) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isupper(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isupper", isupper) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isascii(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "isascii", isascii) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_istitle(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { string_predicate(py, hay_bits, "istitle", istitle) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_isprintable(hay_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        string_predicate(py, hay_bits, "isprintable", isprintable)
    })
}

#[cfg(test)]
type Predicate = (
    &'static str,
    extern "C" fn(u64) -> u64,
    fn(&PyToken<'_>, &[u8]) -> bool,
);

#[cfg(test)]
const PREDICATES: &[Predicate] = &[
    (
        "isidentifier",
        molt_string_isidentifier,
        is_identifier_bytes,
    ),
    ("isdigit", molt_string_isdigit, isdigit),
    ("isdecimal", molt_string_isdecimal, isdecimal),
    ("isnumeric", molt_string_isnumeric, isnumeric),
    ("isspace", molt_string_isspace, isspace),
    ("isalpha", molt_string_isalpha, isalpha),
    ("isalnum", molt_string_isalnum, isalnum),
    ("islower", molt_string_islower, islower),
    ("isupper", molt_string_isupper, isupper),
    ("isascii", molt_string_isascii, isascii),
    ("istitle", molt_string_istitle, istitle),
    ("isprintable", molt_string_isprintable, isprintable),
];

#[cfg(test)]
mod predicate_contract_tests {
    use super::*;

    #[test]
    fn exported_predicates_follow_provisional_and_committed_target_facts() {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let cyrillic = MoltObject::from_ptr(alloc_string(py, "\u{1c89}".as_bytes())).bits();
            let glottal = MoltObject::from_ptr(alloc_string(py, "\u{295}".as_bytes())).bits();
            let symbol = MoltObject::from_ptr(alloc_string(py, "\u{2ffc}".as_bytes())).bits();
            transaction.with_target_python(py, None, || {
                assert_eq!(
                    obj_from_bits(molt_string_isupper(cyrillic)).as_bool(),
                    Some(false)
                );
                assert!(
                    crate::runtime_state(py)
                        .sys_version_info
                        .lock()
                        .unwrap()
                        .is_none()
                );
                for (minor, new_upper, old_lower, new_symbol) in [
                    (15, true, false, true),
                    (12, false, true, false),
                    (14, true, true, true),
                    (13, false, true, true),
                ] {
                    transaction.with_target_python_minor(py, minor, || {
                        assert_eq!(
                            obj_from_bits(molt_string_isupper(cyrillic)).as_bool(),
                            Some(new_upper)
                        );
                        assert_eq!(
                            obj_from_bits(molt_string_islower(glottal)).as_bool(),
                            Some(old_lower)
                        );
                        assert_eq!(
                            obj_from_bits(molt_string_isprintable(symbol)).as_bool(),
                            Some(new_symbol)
                        );
                    });
                }
            });
            for bits in [cyrillic, glottal, symbol] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn split_contract_string_predicates_reject_scalar_and_heap_receivers() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let bytes = alloc_bytes(py, b"text");
            assert!(!bytes.is_null());
            let bytes = MoltObject::from_ptr(bytes).bits();
            for (name, entry, _) in PREDICATES {
                for (receiver, receiver_type) in [
                    (MoltObject::none().bits(), "NoneType"),
                    (MoltObject::from_int(7).bits(), "int"),
                    (MoltObject::from_float(1.5).bits(), "float"),
                    (MoltObject::from_bool(true).bits(), "bool"),
                    (bytes, "bytes"),
                ] {
                    assert!(obj_from_bits(entry(receiver)).is_none(), "{name}");
                    assert!(exception_pending(py), "{name}");
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "TypeError"
                    ));
                    let message = crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    assert_eq!(
                        message,
                        format!(
                            "descriptor '{name}' for 'str' objects doesn't apply to a '{receiver_type}' object"
                        )
                    );
                    clear_exception(py);
                    dec_ref_bits(py, error);
                }
            }
            dec_ref_bits(py, bytes);
        });
    }

    #[test]
    fn split_contract_string_predicates_classify_wtf8_without_whole_string_rejection() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let cases: &[(&[u8], &[&str])] = &[
                (b"", &["isascii", "isprintable"]),
                (
                    b"abc",
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "islower",
                        "isascii",
                        "isprintable",
                    ],
                ),
                (
                    b"ABC",
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "isupper",
                        "isascii",
                        "isprintable",
                    ],
                ),
                (
                    b"Abc",
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "isascii",
                        "istitle",
                        "isprintable",
                    ],
                ),
                (
                    b"123",
                    &[
                        "isdigit",
                        "isdecimal",
                        "isnumeric",
                        "isalnum",
                        "isascii",
                        "isprintable",
                    ],
                ),
                (b"\x1c", &["isspace", "isascii"]),
                ("\u{a0}".as_bytes(), &["isspace"]),
                (
                    "\u{b2}".as_bytes(),
                    &["isdigit", "isnumeric", "isalnum", "isprintable"],
                ),
                (
                    "\u{bc}".as_bytes(),
                    &["isnumeric", "isalnum", "isprintable"],
                ),
                (
                    "\u{660}".as_bytes(),
                    &[
                        "isdigit",
                        "isdecimal",
                        "isnumeric",
                        "isalnum",
                        "isprintable",
                    ],
                ),
                ("\u{345}".as_bytes(), &["islower", "isprintable"]),
                (
                    "a\u{345}".as_bytes(),
                    &["isidentifier", "islower", "isprintable"],
                ),
                (
                    "\u{1c5}".as_bytes(),
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "istitle",
                        "isprintable",
                    ],
                ),
                // U+00AA is lowercase even though both case mappings are unchanged.
                (
                    "\u{aa}".as_bytes(),
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "islower",
                        "isprintable",
                    ],
                ),
                (
                    "A\u{aa}".as_bytes(),
                    &[
                        "isidentifier",
                        "isalpha",
                        "isalnum",
                        "istitle",
                        "isprintable",
                    ],
                ),
                (b"\xed\xa0\x80", &[]),
                (b"\xed\xbf\xbf", &[]),
                (b"a\xed\xa0\x80", &["islower"]),
                (b"\xed\xa0\x80A", &["isupper", "istitle"]),
                (b"A\xed\xa0\x80B", &["isupper", "istitle"]),
                (b"A\xed\xa0\x80b", &[]),
                (b"a\xed\xa0\x80b", &["islower"]),
            ];
            for (bytes, expected) in cases {
                let receiver = alloc_string(py, bytes);
                assert!(!receiver.is_null());
                let receiver = MoltObject::from_ptr(receiver).bits();
                for (name, entry, classify) in PREDICATES {
                    let expected = expected.contains(name);
                    assert_eq!(classify(py, bytes), expected, "{name} {bytes:?}");
                    assert_eq!(
                        entry(receiver),
                        MoltObject::from_bool(expected).bits(),
                        "{name} {bytes:?}"
                    );
                    assert!(!exception_pending(py), "{name}");
                }
                dec_ref_bits(py, receiver);
            }
        });
    }

    #[test]
    fn split_contract_string_predicate_ascii_kernels_match_scalar_authority() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let data = unicode_data(py);
            for code in 0u8..128 {
                let scalar = u32::from(code);
                for len in [1, 15, 16, 17, 33] {
                    let bytes = vec![code; len];
                    assert_eq!(isdigit(py, &bytes), data.is_digit(scalar));
                    assert_eq!(isdecimal(py, &bytes), data.is_decimal(scalar));
                    assert_eq!(isnumeric(py, &bytes), data.is_numeric(scalar));
                    assert_eq!(isspace(py, &bytes), data.is_space(scalar));
                    assert_eq!(isalpha(py, &bytes), data.is_alpha(scalar));
                    assert_eq!(
                        isalnum(py, &bytes),
                        data.is_alpha(scalar) || data.is_numeric(scalar)
                    );
                    assert_eq!(islower(py, &bytes), data.is_lower(scalar));
                    assert_eq!(isupper(py, &bytes), data.is_upper(scalar));
                    assert_eq!(isprintable(py, &bytes), data.is_printable(scalar));
                }
            }
        });
    }
}
