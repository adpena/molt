use super::*;

pub(super) fn alloc_empty_string_bits(py: &PyToken) -> Result<u64, u64> {
    alloc_string_bits(py, "")
}

pub(super) fn alloc_empty_tuple_bits(py: &PyToken) -> Result<u64, u64> {
    alloc_tuple_from_strings(py, &[], "failed to allocate empty tkinter tuple")
}

pub(super) fn alloc_widget_bbox_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [
        String::from("0"),
        String::from("0"),
        String::from("0"),
        String::from("0"),
    ];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter bbox tuple")
}

pub(super) fn alloc_widget_coord_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [String::from("0"), String::from("0")];
    alloc_tuple_from_strings(py, &values, "failed to allocate tkinter coord tuple")
}

pub(super) fn alloc_widget_view_bits(py: &PyToken) -> Result<u64, u64> {
    let values = [String::from("0.0"), String::from("1.0")];
    alloc_tuple_from_strings(
        py,
        &values,
        "failed to allocate tkinter view fraction tuple",
    )
}

pub(super) fn unknown_widget_subcommand_message(widget_path: &str, subcommand: &str) -> String {
    format!("unknown subcommand \"{subcommand}\" for widget \"{widget_path}\"")
}

pub(super) fn evaluate_index_compare(left: usize, op: &str, right: usize) -> Result<bool, String> {
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

pub(super) fn clamp_text_widget_indices(widget: &mut TkWidgetState) {
    let max_index = text_char_count(&widget.text_value);
    widget.insert_cursor = widget.insert_cursor.min(max_index);
    for index in widget.text_marks.values_mut() {
        *index = (*index).min(max_index);
    }
}

pub(super) fn listbox_shift_item_options_for_insert(
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

pub(super) fn listbox_reindex_item_options_after_delete(
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

pub(super) fn ensure_text_tag_order_entry(widget: &mut TkWidgetState, tag_name: &str) {
    if !widget
        .text_tag_order
        .iter()
        .any(|existing| existing == tag_name)
    {
        widget.text_tag_order.push(tag_name.to_string());
    }
}

pub(super) fn normalize_text_tag_ranges(ranges: &mut Vec<(usize, usize)>) {
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

pub(super) fn handle_generic_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };

    match subcommand {
        "create" => {
            widget.next_item_id += 1;
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(widget.next_item_id).bits()));
        }
        "add" => {
            if widget.widget_command == "menu" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu add expects item type and optional key/value pairs",
                    ));
                }
                let item_type =
                    get_string_arg(py, handle, args[2], "menu item type")?.to_ascii_lowercase();
                if !menu_item_type_supported(&item_type) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!(
                            "bad menu entry type \"{item_type}\": must be cascade, checkbutton, command, radiobutton, or separator"
                        ),
                    ));
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 3, "menu add options")?;
                let mut entry = TkMenuEntryState {
                    item_type,
                    ..TkMenuEntryState::default()
                };
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut entry.options, option_name, value_bits);
                }
                widget.menu_entries.push(entry);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "panedwindow" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow add expects child path and optional key/value pairs",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                if !widget
                    .pane_children
                    .iter()
                    .any(|existing| existing == &child)
                {
                    widget.pane_children.push(child.clone());
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
                let pane_options = widget.pane_child_options.entry(child).or_default();
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, pane_options, option_name, value_bits);
                }
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "insert" => {
            if widget.widget_command == "listbox" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox insert expects index and one or more elements",
                    ));
                }
                let Some(mut insert_index) =
                    parse_listbox_index_bits(args[2], widget.list_items.len(), true)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox insert index must be an integer or end",
                    ));
                };
                let original_insert_index = insert_index;
                let inserted_count = args.len().saturating_sub(3);
                for value_bits in &args[3..] {
                    inc_ref_bits(py, *value_bits);
                    widget.list_items.insert(insert_index, *value_bits);
                    insert_index += 1;
                }
                if inserted_count > 0 && !widget.list_selection.is_empty() {
                    let mut shifted = HashSet::with_capacity(widget.list_selection.len());
                    for index in widget.list_selection.drain() {
                        if index >= original_insert_index {
                            shifted.insert(index + inserted_count);
                        } else {
                            shifted.insert(index);
                        }
                    }
                    widget.list_selection = shifted;
                }
                listbox_shift_item_options_for_insert(
                    widget,
                    original_insert_index,
                    inserted_count,
                );
            } else if widget.widget_command == "menu" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu insert expects index, item type, and optional key/value pairs",
                    ));
                }
                let Some(index) = parse_menu_insert_index_bits(args[2], widget.menu_entries.len())
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu insert index must be an integer or end",
                    ));
                };
                let item_type =
                    get_string_arg(py, handle, args[3], "menu item type")?.to_ascii_lowercase();
                if !menu_item_type_supported(&item_type) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!(
                            "bad menu entry type \"{item_type}\": must be cascade, checkbutton, command, radiobutton, or separator"
                        ),
                    ));
                }
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 4, "menu insert options")?;
                let mut entry = TkMenuEntryState {
                    item_type,
                    ..TkMenuEntryState::default()
                };
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut entry.options, option_name, value_bits);
                }
                widget.menu_entries.insert(index, entry);
                if let Some(active_index) = widget.menu_active_index
                    && active_index >= index
                {
                    widget.menu_active_index = Some(active_index + 1);
                }
            } else if widget.widget_command == "panedwindow" {
                if args.len() < 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow insert expects index, child path, and optional key/value pairs",
                    ));
                }
                let Some(index) =
                    parse_simple_end_or_int_index_bits(args[2], widget.pane_children.len())
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow insert index must be an integer or end",
                    ));
                };
                let child = get_string_arg(py, handle, args[3], "panedwindow child path")?;
                widget.pane_children.retain(|existing| existing != &child);
                let insert_index = index.min(widget.pane_children.len());
                widget.pane_children.insert(insert_index, child.clone());
                let option_pairs =
                    parse_widget_option_pairs(py, handle, args, 4, "panedwindow pane options")?;
                let pane_options = widget.pane_child_options.entry(child).or_default();
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, pane_options, option_name, value_bits);
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox")
                && args.len() > 3
            {
                let insert_index = if widget.widget_command == "text" {
                    let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text insert index must be an integer, end, or line.column",
                        ));
                    };
                    index
                } else {
                    let Some(index) = parse_entry_like_index_bits(
                        args[2],
                        text_char_count(&widget.text_value),
                        widget.insert_cursor,
                        widget.selection_anchor,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "entry/spinbox insert index must be an integer, end, insert, or anchor",
                        ));
                    };
                    index
                };
                let value = get_text_arg(py, handle, args[3], "widget insert value")?;
                let byte_index = char_index_to_byte_index(&widget.text_value, insert_index);
                widget.text_value.insert_str(byte_index, &value);
                widget.insert_cursor = insert_index.saturating_add(text_char_count(&value));
                clamp_text_widget_indices(widget);
                if widget.widget_command == "text" {
                    widget.text_edit_modified = true;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "delete" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox delete expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox delete first index must be integer or end",
                    ));
                };
                let last = if args.len() == 4 {
                    let Some(last) =
                        parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                    else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "listbox delete last index must be integer or end",
                        ));
                    };
                    last
                } else {
                    first
                };
                if !widget.list_items.is_empty() && first < widget.list_items.len() {
                    let end = last.min(widget.list_items.len() - 1);
                    if end >= first {
                        let removed_count = end - first + 1;
                        for bits in widget.list_items.drain(first..=end) {
                            dec_ref_bits(py, bits);
                        }
                        if !widget.list_selection.is_empty() {
                            let mut shifted = HashSet::with_capacity(widget.list_selection.len());
                            for index in widget.list_selection.drain() {
                                if index < first {
                                    shifted.insert(index);
                                } else if index > end {
                                    shifted.insert(index - removed_count);
                                }
                            }
                            widget.list_selection = shifted;
                        }
                        listbox_reindex_item_options_after_delete(py, widget, first, end);
                    }
                }
            } else if widget.widget_command == "menu" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu delete expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu delete first index must resolve to an existing entry",
                    ));
                };
                let last = if args.len() == 4 {
                    let Some(last) = parse_menu_existing_index_bits(
                        args[3],
                        widget.menu_entries.len(),
                        widget.menu_active_index,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "menu delete last index must resolve to an existing entry",
                        ));
                    };
                    last
                } else {
                    first
                };
                let end = last.max(first);
                if end < widget.menu_entries.len() {
                    let removed_count = end - first + 1;
                    for mut entry in widget.menu_entries.drain(first..=end) {
                        clear_value_map_refs(py, &mut entry.options);
                    }
                    if let Some(active_index) = widget.menu_active_index {
                        widget.menu_active_index = if active_index < first {
                            Some(active_index)
                        } else if active_index > end {
                            Some(active_index - removed_count)
                        } else {
                            None
                        };
                    }
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "delete expects first index and optional last index",
                    ));
                }
                let start = if widget.widget_command == "text" {
                    let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text delete first index must be an integer, end, or line.column",
                        ));
                    };
                    start
                } else {
                    let Some(start) = parse_entry_like_index_bits(
                        args[2],
                        text_char_count(&widget.text_value),
                        widget.insert_cursor,
                        widget.selection_anchor,
                    ) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "entry/spinbox delete first index must be an integer, end, insert, or anchor",
                        ));
                    };
                    start
                };
                let end = if args.len() == 4 {
                    if widget.widget_command == "text" {
                        let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text delete last index must be an integer, end, or line.column",
                            ));
                        };
                        end
                    } else {
                        let Some(end) = parse_entry_like_index_bits(
                            args[3],
                            text_char_count(&widget.text_value),
                            widget.insert_cursor,
                            widget.selection_anchor,
                        ) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "entry/spinbox delete last index must be an integer, end, insert, or anchor",
                            ));
                        };
                        end
                    }
                } else {
                    (start + 1).min(text_char_count(&widget.text_value))
                };
                if end > start {
                    let start_byte = char_index_to_byte_index(&widget.text_value, start);
                    let end_byte = char_index_to_byte_index(&widget.text_value, end);
                    widget.text_value.replace_range(start_byte..end_byte, "");
                }
                widget.insert_cursor = start;
                widget.selection_range = None;
                clamp_text_widget_indices(widget);
                if widget.widget_command == "text" {
                    widget.text_edit_modified = true;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "get" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 && args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox get expects first index and optional last index",
                    ));
                }
                let Some(first) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox get first index must be integer or end",
                    ));
                };
                if args.len() == 4 {
                    let Some(last) =
                        parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                    else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "listbox get last index must be integer or end",
                        ));
                    };
                    if widget.list_items.is_empty() || first >= widget.list_items.len() {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    let end = last.min(widget.list_items.len() - 1);
                    if end < first {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    let range = widget.list_items[first..=end].to_vec();
                    app.last_error = None;
                    return alloc_tuple_bits(
                        py,
                        range.as_slice(),
                        "failed to allocate listbox get range tuple",
                    )
                    .map(Some);
                }
                if let Some(bits) = widget.list_items.get(first).copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
            } else if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                if widget.widget_command == "text" && (args.len() == 3 || args.len() == 4) {
                    let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text get start index must be an integer, end, or line.column",
                        ));
                    };
                    let end = if args.len() == 4 {
                        let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text get end index must be an integer, end, or line.column",
                            ));
                        };
                        end
                    } else {
                        text_char_count(&widget.text_value)
                    };
                    if end <= start {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    let start_byte = char_index_to_byte_index(&widget.text_value, start);
                    let end_byte = char_index_to_byte_index(&widget.text_value, end);
                    let slice = widget.text_value[start_byte..end_byte].to_string();
                    app.last_error = None;
                    return alloc_string_bits(py, &slice).map(Some);
                }
                let text = widget.text_value.clone();
                app.last_error = None;
                return alloc_string_bits(py, &text).map(Some);
            }
            app.last_error = None;
            return alloc_empty_string_bits(py).map(Some);
        }
        "size" | "count" => {
            let value = if widget.widget_command == "listbox" {
                widget.list_items.len() as i64
            } else {
                0
            };
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(value).bits()));
        }
        "forget" => {
            if widget.widget_command == "panedwindow" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow forget expects exactly one child path",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                widget.pane_children.retain(|existing| existing != &child);
                if let Some(mut options) = widget.pane_child_options.remove(&child) {
                    clear_value_map_refs(py, &mut options);
                }
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "replace" => {
            if widget.widget_command == "text" {
                if args.len() < 5 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace expects index1, index2, and replacement text",
                    ));
                }
                let Some(start) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace start index must be an integer, end, or line.column",
                    ));
                };
                let Some(end) = parse_text_index_bits(args[3], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text replace end index must be an integer, end, or line.column",
                    ));
                };
                let replacement = get_text_arg(py, handle, args[4], "text replace chars")?;
                let replace_start = start.min(end);
                let replace_end = start.max(end);
                let start_byte = char_index_to_byte_index(&widget.text_value, replace_start);
                let end_byte = char_index_to_byte_index(&widget.text_value, replace_end);
                widget
                    .text_value
                    .replace_range(start_byte..end_byte, replacement.as_str());
                widget.insert_cursor = replace_start.saturating_add(text_char_count(&replacement));
                widget.selection_range = None;
                widget.text_edit_modified = true;
                clamp_text_widget_indices(widget);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
        }
        "edit" => {
            if widget.widget_command == "text" {
                if args.len() < 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text edit expects a subcommand",
                    ));
                }
                let op = get_string_arg(py, handle, args[2], "text edit subcommand")?;
                match op.as_str() {
                    "modified" => {
                        if args.len() == 3 {
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(widget.text_edit_modified).bits(),
                            ));
                        }
                        if args.len() != 4 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text edit modified expects optional boolean argument",
                            ));
                        }
                        let value = parse_bool_arg(py, handle, args[3], "text edit modified flag")?;
                        widget.text_edit_modified = value;
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "reset" => {
                        if args.len() != 3 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "text edit reset expects no additional arguments",
                            ));
                        }
                        widget.text_edit_modified = false;
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "separator" | "undo" | "redo" => {
                        if args.len() != 3 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                format!("text edit {op} expects no additional arguments"),
                            ));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("edit {op}")),
                        ));
                    }
                }
            }
        }
        "dump" => {
            if widget.widget_command == "text" {
                let mut idx = 2usize;
                let mut callback_words: Option<Vec<String>> = None;
                let mut include_text = true;
                while idx < args.len() {
                    let token = get_string_arg(py, handle, args[idx], "text dump option")?;
                    if !token.starts_with('-') {
                        break;
                    }
                    match token.as_str() {
                        "-all" | "-text" | "-mark" | "-tag" | "-image" | "-window" => {}
                        "-command" => {
                            idx += 1;
                            if idx >= args.len() {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "text dump -command expects a command name",
                                ));
                            }
                            let command_name =
                                get_string_arg(py, handle, args[idx], "text dump command name")?;
                            callback_words = Some(parse_command_words(&command_name));
                        }
                        "-elide" => {
                            include_text = true;
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                format!("bad text dump option \"{token}\""),
                            ));
                        }
                    }
                    idx += 1;
                }
                if idx >= args.len() {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump expects index1 and optional index2",
                    ));
                }
                let Some(start) = parse_text_index_bits(args[idx], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump start index must be an integer, end, or line.column",
                    ));
                };
                idx += 1;
                let end = if idx < args.len() {
                    let Some(end) = parse_text_index_bits(args[idx], &widget.text_value) else {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "text dump end index must be an integer, end, or line.column",
                        ));
                    };
                    idx += 1;
                    end
                } else {
                    text_char_count(&widget.text_value)
                };
                if idx != args.len() {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text dump received unexpected extra arguments",
                    ));
                }
                let dump_start = start.min(end);
                let dump_end = start.max(end);
                let start_byte = char_index_to_byte_index(&widget.text_value, dump_start);
                let end_byte = char_index_to_byte_index(&widget.text_value, dump_end);
                let slice = widget.text_value[start_byte..end_byte].to_string();
                let mut flat_words: Vec<String> = Vec::new();
                if include_text && !slice.is_empty() {
                    flat_words.push("text".to_string());
                    flat_words.push(slice.clone());
                    flat_words.push(format!("1.{dump_start}"));
                }
                let callback_invocations: Vec<Vec<String>> =
                    if let Some(command_words) = callback_words {
                        let mut calls = Vec::new();
                        for triple in flat_words.chunks_exact(3) {
                            let mut words = command_words.clone();
                            words.push(triple[0].clone());
                            words.push(triple[1].clone());
                            words.push(triple[2].clone());
                            calls.push(words);
                        }
                        calls
                    } else {
                        Vec::new()
                    };
                app.last_error = None;
                if !callback_invocations.is_empty() {
                    drop(registry);
                    run_tk_word_commands(py, handle, &callback_invocations)?;
                    return Ok(Some(MoltObject::none().bits()));
                }
                return alloc_tuple_from_strings(
                    py,
                    flat_words.as_slice(),
                    "failed to allocate text dump tuple",
                )
                .map(Some);
            }
        }
        "subwidget" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "subwidget expects exactly one child name",
                ));
            }
            let child_name = get_string_arg(py, handle, args[2], "subwidget child name")?;
            let child_path = format!("{widget_path}.{child_name}");
            app.last_error = None;
            return alloc_string_bits(py, &child_path).map(Some);
        }
        _ => {}
    }

    match subcommand {
        "bbox" | "coords" => {
            app.last_error = None;
            alloc_widget_bbox_bits(py).map(Some)
        }
        "index" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects exactly one index argument",
                ));
            }
            if widget.widget_command == "listbox" {
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox index must be an integer or end",
                    ));
                };
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            if matches!(widget.widget_command.as_str(), "entry" | "spinbox") {
                let Some(index) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "entry/spinbox index must be an integer, end, insert, or anchor",
                    ));
                };
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            if widget.widget_command == "text" {
                let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text index must be an integer, end, or line.column",
                    ));
                };
                app.last_error = None;
                return alloc_string_bits(py, &format!("1.{index}")).map(Some);
            }
            if widget.widget_command == "menu" {
                let maybe_index = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
                app.last_error = None;
                if let Some(index) = maybe_index {
                    return Ok(Some(MoltObject::from_int(index as i64).bits()));
                }
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "panedwindow" {
                let token = get_string_arg(py, handle, args[2], "panedwindow index")?;
                if let Some(position) = widget.pane_children.iter().position(|item| item == &token)
                {
                    app.last_error = None;
                    return Ok(Some(MoltObject::from_int(position as i64).bits()));
                }
                if let Some(index) =
                    parse_simple_end_or_int_index(token.as_str(), widget.pane_children.len())
                {
                    app.last_error = None;
                    return Ok(Some(MoltObject::from_int(index as i64).bits()));
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad panedwindow index \"{token}\""),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::from_int(0).bits()))
        }
        "nearest" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "nearest expects exactly one coordinate argument",
                ));
            }
            if widget.widget_command == "listbox" {
                let y = parse_i64_arg(py, handle, args[2], "listbox nearest coordinate")?;
                let index = clamp_index_i64(y, widget.list_items.len().saturating_sub(1));
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::from_int(0).bits()))
        }
        "compare" => {
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "compare expects index1, operator, and index2",
                ));
            }
            let op = get_string_arg(py, handle, args[3], "compare operator")?;
            let (left, right) = if widget.widget_command == "text" {
                let Some(left) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text compare index1 must be an integer, end, or line.column",
                    ));
                };
                let Some(right) = parse_text_index_bits(args[4], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text compare index2 must be an integer, end, or line.column",
                    ));
                };
                (left, right)
            } else if widget.widget_command == "listbox" {
                let Some(left) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox compare index1 must be an integer or end",
                    ));
                };
                let Some(right) = parse_listbox_index_bits(args[4], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox compare index2 must be an integer or end",
                    ));
                };
                (left, right)
            } else {
                let Some(left) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "compare index1 must be an integer, end, insert, or anchor",
                    ));
                };
                let Some(right) = parse_entry_like_index_bits(
                    args[4],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "compare index2 must be an integer, end, insert, or anchor",
                    ));
                };
                (left, right)
            };
            let result = evaluate_index_compare(left, &op, right)
                .map_err(|message| app_tcl_error_locked(py, app, message))?;
            app.last_error = None;
            Ok(Some(MoltObject::from_bool(result).bits()))
        }
        "curselection" => {
            if widget.widget_command == "listbox" {
                let mut indices: Vec<String> = widget
                    .list_selection
                    .iter()
                    .copied()
                    .filter(|idx| *idx < widget.list_items.len())
                    .map(|idx| idx.to_string())
                    .collect();
                indices.sort_unstable_by_key(|value| value.parse::<usize>().unwrap_or(0));
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    indices.as_slice(),
                    "failed to allocate listbox curselection tuple",
                )
                .map(Some);
            }
            app.last_error = None;
            alloc_empty_tuple_bits(py).map(Some)
        }
        "find" | "tabs" | "panes" => {
            if subcommand == "panes" && widget.widget_command == "panedwindow" {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    widget.pane_children.as_slice(),
                    "failed to allocate panedwindow panes tuple",
                )
                .map(Some);
            }
            app.last_error = None;
            alloc_empty_tuple_bits(py).map(Some)
        }
        "subwidgets" => {
            let mut names = Vec::new();
            let prefix = format!("{widget_path}.");
            for path in app.widgets.keys() {
                if let Some(name) = path.strip_prefix(&prefix) {
                    names.push(name.to_string());
                }
            }
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate subwidgets tuple")
                .map(Some)
        }
        "identify" => {
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "type" => {
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu type expects exactly one index argument",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    app.last_error = None;
                    return alloc_empty_string_bits(py).map(Some);
                };
                if let Some(entry) = widget.menu_entries.get(index) {
                    app.last_error = None;
                    return alloc_string_bits(py, &entry.item_type).map(Some);
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "itemcget" => {
            if widget.widget_command == "listbox" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox itemcget expects index and option",
                    ));
                }
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox itemcget index must be an integer or end",
                    ));
                };
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "listbox item option name")?;
                if let Some(bits) = widget
                    .list_item_options
                    .get(&index)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "entrycget" => {
            if widget.widget_command == "menu" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu entrycget expects index and option",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu entrycget index must resolve to an existing entry",
                    ));
                };
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "menu entry option name")?;
                if let Some(bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "panecget" => {
            if widget.widget_command == "panedwindow" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "panedwindow panecget expects child and option",
                    ));
                }
                let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
                if let Some(bits) = widget
                    .pane_child_options
                    .get(&child)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "image_cget" | "window_cget" => {
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "bind" => {
            if args.len() < 3 || args.len() > 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "bind expects target, optional sequence, optional script",
                ));
            }
            let target_name = get_string_arg(py, handle, args[2], "bind target")?;
            let bindings = widget.tag_bindings.entry(target_name).or_default();
            if args.len() == 3 {
                let mut sequences: Vec<String> = bindings.keys().cloned().collect();
                sequences.sort_unstable();
                app.last_error = None;
                return alloc_string_bits(py, &sequences.join(" ")).map(Some);
            }
            let sequence = get_string_arg(py, handle, args[3], "bind sequence")?;
            if args.len() == 4 {
                let script = bindings.get(&sequence).cloned().unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &script).map(Some);
            }
            let mut script = get_string_arg(py, handle, args[4], "bind script")?;
            if args.len() == 6 {
                let command_name = get_string_arg(py, handle, args[5], "bind callback id")?;
                if script.is_empty() {
                    script = bindings.get(&sequence).cloned().unwrap_or_default();
                }
                script = remove_bind_script_command_invocations(&script, &command_name);
            } else if script.starts_with('+') {
                script = if let Some(previous) = bindings.get(&sequence) {
                    if previous.trim().is_empty() {
                        script
                    } else {
                        format!("{previous}\n{script}")
                    }
                } else {
                    script
                };
            }
            if script.is_empty() {
                bindings.remove(&sequence);
            } else {
                bindings.insert(sequence, script);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "xview" | "yview" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_widget_view_bits(py).map(Some);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "xposition" | "yposition" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} expects exactly one index argument"),
                ));
            }
            let Some(index) = parse_menu_existing_index_bits(
                args[2],
                widget.menu_entries.len(),
                widget.menu_active_index,
            ) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} index must resolve to an existing menu entry"),
                ));
            };
            let value = if subcommand == "xposition" {
                (index as i64) * 20
            } else {
                (index as i64) * 18
            };
            app.last_error = None;
            Ok(Some(MoltObject::from_int(value).bits()))
        }
        "selection" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "selection subcommand")?;
                if widget.widget_command == "listbox" {
                    match op.as_str() {
                        "anchor" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection anchor expects one index argument",
                                ));
                            }
                            let Some(index) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection anchor index must be an integer or end",
                                ));
                            };
                            widget.selection_anchor = Some(index);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "set" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection set expects first and optional last index",
                                ));
                            }
                            let Some(first) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection set first index must be an integer or end",
                                ));
                            };
                            let last = if args.len() == 5 {
                                let Some(last) = parse_listbox_index_bits(
                                    args[4],
                                    widget.list_items.len(),
                                    false,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection set last index must be an integer or end",
                                    ));
                                };
                                last
                            } else {
                                first
                            };
                            if !widget.list_items.is_empty() {
                                let end = last.min(widget.list_items.len() - 1);
                                if end >= first {
                                    for idx in first..=end {
                                        widget.list_selection.insert(idx);
                                    }
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "clear" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection clear expects first and optional last index",
                                ));
                            }
                            let Some(first) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection clear first index must be an integer or end",
                                ));
                            };
                            let last = if args.len() == 5 {
                                let Some(last) = parse_listbox_index_bits(
                                    args[4],
                                    widget.list_items.len(),
                                    false,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection clear last index must be an integer or end",
                                    ));
                                };
                                last
                            } else {
                                first
                            };
                            let end = last.max(first);
                            widget
                                .list_selection
                                .retain(|index| *index < first || *index > end);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "includes" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes expects one index argument",
                                ));
                            }
                            let Some(index) =
                                parse_listbox_index_bits(args[3], widget.list_items.len(), false)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes index must be an integer or end",
                                ));
                            };
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(widget.list_selection.contains(&index))
                                    .bits(),
                            ));
                        }
                        "present" => {
                            app.last_error = None;
                            return Ok(Some(
                                MoltObject::from_bool(!widget.list_selection.is_empty()).bits(),
                            ));
                        }
                        "get" => {
                            let mut selected: Vec<usize> =
                                widget.list_selection.iter().copied().collect();
                            selected.sort_unstable();
                            if let Some(index) = selected
                                .into_iter()
                                .find(|idx| *idx < widget.list_items.len())
                                && let Some(bits) = widget.list_items.get(index).copied()
                            {
                                inc_ref_bits(py, bits);
                                app.last_error = None;
                                return Ok(Some(bits));
                            }
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("selection {op}"),
                                ),
                            ));
                        }
                    }
                }
                if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
                    match op.as_str() {
                        "clear" => {
                            widget.selection_range = None;
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "present" => {
                            let present = widget
                                .selection_range
                                .is_some_and(|(start, end)| end > start);
                            app.last_error = None;
                            return Ok(Some(MoltObject::from_bool(present).bits()));
                        }
                        "get" => {
                            if let Some((start, end)) = widget.selection_range
                                && end > start
                            {
                                let start_byte =
                                    char_index_to_byte_index(&widget.text_value, start);
                                let end_byte = char_index_to_byte_index(&widget.text_value, end);
                                let slice = widget.text_value[start_byte..end_byte].to_string();
                                app.last_error = None;
                                return alloc_string_bits(py, &slice).map(Some);
                            }
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        "from" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection from expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection from index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection from index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            widget.selection_anchor = Some(index);
                            widget.selection_range = Some((index, index));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "to" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection to expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection to index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection to index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let anchor = widget.selection_anchor.unwrap_or(0);
                            let (start, end) = if index >= anchor {
                                (anchor, index)
                            } else {
                                (index, anchor)
                            };
                            widget.selection_range = Some((start, end));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "range" => {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection range expects start and end indices",
                                ));
                            }
                            let start = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range start index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range start index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let end = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[4], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range end index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[4],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection range end index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            widget.selection_range = Some((start.min(end), start.max(end)));
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "includes" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection includes expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection includes index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection includes index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            let includes = widget
                                .selection_range
                                .is_some_and(|(start, end)| index >= start && index < end);
                            app.last_error = None;
                            return Ok(Some(MoltObject::from_bool(includes).bits()));
                        }
                        "adjust" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "selection adjust expects one index argument",
                                ));
                            }
                            let index = if widget.widget_command == "text" {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection adjust index must be an integer, end, or line.column index",
                                    ));
                                };
                                index
                            } else {
                                let Some(index) = parse_entry_like_index_bits(
                                    args[3],
                                    text_char_count(&widget.text_value),
                                    widget.insert_cursor,
                                    widget.selection_anchor,
                                ) else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "selection adjust index must be an integer, end, insert, or anchor index",
                                    ));
                                };
                                index
                            };
                            if let Some((start, end)) = widget.selection_range {
                                let dist_start = start.abs_diff(index);
                                let dist_end = end.abs_diff(index);
                                widget.selection_range = if dist_start <= dist_end {
                                    Some((index.min(end), index.max(end)))
                                } else {
                                    Some((start.min(index), start.max(index)))
                                };
                            } else {
                                widget.selection_anchor = Some(index);
                                widget.selection_range = Some((index, index));
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "element" => {
                            app.last_error = None;
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("selection {op}"),
                                ),
                            ));
                        }
                    }
                }
                match op.as_str() {
                    "includes" | "present" => {
                        app.last_error = None;
                        return Ok(Some(MoltObject::from_bool(false).bits()));
                    }
                    "get" => {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(
                                widget_path,
                                &format!("selection {op}"),
                            ),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "mark" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "mark subcommand")?;
                if widget.widget_command == "text" {
                    match op.as_str() {
                        "names" => {
                            widget
                                .text_marks
                                .entry("insert".to_string())
                                .or_insert(widget.insert_cursor);
                            widget
                                .text_marks
                                .entry("current".to_string())
                                .or_insert(widget.insert_cursor);
                            let mut names: Vec<String> =
                                widget.text_marks.keys().cloned().collect();
                            names.sort_unstable();
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                names.as_slice(),
                                "failed to allocate text mark names tuple",
                            )
                            .map(Some);
                        }
                        "next" | "previous" => {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark next/previous expects one index or mark name",
                                ));
                            }
                            widget
                                .text_marks
                                .entry("insert".to_string())
                                .or_insert(widget.insert_cursor);
                            widget
                                .text_marks
                                .entry("current".to_string())
                                .or_insert(widget.insert_cursor);
                            let token = get_string_arg(py, handle, args[3], "mark index or name")?;
                            let mut ordered_marks: Vec<(usize, String)> = widget
                                .text_marks
                                .iter()
                                .map(|(name, index)| (*index, name.clone()))
                                .collect();
                            ordered_marks.sort_unstable_by(|left, right| {
                                left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1))
                            });
                            let selected = if let Some(index) =
                                widget.text_marks.get(&token).copied()
                            {
                                if op == "next" {
                                    ordered_marks
                                        .into_iter()
                                        .find_map(|(mark_index, mark_name)| {
                                            ((mark_index, mark_name.as_str())
                                                > (index, token.as_str()))
                                                .then_some(mark_name)
                                        })
                                } else {
                                    ordered_marks.into_iter().rev().find_map(
                                        |(mark_index, mark_name)| {
                                            ((mark_index, mark_name.as_str())
                                                < (index, token.as_str()))
                                                .then_some(mark_name)
                                        },
                                    )
                                }
                            } else {
                                let Some(index) =
                                    parse_text_index_bits(args[3], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "mark next/previous index must be an integer, end, line.column, or mark name",
                                    ));
                                };
                                if op == "next" {
                                    ordered_marks
                                        .into_iter()
                                        .find_map(|(mark_index, mark_name)| {
                                            (mark_index >= index).then_some(mark_name)
                                        })
                                } else {
                                    ordered_marks.into_iter().rev().find_map(
                                        |(mark_index, mark_name)| {
                                            (mark_index <= index).then_some(mark_name)
                                        },
                                    )
                                }
                            };
                            app.last_error = None;
                            if let Some(mark_name) = selected {
                                return alloc_string_bits(py, &mark_name).map(Some);
                            }
                            return alloc_empty_string_bits(py).map(Some);
                        }
                        "set" => {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark set expects mark name and index",
                                ));
                            }
                            let mark_name = get_string_arg(py, handle, args[3], "mark name")?;
                            let Some(index) = parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark set index must be an integer, end, or line.column",
                                ));
                            };
                            if mark_name == "insert" {
                                widget.insert_cursor = index;
                            }
                            widget.text_marks.insert(mark_name, index);
                            clamp_text_widget_indices(widget);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "unset" => {
                            for &mark_bits in &args[3..] {
                                let mark_name = get_string_arg(py, handle, mark_bits, "mark name")?;
                                widget.text_marks.remove(&mark_name);
                                widget.text_mark_gravity.remove(&mark_name);
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        "gravity" => {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark gravity expects mark name and optional direction",
                                ));
                            }
                            let mark_name = get_string_arg(py, handle, args[3], "mark name")?;
                            if args.len() == 4 {
                                let gravity = widget
                                    .text_mark_gravity
                                    .get(&mark_name)
                                    .cloned()
                                    .unwrap_or_else(|| "right".to_string());
                                app.last_error = None;
                                return alloc_string_bits(py, &gravity).map(Some);
                            }
                            let gravity =
                                get_string_arg(py, handle, args[4], "mark gravity direction")?;
                            let normalized = gravity.to_ascii_lowercase();
                            if normalized != "left" && normalized != "right" {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "mark gravity must be left or right",
                                ));
                            }
                            widget.text_mark_gravity.insert(mark_name, normalized);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        _ => {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                unknown_widget_subcommand_message(
                                    widget_path,
                                    &format!("mark {op}"),
                                ),
                            ));
                        }
                    }
                }
                match op.as_str() {
                    "names" => {
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "next" | "previous" => {
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("mark {op}")),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "tag" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "tag subcommand")?;
                match op.as_str() {
                    "bind" => {
                        if args.len() < 4 || args.len() > 7 {
                            return Err(app_tcl_error_locked(
                                py,
                                app,
                                "tag bind expects tagname, optional sequence, optional script",
                            ));
                        }
                        let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                        let bindings = widget.tag_bindings.entry(tag_name.clone()).or_default();
                        if args.len() == 4 {
                            let mut sequences: Vec<String> = bindings.keys().cloned().collect();
                            sequences.sort_unstable();
                            app.last_error = None;
                            return alloc_string_bits(py, &sequences.join(" ")).map(Some);
                        }
                        let sequence = get_string_arg(py, handle, args[4], "tag bind sequence")?;
                        if args.len() == 5 {
                            let script = bindings.get(&sequence).cloned().unwrap_or_default();
                            app.last_error = None;
                            return alloc_string_bits(py, &script).map(Some);
                        }
                        let mut script = get_string_arg(py, handle, args[5], "tag bind script")?;
                        if args.len() == 7 {
                            let command_name =
                                get_string_arg(py, handle, args[6], "tag bind callback id")?;
                            if script.is_empty() {
                                script = bindings.get(&sequence).cloned().unwrap_or_default();
                            }
                            script = remove_bind_script_command_invocations(&script, &command_name);
                        } else if script.starts_with('+') {
                            script = if let Some(previous) = bindings.get(&sequence) {
                                if previous.trim().is_empty() {
                                    script
                                } else {
                                    format!("{previous}\n{script}")
                                }
                            } else {
                                script
                            };
                        }
                        if script.is_empty() {
                            bindings.remove(&sequence);
                        } else {
                            bindings.insert(sequence, script);
                            if widget.widget_command == "text" {
                                ensure_text_tag_order_entry(widget, &tag_name);
                            }
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "names" => {
                        if widget.widget_command == "text" {
                            let mut names: HashSet<String> =
                                widget.text_tag_ranges.keys().cloned().collect();
                            names.extend(widget.tag_bindings.keys().cloned());
                            names.extend(widget.text_tag_options.keys().cloned());
                            if args.len() == 5 {
                                let Some(index) =
                                    parse_text_index_bits(args[4], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag names index must be an integer, end, or line.column",
                                    ));
                                };
                                names.retain(|tag_name| {
                                    widget.text_tag_ranges.get(tag_name).is_some_and(|ranges| {
                                        ranges
                                            .iter()
                                            .any(|(start, end)| index >= *start && index < *end)
                                    })
                                });
                            }
                            let mut ordered: Vec<String> = Vec::new();
                            for tag_name in &widget.text_tag_order {
                                if names.remove(tag_name) {
                                    ordered.push(tag_name.clone());
                                }
                            }
                            let mut leftovers: Vec<String> = names.into_iter().collect();
                            leftovers.sort_unstable();
                            ordered.extend(leftovers);
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                ordered.as_slice(),
                                "failed to allocate text tag names tuple",
                            )
                            .map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "ranges" => {
                        if widget.widget_command == "text" {
                            if args.len() != 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag ranges expects a tag name",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let mut values: Vec<String> = Vec::new();
                            if let Some(ranges) = widget.text_tag_ranges.get(&tag_name) {
                                for (start, end) in ranges {
                                    values.push(format!("1.{start}"));
                                    values.push(format!("1.{end}"));
                                }
                            }
                            app.last_error = None;
                            return alloc_tuple_from_strings(
                                py,
                                values.as_slice(),
                                "failed to allocate text tag ranges tuple",
                            )
                            .map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "nextrange" | "prevrange" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 && args.len() != 6 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag nextrange/prevrange expects tagname, start, and optional stop",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let Some(start_index) =
                                parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag range start index must be an integer, end, or line.column",
                                ));
                            };
                            let stop_index = if args.len() == 6 {
                                let Some(stop) = parse_text_index_bits(args[5], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag range stop index must be an integer, end, or line.column",
                                    ));
                                };
                                Some(stop)
                            } else {
                                None
                            };
                            let mut ranges = widget
                                .text_tag_ranges
                                .get(&tag_name)
                                .cloned()
                                .unwrap_or_default();
                            ranges.sort_unstable_by_key(|(start, _end)| *start);
                            let selected = if op == "nextrange" {
                                ranges.into_iter().find(|(start, end)| {
                                    *end > start_index
                                        && stop_index.is_none_or(|stop| *start < stop)
                                })
                            } else {
                                ranges.into_iter().rev().find(|(start, _end)| {
                                    *start < start_index
                                        && stop_index.is_none_or(|stop| *start >= stop)
                                })
                            };
                            if let Some((start, end)) = selected {
                                app.last_error = None;
                                return alloc_tuple_from_strings(
                                    py,
                                    &[format!("1.{start}"), format!("1.{end}")],
                                    "failed to allocate text tag range tuple",
                                )
                                .map(Some);
                            }
                            app.last_error = None;
                            return alloc_empty_tuple_bits(py).map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_tuple_bits(py).map(Some);
                    }
                    "cget" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag cget expects tagname and option",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let option_name =
                                parse_widget_option_name_arg(py, handle, args[4], "tag option")?;
                            let value = widget
                                .text_tag_options
                                .get(&tag_name)
                                .and_then(|options| options.get(&option_name))
                                .cloned()
                                .unwrap_or_default();
                            app.last_error = None;
                            return alloc_string_bits(py, &value).map(Some);
                        }
                        app.last_error = None;
                        return alloc_empty_string_bits(py).map(Some);
                    }
                    "add" => {
                        if widget.widget_command == "text" {
                            if args.len() < 6 || !(args.len() - 4).is_multiple_of(2) {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag add expects tagname plus one or more start/end index pairs",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let ranges =
                                widget.text_tag_ranges.entry(tag_name.clone()).or_default();
                            let mut idx = 4;
                            while idx + 1 < args.len() {
                                let Some(start) =
                                    parse_text_index_bits(args[idx], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag add start index must be an integer, end, or line.column",
                                    ));
                                };
                                let Some(end) =
                                    parse_text_index_bits(args[idx + 1], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag add end index must be an integer, end, or line.column",
                                    ));
                                };
                                if end > start {
                                    ranges.push((start, end));
                                } else if start > end {
                                    ranges.push((end, start));
                                }
                                idx += 2;
                            }
                            normalize_text_tag_ranges(ranges);
                            ensure_text_tag_order_entry(widget, &tag_name);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "remove" => {
                        if widget.widget_command == "text" {
                            if args.len() != 5 && args.len() != 6 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag remove expects tagname, start, and optional end index",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            let Some(start) = parse_text_index_bits(args[4], &widget.text_value)
                            else {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag remove start index must be an integer, end, or line.column",
                                ));
                            };
                            let end = if args.len() == 6 {
                                let Some(end) = parse_text_index_bits(args[5], &widget.text_value)
                                else {
                                    return Err(app_tcl_error_locked(
                                        py,
                                        app,
                                        "tag remove end index must be an integer, end, or line.column",
                                    ));
                                };
                                end
                            } else {
                                (start + 1).min(text_char_count(&widget.text_value))
                            };
                            let (remove_start, remove_end) = (start.min(end), start.max(end));
                            if let Some(ranges) = widget.text_tag_ranges.get_mut(&tag_name) {
                                let mut updated: Vec<(usize, usize)> =
                                    Vec::with_capacity(ranges.len().saturating_mul(2));
                                for (range_start, range_end) in ranges.iter().copied() {
                                    if range_end <= remove_start || range_start >= remove_end {
                                        updated.push((range_start, range_end));
                                        continue;
                                    }
                                    if range_start < remove_start {
                                        updated.push((range_start, remove_start));
                                    }
                                    if range_end > remove_end {
                                        updated.push((remove_end, range_end));
                                    }
                                }
                                *ranges = updated;
                                normalize_text_tag_ranges(ranges);
                                if ranges.is_empty() {
                                    widget.text_tag_ranges.remove(&tag_name);
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "delete" => {
                        if widget.widget_command == "text" {
                            for &tag_bits in &args[3..] {
                                let tag_name = get_string_arg(py, handle, tag_bits, "tag name")?;
                                widget.text_tag_ranges.remove(&tag_name);
                                widget.tag_bindings.remove(&tag_name);
                                widget.text_tag_options.remove(&tag_name);
                                widget.text_tag_order.retain(|name| name != &tag_name);
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "configure" => {
                        if widget.widget_command == "text" {
                            if args.len() < 4 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag configure expects tagname",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            if args.len() == 4 {
                                let options = widget.text_tag_options.get(&tag_name);
                                let mut out =
                                    Vec::with_capacity(options.map_or(0, HashMap::len) * 2);
                                let mut ordered: Vec<(&String, &String)> =
                                    options.map(|map| map.iter().collect()).unwrap_or_default();
                                ordered.sort_unstable_by(|left, right| left.0.cmp(right.0));
                                for (key, value) in ordered {
                                    out.push(key.clone());
                                    out.push(value.clone());
                                }
                                app.last_error = None;
                                return alloc_tuple_from_strings(
                                    py,
                                    out.as_slice(),
                                    "failed to allocate text tag configure tuple",
                                )
                                .map(Some);
                            }
                            if args.len() == 5 {
                                let option_name = parse_widget_option_name_arg(
                                    py,
                                    handle,
                                    args[4],
                                    "tag option",
                                )?;
                                let value = widget
                                    .text_tag_options
                                    .get(&tag_name)
                                    .and_then(|options| options.get(&option_name))
                                    .cloned()
                                    .unwrap_or_default();
                                app.last_error = None;
                                return alloc_string_bits(py, &value).map(Some);
                            }
                            if !(args.len() - 4).is_multiple_of(2) {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag configure expects key/value pairs",
                                ));
                            }
                            let options =
                                widget.text_tag_options.entry(tag_name.clone()).or_default();
                            let mut idx = 4;
                            while idx + 1 < args.len() {
                                let option_name = parse_widget_option_name_arg(
                                    py,
                                    handle,
                                    args[idx],
                                    "tag option",
                                )?;
                                let option_value =
                                    get_string_arg(py, handle, args[idx + 1], "tag value")?;
                                options.insert(option_name, option_value);
                                idx += 2;
                            }
                            ensure_text_tag_order_entry(widget, &tag_name);
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    "lower" | "raise" => {
                        if widget.widget_command == "text" {
                            if args.len() != 4 && args.len() != 5 {
                                return Err(app_tcl_error_locked(
                                    py,
                                    app,
                                    "tag lower/raise expects tagname and optional reference tag",
                                ));
                            }
                            let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                            ensure_text_tag_order_entry(widget, &tag_name);
                            widget.text_tag_order.retain(|name| name != &tag_name);
                            if args.len() == 4 {
                                if op == "lower" {
                                    widget.text_tag_order.insert(0, tag_name);
                                } else {
                                    widget.text_tag_order.push(tag_name);
                                }
                                app.last_error = None;
                                return Ok(Some(MoltObject::none().bits()));
                            }
                            let reference_name =
                                get_string_arg(py, handle, args[4], "reference tag name")?;
                            let reference_index = widget
                                .text_tag_order
                                .iter()
                                .position(|name| name == &reference_name);
                            match reference_index {
                                Some(index) => {
                                    let insert_index = if op == "lower" {
                                        index
                                    } else {
                                        index.saturating_add(1)
                                    }
                                    .min(widget.text_tag_order.len());
                                    widget.text_tag_order.insert(insert_index, tag_name);
                                }
                                None => {
                                    if op == "lower" {
                                        widget.text_tag_order.insert(0, tag_name);
                                    } else {
                                        widget.text_tag_order.push(tag_name);
                                    }
                                }
                            }
                            app.last_error = None;
                            return Ok(Some(MoltObject::none().bits()));
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    _ => {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            unknown_widget_subcommand_message(widget_path, &format!("tag {op}")),
                        ));
                    }
                }
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "proxy" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "proxy subcommand")?;
                if op == "coord" {
                    app.last_error = None;
                    return alloc_widget_coord_bits(py).map(Some);
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, &format!("proxy {op}")),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "sash" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "sash subcommand")?;
                if op == "coord" {
                    app.last_error = None;
                    return alloc_widget_coord_bits(py).map(Some);
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, &format!("sash {op}")),
                ));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "icursor" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "icursor expects exactly one index argument",
                ));
            }
            let index = if widget.widget_command == "text" {
                let Some(index) = parse_text_index_bits(args[2], &widget.text_value) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "text icursor index must be an integer, end, or line.column",
                    ));
                };
                index
            } else if matches!(widget.widget_command.as_str(), "entry" | "spinbox") {
                let Some(index) = parse_entry_like_index_bits(
                    args[2],
                    text_char_count(&widget.text_value),
                    widget.insert_cursor,
                    widget.selection_anchor,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "entry/spinbox icursor index must be an integer, end, insert, or anchor",
                    ));
                };
                index
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "icursor"),
                ));
            };
            widget.insert_cursor = index;
            clamp_text_widget_indices(widget);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "itemconfigure" => {
            if widget.widget_command != "listbox" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "itemconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "listbox itemconfigure expects index and optional key/value options",
                ));
            }
            let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "listbox itemconfigure index must be an integer or end",
                ));
            };
            if widget.list_items.is_empty() || index >= widget.list_items.len() {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("listbox item \"{index}\" is out of range"),
                ));
            }
            if args.len() == 3 {
                let options = widget
                    .list_item_options
                    .get(&index)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate listbox itemconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "listbox item option")?;
                if let Some(bits) = widget
                    .list_item_options
                    .get(&index)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "listbox item options")?;
            let options = widget.list_item_options.entry(index).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "entryconfigure" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "entryconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure expects index and optional key/value options",
                ));
            }
            let Some(index) = parse_menu_existing_index_bits(
                args[2],
                widget.menu_entries.len(),
                widget.menu_active_index,
            ) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure index must resolve to an existing entry",
                ));
            };
            if args.len() == 3 {
                let options = widget
                    .menu_entries
                    .get(index)
                    .map(|entry| entry.options.clone())
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate menu entryconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "menu entry option")?;
                if let Some(bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "menu entry options")?;
            let Some(entry) = widget.menu_entries.get_mut(index) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu entryconfigure target does not exist",
                ));
            };
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut entry.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "paneconfigure" => {
            if widget.widget_command != "panedwindow" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "paneconfigure"),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow paneconfigure expects child and optional key/value options",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            if !widget
                .pane_children
                .iter()
                .any(|existing| existing == &child)
            {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown pane \"{child}\""),
                ));
            }
            if args.len() == 3 {
                let options = widget
                    .pane_child_options
                    .get(&child)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate panedwindow paneconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
                if let Some(bits) = widget
                    .pane_child_options
                    .get(&child)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
            let options = widget.pane_child_options.entry(child).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "activate" => {
            if widget.widget_command == "listbox" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox activate expects exactly one index argument",
                    ));
                }
                let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
                else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "listbox activate index must be an integer or end",
                    ));
                };
                widget.list_active_index = Some(index);
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu activate expects exactly one index argument",
                    ));
                }
                widget.menu_active_index = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "post" => {
            if widget.widget_command == "menu" {
                if args.len() != 4 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu post expects x and y coordinates",
                    ));
                }
                let x = parse_i64_arg(py, handle, args[2], "menu post x")?;
                let y = parse_i64_arg(py, handle, args[3], "menu post y")?;
                widget.menu_posted_at = Some((x, y));
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "unpost" => {
            if widget.widget_command == "menu" {
                if args.len() != 2 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu unpost expects no additional arguments",
                    ));
                }
                widget.menu_posted_at = None;
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "tk_popup" => {
            if widget.widget_command != "menu" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "tk_popup"),
                ));
            }
            if args.len() != 4 && args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu tk_popup expects x, y, and optional entry index",
                ));
            }
            let x = parse_i64_arg(py, handle, args[2], "menu popup x")?;
            let y = parse_i64_arg(py, handle, args[3], "menu popup y")?;
            widget.menu_posted_at = Some((x, y));
            if args.len() == 5 {
                widget.menu_active_index = parse_menu_existing_index_bits(
                    args[4],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                );
            }
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "invoke" => {
            let mut invoke_words: Option<Vec<String>> = None;
            if widget.widget_command == "menu" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu invoke expects exactly one entry index",
                    ));
                }
                let Some(index) = parse_menu_existing_index_bits(
                    args[2],
                    widget.menu_entries.len(),
                    widget.menu_active_index,
                ) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "menu invoke index must resolve to an existing entry",
                    ));
                };
                if let Some(command_bits) = widget
                    .menu_entries
                    .get(index)
                    .and_then(|entry| entry.options.get("-command"))
                    .copied()
                {
                    let command = get_string_arg(py, handle, command_bits, "menu command")?;
                    if !command.trim().is_empty() {
                        invoke_words = Some(parse_command_words(&command));
                    }
                }
                widget.menu_active_index = Some(index);
            } else if matches!(
                widget.widget_command.as_str(),
                "button" | "checkbutton" | "radiobutton" | "menubutton"
            ) {
                if args.len() != 2 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "invoke expects no additional arguments",
                    ));
                }
                if let Some(command_bits) = widget.options.get("-command").copied() {
                    let command =
                        get_string_arg(py, handle, command_bits, "widget invoke command")?;
                    if !command.trim().is_empty() {
                        invoke_words = Some(parse_command_words(&command));
                    }
                }
            } else if widget.widget_command == "spinbox" {
                if args.len() != 3 {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "spinbox invoke expects exactly one element name",
                    ));
                }
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, "invoke"),
                ));
            }
            app.last_error = None;
            if let Some(words) = invoke_words {
                drop(registry);
                return call_tk_command_from_strings(py, handle, &words).map(Some);
            }
            Ok(Some(MoltObject::none().bits()))
        }
        "add" | "addtag" | "dtag" | "scan" | "image_configure" | "window_configure" | "see" => {
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            unknown_widget_subcommand_message(widget_path, subcommand),
        )),
    }
}

pub(super) fn handle_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget path command requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "widget subcommand")?;
    if let Some(bits) =
        handle_treeview_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) = handle_ttk_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    match subcommand.as_str() {
        "configure" => {
            if args.len() == 2 {
                clear_last_error(handle);
                return Ok(MoltObject::none().bits());
            }
            if !(args.len() - 2).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "configure expects key/value pairs",
                ));
            }
            let mut option_names = Vec::with_capacity((args.len() - 2) / 2);
            for idx in (2..args.len()).step_by(2) {
                option_names.push(get_string_arg(py, handle, args[idx], "widget option name")?);
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get_mut(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            for (idx, option_name) in option_names.into_iter().enumerate() {
                let value_bits = args[3 + idx * 2];
                inc_ref_bits(py, value_bits);
                if let Some(old_bits) = widget.options.insert(option_name, value_bits) {
                    dec_ref_bits(py, old_bits);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "cget expects exactly one option name",
                ));
            }
            let option_name = get_string_arg(py, handle, args[2], "widget option name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            let Some(value_bits) = widget.options.get(&option_name).copied() else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown option \"{option_name}\""),
                ));
            };
            inc_ref_bits(py, value_bits);
            app.last_error = None;
            Ok(value_bits)
        }
        "destroy" => {
            if args.len() != 2 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "destroy expects no additional arguments",
                ));
            }
            if widget_path == "." {
                let mut registry = tk_registry().lock().unwrap();
                let Some(mut app) = registry.apps.remove(&handle) else {
                    return Err(raise_invalid_handle_error(py));
                };
                drop_app_state_refs(py, &mut app);
                return Ok(MoltObject::none().bits());
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.remove(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            clear_widget_refs(py, widget);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            if let Some(bits) =
                handle_generic_widget_path_command(py, handle, widget_path, &subcommand, args)?
            {
                return Ok(bits);
            }
            Err(raise_tcl_for_handle(
                py,
                handle,
                unknown_widget_subcommand_message(widget_path, &subcommand),
            ))
        }
    }
}
