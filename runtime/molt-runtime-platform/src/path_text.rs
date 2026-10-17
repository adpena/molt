//! Runtime-independent path text and native access helpers shared by path consumers.

/// Owned lexical join state. Appending never copies the accumulated tail.
/// A separate NT drive permits case replacement without moving that tail.
pub struct PathJoin {
    drive: Vec<u8>,
    path: Vec<u8>,
    sep: u8,
    text: bool,
}

impl PathJoin {
    pub fn new(mut base: Vec<u8>, sep: u8, text: bool, drive_end: usize) -> Self {
        assert!(matches!(sep, b'/' | b'\\'));
        let drive = base[..drive_end].to_vec();
        base.drain(..drive_end);
        Self {
            drive,
            path: base,
            sep,
            text,
        }
    }

    pub fn push(&mut self, part: &[u8], (drive_end, root_end): (usize, usize)) {
        let drive = &part[..drive_end];
        if root_end != drive_end {
            if !drive.is_empty() || self.drive.is_empty() {
                self.drive.clear();
                self.drive.extend_from_slice(drive);
            }
            self.path.clear();
            self.path.extend_from_slice(&part[drive_end..]);
            return;
        }
        if !drive.is_empty() && drive != self.drive {
            let same_drive = if self.text {
                // ntpath uses str.lower for text and bytes.lower for bytes.
                crate::utils::python_lowercase(drive) == crate::utils::python_lowercase(&self.drive)
            } else {
                drive.eq_ignore_ascii_case(&self.drive)
            };
            if !same_drive {
                self.path.clear();
            }
            self.drive.clear();
            self.drive.extend_from_slice(drive);
        }
        if self
            .path
            .last()
            .is_some_and(|&byte| !path_separator(byte, self.sep))
        {
            self.path.push(self.sep);
        }
        self.path.extend_from_slice(&part[root_end..]);
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.drive.is_empty() {
            return self.path;
        }
        // UNC drive plus a relative tail needs a separator; a drive letter does not.
        if self
            .path
            .first()
            .is_some_and(|&byte| !path_separator(byte, self.sep))
            && self
                .drive
                .last()
                .is_some_and(|&byte| byte != b':' && !path_separator(byte, self.sep))
        {
            self.drive.push(self.sep);
        }
        self.drive.extend_from_slice(&self.path);
        self.drive
    }

    /// Preserve NT's three values when Python subclass operations become observable.
    pub fn into_components(mut self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let rooted = self
            .path
            .first()
            .is_some_and(|&byte| path_separator(byte, self.sep));
        let root = if rooted {
            vec![self.path.remove(0)]
        } else {
            Vec::new()
        };
        (self.drive, root, self.path)
    }
}

pub fn path_join_text(base: String, part: &str, sep: char) -> String {
    let drive = path_root_offsets(base.as_bytes(), sep as u8, PathCoordinate::PythonCodePoints).0;
    let mut join = PathJoin::new(base.into_bytes(), sep as u8, true, drive);
    join.push(
        part.as_bytes(),
        path_root_offsets(part.as_bytes(), sep as u8, PathCoordinate::PythonCodePoints),
    );
    String::from_utf8(join.finish()).expect("ASCII path joining preserves UTF-8")
}

pub fn path_join_many_text(
    base: String,
    parts: impl IntoIterator<Item = impl AsRef<str>>,
    sep: char,
) -> String {
    let drive = path_root_offsets(base.as_bytes(), sep as u8, PathCoordinate::PythonCodePoints).0;
    let mut join = PathJoin::new(base.into_bytes(), sep as u8, true, drive);
    for part in parts {
        join.push(
            part.as_ref().as_bytes(),
            path_root_offsets(
                part.as_ref().as_bytes(),
                sep as u8,
                PathCoordinate::PythonCodePoints,
            ),
        );
    }
    String::from_utf8(join.finish()).expect("ASCII path joining preserves UTF-8")
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

/// Lexically normalize either a text encoding or opaque filesystem bytes.
/// Separators and dot components are ASCII; all other bytes remain unchanged.
pub fn path_normpath_raw(path: &[u8], sep: u8) -> Vec<u8> {
    path_normpath(path, sep, false)
}

fn path_normpath(path: &[u8], sep: u8, text: bool) -> Vec<u8> {
    let windows;
    let path = if sep == b'\\' {
        windows = path
            .iter()
            .map(|&byte| if byte == b'/' { sep } else { byte })
            .collect::<Vec<_>>();
        windows.as_slice()
    } else {
        path
    };
    let (drive_end, root_end) = path_root_offsets(
        path,
        sep,
        if text {
            PathCoordinate::PythonCodePoints
        } else {
            PathCoordinate::Bytes
        },
    );
    let rooted = root_end != drive_end;
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path[root_end..].split(|&byte| byte == sep) {
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." {
            if parts.last().is_some_and(|last| *last != b"..") {
                parts.pop();
            } else if !rooted {
                parts.push(part);
            }
        } else {
            parts.push(part);
        }
    }
    let mut out = Vec::with_capacity(path.len().max(1));
    out.extend_from_slice(&path[..root_end]);
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            out.push(sep);
        }
        out.extend_from_slice(part);
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

pub fn path_normpath_text(path: &str, sep: char) -> String {
    // The public path helpers accept the target's ASCII filesystem separator.
    assert!(matches!(sep, '/' | '\\'));
    String::from_utf8(path_normpath(path.as_bytes(), sep as u8, true))
        .expect("ASCII path normalization preserves UTF-8")
}

fn path_separator(byte: u8, sep: u8) -> bool {
    byte == sep || (sep == b'\\' && byte == b'/')
}

/// Indexing coordinate of the upstream path parser. Offsets returned below are
/// always byte offsets into the original (possibly WTF-8) storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathCoordinate {
    Bytes,
    PythonCodePoints,
    WindowsUtf16,
}

// One drive/root parser for joining, lexical normalization and splitroot projection.
// Offsets lie on ASCII boundaries in either UTF-8 or opaque filesystem bytes.
pub fn path_root_offsets(path: &[u8], sep: u8, coordinate: PathCoordinate) -> (usize, usize) {
    if sep == b'\\' {
        if path
            .get(..2)
            .is_some_and(|prefix| prefix.iter().all(|&byte| path_separator(byte, sep)))
        {
            let start = if path.get(..8).is_some_and(|prefix| {
                prefix
                    .iter()
                    .zip(b"\\\\?\\UNC\\")
                    .all(|(&byte, &expected)| {
                        if expected == sep {
                            path_separator(byte, sep)
                        } else {
                            byte.eq_ignore_ascii_case(&expected)
                        }
                    })
            }) {
                8
            } else {
                2
            };
            let Some(server_end) = path[start..]
                .iter()
                .position(|&byte| path_separator(byte, sep))
                .map(|index| start + index)
            else {
                return (path.len(), path.len());
            };
            let Some(share_end) = path[server_end + 1..]
                .iter()
                .position(|&byte| path_separator(byte, sep))
                .map(|index| server_end + 1 + index)
            else {
                return (path.len(), path.len());
            };
            return (share_end, share_end + 1);
        }
        if path.first().is_some_and(|&byte| path_separator(byte, sep)) {
            return (0, 1);
        }
        // The native Windows splitroot provider indexes UTF-16 code units.
        // A scalar encoded with four WTF-8 bytes occupies two such units, so
        // its following colon cannot be the second unit of a drive prefix.
        let first_width = match coordinate {
            PathCoordinate::Bytes => 1,
            PathCoordinate::PythonCodePoints | PathCoordinate::WindowsUtf16 => {
                let width = path
                    .first()
                    .map_or(1, |byte| byte.leading_ones().max(1) as usize);
                if coordinate == PathCoordinate::WindowsUtf16 && width == 4 {
                    return (0, 0);
                }
                width
            }
        };
        if path.get(first_width) == Some(&b':') {
            let drive_end = first_width + 1;
            return (
                drive_end,
                drive_end
                    + usize::from(
                        path.get(drive_end)
                            .is_some_and(|&byte| path_separator(byte, sep)),
                    ),
            );
        }
    } else if path.first() == Some(&sep) {
        // POSIX reserves exactly two leading separators; three or more collapse.
        return (
            0,
            if path.get(1) == Some(&sep) && path.get(2) != Some(&sep) {
                2
            } else {
                1
            },
        );
    }
    (0, 0)
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

pub fn path_isabs_raw(path: &[u8], sep: char, text: bool) -> bool {
    let (drive, root) = path_root_offsets(
        path,
        sep as u8,
        if text {
            PathCoordinate::PythonCodePoints
        } else {
            PathCoordinate::Bytes
        },
    );
    root > drive
        || (sep == '\\'
            && path
                .get(..2)
                .is_some_and(|start| start.iter().all(|byte| path_separator(*byte, sep as u8))))
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
    assert!(matches!(sep, '/' | '\\'));
    let normalized;
    let path = if sep == '\\' {
        normalized = path.replace('/', "\\");
        normalized.as_str()
    } else {
        path
    };
    let (drive, root) =
        path_root_offsets(path.as_bytes(), sep as u8, PathCoordinate::PythonCodePoints);
    (
        path[..drive].to_string(),
        path[drive..root].to_string(),
        path[root..].to_string(),
    )
}

pub fn path_match_simple_pattern(name: &str, pat: &str) -> bool {
    crate::fnmatch::fnmatch_match_impl(name, pat)
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
    glob_has_magic_raw(pathname.as_bytes())
}

pub fn glob_has_magic_raw(pathname: &[u8]) -> bool {
    pathname.iter().any(|ch| matches!(*ch, b'*' | b'?' | b'['))
}

pub fn glob_is_hidden_text(name: &str) -> bool {
    name.starts_with('.')
}

pub fn glob_split_path_raw(
    pathname: &[u8],
    sep: char,
    coordinate: PathCoordinate,
) -> (Vec<u8>, Vec<u8>) {
    let sep = sep as u8;
    let (_, root_end) = path_root_offsets(pathname, sep, coordinate);
    glob_split_path_with_root(pathname, sep, root_end)
}

pub fn glob_split_path_with_root(pathname: &[u8], sep: u8, root_end: usize) -> (Vec<u8>, Vec<u8>) {
    let tail = &pathname[root_end..];
    if tail.is_empty() {
        return (pathname.to_vec(), Vec::new());
    }
    let Some(index) = tail.iter().rposition(|byte| *byte == sep) else {
        return (pathname[..root_end].to_vec(), tail.to_vec());
    };
    let mut end = root_end + index + 1;
    if !tail[..index + 1].iter().all(|byte| *byte == sep) {
        while end > root_end && pathname[end - 1] == sep {
            end -= 1;
        }
    }
    (pathname[..end].to_vec(), tail[index + 1..].to_vec())
}

pub fn glob_split_path_text(pathname: &str, sep: char) -> (String, String) {
    let (head, tail) =
        glob_split_path_raw(pathname.as_bytes(), sep, PathCoordinate::PythonCodePoints);
    (
        String::from_utf8(head).expect("ASCII separator boundary"),
        String::from_utf8(tail).expect("ASCII separator boundary"),
    )
}

pub fn glob_join_raw(
    base: &[u8],
    part: &[u8],
    sep: char,
    text: bool,
    coordinate: PathCoordinate,
) -> Vec<u8> {
    let mut join = PathJoin::new(
        base.to_vec(),
        sep as u8,
        text,
        path_root_offsets(base, sep as u8, coordinate).0,
    );
    join.push(part, path_root_offsets(part, sep as u8, coordinate));
    join.finish()
}

pub fn glob_join_text(base: &str, part: &str, sep: char) -> String {
    String::from_utf8(glob_join_raw(
        base.as_bytes(),
        part.as_bytes(),
        sep,
        true,
        PathCoordinate::PythonCodePoints,
    ))
    .expect("UTF-8 path join")
}

pub fn glob_escape_raw(pathname: &[u8], sep: char, coordinate: PathCoordinate) -> Vec<u8> {
    let (drive_end, _) = path_root_offsets(pathname, sep as u8, coordinate);
    glob_escape_with_drive(pathname, drive_end)
}

/// Escape only after an already parsed drive (which may use a filesystem codec).
pub fn glob_escape_with_drive(pathname: &[u8], drive_end: usize) -> Vec<u8> {
    let mut out = pathname[..drive_end].to_vec();
    for &byte in &pathname[drive_end..] {
        if matches!(byte, b'*' | b'?' | b'[') {
            out.extend_from_slice(&[b'[', byte, b']']);
        } else {
            out.push(byte);
        }
    }
    out
}

pub fn glob_escape_text(pathname: &str, sep: char) -> String {
    String::from_utf8(glob_escape_raw(
        pathname.as_bytes(),
        sep,
        PathCoordinate::PythonCodePoints,
    ))
    .expect("ASCII wildcard escape")
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

/// Directory traversal retains OS names. Filesystem scan errors suppress that
/// directory, as pathlib selectors do; only the Python decoder can fail outward.
fn glob_walk<E>(
    dir: &std::path::Path,
    relative: &mut std::path::PathBuf,
    pattern: &[Vec<u32>],
    index: usize,
    out: &mut Vec<std::path::PathBuf>,
    terminal_recursive_files: bool,
    decode: &mut impl FnMut(&std::ffi::OsStr) -> Result<Vec<u32>, E>,
) -> Result<(), E> {
    if index == pattern.len() {
        out.push(relative.clone());
        return Ok(());
    }
    let part = &pattern[index];
    if part.is_empty() {
        if dir.is_dir() {
            out.push(relative.clone());
        }
        return Ok(());
    }
    // Parent components select a path directly; directory entries never contain
    // "..". Preserve the lexical component in the returned Path, as pathlib does.
    if part == &[u32::from(b'.'), u32::from(b'.')] {
        relative.push("..");
        let result = glob_walk(
            &dir.join(".."),
            relative,
            pattern,
            index + 1,
            out,
            terminal_recursive_files,
            decode,
        );
        relative.pop();
        return result;
    }
    let recursive = part == &[u32::from(b'*'), u32::from(b'*')];
    if recursive {
        glob_walk(
            dir,
            relative,
            pattern,
            index + 1,
            out,
            terminal_recursive_files,
            decode,
        )?;
    }
    // Close each directory stream before descending, including failure while
    // iterating it. Deep traversal must not retain one descriptor per level.
    let Ok(entries) =
        std::fs::read_dir(dir).and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
    else {
        return Ok(());
    };
    for entry in entries {
        let name = entry.file_name();
        if recursive {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if terminal_recursive_files && index + 1 == pattern.len() {
                    out.push(relative.join(name));
                }
                continue;
            }
        } else if !crate::fnmatch::fnmatch_match_units(&decode(&name)?, part) {
            continue;
        }
        relative.push(&name);
        if !recursive && index + 1 == pattern.len() {
            out.push(relative.clone());
        } else if recursive || entry.path().is_dir() {
            // Ordinary components follow directory symlinks; ** does not.
            glob_walk(
                &entry.path(),
                relative,
                pattern,
                if recursive { index } else { index + 1 },
                out,
                terminal_recursive_files,
                decode,
            )?;
        }
        relative.pop();
    }
    Ok(())
}

pub fn path_glob_matches<E>(
    dir: &std::path::Path,
    pattern: &[Vec<u32>],
    terminal_recursive_files: bool,
    mut decode: impl FnMut(&std::ffi::OsStr) -> Result<Vec<u32>, E>,
) -> Result<Vec<std::path::PathBuf>, E> {
    let mut matches = Vec::new();
    if dir.is_dir() {
        glob_walk(
            dir,
            &mut std::path::PathBuf::new(),
            pattern,
            0,
            &mut matches,
            terminal_recursive_files,
            &mut decode,
        )?;
    }
    // Recursive selectors may reach one path through several ** components.
    if pattern
        .iter()
        .filter(|part| part.as_slice() == [42, 42])
        .count()
        > 1
    {
        let mut seen = std::collections::HashSet::new();
        matches.retain(|path| seen.insert(path.clone()));
    }
    Ok(matches)
}

/// Shared native access predicate. No Python objects or conversion callbacks;
/// each caller owns admission, argument conversion and exception ordering.
pub fn path_access(path: &std::path::Path, mode: i32) -> bool {
    #[cfg(unix)]
    {
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
            return false;
        };
        unsafe { libc::access(path.as_ptr(), mode) == 0 }
    }
    #[cfg(not(unix))]
    {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        // CPython Windows uses attributes: READONLY directories satisfy W_OK.
        // R_OK/X_OK add no attribute restriction. WASI execution is unqualified.
        mode & 2 == 0 || !metadata.permissions().readonly() || metadata.is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_coordinates_distinguish_bytes_python_characters_and_windows_units() {
        use PathCoordinate::{Bytes, PythonCodePoints, WindowsUtf16};
        for (path, byte_root, python_root, wide_root) in [
            (b"C:leaf".as_slice(), (2, 2), (2, 2), (2, 2)),
            ("é:leaf".as_bytes(), (0, 0), (3, 3), (3, 3)),
            ("😀:leaf".as_bytes(), (0, 0), (5, 5), (0, 0)),
            (b"\xed\xa0\x80:leaf".as_slice(), (0, 0), (4, 4), (4, 4)),
            (
                b"\xed\xa0\xbd\xed\xb8\x80:leaf".as_slice(),
                (0, 0),
                (0, 0),
                (0, 0),
            ),
            (
                b"\\\\server\\share\\leaf".as_slice(),
                (14, 15),
                (14, 15),
                (14, 15),
            ),
        ] {
            for (coordinate, expected) in [
                (Bytes, byte_root),
                (PythonCodePoints, python_root),
                (WindowsUtf16, wide_root),
            ] {
                assert_eq!(
                    path_root_offsets(path, b'\\', coordinate),
                    expected,
                    "{path:?} {coordinate:?}"
                );
            }
        }
        for coordinate in [Bytes, PythonCodePoints, WindowsUtf16] {
            assert_eq!(
                path_root_offsets(b"//server/share", b'/', coordinate),
                (0, 2)
            );
            assert_eq!(path_root_offsets(b"///leaf", b'/', coordinate), (0, 1));
        }
    }

    #[test]
    fn joins_match_cpython_posix_nt_text_and_bytes() {
        // Expected bytes from CPython 3.12.15 posixpath/ntpath, including
        // opaque bytes, WTF-8, Unicode drives, UNC and separator spelling.
        /// (separator, text join, parts, expected result)
        type JoinCase = (u8, bool, &'static [&'static [u8]], &'static [u8]);
        let cases: &[JoinCase] = &[
            (47, true, &[b"a", b"", b"b"], b"a/b"),
            (47, true, &[b"a", b"//b", b"c"], b"//b/c"),
            (47, false, &[b"a", b"\xff", b""], b"a/\xff/"),
            (92, true, &[b"C:/a", b"b", b""], b"C:/a\x5cb\x5c"),
            (92, true, &[b"C:/a", b"/root"], b"C:/root"),
            (92, true, &[b"C:/a", b"c:b"], b"c:/a\x5cb"),
            (92, true, &[b"C:/a", b"D:b"], b"D:b"),
            (
                92,
                true,
                &[b"//host/share", b"child"],
                b"//host/share\x5cchild",
            ),
            (
                92,
                true,
                &[b"\x5c\x5cserver\x5c", b"child"],
                b"\x5c\x5cserver\x5cchild",
            ),
            (92, true, &[b"//server/", b"child"], b"//server/child"),
            (
                92,
                true,
                &[b"//?/UNC/host/share", b"child"],
                b"//?/UNC/host/share\x5cchild",
            ),
            (92, true, &[b"C:", b""], b"C:"),
            (92, true, &[b"C:/a", b"../b"], b"C:/a\x5c../b"),
            (
                92,
                true,
                &[b"\xc3\x89:a", b"\xc3\xa9:b"],
                b"\xc3\xa9:a\x5cb",
            ),
            (
                92,
                true,
                &[b"\xc4\xb0:a", b"i\xcc\x87:b"],
                b"\xc4\xb0:a\x5ci\xcc\x87:b",
            ),
            (
                92,
                true,
                &[b"\xed\xa0\x80:a", b"\xed\xa0\x81:b"],
                b"\xed\xa0\x81:b",
            ),
            (
                92,
                true,
                &[b"//H\xc3\x96ST/SHARE", b"//h\xc3\xb6st/share:child"],
                b"//h\xc3\xb6st/share:child",
            ),
            (92, false, &[b"\xc9:a", b"\xe9:b"], b"\xe9:b"),
            (
                92,
                false,
                &[b"\x5c\x5cserver\x5c", b"child"],
                b"\x5c\x5cserver\x5cchild",
            ),
        ];
        for &(sep, text, parts, expected) in cases {
            let coordinate = if text {
                PathCoordinate::PythonCodePoints
            } else {
                PathCoordinate::Bytes
            };
            let mut join = PathJoin::new(
                parts[0].to_vec(),
                sep,
                text,
                path_root_offsets(parts[0], sep, coordinate).0,
            );
            for part in &parts[1..] {
                join.push(part, path_root_offsets(part, sep, coordinate));
            }
            assert_eq!(join.finish(), expected, "{parts:?}");
        }
    }

    #[test]
    fn joins_normalizes_and_splits_text_paths() {
        assert_eq!(path_join_text("a".to_string(), "b", '/'), "a/b");
        assert_eq!(
            path_join_many_text("a".to_string(), ["b", "c"], '/'),
            "a/b/c"
        );
        let mut bytes = PathJoin::new(b"a".to_vec(), b'/', false, 0);
        bytes.push(b"b", (0, 0));
        assert_eq!(bytes.finish(), b"a/b");
        assert_eq!(path_basename_text("/a/b/", '/'), "b");
        assert_eq!(path_dirname_text("/a/b/", '/'), "/a");
        assert_eq!(
            path_splitext_text("/a/b.py", '/'),
            ("/a/b".into(), ".py".into())
        );
        assert_eq!(path_normpath_text("/a/./b/../c", '/'), "/a/c");
    }

    #[test]
    fn normalization_preserves_byte_names_and_cpython_posix_nt_roots() {
        // Independent CPython 3.12 posixpath/ntpath normpath results; these
        // cases exercise the same byte core from both target separator policies.
        for (input, expected) in [
            ("", "."),
            ("//a/../b", "//b"),
            ("///a/../../b", "/b"),
            ("a/../../b", "../b"),
            ("/a/./b/../c", "/a/c"),
        ] {
            assert_eq!(path_normpath_text(input, '/'), expected);
            assert_eq!(
                path_normpath_raw(input.as_bytes(), b'/'),
                expected.as_bytes()
            );
        }
        assert_eq!(
            path_normpath_raw(b"/a/../\xff/./child", b'/'),
            b"/\xff/child"
        );
        for (input, expected) in [
            (r"C:\a\..\..\b", r"C:\b"),
            (r"C:a\..\..\b", r"C:..\b"),
            (r"\\server\share\..\b", r"\\server\share\b"),
            (r"\\?\UNC\server\share\a\..\b", r"\\?\UNC\server\share\b"),
            (r"\\?\C:\a\..\b", r"\\?\C:\b"),
            (r"C:/a//../b", r"C:\b"),
        ] {
            assert_eq!(path_normpath_text(input, '\\'), expected);
            assert_eq!(
                path_normpath_raw(input.as_bytes(), b'\\'),
                expected.as_bytes()
            );
        }
        assert_eq!(
            path_splitroot_text(r"\\server\share\child", '\\'),
            (r"\\server\share".into(), r"\".into(), "child".into())
        );
    }

    #[test]
    fn projects_normalized_parts() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            path_parts_text(&format!("{sep}a{sep}b"), sep),
            vec![sep.to_string(), "a".into(), "b".into()]
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

        let pattern = ["pkg", "**", "*.py"].map(|part| part.chars().map(u32::from).collect());
        let mut matches = path_glob_matches(&root, &pattern, false, |name| {
            Ok::<_, std::convert::Infallible>(
                name.to_str().unwrap().chars().map(u32::from).collect(),
            )
        })
        .unwrap();
        matches.sort();
        let wildcard = [vec![u32::from(b'*')]];
        assert_eq!(
            path_glob_matches(&root, &wildcard, false, |_| Err::<Vec<u32>, _>(
                "decode-failure"
            )),
            Err("decode-failure")
        );
        assert!(
            path_glob_matches(&root.join("pkg/a.py"), &wildcard, false, |_| Err::<
                Vec<u32>,
                _,
            >(
                "must-not-decode"
            ))
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            matches,
            vec![
                std::path::PathBuf::from(format!("pkg{sep}a.py")),
                std::path::PathBuf::from(format!("pkg{sep}sub{sep}b.py"))
            ]
        );
        let parent_pattern = ["..", "a.py"].map(|part| part.chars().map(u32::from).collect());
        assert_eq!(
            path_glob_matches(&root.join("pkg/sub"), &parent_pattern, false, |name| {
                Ok::<_, std::convert::Infallible>(
                    name.to_str().unwrap().chars().map(u32::from).collect(),
                )
            })
            .unwrap(),
            vec![std::path::PathBuf::from("..").join("a.py")]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
