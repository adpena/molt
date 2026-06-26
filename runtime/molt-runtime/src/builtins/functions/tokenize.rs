// Tokenize and linecache source-encoding metadata ABI.
// Extracted from functions.rs so scanner/encoding mechanics stay out of
// function-object and miscellaneous stdlib runtime ownership.

use super::*;
use memchr::{memchr, memmem};

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

        const ENDMARKER: i64 = 0;
        const NAME: i64 = 1;
        const NUMBER: i64 = 2;
        const NEWLINE: i64 = 4;
        const OP: i64 = 54;
        const COMMENT: i64 = 64;
        const NL: i64 = 65;

        fn is_name_start(ch: u8) -> bool {
            ch == b'_' || ch.is_ascii_alphabetic()
        }
        fn is_name_char(ch: u8) -> bool {
            is_name_start(ch) || ch.is_ascii_digit()
        }

        let mut tokens: Vec<u64> = Vec::new();
        let source_bytes = source.as_bytes();
        let mut line_no: i64 = 1;

        if !source_bytes.is_empty() {
            let mut start = 0usize;
            while start < source_bytes.len() {
                let line_end = memchr(b'\n', &source_bytes[start..])
                    .map(|rel| start + rel + 1)
                    .unwrap_or(source_bytes.len());
                let line = &source[start..line_end];
                let line_bytes = line.as_bytes();
                let line_len = line_bytes.len();
                let line_bits =
                    alloc_string_bits(_py, line).unwrap_or_else(|| MoltObject::none().bits());

                // Full-line comment check
                let trimmed_start = line_bytes.iter().position(|&b| b != b' ' && b != b'\t');
                if let Some(ts) = trimmed_start
                    && line_bytes[ts] == b'#'
                {
                    let comment = line.trim();
                    let tok = make_token_tuple(
                        _py,
                        COMMENT,
                        comment,
                        (line_no, 0),
                        (line_no, comment.len() as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    if line.ends_with('\n') {
                        let tok = make_token_tuple(
                            _py,
                            NL,
                            "\n",
                            (line_no, (line_len - 1) as i64),
                            (line_no, line_len as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                    }
                    if line_bits != MoltObject::none().bits() {
                        dec_ref_bits(_py, line_bits);
                    }
                    line_no += 1;
                    start = line_end;
                    continue;
                }

                let mut col: usize = 0;
                while col < line_len {
                    let ch = line_bytes[col];
                    if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                        col += 1;
                        continue;
                    }
                    if ch == b'#' {
                        let comment = line[col..].trim_end_matches(['\r', '\n']);
                        let tok = make_token_tuple(
                            _py,
                            COMMENT,
                            comment,
                            (line_no, col as i64),
                            (line_no, (col + comment.len()) as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        break;
                    }
                    if is_name_start(ch) {
                        let start_col = col;
                        col += 1;
                        while col < line_len && is_name_char(line_bytes[col]) {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NAME,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    if ch.is_ascii_digit() {
                        let start_col = col;
                        col += 1;
                        while col < line_len && line_bytes[col].is_ascii_digit() {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NUMBER,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    // OP
                    let ch_str = &line[col..col + 1];
                    let tok = make_token_tuple(
                        _py,
                        OP,
                        ch_str,
                        (line_no, col as i64),
                        (line_no, (col + 1) as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    col += 1;
                }

                if line.ends_with('\n') {
                    let stripped = line.trim();
                    let has_content = !stripped.is_empty() && !stripped.starts_with('#');
                    let tok_type = if has_content { NEWLINE } else { NL };
                    let tok = make_token_tuple(
                        _py,
                        tok_type,
                        "\n",
                        (line_no, (line_len - 1) as i64),
                        (line_no, line_len as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                }
                if line_bits != MoltObject::none().bits() {
                    dec_ref_bits(_py, line_bits);
                }
                line_no += 1;
                if line_end == source_bytes.len() {
                    break;
                }
                start = line_end;
            }
        }

        // ENDMARKER
        let endmarker_line_bits =
            alloc_string_bits(_py, "").unwrap_or_else(|| MoltObject::none().bits());
        let tok = make_token_tuple(
            _py,
            ENDMARKER,
            "",
            (line_no, 0),
            (line_no, 0),
            endmarker_line_bits,
        );
        tokens.push(tok);
        if endmarker_line_bits != MoltObject::none().bits() {
            dec_ref_bits(_py, endmarker_line_bits);
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

fn skip_encoding_ws(bytes: &[u8]) -> &[u8] {
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b' ' | b'\t' | b'\x0c' => idx += 1,
            _ => break,
        }
    }
    &bytes[idx..]
}

fn find_encoding_cookie(line: &[u8]) -> Option<&str> {
    let stripped = skip_encoding_ws(line);
    if !stripped.starts_with(b"#") {
        return None;
    }
    let coding_idx = memmem::find(stripped, b"coding")?;
    let mut rest = &stripped[coding_idx + "coding".len()..];
    rest = skip_encoding_ws(rest);
    let (sep, rest) = rest.split_first()?;
    if *sep != b':' && *sep != b'=' {
        return None;
    }
    let rest = skip_encoding_ws(rest);
    if rest.is_empty() {
        return None;
    }
    let end = rest
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
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

        let bom_utf8: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut bom_found = false;
        let mut effective_first = first_bytes;
        let mut default_enc = "utf-8";

        if effective_first.starts_with(bom_utf8) {
            bom_found = true;
            effective_first = &effective_first[3..];
            default_enc = "utf-8-sig";
        }

        if effective_first.is_empty() && second_bytes.is_empty() {
            let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check first line
        if let Some(encoding) = find_encoding_cookie(effective_first) {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
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
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check second line
        if !second_bytes.is_empty()
            && let Some(encoding) = find_encoding_cookie(second_bytes)
        {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
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
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Default encoding
        let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
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
    })
}

#[cfg(test)]
mod tokenize_encoding_tests {
    use super::{find_encoding_cookie, skip_encoding_ws};

    #[test]
    fn skip_encoding_ws_trims_python_prefix_whitespace() {
        assert_eq!(skip_encoding_ws(b" \t\x0ccoding"), b"coding");
    }

    #[test]
    fn find_encoding_cookie_handles_standard_cookie() {
        assert_eq!(find_encoding_cookie(b"# coding: utf-8"), Some("utf-8"));
        assert_eq!(
            find_encoding_cookie(b"# -*- coding: latin-1 -*-"),
            Some("latin-1")
        );
    }

    #[test]
    fn find_encoding_cookie_rejects_non_cookie_lines() {
        assert_eq!(find_encoding_cookie(b"print('hi')"), None);
        assert_eq!(find_encoding_cookie(b"# comment only"), None);
    }
}
