use super::super::*;
use super::common::*;

pub(in crate::tk) fn handle_listbox_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if !is_listbox_widget_subcommand(subcommand) {
        return Ok(None);
    }

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Ok(None);
    };
    if widget.widget_command != "listbox" {
        return Ok(None);
    }

    match subcommand {
        "insert" => {
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
            listbox_shift_item_options_for_insert(widget, original_insert_index, inserted_count);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "delete" => {
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
                let Some(last) = parse_listbox_index_bits(args[3], widget.list_items.len(), false)
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
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "get" => {
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
                let Some(last) = parse_listbox_index_bits(args[3], widget.list_items.len(), false)
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
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "size" | "count" => {
            app.last_error = None;
            Ok(Some(
                MoltObject::from_int(widget.list_items.len() as i64).bits(),
            ))
        }
        "index" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects exactly one index argument",
                ));
            }
            let Some(index) = parse_listbox_index_bits(args[2], widget.list_items.len(), false)
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "listbox index must be an integer or end",
                ));
            };
            app.last_error = None;
            Ok(Some(MoltObject::from_int(index as i64).bits()))
        }
        "nearest" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "nearest expects exactly one coordinate argument",
                ));
            }
            let y = parse_i64_arg(py, handle, args[2], "listbox nearest coordinate")?;
            let index = clamp_index_i64(y, widget.list_items.len().saturating_sub(1));
            app.last_error = None;
            Ok(Some(MoltObject::from_int(index as i64).bits()))
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
            let result = evaluate_index_compare(left, &op, right)
                .map_err(|message| app_tcl_error_locked(py, app, message))?;
            app.last_error = None;
            Ok(Some(MoltObject::from_bool(result).bits()))
        }
        "curselection" => {
            let mut indices: Vec<String> = widget
                .list_selection
                .iter()
                .copied()
                .filter(|idx| *idx < widget.list_items.len())
                .map(|idx| idx.to_string())
                .collect();
            indices.sort_unstable_by_key(|value| value.parse::<usize>().unwrap_or(0));
            app.last_error = None;
            alloc_tuple_from_strings(
                py,
                indices.as_slice(),
                "failed to allocate listbox curselection tuple",
            )
            .map(Some)
        }
        "itemcget" => {
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
            alloc_empty_string_bits(py).map(Some)
        }
        "itemconfigure" => {
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
        "activate" => {
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
            Ok(Some(MoltObject::none().bits()))
        }
        _ => Ok(None),
    }
}

fn is_listbox_widget_subcommand(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "insert"
            | "delete"
            | "get"
            | "size"
            | "count"
            | "index"
            | "nearest"
            | "compare"
            | "curselection"
            | "itemcget"
            | "itemconfigure"
            | "activate"
    )
}
