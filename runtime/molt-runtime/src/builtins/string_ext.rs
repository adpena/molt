// === FILE: runtime/molt-runtime/src/builtins/string_ext.rs ===
//
// Intrinsics for Python `string` module: Template $-substitution scanning,
// Formatter parse, and field name splitting.
//
// These are pure string-processing operations — no Python callbacks.
// The Python wrapper handles mapping lookups and method dispatch.

use crate::*;

// ─────────────────────────────────────────────────────────────────────────────
// Template lexical authority: default ASCII identifiers and literal delimiters.
// Custom subclass regex patterns/flags are outside this token protocol.
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn is_identifier_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

#[inline]
fn is_identifier_continue(b: u8) -> bool {
    is_identifier_start(b) || b.is_ascii_digit()
}

fn scan_identifier(text: &[u8], start: usize) -> Option<(usize, usize)> {
    if start >= text.len() || !is_identifier_start(text[start]) {
        return None;
    }
    let mut end = start + 1;
    while end < text.len() && is_identifier_continue(text[end]) {
        end += 1;
    }
    Some((start, end))
}

fn scan_template_name(text: &[u8], start: usize) -> Option<(usize, usize, usize)> {
    if text.get(start) == Some(&b'{') {
        scan_identifier(text, start + 1).and_then(|(name_start, end)| {
            (text.get(end) == Some(&b'}')).then_some((name_start, end, end + 1))
        })
    } else {
        scan_identifier(text, start).map(|(name_start, end)| (name_start, end, end))
    }
}

enum TemplateToken<'a> {
    Literal(&'a str),
    Placeholder {
        literal: &'a str,
        name: &'a str,
        original: &'a str,
    },
    Escaped {
        literal: &'a str,
        original: &'a str,
    },
    Invalid {
        literal: &'a str,
        original: &'a str,
        character: usize,
    },
}

impl<'a> TemplateToken<'a> {
    fn fields(self) -> (&'a str, Option<&'a str>, Option<&'a str>, Option<usize>) {
        match self {
            Self::Literal(text) => (text, None, None, None),
            Self::Placeholder {
                literal,
                name,
                original,
            } => (literal, Some(name), Some(original), None),
            Self::Escaped { literal, original } => (literal, None, Some(original), None),
            Self::Invalid {
                literal,
                original,
                character,
            } => (literal, None, Some(original), Some(character)),
        }
    }
}

struct TemplateScanner<'a> {
    text: &'a str,
    delimiter: &'a str,
    offset: usize,
    characters: usize,
    retry_nonempty: bool,
}

impl<'a> TemplateScanner<'a> {
    fn new(text: &'a str, delimiter: &'a str) -> Self {
        Self {
            text,
            delimiter,
            offset: 0,
            characters: 0,
            retry_nonempty: false,
        }
    }

    fn advance(&mut self, end: usize) {
        // Count each consumed slice once. Error coordinates are Python character
        // offsets, never repeated prefix counts or UTF-8 byte offsets.
        self.characters += self.text[self.offset..end].chars().count();
        self.offset = end;
    }
}

impl<'a> Iterator for TemplateScanner<'a> {
    type Item = TemplateToken<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text;
        let start = self.offset;
        if self.delimiter.is_empty() {
            // After an empty escaped match, re allows a nonempty named/braced
            // match at the same position. If neither matches, it advances one
            // character before trying the next empty escape, including at EOF.
            if !self.retry_nonempty {
                self.retry_nonempty = true;
                return Some(TemplateToken::Escaped {
                    literal: "",
                    original: "",
                });
            }
            if let Some((name_start, name_end, end)) = scan_template_name(text.as_bytes(), start) {
                self.advance(end);
                self.retry_nonempty = false;
                return Some(TemplateToken::Placeholder {
                    literal: "",
                    name: &text[name_start..name_end],
                    original: &text[start..end],
                });
            }
            let character = text[start..].chars().next()?;
            let end = start + character.len_utf8();
            self.advance(end);
            self.retry_nonempty = false;
            return Some(TemplateToken::Literal(&text[start..end]));
        }
        if start == text.len() {
            return None;
        }
        // str::find uses the standard linear substring search. Every successful
        // search consumes its delimiter; there is no unbounded suffix rescan.
        let Some(relative) = text[start..].find(self.delimiter) else {
            self.advance(text.len());
            return Some(TemplateToken::Literal(&text[start..]));
        };
        let delimiter_start = start + relative;
        let after = delimiter_start + self.delimiter.len();
        let literal = &text[start..delimiter_start];
        if text[after..].starts_with(self.delimiter) {
            let end = after + self.delimiter.len();
            self.advance(end);
            return Some(TemplateToken::Escaped {
                literal,
                original: &text[delimiter_start..end],
            });
        }
        if let Some((name_start, name_end, end)) = scan_template_name(text.as_bytes(), after) {
            self.advance(end);
            return Some(TemplateToken::Placeholder {
                literal,
                name: &text[name_start..name_end],
                original: &text[delimiter_start..end],
            });
        }
        // CPython's invalid match consumes only the delimiter. Leave subsequent
        // text (including other placeholders) for the next token, and report
        // the empty invalid group's position immediately AFTER the delimiter.
        self.advance(after);
        Some(TemplateToken::Invalid {
            literal,
            original: &text[delimiter_start..after],
            character: self.characters,
        })
    }
}

fn template_token_tuple(py: &PyToken<'_>, token: TemplateToken<'_>) -> Option<(u64, PtrDropGuard)> {
    let (literal, name, original, character) = token.fields();
    let none = MoltObject::none().bits();
    let mut fields = [none; 4];
    let mut owners: [PtrDropGuard; 4] =
        std::array::from_fn(|_| PtrDropGuard::new(std::ptr::null_mut()));
    for (index, text) in [Some(literal), name, original].into_iter().enumerate() {
        if let Some(text) = text {
            let ptr = alloc_string(py, text.as_bytes());
            if ptr.is_null() {
                return None;
            }
            owners[index] = PtrDropGuard::new(ptr);
            fields[index] = MoltObject::from_ptr(ptr).bits();
        }
    }
    if let Some(character) = character {
        fields[3] = int_bits_from_i64(py, character as i64);
        owners[3] = PtrDropGuard::new(
            obj_from_bits(fields[3])
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        if exception_pending(py) {
            return None;
        }
    }
    let ptr = alloc_tuple(py, &fields);
    if ptr.is_null() {
        return None;
    }
    Some((MoltObject::from_ptr(ptr).bits(), PtrDropGuard::new(ptr)))
}

/// Return (literal, name, original, invalid_character_offset) token tuples.
/// A non-None invalid offset is explicit syntax failure; a name is a lookup;
/// an original without either denotes an escaped delimiter. Only literal
/// tokens have neither name, original nor invalid offset. The Python owner
/// chooses strict/safe behavior and performs every lookup/str callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_scan(template_bits: u64, delimiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(py, "TypeError", "delimiter must be str");
        };
        let mut segments = Vec::new();
        let mut owners = Vec::new();
        for token in TemplateScanner::new(&template, &delimiter) {
            let Some((bits, owner)) = template_token_tuple(py, token) else {
                return MoltObject::none().bits();
            };
            segments.push(bits);
            owners.push(owner);
        }
        let ptr = alloc_list(py, &segments);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_is_valid(template_bits: u64, delimiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(py, "TypeError", "delimiter must be str");
        };
        MoltObject::from_bool(
            TemplateScanner::new(&template, &delimiter)
                .all(|token| !matches!(token, TemplateToken::Invalid { .. })),
        )
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_template_get_identifiers(
    template_bits: u64,
    delimiter_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(template) = string_obj_to_owned(obj_from_bits(template_bits)) else {
            return raise_exception::<_>(py, "TypeError", "template must be str");
        };
        let Some(delimiter) = string_obj_to_owned(obj_from_bits(delimiter_bits)) else {
            return raise_exception::<_>(py, "TypeError", "delimiter must be str");
        };
        let mut seen = std::collections::HashSet::new();
        let mut identifiers = Vec::new();
        let mut owners = Vec::new();
        for token in TemplateScanner::new(&template, &delimiter) {
            if let TemplateToken::Placeholder { name, .. } = token
                && seen.insert(name)
            {
                let ptr = alloc_string(py, name.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                owners.push(PtrDropGuard::new(ptr));
                identifiers.push(MoltObject::from_ptr(ptr).bits());
            }
        }
        let ptr = alloc_list(py, &identifiers);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatter projections of the canonical native field parser
// ─────────────────────────────────────────────────────────────────────────────

use crate::object::ops_string::ops_string_format::{
    FormatName, next_format_lookup, next_format_markup, split_format_field_name,
};

fn formatter_string_ptr(py: &PyToken<'_>, bits: u64) -> Option<*mut u8> {
    let Some(ptr) = obj_from_bits(bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })
    else {
        let message = format!("expected str, got {}", type_name(py, obj_from_bits(bits)));
        return raise_exception(py, "TypeError", &message);
    };
    Some(ptr)
}

fn formatter_string(py: &PyToken<'_>, text: &[u8]) -> Option<(u64, PtrDropGuard)> {
    let ptr = alloc_string(py, text);
    if ptr.is_null() {
        return None;
    }
    Some((MoltObject::from_ptr(ptr).bits(), PtrDropGuard::new(ptr)))
}

fn formatter_name(py: &PyToken<'_>, name: FormatName<'_>) -> Option<(u64, PtrDropGuard)> {
    let Some(index) = name.index else {
        return formatter_string(py, name.text);
    };
    let bits = int_bits_from_i64(py, index as i64);
    let owner = PtrDropGuard::new(obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut()));
    if exception_pending(py) {
        return None;
    }
    Some((bits, owner))
}

fn formatter_tuple(py: &PyToken<'_>, items: &[u64]) -> u64 {
    let ptr = alloc_tuple(py, items);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Reuse the callable iterator's ownership and exhaustion machinery. Its bound
/// native next function owns (immutable source, offset cell); no eager token
/// list, foreign parser, new object layout, or Python callbacks are involved.
fn formatter_iterator(
    py: &PyToken<'_>,
    source_bits: u64,
    offset: usize,
    next: extern "C" fn(u64) -> u64,
    symbol: &str,
) -> u64 {
    let offset_bits = int_bits_from_i64(py, offset as i64);
    let _offset_owner = PtrDropGuard::new(
        obj_from_bits(offset_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let cell_ptr = crate::object::cells::alloc_cell(py, offset_bits);
    if cell_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _cell_owner = PtrDropGuard::new(cell_ptr);
    let state_ptr = alloc_tuple(py, &[source_bits, MoltObject::from_ptr(cell_ptr).bits()]);
    if state_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _state_owner = PtrDropGuard::new(state_ptr);
    let address = crate::builtins::functions::runtime_fn_addr(symbol, next as *const ());
    let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(py, address, 1);
    if function_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _function_owner = PtrDropGuard::new(function_ptr);
    let bound_ptr = alloc_bound_method_obj(
        py,
        MoltObject::from_ptr(function_ptr).bits(),
        MoltObject::from_ptr(state_ptr).bits(),
    );
    if bound_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _bound_owner = PtrDropGuard::new(bound_ptr);
    molt_iter_sentinel(
        MoltObject::from_ptr(bound_ptr).bits(),
        MoltObject::none().bits(),
    )
}

fn formatter_cursor(state_bits: u64) -> Option<(*mut u8, *mut u8, usize)> {
    let state_ptr = obj_from_bits(state_bits).as_ptr()?;
    let (source_bits, cell_bits) = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(state_ptr, |items| {
            (items.len() == 2).then(|| (items[0], items[1]))
        })
        .flatten()?
    };
    let source_ptr = obj_from_bits(source_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })?;
    let cell_ptr = crate::object::cells::cell_ptr_from_bits(cell_bits)?;
    let offset_bits = unsafe { crate::object::cells::cell_value_bits(cell_ptr) };
    let offset = usize::try_from(to_i64(obj_from_bits(offset_bits))?).ok()?;
    let text =
        unsafe { std::slice::from_raw_parts(string_bytes(source_ptr), string_len(source_ptr)) };
    // The callable's state cell can be changed from Python. Never let either
    // iterator expose a substring starting inside a WTF-8 code point.
    if offset > text.len() || (offset < text.len() && text[offset] & 0xc0 == 0x80) {
        return None;
    }
    Some((source_ptr, cell_ptr, offset))
}

fn formatter_set_offset(py: &PyToken<'_>, cell_ptr: *mut u8, offset: usize) -> bool {
    let bits = int_bits_from_i64(py, offset as i64);
    let _owner = PtrDropGuard::new(obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut()));
    if exception_pending(py) {
        return false;
    }
    unsafe { crate::object::cells::cell_replace_value(py, cell_ptr, bits) };
    true
}

extern "C" fn formatter_parse_next(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((source_ptr, cell_ptr, mut offset)) = formatter_cursor(state_bits) else {
            return raise_exception(py, "SystemError", "invalid format parser cursor");
        };
        let text =
            unsafe { std::slice::from_raw_parts(string_bytes(source_ptr), string_len(source_ptr)) };
        let next = next_format_markup(text, &mut offset);
        if !formatter_set_offset(py, cell_ptr, offset) {
            return MoltObject::none().bits();
        }
        let markup = match next {
            Ok(Some(markup)) => markup,
            Ok(None) => return MoltObject::none().bits(),
            Err(message) => return raise_exception(py, "ValueError", message),
        };
        let Some((literal, _literal_owner)) = formatter_string(py, markup.literal) else {
            return MoltObject::none().bits();
        };
        let none = MoltObject::none().bits();
        let Some(field) = markup.field else {
            return formatter_tuple(py, &[literal, none, none, none]);
        };
        let Some((name, _name_owner)) = formatter_string(py, field.field_name) else {
            return MoltObject::none().bits();
        };
        let Some((spec, _spec_owner)) = formatter_string(py, field.format_spec) else {
            return MoltObject::none().bits();
        };
        if field.conversion == 0 {
            return formatter_tuple(py, &[literal, name, spec, none]);
        }
        let Some((conversion, _conversion_owner)) = formatter_string(py, field.conversion_text)
        else {
            return MoltObject::none().bits();
        };
        formatter_tuple(py, &[literal, name, spec, conversion])
    })
}

extern "C" fn formatter_field_name_next(state_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((source_ptr, cell_ptr, mut offset)) = formatter_cursor(state_bits) else {
            return raise_exception(py, "SystemError", "invalid format field cursor");
        };
        let text =
            unsafe { std::slice::from_raw_parts(string_bytes(source_ptr), string_len(source_ptr)) };
        let next = next_format_lookup(crate::object::ops::unicode_data(py), text, &mut offset);
        if !formatter_set_offset(py, cell_ptr, offset) {
            return MoltObject::none().bits();
        }
        let lookup = match next {
            Ok(Some(lookup)) => lookup,
            Ok(None) => return MoltObject::none().bits(),
            Err(message) => return raise_exception(py, "ValueError", message),
        };
        let Some((name, _name_owner)) = formatter_name(py, lookup.name) else {
            return MoltObject::none().bits();
        };
        formatter_tuple(
            py,
            &[MoltObject::from_bool(lookup.is_attribute).bits(), name],
        )
    })
}

/// Return a lazy iterator of (literal, field_name, format_spec, conversion).
/// Trailing syntax errors surface only when the consumer reaches that token.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_formatter_parse(format_string_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if formatter_string_ptr(py, format_string_bits).is_none() {
            return MoltObject::none().bits();
        }
        formatter_iterator(
            py,
            format_string_bits,
            0,
            formatter_parse_next,
            "formatter_parse_next",
        )
    })
}

/// Split the first component immediately, then lazily project attribute/item
/// steps. Each preceding lookup may run before a later invalid suffix raises.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_formatter_field_name_split(field_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(source_ptr) = formatter_string_ptr(py, field_name_bits) else {
            return MoltObject::none().bits();
        };
        let text =
            unsafe { std::slice::from_raw_parts(string_bytes(source_ptr), string_len(source_ptr)) };
        let (first, offset) =
            match split_format_field_name(crate::object::ops::unicode_data(py), text) {
                Ok(first) => first,
                Err(message) => return raise_exception(py, "ValueError", message),
            };
        let Some((first_bits, _first_owner)) = formatter_name(py, first) else {
            return MoltObject::none().bits();
        };
        let rest_bits = formatter_iterator(
            py,
            field_name_bits,
            offset,
            formatter_field_name_next,
            "formatter_field_name_next",
        );
        let _rest_owner = PtrDropGuard::new(
            obj_from_bits(rest_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        formatter_tuple(py, &[first_bits, rest_bits])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_cursor_rejects_crafted_nonboundary_offsets() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // ASCII, two-byte, surrogate and four-byte code points all share
            // the same cursor admission rule, including the exhausted cursor.
            let text = b"a\xc3\xa9\xed\xa0\x80\xf0\xa0\xae\x80.z";
            let source_ptr = alloc_string(py, text);
            assert!(!source_ptr.is_null());
            let _source_owner = PtrDropGuard::new(source_ptr);
            let cell_ptr = crate::object::cells::alloc_cell(py, MoltObject::from_int(0).bits());
            assert!(!cell_ptr.is_null());
            let _cell_owner = PtrDropGuard::new(cell_ptr);
            let state_ptr = alloc_tuple(
                py,
                &[
                    MoltObject::from_ptr(source_ptr).bits(),
                    MoltObject::from_ptr(cell_ptr).bits(),
                ],
            );
            assert!(!state_ptr.is_null());
            let _state_owner = PtrDropGuard::new(state_ptr);
            let state_bits = MoltObject::from_ptr(state_ptr).bits();
            for offset in [0, 1, 3, 6, 10, 11, 12] {
                unsafe {
                    crate::object::cells::cell_replace_value(
                        py,
                        cell_ptr,
                        MoltObject::from_int(offset).bits(),
                    );
                }
                assert_eq!(
                    formatter_cursor(state_bits).map(|(_, _, offset)| offset),
                    Some(offset as usize)
                );
            }
            let system_error =
                crate::builtins::exceptions::exception_type_bits_from_name(py, "SystemError");
            let next_functions: [extern "C" fn(u64) -> u64; 2] =
                [formatter_parse_next, formatter_field_name_next];
            for offset in [-1, 2, 4, 5, 7, 8, 9, 13] {
                let offset_bits = MoltObject::from_int(offset).bits();
                unsafe { crate::object::cells::cell_replace_value(py, cell_ptr, offset_bits) };
                assert!(
                    formatter_cursor(state_bits).is_none(),
                    "admitted offset {offset}"
                );
                for next in next_functions {
                    let result = next(state_bits);
                    assert!(crate::builtins::exceptions::pending_exception_matches_type(
                        py,
                        system_error
                    ));
                    assert_eq!(result, MoltObject::none().bits());
                    assert_eq!(
                        unsafe { crate::object::cells::cell_value_bits(cell_ptr) },
                        offset_bits
                    );
                    clear_exception(py);
                }
            }
        });
    }
}
