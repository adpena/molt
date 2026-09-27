use super::*;
use rustpython_parser::ast::Ranged;
use rustpython_parser::{Mode as ParseMode, ast as pyast, parse as parse_python};

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

pub(crate) fn traceback_frames(
    _py: &PyToken<'_>,
    tb_bits: u64,
    limit: Option<usize>,
) -> Vec<(String, i64, String)> {
    if obj_from_bits(tb_bits).is_none() {
        return Vec::new();
    }
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<(String, i64, String)> = Vec::new();
    let mut current_bits = tb_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if let Some(max) = limit
            && out.len() >= max
        {
            break;
        }
        if depth > 512 {
            break;
        }
        let tb_obj = obj_from_bits(current_bits);
        let Some(tb_ptr) = tb_obj.as_ptr() else {
            break;
        };
        let (frame_bits, line, next_bits, had_tb_fields) = unsafe {
            let dict_bits = instance_dict_bits(tb_ptr);
            let mut frame_bits = MoltObject::none().bits();
            let mut line = 0i64;
            let mut next_bits = MoltObject::none().bits();
            let mut had_tb_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_frame_bits) {
                    frame_bits = bits;
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_lineno_bits) {
                    if let Some(val) = to_i64(obj_from_bits(bits)) {
                        line = val;
                    }
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_next_bits) {
                    next_bits = bits;
                    had_tb_fields = true;
                }
            }
            (frame_bits, line, next_bits, had_tb_fields)
        };
        if !had_tb_fields {
            break;
        }
        let (filename, func_name, frame_line) = unsafe {
            let mut filename = "<unknown>".to_string();
            let mut func_name = "<module>".to_string();
            let mut frame_line = line;
            if let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() {
                let dict_bits = instance_dict_bits(frame_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_bits)
                        && let Some(val) = to_i64(obj_from_bits(bits))
                    {
                        frame_line = val;
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_bits)
                        && let Some(code_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(code_ptr) == TYPE_ID_CODE
                    {
                        let filename_bits = code_filename_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                            filename = name;
                        }
                        let name_bits = code_name_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits))
                            && !name.is_empty()
                        {
                            func_name = name;
                        }
                    }
                }
            }
            (filename, func_name, frame_line)
        };
        let final_line = if line > 0 { line } else { frame_line };
        out.push((filename, final_line, func_name));
        current_bits = next_bits;
        depth += 1;
    }
    out
}

pub(crate) fn traceback_source_line_native(
    _py: &PyToken<'_>,
    filename: &str,
    lineno: i64,
) -> String {
    traceback_source_span_native(_py, filename, lineno, lineno)
}

fn traceback_source_span_native(
    _py: &PyToken<'_>,
    filename: &str,
    lineno: i64,
    end_lineno: i64,
) -> String {
    if lineno <= 0 {
        return String::new();
    }
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "traceback.source_line",
        "fs.read",
        AuditArgs::Path(filename.to_string()),
        allowed,
    );
    if !allowed {
        return String::new();
    }
    let Ok(file) = std::fs::File::open(filename) else {
        return String::new();
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
            return String::new();
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
    source
}

fn traceback_display_width(line: &str, offset: usize, minor: i64) -> usize {
    if line.is_ascii() {
        return offset;
    }
    line.chars()
        .take(offset)
        .map(|ch| {
            match crate::object::ops::unicode_east_asian_width_table::width(ch as u32, minor) {
                "W" | "F" => 2,
                _ => 1,
            }
        })
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
                    filename: "<first>".to_string(),
                    lineno: 3,
                    end_lineno: 3,
                    colno: 8,
                    end_colno: 14,
                    name: "first".to_string(),
                    line: "value = source".to_string(),
                },
                TracebackPayloadFrame {
                    filename: "<second>".to_string(),
                    lineno: 9,
                    end_lineno: 9,
                    colno: -1,
                    end_colno: -1,
                    name: "second".to_string(),
                    line: String::new(),
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
                let entries = traceback_payload_to_formatted_entries(py, &payload);
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0], expected, "target Python 3.{minor}");
                assert_eq!(entries[1], "  File \"<second>\", line 9, in second\n");
                payload[0].line.push('\n');
                let captured = traceback_payload_to_formatted_entries(py, &payload);
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
                filename: "<explicit>".to_string(),
                lineno: 4,
                end_lineno: 4,
                colno: -1,
                end_colno: -1,
                name: "plain".to_string(),
                line: "    x = 1".to_string(),
            };
            assert_eq!(
                traceback_payload_format_frame(py, &frame),
                "  File \"<explicit>\", line 4, in plain\n    x = 1\n"
            );
            crate::MoltObject::none().bits()
        });
    }

    #[test]
    fn public_summary_span_is_first_line_only_in_312_and_multiline_in_313() {
        let frame = TracebackPayloadFrame {
            filename: "<multiline>".to_string(),
            lineno: 1,
            end_lineno: 3,
            colno: 8,
            end_colno: 5,
            name: "demo".to_string(),
            line: "alpha = (\n    1 +\n    2".to_string(),
        };
        let lines_312 = traceback_payload_frame_source_lines_for_target(&frame, 12);
        assert_eq!(lines_312[0], "    alpha = (\n");
        assert!(!lines_312.iter().any(|line| line.contains("1 +")));
        assert!(!lines_312.iter().any(|line| line.trim() == "2"));
        assert_eq!(
            lines_312.iter().filter(|line| line.contains('^')).count(),
            1
        );

        let lines_313 = traceback_payload_frame_source_lines_for_target(&frame, 13);
        assert!(lines_313.iter().any(|line| line.trim() == "alpha = ("));
        assert!(lines_313.iter().any(|line| line.trim() == "1 +"));
        assert!(lines_313.iter().any(|line| line.trim() == "2"));
        assert!(lines_313.iter().filter(|line| line.contains('^')).count() >= 2);
    }

    #[test]
    fn public_summary_full_line_without_anchor_has_no_caret() {
        let frame = TracebackPayloadFrame {
            filename: "<tabs>".to_string(),
            lineno: 5,
            end_lineno: 5,
            colno: 1,
            end_colno: 999,
            name: "boom".to_string(),
            line: "\tassert value and (".to_string(),
        };
        for minor in [12, 13, 14] {
            assert_eq!(
                traceback_payload_frame_source_lines_for_target(&frame, minor),
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
        let one = |source: &str| [source.to_string()];
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
                filename: "<recursive>".to_string(),
                lineno: 7,
                end_lineno: 7,
                colno: -1,
                end_colno: -1,
                name: "recurse".to_string(),
                line: "recurse()".to_string(),
            };
            let entries = traceback_payload_to_formatted_entries(py, &vec![frame; 5]);
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
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
) -> String {
    let value_obj = obj_from_bits(value_bits);
    if let Some(exc_ptr) = value_obj.as_ptr() {
        unsafe {
            if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                let mut kind = "Exception".to_string();
                let class_bits = object_class_bits(exc_ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let name_bits = class_name_bits(class_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        kind = name;
                    }
                }
                let message = format_exception_message(_py, exc_ptr);
                if message.is_empty() {
                    return format!("{kind}\n");
                }
                return format!("{kind}: {message}\n");
            }
        }
    }
    let type_name = if !obj_from_bits(exc_type_bits).is_none() {
        if let Some(tp_ptr) = obj_from_bits(exc_type_bits).as_ptr() {
            unsafe {
                if object_type_id(tp_ptr) == TYPE_ID_TYPE {
                    let name_bits = class_name_bits(tp_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        name
                    } else {
                        "Exception".to_string()
                    }
                } else {
                    class_name_for_error(type_of_bits(_py, exc_type_bits))
                }
            }
        } else {
            "Exception".to_string()
        }
    } else if !value_obj.is_none() {
        class_name_for_error(type_of_bits(_py, value_bits))
    } else {
        "Exception".to_string()
    };
    if value_obj.is_none() {
        return format!("{type_name}\n");
    }
    let text = format_obj_str(_py, value_obj);
    if text.is_empty() {
        format!("{type_name}\n")
    } else {
        format!("{type_name}: {text}\n")
    }
}

pub(crate) fn traceback_exception_type_bits(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return object_class_bits(ptr);
            }
        }
    }
    if obj_from_bits(value_bits).is_none() {
        MoltObject::none().bits()
    } else {
        type_of_bits(_py, value_bits)
    }
}

pub(crate) fn traceback_exception_trace_bits(value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return exception_trace_bits(ptr);
            }
        }
    }
    MoltObject::none().bits()
}

pub(crate) fn traceback_append_exception_single_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    out: &mut Vec<String>,
) {
    if !obj_from_bits(tb_bits).is_none() {
        out.push("Traceback (most recent call last):\n".to_string());
        let payload = traceback_payload_from_source(_py, tb_bits, limit);
        out.extend(traceback_payload_to_formatted_entries(_py, &payload));
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
    seen: &mut HashSet<u64>,
    out: &mut Vec<String>,
) {
    if obj_from_bits(value_bits).is_none() || !chain {
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
    if seen.contains(&value_bits) {
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
    seen.insert(value_bits);
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                let cause_bits = exception_cause_bits(ptr);
                if !obj_from_bits(cause_bits).is_none() {
                    let cause_type_bits = traceback_exception_type_bits(_py, cause_bits);
                    let cause_tb_bits = traceback_exception_trace_bits(cause_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        cause_type_bits,
                        cause_bits,
                        cause_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "The above exception was the direct cause of the following exception:\n\n"
                            .to_string(),
                    );
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
                let context_bits = exception_context_bits(ptr);
                let suppress_context = is_truthy(_py, obj_from_bits(exception_suppress_bits(ptr)));
                if !suppress_context && !obj_from_bits(context_bits).is_none() {
                    let context_type_bits = traceback_exception_type_bits(_py, context_bits);
                    let context_tb_bits = traceback_exception_trace_bits(context_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        context_type_bits,
                        context_bits,
                        context_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "During handling of the above exception, another exception occurred:\n\n"
                            .to_string(),
                    );
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
            }
        }
    }
    traceback_append_exception_single_lines(_py, exc_type_bits, value_bits, tb_bits, limit, out);
}

pub(crate) fn traceback_lines_to_list(_py: &PyToken<'_>, lines: &[String]) -> u64 {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(lines.len());
    for line in lines {
        let ptr = alloc_string(_py, line.as_bytes());
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
    pub(crate) filename: String,
    pub(crate) lineno: i64,
    pub(crate) end_lineno: i64,
    pub(crate) colno: i64,
    pub(crate) end_colno: i64,
    pub(crate) name: String,
    pub(crate) line: String,
}

#[derive(Clone)]
pub(crate) struct TracebackExceptionChainNode {
    pub(crate) value_bits: u64,
    pub(crate) frames: Vec<TracebackPayloadFrame>,
    pub(crate) suppress_context: bool,
    pub(crate) cause_index: Option<usize>,
    pub(crate) context_index: Option<usize>,
}

pub(crate) fn traceback_split_molt_symbol(name: &str) -> (String, String) {
    if let Some((module_hint, func)) = name.split_once("__")
        && !module_hint.is_empty()
    {
        let func_name = if func.is_empty() { name } else { func };
        return (format!("<molt:{module_hint}>"), func_name.to_string());
    }
    ("<molt>".to_string(), name.to_string())
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
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    let f_back_name = intern_static_name(_py, &runtime_state(_py).interned.f_back_name, b"f_back");
    let f_code_name = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_name =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    let mut current_bits = source_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 1024 {
            break;
        }
        let Some(frame_ptr) = obj_from_bits(current_bits).as_ptr() else {
            break;
        };
        let (code_bits, lineno, back_bits, had_frame_fields) = unsafe {
            let dict_bits = instance_dict_bits(frame_ptr);
            let mut code_bits = MoltObject::none().bits();
            let mut lineno = 0i64;
            let mut back_bits = MoltObject::none().bits();
            let mut had_frame_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_name) {
                    code_bits = bits;
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_name) {
                    if let Some(value) = to_i64(obj_from_bits(bits)) {
                        lineno = value;
                    }
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_back_name) {
                    back_bits = bits;
                    had_frame_fields = true;
                }
            }
            (code_bits, lineno, back_bits, had_frame_fields)
        };
        if !had_frame_fields {
            break;
        }

        let mut filename = "<unknown>".to_string();
        let mut name = "<module>".to_string();
        if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(code_ptr) == TYPE_ID_CODE {
                    let filename_bits = code_filename_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                        filename = value;
                    }
                    let name_bits = code_name_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(name_bits))
                        && !value.is_empty()
                    {
                        name = value;
                    }
                }
            }
        }
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
        current_bits = back_bits;
        depth += 1;
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
                break;
            }
            let code_bits = traceback_payload_code_bits(payload_ptr);
            let lineno = traceback_payload_line(payload_ptr);
            let mut filename = "<unknown>".to_string();
            let mut name = "<module>".to_string();
            if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr()
                && object_type_id(code_ptr) == TYPE_ID_CODE
            {
                let filename_bits = code_filename_bits(code_ptr);
                if let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                    filename = value;
                }
                let name_bits = code_name_bits(code_ptr);
                if let Some(value) = string_obj_to_owned(obj_from_bits(name_bits))
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
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let end_lineno = to_i64(obj_from_bits(elems[2])).unwrap_or(lineno);
                    let colno = to_i64(obj_from_bits(elems[3])).unwrap_or(-1);
                    let end_colno = to_i64(obj_from_bits(elems[4])).unwrap_or(-1);
                    let name = format_obj_str(_py, obj_from_bits(elems[5]));
                    let line = if obj_from_bits(elems[6]).is_none() {
                        traceback_source_span_native(_py, &filename, lineno, end_lineno)
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[6]))
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
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = if obj_from_bits(elems[3]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[3]))
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
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
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
                        (string_obj_to_owned(first_obj), to_i64(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(lineno), Some(filename)) =
                        (to_i64(first_obj), string_obj_to_owned(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(symbol), Some(_name)) = (
                        string_obj_to_owned(first_obj),
                        string_obj_to_owned(second_obj),
                    ) {
                        let (filename, name) = traceback_split_molt_symbol(&symbol);
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno: 0,
                            end_lineno: 0,
                            colno: 0,
                            end_colno: 0,
                            name,
                            line: String::new(),
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
                let filename = format_obj_str(_py, obj_from_bits(filename_bits));
                let lineno = to_i64(obj_from_bits(lineno_bits)).unwrap_or(0);
                let name = dict_get_in_place(_py, entry_ptr, name_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| "<module>".to_string());
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
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
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

    if let Some(value) = string_obj_to_owned(entry_obj) {
        let (filename, name) = traceback_split_molt_symbol(&value);
        return Some(TracebackPayloadFrame {
            filename,
            lineno: 0,
            end_lineno: 0,
            colno: 0,
            end_colno: 0,
            name,
            line: String::new(),
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
    let from_lazy = traceback_payload_from_lazy_chain(_py, source_bits, limit);
    if !from_lazy.is_empty() {
        return from_lazy;
    }
    let from_entries = traceback_payload_from_entries(_py, source_bits, limit);
    if !from_entries.is_empty() {
        return from_entries;
    }
    let from_tb = traceback_payload_from_traceback(_py, source_bits, limit);
    if !from_tb.is_empty() {
        return from_tb;
    }
    let from_frame = traceback_payload_from_frame_chain(_py, source_bits, limit);
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
        let filename_ptr = alloc_string(_py, frame.filename.as_bytes());
        if filename_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, frame.name.as_bytes());
        if name_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let line_ptr = alloc_string(_py, frame.line.as_bytes());
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
    lines: &[String],
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
    let start = (start as usize).min(first_line.chars().count());
    let end = (end as usize).min(last_line.chars().count());
    let source = lines.join("\n");
    let source_chars: Vec<char> = source.chars().collect();
    let suffix_len = last_line.chars().count() - end;
    let selected_end = source_chars.len().saturating_sub(suffix_len);
    if selected_end < start {
        return (false, None);
    }
    let segment: String = source_chars[start..selected_end].iter().collect();
    let anchor = traceback_caret_anchor(&segment, include_calls);
    if anchor.is_some() {
        // CPython suppresses a call that is the entire RHS of a simple
        // assignment, even though the selected segment itself is a Call.
        if include_calls
            && let Ok(pyast::Mod::Module(module)) =
                parse_python(&source, ParseMode::Module, "<traceback>")
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
                let last_line_start = source.rfind('\n').map_or(0, |byte| byte + 1);
                if value_start == start && value_end == last_line_start + end {
                    return (false, anchor);
                }
            }
        }
        return (true, anchor);
    }
    (
        first_line.chars().take(start).any(|ch| !ch.is_whitespace())
            || last_line.chars().skip(end).any(|ch| !ch.is_whitespace()),
        anchor,
    )
}

fn traceback_byte_offset_to_char_offset(line: &str, offset: i64) -> i64 {
    if offset < 0 {
        return -1;
    }
    let byte = (offset as usize).min(line.len());
    String::from_utf8_lossy(&line.as_bytes()[..byte])
        .chars()
        .count() as i64
}

fn traceback_payload_frame_source_lines_for_target(
    frame: &TracebackPayloadFrame,
    minor: i64,
) -> Vec<String> {
    let full_span = minor >= 13;
    let display_width = |line: &str, offset| traceback_display_width(line, offset, minor);
    let first = frame.line.lines().next().unwrap_or("");
    if frame.line.trim().is_empty() {
        return Vec::new();
    }
    if !full_span {
        // Unlike the multiline renderer, 3.12 counts the original terminator
        // and trailing whitespace when translating source to display columns.
        let first = frame.line.split_inclusive('\n').next().unwrap_or("");
        let mut result = vec![format!("    {}\n", first.trim())];
        if frame.colno < 0 || frame.end_colno < 0 {
            return result;
        }
        let start = traceback_byte_offset_to_char_offset(first, frame.colno) as usize;
        let end = if frame.end_lineno > frame.lineno {
            first.trim_end().chars().count()
        } else {
            traceback_byte_offset_to_char_offset(first, frame.end_colno) as usize
        };
        let segment: String = first
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();
        let anchor = if frame.end_lineno == frame.lineno {
            traceback_caret_anchor(&segment, false)
        } else {
            None
        };
        if end.saturating_sub(start) < first.trim().chars().count()
            || anchor.is_some_and(|(left, right)| right > left)
        {
            let stripped = first.chars().count() - first.trim().chars().count();
            let padding = (display_width(first, start) + 1).saturating_sub(stripped);
            let extent = display_width(first, end).saturating_sub(display_width(first, start));
            let mut indicator = format!("    {}", " ".repeat(padding));
            if let Some((left, right)) = anchor {
                let left = display_width(&segment, left);
                let right = display_width(&segment, right);
                indicator.push_str(&"~".repeat(left));
                indicator.push_str(&"^".repeat(right.saturating_sub(left)));
                indicator.push_str(&"~".repeat(extent.saturating_sub(right)));
            } else {
                indicator.push_str(&"^".repeat(extent));
            }
            indicator.push('\n');
            result.push(indicator);
        }
        return result;
    }
    if frame.colno < 0 || frame.end_colno < 0 {
        return vec![format!("    {}\n", first.trim())];
    }
    // Source is captured by ingress. Formatting never consults a live frame or
    // rereads a file; dedent and anchors are computed across the whole span.
    let dedented = molt_stdlib_text::textwrap::textwrap_dedent_impl(&frame.line);
    let lines: Vec<String> = dedented.lines().map(str::to_owned).collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let raw_last = frame.line.lines().last().unwrap_or("");
    let removed = first
        .chars()
        .count()
        .saturating_sub(lines[0].chars().count()) as i64;
    let start = (traceback_byte_offset_to_char_offset(first, frame.colno) - removed).max(0);
    let end = (traceback_byte_offset_to_char_offset(raw_last, frame.end_colno) - removed).max(0);
    let last = lines.len() - 1;
    let (show, anchor) = traceback_summary_caret_plan(&lines, start, end, true);
    // Map a segment character offset back to a source line/display column.
    let position = |offset: usize| {
        let mut remaining = offset + start as usize;
        for (index, line) in lines.iter().enumerate() {
            let count = line.chars().count();
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
    let output_line = |index: usize, result: &mut String| {
        let line = &lines[index];
        result.push_str(line);
        result.push('\n');
        if !show {
            return;
        }
        let whitespace = line.chars().take_while(|ch| ch.is_whitespace()).count();
        let extent = display_width(
            line,
            if index == last {
                end as usize
            } else {
                line.chars().count()
            },
        );
        let start_display = if index == 0 {
            display_width(line, start as usize)
        } else {
            0
        };
        for col in 0..extent {
            let ch = if col < whitespace || col < start_display {
                ' '
            } else if let Some((left, right)) = anchor {
                if (index, col) >= left && (index, col) < right {
                    '^'
                } else {
                    '~'
                }
            } else {
                '^'
            };
            result.push(ch);
        }
        result.push('\n');
    };
    let mut result = String::new();
    let mut previous = None;
    for index in significant {
        if let Some(prev) = previous {
            if index == prev + 2 {
                output_line(index - 1, &mut result);
            } else if index > prev + 2 {
                result.push_str(&format!("...<{} lines>...\n", index - prev - 1));
            }
        }
        output_line(index, &mut result);
        previous = Some(index);
    }
    let rendered = molt_stdlib_text::textwrap::textwrap_dedent_impl(&result);
    rendered
        .lines()
        .map(|line| format!("    {line}\n"))
        .collect()
}

fn traceback_payload_frame_source_lines(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Vec<String> {
    traceback_payload_frame_source_lines_for_target(frame, runtime_target_minor(_py))
}

pub(crate) fn traceback_payload_format_frame(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> String {
    let mut entry = format!(
        "  File \"{}\", line {}, in {}\n",
        frame.filename, frame.lineno, frame.name
    );
    for line in traceback_payload_frame_source_lines(_py, frame) {
        entry.push_str(&line);
    }
    entry
}

/// Each entry is one complete frame, as required by format_stack/format_tb.
pub(crate) fn traceback_payload_to_formatted_entries(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> Vec<String> {
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
            entries.push(traceback_payload_format_frame(_py, frame));
        }
        let omitted = run_end - index - RECURSIVE_CUTOFF.min(run_end - index);
        if omitted > 0 {
            entries.push(format!(
                "  [Previous line repeated {} more time{}]\n",
                omitted,
                if omitted == 1 { "" } else { "s" }
            ));
        }
        index = run_end;
    }
    entries
}

pub(crate) fn traceback_exception_components_payload(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let payload = traceback_payload_from_source(_py, tb_bits, limit);
    let frames_bits = traceback_payload_to_list(_py, &payload);
    if obj_from_bits(frames_bits).is_none() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    if !obj_from_bits(cause_bits).is_none() {
        inc_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        inc_ref_bits(_py, context_bits);
    }
    let suppress_bits = MoltObject::from_bool(suppress_context).bits();
    let tuple_ptr = alloc_tuple(_py, &[frames_bits, cause_bits, context_bits, suppress_bits]);
    dec_ref_bits(_py, frames_bits);
    if !obj_from_bits(cause_bits).is_none() {
        dec_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        dec_ref_bits(_py, context_bits);
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

pub(crate) fn traceback_exception_chain_collect(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
    nodes: &mut Vec<TracebackExceptionChainNode>,
    seen: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize, u64> {
    if depth > 1024 {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "traceback exception chain recursion too deep",
        ));
    }
    if let Some(index) = seen.get(&value_bits) {
        return Ok(*index);
    }
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let frames = traceback_payload_from_source(_py, tb_bits, limit);
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    let index = nodes.len();
    seen.insert(value_bits, index);
    nodes.push(TracebackExceptionChainNode {
        value_bits,
        frames,
        suppress_context,
        cause_index: None,
        context_index: None,
    });

    if !obj_from_bits(cause_bits).is_none() {
        let Some(cause_ptr) = obj_from_bits(cause_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __cause__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __cause__ must be an exception instance or None",
                ));
            }
        }
        let cause_index =
            traceback_exception_chain_collect(_py, cause_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].cause_index = Some(cause_index);
    }

    if !suppress_context && !obj_from_bits(context_bits).is_none() {
        let Some(context_ptr) = obj_from_bits(context_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __context__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(context_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __context__ must be an exception instance or None",
                ));
            }
        }
        let context_index =
            traceback_exception_chain_collect(_py, context_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].context_index = Some(context_index);
    }

    Ok(index)
}

pub(crate) fn traceback_exception_chain_payload_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let mut nodes: Vec<TracebackExceptionChainNode> = Vec::new();
    let mut seen: HashMap<u64, usize> = HashMap::new();
    traceback_exception_chain_collect(_py, value_bits, limit, &mut nodes, &mut seen, 0)?;

    let mut tuple_bits: Vec<u64> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let frames_bits = traceback_payload_to_list(_py, &node.frames);
        if obj_from_bits(frames_bits).is_none() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        inc_ref_bits(_py, node.value_bits);
        let suppress_bits = MoltObject::from_bool(node.suppress_context).bits();
        let cause_bits = match node.cause_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let context_bits = match node.context_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                node.value_bits,
                frames_bits,
                suppress_bits,
                cause_bits,
                context_bits,
            ],
        );
        dec_ref_bits(_py, node.value_bits);
        dec_ref_bits(_py, frames_bits);
        if node.cause_index.is_some() {
            dec_ref_bits(_py, cause_bits);
        }
        if node.context_index.is_some() {
            dec_ref_bits(_py, context_bits);
        }
        if tuple_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }

    let list_ptr = alloc_list(_py, tuple_bits.as_slice());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}
