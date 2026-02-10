use crate::object::ops::{DecodeTextError as OpsDecodeTextError, EncodeError as OpsEncodeError};
use crate::DecodeFailure as OpsDecodeFailure;
use crate::*;

const ENCODINGS_ALIASES: &[(&str, &str)] = &[
    ("utf_8", "utf-8"),
    ("utf8", "utf-8"),
    ("utf-8", "utf-8"),
    ("utf_8_sig", "utf-8-sig"),
    ("utf8_sig", "utf-8-sig"),
    ("utf-8-sig", "utf-8-sig"),
    ("latin_1", "latin-1"),
    ("latin1", "latin-1"),
    ("iso8859_1", "latin-1"),
    ("ascii", "ascii"),
    ("us_ascii", "ascii"),
    ("cp1252", "cp1252"),
    ("cp_1252", "cp1252"),
    ("cp-1252", "cp1252"),
    ("windows_1252", "cp1252"),
    ("windows-1252", "cp1252"),
    ("cp437", "cp437"),
    ("cp_437", "cp437"),
    ("cp-437", "cp437"),
    ("ibm437", "cp437"),
    ("437", "cp437"),
    ("cp850", "cp850"),
    ("cp_850", "cp850"),
    ("cp-850", "cp850"),
    ("ibm850", "cp850"),
    ("850", "cp850"),
    ("cp860", "cp860"),
    ("cp_860", "cp860"),
    ("cp-860", "cp860"),
    ("ibm860", "cp860"),
    ("860", "cp860"),
    ("cp862", "cp862"),
    ("cp_862", "cp862"),
    ("cp-862", "cp862"),
    ("ibm862", "cp862"),
    ("862", "cp862"),
    ("cp863", "cp863"),
    ("cp_863", "cp863"),
    ("cp-863", "cp863"),
    ("ibm863", "cp863"),
    ("863", "cp863"),
    ("cp865", "cp865"),
    ("cp_865", "cp865"),
    ("cp-865", "cp865"),
    ("ibm865", "cp865"),
    ("865", "cp865"),
    ("cp866", "cp866"),
    ("cp_866", "cp866"),
    ("cp-866", "cp866"),
    ("ibm866", "cp866"),
    ("866", "cp866"),
    ("cp874", "cp874"),
    ("cp_874", "cp874"),
    ("cp-874", "cp874"),
    ("windows_874", "cp874"),
    ("windows-874", "cp874"),
    ("cp1250", "cp1250"),
    ("cp_1250", "cp1250"),
    ("cp-1250", "cp1250"),
    ("windows_1250", "cp1250"),
    ("windows-1250", "cp1250"),
    ("cp1251", "cp1251"),
    ("cp_1251", "cp1251"),
    ("cp-1251", "cp1251"),
    ("windows_1251", "cp1251"),
    ("windows-1251", "cp1251"),
    ("cp1253", "cp1253"),
    ("cp_1253", "cp1253"),
    ("cp-1253", "cp1253"),
    ("windows_1253", "cp1253"),
    ("windows-1253", "cp1253"),
    ("cp1254", "cp1254"),
    ("cp_1254", "cp1254"),
    ("cp-1254", "cp1254"),
    ("windows_1254", "cp1254"),
    ("windows-1254", "cp1254"),
    ("cp1255", "cp1255"),
    ("cp_1255", "cp1255"),
    ("cp-1255", "cp1255"),
    ("windows_1255", "cp1255"),
    ("windows-1255", "cp1255"),
    ("cp1256", "cp1256"),
    ("cp_1256", "cp1256"),
    ("cp-1256", "cp1256"),
    ("windows_1256", "cp1256"),
    ("windows-1256", "cp1256"),
    ("cp1257", "cp1257"),
    ("cp_1257", "cp1257"),
    ("cp-1257", "cp1257"),
    ("windows_1257", "cp1257"),
    ("windows-1257", "cp1257"),
    ("koi8_r", "koi8-r"),
    ("koi8-r", "koi8-r"),
    ("koi8r", "koi8-r"),
    ("koi8_u", "koi8-u"),
    ("koi8-u", "koi8-u"),
    ("koi8u", "koi8-u"),
    ("iso8859_2", "iso8859-2"),
    ("iso-8859-2", "iso8859-2"),
    ("iso8859-2", "iso8859-2"),
    ("latin2", "iso8859-2"),
    ("latin_2", "iso8859-2"),
    ("latin-2", "iso8859-2"),
    ("iso8859_3", "iso8859-3"),
    ("iso-8859-3", "iso8859-3"),
    ("iso8859-3", "iso8859-3"),
    ("latin3", "iso8859-3"),
    ("latin_3", "iso8859-3"),
    ("latin-3", "iso8859-3"),
    ("iso8859_4", "iso8859-4"),
    ("iso-8859-4", "iso8859-4"),
    ("iso8859-4", "iso8859-4"),
    ("latin4", "iso8859-4"),
    ("latin_4", "iso8859-4"),
    ("latin-4", "iso8859-4"),
    ("iso8859_5", "iso8859-5"),
    ("iso-8859-5", "iso8859-5"),
    ("iso8859-5", "iso8859-5"),
    ("cyrillic", "iso8859-5"),
    ("iso8859_6", "iso8859-6"),
    ("iso-8859-6", "iso8859-6"),
    ("iso8859-6", "iso8859-6"),
    ("arabic", "iso8859-6"),
    ("iso8859_7", "iso8859-7"),
    ("iso-8859-7", "iso8859-7"),
    ("iso8859-7", "iso8859-7"),
    ("greek", "iso8859-7"),
    ("iso8859_8", "iso8859-8"),
    ("iso-8859-8", "iso8859-8"),
    ("iso8859-8", "iso8859-8"),
    ("hebrew", "iso8859-8"),
    ("iso8859_10", "iso8859-10"),
    ("iso-8859-10", "iso8859-10"),
    ("iso8859-10", "iso8859-10"),
    ("latin6", "iso8859-10"),
    ("latin_6", "iso8859-10"),
    ("latin-6", "iso8859-10"),
    ("iso8859_15", "iso8859-15"),
    ("iso-8859-15", "iso8859-15"),
    ("iso8859-15", "iso8859-15"),
    ("latin9", "iso8859-15"),
    ("latin_9", "iso8859-15"),
    ("latin-9", "iso8859-15"),
    ("mac_roman", "mac-roman"),
    ("mac-roman", "mac-roman"),
    ("macroman", "mac-roman"),
];

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
pub extern "C" fn molt_encodings_aliases_map() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut pairs = Vec::with_capacity(ENCODINGS_ALIASES.len() * 2);
        for &(alias, canonical) in ENCODINGS_ALIASES {
            let alias_ptr = alloc_string(_py, alias.as_bytes());
            if alias_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let canonical_ptr = alloc_string(_py, canonical.as_bytes());
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
