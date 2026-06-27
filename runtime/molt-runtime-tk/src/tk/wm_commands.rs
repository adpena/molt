use super::args::{get_string_arg, raise_tcl_for_handle};
use super::parsing::{
    alloc_int_tuple2_bits, alloc_tuple_from_strings, option_map_query_or_empty,
    option_map_to_tuple, parse_bool_arg, parse_i64_arg, parse_widget_option_name_arg,
    parse_widget_option_pairs,
};
use super::state::{
    TkWmState, alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry,
    value_map_set_bits, wm_state_for_path,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::get;
use super::widgets::common::alloc_empty_string_bits;
use molt_runtime_core::prelude::{MoltObject, PyToken};

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
