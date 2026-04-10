// Textwrap implementation (textwrap module support).
// Extracted from functions.rs for compilation-unit size reduction and tree shaking.

use super::functions::*;
use crate::*;
use molt_obj_model::MoltObject;

pub(crate) struct TextWrapOptions {
    width: i64,
    initial_indent: String,
    subsequent_indent: String,
    expand_tabs: bool,
    replace_whitespace: bool,
    fix_sentence_endings: bool,
    break_long_words: bool,
    drop_whitespace: bool,
    break_on_hyphens: bool,
    tabsize: i64,
    max_lines: Option<i64>,
    placeholder: String,
}

pub(crate) fn textwrap_default_options(width: i64) -> TextWrapOptions {
    TextWrapOptions {
        width,
        initial_indent: String::new(),
        subsequent_indent: String::new(),
        expand_tabs: true,
        replace_whitespace: true,
        fix_sentence_endings: false,
        break_long_words: true,
        drop_whitespace: true,
        break_on_hyphens: true,
        tabsize: 8,
        max_lines: None,
        placeholder: " [...]".to_string(),
    }
}

#[inline]
pub(crate) fn textwrap_char_len(value: &str) -> i64 {
    value.chars().count() as i64
}

#[inline]
pub(crate) fn textwrap_is_ascii_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

#[inline]
pub(crate) fn textwrap_is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[inline]
pub(crate) fn textwrap_is_word_punct(ch: char) -> bool {
    textwrap_is_word_char(ch) || matches!(ch, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

#[inline]
pub(crate) fn textwrap_is_letter(ch: char) -> bool {
    ch.is_alphabetic()
}

#[inline]
pub(crate) fn textwrap_chunk_is_whitespace(chunk: &str) -> bool {
    chunk.chars().all(char::is_whitespace)
}

#[inline]
pub(crate) fn textwrap_normalize_index(len: usize, idx: i64) -> usize {
    let len_i64 = len as i64;
    let mut normalized = if idx < 0 {
        len_i64.saturating_add(idx)
    } else {
        idx
    };
    if normalized < 0 {
        normalized = 0;
    }
    if normalized > len_i64 {
        normalized = len_i64;
    }
    normalized as usize
}

pub(crate) fn textwrap_slice_prefix(value: &str, end: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let end = textwrap_normalize_index(chars.len(), end);
    chars[..end].iter().collect()
}

pub(crate) fn textwrap_slice_suffix(value: &str, start: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = textwrap_normalize_index(chars.len(), start);
    chars[start..].iter().collect()
}

pub(crate) fn textwrap_rfind_before(value: &str, needle: char, stop: i64) -> Option<usize> {
    let chars: Vec<char> = value.chars().collect();
    let stop = textwrap_normalize_index(chars.len(), stop);
    chars[..stop].iter().rposition(|ch| *ch == needle)
}

pub(crate) fn textwrap_expand_tabs(text: &str, tabsize: i64) -> String {
    let tabsize = tabsize.max(0) as usize;
    let mut out = String::with_capacity(text.len());
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            if tabsize == 0 {
                continue;
            }
            let spaces = tabsize - (col % tabsize);
            out.extend(std::iter::repeat_n(' ', spaces));
            col = col.saturating_add(spaces);
            continue;
        }
        out.push(ch);
        if matches!(ch, '\n' | '\r') {
            col = 0;
        } else {
            col = col.saturating_add(1);
        }
    }
    out
}

pub(crate) fn textwrap_munge_whitespace(text: &str, options: &TextWrapOptions) -> String {
    let mut out = if options.expand_tabs {
        textwrap_expand_tabs(text, options.tabsize)
    } else {
        text.to_string()
    };
    if options.replace_whitespace {
        out = out
            .chars()
            .map(|ch| {
                if textwrap_is_ascii_whitespace(ch) {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
    }
    out
}

pub(crate) fn textwrap_split_simple(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        let is_ws = textwrap_is_ascii_whitespace(chars[idx]);
        let start = idx;
        idx += 1;
        while idx < chars.len() && textwrap_is_ascii_whitespace(chars[idx]) == is_ws {
            idx += 1;
        }
        chunks.push(chars[start..idx].iter().collect());
    }
    chunks
}

pub(crate) fn textwrap_should_split_hyphen(chars: &[char], idx: usize) -> bool {
    if chars.get(idx).copied() != Some('-') {
        return false;
    }
    let left_ok =
        (idx >= 2 && textwrap_is_letter(chars[idx - 2]) && textwrap_is_letter(chars[idx - 1]))
            || (idx >= 3
                && textwrap_is_letter(chars[idx - 3])
                && chars[idx - 2] == '-'
                && textwrap_is_letter(chars[idx - 1]));
    if !left_ok {
        return false;
    }
    (idx + 2 < chars.len()
        && textwrap_is_letter(chars[idx + 1])
        && textwrap_is_letter(chars[idx + 2]))
        || (idx + 3 < chars.len()
            && textwrap_is_letter(chars[idx + 1])
            && chars[idx + 2] == '-'
            && textwrap_is_letter(chars[idx + 3]))
}

pub(crate) fn textwrap_hyphen_run(chars: &[char], idx: usize) -> usize {
    let mut run = 0usize;
    while idx + run < chars.len() && chars[idx + run] == '-' {
        run += 1;
    }
    run
}

pub(crate) fn textwrap_split_hyphenated_token(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < chars.len() {
        let dash_run = textwrap_hyphen_run(&chars, idx);
        if dash_run >= 2
            && idx > 0
            && idx + dash_run < chars.len()
            && textwrap_is_word_punct(chars[idx - 1])
            && textwrap_is_word_char(chars[idx + dash_run])
        {
            if start < idx {
                out.push(chars[start..idx].iter().collect());
            }
            out.push(chars[idx..idx + dash_run].iter().collect());
            idx += dash_run;
            start = idx;
            continue;
        }
        if textwrap_should_split_hyphen(&chars, idx) {
            idx += 1;
            if start < idx {
                out.push(chars[start..idx].iter().collect());
            }
            start = idx;
            continue;
        }
        idx += 1;
    }
    if start < chars.len() {
        out.push(chars[start..].iter().collect());
    }
    out
}

pub(crate) fn textwrap_split_chunks(text: &str, break_on_hyphens: bool) -> Vec<String> {
    if !break_on_hyphens {
        return textwrap_split_simple(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        if textwrap_is_ascii_whitespace(chars[idx]) {
            let start = idx;
            idx += 1;
            while idx < chars.len() && textwrap_is_ascii_whitespace(chars[idx]) {
                idx += 1;
            }
            chunks.push(chars[start..idx].iter().collect());
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < chars.len() && !textwrap_is_ascii_whitespace(chars[idx]) {
            idx += 1;
        }
        let token: String = chars[start..idx].iter().collect();
        chunks.extend(textwrap_split_hyphenated_token(&token));
    }
    chunks
}

pub(crate) fn textwrap_chunk_has_sentence_end(chunk: &str) -> bool {
    let chars: Vec<char> = chunk.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let mut idx = chars.len();
    if matches!(chars[idx - 1], '"' | '\'') {
        idx -= 1;
        if idx < 2 {
            return false;
        }
    }
    matches!(chars[idx - 1], '.' | '!' | '?') && chars[idx - 2].is_ascii_lowercase()
}

pub(crate) fn textwrap_fix_sentence_endings(chunks: &mut [String]) {
    let mut idx = 0usize;
    while idx + 1 < chunks.len() {
        if chunks[idx + 1] == " " && textwrap_chunk_has_sentence_end(&chunks[idx]) {
            chunks[idx + 1] = "  ".to_string();
            idx += 2;
        } else {
            idx += 1;
        }
    }
}

pub(crate) fn textwrap_handle_long_word(
    chunks: &mut Vec<String>,
    cur_line: &mut Vec<String>,
    cur_len: i64,
    width: i64,
    break_long_words: bool,
    break_on_hyphens: bool,
) {
    let space_left = if width < 1 { 1 } else { width - cur_len };
    if break_long_words {
        let mut end = space_left;
        if let Some(chunk) = chunks.last_mut() {
            if break_on_hyphens
                && textwrap_char_len(chunk) > space_left
                && let Some(hyphen) = textwrap_rfind_before(chunk, '-', space_left)
                && hyphen > 0
                && chunk.chars().take(hyphen).any(|ch| ch != '-')
            {
                end = hyphen as i64 + 1;
            }
            let left = textwrap_slice_prefix(chunk, end);
            let right = textwrap_slice_suffix(chunk, end);
            cur_line.push(left);
            *chunk = right;
        }
    } else if cur_line.is_empty()
        && let Some(chunk) = chunks.pop()
    {
        cur_line.push(chunk);
    }
}

pub(crate) fn textwrap_wrap_chunks(
    mut chunks: Vec<String>,
    options: &TextWrapOptions,
) -> Result<Vec<String>, String> {
    if options.width <= 0 {
        return Err(format!("invalid width {:?} (must be > 0)", options.width));
    }
    if let Some(max_lines) = options.max_lines {
        let indent = if max_lines > 1 {
            &options.subsequent_indent
        } else {
            &options.initial_indent
        };
        let placeholder_lstrip = options.placeholder.trim_start_matches(char::is_whitespace);
        if textwrap_char_len(indent) + textwrap_char_len(placeholder_lstrip) > options.width {
            return Err("placeholder too large for max width".to_string());
        }
    }

    let mut lines: Vec<String> = Vec::new();
    chunks.reverse();

    while !chunks.is_empty() {
        let mut cur_line: Vec<String> = Vec::new();
        let mut cur_len = 0i64;
        let indent = if lines.is_empty() {
            &options.initial_indent
        } else {
            &options.subsequent_indent
        };
        let width = options.width - textwrap_char_len(indent);

        if options.drop_whitespace
            && !chunks.is_empty()
            && !lines.is_empty()
            && chunks
                .last()
                .map(|chunk| textwrap_chunk_is_whitespace(chunk))
                .unwrap_or(false)
        {
            chunks.pop();
        }

        while let Some(last) = chunks.last() {
            let last_len = textwrap_char_len(last);
            if cur_len + last_len <= width {
                cur_len += last_len;
                if let Some(chunk) = chunks.pop() {
                    cur_line.push(chunk);
                }
            } else {
                break;
            }
        }

        if !chunks.is_empty()
            && chunks
                .last()
                .map(|chunk| textwrap_char_len(chunk) > width)
                .unwrap_or(false)
        {
            textwrap_handle_long_word(
                &mut chunks,
                &mut cur_line,
                cur_len,
                width,
                options.break_long_words,
                options.break_on_hyphens,
            );
            cur_len = cur_line.iter().map(|chunk| textwrap_char_len(chunk)).sum();
        }

        if options.drop_whitespace
            && !cur_line.is_empty()
            && cur_line
                .last()
                .map(|chunk| textwrap_chunk_is_whitespace(chunk))
                .unwrap_or(false)
            && let Some(last) = cur_line.pop()
        {
            cur_len -= textwrap_char_len(&last);
        }

        if cur_line.is_empty() {
            continue;
        }

        let allow_full_line = if let Some(max_lines) = options.max_lines {
            (lines.len() as i64 + 1) < max_lines
                || ((chunks.is_empty()
                    || (options.drop_whitespace
                        && chunks.len() == 1
                        && textwrap_chunk_is_whitespace(&chunks[0])))
                    && cur_len <= width)
        } else {
            true
        };

        if allow_full_line {
            lines.push(format!("{indent}{}", cur_line.concat()));
            continue;
        }

        let placeholder_len = textwrap_char_len(&options.placeholder);
        loop {
            let can_append_placeholder = cur_line
                .last()
                .map(|last| {
                    !textwrap_chunk_is_whitespace(last) && cur_len + placeholder_len <= width
                })
                .unwrap_or(false);
            if can_append_placeholder {
                cur_line.push(options.placeholder.clone());
                lines.push(format!("{indent}{}", cur_line.concat()));
                break;
            }
            if let Some(last) = cur_line.pop() {
                cur_len -= textwrap_char_len(&last);
                continue;
            }
            if let Some(prev_line) = lines.last_mut() {
                let trimmed = prev_line.trim_end_matches(char::is_whitespace).to_string();
                if textwrap_char_len(&trimmed) + placeholder_len <= options.width {
                    *prev_line = trimmed + &options.placeholder;
                    return Ok(lines);
                }
            }
            let placeholder_lstrip = options.placeholder.trim_start_matches(char::is_whitespace);
            lines.push(format!("{indent}{placeholder_lstrip}"));
            break;
        }
        break;
    }

    Ok(lines)
}

pub(crate) fn textwrap_wrap_impl(
    text: &str,
    options: &TextWrapOptions,
) -> Result<Vec<String>, String> {
    let munged = textwrap_munge_whitespace(text, options);
    let mut chunks = textwrap_split_chunks(&munged, options.break_on_hyphens);
    if options.fix_sentence_endings {
        textwrap_fix_sentence_endings(&mut chunks);
    }
    textwrap_wrap_chunks(chunks, options)
}

pub(crate) fn textwrap_line_is_space(line: &str) -> bool {
    !line.is_empty() && line.chars().all(char::is_whitespace)
}

pub(crate) fn textwrap_splitlines_keepends(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut line_start = 0usize;
    let mut iter = text.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        let mut end = idx + ch.len_utf8();
        let is_break = match ch {
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => true,
            '\r' => {
                if let Some((next_idx, next_ch)) = iter.peek().copied()
                    && next_ch == '\n'
                {
                    end = next_idx + next_ch.len_utf8();
                    iter.next();
                }
                true
            }
            _ => false,
        };
        if is_break {
            out.push(text[line_start..end].to_string());
            line_start = end;
        }
    }
    if line_start < text.len() {
        out.push(text[line_start..].to_string());
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn textwrap_parse_options_ex(
    _py: &crate::PyToken<'_>,
    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> Result<TextWrapOptions, u64> {
    let Some(width) = to_i64(obj_from_bits(width_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "width must be int",
        ));
    };
    let Some(initial_indent) = string_obj_to_owned(obj_from_bits(initial_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "initial_indent must be str",
        ));
    };
    let Some(subsequent_indent) = string_obj_to_owned(obj_from_bits(subsequent_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "subsequent_indent must be str",
        ));
    };
    let Some(tabsize) = to_i64(obj_from_bits(tabsize_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "tabsize must be int",
        ));
    };
    let Some(max_lines_placeholder_ptr) = obj_from_bits(max_lines_placeholder_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    };
    if unsafe { object_type_id(max_lines_placeholder_ptr) } != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_placeholder = unsafe { seq_vec_ref(max_lines_placeholder_ptr) };
    if max_lines_placeholder.len() != 2 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_bits = max_lines_placeholder[0];
    let placeholder_bits = max_lines_placeholder[1];

    let max_lines = if obj_from_bits(max_lines_bits).is_none() {
        None
    } else {
        let Some(value) = to_i64(obj_from_bits(max_lines_bits)) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "max_lines must be int or None",
            ));
        };
        Some(value)
    };
    let Some(placeholder) = string_obj_to_owned(obj_from_bits(placeholder_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "placeholder must be str",
        ));
    };
    Ok(TextWrapOptions {
        width,
        initial_indent,
        subsequent_indent,
        expand_tabs: is_truthy(_py, obj_from_bits(expand_tabs_bits)),
        replace_whitespace: is_truthy(_py, obj_from_bits(replace_whitespace_bits)),
        fix_sentence_endings: is_truthy(_py, obj_from_bits(fix_sentence_endings_bits)),
        break_long_words: is_truthy(_py, obj_from_bits(break_long_words_bits)),
        drop_whitespace: is_truthy(_py, obj_from_bits(drop_whitespace_bits)),
        break_on_hyphens: is_truthy(_py, obj_from_bits(break_on_hyphens_bits)),
        tabsize,
        max_lines,
        placeholder,
    })
}

pub(crate) fn textwrap_indent_with_predicate(
    _py: &crate::PyToken<'_>,
    text: &str,
    prefix: &str,
    predicate_bits: Option<u64>,
) -> u64 {
    let mut out = String::with_capacity(text.len().saturating_add(prefix.len() * 4));
    for line in textwrap_splitlines_keepends(text) {
        let should_prefix = if let Some(predicate) = predicate_bits {
            let Some(line_bits) = alloc_string_bits(_py, &line) else {
                return MoltObject::none().bits();
            };
            let result_bits = unsafe { call_callable1(_py, predicate, line_bits) };
            dec_ref_bits(_py, line_bits);
            if exception_pending(_py) {
                if !obj_from_bits(result_bits).is_none() {
                    dec_ref_bits(_py, result_bits);
                }
                return MoltObject::none().bits();
            }
            let truthy = is_truthy(_py, obj_from_bits(result_bits));
            if !obj_from_bits(result_bits).is_none() {
                dec_ref_bits(_py, result_bits);
            }
            truthy
        } else {
            !textwrap_line_is_space(&line)
        };
        if should_prefix {
            out.push_str(prefix);
        }
        out.push_str(&line);
    }
    let out_ptr = alloc_string(_py, out.as_bytes());
    if out_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(out_ptr).bits()
    }
}

// ─── textwrap.dedent ────────────────────────────────────────────────────────

pub(crate) fn textwrap_dedent_impl(text: &str) -> String {
    // CPython textwrap.dedent: remove common leading whitespace from all lines.
    let mut margin: Option<&str> = None;
    let lines: Vec<&str> = text.split('\n').collect();
    for &line in &lines {
        let stripped = line.trim_start();
        if stripped.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - stripped.len()];
        if let Some(m) = margin {
            // Find common prefix between margin and indent
            let common_len = m
                .chars()
                .zip(indent.chars())
                .take_while(|(a, b)| a == b)
                .count();
            // Need byte length of common prefix
            let byte_len = m
                .char_indices()
                .nth(common_len)
                .map(|(i, _)| i)
                .unwrap_or(m.len());
            margin = Some(&m[..byte_len]);
        } else {
            margin = Some(indent);
        }
    }
    let margin = margin.unwrap_or("");
    if margin.is_empty() {
        return text.to_string();
    }
    let margin_len = margin.len();
    let mut result = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.trim_start().is_empty() {
            // Whitespace-only line: strip all leading whitespace
            result.push_str(line.trim_start());
        } else if line.len() >= margin_len && &line[..margin_len] == margin {
            result.push_str(&line[margin_len..]);
        } else {
            result.push_str(line);
        }
    }
    result
}
