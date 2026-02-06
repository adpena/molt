use crate::object::ops::{DecodeTextError as OpsDecodeTextError, EncodeError as OpsEncodeError};
use crate::DecodeFailure as OpsDecodeFailure;
use crate::*;

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
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not '{type_name}'");
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

fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
    format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}")
}

#[no_mangle]
pub extern "C" fn molt_codecs_decode(obj_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            Err(OpsDecodeTextError::Failure(_failure, _label)) => {}
        }

        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            let type_name = type_name(_py, obj);
            let msg = format!("a bytes-like object is required, not '{type_name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(bytes) = (unsafe { bytes_like_slice(ptr) }) else {
            let type_name = type_name(_py, obj);
            let msg = format!("a bytes-like object is required, not '{type_name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let out_bits = match decode_bytes_text(&encoding, &errors, bytes) {
            Ok((text_bytes, _label)) => {
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(OpsDecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::Byte { pos, byte, message },
                label,
            )) => {
                let msg = decode_error_byte(&label, byte, pos, message);
                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                label,
            )) => {
                let msg = decode_error_range(&label, start, end, message);
                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
            }
            Err(OpsDecodeTextError::Failure(
                OpsDecodeFailure::UnknownErrorHandler(name),
                _label,
            )) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
        };
        out_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_codecs_encode(obj_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let encoding = match codec_arg_to_str(_py, encoding_bits, "encode", "encoding") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match codec_arg_to_str(_py, errors_bits, "encode", "errors") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };

        match encode_string_with_errors(&[], &encoding, Some(&errors)) {
            Ok(_) => {}
            Err(OpsEncodeError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err(OpsEncodeError::InvalidChar { .. }) => {}
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
                Err(OpsEncodeError::UnknownEncoding(name)) => {
                    let msg = format!("unknown encoding: {name}");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(OpsEncodeError::UnknownErrorHandler(name)) => {
                    let msg = format!("unknown error handler name '{name}'");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(OpsEncodeError::InvalidChar {
                    encoding,
                    code,
                    pos,
                    limit,
                }) => {
                    let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                    return raise_unicode_encode_error::<_>(
                        _py,
                        encoding,
                        obj_bits,
                        pos,
                        pos + 1,
                        &reason,
                    );
                }
            };
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_codecs_lookup_name(encoding_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
