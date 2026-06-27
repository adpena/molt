use super::args::{clear_last_error, get_string_arg, get_text_arg, raise_tcl_for_handle};
use super::parsing::{
    alloc_int_tuple2_bits, alloc_tuple_from_strings, option_map_query_or_empty,
    option_map_to_tuple, parse_widget_option_name_arg, parse_widget_option_pairs,
};
use super::state::{
    alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry, value_map_set_bits,
};
use super::widgets::common::alloc_empty_string_bits;
use molt_runtime_core::prelude::{MoltObject, PyToken};

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
