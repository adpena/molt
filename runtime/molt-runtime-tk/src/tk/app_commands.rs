use super::*;

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
