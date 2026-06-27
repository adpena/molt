use super::*;

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
