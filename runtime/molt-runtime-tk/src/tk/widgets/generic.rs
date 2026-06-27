use super::super::args::{get_string_arg, get_text_arg};
use super::super::event_commands::remove_bind_script_command_invocations;
use super::super::parsing::{
    alloc_tuple_from_strings, char_index_to_byte_index, parse_bool_arg, parse_command_words,
    parse_entry_like_index_bits, parse_text_index_bits, run_tk_word_commands, text_char_count,
};
use super::super::state::{
    alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry,
};
use super::super::trace_commands::call_tk_command_from_strings;
use super::common::{
    alloc_empty_string_bits, alloc_empty_tuple_bits, alloc_widget_bbox_bits,
    alloc_widget_view_bits, clamp_text_widget_indices, evaluate_index_compare,
    unknown_widget_subcommand_message,
};
use molt_runtime_core::prelude::{MoltObject, PyToken};

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
        "insert" => {
            if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox")
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
            if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
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
            if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
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
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(0).bits()));
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
            app.last_error = None;
            alloc_empty_tuple_bits(py).map(Some)
        }
        "find" | "tabs" | "panes" => {
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
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "itemcget" => {
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "entrycget" => {
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
        "itemconfigure" => Err(app_tcl_error_locked(
            py,
            app,
            unknown_widget_subcommand_message(widget_path, "itemconfigure"),
        )),
        "activate" => {
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "post" => {
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "unpost" => {
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "invoke" => {
            let mut invoke_words: Option<Vec<String>> = None;
            if matches!(
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
