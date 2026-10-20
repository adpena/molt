// Token metadata and scanner/encoding ABI. Numeric identities have one source:
// the target tables generated from receipted CPython source in molt-stdlib-text.
use super::*;
use crate::builtins::exceptions::ExceptionValue;
use molt_stdlib_text::token_metadata_generated::{TokenTable, token_table};
use molt_stdlib_text::tokenize::{detect_source_encoding, scan_tokens};

fn token_allocation<'a, 'py>(
    py: &'a crate::PyToken<'py>,
    ptr: *mut u8,
) -> Option<ExceptionValue<'a, 'py>> {
    (!ptr.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

fn target_token_table(py: &crate::PyToken<'_>) -> Option<&'static TokenTable> {
    let (major, minor) = crate::object::ops_sys::runtime_target_version(py);
    let table = token_table(major, minor);
    if table.is_none() {
        crate::raise_exception::<u64>(
            py,
            "RuntimeError",
            "unsupported target Python token metadata",
        );
    }
    table
}

fn token_payload(py: &crate::PyToken<'_>, table: &TokenTable) -> Option<u64> {
    let constant_count = table.constants().count();
    let mut owned_names = Vec::with_capacity(constant_count);
    let mut pairs = Vec::with_capacity((constant_count + 3) * 2);
    let mut names = Vec::with_capacity(constant_count * 2);
    for (name, code) in table.constants() {
        let name = token_allocation(py, crate::alloc_string(py, name.as_bytes()))?;
        let value = MoltObject::from_int(code).bits();
        pairs.extend_from_slice(&[name.bits(), value]);
        names.extend_from_slice(&[value, name.bits()]);
        owned_names.push(name);
    }
    let tok_name = token_allocation(py, crate::alloc_dict_with_pairs(py, &names))?;
    let mut symbols = Vec::with_capacity(table.exact_types().count());
    let mut exact_pairs = Vec::with_capacity(symbols.capacity() * 2);
    for (symbol, code) in table.exact_types() {
        let symbol = token_allocation(py, crate::alloc_string(py, symbol.as_bytes()))?;
        exact_pairs.extend_from_slice(&[symbol.bits(), MoltObject::from_int(code).bits()]);
        symbols.push(symbol);
    }
    let exact = token_allocation(py, crate::alloc_dict_with_pairs(py, &exact_pairs))?;
    let mut base_names = Vec::with_capacity(table.base_exports().len());
    let mut exports = Vec::with_capacity(base_names.capacity() + constant_count);
    for name in table.base_exports() {
        let name = token_allocation(py, crate::alloc_string(py, name.as_bytes()))?;
        exports.push(name.bits());
        base_names.push(name);
    }
    exports.extend(owned_names.iter().map(ExceptionValue::bits));
    let all = token_allocation(py, crate::alloc_list(py, &exports))?;
    let mut keys = Vec::with_capacity(3);
    for (name, value) in [
        ("tok_name", tok_name.bits()),
        ("EXACT_TOKEN_TYPES", exact.bits()),
        ("__all__", all.bits()),
    ] {
        let key = token_allocation(py, crate::alloc_string(py, name.as_bytes()))?;
        pairs.extend_from_slice(&[key.bits(), value]);
        keys.push(key);
    }
    token_allocation(py, crate::alloc_dict_with_pairs(py, &pairs)).map(ExceptionValue::into_bits)
}

/// Project the selected target's constants and public maps without runtime JSON.
#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::exception_pending(py) {
            return MoltObject::none().bits();
        }
        target_token_table(py)
            .and_then(|table| token_payload(py, table))
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

/// Tokenize source; scanner identities are projected through the same target table.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_scan(source_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(table) = target_token_table(py) else {
            return MoltObject::none().bits();
        };
        let Some(source) = crate::string_obj_to_owned(crate::obj_from_bits(source_bits)) else {
            return crate::raise_exception::<_>(py, "TypeError", "source must be str");
        };
        let scan = scan_tokens(&source);
        let mut lines = Vec::with_capacity(scan.lines.len());
        for line in &scan.lines {
            let Some(line) = token_allocation(py, crate::alloc_string(py, line.as_bytes())) else {
                return MoltObject::none().bits();
            };
            lines.push(line);
        }
        let mut tokens = Vec::with_capacity(scan.tokens.len());
        for token in &scan.tokens {
            let Some(code) = table.number(token.kind) else {
                return crate::raise_exception::<_>(
                    py,
                    "RuntimeError",
                    "scanner emitted a token absent from target Python",
                );
            };
            let Some(line) = lines.get(token.line_index) else {
                return crate::raise_exception::<_>(
                    py,
                    "RuntimeError",
                    "scanner emitted an invalid source line",
                );
            };
            let Some(tuple) =
                make_token_tuple(py, code, &token.text, token.start, token.end, line.bits())
            else {
                return MoltObject::none().bits();
            };
            tokens.push(ExceptionValue::adopt(py, tuple));
        }
        let bits: Vec<_> = tokens.iter().map(ExceptionValue::bits).collect();
        token_allocation(py, crate::alloc_list(py, &bits))
            .map(ExceptionValue::into_bits)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

fn make_token_tuple(
    py: &crate::PyToken<'_>,
    tok_type: i64,
    string: &str,
    start: (i64, i64),
    end: (i64, i64),
    line_bits: u64,
) -> Option<u64> {
    if crate::exception_pending(py) {
        return None;
    }
    let string = token_allocation(py, crate::alloc_string(py, string.as_bytes()))?;
    let start = token_allocation(
        py,
        crate::alloc_tuple(
            py,
            &[
                MoltObject::from_int(start.0).bits(),
                MoltObject::from_int(start.1).bits(),
            ],
        ),
    )?;
    let end = token_allocation(
        py,
        crate::alloc_tuple(
            py,
            &[
                MoltObject::from_int(end.0).bits(),
                MoltObject::from_int(end.1).bits(),
            ],
        ),
    )?;
    let elems = [
        MoltObject::from_int(tok_type).bits(),
        string.bits(),
        start.bits(),
        end.bits(),
        line_bits,
    ];
    token_allocation(py, crate::alloc_tuple(py, &elems)).map(ExceptionValue::into_bits)
}

/// Detect source encoding using the first two byte lines.
#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_detect_encoding(first_bits: u64, second_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::exception_pending(py) {
            return MoltObject::none().bits();
        }
        let first_obj = crate::obj_from_bits(first_bits);
        let second_obj = crate::obj_from_bits(second_bits);
        let first_bytes = first_obj
            .as_ptr()
            .and_then(|ptr| unsafe { crate::bytes_like_slice(ptr) })
            .unwrap_or(&[]);
        let second_bytes = second_obj
            .as_ptr()
            .and_then(|ptr| unsafe { crate::bytes_like_slice(ptr) })
            .unwrap_or(&[]);
        let detection = detect_source_encoding(first_bytes, second_bytes);
        encoding_detection_tuple(py, &detection.encoding, detection.bom_found)
    })
}

fn encoding_detection_tuple(py: &crate::PyToken<'_>, encoding: &str, bom_found: bool) -> u64 {
    if crate::exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some(encoding) = token_allocation(py, crate::alloc_string(py, encoding.as_bytes())) else {
        return MoltObject::none().bits();
    };
    let elems = [encoding.bits(), MoltObject::from_bool(bom_found).bits()];
    token_allocation(py, crate::alloc_tuple(py, &elems))
        .map(ExceptionValue::into_bits)
        .unwrap_or_else(|| MoltObject::none().bits())
}

#[cfg(test)]
#[path = "tokenize_tests.rs"]
mod tests;
