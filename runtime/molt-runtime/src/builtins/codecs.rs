use crate::object::ops::DecodeTextError as OpsDecodeTextError;
use crate::*;
use molt_stdlib_text::codec_registry::PYTHON_ENCODING_ALIASES;

fn codec_arg_to_str(
    _py: &PyToken<'_>,
    bits: u64,
    func_name: &str,
    arg_name: &str,
) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not None");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    Some(text)
}

fn lookup_arg_to_str(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return raise_exception::<_>(_py, "TypeError", "lookup() argument must be str, not None");
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("lookup() argument must be str, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    Some(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_encodings_aliases_map() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut pairs = Vec::with_capacity(PYTHON_ENCODING_ALIASES.len() * 2);
        for entry in PYTHON_ENCODING_ALIASES {
            let alias_ptr = alloc_string(_py, entry.alias.as_bytes());
            if alias_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let canonical_ptr = alloc_string(_py, entry.module.as_bytes());
            if canonical_ptr.is_null() {
                return MoltObject::none().bits();
            }
            pairs.push(MoltObject::from_ptr(alias_ptr).bits());
            pairs.push(MoltObject::from_ptr(canonical_ptr).bits());
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

// Both public codec result shapes share the same decode/encode and input owner.
// Value-only calls never allocate a tuple or scan the input for a count.
pub(super) fn codec_result(
    py: &PyToken<'_>,
    result_bits: u64,
    consumed: i64,
    with_consumed: bool,
) -> u64 {
    if !with_consumed {
        return result_bits;
    }
    let result = crate::builtins::exceptions::ExceptionValue::adopt(py, result_bits);
    let count_bits = crate::builtins::numbers::int_bits_from_i64(py, consumed);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let count = crate::builtins::exceptions::ExceptionValue::adopt(py, count_bits);
    let tuple = alloc_tuple(py, &[result.bits(), count.bits()]);
    if tuple.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_decode(
    obj_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    with_consumed_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(with_consumed) = obj_from_bits(with_consumed_bits).as_bool() else {
            return raise_exception::<_>(_py, "TypeError", "codec result selector must be bool");
        };
        let encoding = match codec_arg_to_str(_py, encoding_bits, "decode", "encoding") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match codec_arg_to_str(_py, errors_bits, "decode", "errors") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };

        match decode_bytes_text(&encoding, &errors, &[]) {
            Ok(_) => {}
            Err(OpsDecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::Failure(..)) => {}
        }

        let obj = obj_from_bits(obj_bits);
        let Some(_ptr) = obj.as_ptr() else {
            let type_name = type_name(_py, obj);
            let msg = format!("a bytes-like object is required, not '{type_name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let input = match crate::object::buffer_exports::ScopedBuffer::simple(_py, obj_bits) {
            Ok(input) => input,
            Err(error) => return error.raise(_py, "a bytes-like object is required"),
        };
        let bytes = input.as_slice();

        match decode_bytes_text(&encoding, &errors, bytes) {
            Ok((text_bytes, _label)) => {
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                codec_result(
                    _py,
                    MoltObject::from_ptr(ptr).bits(),
                    bytes.len() as i64,
                    with_consumed,
                )
            }
            Err(error) => error.raise(_py, bytes),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_encode(
    obj_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    with_consumed_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(with_consumed) = obj_from_bits(with_consumed_bits).as_bool() else {
            return raise_exception::<_>(_py, "TypeError", "codec result selector must be bool");
        };
        let encoding = match codec_arg_to_str(_py, encoding_bits, "encode", "encoding") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match codec_arg_to_str(_py, errors_bits, "encode", "errors") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };

        if let Err(error) = encode_string_with_errors(&[], &encoding, Some(&errors)) {
            return error.raise(_py, obj_bits);
        }

        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "utf_8_encode() argument 1 must be str, not None",
            );
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let type_name = type_name(_py, obj);
                let msg = format!("utf_8_encode() argument 1 must be str, not {type_name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let out = match encode_string_with_errors(bytes, &encoding, Some(&errors)) {
                Ok(bytes) => bytes,
                Err(error) => return error.raise(_py, obj_bits),
            };
            let consumed = if with_consumed {
                crate::object::ops_string::utf8_codepoint_count_cached(
                    _py,
                    bytes,
                    Some(ptr as usize),
                )
            } else {
                0
            };
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            codec_result(
                _py,
                MoltObject::from_ptr(ptr).bits(),
                consumed,
                with_consumed,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_codecs_lookup_name(encoding_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let encoding = match lookup_arg_to_str(_py, encoding_bits) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let Some(kind) = crate::object::ops::normalize_encoding(&encoding) else {
            let msg = format!("unknown encoding: {encoding}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        };
        let ptr = alloc_string(_py, crate::object::ops::encoding_kind_name(kind).as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(test)]
mod result_count_tests {
    use super::*;
    use crate::builtins::exceptions::ExceptionValue;

    #[test]
    fn codec_result_count_preserves_full_native_extent() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // No huge input allocation: prove the exact publication boundary
            // across the inline integer limit and at the native signed maximum.
            for count in [(1i64 << 46) - 1, 1i64 << 46, i64::MAX] {
                let result = ExceptionValue::adopt(
                    py,
                    codec_result(py, MoltObject::none().bits(), count, true),
                );
                assert!(!exception_pending(py));
                let elements = unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(
                        obj_from_bits(result.bits()).as_ptr().unwrap(),
                        |values| {
                            assert_eq!(values.len(), 2);
                            [values[0], values[1]]
                        },
                    )
                }
                .unwrap();
                assert_eq!(elements[0], MoltObject::none().bits());
                assert_eq!(to_i64(obj_from_bits(elements[1])), Some(count));
            }
        });
    }

    #[test]
    fn builtin_codec_count_uses_bytes_for_decode_and_codepoints_for_encode() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let encoding =
                ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_string(py, b"utf-8")).bits());
            let errors = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_string(py, b"surrogatepass")).bits(),
            );
            // Three Python code points, including a surrogate, occupy six WTF-8 bytes.
            let bytes = b"a\xc3\xa9\xed\xa0\x80";
            let input =
                ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_bytes(py, bytes)).bits());
            let text =
                ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_string(py, bytes)).bits());
            for with_consumed in [false, true] {
                let mode = MoltObject::from_bool(with_consumed).bits();
                let decoded = ExceptionValue::adopt(
                    py,
                    molt_codecs_decode(input.bits(), encoding.bits(), errors.bits(), mode),
                );
                let encoded = ExceptionValue::adopt(
                    py,
                    molt_codecs_encode(text.bits(), encoding.bits(), errors.bits(), mode),
                );
                assert!(!exception_pending(py));
                unsafe {
                    if with_consumed {
                        let decoded_values = crate::object::seq_access::with_immutable_tuple_slice(
                            obj_from_bits(decoded.bits()).as_ptr().unwrap(),
                            |values| {
                                assert_eq!(values.len(), 2);
                                [values[0], values[1]]
                            },
                        )
                        .unwrap();
                        let encoded_values = crate::object::seq_access::with_immutable_tuple_slice(
                            obj_from_bits(encoded.bits()).as_ptr().unwrap(),
                            |values| {
                                assert_eq!(values.len(), 2);
                                [values[0], values[1]]
                            },
                        )
                        .unwrap();
                        assert_eq!(to_i64(obj_from_bits(decoded_values[1])), Some(6));
                        assert_eq!(to_i64(obj_from_bits(encoded_values[1])), Some(3));
                        let decoded_ptr = obj_from_bits(decoded_values[0]).as_ptr().unwrap();
                        assert_eq!(object_type_id(decoded_ptr), TYPE_ID_STRING);
                        assert_eq!(
                            std::slice::from_raw_parts(
                                string_bytes(decoded_ptr),
                                string_len(decoded_ptr)
                            ),
                            bytes
                        );
                        let encoded_ptr = obj_from_bits(encoded_values[0]).as_ptr().unwrap();
                        assert_eq!(
                            std::slice::from_raw_parts(
                                bytes_data(encoded_ptr),
                                bytes_len(encoded_ptr)
                            ),
                            bytes
                        );
                    } else {
                        assert_eq!(
                            object_type_id(obj_from_bits(decoded.bits()).as_ptr().unwrap()),
                            TYPE_ID_STRING
                        );
                        assert_eq!(
                            object_type_id(obj_from_bits(encoded.bits()).as_ptr().unwrap()),
                            TYPE_ID_BYTES
                        );
                    }
                }
            }
        });
    }
}
