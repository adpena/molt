//! Runtime ABI bridge for the `encodings.punycode` codec.
//!
//! Pure RFC 3492 encode/decode behavior lives in `molt-stdlib-text`; this
//! module owns only runtime object marshaling, allocation, and exception shape.

use crate::BuiltinExc;
use crate::*;
use molt_stdlib_text::punycode::{punycode_decode_impl, punycode_encode_impl};

/// `molt_punycode_encode(text) -> bytes`

pub(crate) fn molt_punycode_encode_attached_body(_py: &crate::PyToken<'_>, text_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "punycode_encode: expected str",
            );
        };
        let encoded = punycode_encode_impl(&text);
        let ptr = alloc_bytes(_py, &encoded);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `molt_punycode_decode(data, errors) -> str`

pub(crate) fn molt_punycode_decode_attached_body(
    _py: &crate::PyToken<'_>,
    data_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let raw: Vec<u8> = if let Some(s) = string_obj_to_owned(obj_from_bits(data_bits)) {
            s.into_bytes()
        } else {
            let obj = obj_from_bits(data_bits);
            let Some(ptr) = obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "punycode_decode: expected str or bytes",
                );
            };
            match unsafe { bytes_like_slice(ptr) } {
                Some(slice) => slice.to_vec(),
                None => {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "punycode_decode: expected str or bytes",
                    );
                }
            }
        };

        let errors_str =
            string_obj_to_owned(obj_from_bits(errors_bits)).unwrap_or_else(|| "strict".to_string());

        match punycode_decode_impl(&raw, &errors_str) {
            Ok(decoded) => {
                let s_ptr = alloc_string(_py, decoded.as_bytes());
                if s_ptr.is_null() {
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                MoltObject::from_ptr(s_ptr).bits()
            }
            Err(msg) => raise_exception::<_>(_py, BuiltinExc::UnicodeError, &msg),
        }
    })
}

include!("../attached_exports_generated/builtins/punycode.rs");
