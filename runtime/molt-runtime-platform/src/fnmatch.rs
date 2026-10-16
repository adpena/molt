//! Pure fnmatch algorithms shared by Python fnmatch and platform path matching.
//!
//! Runtime object/ABI shims stay in `molt-runtime`; this module owns matching,
//! normalization, and regex translation without a dependency on the stdlib layer.

type ParsedClass<T> = (Vec<T>, Vec<(T, T)>, bool, usize);

fn parse_class<T: Copy + Ord + From<u8>>(pat: &[T], mut idx: usize) -> Option<ParsedClass<T>> {
    if idx >= pat.len() || pat[idx] != T::from(b'[') {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == T::from(b'!') {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<T> = Vec::new();
    let mut ranges: Vec<(T, T)> = Vec::new();

    if pat[idx] == T::from(b']') {
        singles.push(T::from(b']'));
        idx += 1;
    }

    while idx < pat.len() && pat[idx] != T::from(b']') {
        if idx + 2 < pat.len() && pat[idx + 1] == T::from(b'-') && pat[idx + 2] != T::from(b']') {
            let start = pat[idx];
            let end = pat[idx + 2];
            if start <= end {
                ranges.push((start, end));
            }
            idx += 3;
            continue;
        }
        singles.push(pat[idx]);
        idx += 1;
    }
    if idx >= pat.len() || pat[idx] != T::from(b']') {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn class_hit<T: Copy + Ord>(ch: T, singles: &[T], ranges: &[(T, T)], negate: bool) -> bool {
    let mut hit = singles.contains(&ch);
    if !hit {
        hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
    }
    if negate { !hit } else { hit }
}

/// Match Unicode code points or bytes without changing their coordinate system.
/// Bytes retain their borrowed input; Unicode callers supply Python code points,
/// including surrogate values which Rust `char` cannot represent.
pub fn fnmatch_match_units<T: Copy + Ord + From<u8>>(name: &[T], pat: &[T]) -> bool {
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name.len() {
        if pi < pat.len() && pat[pi] == T::from(b'*') {
            while pi < pat.len() && pat[pi] == T::from(b'*') {
                pi += 1;
            }
            if pi == pat.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat.len() && pat[pi] == T::from(b'?') {
            pi += 1;
            ni += 1;
            continue;
        }
        if pi < pat.len()
            && pat[pi] == T::from(b'[')
            && let Some((singles, ranges, negate, next_idx)) = parse_class(pat, pi)
        {
            let hit = class_hit(name[ni], &singles, &ranges, negate);
            if hit {
                pi = next_idx;
                ni += 1;
                continue;
            }
            if let Some(star) = star_idx {
                matched_from_star += 1;
                ni = matched_from_star;
                pi = star;
                continue;
            }
            return false;
        }
        if pi < pat.len() && pat[pi] == name[ni] {
            pi += 1;
            ni += 1;
            continue;
        }
        if let Some(star) = star_idx {
            matched_from_star += 1;
            ni = matched_from_star;
            pi = star;
            continue;
        }
        return false;
    }

    while pi < pat.len() && pat[pi] == T::from(b'*') {
        pi += 1;
    }
    pi == pat.len()
}

pub fn fnmatch_match_impl(name: &str, pat: &str) -> bool {
    fnmatch_match_units(
        &name.chars().collect::<Vec<_>>(),
        &pat.chars().collect::<Vec<_>>(),
    )
}

pub fn fnmatch_match_bytes_impl(name: &[u8], pat: &[u8]) -> bool {
    fnmatch_match_units(name, pat)
}

pub fn fnmatch_normcase_text(input: &str) -> String {
    if cfg!(windows) {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            if ch == '/' {
                out.push('\\');
            } else {
                out.extend(ch.to_lowercase());
            }
        }
        out
    } else {
        input.to_string()
    }
}

pub fn fnmatch_normcase_bytes(input: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        let mut out = Vec::with_capacity(input.len());
        for b in input {
            let mut ch = *b;
            if ch == b'/' {
                ch = b'\\';
            } else if ch.is_ascii_uppercase() {
                ch += 32;
            }
            out.push(ch);
        }
        out
    } else {
        input.to_vec()
    }
}

fn fnmatch_escape_regex_char(out: &mut String, ch: char) {
    if ch.is_alphanumeric() || ch == '_' {
        out.push(ch);
        return;
    }
    out.push('\\');
    out.push(ch);
}

pub fn fnmatch_translate_impl(pat: &str) -> String {
    #[derive(Clone)]
    enum Token {
        Star,
        Text(String),
    }

    let chars: Vec<char> = pat.chars().collect();
    let mut res: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let n = chars.len();
    while i < n {
        let ch = chars[i];
        i += 1;
        match ch {
            '*' => {
                if res.last().is_none_or(|token| !matches!(token, Token::Star)) {
                    res.push(Token::Star);
                }
            }
            '?' => res.push(Token::Text(".".to_string())),
            '[' => {
                let mut j = i;
                if j < n && chars[j] == '!' {
                    j += 1;
                }
                if j < n && chars[j] == ']' {
                    j += 1;
                }
                while j < n && chars[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    res.push(Token::Text("\\[".to_string()));
                    continue;
                }
                let mut stuff: String = chars[i..j].iter().collect();
                if !stuff.contains('-') {
                    stuff = stuff.replace('\\', r"\\");
                } else {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut sub_i = i;
                    let mut k = if chars[sub_i] == '!' {
                        sub_i + 2
                    } else {
                        sub_i + 1
                    };
                    loop {
                        let found = chars[k..j]
                            .iter()
                            .position(|ch| *ch == '-')
                            .map(|offset| k + offset);
                        let Some(k_idx) = found else {
                            break;
                        };
                        chunks.push(chars[sub_i..k_idx].iter().collect());
                        sub_i = k_idx + 1;
                        k = sub_i + 2;
                    }
                    let chunk: String = chars[sub_i..j].iter().collect();
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    } else if let Some(last) = chunks.last_mut() {
                        last.push('-');
                    }
                    for idx in (1..chunks.len()).rev() {
                        if let (Some(prev_last), Some(next_first)) =
                            (chunks[idx - 1].chars().last(), chunks[idx].chars().next())
                            && prev_last > next_first
                        {
                            let mut updated = chunks[idx - 1].chars().collect::<Vec<_>>();
                            updated.pop();
                            let mut new_chunk: String = updated.into_iter().collect();
                            let mut next_chars = chunks[idx].chars();
                            next_chars.next();
                            new_chunk.push_str(&next_chars.collect::<String>());
                            chunks[idx - 1] = new_chunk;
                            chunks.remove(idx);
                        }
                    }
                    let escaped_chunks: Vec<String> = chunks
                        .into_iter()
                        .map(|chunk| {
                            let mut out = String::new();
                            for ch in chunk.chars() {
                                match ch {
                                    '\\' => out.push_str(r"\\"),
                                    '-' => out.push_str(r"\-"),
                                    _ => out.push(ch),
                                }
                            }
                            out
                        })
                        .collect();
                    stuff = escaped_chunks.join("-");
                }
                if stuff.contains('&') || stuff.contains('~') || stuff.contains('|') {
                    let mut escaped = String::with_capacity(stuff.len());
                    for ch in stuff.chars() {
                        if matches!(ch, '&' | '~' | '|') {
                            escaped.push('\\');
                        }
                        escaped.push(ch);
                    }
                    stuff = escaped;
                }
                i = j + 1;
                if stuff.is_empty() {
                    res.push(Token::Text("(?!)".to_string()));
                } else if stuff == "!" {
                    res.push(Token::Text(".".to_string()));
                } else {
                    if stuff.starts_with('!') {
                        stuff = format!("^{}", &stuff[1..]);
                    } else if let Some(first) = stuff.chars().next()
                        && (first == '^' || first == '[')
                    {
                        stuff = format!("\\{}", stuff);
                    }
                    res.push(Token::Text(format!("[{stuff}]")));
                }
            }
            other => {
                let mut out = String::new();
                fnmatch_escape_regex_char(&mut out, other);
                res.push(Token::Text(out));
            }
        }
    }

    let inp = res;
    let mut out: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < inp.len() {
        match &inp[idx] {
            Token::Star => break,
            Token::Text(text) => out.push(text.clone()),
        }
        idx += 1;
    }
    while idx < inp.len() {
        idx += 1;
        if idx == inp.len() {
            out.push(".*".to_string());
            break;
        }
        let mut fixed = String::new();
        while idx < inp.len() {
            match &inp[idx] {
                Token::Star => break,
                Token::Text(text) => fixed.push_str(text),
            }
            idx += 1;
        }
        if idx == inp.len() {
            out.push(".*".to_string());
            out.push(fixed);
        } else {
            out.push(format!("(?>.*?{fixed})"));
        }
    }
    let res = out.join("");
    format!("(?s:{res})\\Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_keeps_python_codepoints_distinct_from_encoded_bytes() {
        // CPython fnmatch.fnmatchcase: a surrogate and a non-ASCII scalar each
        // consume one '?'; their UTF-8/WTF-8 byte encodings consume more.
        assert!(fnmatch_match_units(&[0xdcffu32], &[b'?' as u32]));
        assert!(fnmatch_match_units(&[0x00e9u32], &[b'?' as u32]));
        assert!(!fnmatch_match_bytes_impl(&[0xc3, 0xa9], b"?"));
        assert!(fnmatch_match_units(
            &[0xdcffu32],
            &[b'[' as u32, 0xdc80, b'-' as u32, 0xdcff, b']' as u32]
        ));
        assert!(!fnmatch_match_units(
            &[0xe9u32],
            &[b'[' as u32, 0xdc80, b'-' as u32, 0xdcff, b']' as u32]
        ));
    }

    #[test]
    fn matches_text_patterns() {
        assert!(fnmatch_match_impl("field_solve.py", "*.py"));
        assert!(fnmatch_match_impl("file7.py", "file[0-9].py"));
        assert!(fnmatch_match_impl("readme.md", "[!a]*.md"));
        assert!(fnmatch_match_impl("literal[", "literal["));
        assert!(!fnmatch_match_impl("filex.py", "file[0-9].py"));
    }

    #[test]
    fn matches_bytes_patterns() {
        assert!(fnmatch_match_bytes_impl(b"kernel.wasm", b"*.wasm"));
        assert!(fnmatch_match_bytes_impl(b"item9", b"item[0-9]"));
        assert!(!fnmatch_match_bytes_impl(b"itemx", b"item[0-9]"));
    }

    #[test]
    fn normalizes_like_host_fnmatch() {
        if cfg!(windows) {
            assert_eq!(fnmatch_normcase_text("A/B"), "a\\b");
            assert_eq!(fnmatch_normcase_bytes(b"A/B"), b"a\\b");
        } else {
            assert_eq!(fnmatch_normcase_text("A/B"), "A/B");
            assert_eq!(fnmatch_normcase_bytes(b"A/B"), b"A/B");
        }
    }

    #[test]
    fn translates_patterns_to_regex_text() {
        assert_eq!(fnmatch_translate_impl("*.py"), r"(?s:.*\.py)\Z");
        assert_eq!(fnmatch_translate_impl("file?.[ch]"), r"(?s:file.\.[ch])\Z");
        assert_eq!(fnmatch_translate_impl("[!a]*"), r"(?s:[^a].*)\Z");
    }
}
