use super::super::args::get_string_arg;
use super::super::parsing::{
    alloc_tuple_bits, alloc_tuple_from_strings, clamp_index_i64, option_map_to_tuple,
    parse_i64_arg, parse_listbox_index_bits, parse_widget_option_name_arg,
    parse_widget_option_pairs,
};
use super::super::state::{
    TkWidgetState, app_mut_from_registry, app_tcl_error_locked, clear_value_map_refs, tk_registry,
    value_map_set_bits,
};
use super::common::{
    alloc_empty_string_bits, alloc_empty_tuple_bits, evaluate_index_compare,
    unknown_widget_subcommand_message,
};
use crate::bridge::{dec_ref_bits, inc_ref_bits};
use molt_runtime_core::prelude::{MoltObject, PyToken};
use std::collections::{HashMap, HashSet};

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
            listbox_shift_selection_for_insert(widget, original_insert_index, inserted_count);
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
                    listbox_reindex_selection_after_delete(widget, first, end, removed_count);
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
        "selection" => {
            if args.len() < 3 {
                app.last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            let op = get_string_arg(py, handle, args[2], "selection subcommand")?;
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
                    Ok(Some(MoltObject::none().bits()))
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
                        let Some(last) =
                            parse_listbox_index_bits(args[4], widget.list_items.len(), false)
                        else {
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
                    listbox_select_range(widget, first, last);
                    app.last_error = None;
                    Ok(Some(MoltObject::none().bits()))
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
                        let Some(last) =
                            parse_listbox_index_bits(args[4], widget.list_items.len(), false)
                        else {
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
                    listbox_clear_range(widget, first, last);
                    app.last_error = None;
                    Ok(Some(MoltObject::none().bits()))
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
                    Ok(Some(
                        MoltObject::from_bool(widget.list_selection.contains(&index)).bits(),
                    ))
                }
                "present" => {
                    app.last_error = None;
                    Ok(Some(
                        MoltObject::from_bool(!widget.list_selection.is_empty()).bits(),
                    ))
                }
                "get" => {
                    let mut selected: Vec<usize> = widget.list_selection.iter().copied().collect();
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
                    alloc_empty_string_bits(py).map(Some)
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, &format!("selection {op}")),
                )),
            }
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
            | "selection"
            | "itemcget"
            | "itemconfigure"
            | "activate"
    )
}

fn listbox_shift_selection_for_insert(
    widget: &mut TkWidgetState,
    insert_index: usize,
    inserted_count: usize,
) {
    if inserted_count == 0 || widget.list_selection.is_empty() {
        return;
    }
    let mut shifted = HashSet::with_capacity(widget.list_selection.len());
    for index in widget.list_selection.drain() {
        if index >= insert_index {
            shifted.insert(index + inserted_count);
        } else {
            shifted.insert(index);
        }
    }
    widget.list_selection = shifted;
}

fn listbox_reindex_selection_after_delete(
    widget: &mut TkWidgetState,
    first: usize,
    end: usize,
    removed_count: usize,
) {
    if widget.list_selection.is_empty() {
        return;
    }
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

fn listbox_select_range(widget: &mut TkWidgetState, first: usize, last: usize) {
    if widget.list_items.is_empty() {
        return;
    }
    let end = last.min(widget.list_items.len() - 1);
    if end >= first {
        for idx in first..=end {
            widget.list_selection.insert(idx);
        }
    }
}

fn listbox_clear_range(widget: &mut TkWidgetState, first: usize, last: usize) {
    let end = last.max(first);
    widget
        .list_selection
        .retain(|index| *index < first || *index > end);
}

fn listbox_shift_item_options_for_insert(
    widget: &mut TkWidgetState,
    insert_index: usize,
    inserted_count: usize,
) {
    if inserted_count == 0 {
        return;
    }
    if !widget.list_item_options.is_empty() {
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
    }
    if let Some(active_index) = widget.list_active_index
        && active_index >= insert_index
    {
        widget.list_active_index = Some(active_index.saturating_add(inserted_count));
    }
}

fn listbox_reindex_item_options_after_delete(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn listbox_with_len(len: usize) -> TkWidgetState {
        TkWidgetState {
            widget_command: "listbox".to_string(),
            list_items: (0..len).map(|idx| idx as u64).collect(),
            ..TkWidgetState::default()
        }
    }

    fn sorted_indices(indices: &HashSet<usize>) -> Vec<usize> {
        let mut out: Vec<usize> = indices.iter().copied().collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn listbox_state_reindexing_keeps_selection_active_and_options_coherent() {
        let py = PyToken::new();
        let mut widget = listbox_with_len(6);
        widget.list_selection = HashSet::from([1, 3]);
        widget.list_active_index = Some(3);
        widget.list_item_options.insert(2, HashMap::new());
        widget.list_item_options.insert(4, HashMap::new());

        listbox_shift_selection_for_insert(&mut widget, 2, 2);
        listbox_shift_item_options_for_insert(&mut widget, 2, 2);

        assert_eq!(sorted_indices(&widget.list_selection), vec![1, 5]);
        assert_eq!(widget.list_active_index, Some(5));
        assert!(widget.list_item_options.contains_key(&4));
        assert!(widget.list_item_options.contains_key(&6));

        listbox_select_range(&mut widget, 2, 4);
        listbox_clear_range(&mut widget, 3, 3);
        assert_eq!(sorted_indices(&widget.list_selection), vec![1, 2, 4, 5]);

        listbox_reindex_selection_after_delete(&mut widget, 2, 4, 3);
        listbox_reindex_item_options_after_delete(&py, &mut widget, 2, 4);

        assert_eq!(sorted_indices(&widget.list_selection), vec![1, 2]);
        assert_eq!(widget.list_active_index, Some(2));
        assert!(!widget.list_item_options.contains_key(&4));
        assert!(widget.list_item_options.contains_key(&3));
    }

    #[test]
    fn listbox_insert_shifts_active_index_without_item_options() {
        let mut widget = listbox_with_len(3);
        widget.list_active_index = Some(2);

        listbox_shift_item_options_for_insert(&mut widget, 1, 2);

        assert_eq!(widget.list_active_index, Some(4));
        assert!(widget.list_item_options.is_empty());
    }
}
