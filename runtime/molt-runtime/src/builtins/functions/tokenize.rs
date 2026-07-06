// Tokenize and linecache source-encoding metadata ABI shims.
//
// Pure scanner and encoding-cookie algorithms live in `molt-runtime-text`; this
// module owns Molt object conversion and exported ABI entrypoints.

use super::*;
use molt_runtime_text::tokenize::{detect_source_encoding, scan_tokens};

#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

/// Tokenize a UTF-8 source string into a list of (type, string, start, end, line) tuples.
/// Token types: 0=ENDMARKER, 1=NAME, 2=NUMBER, 4=NEWLINE, 54=OP, 64=COMMENT, 65=NL, 67=ENCODING
#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_scan(source_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let source_obj = crate::obj_from_bits(source_bits);
        let Some(source) = crate::string_obj_to_owned(source_obj) else {
            return crate::raise_exception::<_>(_py, "TypeError", "source must be str");
        };

        let scan = scan_tokens(&source);
        let mut line_bits: Vec<u64> = Vec::with_capacity(scan.lines.len());
        for line in &scan.lines {
            line_bits
                .push(alloc_string_bits(_py, line).unwrap_or_else(|| MoltObject::none().bits()));
        }

        let mut tokens: Vec<u64> = Vec::with_capacity(scan.tokens.len());
        for token in &scan.tokens {
            let line_bits = line_bits
                .get(token.line_index)
                .copied()
                .unwrap_or_else(|| MoltObject::none().bits());
            tokens.push(make_token_tuple(
                _py,
                token.kind.code(),
                &token.text,
                token.start,
                token.end,
                line_bits,
            ));
        }

        for bits in &line_bits {
            if *bits != MoltObject::none().bits() {
                dec_ref_bits(_py, *bits);
            }
        }

        let list_ptr = crate::alloc_list(_py, &tokens);
        for bits in &tokens {
            crate::dec_ref_bits(_py, *bits);
        }
        if list_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

fn make_token_tuple(
    _py: &crate::PyToken<'_>,
    tok_type: i64,
    string: &str,
    start: (i64, i64),
    end: (i64, i64),
    line_bits: u64,
) -> u64 {
    let type_bits = MoltObject::from_int(tok_type).bits();
    let string_ptr = crate::alloc_string(_py, string.as_bytes());
    let string_bits = if string_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(string_ptr).bits()
    };
    let start_elems = [
        MoltObject::from_int(start.0).bits(),
        MoltObject::from_int(start.1).bits(),
    ];
    let start_ptr = crate::alloc_tuple(_py, &start_elems);
    let start_bits = if start_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(start_ptr).bits()
    };
    let end_elems = [
        MoltObject::from_int(end.0).bits(),
        MoltObject::from_int(end.1).bits(),
    ];
    let end_ptr = crate::alloc_tuple(_py, &end_elems);
    let end_bits = if end_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(end_ptr).bits()
    };
    let elems = [type_bits, string_bits, start_bits, end_bits, line_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

/// Detect Python source file encoding from the first two lines.
/// `first_bits`: first line bytes, `second_bits`: second line bytes
/// Returns (encoding_name, has_bom) tuple.
#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_detect_encoding(first_bits: u64, second_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let first_obj = crate::obj_from_bits(first_bits);
        let second_obj = crate::obj_from_bits(second_bits);

        let first_bytes = if let Some(ptr) = first_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let second_bytes = if let Some(ptr) = second_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let detection = detect_source_encoding(first_bytes, second_bytes);
        encoding_detection_tuple(_py, &detection.encoding, detection.bom_found)
    })
}

fn encoding_detection_tuple(_py: &crate::PyToken<'_>, encoding: &str, bom_found: bool) -> u64 {
    let enc_ptr = crate::alloc_string(_py, encoding.as_bytes());
    let bom_bits = MoltObject::from_bool(bom_found).bits();
    if enc_ptr.is_null() {
        return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}
