use super::super::args::{get_string_arg, raise_tcl_for_handle};
use super::super::parsing::{
    menu_item_type_supported, option_map_to_tuple, parse_command_words, parse_i64_arg,
    parse_menu_existing_index_bits, parse_menu_insert_index_bits, parse_widget_option_name_arg,
    parse_widget_option_pairs,
};
use super::super::state::{
    TkMenuEntryState, TkWidgetState, alloc_string_bits, app_mut_from_registry,
    app_tcl_error_locked, clear_value_map_refs, tk_registry, value_map_set_bits,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::tcl::get;
use super::super::trace_commands::call_tk_command_from_strings;
use super::common::alloc_empty_string_bits;
use crate::bridge::inc_ref_bits;
use molt_runtime_core::prelude::{MoltObject, PyToken};

pub(in crate::tk) fn handle_menu_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if !is_menu_widget_subcommand(subcommand) {
        return Ok(None);
    }

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Ok(None);
    };
    if widget.widget_command != "menu" {
        return Ok(None);
    }

    match subcommand {
        "add" => {
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
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "menu add options")?;
            let mut entry = TkMenuEntryState {
                item_type,
                ..TkMenuEntryState::default()
            };
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut entry.options, option_name, value_bits);
            }
            widget.menu_entries.push(entry);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "insert" => {
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
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "delete" => {
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
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "index" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects exactly one index argument",
                ));
            }
            let maybe_index = parse_menu_existing_index_bits(
                args[2],
                widget.menu_entries.len(),
                widget.menu_active_index,
            );
            app.last_error = None;
            if let Some(index) = maybe_index {
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            Ok(Some(MoltObject::none().bits()))
        }
        "type" => {
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
            alloc_empty_string_bits(py).map(Some)
        }
        "entrycget" => {
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
            alloc_empty_string_bits(py).map(Some)
        }
        "entryconfigure" => {
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
        "xposition" | "yposition" => {
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
        "activate" => {
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
            Ok(Some(MoltObject::none().bits()))
        }
        "post" => {
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
            Ok(Some(MoltObject::none().bits()))
        }
        "unpost" => {
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu unpost expects no additional arguments",
                ));
            }
            widget.menu_posted_at = None;
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "tk_popup" => {
            if args.len() != 4 && args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "menu tk_popup expects x, y, and optional entry index",
                ));
            }
            let x = parse_i64_arg(py, handle, args[2], "menu popup x")?;
            let y = parse_i64_arg(py, handle, args[3], "menu popup y")?;
            set_menu_popup_state_locked(widget, x, y, args.get(4).copied());
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "invoke" => {
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
            let mut invoke_words: Option<Vec<String>> = None;
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
            app.last_error = None;
            if let Some(words) = invoke_words {
                drop(registry);
                return call_tk_command_from_strings(py, handle, &words).map(Some);
            }
            Ok(Some(MoltObject::none().bits()))
        }
        _ => Ok(None),
    }
}

pub(in crate::tk) fn handle_menu_popup_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 4 && args.len() != 5 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tk_popup expects menu path, x, y, and optional entry index",
        ));
    }
    let menu_path = get_string_arg(py, handle, args[1], "tk_popup menu path")?;
    let x = parse_i64_arg(py, handle, args[2], "tk_popup x")?;
    let y = parse_i64_arg(py, handle, args[3], "tk_popup y")?;
    set_menu_popup_state(py, handle, &menu_path, x, y, args.get(4).copied())
}

fn is_menu_widget_subcommand(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "add"
            | "insert"
            | "delete"
            | "index"
            | "type"
            | "entrycget"
            | "entryconfigure"
            | "xposition"
            | "yposition"
            | "activate"
            | "post"
            | "unpost"
            | "tk_popup"
            | "invoke"
    )
}

fn set_menu_popup_state(
    py: &PyToken,
    handle: i64,
    menu_path: &str,
    x: i64,
    y: i64,
    entry_bits: Option<u64>,
) -> Result<u64, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(menu_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{menu_path}\""),
        ));
    };
    if widget.widget_command != "menu" {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("widget \"{menu_path}\" is not a menu"),
        ));
    }
    set_menu_popup_state_locked(widget, x, y, entry_bits);
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

fn set_menu_popup_state_locked(
    widget: &mut TkWidgetState,
    x: i64,
    y: i64,
    entry_bits: Option<u64>,
) {
    widget.menu_posted_at = Some((x, y));
    if let Some(bits) = entry_bits {
        widget.menu_active_index = parse_menu_existing_index_bits(
            bits,
            widget.menu_entries.len(),
            widget.menu_active_index,
        );
    }
}
