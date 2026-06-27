use super::super::*;
use super::common::*;
use super::selection;
use super::text;

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
        "selection" => selection::handle_selection_subcommand(
            py,
            handle,
            widget_path,
            &mut app.last_error,
            widget,
            args,
        ),
        "mark" => {
            text::handle_mark_subcommand(py, handle, widget_path, &mut app.last_error, widget, args)
        }
        "tag" => {
            text::handle_tag_subcommand(py, handle, widget_path, &mut app.last_error, widget, args)
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
