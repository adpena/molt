//! Pure textwrap algorithms for the `textwrap` stdlib module.
//!
//! Runtime object/ABI shims stay in `molt-runtime`; this crate owns the
//! deterministic string/chunk/line algorithms so text stdlib code lives in the
//! text leaf crate instead of the runtime god-crate.

pub struct TextWrapOptions {
    pub width: i64,
    pub initial_indent: String,
    pub subsequent_indent: String,
    pub expand_tabs: bool,
    pub replace_whitespace: bool,
    pub fix_sentence_endings: bool,
    pub break_long_words: bool,
    pub drop_whitespace: bool,
    pub break_on_hyphens: bool,
    pub tabsize: i64,
    pub max_lines: Option<i64>,
    pub placeholder: String,
}

pub fn textwrap_default_options(width: i64) -> TextWrapOptions {
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
fn textwrap_char_len(value: &str) -> i64 {
    value.chars().count() as i64
}

#[inline]
fn textwrap_is_ascii_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

#[inline]
fn textwrap_is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[inline]
fn textwrap_is_word_punct(ch: char) -> bool {
    textwrap_is_word_char(ch) || matches!(ch, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

#[inline]
fn textwrap_is_letter(ch: char) -> bool {
    ch.is_alphabetic()
}

#[inline]
fn textwrap_chunk_is_whitespace(chunk: &str) -> bool {
    chunk.chars().all(char::is_whitespace)
}

#[inline]
fn textwrap_normalize_index(len: usize, idx: i64) -> usize {
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

fn textwrap_slice_prefix(value: &str, end: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let end = textwrap_normalize_index(chars.len(), end);
    chars[..end].iter().collect()
}

fn textwrap_slice_suffix(value: &str, start: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = textwrap_normalize_index(chars.len(), start);
    chars[start..].iter().collect()
}

fn textwrap_rfind_before(value: &str, needle: char, stop: i64) -> Option<usize> {
    let chars: Vec<char> = value.chars().collect();
    let stop = textwrap_normalize_index(chars.len(), stop);
    chars[..stop].iter().rposition(|ch| *ch == needle)
}

fn textwrap_expand_tabs(text: &str, tabsize: i64) -> String {
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

fn textwrap_munge_whitespace(text: &str, options: &TextWrapOptions) -> String {
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

fn textwrap_split_simple(text: &str) -> Vec<String> {
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

fn textwrap_should_split_hyphen(chars: &[char], idx: usize) -> bool {
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

fn textwrap_hyphen_run(chars: &[char], idx: usize) -> usize {
    let mut run = 0usize;
    while idx + run < chars.len() && chars[idx + run] == '-' {
        run += 1;
    }
    run
}

fn textwrap_split_hyphenated_token(token: &str) -> Vec<String> {
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

fn textwrap_split_chunks(text: &str, break_on_hyphens: bool) -> Vec<String> {
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

fn textwrap_chunk_has_sentence_end(chunk: &str) -> bool {
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

fn textwrap_fix_sentence_endings(chunks: &mut [String]) {
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

fn textwrap_handle_long_word(
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

fn textwrap_wrap_chunks(
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

pub fn textwrap_wrap_impl(text: &str, options: &TextWrapOptions) -> Result<Vec<String>, String> {
    let munged = textwrap_munge_whitespace(text, options);
    let mut chunks = textwrap_split_chunks(&munged, options.break_on_hyphens);
    if options.fix_sentence_endings {
        textwrap_fix_sentence_endings(&mut chunks);
    }
    textwrap_wrap_chunks(chunks, options)
}

pub fn textwrap_shorten_impl(text: &str, width: i64, placeholder: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
    if textwrap_char_len(&collapsed) <= width {
        return collapsed;
    }

    let max_text = width - textwrap_char_len(placeholder);
    if max_text < 0 {
        return placeholder.to_string();
    }

    let chars: Vec<char> = collapsed.chars().collect();
    let mut truncate_at = max_text as usize;
    if truncate_at < chars.len()
        && let Some(pos) = chars[..truncate_at].iter().rposition(|ch| *ch == ' ')
    {
        truncate_at = pos;
    }

    let prefix = chars[..truncate_at]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string();
    format!("{prefix}{placeholder}")
}

pub fn textwrap_line_is_space(line: &str) -> bool {
    !line.is_empty() && line.chars().all(char::is_whitespace)
}

pub fn textwrap_splitlines_keepends(text: &str) -> Vec<String> {
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

pub fn textwrap_indent_result_impl<E, F>(
    text: &str,
    prefix: &str,
    mut predicate: F,
) -> Result<String, E>
where
    F: FnMut(&str) -> Result<bool, E>,
{
    let mut out = String::with_capacity(text.len().saturating_add(prefix.len() * 4));
    for line in textwrap_splitlines_keepends(text) {
        if predicate(&line)? {
            out.push_str(prefix);
        }
        out.push_str(&line);
    }
    Ok(out)
}

pub fn textwrap_indent_impl<F>(text: &str, prefix: &str, mut predicate: F) -> String
where
    F: FnMut(&str) -> bool,
{
    match textwrap_indent_result_impl(text, prefix, |line| Ok::<bool, ()>(predicate(line))) {
        Ok(out) => out,
        Err(()) => unreachable!(),
    }
}

pub fn textwrap_indent_default_impl(text: &str, prefix: &str) -> String {
    textwrap_indent_impl(text, prefix, |line| !textwrap_line_is_space(line))
}

pub fn textwrap_dedent_impl(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_fill_width_contract() {
        let options = textwrap_default_options(5);
        let wrapped = textwrap_wrap_impl("hello world from molt", &options).unwrap();
        assert_eq!(wrapped, ["hello", "world", "from", "molt"]);
        assert_eq!(wrapped.join("\n"), "hello\nworld\nfrom\nmolt");
    }

    #[test]
    fn whitespace_options_match_textwrap_contract() {
        let mut drop_true = textwrap_default_options(6);
        drop_true.drop_whitespace = true;
        assert_eq!(
            textwrap_wrap_impl("a   b   c", &drop_true).unwrap(),
            ["a   b", "c"]
        );

        let mut drop_false = textwrap_default_options(6);
        drop_false.drop_whitespace = false;
        assert_eq!(
            textwrap_wrap_impl("a   b   c", &drop_false).unwrap(),
            ["a   b", "   c"]
        );

        let mut expand_true = textwrap_default_options(4);
        expand_true.expand_tabs = true;
        expand_true.replace_whitespace = false;
        assert_eq!(
            textwrap_wrap_impl("a\tb", &expand_true).unwrap(),
            ["a", "b"]
        );

        let mut expand_false = textwrap_default_options(4);
        expand_false.expand_tabs = false;
        expand_false.replace_whitespace = false;
        assert_eq!(textwrap_wrap_impl("a\tb", &expand_false).unwrap(), ["a\tb"]);
    }

    #[test]
    fn hyphen_and_max_lines_contracts() {
        let mut break_true = textwrap_default_options(8);
        break_true.break_on_hyphens = true;
        break_true.break_long_words = false;
        assert_eq!(
            textwrap_wrap_impl("alpha-beta-gamma", &break_true).unwrap(),
            ["alpha-", "beta-", "gamma"]
        );

        let mut break_false = textwrap_default_options(8);
        break_false.break_on_hyphens = false;
        break_false.break_long_words = false;
        assert_eq!(
            textwrap_wrap_impl("alpha-beta-gamma", &break_false).unwrap(),
            ["alpha-beta-gamma"]
        );

        let mut limited = textwrap_default_options(10);
        limited.max_lines = Some(2);
        limited.placeholder = " [...]".to_string();
        let wrapped = textwrap_wrap_impl("alpha beta gamma delta", &limited).unwrap();
        assert_eq!(wrapped, ["alpha beta", "[...]"]);
        assert_eq!(wrapped.join("\n"), "alpha beta\n[...]");
    }

    #[test]
    fn splitlines_indent_predicate_and_dedent_contracts() {
        let lines = textwrap_splitlines_keepends("line1\n\nline2");
        assert_eq!(lines, ["line1\n", "\n", "line2"]);
        assert!(!textwrap_line_is_space("line1\n"));
        assert!(textwrap_line_is_space("\n"));

        assert_eq!(
            textwrap_indent_default_impl("line1\n\nline2", "> "),
            "> line1\n\n> line2"
        );
        assert_eq!(
            textwrap_indent_impl("line1\n\nline2", "> ", |_| true),
            "> line1\n> \n> line2"
        );
        let fallible =
            textwrap_indent_result_impl("a\nb", "> ", |line| Ok::<bool, ()>(line.starts_with('b')))
                .unwrap();
        assert_eq!(fallible, "a\n> b");

        let dedented = textwrap_dedent_impl("    alpha\n      beta\n");
        assert_eq!(dedented, "alpha\n  beta\n");
    }

    #[test]
    fn shorten_contracts_live_in_text_authority() {
        assert_eq!(
            textwrap_shorten_impl("Hello  world!", 12, " [...]"),
            "Hello world!"
        );
        assert_eq!(
            textwrap_shorten_impl("Hello  world!", 11, " [...]"),
            "Hello [...]"
        );
        assert_eq!(textwrap_shorten_impl("alpha beta", 2, "..."), "...");
        assert_eq!(textwrap_shorten_impl("å β gamma", 5, "..."), "å...");
    }
}
