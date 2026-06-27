use super::args::{get_string_arg, raise_tcl_for_handle};
use super::state::{
    TkAppState, TkExprLiteral, TkTreeviewItem, TkTreeviewState, alloc_string_bits,
    clear_value_map_refs,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new};
use super::trace_commands::{call_tk_command_from_strings, release_result_bits};
use crate::bridge::{
    alloc_tuple_result, dec_ref_bits, decode_value_list, inc_ref_bits, string_obj_to_owned, to_i64,
};
use molt_runtime_core::prelude::{MoltObject, PyToken, obj_from_bits};
use std::collections::{HashMap, HashSet};

pub(super) fn parse_tcl_script_commands(script: &str) -> Vec<Vec<String>> {
    fn push_word(words: &mut Vec<String>, current_word: &mut String) {
        if !current_word.is_empty() {
            words.push(std::mem::take(current_word));
        }
    }

    fn push_command(
        commands: &mut Vec<Vec<String>>,
        words: &mut Vec<String>,
        current_word: &mut String,
    ) {
        push_word(words, current_word);
        if !words.is_empty() {
            commands.push(std::mem::take(words));
        }
    }

    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut current_word = String::new();

    let mut in_quote = false;
    let mut brace_depth = 0usize;
    let mut escaped = false;
    let mut command_start = true;
    let mut in_comment = false;

    for ch in script.chars() {
        if in_comment {
            if ch == '\n' || ch == '\r' {
                in_comment = false;
                push_command(&mut commands, &mut words, &mut current_word);
                command_start = true;
            }
            continue;
        }

        if escaped {
            if ch != '\n' && ch != '\r' {
                current_word.push(ch);
            }
            escaped = false;
            command_start = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            command_start = false;
            continue;
        }

        if brace_depth > 0 {
            match ch {
                '{' => {
                    brace_depth = brace_depth.saturating_add(1);
                    current_word.push('{');
                }
                '}' => {
                    brace_depth = brace_depth.saturating_sub(1);
                    if brace_depth > 0 {
                        current_word.push('}');
                    }
                }
                _ => current_word.push(ch),
            }
            command_start = false;
            continue;
        }

        if in_quote {
            if ch == '"' {
                in_quote = false;
            } else {
                current_word.push(ch);
            }
            command_start = false;
            continue;
        }

        if command_start && ch == '#' {
            in_comment = true;
            continue;
        }

        match ch {
            '{' if current_word.is_empty() => {
                brace_depth = 1;
                command_start = false;
            }
            '"' => {
                in_quote = true;
                command_start = false;
            }
            ';' | '\n' | '\r' => {
                push_command(&mut commands, &mut words, &mut current_word);
                command_start = true;
            }
            _ if ch.is_whitespace() => {
                push_word(&mut words, &mut current_word);
                command_start = words.is_empty();
            }
            _ => {
                current_word.push(ch);
                command_start = false;
            }
        }
    }

    if escaped {
        current_word.push('\\');
    }
    push_command(&mut commands, &mut words, &mut current_word);
    commands
}

pub(super) fn parse_expr_literal(expression: &str) -> Option<TkExprLiteral> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(TkExprLiteral::Int(value));
    }
    if let Ok(value) = trimmed.parse::<f64>()
        && value.is_finite()
    {
        return Some(TkExprLiteral::Float(value));
    }
    None
}

pub(super) fn alloc_tuple_bits(
    py: &PyToken,
    elems: &[u64],
    alloc_context: &str,
) -> Result<u64, u64> {
    let _ = py;
    alloc_tuple_result(elems, alloc_context)
}

pub(super) fn alloc_tuple_from_strings(
    py: &PyToken,
    values: &[String],
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut bits = Vec::with_capacity(values.len());
    for value in values {
        match alloc_string_bits(py, value) {
            Ok(value_bits) => bits.push(value_bits),
            Err(err_bits) => {
                for value_bits in bits {
                    dec_ref_bits(py, value_bits);
                }
                return Err(err_bits);
            }
        }
    }
    let tuple_bits = alloc_tuple_bits(py, bits.as_slice(), alloc_context);
    for value_bits in bits {
        dec_ref_bits(py, value_bits);
    }
    tuple_bits
}

pub(super) fn normalize_widget_option_name(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

pub(super) fn parse_widget_option_name_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    let name = get_string_arg(py, handle, bits, label)?;
    Ok(normalize_widget_option_name(&name))
}

pub(super) fn parse_widget_option_pairs(
    py: &PyToken,
    handle: i64,
    args: &[u64],
    start: usize,
    label: &str,
) -> Result<Vec<(String, u64)>, u64> {
    if start >= args.len() {
        return Ok(Vec::new());
    }
    if !(args.len() - start).is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be key/value pairs"),
        ));
    }
    let mut option_names = Vec::with_capacity((args.len() - start) / 2);
    for idx in (start..args.len()).step_by(2) {
        option_names.push(parse_widget_option_name_arg(
            py,
            handle,
            args[idx],
            "widget option name",
        )?);
    }
    let mut out = Vec::with_capacity(option_names.len());
    for (idx, option_name) in option_names.into_iter().enumerate() {
        let value_bits = args[start + idx * 2 + 1];
        if obj_from_bits(value_bits).is_none() {
            continue;
        }
        out.push((option_name, value_bits));
    }
    Ok(out)
}

pub(super) fn option_map_to_tuple(
    py: &PyToken,
    values: &HashMap<String, u64>,
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut keys: Vec<String> = values.keys().cloned().collect();
    keys.sort_unstable();
    let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
    for key in keys {
        let Some(value_bits) = values.get(&key).copied() else {
            continue;
        };
        let key_bits = alloc_string_bits(py, &key)?;
        tuple_elems.push(key_bits);
        tuple_elems.push(value_bits);
    }
    let out = alloc_tuple_bits(py, tuple_elems.as_slice(), alloc_context);
    for bits in tuple_elems {
        dec_ref_bits(py, bits);
    }
    out
}

pub(super) fn option_map_query_or_empty(
    py: &PyToken,
    values: &HashMap<String, u64>,
    option_name: &str,
) -> Result<u64, u64> {
    if let Some(value_bits) = values.get(option_name).copied() {
        inc_ref_bits(py, value_bits);
        return Ok(value_bits);
    }
    alloc_string_bits(py, "")
}

pub(super) fn set_to_sorted_tuple(
    py: &PyToken,
    values: &HashSet<String>,
    alloc_context: &str,
) -> Result<u64, u64> {
    let mut items: Vec<String> = values.iter().cloned().collect();
    items.sort_unstable();
    alloc_tuple_from_strings(py, &items, alloc_context)
}

pub(super) fn parse_bool_text(value: &str) -> Option<bool> {
    let lowered = value.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    match lowered.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            let truthy = ["true", "yes", "on"];
            let falsy = ["false", "no", "off"];
            let truthy_match = truthy
                .iter()
                .filter(|candidate| candidate.starts_with(lowered.as_str()))
                .count();
            let falsy_match = falsy
                .iter()
                .filter(|candidate| candidate.starts_with(lowered.as_str()))
                .count();
            match (truthy_match, falsy_match) {
                (1, 0) => Some(true),
                (0, 1) => Some(false),
                _ => None,
            }
        }
    }
}

pub(super) fn parse_bool_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<bool, u64> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Ok(value != 0);
    }
    if let Some(text) = string_obj_to_owned(obj)
        && let Some(value) = parse_bool_text(&text)
    {
        return Ok(value);
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        format!("{label} must be a boolean-compatible value"),
    ))
}

pub(super) fn parse_i64_arg(py: &PyToken, handle: i64, bits: u64, label: &str) -> Result<i64, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be an integer"),
        ));
    };
    Ok(value)
}

pub(super) fn alloc_int_tuple2_bits(
    py: &PyToken,
    first: i64,
    second: i64,
    alloc_context: &str,
) -> Result<u64, u64> {
    let values = vec![
        MoltObject::from_int(first).bits(),
        MoltObject::from_int(second).bits(),
    ];
    alloc_tuple_bits(py, values.as_slice(), alloc_context)
}

pub(super) fn remove_widget_from_layout_lists(app: &mut TkAppState, widget_path: &str) {
    app.pack_slaves.retain(|name| name != widget_path);
    app.grid_slaves.retain(|name| name != widget_path);
    app.place_slaves.retain(|name| name != widget_path);
}

pub(super) fn ensure_layout_membership(app: &mut TkAppState, manager: &str, widget_path: &str) {
    remove_widget_from_layout_lists(app, widget_path);
    match manager {
        "pack" => app.pack_slaves.push(widget_path.to_string()),
        "grid" => app.grid_slaves.push(widget_path.to_string()),
        "place" => app.place_slaves.push(widget_path.to_string()),
        _ => {}
    }
}

pub(super) fn tk_widget_class_name(widget_command: &str) -> String {
    match widget_command {
        "button" => "Button".to_string(),
        "canvas" => "Canvas".to_string(),
        "checkbutton" => "Checkbutton".to_string(),
        "entry" => "Entry".to_string(),
        "frame" => "Frame".to_string(),
        "label" => "Label".to_string(),
        "labelframe" => "Labelframe".to_string(),
        "listbox" => "Listbox".to_string(),
        "menu" => "Menu".to_string(),
        "menubutton" => "Menubutton".to_string(),
        "message" => "Message".to_string(),
        "panedwindow" => "Panedwindow".to_string(),
        "radiobutton" => "Radiobutton".to_string(),
        "scale" => "Scale".to_string(),
        "scrollbar" => "Scrollbar".to_string(),
        "spinbox" => "Spinbox".to_string(),
        "text" => "Text".to_string(),
        "toplevel" => "Toplevel".to_string(),
        "ttk::button" => "TButton".to_string(),
        "ttk::checkbutton" => "TCheckbutton".to_string(),
        "ttk::combobox" => "TCombobox".to_string(),
        "ttk::entry" => "TEntry".to_string(),
        "ttk::frame" => "TFrame".to_string(),
        "ttk::label" => "TLabel".to_string(),
        "ttk::labelframe" => "TLabelframe".to_string(),
        "ttk::menubutton" => "TMenubutton".to_string(),
        "ttk::notebook" => "TNotebook".to_string(),
        "ttk::panedwindow" => "TPanedwindow".to_string(),
        "ttk::progressbar" => "TProgressbar".to_string(),
        "ttk::radiobutton" => "TRadiobutton".to_string(),
        "ttk::scale" => "TScale".to_string(),
        "ttk::scrollbar" => "TScrollbar".to_string(),
        "ttk::separator" => "TSeparator".to_string(),
        "ttk::sizegrip" => "TSizegrip".to_string(),
        "ttk::spinbox" => "TSpinbox".to_string(),
        "ttk::treeview" => "Treeview".to_string(),
        _ => widget_command
            .rsplit("::")
            .next()
            .unwrap_or(widget_command)
            .to_string(),
    }
}

pub(super) fn value_bits_to_i64_default(bits: u64, default: i64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return value;
    }
    if let Some(text) = string_obj_to_owned(obj)
        && let Ok(value) = text.trim().parse::<i64>()
    {
        return value;
    }
    default
}

pub(super) fn widget_option_i64_default(
    options: &HashMap<String, u64>,
    key: &str,
    default: i64,
) -> i64 {
    options
        .get(key)
        .copied()
        .map(|bits| value_bits_to_i64_default(bits, default))
        .unwrap_or(default)
}

pub(super) fn parse_winfo_rgb_components(color: &str) -> (i64, i64, i64) {
    let trimmed = color.trim();
    if trimmed.len() == 7 && trimmed.starts_with('#') {
        let r = i64::from_str_radix(&trimmed[1..3], 16).unwrap_or(0) * 257;
        let g = i64::from_str_radix(&trimmed[3..5], 16).unwrap_or(0) * 257;
        let b = i64::from_str_radix(&trimmed[5..7], 16).unwrap_or(0) * 257;
        return (r, g, b);
    }
    if trimmed.len() == 4 && trimmed.starts_with('#') {
        let r = i64::from_str_radix(&trimmed[1..2], 16).unwrap_or(0) * 0x1111;
        let g = i64::from_str_radix(&trimmed[2..3], 16).unwrap_or(0) * 0x1111;
        let b = i64::from_str_radix(&trimmed[3..4], 16).unwrap_or(0) * 0x1111;
        return (r, g, b);
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "red" => (65535, 0, 0),
        "green" => (0, 65535, 0),
        "blue" => (0, 0, 65535),
        "white" => (65535, 65535, 65535),
        "black" => (0, 0, 0),
        _ => (0, 0, 0),
    }
}

pub(super) fn parse_treeview_index_strict(value: &str, len: usize) -> Option<usize> {
    if value.eq_ignore_ascii_case("end") {
        return Some(len);
    }
    value.trim().parse::<i64>().ok().map(|parsed| {
        if parsed <= 0 {
            0
        } else {
            (parsed as usize).min(len)
        }
    })
}

pub(super) fn parse_ttk_insert_index_strict(value: &str, len: usize) -> Option<usize> {
    parse_treeview_index_strict(value, len)
}

pub(super) fn parse_notebook_index_strict(value: &str, len: usize) -> Result<i64, String> {
    if value.eq_ignore_ascii_case("end") {
        return Ok(len as i64);
    }
    if let Ok(parsed) = value.parse::<i64>() {
        if parsed < 0 || (parsed as usize) >= len {
            return Err(format!("Slave index {parsed} out of bounds"));
        }
        return Ok(parsed);
    }
    Err(format!("invalid tab identifier \"{value}\""))
}

pub(super) fn first_missing_treeview_item<'a>(
    treeview: &TkTreeviewState,
    items: &'a [String],
) -> Option<&'a str> {
    items
        .iter()
        .find(|item| !treeview.items.contains_key(item.as_str()))
        .map(String::as_str)
}

pub(super) fn treeview_visible_items(treeview: &TkTreeviewState) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = treeview.root_children.iter().rev().cloned().collect();
    let mut visited = HashSet::new();
    while let Some(item_id) = stack.pop() {
        if !visited.insert(item_id.clone()) {
            continue;
        }
        let Some(item) = treeview.items.get(&item_id) else {
            continue;
        };
        out.push(item_id);
        for child in item.children.iter().rev() {
            stack.push(child.clone());
        }
    }
    out
}

pub(super) fn treeview_hit_item_by_y(treeview: &TkTreeviewState, y: i64) -> Option<String> {
    if y < 0 {
        return None;
    }
    let row = (y as usize) / 20;
    treeview_visible_items(treeview).get(row).cloned()
}

pub(super) fn parse_treeview_column_offset(spec: &str) -> Option<i64> {
    let normalized = spec.trim();
    let suffix = normalized.strip_prefix('#')?;
    let column = suffix.parse::<i64>().ok()?;
    (column >= 0).then_some(column * 120)
}

pub(super) const TREEVIEW_COLUMN_OPTIONS: &[&str] =
    &["-id", "-anchor", "-minwidth", "-stretch", "-width"];
pub(super) const TREEVIEW_HEADING_OPTIONS: &[&str] =
    &["-text", "-image", "-anchor", "-command", "-state"];
pub(super) const TREEVIEW_ITEM_OPTIONS: &[&str] = &["-text", "-image", "-values", "-open", "-tags"];
pub(super) const TREEVIEW_TAG_OPTIONS: &[&str] = &["-foreground", "-background", "-font", "-image"];
pub(super) const TTK_NOTEBOOK_TAB_OPTIONS: &[&str] = &[
    "-state",
    "-sticky",
    "-padding",
    "-text",
    "-image",
    "-compound",
    "-underline",
];
pub(super) const TTK_PANEDWINDOW_PANE_OPTIONS: &[&str] = &["-weight"];

pub(super) fn option_allowed(option_name: &str, allowed: &[&str]) -> bool {
    allowed.contains(&option_name)
}

pub(super) fn clamp_index_i64(value: i64, upper: usize) -> usize {
    if value <= 0 {
        0
    } else {
        (value as usize).min(upper)
    }
}

pub(super) fn text_char_count(text: &str) -> usize {
    text.chars().count()
}

pub(super) fn char_index_to_byte_index(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

pub(super) fn parse_simple_end_or_int_index(spec: &str, upper: usize) -> Option<usize> {
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(upper);
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, upper))
}

pub(super) fn parse_simple_end_or_int_index_bits(bits: u64, upper: usize) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, upper));
    }
    let spec = string_obj_to_owned(obj)?;
    parse_simple_end_or_int_index(&spec, upper)
}

pub(super) fn parse_listbox_index_bits(bits: u64, len: usize, for_insert: bool) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        let upper = if for_insert {
            len
        } else {
            len.saturating_sub(1)
        };
        return Some(clamp_index_i64(value, upper));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return if for_insert {
            Some(len)
        } else if len == 0 {
            Some(0)
        } else {
            Some(len - 1)
        };
    }
    trimmed.parse::<i64>().ok().map(|value| {
        clamp_index_i64(
            value,
            if for_insert {
                len
            } else {
                len.saturating_sub(1)
            },
        )
    })
}

pub(super) fn parse_menu_existing_index_bits(
    bits: u64,
    len: usize,
    active_index: Option<usize>,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, len - 1));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") || trimmed.eq_ignore_ascii_case("last") {
        return Some(len - 1);
    }
    if trimmed.eq_ignore_ascii_case("active") {
        return active_index.filter(|idx| *idx < len);
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, len - 1))
}

pub(super) fn parse_menu_insert_index_bits(bits: u64, len: usize) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, len));
    }
    let spec = string_obj_to_owned(obj)?;
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(len);
    }
    trimmed
        .parse::<i64>()
        .ok()
        .map(|value| clamp_index_i64(value, len))
}

pub(super) fn menu_item_type_supported(item_type: &str) -> bool {
    matches!(
        item_type,
        "cascade" | "checkbutton" | "command" | "radiobutton" | "separator"
    )
}

pub(super) fn parse_command_words(command: &str) -> Vec<String> {
    let parsed = parse_tcl_script_commands(command);
    if let Some(first) = parsed.into_iter().find(|words| !words.is_empty()) {
        return first;
    }
    vec![command.to_string()]
}

pub(super) fn run_tk_word_commands(
    py: &PyToken,
    handle: i64,
    commands: &[Vec<String>],
) -> Result<(), u64> {
    for words in commands {
        let out_bits = call_tk_command_from_strings(py, handle, words)?;
        release_result_bits(py, out_bits);
    }
    Ok(())
}

pub(super) fn parse_entry_like_index_bits(
    bits: u64,
    text_len: usize,
    insert_cursor: usize,
    selection_anchor: Option<usize>,
) -> Option<usize> {
    if let Some(index) = parse_simple_end_or_int_index_bits(bits, text_len) {
        return Some(index.min(text_len));
    }
    let spec = string_obj_to_owned(obj_from_bits(bits))?;
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "insert" => Some(insert_cursor.min(text_len)),
        "anchor" => Some(selection_anchor.unwrap_or(0).min(text_len)),
        _ => None,
    }
}

pub(super) fn parse_text_end_delta(spec: &str) -> Option<i64> {
    let compact: String = spec.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return Some(0);
    }
    let (sign, tail) = if let Some(rest) = compact.strip_prefix('+') {
        (1, rest)
    } else if let Some(rest) = compact.strip_prefix('-') {
        (-1, rest)
    } else {
        return None;
    };
    let tail = tail
        .strip_suffix('c')
        .or_else(|| tail.strip_suffix('C'))
        .unwrap_or(tail);
    if tail.is_empty() {
        return None;
    }
    let delta = tail.parse::<i64>().ok()?;
    Some(sign * delta)
}

pub(super) fn parse_text_line_column_index(spec: &str, text: &str) -> Option<usize> {
    let (line_part, column_part) = spec.split_once('.')?;
    let line = line_part.trim().parse::<i64>().ok()?;
    let column = column_part.trim().parse::<i64>().ok()?;
    let line_number = line.max(1) as usize;
    let column = column.max(0) as usize;

    let mut line_starts = vec![0usize];
    for (char_idx, ch) in text.chars().enumerate() {
        if ch == '\n' {
            line_starts.push(char_idx + 1);
        }
    }

    let total_chars = text_char_count(text);
    let Some(&line_start) = line_starts.get(line_number.saturating_sub(1)) else {
        return Some(total_chars);
    };
    let line_end = line_starts
        .get(line_number)
        .copied()
        .map(|next_start| next_start.saturating_sub(1))
        .unwrap_or(total_chars);
    let line_len = line_end.saturating_sub(line_start);
    Some((line_start + column).min(line_start + line_len))
}

pub(super) fn parse_text_index_spec(spec: &str, text: &str) -> Option<usize> {
    let trimmed = spec.trim();
    let total_chars = text_char_count(text);
    if trimmed.eq_ignore_ascii_case("end") {
        return Some(total_chars);
    }
    if let Some(rest) = trimmed.strip_prefix("end")
        && let Some(delta) = parse_text_end_delta(rest)
    {
        let index = (total_chars as i64).saturating_add(delta);
        return Some(clamp_index_i64(index, total_chars));
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(clamp_index_i64(value, total_chars));
    }
    parse_text_line_column_index(trimmed, text)
}

pub(super) fn parse_text_index_bits(bits: u64, text: &str) -> Option<usize> {
    let obj = obj_from_bits(bits);
    if let Some(value) = to_i64(obj) {
        return Some(clamp_index_i64(value, text_char_count(text)));
    }
    let spec = string_obj_to_owned(obj)?;
    parse_text_index_spec(&spec, text)
}

pub(super) fn parse_treeview_item_list_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Vec<String>, u64> {
    if let Some(raw_items) = decode_value_list(obj_from_bits(bits)) {
        let mut out = Vec::with_capacity(raw_items.len());
        for item_bits in raw_items {
            out.push(get_string_arg(py, handle, item_bits, label)?);
        }
        return Ok(out);
    }
    Ok(vec![get_string_arg(py, handle, bits, label)?])
}

pub(super) fn parse_treeview_tags(item: &TkTreeviewItem) -> Vec<String> {
    let Some(tags_bits) = item.options.get("-tags").copied() else {
        return Vec::new();
    };
    if let Some(raw) = decode_value_list(obj_from_bits(tags_bits)) {
        let mut out = Vec::with_capacity(raw.len());
        for tag_bits in raw {
            if let Some(tag) = string_obj_to_owned(obj_from_bits(tag_bits)) {
                out.push(tag);
            }
        }
        return out;
    }
    let value = obj_from_bits(tags_bits);
    if let Some(tag) = string_obj_to_owned(value) {
        if tag.trim().is_empty() {
            return Vec::new();
        }
        return tag.split_whitespace().map(str::to_string).collect();
    }
    Vec::new()
}

pub(super) fn treeview_item_is_descendant_of(
    treeview: &TkTreeviewState,
    item_id: &str,
    ancestor_id: &str,
) -> bool {
    if item_id == ancestor_id {
        return true;
    }
    let mut cursor = treeview.items.get(item_id).map(|item| item.parent.clone());
    while let Some(parent) = cursor {
        if parent.is_empty() {
            return false;
        }
        if parent == ancestor_id {
            return true;
        }
        cursor = treeview.items.get(&parent).map(|item| item.parent.clone());
    }
    false
}

pub(super) fn treeview_remove_from_parent(treeview: &mut TkTreeviewState, item_id: &str) {
    if let Some(parent_name) = treeview.items.get(item_id).map(|item| item.parent.clone()) {
        if parent_name.is_empty() {
            treeview.root_children.retain(|child| child != item_id);
            return;
        }
        if let Some(parent) = treeview.items.get_mut(&parent_name) {
            parent.children.retain(|child| child != item_id);
        }
    } else {
        treeview.root_children.retain(|child| child != item_id);
    }
}

pub(super) fn treeview_insert_into_parent(
    treeview: &mut TkTreeviewState,
    parent_id: &str,
    index: usize,
    item_id: String,
) {
    if parent_id.is_empty() {
        let idx = index.min(treeview.root_children.len());
        treeview.root_children.insert(idx, item_id);
        return;
    }
    if let Some(parent) = treeview.items.get_mut(parent_id) {
        let idx = index.min(parent.children.len());
        parent.children.insert(idx, item_id);
    }
}

pub(super) fn treeview_remove_item(py: &PyToken, treeview: &mut TkTreeviewState, item_id: &str) {
    let Some(mut item) = treeview.items.remove(item_id) else {
        return;
    };
    let children = std::mem::take(&mut item.children);
    for child in children {
        treeview_remove_item(py, treeview, &child);
    }
    clear_value_map_refs(py, &mut item.options);
    clear_value_map_refs(py, &mut item.values);
    treeview.selection.retain(|selected| selected != item_id);
    if treeview.focus.as_deref() == Some(item_id) {
        treeview.focus = None;
    }
}

pub(super) fn treeview_set_pairs_to_tuple(py: &PyToken, item: &TkTreeviewItem) -> Result<u64, u64> {
    let mut keys: Vec<String> = item.values.keys().cloned().collect();
    keys.sort_unstable();
    let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
    for column in keys {
        let Some(value_bits) = item.values.get(&column).copied() else {
            continue;
        };
        let column_bits = alloc_string_bits(py, &column)?;
        tuple_elems.push(column_bits);
        tuple_elems.push(value_bits);
    }
    let out = alloc_tuple_bits(
        py,
        tuple_elems.as_slice(),
        "failed to allocate treeview set tuple",
    );
    for bits in tuple_elems {
        dec_ref_bits(py, bits);
    }
    out
}
