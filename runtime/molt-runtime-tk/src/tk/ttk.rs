use super::args::{get_string_arg, raise_tcl_for_handle};
use super::parsing::{
    TTK_NOTEBOOK_TAB_OPTIONS, TTK_PANEDWINDOW_PANE_OPTIONS, alloc_tuple_from_strings,
    option_allowed, option_map_query_or_empty, option_map_to_tuple, parse_notebook_index_strict,
    parse_ttk_insert_index_strict, parse_widget_option_name_arg, parse_widget_option_pairs,
    set_to_sorted_tuple,
};
use super::state::{
    alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, clear_value_map_refs,
    tk_registry, value_map_set_bits,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new};
use super::widgets::common::unknown_widget_subcommand_message;
use crate::bridge::{dec_ref_bits, inc_ref_bits, string_obj_to_owned, to_f64, to_i64};
use molt_runtime_core::prelude::{MoltObject, PyToken, obj_from_bits};

pub(super) fn handle_ttk_style_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "ttk::style requires a subcommand",
        ));
    }
    let style_subcommand = get_string_arg(py, handle, args[1], "ttk::style subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let style_state = &mut app.ttk_style;

    match style_subcommand.as_str() {
        "configure" | "map" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style configure/map expects a style name",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            let style_options = if style_subcommand == "configure" {
                style_state.configure.entry(style_name).or_default()
            } else {
                style_state.style_map.entry(style_name).or_default()
            };
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    style_options,
                    "failed to allocate ttk style option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "ttk style option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, style_options, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "ttk::style options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, style_options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "lookup" => {
            if args.len() < 4 || args.len() > 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style lookup expects style, option, optional state, optional default",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            let option_name =
                parse_widget_option_name_arg(py, handle, args[3], "ttk style option name")?;
            if let Some(value_bits) = style_state
                .style_map
                .get(&style_name)
                .and_then(|options| options.get(&option_name).copied())
                .or_else(|| {
                    style_state
                        .configure
                        .get(&style_name)
                        .and_then(|options| options.get(&option_name).copied())
                })
            {
                inc_ref_bits(py, value_bits);
                app.last_error = None;
                return Ok(value_bits);
            }
            if args.len() >= 6 {
                let default_bits = args[5];
                inc_ref_bits(py, default_bits);
                app.last_error = None;
                return Ok(default_bits);
            }
            app.last_error = None;
            alloc_string_bits(py, "")
        }
        "layout" => {
            if args.len() < 3 || args.len() > 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style layout expects style and optional layout spec",
                ));
            }
            let style_name = get_string_arg(py, handle, args[2], "ttk style name")?;
            if args.len() == 3 {
                if let Some(layout_bits) = style_state.layouts.get(&style_name).copied() {
                    inc_ref_bits(py, layout_bits);
                    app.last_error = None;
                    return Ok(layout_bits);
                }
                app.last_error = None;
                return alloc_string_bits(py, "");
            }
            let layout_bits = args[3];
            inc_ref_bits(py, layout_bits);
            if let Some(old_bits) = style_state.layouts.insert(style_name, layout_bits) {
                dec_ref_bits(py, old_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "element" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style element requires an operation",
                ));
            }
            let element_subcommand = get_string_arg(py, handle, args[2], "ttk style element op")?;
            match element_subcommand.as_str() {
                "create" => {
                    if args.len() < 5 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element create expects element and type",
                        ));
                    }
                    let element_name =
                        get_string_arg(py, handle, args[3], "ttk style element name")?;
                    style_state.elements.insert(element_name.clone());
                    let mut option_names = Vec::new();
                    let mut idx = 5;
                    while idx < args.len() {
                        let Some(name) = string_obj_to_owned(obj_from_bits(args[idx])) else {
                            idx += 1;
                            continue;
                        };
                        if !name.starts_with('-') {
                            idx += 1;
                            continue;
                        }
                        option_names.push(name);
                        idx += 2;
                    }
                    style_state
                        .element_options
                        .insert(element_name, option_names);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "names" => {
                    if args.len() != 3 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element names expects no extra arguments",
                        ));
                    }
                    app.last_error = None;
                    set_to_sorted_tuple(
                        py,
                        &style_state.elements,
                        "failed to allocate ttk style element tuple",
                    )
                }
                "options" => {
                    if args.len() != 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style element options expects an element name",
                        ));
                    }
                    let element_name =
                        get_string_arg(py, handle, args[3], "ttk style element name")?;
                    let option_names = style_state
                        .element_options
                        .get(&element_name)
                        .cloned()
                        .unwrap_or_default();
                    app.last_error = None;
                    alloc_tuple_from_strings(
                        py,
                        option_names.as_slice(),
                        "failed to allocate ttk style element option tuple",
                    )
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!(
                        "bad ttk::style element option \"{element_subcommand}\": must be create, names, or options"
                    ),
                )),
            }
        }
        "theme" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "ttk::style theme requires an operation",
                ));
            }
            let theme_subcommand = get_string_arg(py, handle, args[2], "ttk style theme op")?;
            match theme_subcommand.as_str() {
                "create" => {
                    if args.len() < 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme create expects a theme name",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name.clone());
                    if style_state.current_theme.is_none() {
                        style_state.current_theme = Some(theme_name);
                    }
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "settings" => {
                    if args.len() != 5 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme settings expects theme and settings",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "names" => {
                    if args.len() != 3 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme names expects no extra arguments",
                        ));
                    }
                    app.last_error = None;
                    set_to_sorted_tuple(
                        py,
                        &style_state.themes,
                        "failed to allocate ttk style theme tuple",
                    )
                }
                "use" => {
                    if args.len() == 3 {
                        app.last_error = None;
                        return if let Some(current) = style_state.current_theme.as_deref() {
                            alloc_string_bits(py, current)
                        } else {
                            alloc_string_bits(py, "")
                        };
                    }
                    if args.len() != 4 {
                        return Err(app_tcl_error_locked(
                            py,
                            app,
                            "ttk::style theme use expects optional theme name",
                        ));
                    }
                    let theme_name = get_string_arg(py, handle, args[3], "ttk theme name")?;
                    style_state.themes.insert(theme_name.clone());
                    style_state.current_theme = Some(theme_name);
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!(
                        "bad ttk::style theme option \"{theme_subcommand}\": must be create, names, settings, or use"
                    ),
                )),
            }
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!(
                "bad ttk::style option \"{style_subcommand}\": must be configure, element, layout, lookup, map, or theme"
            ),
        )),
    }
}

pub(super) fn handle_ttk_notebook_enable_traversal(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "ttk::notebook::enableTraversal expects a notebook widget path",
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "notebook widget path")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    if widget.widget_command != "ttk::notebook" {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("widget \"{widget_path}\" is not a ttk::notebook"),
        ));
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_ttk_widget_path_command(
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
    if !widget.widget_command.starts_with("ttk::") {
        return Ok(None);
    }
    let widget_command = widget.widget_command.as_str();
    let is_ttk_entry = widget_command == "ttk::entry";
    let is_ttk_combobox = widget_command == "ttk::combobox";
    let is_ttk_spinbox = widget_command == "ttk::spinbox";
    let is_ttk_progressbar = widget_command == "ttk::progressbar";
    let is_ttk_notebook = widget_command == "ttk::notebook";
    let is_ttk_panedwindow = widget_command == "ttk::panedwindow";
    let supports_ttk_invoke = matches!(
        widget_command,
        "ttk::button" | "ttk::checkbutton" | "ttk::radiobutton" | "ttk::menubutton"
    );
    let supports_ttk_current = is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_set = is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_bbox = is_ttk_entry || is_ttk_combobox || is_ttk_spinbox;
    let supports_ttk_validate = is_ttk_entry || is_ttk_combobox || is_ttk_spinbox;

    match subcommand {
        "state" => {
            if args.len() == 2 {
                app.last_error = None;
                return set_to_sorted_tuple(
                    py,
                    &widget.ttk_state,
                    "failed to allocate ttk state tuple",
                )
                .map(Some);
            }
            for &state_bits in &args[2..] {
                let state_spec = get_string_arg(py, handle, state_bits, "ttk state spec")?;
                if state_spec.is_empty() {
                    continue;
                }
                if let Some(removed) = state_spec.strip_prefix('!') {
                    if !removed.is_empty() {
                        widget.ttk_state.remove(removed);
                    }
                    continue;
                }
                widget.ttk_state.insert(state_spec);
            }
            app.last_error = None;
            return set_to_sorted_tuple(
                py,
                &widget.ttk_state,
                "failed to allocate ttk state tuple",
            )
            .map(Some);
        }
        "instate" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "instate expects at least one state specifier",
                ));
            }
            let mut matches_all = true;
            for &state_bits in &args[2..] {
                let state_spec = get_string_arg(py, handle, state_bits, "ttk state spec")?;
                if state_spec.is_empty() {
                    continue;
                }
                let (negated, state_name) = if let Some(raw) = state_spec.strip_prefix('!') {
                    (true, raw)
                } else {
                    (false, state_spec.as_str())
                };
                let has_state = widget.ttk_state.contains(state_name);
                if (negated && has_state) || (!negated && !has_state) {
                    matches_all = false;
                    break;
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::from_bool(matches_all).bits()));
        }
        "identify" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "identify expects x and y coordinates",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "element").map(Some);
        }
        "invoke" => {
            if !supports_ttk_invoke {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "invoke expects no extra arguments",
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "current" => {
            if !supports_ttk_current {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() == 2 {
                if let Some(bits) = widget.ttk_values.get("-current").copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(-1).bits()));
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "current expects optional index argument",
                ));
            }
            let Some(index) = to_i64(obj_from_bits(args[2])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "combobox index must be an integer",
                ));
            };
            value_map_set_bits(
                py,
                &mut widget.ttk_values,
                "-current".to_string(),
                MoltObject::from_int(index).bits(),
            );
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "set" => {
            if !supports_ttk_set {
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
                    "set expects a value argument",
                ));
            }
            value_map_set_bits(py, &mut widget.ttk_values, "-value".to_string(), args[2]);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "bbox" => {
            if !supports_ttk_bbox {
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
                    "bbox expects an index argument",
                ));
            }
            let bbox = vec![
                "1".to_string(),
                "2".to_string(),
                "3".to_string(),
                "4".to_string(),
            ];
            app.last_error = None;
            return alloc_tuple_from_strings(py, &bbox, "failed to allocate ttk bbox tuple")
                .map(Some);
        }
        "validate" => {
            if !supports_ttk_validate {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "validate expects no extra arguments",
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::from_bool(true).bits()));
        }
        "get" => {
            if is_ttk_entry {
                return Ok(None);
            }
            if !supports_ttk_set {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "get expects no extra arguments",
                ));
            }
            if let Some(bits) = widget.ttk_values.get("-value").copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(Some(bits));
            }
            app.last_error = None;
            return alloc_string_bits(py, "").map(Some);
        }
        "start" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 && args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "start expects optional interval argument",
                ));
            }
            widget.ttk_state.insert("running".to_string());
            if args.len() == 3 {
                value_map_set_bits(py, &mut widget.ttk_values, "-interval".to_string(), args[2]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "step" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 && args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "step expects optional amount argument",
                ));
            }
            let current = widget
                .ttk_values
                .get("-value")
                .and_then(|bits| {
                    to_f64(obj_from_bits(*bits))
                        .or_else(|| to_i64(obj_from_bits(*bits)).map(|v| v as f64))
                })
                .unwrap_or(0.0);
            let amount = if args.len() == 3 {
                let amount_obj = obj_from_bits(args[2]);
                if let Some(value) = to_f64(amount_obj) {
                    value
                } else if let Some(value) = to_i64(amount_obj) {
                    value as f64
                } else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "progressbar step amount must be numeric",
                    ));
                }
            } else {
                1.0
            };
            value_map_set_bits(
                py,
                &mut widget.ttk_values,
                "-value".to_string(),
                MoltObject::from_float(current + amount).bits(),
            );
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "stop" => {
            if !is_ttk_progressbar {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "stop expects no extra arguments",
                ));
            }
            widget.ttk_state.remove("running");
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        _ => {}
    }

    match subcommand {
        "add" => {
            if !is_ttk_notebook && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "add expects a child widget path",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk child path")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                widget.ttk_items.push(child.clone());
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "ttk item options")?;
            let allowed_options = if is_ttk_notebook {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            let item_options = widget.ttk_item_options.entry(child.clone()).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            if is_ttk_notebook
                && !widget.ttk_values.contains_key("-selected")
                && let Ok(child_bits) = alloc_string_bits(py, &child)
            {
                value_map_set_bits(
                    py,
                    &mut widget.ttk_values,
                    "-selected".to_string(),
                    child_bits,
                );
                dec_ref_bits(py, child_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "insert" => {
            if !is_ttk_notebook && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "insert expects index and child widget path",
                ));
            }
            let index_spec = get_string_arg(py, handle, args[2], "ttk insert index")?;
            let child = get_string_arg(py, handle, args[3], "ttk child path")?;
            widget.ttk_items.retain(|existing| existing != &child);
            let Some(index) = parse_ttk_insert_index_strict(&index_spec, widget.ttk_items.len())
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("ttk insert index \"{index_spec}\" must be an integer or end"),
                ));
            };
            widget.ttk_items.insert(index, child.clone());
            let option_pairs = parse_widget_option_pairs(py, handle, args, 4, "ttk item options")?;
            let allowed_options = if is_ttk_notebook {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            let item_options = widget.ttk_item_options.entry(child.clone()).or_default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            if is_ttk_notebook
                && !widget.ttk_values.contains_key("-selected")
                && let Ok(child_bits) = alloc_string_bits(py, &child)
            {
                value_map_set_bits(
                    py,
                    &mut widget.ttk_values,
                    "-selected".to_string(),
                    child_bits,
                );
                dec_ref_bits(py, child_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "forget" | "hide" => {
            if subcommand == "hide" && !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if subcommand == "forget" && !is_ttk_notebook && !is_ttk_panedwindow {
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
                    format!("{subcommand} expects a child widget path"),
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk child path")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} \"{child}\" is not managed by {widget_path}"),
                ));
            }
            widget.ttk_items.retain(|existing| existing != &child);
            if subcommand == "forget"
                && let Some(mut old_options) = widget.ttk_item_options.remove(&child)
            {
                clear_value_map_refs(py, &mut old_options);
            }
            if is_ttk_notebook {
                let selected_child = widget
                    .ttk_values
                    .get("-selected")
                    .and_then(|bits| string_obj_to_owned(obj_from_bits(*bits)));
                if selected_child.as_deref() == Some(child.as_str()) {
                    if let Some(next_selected) = widget.ttk_items.first()
                        && let Ok(bits) = alloc_string_bits(py, next_selected)
                    {
                        value_map_set_bits(
                            py,
                            &mut widget.ttk_values,
                            "-selected".to_string(),
                            bits,
                        );
                        dec_ref_bits(py, bits);
                    } else if let Some(old_bits) = widget.ttk_values.remove("-selected") {
                        dec_ref_bits(py, old_bits);
                    }
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "index" => {
            if !is_ttk_notebook {
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
                    "index expects a tab identifier",
                ));
            }
            let target = get_string_arg(py, handle, args[2], "ttk tab identifier")?;
            let idx =
                if let Some(position) = widget.ttk_items.iter().position(|item| item == &target) {
                    position as i64
                } else {
                    match parse_notebook_index_strict(&target, widget.ttk_items.len()) {
                        Ok(value) => value,
                        Err(message) => return Err(app_tcl_error_locked(py, app, message)),
                    }
                };
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(idx).bits()));
        }
        "select" => {
            if !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() == 2 {
                if let Some(bits) = widget.ttk_values.get("-selected").copied() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                if let Some(first_child) = widget.ttk_items.first() {
                    app.last_error = None;
                    return alloc_string_bits(py, first_child).map(Some);
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "select expects optional tab identifier",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "ttk tab identifier")?;
            if !widget.ttk_items.iter().any(|existing| existing == &child) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("tab \"{child}\" is not managed by {widget_path}"),
                ));
            }
            if let Ok(bits) = alloc_string_bits(py, &child) {
                value_map_set_bits(py, &mut widget.ttk_values, "-selected".to_string(), bits);
                dec_ref_bits(py, bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tab" | "pane" => {
            if subcommand == "tab" && !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if subcommand == "pane" && !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} expects an item identifier"),
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "ttk item id")?;
            if !widget.ttk_items.iter().any(|existing| existing == &item_id) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{subcommand} \"{item_id}\" is not managed by {widget_path}"),
                ));
            }
            let allowed_options = if subcommand == "tab" {
                TTK_NOTEBOOK_TAB_OPTIONS
            } else {
                TTK_PANEDWINDOW_PANE_OPTIONS
            };
            let item_options = widget.ttk_item_options.entry(item_id).or_default();
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    item_options,
                    "failed to allocate ttk item option tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "ttk option name")?;
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
                app.last_error = None;
                return option_map_query_or_empty(py, item_options, &option_name).map(Some);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "ttk item options")?;
            for (option_name, _) in &option_pairs {
                if !option_allowed(option_name.as_str(), allowed_options) {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
            }
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, item_options, option_name, value_bits);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tabs" => {
            if !is_ttk_notebook {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tabs expects no extra arguments",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                widget.ttk_items.as_slice(),
                "failed to allocate ttk tabs tuple",
            )
            .map(Some);
        }
        "panes" => {
            if !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panes expects no extra arguments",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                widget.ttk_items.as_slice(),
                "failed to allocate ttk panes tuple",
            )
            .map(Some);
        }
        "sashpos" => {
            if !is_ttk_panedwindow {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    unknown_widget_subcommand_message(widget_path, subcommand),
                ));
            }
            if args.len() != 3 && args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sashpos expects index and optional position",
                ));
            }
            let Some(index) = to_i64(obj_from_bits(args[2])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sash index must be an integer",
                ));
            };
            if args.len() == 3 {
                let current = widget.ttk_sash_positions.get(&index).copied().unwrap_or(0);
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(current).bits()));
            }
            let Some(position) = to_i64(obj_from_bits(args[3])) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "sash position must be an integer",
                ));
            };
            widget.ttk_sash_positions.insert(index, position);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        _ => {}
    }

    Ok(None)
}
