use super::*;
use crate::builtins::exceptions::{
    ExceptionFieldSlot, ExceptionStorage, ExceptionValue, exception_class, exception_field,
    exception_is_instance,
};
use crate::object::ops_format::{format_obj_str_bytes, string_obj_bytes};
use rustpython_parser::ast::Ranged;
use rustpython_parser::{Mode as ParseMode, ast as pyast, parse as parse_python};
use std::collections::HashSet;

pub(crate) fn traceback_limit_from_bits(
    _py: &PyToken<'_>,
    limit_bits: u64,
) -> Result<Option<usize>, u64> {
    let obj = obj_from_bits(limit_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(limit) = to_i64(obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "limit must be an integer",
        ));
    };
    if limit < 0 {
        return Ok(Some(0));
    }
    Ok(Some(limit as usize))
}

/// Attribute dispatch owns representation and descriptor behavior for both
/// managed and native traceback/frame objects. Each result stays owned across
/// later lookups, which may call Python and replace neighboring fields.
fn traceback_attribute<'a, 'py>(
    py: &'a PyToken<'py>,
    value: u64,
    name: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    let ptr = obj_from_bits(value).as_ptr()?;
    unsafe { crate::builtins::attr::attr_lookup_ptr_allow_missing(py, ptr, name) }
        .map(|bits| ExceptionValue::adopt(py, bits))
}

fn traceback_frame_info(py: &PyToken<'_>, frame: u64) -> Option<(Vec<u8>, i64, Vec<u8>)> {
    let interned = &runtime_state(py).interned;
    let code_name = intern_static_name(py, &interned.f_code_name, b"f_code");
    let line_name = intern_static_name(py, &interned.f_lineno_name, b"f_lineno");
    let code = traceback_attribute(py, frame, code_name);
    if exception_pending(py) {
        return None;
    }
    let line = traceback_attribute(py, frame, line_name);
    if exception_pending(py) {
        return None;
    }
    if code.is_none() && line.is_none() {
        return None;
    }
    let lineno = line
        .as_ref()
        .and_then(|value| to_i64(obj_from_bits(value.bits())))
        .unwrap_or(0);
    let mut filename = b"<unknown>".to_vec();
    let mut function = b"<module>".to_vec();
    if let Some(code) = code {
        let filename_name =
            ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"co_filename")?);
        let function_name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"co_name")?);
        let file = traceback_attribute(py, code.bits(), filename_name.bits());
        if exception_pending(py) {
            return None;
        }
        let name = traceback_attribute(py, code.bits(), function_name.bits());
        if exception_pending(py) {
            return None;
        }
        if let Some(file) = file.and_then(|value| string_obj_bytes(obj_from_bits(value.bits()))) {
            filename = file;
        }
        if let Some(name) = name.and_then(|value| string_obj_bytes(obj_from_bits(value.bits())))
            && !name.is_empty()
        {
            function = name;
        }
    }
    Some((filename, lineno, function))
}

pub(crate) fn traceback_frames(
    py: &PyToken<'_>,
    tb_bits: u64,
    limit: Option<usize>,
) -> Vec<(Vec<u8>, i64, Vec<u8>)> {
    let interned = &runtime_state(py).interned;
    let frame_name = intern_static_name(py, &interned.tb_frame_name, b"tb_frame");
    let line_name = intern_static_name(py, &interned.tb_lineno_name, b"tb_lineno");
    let next_name = intern_static_name(py, &interned.tb_next_name, b"tb_next");
    let mut out = Vec::new();
    let mut current = ExceptionValue::pin(py, tb_bits);
    let mut seen = HashSet::new();
    let mut owners = Vec::new();
    while !obj_from_bits(current.bits()).is_none() {
        if limit.is_some_and(|max| out.len() >= max) || !seen.insert(current.bits()) {
            break;
        }
        let Some(frame) = traceback_attribute(py, current.bits(), frame_name) else {
            break;
        };
        let line = traceback_attribute(py, current.bits(), line_name);
        if exception_pending(py) {
            return Vec::new();
        }
        let next = traceback_attribute(py, current.bits(), next_name);
        if exception_pending(py) {
            return Vec::new();
        }
        let info = traceback_frame_info(py, frame.bits());
        if exception_pending(py) {
            return Vec::new();
        }
        let (filename, frame_line, name) =
            info.unwrap_or_else(|| (b"<unknown>".to_vec(), 0, b"<module>".to_vec()));
        let lineno = line
            .as_ref()
            .and_then(|value| to_i64(obj_from_bits(value.bits())))
            .filter(|line| *line > 0)
            .unwrap_or(frame_line);
        out.push((filename, lineno, name));
        let Some(next) = next else {
            break;
        };
        owners.push(current);
        current = next;
    }
    out
}
pub(crate) fn traceback_source_line_native(
    _py: &PyToken<'_>,
    filename: &[u8],
    lineno: i64,
) -> Vec<u8> {
    traceback_source_span_native(_py, filename, lineno, lineno)
}

fn traceback_source_span_native(
    _py: &PyToken<'_>,
    filename: &[u8],
    lineno: i64,
    end_lineno: i64,
) -> Vec<u8> {
    if lineno <= 0 {
        return Vec::new();
    }
    let Ok(filename) = std::str::from_utf8(filename) else {
        return Vec::new();
    };
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "traceback.source_line",
        "fs.read",
        AuditArgs::Path(filename.to_string()),
        allowed,
    );
    if !allowed {
        return Vec::new();
    }
    let Ok(file) = std::fs::File::open(filename) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let full_span = runtime_target_at_least(_py, 3, 13);
    let last = if full_span {
        end_lineno.max(lineno)
    } else {
        lineno
    };
    let mut source = String::new();
    for (idx, line_result) in reader.lines().enumerate() {
        let number = idx as i64 + 1;
        if number < lineno {
            continue;
        }
        if number > last {
            break;
        }
        let Ok(line) = line_result else {
            return Vec::new();
        };
        // 3.12 observes the raw linecache line, including its normalized EOF
        // newline. 3.13+ FrameSummary._set_lines strips each captured source
        // line before joining the span. Explicitly supplied lines bypass this.
        source.push_str(if full_span { line.trim_end() } else { &line });
        source.push('\n');
        if number == last {
            break;
        }
    }
    source.into_bytes()
}

fn traceback_chars(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    crate::object::ops_string::wtf8_from_bytes(bytes)
        .code_points()
        .map(|cp| cp.to_u32())
}

fn traceback_whitespace(code: u32) -> bool {
    char::from_u32(code).is_some_and(char::is_whitespace)
}

fn traceback_trim(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end {
        let (next, code) = crate::object::ops_string::wtf8_step(bytes, start, false).unwrap();
        if !traceback_whitespace(code) {
            break;
        }
        start = next;
    }
    while end > start {
        let (previous, code) = crate::object::ops_string::wtf8_step(bytes, end, true).unwrap();
        if !traceback_whitespace(code) {
            break;
        }
        end = previous;
    }
    &bytes[start..end]
}

fn traceback_slice(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    let mut byte_start = 0;
    let mut byte_end = 0;
    let mut cursor = 0;
    let mut index = 0;
    while let Some((next, _)) = crate::object::ops_string::wtf8_step(bytes, cursor, false) {
        if index < start {
            byte_start = next;
        }
        if index < end {
            byte_end = next;
        }
        cursor = next;
        index += 1;
    }
    &bytes[byte_start..byte_end.max(byte_start)]
}

fn traceback_text_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .map(|line| {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            line.strip_suffix(b"\r").unwrap_or(line)
        })
        .collect()
}

fn traceback_display_width(line: impl AsRef<[u8]>, offset: usize, minor: i64) -> usize {
    let line = line.as_ref();
    if line.is_ascii() {
        return offset;
    }
    traceback_chars(line)
        .take(offset)
        .map(
            |code| match crate::object::ops::unicode_east_asian_width_table::width(code, minor) {
                "W" | "F" => 2,
                _ => 1,
            },
        )
        .sum()
}

/// Return the CPython caret anchor as character offsets in the selected source
/// segment. AST ranges and lexer token ranges are UTF-8 byte offsets.
fn traceback_caret_anchor(segment: &str, include_calls: bool) -> Option<(usize, usize)> {
    let wrapped = if include_calls {
        format!("(\n{segment}\n)")
    } else {
        segment.to_owned()
    };
    let pyast::Mod::Module(module) =
        parse_python(&wrapped, ParseMode::Module, "<traceback>").ok()?
    else {
        return None;
    };
    let [pyast::Stmt::Expr(statement)] = module.body.as_slice() else {
        return None;
    };
    let expression = statement.value.as_ref();
    let chars: Vec<char> = segment.chars().collect();
    let normalize =
        |byte: usize| traceback_byte_offset_to_char_offset(segment, byte as i64) as usize;
    // 3.12 parses the unwrapped segment and uses AST columns. Its BinOp
    // algorithm intentionally mixes byte columns and character-indexed scanning
    // before a second byte conversion. Preserve that observable versioned rule,
    // including its non-ASCII placement, rather than guessing a token anchor.
    if !include_calls {
        let column = |offset| {
            let byte = u32::from(offset) as usize;
            wrapped[..byte].rsplit('\n').next().unwrap_or("").len()
        };
        return match expression {
            pyast::Expr::BinOp(node) => {
                let left = column(node.left.range().end());
                let right = column(node.right.range().start());
                let gap = chars.get(normalize(left)..normalize(right))?;
                let whitespace = gap.iter().take_while(|ch| ch.is_whitespace()).count();
                let mut start = left + whitespace;
                let mut end = start + 1;
                if gap
                    .get(whitespace + 1)
                    .is_some_and(|ch| !ch.is_whitespace())
                {
                    end += 1;
                }
                while chars
                    .get(start)
                    .is_some_and(|ch| ch.is_whitespace() || matches!(ch, ')' | '#'))
                {
                    start += 1;
                    end += 1;
                }
                Some((normalize(start), normalize(end)))
            }
            pyast::Expr::Subscript(node) => {
                let mut start = normalize(column(node.value.range().end()));
                let mut end = normalize(column(node.slice.range().end()) + 1);
                while chars.get(start).is_some_and(|ch| *ch != '[') {
                    start += 1;
                }
                while chars.get(end).is_some_and(|ch| *ch != ']') {
                    end += 1;
                }
                if end < chars.len() {
                    end += 1;
                }
                Some((start, end))
            }
            _ => None,
        };
    }
    let position = |offset| normalize((u32::from(offset) as usize).saturating_sub(2));
    let scan = |mut index: usize, stop: fn(char) -> bool| -> Option<usize> {
        while let Some(&ch) = chars.get(index) {
            if matches!(ch, '\\' | '#') {
                while chars.get(index).is_some_and(|ch| *ch != '\n') {
                    index += 1;
                }
            } else if stop(ch) {
                return Some(index);
            }
            index += 1;
        }
        None
    };
    match expression {
        pyast::Expr::BinOp(node) => {
            let start = scan(position(node.left.range().end()), |ch| {
                !ch.is_whitespace() && ch != ')'
            })?;
            let mut end = start + 1;
            if end < position(node.right.range().start())
                && chars
                    .get(end)
                    .is_some_and(|ch| !ch.is_whitespace() && !matches!(ch, '\\' | '#'))
            {
                end += 1;
            }
            Some((start, end))
        }
        pyast::Expr::Subscript(node) => Some((
            scan(position(node.value.range().end()), |ch| ch == '[')?,
            position(node.range.end()),
        )),
        pyast::Expr::Call(node) => Some((
            scan(position(node.func.range().end()), |ch| ch == '(')?,
            position(node.range.end()),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod traceback_format_tests {
    use super::{
        PythonVersionInfo, TracebackPayloadFrame, format_sys_version, traceback_caret_anchor,
        traceback_payload_format_frame, traceback_payload_frame_source_lines_for_target,
        traceback_payload_to_formatted_entries, traceback_summary_caret_plan,
    };

    #[test]
    fn formatted_traceback_entries_keep_each_frame_together() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let mut payload = [
                TracebackPayloadFrame {
                    filename: "<first>".to_string().into_bytes(),
                    lineno: 3,
                    end_lineno: 3,
                    colno: 8,
                    end_colno: 14,
                    name: "first".to_string().into_bytes(),
                    line: "value = source".to_string().into_bytes(),
                },
                TracebackPayloadFrame {
                    filename: "<second>".to_string().into_bytes(),
                    lineno: 9,
                    end_lineno: 9,
                    colno: -1,
                    end_colno: -1,
                    name: "second".to_string().into_bytes(),
                    line: Vec::new(),
                },
            ];
            let state = crate::runtime_state(py);
            let saved_version = state.sys_version_info.lock().unwrap().clone();
            for (minor, expected) in [
                (
                    12,
                    "  File \"<first>\", line 3, in first\n    value = source\n             ^^^^^^\n",
                ),
                (
                    13,
                    "  File \"<first>\", line 3, in first\n    value = source\n            ^^^^^^\n",
                ),
                (
                    14,
                    "  File \"<first>\", line 3, in first\n    value = source\n            ^^^^^^\n",
                ),
            ] {
                *state.sys_version_info.lock().unwrap() = Some(PythonVersionInfo {
                    major: 3,
                    minor,
                    micro: 0,
                    releaselevel: "final".to_string(),
                    serial: 0,
                });
                let entries = traceback_payload_to_formatted_entries(py, &payload)
                    .unwrap()
                    .into_iter()
                    .map(|line| String::from_utf8(line).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0], expected, "target Python 3.{minor}");
                assert_eq!(entries[1], "  File \"<second>\", line 9, in second\n");
                payload[0].line.push(b'\n');
                let captured = traceback_payload_to_formatted_entries(py, &payload)
                    .unwrap()
                    .into_iter()
                    .map(|line| String::from_utf8(line).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(
                    captured[0],
                    "  File \"<first>\", line 3, in first\n    value = source\n            ^^^^^^\n",
                    "newline retained in captured source for Python 3.{minor}"
                );
                payload[0].line.pop();
            }
            *state.sys_version_info.lock().unwrap() = saved_version;
            crate::MoltObject::none().bits()
        });
    }

    #[test]
    fn explicit_source_without_columns_is_trimmed_without_inferred_caret() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let frame = TracebackPayloadFrame {
                filename: "<explicit>".to_string().into_bytes(),
                lineno: 4,
                end_lineno: 4,
                colno: -1,
                end_colno: -1,
                name: "plain".to_string().into_bytes(),
                line: "    x = 1".to_string().into_bytes(),
            };
            assert_eq!(
                String::from_utf8(traceback_payload_format_frame(py, &frame).unwrap()).unwrap(),
                "  File \"<explicit>\", line 4, in plain\n    x = 1\n"
            );
            crate::MoltObject::none().bits()
        });
    }

    #[test]
    fn source_columns_reject_surrogates_while_plain_python_text_is_lossless() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let mut frame = TracebackPayloadFrame {
                filename: b"\xed\xa0\x80.py".to_vec(),
                lineno: 3,
                end_lineno: 3,
                colno: -1,
                end_colno: -1,
                name: b"\xed\xbf\xbf".to_vec(),
                line: b"  x = '\xed\xa0\x80'".to_vec(),
            };
            let expected =
                b"  File \"\xed\xa0\x80.py\", line 3, in \xed\xbf\xbf\n    x = '\xed\xa0\x80'\n";
            assert_eq!(
                traceback_payload_format_frame(py, &frame).unwrap(),
                expected
            );
            assert!(!crate::exception_pending(py));
            frame.colno = 2;
            frame.end_colno = 9;
            assert!(traceback_payload_to_formatted_entries(py, &[frame]).is_err());
            let error = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "UnicodeEncodeError"
            ));
            crate::molt_exception_clear();
            let rendered = crate::molt_str_from_obj(error);
            assert_eq!(crate::object::ops_format::string_obj_bytes(crate::obj_from_bits(rendered)).unwrap(),
                b"'utf-8' codec can't encode character '\\ud800' in position 7: surrogates not allowed");
            crate::dec_ref_bits(py, rendered);
            crate::dec_ref_bits(py, error);
        });
    }

    #[test]
    fn public_summary_span_is_first_line_only_in_312_and_multiline_in_313() {
        let frame = TracebackPayloadFrame {
            filename: "<multiline>".to_string().into_bytes(),
            lineno: 1,
            end_lineno: 3,
            colno: 8,
            end_colno: 5,
            name: "demo".to_string().into_bytes(),
            line: "alpha = (\n    1 +\n    2".to_string().into_bytes(),
        };
        let lines_312 = traceback_payload_frame_source_lines_for_target(&frame, 12)
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines_312[0], "    alpha = (\n");
        assert!(!lines_312.iter().any(|line| line.contains("1 +")));
        assert!(!lines_312.iter().any(|line| line.trim() == "2"));
        assert_eq!(
            lines_312.iter().filter(|line| line.contains('^')).count(),
            1
        );

        let lines_313 = traceback_payload_frame_source_lines_for_target(&frame, 13)
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect::<Vec<_>>();
        assert!(lines_313.iter().any(|line| line.trim() == "alpha = ("));
        assert!(lines_313.iter().any(|line| line.trim() == "1 +"));
        assert!(lines_313.iter().any(|line| line.trim() == "2"));
        assert!(lines_313.iter().filter(|line| line.contains('^')).count() >= 2);
    }

    #[test]
    fn public_summary_full_line_without_anchor_has_no_caret() {
        let frame = TracebackPayloadFrame {
            filename: "<tabs>".to_string().into_bytes(),
            lineno: 5,
            end_lineno: 5,
            colno: 1,
            end_colno: 999,
            name: "boom".to_string().into_bytes(),
            line: "\tassert value and (".to_string().into_bytes(),
        };
        for minor in [12, 13, 14] {
            assert_eq!(
                traceback_payload_frame_source_lines_for_target(&frame, minor)
                    .into_iter()
                    .map(|line| String::from_utf8(line).unwrap())
                    .collect::<Vec<_>>(),
                ["    assert value and (\n"]
            );
        }
    }

    #[test]
    fn aggregate_anchors_follow_target_ast_and_offset_semantics() {
        for (text, byte, character) in [("é", 1, 1), ("漢", 1, 1), ("漢", 2, 1), ("漢", 3, 1)] {
            assert_eq!(
                super::traceback_byte_offset_to_char_offset(text, byte),
                character
            );
        }
        assert_eq!(traceback_caret_anchor("a + b", false), Some((2, 3)));
        assert_eq!(traceback_caret_anchor("(a+b) * c", false), Some((6, 7)));
        assert_eq!(traceback_caret_anchor("café / 0", false), Some((6, 7)));
        assert_eq!(traceback_caret_anchor("café / 0", true), Some((5, 6)));
        for (source, expected) in [
            ("éé / 0", (3, 4)),
            ("漢字 / 0", (3, 4)),
            ("(éé) / 0", (3, 4)),
            ("éé ** 0", (4, 6)),
            ("éé + z + q", (7, 8)),
        ] {
            assert_eq!(
                traceback_caret_anchor(source, false),
                Some(expected),
                "{source}"
            );
        }
        assert_eq!(traceback_caret_anchor("a[\"=\"]", false), Some((1, 6)));
        assert_eq!(traceback_caret_anchor("f(x=1)", false), None);
        assert_eq!(traceback_caret_anchor("f(x=1)", true), Some((1, 6)));
        assert_eq!(traceback_caret_anchor("a == b", true), None);
        assert_eq!(traceback_caret_anchor("\"a+b\"", true), None);
    }

    #[test]
    fn aggregate_full_line_suppression_uses_parsed_anchor() {
        let one = |source: &str| [source.as_bytes().to_vec()];
        assert!(traceback_summary_caret_plan(&one("a + b"), 0, 5, false).0);
        assert!(traceback_summary_caret_plan(&one("a[0]"), 0, 4, false).0);
        assert!(!traceback_summary_caret_plan(&one("f(x=1)"), 0, 6, false).0);
        assert!(traceback_summary_caret_plan(&one("f(x=1)"), 0, 6, true).0);
        assert!(!traceback_summary_caret_plan(&one("x = y"), 0, 5, true).0);
        assert!(!traceback_summary_caret_plan(&one("a == b"), 0, 6, true).0);
        assert!(!traceback_summary_caret_plan(&one("obj.attr"), 0, 8, true).0);
        assert!(!traceback_summary_caret_plan(&one("\"a+b\""), 0, 5, true).0);
        assert!(traceback_summary_caret_plan(&one("x = a + b"), 4, 9, true).0);
        assert!(!traceback_summary_caret_plan(&one("x = f()"), 4, 7, true).0);
    }

    #[test]
    fn formatted_traceback_entries_compact_consecutive_recursive_frames() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let frame = TracebackPayloadFrame {
                filename: "<recursive>".to_string().into_bytes(),
                lineno: 7,
                end_lineno: 7,
                colno: -1,
                end_colno: -1,
                name: "recurse".to_string().into_bytes(),
                line: "recurse()".to_string().into_bytes(),
            };
            let entries = traceback_payload_to_formatted_entries(py, &vec![frame; 5])
                .unwrap()
                .into_iter()
                .map(|line| String::from_utf8(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(entries.len(), 4);
            assert!(
                entries[..3]
                    .iter()
                    .all(|entry| entry.starts_with("  File \"<recursive>\", line 7, in recurse\n"))
            );
            assert_eq!(entries[3], "  [Previous line repeated 2 more times]\n");
            crate::MoltObject::none().bits()
        });
    }

    #[test]
    fn diagnostic_width_and_public_ucd_version_follow_target_not_host() {
        use crate::object::ops::unicode_east_asian_width_table::{for_minor, width};
        for (minor, version, emoji_width) in [
            (12, "15.0.0", "N"),
            (13, "15.1.0", "N"),
            (14, "16.0.0", "W"),
        ] {
            assert_eq!(for_minor(minor).unwrap().0, version);
            assert_eq!(width(0x1fae9, minor), emoji_width);
            assert_eq!(width(0x231a, minor), "W");
            assert_eq!(width(0xd800, minor), "N");
            assert_eq!(width(0xff21, minor), "F");
            assert_eq!(width(0x0301, minor), "A");
            assert_eq!(
                super::traceback_display_width("\u{1fae9}", 1, minor),
                if minor == 14 { 2 } else { 1 }
            );
        }
        assert!(for_minor(99).is_none());
    }

    #[test]
    fn format_sys_version_final_release() {
        let info = PythonVersionInfo {
            major: 3,
            minor: 12,
            micro: 7,
            releaselevel: "final".to_string(),
            serial: 0,
        };
        assert_eq!(format_sys_version(&info), "3.12.7 (molt)");
    }

    #[test]
    fn format_sys_version_candidate_release() {
        let info = PythonVersionInfo {
            major: 3,
            minor: 13,
            micro: 0,
            releaselevel: "candidate".to_string(),
            serial: 2,
        };
        assert_eq!(format_sys_version(&info), "3.13.0rc2 (molt)");
    }
}

pub(crate) fn traceback_format_exception_only_line(
    py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
) -> Vec<u8> {
    let value = obj_from_bits(value_bits);
    let class_bits = if !obj_from_bits(exc_type_bits).is_none() {
        exc_type_bits
    } else if !value.is_none() {
        type_of_bits(py, value_bits)
    } else {
        0
    };
    let mut line = if let Some(storage) = ExceptionStorage::for_exception(py, value_bits) {
        storage.class_name().into_bytes()
    } else {
        obj_from_bits(class_bits)
            .as_ptr()
            .filter(|class| unsafe { object_type_id(*class) } == TYPE_ID_TYPE)
            .and_then(|class| string_obj_bytes(obj_from_bits(unsafe { class_name_bits(class) })))
            .unwrap_or_else(|| b"Exception".to_vec())
    };
    if !value.is_none() {
        let message = if let Some(ptr) = value.as_ptr()
            && exception_is_instance(py, value_bits)
        {
            crate::builtins::exceptions::format_exception_message_bytes(py, ptr)
        } else {
            format_obj_str_bytes(py, value)
        };
        if !message.is_empty() {
            line.extend_from_slice(b": ");
            line.extend_from_slice(&message);
        }
    }
    line.push(b'\n');
    line
}

pub(crate) fn traceback_exception_type<'a, 'py>(
    py: &'a PyToken<'py>,
    value_bits: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    if exception_is_instance(py, value_bits) {
        exception_class(py, value_bits)
    } else {
        let bits = if obj_from_bits(value_bits).is_none() {
            MoltObject::none().bits()
        } else {
            type_of_bits(py, value_bits)
        };
        Some(ExceptionValue::pin(py, bits))
    }
}

pub(crate) fn traceback_exception_trace<'a, 'py>(
    py: &'a PyToken<'py>,
    value_bits: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    if exception_is_instance(py, value_bits) {
        exception_field(py, value_bits, ExceptionFieldSlot::Traceback)
    } else {
        Some(ExceptionValue::pin(py, MoltObject::none().bits()))
    }
}

pub(crate) struct TracebackExceptionLink<'a, 'py> {
    pub(crate) value: ExceptionValue<'a, 'py>,
    /// Separator between this exception and the preceding, newer exception.
    pub(crate) separator: Option<&'static str>,
}

/// One CPython chain policy for diagnostic and public traceback rendering.
/// Explicit causes always take precedence; suppression applies only to context.
/// Retain every visited identity so callbacks cannot recycle an identity while
/// rendering, and terminate before a cycle would repeat an exception.
pub(crate) fn traceback_exception_chain<'a, 'py>(
    py: &'a PyToken<'py>,
    value: u64,
) -> Option<Vec<TracebackExceptionLink<'a, 'py>>> {
    let mut chain = vec![TracebackExceptionLink {
        value: ExceptionValue::pin(py, value),
        separator: None,
    }];
    let mut seen = HashSet::from([value]);
    loop {
        let current = chain
            .last()
            .expect("exception chain has its root")
            .value
            .bits();
        let Some(storage) = ExceptionStorage::for_exception(py, current) else {
            return raise_exception(py, "TypeError", "value must be an exception instance");
        };
        let cause = exception_field(py, current, ExceptionFieldSlot::Cause)?;
        let (next, separator) = if !obj_from_bits(cause.bits()).is_none() {
            (
                cause,
                "\nThe above exception was the direct cause of the following exception:\n\n",
            )
        } else if !storage.suppress_context() {
            (
                exception_field(py, current, ExceptionFieldSlot::Context)?,
                "\nDuring handling of the above exception, another exception occurred:\n\n",
            )
        } else {
            break;
        };
        if obj_from_bits(next.bits()).is_none() || !seen.insert(next.bits()) {
            break;
        }
        if !exception_is_instance(py, next.bits()) {
            return raise_exception(
                py,
                "TypeError",
                "exception chain must contain exception instances",
            );
        }
        chain.push(TracebackExceptionLink {
            value: next,
            separator: Some(separator),
        });
    }
    Some(chain)
}

pub(crate) fn traceback_append_exception_single_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    out: &mut Vec<Vec<u8>>,
) {
    if exception_pending(_py) {
        return;
    }
    if !obj_from_bits(tb_bits).is_none() {
        out.push(b"Traceback (most recent call last):\n".to_vec());
        let payload = traceback_payload_from_source(_py, tb_bits, limit);
        if exception_pending(_py) {
            return;
        }
        let Ok(entries) = traceback_payload_to_formatted_entries(_py, &payload) else {
            return;
        };
        out.extend(entries);
    }
    out.push(traceback_format_exception_only_line(
        _py,
        exc_type_bits,
        value_bits,
    ));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn traceback_append_exception_chain_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    chain: bool,
    out: &mut Vec<Vec<u8>>,
) {
    if exception_pending(_py) {
        return;
    }
    if !exception_is_instance(_py, value_bits) || !chain {
        traceback_append_exception_single_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            out,
        );
        return;
    }
    let Some(entries) = traceback_exception_chain(_py, value_bits) else {
        return;
    };
    for index in (0..entries.len()).rev() {
        if index + 1 < entries.len() {
            out.push(
                entries[index + 1]
                    .separator
                    .expect("linked exception")
                    .as_bytes()
                    .to_vec(),
            );
        }
        if index == 0 {
            traceback_append_exception_single_lines(
                _py,
                exc_type_bits,
                value_bits,
                tb_bits,
                limit,
                out,
            );
        } else {
            let value = entries[index].value.bits();
            let Some(class) = traceback_exception_type(_py, value) else {
                return;
            };
            let Some(traceback) = traceback_exception_trace(_py, value) else {
                return;
            };
            traceback_append_exception_single_lines(
                _py,
                class.bits(),
                value,
                traceback.bits(),
                limit,
                out,
            );
        }
        if exception_pending(_py) {
            return;
        }
    }
}

pub(crate) fn traceback_lines_to_list(_py: &PyToken<'_>, lines: &[Vec<u8>]) -> u64 {
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let mut bits_vec: Vec<u64> = Vec::with_capacity(lines.len());
    for line in lines {
        let ptr = alloc_string(_py, line);
        if ptr.is_null() {
            for bits in bits_vec {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        bits_vec.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, bits_vec.as_slice());
    for bits in bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[derive(Clone)]
pub(crate) struct TracebackPayloadFrame {
    pub(crate) filename: Vec<u8>,
    pub(crate) lineno: i64,
    pub(crate) end_lineno: i64,
    pub(crate) colno: i64,
    pub(crate) end_colno: i64,
    pub(crate) name: Vec<u8>,
    pub(crate) line: Vec<u8>,
}

pub(crate) struct TracebackExceptionChainNode<'a, 'py> {
    pub(crate) value: ExceptionValue<'a, 'py>,
    pub(crate) frames: Vec<TracebackPayloadFrame>,
    pub(crate) suppress_context: bool,
    pub(crate) cause_index: Option<usize>,
    pub(crate) context_index: Option<usize>,
}

pub(crate) fn traceback_split_molt_symbol(name: &[u8]) -> (Vec<u8>, Vec<u8>) {
    if let Some(split) = name.windows(2).position(|bytes| bytes == b"__")
        && split != 0
    {
        let module = &name[..split];
        let function = &name[split + 2..];
        return (
            [b"<molt:".as_slice(), module, b">"].concat(),
            if function.is_empty() { name } else { function }.to_vec(),
        );
    }
    (b"<molt>".to_vec(), name.to_vec())
}

pub(crate) fn traceback_payload_from_traceback(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for (filename, lineno, name) in traceback_frames(_py, source_bits, limit) {
        let line = traceback_source_line_native(_py, &filename, lineno);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno: -1,
            end_colno: -1,
            name,
            line,
        });
    }
    out
}

pub(crate) fn traceback_payload_from_frame_chain(
    py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let back_name = intern_static_name(py, &runtime_state(py).interned.f_back_name, b"f_back");
    let mut out = Vec::new();
    let mut current = ExceptionValue::pin(py, source_bits);
    let mut seen = HashSet::new();
    let mut owners = Vec::new();
    while !obj_from_bits(current.bits()).is_none() && seen.insert(current.bits()) {
        let Some((filename, lineno, name)) = traceback_frame_info(py, current.bits()) else {
            break;
        };
        let back = traceback_attribute(py, current.bits(), back_name);
        if exception_pending(py) {
            return Vec::new();
        }
        let line = traceback_source_line_native(py, &filename, lineno);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno: -1,
            end_colno: -1,
            name,
            line,
        });
        let Some(back) = back else {
            break;
        };
        owners.push(current);
        current = back;
    }
    out.reverse();
    if let Some(max) = limit
        && out.len() > max
    {
        return out[out.len() - max..].to_vec();
    }
    out
}
pub(crate) fn traceback_payload_from_lazy_chain(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    let mut current_bits = source_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 1024 {
            break;
        }
        let Some(payload_ptr) = obj_from_bits(current_bits).as_ptr() else {
            break;
        };
        unsafe {
            if object_type_id(payload_ptr) != TYPE_ID_TRACEBACK_PAYLOAD {
                out.extend(traceback_payload_from_traceback(_py, current_bits, None));
                break;
            }
            let code_bits = traceback_payload_code_bits(payload_ptr);
            let lineno = traceback_payload_line(payload_ptr);
            let mut filename = "<unknown>".as_bytes().to_vec();
            let mut name = "<module>".as_bytes().to_vec();
            if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr()
                && object_type_id(code_ptr) == TYPE_ID_CODE
            {
                let filename_bits = code_filename_bits(code_ptr);
                if let Some(value) = string_obj_bytes(obj_from_bits(filename_bits)) {
                    filename = value;
                }
                let name_bits = code_name_bits(code_ptr);
                if let Some(value) = string_obj_bytes(obj_from_bits(name_bits))
                    && !value.is_empty()
                {
                    name = value;
                }
            }
            let line = traceback_source_line_native(_py, &filename, lineno);
            let colno = traceback_payload_col(payload_ptr);
            let end_colno = traceback_payload_end_col(payload_ptr);
            out.push(TracebackPayloadFrame {
                filename,
                lineno,
                end_lineno: lineno,
                colno,
                end_colno,
                name,
                line,
            });
            current_bits = traceback_payload_next_bits(payload_ptr);
        }
        depth += 1;
    }
    if let Some(max) = limit
        && out.len() > max
    {
        return out[out.len() - max..].to_vec();
    }
    out
}

pub(crate) fn traceback_payload_from_entry(
    _py: &PyToken<'_>,
    entry_bits: u64,
) -> Option<TracebackPayloadFrame> {
    if exception_pending(_py) {
        return None;
    }
    if obj_from_bits(entry_bits).is_none() {
        return None;
    }
    let entry_obj = obj_from_bits(entry_bits);
    if let Some(entry_ptr) = entry_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(entry_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = crate::object::seq_access::snapshot(
                    _py,
                    entry_ptr,
                    "traceback entry snapshot allocation failed",
                )?;
                if elems.is_empty() {
                    return None;
                }
                if elems.len() == 1 {
                    return traceback_payload_from_entry(_py, elems[0]);
                }
                if elems.len() >= 7 {
                    let filename = format_obj_str_bytes(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let end_lineno = to_i64(obj_from_bits(elems[2])).unwrap_or(lineno);
                    let colno = to_i64(obj_from_bits(elems[3])).unwrap_or(-1);
                    let end_colno = to_i64(obj_from_bits(elems[4])).unwrap_or(-1);
                    let name = format_obj_str_bytes(_py, obj_from_bits(elems[5]));
                    let line = if obj_from_bits(elems[6]).is_none() {
                        traceback_source_span_native(_py, &filename, lineno, end_lineno)
                    } else {
                        format_obj_str_bytes(_py, obj_from_bits(elems[6]))
                    };
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 4 {
                    let filename = format_obj_str_bytes(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str_bytes(_py, obj_from_bits(elems[2]));
                    let line = if obj_from_bits(elems[3]).is_none() {
                        Vec::new()
                    } else {
                        format_obj_str_bytes(_py, obj_from_bits(elems[3]))
                    };
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno: -1,
                        end_colno: -1,
                        name,
                        line,
                    });
                }
                if elems.len() >= 3 {
                    let filename = format_obj_str_bytes(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str_bytes(_py, obj_from_bits(elems[2]));
                    let line = traceback_source_line_native(_py, &filename, lineno);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno: -1,
                        end_colno: -1,
                        name,
                        line,
                    });
                }
                if elems.len() == 2 {
                    let first_obj = obj_from_bits(elems[0]);
                    let second_obj = obj_from_bits(elems[1]);
                    if let (Some(filename), Some(lineno)) =
                        (string_obj_bytes(first_obj), to_i64(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".as_bytes().to_vec(),
                            line: Vec::new(),
                        });
                    }
                    if let (Some(lineno), Some(filename)) =
                        (to_i64(first_obj), string_obj_bytes(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".as_bytes().to_vec(),
                            line: Vec::new(),
                        });
                    }
                    if let (Some(symbol), Some(_name)) =
                        (string_obj_bytes(first_obj), string_obj_bytes(second_obj))
                    {
                        let (filename, name) = traceback_split_molt_symbol(&symbol);
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno: 0,
                            end_lineno: 0,
                            colno: 0,
                            end_colno: 0,
                            name,
                            line: Vec::new(),
                        });
                    }
                }
                return None;
            }
            if type_id == TYPE_ID_DICT {
                let interned = &runtime_state(_py).interned;
                let filename_key = intern_static_name(_py, &interned.filename_name, b"filename");
                let lineno_key = intern_static_name(_py, &interned.lineno_name, b"lineno");
                let name_key = intern_static_name(_py, &interned.plain_name, b"name");
                let line_key = intern_static_name(_py, &interned.line_name, b"line");
                let end_lineno_key =
                    intern_static_name(_py, &interned.end_lineno_name, b"end_lineno");
                let colno_key = intern_static_name(_py, &interned.colno_name, b"colno");
                let end_colno_key = intern_static_name(_py, &interned.end_colno_name, b"end_colno");
                let filename_bits = dict_get_in_place(_py, entry_ptr, filename_key)?;
                let lineno_bits = dict_get_in_place(_py, entry_ptr, lineno_key)?;
                let lineno = to_i64(obj_from_bits(lineno_bits)).unwrap_or(0);
                let filename = format_obj_str_bytes(_py, obj_from_bits(filename_bits));
                if exception_pending(_py) {
                    return None;
                }
                let name = dict_get_in_place(_py, entry_ptr, name_key)
                    .map(|bits| format_obj_str_bytes(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| "<module>".as_bytes().to_vec());
                if exception_pending(_py) {
                    return None;
                }
                let colno = dict_get_in_place(_py, entry_ptr, colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(-1);
                let end_colno = dict_get_in_place(_py, entry_ptr, end_colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(-1);
                let end_lineno = dict_get_in_place(_py, entry_ptr, end_lineno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(lineno);
                let line = dict_get_in_place(_py, entry_ptr, line_key)
                    .filter(|bits| !obj_from_bits(*bits).is_none())
                    .map(|bits| format_obj_str_bytes(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| {
                        traceback_source_span_native(_py, &filename, lineno, end_lineno)
                    });
                return Some(TracebackPayloadFrame {
                    filename,
                    lineno,
                    end_lineno,
                    colno,
                    end_colno,
                    name,
                    line,
                });
            }
        }
    }

    if let Some(value) = string_obj_bytes(entry_obj) {
        let (filename, name) = traceback_split_molt_symbol(&value);
        return Some(TracebackPayloadFrame {
            filename,
            lineno: 0,
            end_lineno: 0,
            colno: 0,
            end_colno: 0,
            name,
            line: Vec::new(),
        });
    }

    let mut from_tb = traceback_payload_from_traceback(_py, entry_bits, Some(1));
    if let Some(frame) = from_tb.pop() {
        return Some(frame);
    }
    let mut from_frame = traceback_payload_from_frame_chain(_py, entry_bits, Some(1));
    from_frame.pop()
}

pub(crate) fn traceback_payload_from_entries(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
        return Vec::new();
    };
    let type_id = unsafe { object_type_id(source_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Vec::new();
    }
    let Some(elems) = (unsafe {
        crate::object::seq_access::snapshot(
            _py,
            source_ptr,
            "traceback source snapshot allocation failed",
        )
    }) else {
        return Vec::new();
    };
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for bits in elems.iter().copied() {
        if let Some(frame) = traceback_payload_from_entry(_py, bits) {
            out.push(frame);
            if let Some(max) = limit
                && out.len() >= max
            {
                break;
            }
        }
    }
    out
}

pub(crate) fn traceback_payload_from_source(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    if obj_from_bits(source_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TRACEBACK_PAYLOAD })
    {
        return traceback_payload_from_lazy_chain(_py, source_bits, limit);
    }
    // Real traceback and frame objects have one protocol walk, including an
    // empty result. Reprobing them through fallback shapes would repeat native
    // descriptors and other Python callbacks.
    let builtins = builtin_classes(_py);
    if unsafe {
        crate::object::class_layout::is_real_instance(_py, source_bits, builtins.traceback)
    } {
        return traceback_payload_from_traceback(_py, source_bits, limit);
    }
    if unsafe { crate::object::class_layout::is_real_instance(_py, source_bits, builtins.frame) } {
        return traceback_payload_from_frame_chain(_py, source_bits, limit);
    }
    let from_entries = traceback_payload_from_entries(_py, source_bits, limit);
    if exception_pending(_py) {
        return Vec::new();
    }
    if !from_entries.is_empty() {
        return from_entries;
    }
    let from_tb = traceback_payload_from_traceback(_py, source_bits, limit);
    if exception_pending(_py) {
        return Vec::new();
    }
    if !from_tb.is_empty() {
        return from_tb;
    }
    let from_frame = traceback_payload_from_frame_chain(_py, source_bits, limit);
    if exception_pending(_py) {
        return Vec::new();
    }
    if !from_frame.is_empty() {
        return from_frame;
    }
    if let Some(frame) = traceback_payload_from_entry(_py, source_bits) {
        return vec![frame];
    }
    Vec::new()
}

pub(crate) fn traceback_payload_to_list(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> u64 {
    let mut tuples: Vec<u64> = Vec::new();
    for frame in payload {
        let filename_ptr = alloc_string(_py, &frame.filename);
        if filename_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, &frame.name);
        if name_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let line_ptr = alloc_string(_py, &frame.line);
        if line_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let lineno_bits = MoltObject::from_int(frame.lineno).bits();
        let end_lineno_bits = MoltObject::from_int(frame.end_lineno).bits();
        let colno_bits = MoltObject::from_int(frame.colno).bits();
        let end_colno_bits = MoltObject::from_int(frame.end_colno).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let line_bits = MoltObject::from_ptr(line_ptr).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                filename_bits,
                lineno_bits,
                end_lineno_bits,
                colno_bits,
                end_colno_bits,
                name_bits,
                line_bits,
            ],
        );
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, end_lineno_bits);
        dec_ref_bits(_py, colno_bits);
        dec_ref_bits(_py, end_colno_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, line_bits);
        if tuple_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        tuples.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, tuples.as_slice());
    for bits in tuples {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn traceback_summary_caret_plan(
    lines: &[Vec<u8>],
    start: i64,
    end: i64,
    include_calls: bool,
) -> (bool, Option<(usize, usize)>) {
    if start < 0 || end < 0 {
        return (false, None);
    }
    let Some(first_line) = lines.first() else {
        return (false, None);
    };
    let Some(last_line) = lines.last() else {
        return (false, None);
    };
    let start = (start as usize).min(traceback_chars(first_line).count());
    let end = (end as usize).min(traceback_chars(last_line).count());
    let source = lines.join(b"\n".as_slice());
    let source_len = traceback_chars(&source).count();
    let suffix_len = traceback_chars(last_line).count() - end;
    let selected_end = source_len.saturating_sub(suffix_len);
    if selected_end < start {
        return (false, None);
    }
    let segment = traceback_slice(&source, start, selected_end);
    let anchor = std::str::from_utf8(segment)
        .ok()
        .and_then(|segment| traceback_caret_anchor(segment, include_calls));
    if anchor.is_some() {
        // CPython suppresses a call that is the entire RHS of a simple
        // assignment, even though the selected segment itself is a Call.
        if include_calls
            && let Ok(source_text) = std::str::from_utf8(&source)
            && let Ok(pyast::Mod::Module(module)) =
                parse_python(source_text, ParseMode::Module, "<traceback>")
            && let [statement] = module.body.as_slice()
        {
            let value = match statement {
                pyast::Stmt::Assign(node) if matches!(node.targets.as_slice(), [pyast::Expr::Name(_)])
                    && matches!(node.value.as_ref(), pyast::Expr::Call(_)) => Some(node.value.as_ref()),
                pyast::Stmt::Return(node) => node.value.as_deref().filter(|value|
                    matches!(value, pyast::Expr::Call(call) if matches!(call.func.as_ref(), pyast::Expr::Name(_)))),
                _ => None,
            };
            if let Some(value) = value {
                // CPython compares AST byte columns to the selected character
                // columns here, including its behavior for non-ASCII prefixes.
                let value_start = u32::from(value.range().start()) as usize;
                let value_end = u32::from(value.range().end()) as usize;
                let last_line_start = source
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |byte| byte + 1);
                if value_start == start && value_end == last_line_start + end {
                    return (false, anchor);
                }
            }
        }
        return (true, anchor);
    }
    (
        traceback_chars(first_line)
            .take(start)
            .any(|ch| !traceback_whitespace(ch))
            || traceback_chars(last_line)
                .skip(end)
                .any(|ch| !traceback_whitespace(ch)),
        anchor,
    )
}

fn traceback_byte_offset_to_char_offset(line: impl AsRef<[u8]>, offset: i64) -> i64 {
    if offset < 0 {
        return -1;
    }
    let bytes = line.as_ref();
    bytes[..(offset as usize).min(bytes.len())]
        .iter()
        .filter(|byte| **byte & 0xc0 != 0x80)
        .count() as i64
}

fn traceback_payload_frame_source_lines_for_target(
    frame: &TracebackPayloadFrame,
    minor: i64,
) -> Vec<Vec<u8>> {
    let full_span = minor >= 13;
    let display_width = |line: &[u8], offset| traceback_display_width(line, offset, minor);
    let raw_lines = traceback_text_lines(&frame.line);
    let first = raw_lines.first().copied().unwrap_or(b"");
    if traceback_trim(&frame.line).is_empty() {
        return Vec::new();
    }
    if !full_span {
        // 3.12 includes the original terminator in its column translation.
        let first = frame
            .line
            .split_inclusive(|byte| *byte == b'\n')
            .next()
            .unwrap_or(b"");
        let mut result = vec![[b"    ".as_slice(), traceback_trim(first), b"\n"].concat()];
        if frame.colno < 0 || frame.end_colno < 0 {
            return result;
        }
        let start = traceback_byte_offset_to_char_offset(first, frame.colno) as usize;
        let end = if frame.end_lineno > frame.lineno {
            let leading = traceback_chars(first)
                .take_while(|cp| traceback_whitespace(*cp))
                .count();
            leading + traceback_chars(traceback_trim(first)).count()
        } else {
            traceback_byte_offset_to_char_offset(first, frame.end_colno) as usize
        };
        let segment = traceback_slice(first, start, end);
        let anchor = if frame.end_lineno == frame.lineno {
            std::str::from_utf8(segment)
                .ok()
                .and_then(|segment| traceback_caret_anchor(segment, false))
        } else {
            None
        };
        if end.saturating_sub(start) < traceback_chars(traceback_trim(first)).count()
            || anchor.is_some_and(|(left, right)| right > left)
        {
            let stripped =
                traceback_chars(first).count() - traceback_chars(traceback_trim(first)).count();
            let padding = (display_width(first, start) + 1).saturating_sub(stripped);
            let extent = display_width(first, end).saturating_sub(display_width(first, start));
            let mut indicator = format!("    {}", " ".repeat(padding));
            if let Some((left, right)) = anchor {
                let left = display_width(segment, left);
                let right = display_width(segment, right);
                indicator.push_str(&"~".repeat(left));
                indicator.push_str(&"^".repeat(right.saturating_sub(left)));
                indicator.push_str(&"~".repeat(extent.saturating_sub(right)));
            } else {
                indicator.push_str(&"^".repeat(extent));
            }
            indicator.push('\n');
            result.push(indicator.into_bytes());
        }
        return result;
    }
    if frame.colno < 0 || frame.end_colno < 0 {
        return vec![[b"    ".as_slice(), traceback_trim(first), b"\n"].concat()];
    }
    let dedented = molt_stdlib_text::textwrap::textwrap_dedent_bytes(&frame.line);
    let lines: Vec<Vec<u8>> = traceback_text_lines(&dedented)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let raw_last = raw_lines.last().copied().unwrap_or(b"");
    let removed = traceback_chars(first)
        .count()
        .saturating_sub(traceback_chars(&lines[0]).count()) as i64;
    let start = (traceback_byte_offset_to_char_offset(first, frame.colno) - removed).max(0);
    let end = (traceback_byte_offset_to_char_offset(raw_last, frame.end_colno) - removed).max(0);
    let last = lines.len() - 1;
    let (show, anchor) = traceback_summary_caret_plan(&lines, start, end, true);
    let position = |offset: usize| {
        let mut remaining = offset + start as usize;
        for (index, line) in lines.iter().enumerate() {
            let count = traceback_chars(line).count();
            if remaining <= count || index == last {
                return (index, display_width(line, remaining));
            }
            remaining -= count + 1;
        }
        unreachable!("source contains at least one line")
    };
    let anchor = anchor.map(|(left, right)| (position(left), position(right)));
    let mut significant = std::collections::BTreeSet::from([0, last]);
    if let Some((left, right)) = anchor {
        for index in [left.0, right.0] {
            significant.extend(index.saturating_sub(1)..=(index + 1).min(last));
        }
    }
    let output_line = |index: usize, result: &mut Vec<u8>| {
        let line = &lines[index];
        result.extend_from_slice(line);
        result.push(b'\n');
        if !show {
            return;
        }
        let whitespace = traceback_chars(line)
            .take_while(|cp| traceback_whitespace(*cp))
            .count();
        let extent = display_width(
            line,
            if index == last {
                end as usize
            } else {
                traceback_chars(line).count()
            },
        );
        let start_display = if index == 0 {
            display_width(line, start as usize)
        } else {
            0
        };
        for col in 0..extent {
            result.push(if col < whitespace || col < start_display {
                b' '
            } else if let Some((left, right)) = anchor {
                if (index, col) >= left && (index, col) < right {
                    b'^'
                } else {
                    b'~'
                }
            } else {
                b'^'
            });
        }
        result.push(b'\n');
    };
    let mut result = Vec::new();
    let mut previous = None;
    for index in significant {
        if let Some(prev) = previous {
            if index == prev + 2 {
                output_line(index - 1, &mut result);
            } else if index > prev + 2 {
                result
                    .extend_from_slice(format!("...<{} lines>...\n", index - prev - 1).as_bytes());
            }
        }
        output_line(index, &mut result);
        previous = Some(index);
    }
    let rendered = molt_stdlib_text::textwrap::textwrap_dedent_bytes(&result);
    traceback_text_lines(&rendered)
        .into_iter()
        .map(|line| [b"    ".as_slice(), line, b"\n"].concat())
        .collect()
}

fn traceback_payload_frame_source_lines(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Vec<Vec<u8>> {
    traceback_payload_frame_source_lines_for_target(frame, runtime_target_minor(_py))
}

/// Column offsets are measured in strict UTF-8 by CPython. Filename, function
/// name and source without columns remain lossless Python text. Validate only
/// the source lines that participate in byte-offset translation for this target.
fn traceback_require_column_text(
    py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Result<(), u64> {
    if frame.colno < 0 || frame.end_colno < 0 || traceback_trim(&frame.line).is_empty() {
        return Ok(());
    }
    let validate = |bytes: &[u8]| {
        if std::str::from_utf8(bytes).is_ok() {
            return Ok(());
        }
        // Allocate only for the error path, so the canonical codec boundary
        // can retain the original Python string and exact surrogate span.
        let ptr = alloc_string(py, bytes);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        let valid = crate::object::ops_string::require_strict_utf8(py, bits);
        dec_ref_bits(py, bits);
        if valid {
            Ok(())
        } else {
            Err(MoltObject::none().bits())
        }
    };
    if runtime_target_minor(py) < 13 {
        return validate(&frame.line);
    }
    let lines = traceback_text_lines(&frame.line);
    let first = lines.first().copied().unwrap_or(b"");
    let last_index = frame.end_lineno.saturating_sub(frame.lineno).max(0) as usize;
    let Some(last) = lines.get(last_index).copied() else {
        return Err(raise_exception::<_>(
            py,
            "IndexError",
            "list index out of range",
        ));
    };
    validate(first)?;
    validate(last)
}

pub(crate) fn traceback_payload_format_frame(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Result<Vec<u8>, u64> {
    traceback_require_column_text(_py, frame)?;
    let mut entry = [
        b"  File \"".as_slice(),
        &frame.filename,
        format!("\", line {}, in ", frame.lineno).as_bytes(),
        &frame.name,
        b"\n",
    ]
    .concat();
    for line in traceback_payload_frame_source_lines(_py, frame) {
        entry.extend_from_slice(&line);
    }
    Ok(entry)
}

/// Each entry is one complete frame, as required by format_stack/format_tb.
pub(crate) fn traceback_payload_to_formatted_entries(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> Result<Vec<Vec<u8>>, u64> {
    const RECURSIVE_CUTOFF: usize = 3;
    let mut entries = Vec::with_capacity(payload.len());
    let mut index = 0;
    while index < payload.len() {
        let first = &payload[index];
        let mut run_end = index + 1;
        while run_end < payload.len() {
            let next = &payload[run_end];
            if next.filename != first.filename
                || next.lineno != first.lineno
                || next.name != first.name
            {
                break;
            }
            run_end += 1;
        }
        for frame in &payload[index..run_end.min(index + RECURSIVE_CUTOFF)] {
            entries.push(traceback_payload_format_frame(_py, frame)?);
        }
        let omitted = run_end - index - RECURSIVE_CUTOFF.min(run_end - index);
        if omitted > 0 {
            entries.push(
                format!(
                    "  [Previous line repeated {} more time{}]\n",
                    omitted,
                    if omitted == 1 { "" } else { "s" }
                )
                .into_bytes(),
            );
        }
        index = run_end;
    }
    Ok(entries)
}

fn traceback_payload_failure(py: &PyToken<'_>) -> u64 {
    if exception_pending(py) {
        MoltObject::none().bits()
    } else {
        raise_exception(py, "MemoryError", "out of memory")
    }
}

pub(crate) fn traceback_exception_components_payload(
    py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let value = ExceptionValue::pin(py, value_bits);
    let Some(storage) = ExceptionStorage::for_exception(py, value.bits()) else {
        return Err(raise_exception(
            py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    // Snapshot every physical field before source retrieval can call Python.
    let traceback = exception_field(py, value.bits(), ExceptionFieldSlot::Traceback)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let cause = exception_field(py, value.bits(), ExceptionFieldSlot::Cause)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let context = exception_field(py, value.bits(), ExceptionFieldSlot::Context)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let suppress_context = storage.suppress_context();
    let payload = traceback_payload_from_source(py, traceback.bits(), limit);
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    let frames = ExceptionValue::adopt(py, traceback_payload_to_list(py, &payload));
    if obj_from_bits(frames.bits()).is_none() || exception_pending(py) {
        return Err(traceback_payload_failure(py));
    }
    let tuple = alloc_tuple(
        py,
        &[
            frames.bits(),
            cause.bits(),
            context.bits(),
            MoltObject::from_bool(suppress_context).bits(),
        ],
    );
    if tuple.is_null() {
        Err(traceback_payload_failure(py))
    } else {
        Ok(MoltObject::from_ptr(tuple).bits())
    }
}

pub(crate) fn traceback_exception_chain_collect<'a, 'py>(
    py: &'a PyToken<'py>,
    value_bits: u64,
    limit: Option<usize>,
    nodes: &mut Vec<TracebackExceptionChainNode<'a, 'py>>,
    seen: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize, u64> {
    if let Some(index) = seen.get(&value_bits) {
        return Ok(*index);
    }
    if depth > 1024 {
        return Err(raise_exception(
            py,
            "RuntimeError",
            "traceback exception chain recursion too deep",
        ));
    }
    let value = ExceptionValue::pin(py, value_bits);
    let Some(storage) = ExceptionStorage::for_exception(py, value.bits()) else {
        return Err(raise_exception(
            py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    let traceback = exception_field(py, value.bits(), ExceptionFieldSlot::Traceback)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let cause = exception_field(py, value.bits(), ExceptionFieldSlot::Cause)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let context = exception_field(py, value.bits(), ExceptionFieldSlot::Context)
        .ok_or_else(|| traceback_payload_failure(py))?;
    let suppress_context = storage.suppress_context();
    let frames = traceback_payload_from_source(py, traceback.bits(), limit);
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    let index = nodes.len();
    seen.insert(value_bits, index);
    nodes.push(TracebackExceptionChainNode {
        value,
        frames,
        suppress_context,
        cause_index: None,
        context_index: None,
    });
    if !obj_from_bits(cause.bits()).is_none() {
        if !exception_is_instance(py, cause.bits()) {
            return Err(raise_exception(
                py,
                "TypeError",
                "exception __cause__ must be an exception instance or None",
            ));
        }
        nodes[index].cause_index = Some(traceback_exception_chain_collect(
            py,
            cause.bits(),
            limit,
            nodes,
            seen,
            depth + 1,
        )?);
    }
    if !suppress_context && !obj_from_bits(context.bits()).is_none() {
        if !exception_is_instance(py, context.bits()) {
            return Err(raise_exception(
                py,
                "TypeError",
                "exception __context__ must be an exception instance or None",
            ));
        }
        nodes[index].context_index = Some(traceback_exception_chain_collect(
            py,
            context.bits(),
            limit,
            nodes,
            seen,
            depth + 1,
        )?);
    }
    Ok(index)
}

pub(crate) fn traceback_exception_chain_payload_bits(
    py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let mut nodes = Vec::new();
    let mut seen = HashMap::new();
    traceback_exception_chain_collect(py, value_bits, limit, &mut nodes, &mut seen, 0)?;
    let mut tuples = Vec::with_capacity(nodes.len());
    for node in nodes {
        let frames = ExceptionValue::adopt(py, traceback_payload_to_list(py, &node.frames));
        if obj_from_bits(frames.bits()).is_none() || exception_pending(py) {
            return Err(traceback_payload_failure(py));
        }
        let cause = ExceptionValue::adopt(
            py,
            match node.cause_index {
                Some(index) => int_bits_from_i64(py, index as i64),
                None => MoltObject::none().bits(),
            },
        );
        let context = ExceptionValue::adopt(
            py,
            match node.context_index {
                Some(index) => int_bits_from_i64(py, index as i64),
                None => MoltObject::none().bits(),
            },
        );
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let tuple = alloc_tuple(
            py,
            &[
                node.value.bits(),
                frames.bits(),
                MoltObject::from_bool(node.suppress_context).bits(),
                cause.bits(),
                context.bits(),
            ],
        );
        if tuple.is_null() {
            return Err(traceback_payload_failure(py));
        }
        tuples.push(ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(tuple).bits(),
        ));
    }
    let tuple_bits: Vec<_> = tuples.iter().map(ExceptionValue::bits).collect();
    let list = alloc_list(py, &tuple_bits);
    if list.is_null() {
        Err(traceback_payload_failure(py))
    } else {
        Ok(MoltObject::from_ptr(list).bits())
    }
}
