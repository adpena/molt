// Shlex stdlib implementation.
// Extracted from functions.rs for tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
use super::functions::*;


pub(crate) fn shlex_is_safe(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(
            b,
            b'a'..=b'z'
                | b'A'..=b'Z'
                | b'0'..=b'9'
                | b'_'
                | b'@'
                | b'%'
                | b'+'
                | b'='
                | b':'
                | b','
                | b'.'
                | b'/'
                | b'-'
        )
    })
}


pub(crate) fn shlex_quote_impl(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }
    if shlex_is_safe(input) {
        return input.to_string();
    }
    let escaped = input.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}


pub(crate) fn shlex_split_impl(
    input: &str,


pub(crate) fn shlex_join_impl(parts: &[String]) -> String {
    parts
        .iter()
        .map(|item| shlex_quote_impl(item))
        .collect::<Vec<String>>()
        .join(" ")
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_quote(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.quote argument must be str");
        };
        let out = shlex_quote_impl(&text);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split(text_bits: u64, whitespace_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split argument must be str");
        };
        let Some(whitespace) = string_obj_to_owned(obj_from_bits(whitespace_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split whitespace must be str");
        };
        let parts = match shlex_split_impl(&text, &whitespace, true, false, "#", true, "") {
            Ok(parts) => parts,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &parts)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split_ex(
    text_bits: u64,


#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_join(words_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match iterable_to_string_vec(_py, words_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let out = shlex_join_impl(&parts);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

