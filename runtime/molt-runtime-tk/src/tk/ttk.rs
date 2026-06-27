use super::*;

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

pub(super) fn handle_treeview_widget_path_command(
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
    let Some(treeview) = widget.treeview.as_mut() else {
        return Ok(None);
    };

    match subcommand {
        "bbox" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "bbox expects item and optional column",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item")?;
            if !treeview.items.contains_key(&item_id) {
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            let visible = treeview_visible_items(treeview);
            let Some(row_index) = visible.iter().position(|candidate| candidate == &item_id) else {
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            };
            let x = if args.len() == 4 {
                let column = get_string_arg(py, handle, args[3], "treeview bbox column")?;
                let Some(offset) = parse_treeview_column_offset(&column) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("invalid column index \"{column}\""),
                    ));
                };
                offset
            } else {
                0
            };
            let y = (row_index as i64) * 20;
            let bbox = vec![
                x.to_string(),
                y.to_string(),
                "120".to_string(),
                "20".to_string(),
            ];
            app.last_error = None;
            return alloc_tuple_from_strings(py, &bbox, "failed to allocate treeview bbox")
                .map(Some);
        }
        "children" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "children expects item and optional replacement children",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item")?;
            if args.len() == 3 {
                let children = if item_id.is_empty() {
                    treeview.root_children.clone()
                } else {
                    let Some(item) = treeview.items.get(&item_id) else {
                        return Err(raise_tcl_for_handle(
                            py,
                            handle,
                            format!("item \"{item_id}\" not found"),
                        ));
                    };
                    item.children.clone()
                };
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    &children,
                    "failed to allocate treeview children tuple",
                )
                .map(Some);
            }

            let replacement = parse_treeview_item_list_arg(
                py,
                handle,
                args[3],
                "treeview replacement child item",
            )?;
            let mut replacement_seen = HashSet::new();
            for child in &replacement {
                if !treeview.items.contains_key(child) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" not found"),
                    ));
                }
                if !replacement_seen.insert(child.clone()) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" appears more than once"),
                    ));
                }
                if !item_id.is_empty() && child == &item_id {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" cannot be its own child"),
                    ));
                }
                if !item_id.is_empty() && treeview_item_is_descendant_of(treeview, &item_id, child)
                {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{child}\" is an ancestor of \"{item_id}\""),
                    ));
                }
            }

            let old_children = if item_id.is_empty() {
                std::mem::take(&mut treeview.root_children)
            } else {
                let Some(parent) = treeview.items.get_mut(&item_id) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                };
                std::mem::take(&mut parent.children)
            };
            for child in old_children {
                if let Some(item) = treeview.items.get_mut(&child) {
                    item.parent.clear();
                }
            }
            for child in &replacement {
                treeview_remove_from_parent(treeview, child);
                if let Some(item) = treeview.items.get_mut(child) {
                    item.parent = item_id.clone();
                }
            }
            if item_id.is_empty() {
                treeview.root_children = replacement;
            } else if let Some(parent) = treeview.items.get_mut(&item_id) {
                parent.children = replacement;
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "column" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "column expects a column identifier",
                ));
            }
            let column = get_string_arg(py, handle, args[2], "treeview column")?;
            let options = treeview.columns.entry(column).or_default();
            if args.len() == 4 {
                let opt = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview column option",
                )?);
                if !option_allowed(opt.as_str(), TREEVIEW_COLUMN_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{opt}\""),
                    ));
                }
                let bits = options
                    .get(&opt)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "column configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview column option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_COLUMN_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "delete" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "delete expects one or more item ids",
                ));
            }
            let mut item_ids = Vec::with_capacity(args.len() - 2);
            for &item_bits in &args[2..] {
                let item_id = get_string_arg(py, handle, item_bits, "treeview item id")?;
                if !treeview.items.contains_key(&item_id) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                }
                item_ids.push(item_id);
            }
            for item_id in item_ids {
                treeview_remove_from_parent(treeview, &item_id);
                treeview_remove_item(py, treeview, &item_id);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "detach" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "detach expects one or more item ids",
                ));
            }
            let mut item_ids = Vec::with_capacity(args.len() - 2);
            for &item_bits in &args[2..] {
                let item_id = get_string_arg(py, handle, item_bits, "treeview item id")?;
                if !treeview.items.contains_key(&item_id) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("item \"{item_id}\" not found"),
                    ));
                }
                item_ids.push(item_id);
            }
            for item_id in item_ids {
                treeview_remove_from_parent(treeview, &item_id);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "exists" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "exists expects exactly one item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            app.last_error = None;
            return Ok(Some(
                MoltObject::from_bool(treeview.items.contains_key(&item_id)).bits(),
            ));
        }
        "focus" => {
            if args.len() == 2 {
                let value = treeview.focus.clone().unwrap_or_default();
                app.last_error = None;
                return alloc_string_bits(py, &value).map(Some);
            }
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "focus expects zero or one item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            if !item_id.is_empty() && !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            treeview.focus = if item_id.is_empty() {
                None
            } else {
                Some(item_id)
            };
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "heading" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "heading expects a column identifier",
                ));
            }
            let column = get_string_arg(py, handle, args[2], "treeview heading column")?;
            let options = treeview.headings.entry(column).or_default();
            if args.len() == 4 {
                let opt = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview heading option",
                )?);
                if !option_allowed(opt.as_str(), TREEVIEW_HEADING_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{opt}\""),
                    ));
                }
                let bits = options
                    .get(&opt)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "heading configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview heading option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_HEADING_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "identify" => {
            if args.len() != 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "identify expects component, x, y",
                ));
            }
            let component = get_string_arg(py, handle, args[2], "treeview identify component")?;
            let x = parse_i64_arg(py, handle, args[3], "treeview identify x")?;
            let y = parse_i64_arg(py, handle, args[4], "treeview identify y")?;
            let hit_item = treeview_hit_item_by_y(treeview, y);
            let result = match component.as_str() {
                "row" | "item" => hit_item.clone().unwrap_or_default(),
                "column" => {
                    if x < 0 {
                        String::new()
                    } else {
                        format!("#{}", x / 120)
                    }
                }
                "region" => {
                    if y < 0 {
                        "heading".to_string()
                    } else if hit_item.is_some() {
                        "cell".to_string()
                    } else {
                        String::new()
                    }
                }
                "element" => {
                    if hit_item.is_some() {
                        "text".to_string()
                    } else {
                        String::new()
                    }
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad identify component \"{component}\": must be column, element, item, region, or row"
                        ),
                    ));
                }
            };
            app.last_error = None;
            return alloc_string_bits(py, &result).map(Some);
        }
        "index" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(py, handle, "index expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            let siblings = if item.parent.is_empty() {
                &treeview.root_children
            } else {
                let Some(parent) = treeview.items.get(&item.parent) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("parent \"{}\" not found", item.parent),
                    ));
                };
                &parent.children
            };
            let position = siblings
                .iter()
                .position(|candidate| candidate == &item_id)
                .unwrap_or(0) as i64;
            app.last_error = None;
            return Ok(Some(MoltObject::from_int(position).bits()));
        }
        "insert" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "insert expects parent and index",
                ));
            }
            let parent = get_string_arg(py, handle, args[2], "treeview parent item")?;
            let index_spec = get_string_arg(py, handle, args[3], "treeview insert index")?;
            if !parent.is_empty() && !treeview.items.contains_key(&parent) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{parent}\" not found"),
                ));
            }
            if !(args.len() - 4).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "insert options must be key/value pairs",
                ));
            }
            let mut item_id: Option<String> = None;
            let mut item_options: HashMap<String, u64> = HashMap::new();
            for idx in (4..args.len()).step_by(2) {
                let option_name = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview insert option name",
                )?);
                let value_bits = args[idx + 1];
                if option_name == "-id" {
                    item_id = Some(get_string_arg(
                        py,
                        handle,
                        value_bits,
                        "treeview inserted item id",
                    )?);
                    continue;
                }
                if !option_allowed(option_name.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    clear_value_map_refs(py, &mut item_options);
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option_name}\""),
                    ));
                }
                value_map_set_bits(py, &mut item_options, option_name, value_bits);
            }
            let resolved_item_id = if let Some(value) = item_id {
                value
            } else {
                treeview.next_auto_id = treeview.next_auto_id.saturating_add(1);
                format!("I{}", treeview.next_auto_id)
            };
            if treeview.items.contains_key(&resolved_item_id) {
                clear_value_map_refs(py, &mut item_options);
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{resolved_item_id}\" already exists"),
                ));
            }
            let sibling_len = if parent.is_empty() {
                treeview.root_children.len()
            } else {
                treeview
                    .items
                    .get(&parent)
                    .map(|item| item.children.len())
                    .unwrap_or(0)
            };
            let Some(index) = parse_treeview_index_strict(&index_spec, sibling_len) else {
                clear_value_map_refs(py, &mut item_options);
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("treeview index \"{index_spec}\" must be an integer or end"),
                ));
            };
            treeview_insert_into_parent(treeview, &parent, index, resolved_item_id.clone());
            treeview.items.insert(
                resolved_item_id.clone(),
                TkTreeviewItem {
                    parent,
                    children: Vec::new(),
                    options: item_options,
                    values: HashMap::new(),
                },
            );
            app.last_error = None;
            return alloc_string_bits(py, &resolved_item_id).map(Some);
        }
        "item" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(py, handle, "item expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get_mut(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            if args.len() == 3 {
                let mut keys: Vec<String> = item.options.keys().cloned().collect();
                keys.sort_unstable();
                let mut tuple_elems = Vec::with_capacity(keys.len() * 2);
                for key in keys {
                    let key_bits = alloc_string_bits(py, &key)?;
                    tuple_elems.push(key_bits);
                    if let Some(bits) = item.options.get(&key).copied() {
                        tuple_elems.push(bits);
                    } else {
                        tuple_elems.push(MoltObject::none().bits());
                    }
                }
                let out = alloc_tuple_bits(
                    py,
                    tuple_elems.as_slice(),
                    "failed to allocate treeview item tuple",
                );
                for bits in tuple_elems {
                    dec_ref_bits(py, bits);
                }
                app.last_error = None;
                return out.map(Some);
            }
            if args.len() == 4 {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[3],
                    "treeview item option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                let bits = item
                    .options
                    .get(&option)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "item configure expects key/value pairs",
                ));
            }
            for idx in (3..args.len()).step_by(2) {
                let option = normalize_widget_option_name(&get_string_arg(
                    py,
                    handle,
                    args[idx],
                    "treeview item option",
                )?);
                if !option_allowed(option.as_str(), TREEVIEW_ITEM_OPTIONS) {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("unknown option \"{option}\""),
                    ));
                }
                value_map_set_bits(py, &mut item.options, option, args[idx + 1]);
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "move" => {
            if args.len() != 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "move expects item, parent, index",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let parent = get_string_arg(py, handle, args[3], "treeview parent item")?;
            let index_spec = get_string_arg(py, handle, args[4], "treeview index")?;
            if !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            if !parent.is_empty() && !treeview.items.contains_key(&parent) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{parent}\" not found"),
                ));
            }
            if !parent.is_empty() && parent == item_id {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" cannot be moved under itself"),
                ));
            }
            if !parent.is_empty() && treeview_item_is_descendant_of(treeview, &parent, &item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" cannot be moved under its descendant \"{parent}\""),
                ));
            }
            treeview_remove_from_parent(treeview, &item_id);
            let sibling_len = if parent.is_empty() {
                treeview.root_children.len()
            } else {
                treeview
                    .items
                    .get(&parent)
                    .map(|item| item.children.len())
                    .unwrap_or(0)
            };
            let Some(index) = parse_treeview_index_strict(&index_spec, sibling_len) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("treeview index \"{index_spec}\" must be an integer or end"),
                ));
            };
            if let Some(item) = treeview.items.get_mut(&item_id) {
                item.parent = parent.clone();
            }
            treeview_insert_into_parent(treeview, &parent, index, item_id);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "next" | "prev" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("{subcommand} expects an item id"),
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            let siblings = if item.parent.is_empty() {
                &treeview.root_children
            } else if let Some(parent) = treeview.items.get(&item.parent) {
                &parent.children
            } else {
                &treeview.root_children
            };
            let mut result = String::new();
            if let Some(position) = siblings.iter().position(|candidate| candidate == &item_id) {
                let neighbor = if subcommand == "next" {
                    siblings.get(position + 1)
                } else if position > 0 {
                    siblings.get(position - 1)
                } else {
                    None
                };
                if let Some(item) = neighbor {
                    result = item.clone();
                }
            }
            app.last_error = None;
            return alloc_string_bits(py, &result).map(Some);
        }
        "parent" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "parent expects an item id",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            app.last_error = None;
            return alloc_string_bits(py, &item.parent).map(Some);
        }
        "see" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(py, handle, "see expects an item id"));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            if !treeview.items.contains_key(&item_id) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "selection" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    &treeview.selection,
                    "failed to allocate treeview selection tuple",
                )
                .map(Some);
            }
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "selection expects operation and optional item ids",
                ));
            }
            let op = get_string_arg(py, handle, args[2], "treeview selection operation")?;
            let mut items = Vec::new();
            if args.len() > 3 {
                items.reserve(args.len() - 3);
                for &item_bits in &args[3..] {
                    items.push(get_string_arg(
                        py,
                        handle,
                        item_bits,
                        "treeview selection item",
                    )?);
                }
            }
            if let Some(missing_item) = first_missing_treeview_item(treeview, &items) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{missing_item}\" not found"),
                ));
            }
            match op.as_str() {
                "set" => {
                    treeview.selection.clear();
                    let mut selected: HashSet<String> = HashSet::with_capacity(items.len());
                    for item in items {
                        if selected.insert(item.clone()) {
                            treeview.selection.push(item);
                        }
                    }
                }
                "add" => {
                    let mut selected: HashSet<String> =
                        treeview.selection.iter().cloned().collect();
                    for item in items {
                        if selected.insert(item.clone()) {
                            treeview.selection.push(item);
                        }
                    }
                }
                "remove" => {
                    if !items.is_empty() {
                        let remove_set: HashSet<String> = items.into_iter().collect();
                        treeview
                            .selection
                            .retain(|selected| !remove_set.contains(selected));
                    }
                }
                "toggle" => {
                    let mut selected: HashSet<String> =
                        treeview.selection.iter().cloned().collect();
                    let mut remove_set: HashSet<String> = HashSet::new();
                    let mut add_items: Vec<String> = Vec::new();
                    for item in items {
                        if selected.remove(&item) {
                            remove_set.insert(item);
                        } else {
                            selected.insert(item.clone());
                            add_items.push(item);
                        }
                    }
                    if !remove_set.is_empty() {
                        treeview
                            .selection
                            .retain(|selected| !remove_set.contains(selected));
                    }
                    treeview.selection.extend(add_items);
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad selection operation \"{op}\": must be add, remove, set, or toggle"
                        ),
                    ));
                }
            }
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "set" => {
            if args.len() < 3 || args.len() > 5 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "set expects item, optional column, and optional value",
                ));
            }
            let item_id = get_string_arg(py, handle, args[2], "treeview item id")?;
            let Some(item) = treeview.items.get_mut(&item_id) else {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("item \"{item_id}\" not found"),
                ));
            };
            if args.len() == 3 {
                app.last_error = None;
                return treeview_set_pairs_to_tuple(py, item).map(Some);
            }
            let column = get_string_arg(py, handle, args[3], "treeview column")?;
            if args.len() == 4 {
                let bits = item
                    .values
                    .get(&column)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if bits != MoltObject::none().bits() {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_string_bits(py, "").map(Some);
            }
            value_map_set_bits(py, &mut item.values, column, args[4]);
            app.last_error = None;
            return Ok(Some(MoltObject::none().bits()));
        }
        "tag" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "tag expects operation and tagname",
                ));
            }
            let tag_op = get_string_arg(py, handle, args[2], "treeview tag operation")?;
            let tagname = get_string_arg(py, handle, args[3], "treeview tag name")?;
            match tag_op.as_str() {
                "bind" => {
                    let tag_state = treeview.tags.entry(tagname).or_default();
                    if args.len() == 4 {
                        let mut sequences: Vec<String> =
                            tag_state.bindings.keys().cloned().collect();
                        sequences.sort_unstable();
                        let sequence_list = sequences.join(" ");
                        app.last_error = None;
                        return alloc_string_bits(py, &sequence_list).map(Some);
                    }
                    if args.len() == 5 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let script = tag_state
                            .bindings
                            .get(&sequence)
                            .cloned()
                            .unwrap_or_default();
                        app.last_error = None;
                        return alloc_string_bits(py, &script).map(Some);
                    }
                    if args.len() == 6 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let mut script =
                            get_string_arg(py, handle, args[5], "treeview tag bind script")?;
                        if script.starts_with('+') {
                            script = if let Some(previous) = tag_state.bindings.get(&sequence) {
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
                            tag_state.bindings.remove(&sequence);
                        } else {
                            tag_state.bindings.insert(sequence, script);
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    if args.len() == 7 {
                        let sequence =
                            get_string_arg(py, handle, args[4], "treeview tag bind sequence")?;
                        let command_name =
                            get_string_arg(py, handle, args[6], "treeview tag bind callback id")?;
                        if let Some(existing_script) = tag_state.bindings.get(&sequence).cloned() {
                            let replacement = remove_bind_script_command_invocations(
                                &existing_script,
                                &command_name,
                            );
                            if replacement.is_empty() {
                                tag_state.bindings.remove(&sequence);
                            } else {
                                tag_state.bindings.insert(sequence, replacement);
                            }
                        }
                        app.last_error = None;
                        return Ok(Some(MoltObject::none().bits()));
                    }
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tag bind expects tagname, optional sequence, optional script",
                    ));
                }
                "configure" => {
                    let tag_state = treeview.tags.entry(tagname).or_default();
                    if args.len() == 4 {
                        app.last_error = None;
                        return option_map_to_tuple(
                            py,
                            &tag_state.options,
                            "failed to allocate treeview tag option tuple",
                        )
                        .map(Some);
                    }
                    if args.len() == 5 {
                        let option = parse_widget_option_name_arg(
                            py,
                            handle,
                            args[4],
                            "treeview tag configure option",
                        )?;
                        if !option_allowed(option.as_str(), TREEVIEW_TAG_OPTIONS) {
                            return Err(raise_tcl_for_handle(
                                py,
                                handle,
                                format!("unknown option \"{option}\""),
                            ));
                        }
                        let bits = tag_state
                            .options
                            .get(&option)
                            .copied()
                            .unwrap_or_else(|| MoltObject::none().bits());
                        if bits != MoltObject::none().bits() {
                            inc_ref_bits(py, bits);
                            app.last_error = None;
                            return Ok(Some(bits));
                        }
                        app.last_error = None;
                        return alloc_string_bits(py, "").map(Some);
                    }
                    if !(args.len() - 4).is_multiple_of(2) {
                        return Err(raise_tcl_for_handle(
                            py,
                            handle,
                            "tag configure expects key/value pairs",
                        ));
                    }
                    for idx in (4..args.len()).step_by(2) {
                        let option = parse_widget_option_name_arg(
                            py,
                            handle,
                            args[idx],
                            "treeview tag option",
                        )?;
                        if !option_allowed(option.as_str(), TREEVIEW_TAG_OPTIONS) {
                            return Err(raise_tcl_for_handle(
                                py,
                                handle,
                                format!("unknown option \"{option}\""),
                            ));
                        }
                        value_map_set_bits(py, &mut tag_state.options, option, args[idx + 1]);
                    }
                    app.last_error = None;
                    return Ok(Some(MoltObject::none().bits()));
                }
                "has" => {
                    if args.len() == 4 {
                        let mut item_ids: Vec<String> = treeview
                            .items
                            .iter()
                            .filter_map(|(item_id, item)| {
                                parse_treeview_tags(item)
                                    .iter()
                                    .any(|tag| tag == &tagname)
                                    .then_some(item_id.clone())
                            })
                            .collect();
                        item_ids.sort_unstable();
                        app.last_error = None;
                        return alloc_tuple_from_strings(
                            py,
                            &item_ids,
                            "failed to allocate treeview tag has tuple",
                        )
                        .map(Some);
                    }
                    if args.len() == 5 {
                        let item_id = get_string_arg(py, handle, args[4], "treeview tag has item")?;
                        let has_tag = treeview.items.get(&item_id).is_some_and(|item| {
                            parse_treeview_tags(item).iter().any(|tag| tag == &tagname)
                        });
                        app.last_error = None;
                        return Ok(Some(MoltObject::from_bool(has_tag).bits()));
                    }
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tag has expects tagname and optional item",
                    ));
                }
                _ => {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!(
                            "bad treeview tag operation \"{tag_op}\": must be bind, configure, or has"
                        ),
                    ));
                }
            }
        }
        "configure" | "cget" | "destroy" | "state" | "instate" | "xview" | "yview" => {}
        _ => {
            return Err(app_tcl_error_locked(
                py,
                app,
                unknown_widget_subcommand_message(widget_path, subcommand),
            ));
        }
    }
    Ok(None)
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
