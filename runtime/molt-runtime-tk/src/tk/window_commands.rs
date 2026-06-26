use super::*;

pub(super) fn handle_focus_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match args.len() {
        1 => {
            let value = app.focus_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        2 => {
            let target = get_string_arg(py, handle, args[1], "focus target")?;
            app.focus_widget = if target.is_empty() {
                None
            } else {
                Some(target)
            };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        3 => {
            let op = get_string_arg(py, handle, args[1], "focus option")?;
            let target = get_string_arg(py, handle, args[2], "focus target")?;
            match op.as_str() {
                "-force" => {
                    app.focus_widget = if target.is_empty() {
                        None
                    } else {
                        Some(target)
                    };
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "-lastfor" => {
                    if app.focus_widget.is_none() {
                        app.focus_widget = if target.is_empty() {
                            None
                        } else {
                            Some(target.clone())
                        };
                    }
                    let value = app.focus_widget.clone().unwrap_or_default();
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                "-displayof" => {
                    let value = app.focus_widget.clone().unwrap_or(target);
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad focus option \"{op}\": must be -displayof, -force, or -lastfor"),
                )),
            }
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            "focus expects no args, a target, or -force/-lastfor target",
        )),
    }
}

pub(super) fn handle_focus_direction_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
    label: &str,
) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} expects a widget target"),
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "focus widget")?;
    clear_last_error(handle);
    alloc_string_bits(py, &widget_path)
}

pub(super) fn handle_grab_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "grab requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "grab subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "set" => {
            if args.len() == 3 {
                let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = false;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            if args.len() == 4 {
                let scope = get_string_arg(py, handle, args[2], "grab scope")?;
                if scope != "-global" {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "grab set scope must be -global",
                    ));
                }
                let widget_path = get_string_arg(py, handle, args[3], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = true;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            Err(app_tcl_error_locked(
                py,
                app,
                "grab set expects widget or -global widget",
            ))
        }
        "release" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab release expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                app.grab_widget = None;
                app.grab_is_global = false;
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "current" => {
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab current expects no extra arguments",
                ));
            }
            let widget_path = app.grab_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &widget_path)
        }
        "status" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab status expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            let status = if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                if app.grab_is_global {
                    "global"
                } else {
                    "local"
                }
            } else {
                ""
            };
            app.last_error = None;
            alloc_string_bits(py, status)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad grab option \"{subcommand}\": must be current, release, set, or status"),
        )),
    }
}

pub(super) fn handle_clipboard_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "clipboard requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "clipboard subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.clipboard_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "append" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "clipboard append expects a string payload",
                ));
            }
            let mut payload = String::new();
            let mut idx = 2;
            while idx < args.len() {
                let token = get_string_arg(py, handle, args[idx], "clipboard token")?;
                if token == "--" && idx + 1 < args.len() {
                    payload = get_string_arg(py, handle, args[idx + 1], "clipboard payload")?;
                    break;
                }
                payload = token;
                idx += 1;
            }
            app.clipboard_text.push_str(&payload);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            app.last_error = None;
            alloc_string_bits(py, &app.clipboard_text)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad clipboard option \"{subcommand}\": must be append, clear, or get"),
        )),
    }
}

pub(super) fn handle_selection_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "selection requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "selection subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.selection_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            let value = if app.selection_text.is_empty() {
                app.clipboard_text.clone()
            } else {
                app.selection_text.clone()
            };
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        "own" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_string_bits(py, app.selection_owner.as_deref().unwrap_or(""));
            }
            let mut owner: Option<String> = None;
            for &bits in &args[2..] {
                let token = get_string_arg(py, handle, bits, "selection own argument")?;
                if token.starts_with('-') {
                    continue;
                }
                owner = Some(token);
            }
            app.selection_owner = owner;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "handle" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad selection option \"{subcommand}\": must be clear, get, handle, or own"),
        )),
    }
}

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

pub(super) fn handle_wm_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "wm expects operation and toplevel path",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "wm subcommand")?;
    let toplevel = get_string_arg(py, handle, args[2], "wm toplevel path")?;
    if toplevel != "." {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        if wm_state_for_path(app, &toplevel).is_none() {
            return Err(app_tcl_error_locked(
                py,
                app,
                format!("bad window path name \"{toplevel}\""),
            ));
        }
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(wm_ptr) = (if toplevel == "." {
        Some((&mut app.wm) as *mut TkWmState)
    } else {
        app.widgets
            .get_mut(&toplevel)
            .and_then(|widget| widget.wm.as_mut())
            .map(|wm| wm as *mut TkWmState)
    }) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{toplevel}\""),
        ));
    };
    // The target WM state lives inside `app` and remains valid for the duration
    // of this command because we do not mutate `app.widgets` while handling a
    // single `wm` subcommand. A raw pointer keeps Rust's borrow checker from
    // treating `app.last_error` updates as overlapping borrows of the same app.
    let wm = unsafe { &mut *wm_ptr };
    match subcommand.as_str() {
        "title" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.title);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm title expects optional title value",
                ));
            }
            wm.title = get_string_arg(py, handle, args[3], "wm title")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "geometry" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.geometry);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm geometry expects optional geometry spec",
                ));
            }
            wm.geometry = get_string_arg(py, handle, args[3], "wm geometry spec")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "state" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.state);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm state expects optional state value",
                ));
            }
            wm.state = get_string_arg(py, handle, args[3], "wm state")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "attributes" => {
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(py, &wm.attributes, "failed to allocate wm attributes");
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "wm attribute name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &wm.attributes, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "wm attributes")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut wm.attributes, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "aspect" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((min_num, min_den, max_num, max_den)) = wm.aspect {
                    return alloc_tuple_from_strings(
                        py,
                        &[
                            min_num.to_string(),
                            min_den.to_string(),
                            max_num.to_string(),
                            max_den.to_string(),
                        ],
                        "failed to allocate wm aspect tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() == 4 {
                let value = get_string_arg(py, handle, args[3], "wm aspect value")?;
                if value.is_empty() {
                    wm.aspect = None;
                    app.last_error = None;
                    return Ok(MoltObject::none().bits());
                }
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm aspect expects 4 integer arguments or empty string",
                ));
            }
            if args.len() != 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm aspect expects 4 integer arguments",
                ));
            }
            wm.aspect = Some((
                parse_i64_arg(py, handle, args[3], "wm aspect minNumerator")?,
                parse_i64_arg(py, handle, args[4], "wm aspect minDenominator")?,
                parse_i64_arg(py, handle, args[5], "wm aspect maxNumerator")?,
                parse_i64_arg(py, handle, args[6], "wm aspect maxDenominator")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "client" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.client);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm client expects optional name",
                ));
            }
            wm.client = get_string_arg(py, handle, args[3], "wm client name")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "colormapwindows" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    wm.colormapwindows.as_slice(),
                    "failed to allocate wm colormapwindows tuple",
                );
            }
            wm.colormapwindows.clear();
            for &bits in &args[3..] {
                wm.colormapwindows.push(get_string_arg(
                    py,
                    handle,
                    bits,
                    "wm colormap window path",
                )?);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "command" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    wm.command.as_slice(),
                    "failed to allocate wm command tuple",
                );
            }
            wm.command.clear();
            for &bits in &args[3..] {
                wm.command
                    .push(get_string_arg(py, handle, bits, "wm command argument")?);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "focusmodel" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.focusmodel);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm focusmodel expects optional model",
                ));
            }
            wm.focusmodel = get_string_arg(py, handle, args[3], "wm focusmodel")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "forget" | "manage" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "frame" => {
            app.last_error = None;
            alloc_string_bits(py, &wm.frame)
        }
        "grid" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((base_width, base_height, width_inc, height_inc)) = wm.grid {
                    return alloc_tuple_from_strings(
                        py,
                        &[
                            base_width.to_string(),
                            base_height.to_string(),
                            width_inc.to_string(),
                            height_inc.to_string(),
                        ],
                        "failed to allocate wm grid tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() == 4 {
                let value = get_string_arg(py, handle, args[3], "wm grid value")?;
                if value.is_empty() {
                    wm.grid = None;
                    app.last_error = None;
                    return Ok(MoltObject::none().bits());
                }
            }
            if args.len() != 7 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm grid expects 4 integer arguments",
                ));
            }
            wm.grid = Some((
                parse_i64_arg(py, handle, args[3], "wm grid baseWidth")?,
                parse_i64_arg(py, handle, args[4], "wm grid baseHeight")?,
                parse_i64_arg(py, handle, args[5], "wm grid widthInc")?,
                parse_i64_arg(py, handle, args[6], "wm grid heightInc")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "group" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.group.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm group expects optional path",
                ));
            }
            let value = get_string_arg(py, handle, args[3], "wm group path")?;
            wm.group = if value.is_empty() { None } else { Some(value) };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconbitmap" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconbitmap);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconbitmap expects optional bitmap path",
                ));
            }
            wm.iconbitmap = get_string_arg(py, handle, args[3], "wm iconbitmap path")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconmask" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconmask);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconmask expects optional mask path",
                ));
            }
            wm.iconmask = get_string_arg(py, handle, args[3], "wm iconmask path")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconphoto" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconposition" => {
            if args.len() == 3 {
                app.last_error = None;
                if let Some((x, y)) = wm.iconposition {
                    return alloc_int_tuple2_bits(
                        py,
                        x,
                        y,
                        "failed to allocate wm iconposition tuple",
                    );
                }
                return alloc_empty_string_bits(py);
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconposition expects x and y",
                ));
            }
            wm.iconposition = Some((
                parse_i64_arg(py, handle, args[3], "wm iconposition x")?,
                parse_i64_arg(py, handle, args[4], "wm iconposition y")?,
            ));
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconwindow" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.iconwindow.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconwindow expects optional widget path",
                ));
            }
            let value = get_string_arg(py, handle, args[3], "wm iconwindow path")?;
            wm.iconwindow = if value.is_empty() { None } else { Some(value) };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "resizable" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    i64::from(wm.resizable_width),
                    i64::from(wm.resizable_height),
                    "failed to allocate wm resizable tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm resizable expects width and height",
                ));
            }
            wm.resizable_width = parse_bool_arg(py, handle, args[3], "wm resizable width")?;
            wm.resizable_height = parse_bool_arg(py, handle, args[4], "wm resizable height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "protocol" => {
            if args.len() == 3 {
                let mut names: Vec<String> = wm.protocols.keys().cloned().collect();
                names.sort_unstable();
                let mut flat = Vec::with_capacity(names.len() * 2);
                for name in names {
                    let Some(cmd) = wm.protocols.get(&name) else {
                        continue;
                    };
                    flat.push(name);
                    flat.push(cmd.clone());
                }
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    flat.as_slice(),
                    "failed to allocate wm protocol tuple",
                );
            }
            if args.len() == 4 {
                let protocol_name = get_string_arg(py, handle, args[3], "wm protocol name")?;
                let command_name = wm
                    .protocols
                    .get(&protocol_name)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &command_name);
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm protocol expects name and optional command",
                ));
            }
            let protocol_name = get_string_arg(py, handle, args[3], "wm protocol name")?;
            let command_name = get_string_arg(py, handle, args[4], "wm protocol callback")?;
            if command_name.is_empty() {
                wm.protocols.remove(&protocol_name);
            } else {
                wm.protocols.insert(protocol_name, command_name);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconify" => {
            wm.state = "iconic".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "deiconify" => {
            wm.state = "normal".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "withdraw" => {
            wm.state = "withdrawn".to_string();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "minsize" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    wm.minsize.0,
                    wm.minsize.1,
                    "failed to allocate wm minsize tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm minsize expects width and height",
                ));
            }
            wm.minsize.0 = parse_i64_arg(py, handle, args[3], "wm minsize width")?;
            wm.minsize.1 = parse_i64_arg(py, handle, args[4], "wm minsize height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "maxsize" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_int_tuple2_bits(
                    py,
                    wm.maxsize.0,
                    wm.maxsize.1,
                    "failed to allocate wm maxsize tuple",
                );
            }
            if args.len() != 5 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm maxsize expects width and height",
                ));
            }
            wm.maxsize.0 = parse_i64_arg(py, handle, args[3], "wm maxsize width")?;
            wm.maxsize.1 = parse_i64_arg(py, handle, args[4], "wm maxsize height")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "overrideredirect" => {
            if args.len() == 3 {
                app.last_error = None;
                return Ok(MoltObject::from_bool(wm.overrideredirect).bits());
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm overrideredirect expects optional boolean",
                ));
            }
            wm.overrideredirect = parse_bool_arg(py, handle, args[3], "wm overrideredirect flag")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "transient" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, wm.transient.as_deref().unwrap_or(""));
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm transient expects optional master path",
                ));
            }
            let master_path = get_string_arg(py, handle, args[3], "wm transient master")?;
            wm.transient = if master_path.is_empty() {
                None
            } else {
                Some(master_path)
            };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "iconname" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.iconname);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm iconname expects optional string",
                ));
            }
            wm.iconname = get_string_arg(py, handle, args[3], "wm iconname")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "positionfrom" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.positionfrom);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm positionfrom expects optional source",
                ));
            }
            wm.positionfrom = get_string_arg(py, handle, args[3], "wm positionfrom source")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "sizefrom" => {
            if args.len() == 3 {
                app.last_error = None;
                return alloc_string_bits(py, &wm.sizefrom);
            }
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "wm sizefrom expects optional source",
                ));
            }
            wm.sizefrom = get_string_arg(py, handle, args[3], "wm sizefrom source")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            if args.len() == 3 {
                app.last_error = None;
                alloc_empty_string_bits(py)
            } else {
                app.last_error = None;
                Ok(MoltObject::none().bits())
            }
        }
    }
}

pub(super) fn handle_winfo_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "winfo requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "winfo subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "children" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo children expects a widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let children: Vec<String> = if path == "." {
                let mut names: Vec<String> = app.widgets.keys().cloned().collect();
                names.sort_unstable();
                names
            } else {
                Vec::new()
            };
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                children.as_slice(),
                "failed to allocate children",
            );
        }
        "exists" | "ismapped" | "viewable" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo exists/ismapped/viewable expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let exists = path == "." || app.widgets.contains_key(&path);
            let value = if subcommand == "exists" {
                exists
            } else if path == "." {
                true
            } else {
                app.widgets
                    .get(&path)
                    .is_some_and(|widget| widget.manager.is_some())
            };
            app.last_error = None;
            return Ok(MoltObject::from_bool(value).bits());
        }
        "manager" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo manager expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                "wm".to_string()
            } else {
                app.widgets
                    .get(&path)
                    .and_then(|widget| widget.manager.clone())
                    .unwrap_or_default()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "class" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo class expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let class_name = if path == "." {
                "Tk".to_string()
            } else if let Some(widget) = app.widgets.get(&path) {
                tk_widget_class_name(&widget.widget_command)
            } else {
                String::new()
            };
            app.last_error = None;
            return alloc_string_bits(py, &class_name);
        }
        "name" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo name expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let name = if path == "." {
                "tk".to_string()
            } else {
                path.trim_start_matches('.')
                    .trim_start_matches('!')
                    .to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "parent" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo parent expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let parent = if path == "." {
                String::new()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &parent);
        }
        "toplevel" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo toplevel expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ".");
        }
        "id" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo id expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let id = if path == "." {
                1
            } else {
                (path
                    .bytes()
                    .fold(17_u64, |acc, b| acc.wrapping_mul(33).wrapping_add(b as u64))
                    % 1_000_000) as i64
                    + 2
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "width" | "reqwidth" | "height" | "reqheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo width/height/reqwidth/reqheight expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                if subcommand.ends_with("width") {
                    200
                } else {
                    160
                }
            } else if let Some(widget) = app.widgets.get(&path) {
                if subcommand.ends_with("width") {
                    widget_option_i64_default(&widget.options, "-width", 200)
                } else {
                    widget_option_i64_default(&widget.options, "-height", 160)
                }
            } else {
                0
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "x" | "y" | "rootx" | "rooty" | "pointerx" | "pointery" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo coordinate query expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        "screenwidth" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "screenheight" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "pointerxy" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pointerxy expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_int_tuple2_bits(py, 0, 0, "failed to allocate pointerxy tuple");
        }
        "rgb" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo rgb expects widget path and color",
                ));
            }
            let color = get_string_arg(py, handle, args[3], "winfo color")?;
            let (r, g, b) = parse_winfo_rgb_components(&color);
            let elems = vec![
                MoltObject::from_int(r).bits(),
                MoltObject::from_int(g).bits(),
                MoltObject::from_int(b).bits(),
            ];
            app.last_error = None;
            return alloc_tuple_bits(py, elems.as_slice(), "failed to allocate winfo rgb tuple");
        }
        "atom" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atom expects atom name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "atom name")?;
            let id = if let Some(id) = app.atoms_by_name.get(&name).copied() {
                id
            } else {
                app.next_atom_id = app.next_atom_id.saturating_add(1);
                let id = app.next_atom_id;
                app.atoms_by_name.insert(name.clone(), id);
                app.atoms_by_id.insert(id, name);
                id
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "atomname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atomname expects atom id",
                ));
            }
            let atom_id = parse_i64_arg(py, handle, args[2], "atom id")?;
            let name = app.atoms_by_id.get(&atom_id).cloned().unwrap_or_default();
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "containing" => {
            if args.len() != 4 && args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo containing expects root coordinates with optional -displayof",
                ));
            }
            let value = if let Some(first) = app.widgets.keys().next() {
                first.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "cells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo cells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(256).bits());
        }
        "colormapfull" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo colormapfull expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_bool(false).bits());
        }
        "depth" | "screendepth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo depth/screendepth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(24).bits());
        }
        "fpixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo fpixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo fpixels distance")?;
            let value = distance.trim().parse::<f64>().unwrap_or(0.0);
            app.last_error = None;
            return Ok(MoltObject::from_float(value).bits());
        }
        "pixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo pixels distance")?;
            let value = distance
                .trim()
                .parse::<f64>()
                .map(|v| v.round() as i64)
                .unwrap_or(0);
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "geometry" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo geometry expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let (width, height) = if path == "." {
                (200, 160)
            } else if let Some(widget) = app.widgets.get(&path) {
                (
                    widget_option_i64_default(&widget.options, "-width", 200),
                    widget_option_i64_default(&widget.options, "-height", 160),
                )
            } else {
                (0, 0)
            };
            app.last_error = None;
            return alloc_string_bits(py, &format!("{width}x{height}+0+0"));
        }
        "interps" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo interps expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                &[String::from("molt")],
                "failed to allocate winfo interps",
            );
        }
        "pathname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pathname expects window id",
                ));
            }
            let window_id = parse_i64_arg(py, handle, args[2], "winfo window id")?;
            let value = if window_id <= 1 {
                ".".to_string()
            } else if let Some(path) = app.widgets.keys().next() {
                path.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "screen" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screen expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ":0.0");
        }
        "screencells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screencells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(16_777_216).bits());
        }
        "screenmmheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(270).bits());
        }
        "screenmmwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(340).bits());
        }
        "screenvisual" | "visual" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visual/screenvisual expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "truecolor");
        }
        "server" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo server expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "MoltTk");
        }
        "visualid" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visualid expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "0x00000021");
        }
        "vrootheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "vrootwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "vrootx" | "vrooty" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootx/vrooty expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        _ => {}
    }
    app.last_error = None;
    alloc_empty_string_bits(py)
}

pub(super) fn handle_tix_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(py, handle, "tix expects a subcommand"));
    }
    let subcommand = get_string_arg(py, handle, args[1], "tix subcommand")?;
    match subcommand.as_str() {
        "addbitmapdir" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix addbitmapdir expects a directory",
                ));
            }
            let _directory = get_string_arg(py, handle, args[2], "bitmap directory")?;
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix cget expects one option",
                ));
            }
            let option_name = parse_widget_option_name_arg(py, handle, args[2], "tix option")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            option_map_query_or_empty(py, &app.tix_options, &option_name)
        }
        "configure" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            if args.len() == 2 {
                app.last_error = None;
                return option_map_to_tuple(py, &app.tix_options, "failed to allocate tix options");
            }
            if args.len() == 3 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[2], "tix option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &app.tix_options, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 2, "tix options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut app.tix_options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "filedialog" => {
            clear_last_error(handle);
            alloc_empty_string_bits(py)
        }
        "getbitmap" | "getimage" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("tix {subcommand} expects a name"),
                ));
            }
            let name = get_string_arg(py, handle, args[2], "tix image name")?;
            clear_last_error(handle);
            alloc_string_bits(py, &name)
        }
        "option" => {
            if args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tix option expects `get <name>`",
                ));
            }
            let op = get_string_arg(py, handle, args[2], "tix option operation")?;
            if op != "get" {
                clear_last_error(handle);
                return alloc_empty_string_bits(py);
            }
            let name = get_string_arg(py, handle, args[3], "tix option name")?;
            let option_name = if name.starts_with('-') {
                name
            } else {
                format!("-{name}")
            };
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            option_map_query_or_empty(py, &app.tix_options, &option_name)
        }
        "resetoptions" => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

pub(super) fn handle_tix_form_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tixForm expects a widget path or subcommand",
        ));
    }
    let first = get_string_arg(py, handle, args[1], "tixForm argument")?;
    let (subcommand, widget_path, option_start) = match first.as_str() {
        "check" | "forget" | "grid" | "info" | "slaves" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("tixForm {first} expects a widget path"),
                ));
            }
            (
                first.clone(),
                get_string_arg(py, handle, args[2], "tixForm widget path")?,
                3,
            )
        }
        _ => (
            "configure".to_string(),
            get_string_arg(py, handle, args[1], "tixForm widget path")?,
            2,
        ),
    };
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(&widget_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{widget_path}\""),
        ));
    };
    match subcommand.as_str() {
        "configure" => {
            if (args.len() - option_start).is_multiple_of(2) {
                let option_pairs = parse_widget_option_pairs(
                    py,
                    handle,
                    args,
                    option_start,
                    "tixForm configure options",
                )?;
                for (option_name, value_bits) in option_pairs {
                    value_map_set_bits(py, &mut widget.place_options, option_name, value_bits);
                }
            } else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tixForm configure expects key/value options",
                ));
            }
            widget.manager = Some("place".to_string());
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "check" | "forget" => {
            if subcommand == "forget" {
                widget.manager = None;
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "grid" => {
            if args.len() == option_start {
                app.last_error = None;
                alloc_int_tuple2_bits(py, 0, 0, "failed to allocate tixForm grid tuple")
            } else {
                app.last_error = None;
                Ok(MoltObject::none().bits())
            }
        }
        "info" => {
            if args.len() == option_start {
                app.last_error = None;
                option_map_to_tuple(py, &widget.place_options, "failed to allocate tixForm info")
            } else if args.len() == option_start + 1 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[option_start], "tixForm option")?;
                app.last_error = None;
                option_map_query_or_empty(py, &widget.place_options, &option_name)
            } else {
                Err(app_tcl_error_locked(
                    py,
                    app,
                    "tixForm info expects an optional option name",
                ))
            }
        }
        "slaves" => {
            let mut slaves: Vec<String> = app
                .widgets
                .iter()
                .filter(|(_, child)| child.manager.as_deref() == Some("place"))
                .map(|(path, _)| path.clone())
                .collect();
            slaves.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, slaves.as_slice(), "failed to allocate tixForm slaves")
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad tixForm option \"{subcommand}\""),
        )),
    }
}

pub(super) fn handle_tix_set_silent_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tixSetSilent expects widget path and value",
        ));
    }
    let _widget_path = get_string_arg(py, handle, args[1], "tixSetSilent widget path")?;
    let _value = get_text_arg(py, handle, args[2], "tixSetSilent value")?;
    clear_last_error(handle);
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_option_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "option expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "option subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "option add expects pattern and value",
                ));
            }
            let pattern = get_string_arg(py, handle, args[2], "option pattern")?;
            value_map_set_bits(py, &mut app.option_db, pattern, args[3]);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "clear" => {
            clear_value_map_refs(py, &mut app.option_db);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "option get expects name and class",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "option name")?;
            let class_name = get_string_arg(py, handle, args[3], "option class")?;
            if let Some(bits) = app.option_db.get(&name).copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(bits);
            }
            if let Some(bits) = app.option_db.get(&class_name).copied() {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(bits);
            }
            app.last_error = None;
            alloc_empty_string_bits(py)
        }
        "readfile" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad option option \"{subcommand}\": must be add, clear, get, or readfile"),
        )),
    }
}

pub(super) fn handle_send_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "send expects an interpreter and script",
        ));
    }
    clear_last_error(handle);
    alloc_empty_string_bits(py)
}

pub(super) fn handle_tk_global_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.is_empty() {
        return Err(raise_tcl_for_handle(py, handle, "empty tk global command"));
    }
    let command = get_string_arg(py, handle, args[0], "tk global command")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match command.as_str() {
        "tk_strictMotif" => {
            if args.len() == 1 {
                app.last_error = None;
                return Ok(MoltObject::from_bool(app.strict_motif).bits());
            }
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "tk_strictMotif expects optional boolean",
                ));
            }
            app.strict_motif = parse_bool_arg(py, handle, args[1], "tk_strictMotif value")?;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "tk_bisque" | "tk_setPalette" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("unknown tk command \"{command}\""),
        )),
    }
}

pub(super) fn handle_rename_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "rename expects exactly old/new command names",
        ));
    }
    let old_name = get_string_arg(py, handle, args[1], "rename old command name")?;
    let new_name = get_string_arg(py, handle, args[2], "rename new command name")?;

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(callback_bits) = app.callbacks.remove(&old_name) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("invalid command name \"{old_name}\""),
        ));
    };
    if new_name.is_empty() {
        dec_ref_bits(py, callback_bits);
        app.last_error = None;
        return Ok(MoltObject::none().bits());
    }
    if let Some(old_bits) = app.callbacks.insert(new_name, callback_bits) {
        dec_ref_bits(py, old_bits);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}
