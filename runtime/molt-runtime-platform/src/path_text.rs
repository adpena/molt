//! Runtime-independent path text helpers shared by platform/importlib/path builtins.

pub fn path_join_text(mut base: String, part: &str, sep: char) -> String {
    if part.starts_with(sep) {
        return part.to_string();
    }
    if !base.is_empty() && !base.ends_with(sep) {
        base.push(sep);
    }
    base.push_str(part);
    base
}

pub fn path_join_many_text(mut base: String, parts: &[String], sep: char) -> String {
    for part in parts {
        base = path_join_text(base, part, sep);
    }
    base
}

/// Join two raw byte paths using the given separator byte (posixpath-style).
pub fn path_join_raw(base: &[u8], part: &[u8], sep: u8) -> Vec<u8> {
    if part.first() == Some(&sep) {
        return part.to_vec();
    }
    let mut out = base.to_vec();
    if !out.is_empty() && out.last() != Some(&sep) {
        out.push(sep);
    }
    out.extend_from_slice(part);
    out
}

pub fn path_basename_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(idx) => stripped[idx + sep.len_utf8()..].to_string(),
        None => stripped.to_string(),
    }
}

pub fn path_dirname_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(0) => sep.to_string(),
        Some(idx) => stripped[..idx].to_string(),
        None => String::new(),
    }
}

pub fn path_splitext_text(path: &str, sep: char) -> (String, String) {
    let base = path_basename_text(path, sep);
    if !base.contains('.') || base == "." || base == ".." {
        return (path.to_string(), String::new());
    }
    let idx = match base.rfind('.') {
        Some(idx) => idx,
        None => return (path.to_string(), String::new()),
    };
    let root_len = path.len().saturating_sub(base.len()) + idx;
    let root = path[..root_len].to_string();
    let ext = base[idx..].to_string();
    (root, ext)
}

pub fn path_name_text(path: &str, sep: char) -> String {
    let parts = path_parts_text(path, sep);
    if parts.is_empty() {
        return String::new();
    }
    let sep_s = sep.to_string();
    if parts.len() == 1 && parts[0] == sep_s {
        return String::new();
    }
    parts.last().cloned().unwrap_or_default()
}

pub fn path_suffix_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (_, suffix) = path_splitext_text(&name, sep);
    suffix
}

pub fn path_suffixes_text(path: &str, sep: char) -> Vec<String> {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return Vec::new();
    }
    let mut suffixes: Vec<String> = Vec::new();
    let mut stem = name;
    loop {
        let (next_stem, suffix) = path_splitext_text(&stem, sep);
        if suffix.is_empty() {
            break;
        }
        suffixes.push(suffix);
        stem = next_stem;
    }
    suffixes.reverse();
    suffixes
}

pub fn path_stem_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (stem, _) = path_splitext_text(&name, sep);
    stem
}

pub fn path_as_uri_text(path: &str, sep: char) -> Result<String, String> {
    if !path.starts_with(sep) {
        return Err("relative path can't be expressed as a file URI".to_string());
    }
    let mut posix = if sep == '/' {
        path.to_string()
    } else {
        path.replace(sep, "/")
    };
    if !posix.starts_with('/') {
        posix.insert(0, '/');
    }
    Ok(format!("file://{posix}"))
}

pub fn path_normpath_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with(sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.last().is_some_and(|last| *last != "..") {
                parts.pop();
            } else if !absolute {
                parts.push(part);
            }
            continue;
        }
        parts.push(part);
    }
    let sep_s = sep.to_string();
    if absolute {
        let normalized = format!("{sep}{}", parts.join(&sep_s));
        if normalized.is_empty() {
            sep.to_string()
        } else {
            normalized
        }
    } else {
        let normalized = parts.join(&sep_s);
        if normalized.is_empty() {
            ".".to_string()
        } else {
            normalized
        }
    }
}

pub fn path_parts_text(path: &str, sep: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let (drive, root, tail) = path_splitroot_text(path, sep);
    if !drive.is_empty() || !root.is_empty() {
        out.push(format!("{drive}{root}"));
    }
    for part in tail.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        out.push(part.to_string());
    }
    out
}

pub fn path_compare_text(lhs: &str, rhs: &str, sep: char) -> i64 {
    let lhs_parts = path_parts_text(lhs, sep);
    let rhs_parts = path_parts_text(rhs, sep);
    use std::cmp::Ordering;
    match lhs_parts.cmp(&rhs_parts) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

pub fn path_parents_text(path: &str, sep: char) -> Vec<String> {
    let (drive, root, tail) = path_splitroot_text(path, sep);
    let anchor = format!("{drive}{root}");
    let tail_parts = tail
        .split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect::<Vec<String>>();
    if tail_parts.is_empty() {
        return Vec::new();
    }
    let sep_s = sep.to_string();
    let mut out: Vec<String> = Vec::new();
    let mut idx = tail_parts.len();
    while idx > 0 {
        idx -= 1;
        if idx == 0 {
            if anchor.is_empty() {
                out.push(".".to_string());
            } else {
                out.push(anchor.clone());
            }
            continue;
        }
        let prefix = tail_parts[..idx].join(&sep_s);
        if anchor.is_empty() {
            out.push(prefix);
        } else {
            out.push(format!("{anchor}{prefix}"));
        }
    }
    out
}

pub fn path_isabs_text(path: &str, sep: char) -> bool {
    #[cfg(windows)]
    {
        let _ = sep;
        let text = path.replace('/', "\\");
        if text.starts_with("\\\\") || text.starts_with('\\') {
            return true;
        }
        let bytes = text.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        false
    }
    #[cfg(not(windows))]
    {
        path.starts_with(sep)
    }
}

pub fn path_splitroot_text(path: &str, sep: char) -> (String, String, String) {
    #[cfg(windows)]
    {
        let text = path.replace('/', "\\");
        if text.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        let mut drive = String::new();
        let mut root = String::new();
        let mut rest = text.as_str();
        if rest.starts_with("\\\\") {
            let unc = &rest[2..];
            let mut parts = unc.split('\\');
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            if !server.is_empty() && !share.is_empty() {
                drive = format!("\\\\{server}\\{share}");
                let consumed = 2 + server.len() + 1 + share.len();
                rest = &rest[consumed..];
            }
        } else {
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                drive = rest[..2].to_string();
                rest = &rest[2..];
            }
        }
        if rest.starts_with('\\') {
            root = sep.to_string();
            rest = rest.trim_start_matches('\\');
        }
        (drive, root, rest.to_string())
    }
    #[cfg(not(windows))]
    {
        if path.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        if path.starts_with("//") && !path.starts_with("///") {
            let tail = path.trim_start_matches('/').to_string();
            return (String::new(), "//".to_string(), tail);
        }
        if path.starts_with(sep) {
            return (
                String::new(),
                sep.to_string(),
                path.trim_start_matches(sep).to_string(),
            );
        }
        (String::new(), String::new(), path.to_string())
    }
}

pub fn path_relative_to_text(path: &str, base: &str, sep: char) -> Result<String, String> {
    let sep_s = sep.to_string();
    let target_parts = path_parts_text(path, sep);
    let base_parts = path_parts_text(base, sep);
    let target_abs = target_parts.first().is_some_and(|part| part == &sep_s);
    let base_abs = base_parts.first().is_some_and(|part| part == &sep_s);
    if (base_abs && !target_abs) || (!base_abs && target_abs) {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    if base_parts.len() > target_parts.len() {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    for (idx, part) in base_parts.iter().enumerate() {
        if target_parts.get(idx) != Some(part) {
            return Err(format!("{path:?} is not in the subpath of {base:?}"));
        }
    }
    let rel_parts = &target_parts[base_parts.len()..];
    if rel_parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel_parts.join(&sep_s))
    }
}

pub fn path_expandvars_with_lookup(
    path: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let is_var_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let chars: Vec<char> = path.chars().collect();
    let mut out = String::with_capacity(path.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch != '$' {
            out.push(ch);
            idx += 1;
            continue;
        }
        if idx + 1 >= chars.len() {
            out.push('$');
            idx += 1;
            continue;
        }
        let next = chars[idx + 1];
        if next == '{' {
            let mut end = idx + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end >= chars.len() {
                for c in &chars[idx..] {
                    out.push(*c);
                }
                break;
            }
            let name: String = chars[idx + 2..end].iter().collect();
            if name.is_empty() {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            } else if let Some(value) = lookup(&name) {
                out.push_str(&value);
            } else {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            }
            idx = end + 1;
            continue;
        }
        if next == '$' {
            out.push('$');
            out.push('$');
            idx += 2;
            continue;
        }
        let start = idx + 1;
        let mut end = start;
        while end < chars.len() && is_var_char(chars[end]) {
            end += 1;
        }
        if end == start {
            out.push('$');
            idx += 1;
            continue;
        }
        let name: String = chars[start..end].iter().collect();
        if let Some(value) = lookup(&name) {
            out.push_str(&value);
        } else {
            for c in &chars[idx..end] {
                out.push(*c);
            }
        }
        idx = end;
    }
    out
}

pub fn path_match_simple_pattern(name: &str, pat: &str) -> bool {
    type GlobCharClassParse = (Vec<char>, Vec<(char, char)>, bool, usize);

    fn parse_char_class(pat: &[char], mut idx: usize) -> Option<GlobCharClassParse> {
        if idx >= pat.len() || pat[idx] != '[' {
            return None;
        }
        idx += 1;
        if idx >= pat.len() {
            return None;
        }

        let mut negate = false;
        if pat[idx] == '!' {
            negate = true;
            idx += 1;
        }
        if idx >= pat.len() {
            return None;
        }

        let mut singles: Vec<char> = Vec::new();
        let mut ranges: Vec<(char, char)> = Vec::new();

        if pat[idx] == ']' {
            singles.push(']');
            idx += 1;
        }

        while idx < pat.len() && pat[idx] != ']' {
            if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
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
        if idx >= pat.len() || pat[idx] != ']' {
            return None;
        }
        Some((singles, ranges, negate, idx + 1))
    }

    fn char_class_hit(ch: char, singles: &[char], ranges: &[(char, char)], negate: bool) -> bool {
        let mut hit = singles.contains(&ch);
        if !hit {
            hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
        }
        if negate { !hit } else { hit }
    }

    let name_chars: Vec<char> = name.chars().collect();
    let pat_chars: Vec<char> = pat.chars().collect();
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name_chars.len() {
        if pi < pat_chars.len() && pat_chars[pi] == '*' {
            while pi < pat_chars.len() && pat_chars[pi] == '*' {
                pi += 1;
            }
            if pi == pat_chars.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat_chars.len()
            && pat_chars[pi] == '['
            && let Some((singles, ranges, negate, next_idx)) = parse_char_class(&pat_chars, pi)
        {
            let hit = char_class_hit(name_chars[ni], &singles, &ranges, negate);
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
        if pi < pat_chars.len() && (pat_chars[pi] == '?' || pat_chars[pi] == name_chars[ni]) {
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
    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }
    pi == pat_chars.len()
}

pub fn path_match_text(path: &str, pattern: &str, sep: char) -> bool {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let absolute = pattern.starts_with(sep);
    if absolute && !path.starts_with(sep) {
        return false;
    }
    let pat = if absolute {
        pattern.trim_start_matches(sep)
    } else {
        pattern.as_str()
    };
    let path_trimmed = path.trim_start_matches(sep);
    if !pat.contains(sep) && !pat.contains('/') {
        let name = path_basename_text(path, sep);
        if pat == "*" {
            return !name.is_empty();
        }
        if pat.starts_with("*.") && pat.matches('*').count() == 1 && !pat.contains('?') {
            return name.ends_with(&pat[1..]);
        }
        return path_match_simple_pattern(&name, pat);
    }

    fn split_components(text: &str, sep: char) -> Vec<&str> {
        text.split(sep)
            .filter(|part| !part.is_empty() && *part != ".")
            .collect()
    }

    fn match_components(path_parts: &[&str], pat_parts: &[&str]) -> bool {
        fn inner(path_parts: &[&str], pat_parts: &[&str], pi: usize, pj: usize) -> bool {
            if pj >= pat_parts.len() {
                return pi >= path_parts.len();
            }
            let pat = pat_parts[pj];
            if pat == "**" {
                if inner(path_parts, pat_parts, pi, pj + 1) {
                    return true;
                }
                return pi < path_parts.len() && inner(path_parts, pat_parts, pi + 1, pj);
            }
            if pi >= path_parts.len() {
                return false;
            }
            path_match_simple_pattern(path_parts[pi], pat)
                && inner(path_parts, pat_parts, pi + 1, pj + 1)
        }
        inner(path_parts, pat_parts, 0, 0)
    }

    let pat_parts = split_components(pat, sep);
    let path_parts = split_components(path_trimmed, sep);
    if absolute {
        return match_components(&path_parts, &pat_parts);
    }
    for start in 0..=path_parts.len() {
        if match_components(&path_parts[start..], &pat_parts) {
            return true;
        }
    }
    false
}

pub fn glob_has_magic_text(pathname: &str) -> bool {
    pathname
        .as_bytes()
        .iter()
        .any(|ch| matches!(*ch, b'*' | b'?' | b'['))
}

pub fn glob_is_hidden_text(name: &str) -> bool {
    name.starts_with('.')
}

pub fn glob_split_path_text(pathname: &str, sep: char) -> (String, String) {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    if tail.is_empty() {
        return (format!("{drive}{root}"), String::new());
    }

    let mut head = String::new();
    let mut base = tail.clone();
    if let Some(idx) = tail.rfind(sep) {
        head = tail[..idx + sep.len_utf8()].to_string();
        base = tail[idx + sep.len_utf8()..].to_string();
    }

    if !head.is_empty() {
        let all_sep = head.chars().all(|ch| ch == sep);
        if !all_sep {
            head = head.trim_end_matches(sep).to_string();
        }
    }

    let dirname = format!("{drive}{root}{head}");
    (dirname, base)
}

pub fn glob_join_text(base: &str, part: &str, sep: char) -> String {
    if base.is_empty() {
        return part.to_string();
    }
    if path_isabs_text(part, sep) {
        return part.to_string();
    }
    #[cfg(windows)]
    {
        let (part_drive, _part_root, _part_tail) = path_splitroot_text(part, sep);
        if !part_drive.is_empty() {
            return part.to_string();
        }
    }
    path_join_text(base.to_string(), part, sep)
}

pub fn glob_escape_text(pathname: &str, sep: char) -> String {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    let mut out = String::new();
    out.push_str(&drive);
    out.push_str(&root);
    for ch in tail.chars() {
        if matches!(ch, '*' | '?' | '[') {
            out.push('[');
            out.push(ch);
            out.push(']');
        } else {
            out.push(ch);
        }
    }
    out
}

fn glob_regex_escape_char(out: &mut String, ch: char) {
    if matches!(
        ch,
        '.' | '^' | '$' | '+' | '{' | '}' | '(' | ')' | '|' | '\\' | '[' | ']'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn glob_regex_escape_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        glob_regex_escape_char(&mut out, ch);
    }
    out
}

fn glob_split_on_seps(pat: &str, seps: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in pat.chars() {
        if seps.contains(&ch) {
            out.push(cur);
            cur = String::new();
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

type GlobTranslateCharClassParse = (Vec<char>, Vec<(char, char)>, bool, usize);

fn glob_translate_parse_char_class(
    pat: &[char],
    mut idx: usize,
) -> Option<GlobTranslateCharClassParse> {
    if idx >= pat.len() || pat[idx] != '[' {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == '!' {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<char> = Vec::new();
    let mut ranges: Vec<(char, char)> = Vec::new();

    if pat[idx] == ']' {
        singles.push(']');
        idx += 1;
    }
    while idx < pat.len() && pat[idx] != ']' {
        if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
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
    if idx >= pat.len() || pat[idx] != ']' {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn glob_translate_char_class(
    singles: Vec<char>,
    ranges: Vec<(char, char)>,
    negate: bool,
) -> String {
    let mut out = String::new();
    out.push('[');
    if negate {
        out.push('^');
    }
    for ch in singles {
        if matches!(ch, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    for (start, end) in ranges {
        if matches!(start, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(start);
        out.push('-');
        if matches!(end, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(end);
    }
    out.push(']');
    out
}

fn glob_translate_segment(part: &str, star_expr: &str, ques_expr: &str) -> String {
    let chars: Vec<char> = part.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        match chars[idx] {
            '*' => out.push_str(star_expr),
            '?' => out.push_str(ques_expr),
            '[' => {
                if let Some((singles, ranges, negate, next_idx)) =
                    glob_translate_parse_char_class(&chars, idx)
                {
                    out.push_str(&glob_translate_char_class(singles, ranges, negate));
                    idx = next_idx;
                    continue;
                } else {
                    out.push_str("\\[");
                }
            }
            ch => glob_regex_escape_char(&mut out, ch),
        }
        idx += 1;
    }
    out
}

fn glob_default_seps_text() -> String {
    #[cfg(windows)]
    {
        "\\/".to_string()
    }
    #[cfg(not(windows))]
    {
        "/".to_string()
    }
}

pub fn glob_translate_text(
    pat: &str,
    recursive: bool,
    include_hidden: bool,
    seps: Option<&str>,
) -> String {
    let seps_text = if let Some(raw) = seps {
        if raw.is_empty() {
            glob_default_seps_text()
        } else {
            raw.to_string()
        }
    } else {
        glob_default_seps_text()
    };
    let sep_chars: Vec<char> = seps_text.chars().collect();
    let escaped_seps = glob_regex_escape_text(&seps_text);
    let any_sep = if sep_chars.len() > 1 {
        format!("[{escaped_seps}]")
    } else {
        escaped_seps.clone()
    };
    let not_sep = format!("[^{escaped_seps}]");
    let (one_last_segment, one_segment, any_segments, any_last_segments) = if include_hidden {
        let one_last_segment = format!("{not_sep}+");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:.+{any_sep})?");
        let any_last_segments = ".*".to_string();
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    } else {
        let one_last_segment = format!("[^{escaped_seps}.]{not_sep}*");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:{one_segment})*");
        let any_last_segments = format!("{any_segments}(?:{one_last_segment})?");
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    };

    let parts = glob_split_on_seps(pat, &sep_chars);
    let last_part_idx = parts.len().saturating_sub(1);
    let mut results: Vec<String> = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        if part == "*" {
            if idx < last_part_idx {
                results.push(one_segment.clone());
            } else {
                results.push(one_last_segment.clone());
            }
        } else if recursive && part == "**" {
            if idx < last_part_idx {
                if parts[idx + 1] != "**" {
                    results.push(any_segments.clone());
                }
            } else {
                results.push(any_last_segments.clone());
            }
        } else {
            if !part.is_empty() {
                if !include_hidden && part.chars().next().is_some_and(|ch| ch == '*' || ch == '?') {
                    results.push(r"(?!\.)".to_string());
                }
                let star_expr = format!("{not_sep}*");
                results.push(glob_translate_segment(part, &star_expr, &not_sep));
            }
            if idx < last_part_idx {
                results.push(any_sep.clone());
            }
        }
    }
    let body = results.join("");
    format!("(?s:{body})\\Z")
}

fn glob_split_components(text: &str, sep: char) -> Vec<String> {
    text.split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect()
}

fn glob_walk(
    dir: &std::path::Path,
    rel_parts: &mut Vec<String>,
    pat_parts: &[String],
    pi: usize,
    sep: char,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    let sep_s = sep.to_string();
    if pi >= pat_parts.len() {
        if !rel_parts.is_empty() {
            out.push(rel_parts.join(&sep_s));
        }
        return Ok(());
    }
    let pat = &pat_parts[pi];
    if pat == "**" {
        glob_walk(dir, rel_parts, pat_parts, pi + 1, sep, out)?;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            rel_parts.push(name);
            glob_walk(&entry.path(), rel_parts, pat_parts, pi, sep, out)?;
            rel_parts.pop();
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path_match_simple_pattern(&name, pat) {
            continue;
        }
        let file_type = entry.file_type()?;
        rel_parts.push(name);
        if pi + 1 >= pat_parts.len() {
            out.push(rel_parts.join(&sep_s));
        } else if file_type.is_dir() {
            glob_walk(&entry.path(), rel_parts, pat_parts, pi + 1, sep, out)?;
        }
        rel_parts.pop();
    }
    Ok(())
}

pub fn path_glob_matches(
    dir: &std::path::Path,
    pattern: &str,
    sep: char,
) -> std::io::Result<Vec<String>> {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let pat_parts = glob_split_components(&pattern, sep);
    let mut matches: Vec<String> = Vec::new();
    if !pat_parts.is_empty() {
        let mut rel_parts: Vec<String> = Vec::new();
        glob_walk(dir, &mut rel_parts, &pat_parts, 0, sep, &mut matches)?;
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_normalizes_and_splits_text_paths() {
        assert_eq!(path_join_text("a".to_string(), "b", '/'), "a/b");
        assert_eq!(
            path_join_many_text("a".to_string(), &["b".into(), "c".into()], '/'),
            "a/b/c"
        );
        assert_eq!(path_join_raw(b"a", b"b", b'/'), b"a/b".to_vec());
        assert_eq!(path_basename_text("/a/b/", '/'), "b");
        assert_eq!(path_dirname_text("/a/b/", '/'), "/a");
        assert_eq!(
            path_splitext_text("/a/b.py", '/'),
            ("/a/b".into(), ".py".into())
        );
        assert_eq!(path_normpath_text("/a/./b/../c", '/'), "/a/c");
    }

    #[test]
    fn derives_names_suffixes_parts_and_relative_paths() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = sep.to_string();
        let pkg = format!("{sep}a{sep}pkg.tar.gz");
        assert_eq!(path_name_text(&pkg, sep), "pkg.tar.gz");
        assert_eq!(path_suffix_text(&pkg, sep), ".gz");
        assert_eq!(path_suffixes_text(&pkg, sep), vec![".tar", ".gz"]);
        assert_eq!(path_stem_text(&pkg, sep), "pkg.tar");
        assert_eq!(
            path_parts_text(&format!("{sep}a{sep}b"), sep),
            vec![root.clone(), "a".into(), "b".into()]
        );
        assert_eq!(
            path_parents_text(&format!("{sep}a{sep}b{sep}c"), sep),
            vec![format!("{sep}a{sep}b"), format!("{sep}a"), root]
        );
        assert_eq!(
            path_compare_text(&format!("{sep}a"), &format!("{sep}b"), sep),
            -1
        );
        assert_eq!(
            path_relative_to_text(&format!("{sep}a{sep}b{sep}c"), &format!("{sep}a"), sep),
            Ok(format!("b{sep}c"))
        );
    }

    #[test]
    fn expands_variables_through_caller_lookup() {
        assert_eq!(
            path_expandvars_with_lookup("$ROOT/${NAME}/$$/$MISSING", |name| match name {
                "ROOT" => Some("r".into()),
                "NAME" => Some("n".into()),
                _ => None,
            }),
            "r/n/$$/$MISSING"
        );
    }

    #[test]
    fn matches_glob_text_patterns() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("root{sep}pkg{sep}mod.py");
        assert!(path_match_simple_pattern("mod.py", "*.py"));
        assert!(path_match_simple_pattern("mod7.py", "mod[0-9].py"));
        assert!(!path_match_simple_pattern("modx.py", "mod[!x].py"));
        assert!(path_match_text(&path, &format!("pkg{sep}*.py"), sep));
        assert!(path_match_text(
            &path,
            &format!("root{sep}**{sep}*.py"),
            sep
        ));
        assert!(glob_has_magic_text("*.py"));
        assert!(!glob_has_magic_text("plain.py"));
        assert!(glob_is_hidden_text(".secret"));
        assert_eq!(
            glob_split_path_text(&format!("root{sep}pkg{sep}*.py"), sep),
            (format!("root{sep}pkg"), "*.py".to_string())
        );
        assert_eq!(
            glob_join_text(&format!("root{sep}pkg"), "mod.py", sep),
            format!("root{sep}pkg{sep}mod.py")
        );
        assert_eq!(glob_escape_text("a*b?[c]", sep), "a[*]b[?][[]c]");
        assert_eq!(
            glob_translate_text("*.py", false, false, Some("/")),
            r"(?s:(?!\.)[^/]*\.py)\Z"
        );
    }

    #[test]
    fn walks_glob_matches_from_platform_authority() {
        let sep = std::path::MAIN_SEPARATOR;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt_path_text_glob_{}_{}",
            std::process::id(),
            stamp
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pkg").join("sub")).unwrap();
        std::fs::write(root.join("pkg").join("a.py"), b"a").unwrap();
        std::fs::write(root.join("pkg").join("sub").join("b.py"), b"b").unwrap();
        std::fs::write(root.join("pkg").join("sub").join("c.txt"), b"c").unwrap();

        let mut matches = path_glob_matches(&root, &format!("pkg{sep}**{sep}*.py"), sep).unwrap();
        matches.sort();
        assert_eq!(
            matches,
            vec![format!("pkg{sep}a.py"), format!("pkg{sep}sub{sep}b.py")]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
