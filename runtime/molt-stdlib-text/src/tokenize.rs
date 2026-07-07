//! Pure tokenize and source-encoding algorithms for runtime stdlib support.
//!
//! Molt object allocation stays in `molt-runtime`; this module owns the
//! deterministic byte/string scanning and PEP-263-style encoding-cookie logic.

use memchr::{memchr, memmem};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Endmarker,
    Name,
    Number,
    Newline,
    Op,
    Comment,
    Nl,
}

impl TokenKind {
    pub fn code(self) -> i64 {
        match self {
            Self::Endmarker => 0,
            Self::Name => 1,
            Self::Number => 2,
            Self::Newline => 4,
            Self::Op => 54,
            Self::Comment => 64,
            Self::Nl => 65,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenRecord {
    pub kind: TokenKind,
    pub text: String,
    pub start: (i64, i64),
    pub end: (i64, i64),
    pub line_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenScan {
    pub lines: Vec<String>,
    pub tokens: Vec<TokenRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodingDetection {
    pub encoding: String,
    pub bom_found: bool,
}

#[inline]
fn is_name_start(ch: u8) -> bool {
    ch == b'_' || ch.is_ascii_alphabetic()
}

#[inline]
fn is_name_char(ch: u8) -> bool {
    is_name_start(ch) || ch.is_ascii_digit()
}

#[inline]
fn push_token(
    tokens: &mut Vec<TokenRecord>,
    kind: TokenKind,
    text: impl Into<String>,
    start: (i64, i64),
    end: (i64, i64),
    line_index: usize,
) {
    tokens.push(TokenRecord {
        kind,
        text: text.into(),
        start,
        end,
        line_index,
    });
}

pub fn scan_tokens(source: &str) -> TokenScan {
    let mut lines = Vec::new();
    let mut tokens = Vec::new();
    let source_bytes = source.as_bytes();
    let mut line_no: i64 = 1;

    if !source_bytes.is_empty() {
        let mut start = 0usize;
        while start < source_bytes.len() {
            let line_end = memchr(b'\n', &source_bytes[start..])
                .map(|rel| start + rel + 1)
                .unwrap_or(source_bytes.len());
            let line = &source[start..line_end];
            let line_index = lines.len();
            lines.push(line.to_string());
            let line_bytes = line.as_bytes();
            let line_len = line_bytes.len();

            let trimmed_start = line_bytes.iter().position(|&b| b != b' ' && b != b'\t');
            if let Some(ts) = trimmed_start
                && line_bytes[ts] == b'#'
            {
                let comment = line.trim();
                push_token(
                    &mut tokens,
                    TokenKind::Comment,
                    comment,
                    (line_no, 0),
                    (line_no, comment.len() as i64),
                    line_index,
                );
                if line.ends_with('\n') {
                    push_token(
                        &mut tokens,
                        TokenKind::Nl,
                        "\n",
                        (line_no, (line_len - 1) as i64),
                        (line_no, line_len as i64),
                        line_index,
                    );
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
                    push_token(
                        &mut tokens,
                        TokenKind::Comment,
                        comment,
                        (line_no, col as i64),
                        (line_no, (col + comment.len()) as i64),
                        line_index,
                    );
                    break;
                }
                if is_name_start(ch) {
                    let start_col = col;
                    col += 1;
                    while col < line_len && is_name_char(line_bytes[col]) {
                        col += 1;
                    }
                    push_token(
                        &mut tokens,
                        TokenKind::Name,
                        &line[start_col..col],
                        (line_no, start_col as i64),
                        (line_no, col as i64),
                        line_index,
                    );
                    continue;
                }
                if ch.is_ascii_digit() {
                    let start_col = col;
                    col += 1;
                    while col < line_len && line_bytes[col].is_ascii_digit() {
                        col += 1;
                    }
                    push_token(
                        &mut tokens,
                        TokenKind::Number,
                        &line[start_col..col],
                        (line_no, start_col as i64),
                        (line_no, col as i64),
                        line_index,
                    );
                    continue;
                }
                push_token(
                    &mut tokens,
                    TokenKind::Op,
                    &line[col..col + 1],
                    (line_no, col as i64),
                    (line_no, (col + 1) as i64),
                    line_index,
                );
                col += 1;
            }

            if line.ends_with('\n') {
                let stripped = line.trim();
                let has_content = !stripped.is_empty() && !stripped.starts_with('#');
                let kind = if has_content {
                    TokenKind::Newline
                } else {
                    TokenKind::Nl
                };
                push_token(
                    &mut tokens,
                    kind,
                    "\n",
                    (line_no, (line_len - 1) as i64),
                    (line_no, line_len as i64),
                    line_index,
                );
            }
            line_no += 1;
            if line_end == source_bytes.len() {
                break;
            }
            start = line_end;
        }
    }

    let end_line_index = lines.len();
    lines.push(String::new());
    push_token(
        &mut tokens,
        TokenKind::Endmarker,
        "",
        (line_no, 0),
        (line_no, 0),
        end_line_index,
    );

    TokenScan { lines, tokens }
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

pub fn detect_source_encoding(first_bytes: &[u8], second_bytes: &[u8]) -> EncodingDetection {
    let bom_utf8: &[u8] = &[0xEF, 0xBB, 0xBF];
    let mut bom_found = false;
    let mut effective_first = first_bytes;
    let mut default_enc = "utf-8";

    if effective_first.starts_with(bom_utf8) {
        bom_found = true;
        effective_first = &effective_first[3..];
        default_enc = "utf-8-sig";
    }

    if let Some(encoding) = find_encoding_cookie(effective_first) {
        let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
            "utf-8-sig"
        } else {
            encoding
        };
        return EncodingDetection {
            encoding: encoding.to_string(),
            bom_found,
        };
    }

    if !second_bytes.is_empty()
        && let Some(encoding) = find_encoding_cookie(second_bytes)
    {
        let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
            "utf-8-sig"
        } else {
            encoding
        };
        return EncodingDetection {
            encoding: encoding.to_string(),
            bom_found,
        };
    }

    EncodingDetection {
        encoding: default_enc.to_string(),
        bom_found,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type ObservedToken<'a> = (i64, &'a str, (i64, i64), (i64, i64), &'a str);

    #[test]
    fn scan_tokens_matches_runtime_tuple_contract() {
        let scan = scan_tokens("name = 42 # hi\n# full\n");
        let observed: Vec<ObservedToken<'_>> = scan
            .tokens
            .iter()
            .map(|token| {
                (
                    token.kind.code(),
                    token.text.as_str(),
                    token.start,
                    token.end,
                    scan.lines[token.line_index].as_str(),
                )
            })
            .collect();
        assert_eq!(
            observed,
            [
                (1, "name", (1, 0), (1, 4), "name = 42 # hi\n"),
                (54, "=", (1, 5), (1, 6), "name = 42 # hi\n"),
                (2, "42", (1, 7), (1, 9), "name = 42 # hi\n"),
                (64, "# hi", (1, 10), (1, 14), "name = 42 # hi\n"),
                (4, "\n", (1, 14), (1, 15), "name = 42 # hi\n"),
                (64, "# full", (2, 0), (2, 6), "# full\n"),
                (65, "\n", (2, 6), (2, 7), "# full\n"),
                (0, "", (3, 0), (3, 0), ""),
            ]
        );
    }

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

    #[test]
    fn detect_source_encoding_handles_bom_and_second_line_cookie() {
        assert_eq!(
            detect_source_encoding(b"\xEF\xBB\xBF# coding: utf-8", b""),
            EncodingDetection {
                encoding: "utf-8-sig".to_string(),
                bom_found: true,
            }
        );
        assert_eq!(
            detect_source_encoding(b"#!/usr/bin/python\n", b"# coding=latin-1\n"),
            EncodingDetection {
                encoding: "latin-1".to_string(),
                bom_found: false,
            }
        );
        assert_eq!(
            detect_source_encoding(b"", b""),
            EncodingDetection {
                encoding: "utf-8".to_string(),
                bom_found: false,
            }
        );
    }
}
