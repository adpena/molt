//! Runtime-independent importlib support shared by the platform bridge.

use std::collections::HashSet;

pub fn append_unique_path(paths: &mut Vec<String>, entry: &str) {
    if entry.is_empty() {
        return;
    }
    if paths.iter().any(|existing| existing == entry) {
        return;
    }
    paths.push(entry.to_string());
}

pub fn append_unique_path_hashed(paths: &mut Vec<String>, seen: &mut HashSet<String>, entry: &str) {
    if entry.is_empty() {
        return;
    }
    if seen.insert(entry.to_string()) {
        paths.push(entry.to_string());
    }
}

pub fn split_nonempty_paths(raw: &str, sep: char) -> Vec<String> {
    raw.split(sep)
        .filter_map(|part| {
            if part.is_empty() {
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect()
}

pub fn split_zip_archive_path(path: &str) -> Option<(String, String)> {
    const ARCHIVE_SUFFIXES: [&str; 3] = [".zip", ".whl", ".egg"];
    if path.is_empty() {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    let mut best_idx: Option<usize> = None;
    let mut best_suffix_len: usize = 0;
    for suffix in ARCHIVE_SUFFIXES {
        let Some(idx) = lower.rfind(suffix) else {
            continue;
        };
        let archive_end = idx + suffix.len();
        if archive_end < path.len() {
            let tail = path.as_bytes()[archive_end];
            if tail != b'/' && tail != b'\\' {
                continue;
            }
        }
        if best_idx.is_none_or(|current| idx > current) {
            best_idx = Some(idx);
            best_suffix_len = suffix.len();
        }
    }
    let idx = best_idx?;
    let archive_end = idx + best_suffix_len;
    let archive = path[..archive_end].to_string();
    let remainder = path[archive_end..]
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    Some((archive, remainder))
}

pub fn zip_entry_join(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}/{rel}")
    }
}

pub fn importlib_metadata_normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
            continue;
        }
        for lowered in ch.to_lowercase() {
            out.push(lowered);
        }
        prev_sep = false;
    }
    out
}

pub fn importlib_metadata_parse_headers(text: &str) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = Vec::new();
    let mut current_idx: Option<usize> = None;
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            current_idx = None;
            continue;
        }
        let bytes = raw_line.as_bytes();
        if !bytes.is_empty() && (bytes[0] == b' ' || bytes[0] == b'\t') {
            if let Some(idx) = current_idx {
                mapping[idx].1.push('\n');
                mapping[idx].1.push_str(raw_line.trim());
            }
            continue;
        }
        if let Some((key, value)) = raw_line.split_once(':') {
            mapping.push((key.trim().to_string(), value.trim().to_string()));
            current_idx = Some(mapping.len() - 1);
        }
    }
    mapping
}

pub fn importlib_metadata_header_values(headers: &[(String, String)], key: &str) -> Vec<String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            if k.eq_ignore_ascii_case(key) {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}

pub fn importlib_metadata_first_nonempty(
    headers: &[(String, String)],
    key: &str,
) -> Option<String> {
    importlib_metadata_header_values(headers, key)
        .into_iter()
        .find(|value| !value.is_empty())
}

pub fn importlib_metadata_parse_entry_points(text: &str) -> Vec<(String, String, String)> {
    let mut group: Option<String> = None;
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if stripped.starts_with('[') && stripped.ends_with(']') {
            group = Some(stripped[1..stripped.len() - 1].trim().to_string());
            continue;
        }
        let Some(current_group) = group.as_ref() else {
            continue;
        };
        let Some((name, value)) = stripped.split_once('=') else {
            continue;
        };
        out.push((
            name.trim().to_string(),
            value.trim().to_string(),
            current_group.clone(),
        ));
    }
    out
}

pub fn importlib_metadata_parse_csv_row(row: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = row.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek().is_some_and(|next| *next == '"') {
                    current.push('"');
                    let _ = chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            ',' => {
                out.push(current);
                current = String::new();
            }
            '"' => in_quotes = true,
            _ => current.push(ch),
        }
    }
    out.push(current);
    out
}

pub fn importlib_normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn importlib_is_archive_member_path(path: &str) -> bool {
    importlib_normalize_path_text(path).contains(".zip/")
}

pub fn importlib_package_root_from_origin(path: &str) -> Option<String> {
    let normalized = importlib_normalize_path_text(path);
    if normalized.ends_with("/__init__.py") || normalized.ends_with("/__init__.pyc") {
        return normalized
            .rsplit_once('/')
            .map(|(root, _)| root.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_zip_archive_path_uses_rightmost_archive_boundary() {
        assert_eq!(
            split_zip_archive_path(r"C:\pkg.whl\nested\inner.zip\mod.py"),
            Some((
                String::from(r"C:\pkg.whl\nested\inner.zip"),
                String::from("mod.py")
            ))
        );
        assert_eq!(split_zip_archive_path("module.zipdata/path.py"), None);
    }

    #[test]
    fn metadata_headers_preserve_continuations_case_insensitively() {
        let headers = importlib_metadata_parse_headers(
            "Name: Molt\nRequires-Dist: alpha\n beta\nRequires-Dist: gamma\n\nVersion: 1\n",
        );
        assert_eq!(
            importlib_metadata_header_values(&headers, "requires-dist"),
            vec![String::from("alpha\nbeta"), String::from("gamma")]
        );
        assert_eq!(
            importlib_metadata_first_nonempty(&headers, "version"),
            Some(String::from("1"))
        );
    }

    #[test]
    fn metadata_name_normalization_collapses_separator_runs() {
        assert_eq!(
            importlib_metadata_normalize_name("Molt__Runtime..Platform"),
            "molt-runtime-platform"
        );
    }

    #[test]
    fn metadata_csv_parser_handles_quoted_commas_and_quotes() {
        assert_eq!(
            importlib_metadata_parse_csv_row(r#"pkg/"a,b".py,sha256=abc,"12""3""#),
            vec![
                String::from("pkg/a,b.py"),
                String::from("sha256=abc"),
                String::from("12\"3"),
            ]
        );
    }

    #[test]
    fn package_root_from_origin_recognizes_package_init_files() {
        assert_eq!(
            importlib_package_root_from_origin(r"C:\pkg\__init__.py"),
            Some(String::from("C:/pkg"))
        );
        assert_eq!(importlib_package_root_from_origin("pkg/mod.py"), None);
    }

    #[test]
    fn path_list_helpers_skip_empty_and_deduplicate() {
        let mut paths = Vec::new();
        append_unique_path(&mut paths, "");
        append_unique_path(&mut paths, "alpha");
        append_unique_path(&mut paths, "alpha");
        append_unique_path(&mut paths, "beta");
        assert_eq!(paths, vec![String::from("alpha"), String::from("beta")]);

        let mut hashed = Vec::new();
        let mut seen = HashSet::new();
        append_unique_path_hashed(&mut hashed, &mut seen, "alpha");
        append_unique_path_hashed(&mut hashed, &mut seen, "alpha");
        append_unique_path_hashed(&mut hashed, &mut seen, "beta");
        assert_eq!(hashed, vec![String::from("alpha"), String::from("beta")]);
        assert_eq!(
            split_nonempty_paths("alpha::beta:", ':'),
            vec![String::from("alpha"), String::from("beta")]
        );
    }
}
