use super::*;

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
    let target = lineno as usize;
    for (idx, line_result) in reader.lines().enumerate() {
        if idx + 1 == target {
            if let Ok(line) = line_result {
                return line;
            }
            return String::new();
        }
    }
    String::new()
}

pub(crate) fn traceback_line_trim_bounds(line: &str) -> Option<(i64, i64)> {
    if line.is_empty() {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end <= start {
        return None;
    }
    Some((start as i64, end as i64))
}

pub(crate) fn traceback_infer_column_offsets(line: &str) -> (i64, i64) {
    if line.is_empty() {
        return (0, 0);
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    if start >= chars.len() {
        return (0, 0);
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let trimmed: String = chars[start..end].iter().collect();
    let mut highlighted_start = start;
    if let Some(rest) = trimmed
        .strip_prefix("return ")
        .or_else(|| trimmed.strip_prefix("raise "))
        .or_else(|| trimmed.strip_prefix("yield "))
        .or_else(|| trimmed.strip_prefix("await "))
        .or_else(|| trimmed.strip_prefix("assert "))
    {
        highlighted_start = end.saturating_sub(rest.chars().count());
        while highlighted_start < end && chars[highlighted_start].is_whitespace() {
            highlighted_start += 1;
        }
    } else {
        let trimmed_chars: Vec<char> = trimmed.chars().collect();
        for idx in 0..trimmed_chars.len() {
            if trimmed_chars[idx] != '=' {
                continue;
            }
            let prev = if idx > 0 {
                Some(trimmed_chars[idx - 1])
            } else {
                None
            };
            let next = if idx + 1 < trimmed_chars.len() {
                Some(trimmed_chars[idx + 1])
            } else {
                None
            };
            if matches!(prev, Some('=' | '!' | '<' | '>' | ':')) || matches!(next, Some('=')) {
                continue;
            }
            let mut rhs_start = start + idx + 1;
            while rhs_start < end && chars[rhs_start].is_whitespace() {
                rhs_start += 1;
            }
            if rhs_start < end {
                highlighted_start = rhs_start;
            }
            break;
        }
    }
    let col = highlighted_start as i64;
    let end_col = end.max(highlighted_start) as i64;
    if end_col <= col {
        (col, col + 1)
    } else {
        (col, end_col)
    }
}

pub(crate) fn traceback_format_caret_line_native(
    line: &str,
    mut colno: i64,
    mut end_colno: i64,
) -> String {
    if line.is_empty() || colno < 0 {
        return String::new();
    }
    let text_len = line.chars().count() as i64;
    if text_len <= 0 {
        return String::new();
    }
    if end_colno < colno {
        end_colno = colno;
    }
    if colno > text_len {
        colno = text_len;
    }
    if end_colno > text_len {
        end_colno = text_len;
    }
    let Some((trim_start, trim_end)) = traceback_line_trim_bounds(line) else {
        return String::new();
    };
    if colno < trim_start {
        colno = trim_start;
    }
    if end_colno > trim_end {
        end_colno = trim_end;
    }
    if end_colno <= colno {
        return String::new();
    }
    let width = (end_colno - colno) as usize;
    let col_usize = colno as usize;
    let mut out = String::with_capacity(4 + col_usize + width + 1);
    out.push_str("    ");
    for ch in line.chars().take(col_usize) {
        if ch == '\t' {
            out.push('\t');
        } else {
            out.push(' ');
        }
    }

    // CPython 3.12 uses ^ for the "anchor" (operator, dot, paren) and ~ for
    // the rest.  Find the anchor within the highlighted region by scanning
    // for operator tokens in the source text.
    let chars: Vec<char> = line.chars().skip(col_usize).take(width).collect();
    let anchor = find_caret_anchor(&chars);
    match anchor {
        Some((a_start, a_end)) => {
            for i in 0..width {
                if i >= a_start && i < a_end {
                    out.push('^');
                } else {
                    out.push('~');
                }
            }
        }
        None => {
            for _ in 0..width {
                out.push('^');
            }
        }
    }
    out.push('\n');
    out
}

/// Find the binary-operator anchor position within a highlighted region.
/// Returns (start, end) as char offsets within `region`, or None if the whole
/// region should use `^`.  Matches CPython 3.12 which only uses `~`/`^` for
/// binary operations — attribute access, calls, subscripts all use `^`.
fn find_caret_anchor(region: &[char]) -> Option<(usize, usize)> {
    if region.len() <= 2 {
        return None; // too short for binary op pattern
    }
    // Binary operators: find a run of operator chars in the interior,
    // indicating `operand OP operand`.  Whitespace around the operator
    // is expected (e.g. `1 / 0` has spaces around `/`).
    let op_char = |c: char| {
        matches!(
            c,
            '+' | '-' | '*' | '/' | '%' | '|' | '&' | '^' | '~' | '<' | '>' | '=' | '!' | '@'
        )
    };
    let mut i = 0;
    // Skip leading non-operator chars (left operand + whitespace).
    while i < region.len() && !op_char(region[i]) {
        i += 1;
    }
    if i == 0 || i >= region.len() {
        return None; // no left operand or no operator
    }
    let op_start = i;
    // Consume the operator token (may be multi-char: //, **, <<, etc.)
    while i < region.len() && op_char(region[i]) {
        i += 1;
    }
    let op_end = i;
    // Skip whitespace after operator.
    while i < region.len() && region[i] == ' ' {
        i += 1;
    }
    // Must have a right operand remaining.
    if i >= region.len() {
        return None;
    }
    // Verify left operand has non-whitespace content before operator.
    let left_has_content = region[..op_start].iter().any(|c| !c.is_whitespace());
    if !left_has_content {
        return None;
    }
    Some((op_start, op_end))
}

#[cfg(test)]
mod traceback_format_tests {
    use super::{
        PythonVersionInfo, format_sys_version, traceback_format_caret_line_native,
        traceback_infer_column_offsets,
    };

    #[test]
    fn infer_column_offsets_prefers_rhs_for_assignment() {
        let (col, end_col) = traceback_infer_column_offsets("total = left + right   ");
        assert_eq!(col, 8);
        assert!(end_col > col);
    }

    #[test]
    fn infer_column_offsets_skips_return_keyword() {
        let (col, end_col) = traceback_infer_column_offsets("    return value");
        assert_eq!(col, 11);
        assert_eq!(end_col, 16);
    }

    #[test]
    fn caret_line_preserves_tabs_for_alignment() {
        let line = "\titem = source";
        let caret = traceback_format_caret_line_native(line, 1, 5);
        assert!(caret.starts_with("    \t"));
        assert!(caret.contains("^^^^"));
    }

    #[test]
    fn caret_line_omits_invalid_ranges() {
        let line = "value = source";
        assert!(traceback_format_caret_line_native(line, 0, 0).is_empty());
        assert!(traceback_format_caret_line_native(line, 10, 5).is_empty());
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
                let class_bits = exception_class_bits(exc_ptr);
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
                return exception_class_bits(ptr);
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
        out.extend(traceback_payload_to_formatted_lines(_py, &payload));
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
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
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
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
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
            let mut colno = traceback_payload_col(payload_ptr);
            let mut end_colno = traceback_payload_end_col(payload_ptr);
            if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                let inferred = traceback_infer_column_offsets(&line);
                colno = inferred.0;
                end_colno = inferred.1;
            }
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
                let elems = seq_vec_ref(entry_ptr);
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
                    let mut colno = to_i64(obj_from_bits(elems[3])).unwrap_or(0);
                    let mut end_colno = to_i64(obj_from_bits(elems[4])).unwrap_or(colno.max(0));
                    let name = format_obj_str(_py, obj_from_bits(elems[5]));
                    let line = if obj_from_bits(elems[6]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[6]))
                    };
                    if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                        let inferred = traceback_infer_column_offsets(&line);
                        colno = inferred.0;
                        end_colno = inferred.1;
                    }
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
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 3 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = traceback_source_line_native(_py, &filename, lineno);
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
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
                let line = dict_get_in_place(_py, entry_ptr, line_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| traceback_source_line_native(_py, &filename, lineno));
                let (mut colno, mut end_colno) = traceback_infer_column_offsets(&line);
                if let Some(value) = dict_get_in_place(_py, entry_ptr, colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    colno = value;
                }
                if let Some(value) = dict_get_in_place(_py, entry_ptr, end_colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    end_colno = value;
                }
                if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                    let inferred = traceback_infer_column_offsets(&line);
                    colno = inferred.0;
                    end_colno = inferred.1;
                }
                let end_lineno = dict_get_in_place(_py, entry_ptr, end_lineno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(lineno);
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
    let elems: Vec<u64> = unsafe { seq_vec_ref(source_ptr).to_vec() };
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for bits in elems {
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

pub(crate) fn traceback_payload_frame_source_lines(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut first_line = frame.line.clone();
    let mut first_colno = frame.colno;
    let mut first_end_colno = frame.end_colno;
    if first_line.is_empty() {
        first_line = traceback_source_line_native(_py, &frame.filename, frame.lineno);
        if first_line.is_empty() {
            return lines;
        }
        if first_colno < 0 || first_end_colno <= first_colno {
            let (col, end_col) = traceback_infer_column_offsets(&first_line);
            first_colno = col;
            first_end_colno = end_col;
        }
    }

    let span_end = frame.end_lineno.max(frame.lineno);
    if span_end <= frame.lineno || frame.lineno <= 0 || (span_end - frame.lineno) > 64 {
        lines.push(format!("    {}\n", first_line));
        let caret = traceback_format_caret_line_native(&first_line, first_colno, first_end_colno);
        if !caret.is_empty() {
            lines.push(caret);
        }
        return lines;
    }

    for lineno in frame.lineno..=span_end {
        let text = if lineno == frame.lineno {
            first_line.clone()
        } else {
            traceback_source_line_native(_py, &frame.filename, lineno)
        };
        if text.is_empty() {
            continue;
        }
        lines.push(format!("    {}\n", text));

        let text_len = text.chars().count() as i64;
        if text_len <= 0 {
            continue;
        }
        let (trim_start, trim_end) = traceback_line_trim_bounds(&text).unwrap_or((0, text_len));
        let (start, end) = if lineno == frame.lineno {
            let start = if first_colno >= 0 {
                first_colno
            } else {
                trim_start
            };
            let end = if lineno == span_end {
                if first_end_colno > start {
                    first_end_colno
                } else {
                    trim_end
                }
            } else {
                trim_end
            };
            (start, end)
        } else if lineno == span_end {
            let end = if frame.end_colno > trim_start {
                frame.end_colno
            } else {
                trim_end
            };
            (trim_start, end)
        } else {
            (trim_start, trim_end)
        };
        let caret = traceback_format_caret_line_native(&text, start, end);
        if !caret.is_empty() {
            lines.push(caret);
        }
    }

    lines
}

pub(crate) fn traceback_payload_to_formatted_lines(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for frame in payload {
        lines.push(format!(
            "  File \"{}\", line {}, in {}\n",
            frame.filename, frame.lineno, frame.name
        ));
        lines.extend(traceback_payload_frame_source_lines(_py, frame));
    }
    lines
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
