use super::super::*;
use super::common::*;

pub(super) fn handle_selection_subcommand(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    last_error: &mut Option<String>,
    widget: &mut TkWidgetState,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if args.len() >= 3 {
        let op = get_string_arg(py, handle, args[2], "selection subcommand")?;
        if matches!(widget.widget_command.as_str(), "entry" | "text" | "spinbox") {
            match op.as_str() {
                "clear" => {
                    widget.selection_range = None;
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "present" => {
                    let present = widget
                        .selection_range
                        .is_some_and(|(start, end)| end > start);
                    *last_error = None;
                    return Ok(Some(MoltObject::from_bool(present).bits()));
                }
                "get" => {
                    if let Some((start, end)) = widget.selection_range
                        && end > start
                    {
                        let start_byte = char_index_to_byte_index(&widget.text_value, start);
                        let end_byte = char_index_to_byte_index(&widget.text_value, end);
                        let slice = widget.text_value[start_byte..end_byte].to_string();
                        *last_error = None;
                        return alloc_string_bits(py, &slice).map(Some);
                    }
                    *last_error = None;
                    return alloc_empty_string_bits(py).map(Some);
                }
                "from" => {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "selection from expects one index argument",
                        ));
                    }
                    let index = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
                                "selection from index must be an integer, end, insert, or anchor index",
                            ));
                        };
                        index
                    };
                    widget.selection_anchor = Some(index);
                    widget.selection_range = Some((index, index));
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "to" => {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "selection to expects one index argument",
                        ));
                    }
                    let index = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "range" => {
                    if args.len() != 5 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "selection range expects start and end indices",
                        ));
                    }
                    let start = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
                                "selection range start index must be an integer, end, insert, or anchor index",
                            ));
                        };
                        index
                    };
                    let end = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[4], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
                                "selection range end index must be an integer, end, insert, or anchor index",
                            ));
                        };
                        index
                    };
                    widget.selection_range = Some((start.min(end), start.max(end)));
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "includes" => {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "selection includes expects one index argument",
                        ));
                    }
                    let index = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
                                "selection includes index must be an integer, end, insert, or anchor index",
                            ));
                        };
                        index
                    };
                    let includes = widget
                        .selection_range
                        .is_some_and(|(start, end)| index >= start && index < end);
                    *last_error = None;
                    return Ok(Some(MoltObject::from_bool(includes).bits()));
                }
                "adjust" => {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "selection adjust expects one index argument",
                        ));
                    }
                    let index = if widget.widget_command == "text" {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "element" => {
                    *last_error = None;
                    return alloc_empty_string_bits(py).map(Some);
                }
                _ => {
                    return Err(widget_tcl_error(
                        py,
                        last_error,
                        unknown_widget_subcommand_message(widget_path, &format!("selection {op}")),
                    ));
                }
            }
        }
        match op.as_str() {
            "includes" | "present" => {
                *last_error = None;
                return Ok(Some(MoltObject::from_bool(false).bits()));
            }
            "get" => {
                *last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            _ => {
                return Err(widget_tcl_error(
                    py,
                    last_error,
                    unknown_widget_subcommand_message(widget_path, &format!("selection {op}")),
                ));
            }
        }
    }
    *last_error = None;
    Ok(Some(MoltObject::none().bits()))
}
