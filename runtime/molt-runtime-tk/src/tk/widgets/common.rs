use super::super::*;

pub(in crate::tk) fn alloc_empty_string_bits(py: &PyToken) -> Result<u64, u64> {
    alloc_string_bits(py, "")
}

pub(in crate::tk) fn alloc_empty_tuple_bits(py: &PyToken) -> Result<u64, u64> {
    alloc_tuple_from_strings(py, &[], "failed to allocate empty tkinter tuple")
}

pub(in crate::tk) fn alloc_widget_bbox_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [
        String::from("0"),
        String::from("0"),
        String::from("0"),
        String::from("0"),
    ];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter bbox tuple")
}

pub(in crate::tk) fn alloc_widget_coord_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [String::from("0"), String::from("0")];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter coord tuple")
}

pub(in crate::tk) fn alloc_widget_view_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [String::from("0.0"), String::from("1.0")];
    alloc_tuple_from_strings(
        py,
        &values,
        "failed to allocate tkinter view fraction tuple",
    )
}

pub(in crate::tk) fn unknown_widget_subcommand_message(
    widget_path: &str,
    subcommand: &str,
) -> String {
    format!("unknown subcommand \"{subcommand}\" for widget \"{widget_path}\"")
}

pub(super) fn widget_tcl_error(
    py: &PyToken,
    last_error: &mut Option<String>,
    message: impl Into<String>,
) -> u64 {
    let message = message.into();
    *last_error = Some(message.clone());
    raise_tcl_error(py, &message)
}

pub(in crate::tk) fn evaluate_index_compare(
    left: usize,
    op: &str,
    right: usize,
) -> Result<bool, String> {
    match op {
        "<" => Ok(left < right),
        "<=" => Ok(left <= right),
        "==" => Ok(left == right),
        ">=" => Ok(left >= right),
        ">" => Ok(left > right),
        "!=" => Ok(left != right),
        _ => Err(format!(
            "bad comparison operator \"{op}\": must be <, <=, ==, >=, >, or !="
        )),
    }
}

pub(in crate::tk) fn clamp_text_widget_indices(widget: &mut TkWidgetState) {
    let max_index = text_char_count(&widget.text_value);
    widget.insert_cursor = widget.insert_cursor.min(max_index);
    for index in widget.text_marks.values_mut() {
        *index = (*index).min(max_index);
    }
}

pub(in crate::tk) fn listbox_shift_item_options_for_insert(
    widget: &mut TkWidgetState,
    insert_index: usize,
    inserted_count: usize,
) {
    if inserted_count == 0 || widget.list_item_options.is_empty() {
        return;
    }
    let mut shifted = HashMap::with_capacity(widget.list_item_options.len());
    for (index, options) in widget.list_item_options.drain() {
        let target = if index >= insert_index {
            index.saturating_add(inserted_count)
        } else {
            index
        };
        shifted.insert(target, options);
    }
    widget.list_item_options = shifted;
    if let Some(active_index) = widget.list_active_index
        && active_index >= insert_index
    {
        widget.list_active_index = Some(active_index.saturating_add(inserted_count));
    }
}

pub(in crate::tk) fn listbox_reindex_item_options_after_delete(
    py: &PyToken,
    widget: &mut TkWidgetState,
    first: usize,
    end: usize,
) {
    if first > end {
        return;
    }
    let removed_count = end - first + 1;
    if widget.list_item_options.is_empty() {
        if let Some(active_index) = widget.list_active_index {
            widget.list_active_index = if active_index < first {
                Some(active_index)
            } else if active_index > end {
                Some(active_index - removed_count)
            } else {
                None
            };
        }
        return;
    }
    let mut shifted = HashMap::with_capacity(widget.list_item_options.len());
    for (index, mut options) in widget.list_item_options.drain() {
        if index < first {
            shifted.insert(index, options);
            continue;
        }
        if index > end {
            shifted.insert(index - removed_count, options);
            continue;
        }
        clear_value_map_refs(py, &mut options);
    }
    widget.list_item_options = shifted;
    if let Some(active_index) = widget.list_active_index {
        widget.list_active_index = if active_index < first {
            Some(active_index)
        } else if active_index > end {
            Some(active_index - removed_count)
        } else {
            None
        };
    }
}

pub(in crate::tk) fn ensure_text_tag_order_entry(widget: &mut TkWidgetState, tag_name: &str) {
    if !widget
        .text_tag_order
        .iter()
        .any(|existing| existing == tag_name)
    {
        widget.text_tag_order.push(tag_name.to_string());
    }
}

pub(in crate::tk) fn normalize_text_tag_ranges(ranges: &mut Vec<(usize, usize)>) {
    ranges.retain(|(start, end)| end > start);
    ranges.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    if ranges.is_empty() {
        return;
    }
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.iter().copied() {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            if end > last.1 {
                last.1 = end;
            }
            continue;
        }
        merged.push((start, end));
    }
    *ranges = merged;
}
