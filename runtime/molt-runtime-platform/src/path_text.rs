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
}
