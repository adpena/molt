use super::super::args::get_string_arg;
use super::super::event_commands::remove_bind_script_command_invocations;
use super::super::parsing::{
    alloc_tuple_from_strings, parse_text_index_bits, parse_widget_option_name_arg, text_char_count,
};
use super::super::state::{TkWidgetState, alloc_string_bits};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::tcl::{get, new};
use super::common::{
    alloc_empty_string_bits, alloc_empty_tuple_bits, clamp_text_widget_indices,
    ensure_text_tag_order_entry, normalize_text_tag_ranges, unknown_widget_subcommand_message,
    widget_tcl_error,
};
use molt_runtime_core::prelude::{MoltObject, PyToken};
use std::collections::{HashMap, HashSet};

pub(super) fn handle_mark_subcommand(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    last_error: &mut Option<String>,
    widget: &mut TkWidgetState,
    args: &[u64],
) -> Result<Option<u64>, u64> {
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
                    let mut names: Vec<String> = widget.text_marks.keys().cloned().collect();
                    names.sort_unstable();
                    *last_error = None;
                    return alloc_tuple_from_strings(
                        py,
                        names.as_slice(),
                        "failed to allocate text mark names tuple",
                    )
                    .map(Some);
                }
                "next" | "previous" => {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
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
                    let selected = if let Some(index) = widget.text_marks.get(&token).copied() {
                        if op == "next" {
                            ordered_marks
                                .into_iter()
                                .find_map(|(mark_index, mark_name)| {
                                    ((mark_index, mark_name.as_str()) > (index, token.as_str()))
                                        .then_some(mark_name)
                                })
                        } else {
                            ordered_marks
                                .into_iter()
                                .rev()
                                .find_map(|(mark_index, mark_name)| {
                                    ((mark_index, mark_name.as_str()) < (index, token.as_str()))
                                        .then_some(mark_name)
                                })
                        }
                    } else {
                        let Some(index) = parse_text_index_bits(args[3], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            ordered_marks
                                .into_iter()
                                .rev()
                                .find_map(|(mark_index, mark_name)| {
                                    (mark_index <= index).then_some(mark_name)
                                })
                        }
                    };
                    *last_error = None;
                    if let Some(mark_name) = selected {
                        return alloc_string_bits(py, &mark_name).map(Some);
                    }
                    return alloc_empty_string_bits(py).map(Some);
                }
                "set" => {
                    if args.len() != 5 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "mark set expects mark name and index",
                        ));
                    }
                    let mark_name = get_string_arg(py, handle, args[3], "mark name")?;
                    let Some(index) = parse_text_index_bits(args[4], &widget.text_value) else {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "mark set index must be an integer, end, or line.column",
                        ));
                    };
                    if mark_name == "insert" {
                        widget.insert_cursor = index;
                    }
                    widget.text_marks.insert(mark_name, index);
                    clamp_text_widget_indices(widget);
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "unset" => {
                    for &mark_bits in &args[3..] {
                        let mark_name = get_string_arg(py, handle, mark_bits, "mark name")?;
                        widget.text_marks.remove(&mark_name);
                        widget.text_mark_gravity.remove(&mark_name);
                    }
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "gravity" => {
                    if args.len() != 4 && args.len() != 5 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
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
                        *last_error = None;
                        return alloc_string_bits(py, &gravity).map(Some);
                    }
                    let gravity = get_string_arg(py, handle, args[4], "mark gravity direction")?;
                    let normalized = gravity.to_ascii_lowercase();
                    if normalized != "left" && normalized != "right" {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "mark gravity must be left or right",
                        ));
                    }
                    widget.text_mark_gravity.insert(mark_name, normalized);
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                _ => {
                    return Err(widget_tcl_error(
                        py,
                        last_error,
                        unknown_widget_subcommand_message(widget_path, &format!("mark {op}")),
                    ));
                }
            }
        }
        match op.as_str() {
            "names" => {
                *last_error = None;
                return alloc_empty_tuple_bits(py).map(Some);
            }
            "next" | "previous" => {
                *last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            _ => {
                return Err(widget_tcl_error(
                    py,
                    last_error,
                    unknown_widget_subcommand_message(widget_path, &format!("mark {op}")),
                ));
            }
        }
    }
    *last_error = None;
    Ok(Some(MoltObject::none().bits()))
}

pub(super) fn handle_tag_subcommand(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    last_error: &mut Option<String>,
    widget: &mut TkWidgetState,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if args.len() >= 3 {
        let op = get_string_arg(py, handle, args[2], "tag subcommand")?;
        match op.as_str() {
            "bind" => {
                if args.len() < 4 || args.len() > 7 {
                    return Err(widget_tcl_error(
                        py,
                        last_error,
                        "tag bind expects tagname, optional sequence, optional script",
                    ));
                }
                let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                let bindings = widget.tag_bindings.entry(tag_name.clone()).or_default();
                if args.len() == 4 {
                    let mut sequences: Vec<String> = bindings.keys().cloned().collect();
                    sequences.sort_unstable();
                    *last_error = None;
                    return alloc_string_bits(py, &sequences.join(" ")).map(Some);
                }
                let sequence = get_string_arg(py, handle, args[4], "tag bind sequence")?;
                if args.len() == 5 {
                    let script = bindings.get(&sequence).cloned().unwrap_or_default();
                    *last_error = None;
                    return alloc_string_bits(py, &script).map(Some);
                }
                let mut script = get_string_arg(py, handle, args[5], "tag bind script")?;
                if args.len() == 7 {
                    let command_name = get_string_arg(py, handle, args[6], "tag bind callback id")?;
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
                *last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            "names" => {
                if widget.widget_command == "text" {
                    let mut names: HashSet<String> =
                        widget.text_tag_ranges.keys().cloned().collect();
                    names.extend(widget.tag_bindings.keys().cloned());
                    names.extend(widget.text_tag_options.keys().cloned());
                    if args.len() == 5 {
                        let Some(index) = parse_text_index_bits(args[4], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                    *last_error = None;
                    return alloc_tuple_from_strings(
                        py,
                        ordered.as_slice(),
                        "failed to allocate text tag names tuple",
                    )
                    .map(Some);
                }
                *last_error = None;
                return alloc_empty_tuple_bits(py).map(Some);
            }
            "ranges" => {
                if widget.widget_command == "text" {
                    if args.len() != 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
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
                    *last_error = None;
                    return alloc_tuple_from_strings(
                        py,
                        values.as_slice(),
                        "failed to allocate text tag ranges tuple",
                    )
                    .map(Some);
                }
                *last_error = None;
                return alloc_empty_tuple_bits(py).map(Some);
            }
            "nextrange" | "prevrange" => {
                if widget.widget_command == "text" {
                    if args.len() != 5 && args.len() != 6 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag nextrange/prevrange expects tagname, start, and optional stop",
                        ));
                    }
                    let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                    let Some(start_index) = parse_text_index_bits(args[4], &widget.text_value)
                    else {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag range start index must be an integer, end, or line.column",
                        ));
                    };
                    let stop_index = if args.len() == 6 {
                        let Some(stop) = parse_text_index_bits(args[5], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                            *end > start_index && stop_index.is_none_or(|stop| *start < stop)
                        })
                    } else {
                        ranges.into_iter().rev().find(|(start, _end)| {
                            *start < start_index && stop_index.is_none_or(|stop| *start >= stop)
                        })
                    };
                    if let Some((start, end)) = selected {
                        *last_error = None;
                        return alloc_tuple_from_strings(
                            py,
                            &[format!("1.{start}"), format!("1.{end}")],
                            "failed to allocate text tag range tuple",
                        )
                        .map(Some);
                    }
                    *last_error = None;
                    return alloc_empty_tuple_bits(py).map(Some);
                }
                *last_error = None;
                return alloc_empty_tuple_bits(py).map(Some);
            }
            "cget" => {
                if widget.widget_command == "text" {
                    if args.len() != 5 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
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
                    *last_error = None;
                    return alloc_string_bits(py, &value).map(Some);
                }
                *last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            "add" => {
                if widget.widget_command == "text" {
                    if args.len() < 6 || !(args.len() - 4).is_multiple_of(2) {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag add expects tagname plus one or more start/end index pairs",
                        ));
                    }
                    let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                    let ranges = widget.text_tag_ranges.entry(tag_name.clone()).or_default();
                    let mut idx = 4;
                    while idx + 1 < args.len() {
                        let Some(start) = parse_text_index_bits(args[idx], &widget.text_value)
                        else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
                                "tag add start index must be an integer, end, or line.column",
                            ));
                        };
                        let Some(end) = parse_text_index_bits(args[idx + 1], &widget.text_value)
                        else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                *last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            "remove" => {
                if widget.widget_command == "text" {
                    if args.len() != 5 && args.len() != 6 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag remove expects tagname, start, and optional end index",
                        ));
                    }
                    let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                    let Some(start) = parse_text_index_bits(args[4], &widget.text_value) else {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag remove start index must be an integer, end, or line.column",
                        ));
                    };
                    let end = if args.len() == 6 {
                        let Some(end) = parse_text_index_bits(args[5], &widget.text_value) else {
                            return Err(widget_tcl_error(
                                py,
                                last_error,
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                *last_error = None;
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                *last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            "configure" => {
                if widget.widget_command == "text" {
                    if args.len() < 4 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag configure expects tagname",
                        ));
                    }
                    let tag_name = get_string_arg(py, handle, args[3], "tag name")?;
                    if args.len() == 4 {
                        let options = widget.text_tag_options.get(&tag_name);
                        let mut out = Vec::with_capacity(options.map_or(0, HashMap::len) * 2);
                        let mut ordered: Vec<(&String, &String)> =
                            options.map(|map| map.iter().collect()).unwrap_or_default();
                        ordered.sort_unstable_by(|left, right| left.0.cmp(right.0));
                        for (key, value) in ordered {
                            out.push(key.clone());
                            out.push(value.clone());
                        }
                        *last_error = None;
                        return alloc_tuple_from_strings(
                            py,
                            out.as_slice(),
                            "failed to allocate text tag configure tuple",
                        )
                        .map(Some);
                    }
                    if args.len() == 5 {
                        let option_name =
                            parse_widget_option_name_arg(py, handle, args[4], "tag option")?;
                        let value = widget
                            .text_tag_options
                            .get(&tag_name)
                            .and_then(|options| options.get(&option_name))
                            .cloned()
                            .unwrap_or_default();
                        *last_error = None;
                        return alloc_string_bits(py, &value).map(Some);
                    }
                    if !(args.len() - 4).is_multiple_of(2) {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
                            "tag configure expects key/value pairs",
                        ));
                    }
                    let options = widget.text_tag_options.entry(tag_name.clone()).or_default();
                    let mut idx = 4;
                    while idx + 1 < args.len() {
                        let option_name =
                            parse_widget_option_name_arg(py, handle, args[idx], "tag option")?;
                        let option_value = get_string_arg(py, handle, args[idx + 1], "tag value")?;
                        options.insert(option_name, option_value);
                        idx += 2;
                    }
                    ensure_text_tag_order_entry(widget, &tag_name);
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                *last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            "lower" | "raise" => {
                if widget.widget_command == "text" {
                    if args.len() != 4 && args.len() != 5 {
                        return Err(widget_tcl_error(
                            py,
                            last_error,
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
                        *last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    let reference_name = get_string_arg(py, handle, args[4], "reference tag name")?;
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
                    *last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                *last_error = None;
                return Ok(Some(MoltObject::none().bits()));
            }
            _ => {
                return Err(widget_tcl_error(
                    py,
                    last_error,
                    unknown_widget_subcommand_message(widget_path, &format!("tag {op}")),
                ));
            }
        }
    }
    *last_error = None;
    Ok(Some(MoltObject::none().bits()))
}
