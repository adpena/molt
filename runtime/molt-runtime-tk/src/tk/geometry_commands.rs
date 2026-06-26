use super::*;

pub(super) fn widget_layout_options_mut<'a>(
    widget: &'a mut TkWidgetState,
    manager: &str,
) -> &'a mut HashMap<String, u64> {
    match manager {
        "pack" => &mut widget.pack_options,
        "grid" => &mut widget.grid_options,
        "place" => &mut widget.place_options,
        _ => &mut widget.pack_options,
    }
}

pub(super) fn widget_layout_options<'a>(
    widget: &'a TkWidgetState,
    manager: &str,
) -> &'a HashMap<String, u64> {
    match manager {
        "pack" => &widget.pack_options,
        "grid" => &widget.grid_options,
        "place" => &widget.place_options,
        _ => &widget.pack_options,
    }
}

pub(super) fn handle_geometry_command(
    py: &PyToken,
    handle: i64,
    manager: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{manager} requires a subcommand"),
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "geometry subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "configure" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} configure expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            if args.len() == 3 {
                let Some(widget) = app.widgets.get(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    widget_layout_options(widget, manager),
                    "failed to allocate geometry option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "geometry option name")?;
                let Some(widget) = app.widgets.get(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                app.last_error = None;
                return option_map_query_or_empty(
                    py,
                    widget_layout_options(widget, manager),
                    &option_name,
                );
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "geometry options")?;
            {
                let Some(widget) = app.widgets.get_mut(&widget_path) else {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        format!("bad window path name \"{widget_path}\""),
                    ));
                };
                let options = widget_layout_options_mut(widget, manager);
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, options, option_name, value_bits);
                }
                widget.manager = Some(manager.to_string());
            }
            ensure_layout_membership(app, manager, &widget_path);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "forget" | "remove" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} {subcommand} expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            let Some(widget) = app.widgets.get_mut(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            if widget.manager.as_deref() == Some(manager) {
                widget.manager = None;
            }
            remove_widget_from_layout_lists(app, &widget_path);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} info expects a widget path"),
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "geometry widget path")?;
            let Some(widget) = app.widgets.get(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            app.last_error = None;
            option_map_to_tuple(
                py,
                widget_layout_options(widget, manager),
                "failed to allocate geometry info tuple",
            )
        }
        "propagate" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} propagate expects widget and optional flag"),
                ));
            }
            let container = get_string_arg(py, handle, args[2], "geometry container path")?;
            let propagate_map = if manager == "grid" {
                &mut app.grid_propagate
            } else {
                &mut app.pack_propagate
            };
            if args.len() == 3 {
                let current = propagate_map.get(&container).copied().unwrap_or(true);
                app.last_error = None;
                return Ok(MoltObject::from_bool(current).bits());
            }
            let enabled = parse_bool_arg(py, handle, args[3], "geometry propagate flag")?;
            propagate_map.insert(container, enabled);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "slaves" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("{manager} slaves expects a container path"),
                ));
            }
            let container = get_string_arg(py, handle, args[2], "geometry container path")?;
            let items = if container == "." {
                if manager == "pack" {
                    app.pack_slaves.clone()
                } else if manager == "grid" {
                    app.grid_slaves.clone()
                } else {
                    app.place_slaves.clone()
                }
            } else {
                Vec::new()
            };
            app.last_error = None;
            alloc_tuple_from_strings(py, items.as_slice(), "failed to allocate geometry slaves")
        }
        "bbox" if manager == "grid" => {
            if args.len() < 3 || args.len() > 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid bbox expects container and optional index bounds",
                ));
            }
            let bbox = vec![
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
            ];
            app.last_error = None;
            alloc_tuple_from_strings(py, &bbox, "failed to allocate grid bbox tuple")
        }
        "location" if manager == "grid" => {
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid location expects container path and x/y coordinates",
                ));
            }
            app.last_error = None;
            alloc_int_tuple2_bits(py, 0, 0, "failed to allocate grid location tuple")
        }
        "size" if manager == "grid" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid size expects a container path",
                ));
            }
            app.last_error = None;
            alloc_int_tuple2_bits(
                py,
                0,
                app.grid_slaves.len() as i64,
                "failed to allocate grid size tuple",
            )
        }
        "columnconfigure" | "rowconfigure" if manager == "grid" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grid row/columnconfigure expects container and index",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grid container path")?;
            let index = get_string_arg(py, handle, args[3], "grid index")?;
            let Some(widget) = app.widgets.get_mut(&widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            let configs = if subcommand == "columnconfigure" {
                widget.grid_columnconfigure.entry(index).or_default()
            } else {
                widget.grid_rowconfigure.entry(index).or_default()
            };
            if args.len() == 4 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    configs,
                    "failed to allocate grid row/columnconfigure tuple",
                );
            }
            if args.len() == 5 {
                let option_name = parse_widget_option_name_arg(
                    py,
                    handle,
                    args[4],
                    "grid row/columnconfigure option",
                )?;
                app.last_error = None;
                return option_map_query_or_empty(py, configs, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 4, "grid row/columnconfigure options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, configs, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad {manager} option \"{subcommand}\""),
        )),
    }
}

pub(super) fn handle_raise_or_lower_command(
    py: &PyToken,
    handle: i64,
    command: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{command} expects widget and optional sibling"),
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "widget path")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    let manager = widget.manager.clone();
    let order_list = match manager.as_deref() {
        Some("pack") => &mut app.pack_slaves,
        Some("grid") => &mut app.grid_slaves,
        Some("place") => &mut app.place_slaves,
        _ => {
            app.last_error = None;
            return Ok(MoltObject::none().bits());
        }
    };
    if let Some(idx) = order_list.iter().position(|name| name == &widget_path) {
        order_list.remove(idx);
    }
    if command == "raise" {
        if args.len() == 3 {
            let sibling = get_string_arg(py, handle, args[2], "sibling widget path")?;
            if let Some(idx) = order_list.iter().position(|name| name == &sibling) {
                order_list.insert(idx + 1, widget_path);
            } else {
                order_list.push(widget_path);
            }
        } else {
            order_list.push(widget_path);
        }
    } else if args.len() == 3 {
        let sibling = get_string_arg(py, handle, args[2], "sibling widget path")?;
        if let Some(idx) = order_list.iter().position(|name| name == &sibling) {
            order_list.insert(idx, widget_path);
        } else {
            order_list.insert(0, widget_path);
        }
    } else {
        order_list.insert(0, widget_path);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}
