use super::*;

pub(super) fn normalize_trace_mode_name(mode_name: &str) -> Result<String, String> {
    let mut has_array = false;
    let mut has_read = false;
    let mut has_write = false;
    let mut has_unset = false;
    let mut saw_token = false;
    for token in mode_name
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
    {
        saw_token = true;
        match token.to_ascii_lowercase().as_str() {
            "array" => has_array = true,
            "read" | "r" => has_read = true,
            "write" | "w" => has_write = true,
            "unset" | "u" => has_unset = true,
            _ => {
                return Err(format!(
                    "bad operation \"{token}\": must be array, read, unset, or write"
                ));
            }
        }
    }
    if !saw_token {
        return Err(format!(
            "bad operation \"{mode_name}\": must be array, read, unset, or write"
        ));
    }
    let mut normalized = Vec::with_capacity(4);
    if has_array {
        normalized.push("array");
    }
    if has_read {
        normalized.push("read");
    }
    if has_write {
        normalized.push("write");
    }
    if has_unset {
        normalized.push("unset");
    }
    Ok(normalized.join(" "))
}

pub(super) fn trace_mode_matches(mode_name: &str, op: &str) -> bool {
    mode_name
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .any(|part| part == op)
}

pub(super) fn split_array_variable_reference(variable_name: &str) -> (String, Option<String>) {
    let Some(open_idx) = variable_name.find('(') else {
        return (variable_name.to_string(), None);
    };
    if open_idx == 0 || !variable_name.ends_with(')') {
        return (variable_name.to_string(), None);
    }
    let close_idx = variable_name.len().saturating_sub(1);
    if open_idx + 1 > close_idx {
        return (variable_name.to_string(), None);
    }
    let base = variable_name[..open_idx].to_string();
    let index_text = variable_name[open_idx + 1..close_idx].to_string();
    if index_text.is_empty() {
        return (variable_name.to_string(), None);
    }
    (base, Some(index_text))
}

pub(super) fn collect_trace_callbacks_for_operation(
    app: &TkAppState,
    variable_name: &str,
    op: &str,
    index: Option<&str>,
) -> Vec<(String, String)> {
    let mut ordered: Vec<&TkTraceRegistration> = Vec::new();
    if let Some(registrations) = app.traces.get(variable_name) {
        ordered.extend(registrations.iter());
    }
    let (base_name, _) = split_array_variable_reference(variable_name);
    if base_name != variable_name
        && let Some(registrations) = app.traces.get(base_name.as_str())
    {
        ordered.extend(registrations.iter());
    }
    ordered.sort_by_key(|registration| registration.order);
    let mut callbacks: Vec<(String, String)> = Vec::new();
    for registration in ordered {
        if trace_mode_matches(&registration.mode_name, op) {
            callbacks.push((registration.callback_name.clone(), op.to_string()));
        } else if index.is_some() && trace_mode_matches(&registration.mode_name, "array") {
            callbacks.push((registration.callback_name.clone(), "array".to_string()));
        }
    }
    callbacks
}

pub(super) fn bump_variable_version(app: &mut TkAppState, variable_name: &str) {
    app.next_variable_version = app.next_variable_version.saturating_add(1);
    if app.next_variable_version == 0 {
        app.next_variable_version = 1;
    }
    app.variable_versions
        .insert(variable_name.to_string(), app.next_variable_version);
}

pub(super) fn bump_variable_versions_for_reference(app: &mut TkAppState, variable_name: &str) {
    bump_variable_version(app, variable_name);
    let (base_name, index) = split_array_variable_reference(variable_name);
    if index.is_some() && base_name != variable_name {
        bump_variable_version(app, &base_name);
    }
}

pub(super) fn variable_version(app: &TkAppState, variable_name: &str) -> u64 {
    app.variable_versions
        .get(variable_name)
        .copied()
        .unwrap_or_default()
}

pub(super) fn call_tk_command_from_strings(
    py: &PyToken,
    handle: i64,
    argv: &[String],
) -> Result<u64, u64> {
    let mut arg_bits = Vec::with_capacity(argv.len());
    for word in argv {
        match alloc_string_bits(py, word) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                for owned in arg_bits {
                    dec_ref_bits(py, owned);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &arg_bits);
    for owned in arg_bits {
        dec_ref_bits(py, owned);
    }
    out
}

pub(super) fn release_result_bits(py: &PyToken, result_bits: u64) {
    if !obj_from_bits(result_bits).is_none() {
        dec_ref_bits(py, result_bits);
    }
}

pub(super) fn invoke_trace_callbacks(
    py: &PyToken,
    handle: i64,
    variable_name: &str,
    index: Option<&str>,
    callbacks: &[(String, String)],
) -> Result<(), u64> {
    let index_text = index.unwrap_or("");
    for (callback_name, op_name) in callbacks {
        let mut argv = trace_callback_command_words(callback_name.as_str());
        argv.push(variable_name.to_string());
        argv.push(index_text.to_string());
        argv.push(op_name.clone());
        let out_bits = call_tk_command_from_strings(py, handle, &argv)?;
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
    }
    clear_last_error(handle);
    Ok(())
}

pub(super) fn trace_callback_command_words(callback_name: &str) -> Vec<String> {
    let parsed = parse_tcl_script_commands(callback_name);
    if parsed.len() == 1 && !parsed[0].is_empty() {
        return parsed.into_iter().next().unwrap_or_default();
    }
    vec![callback_name.to_string()]
}

pub(super) fn handle_set_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "set expects 1 or 2 arguments",
        ));
    }
    let var_name = get_string_arg(py, handle, args[1], "set variable name")?;
    let (trace_var_name, trace_index) = split_array_variable_reference(&var_name);
    let (result_bits, trace_callbacks) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        if args.len() == 2 {
            let Some(bits) = app.variables.get(&var_name).copied() else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("can't read \"{var_name}\": no such variable"),
                ));
            };
            inc_ref_bits(py, bits);
            let callbacks = collect_trace_callbacks_for_operation(
                app,
                &var_name,
                "read",
                trace_index.as_deref(),
            );
            app.last_error = None;
            (bits, callbacks)
        } else {
            let value_bits = args[2];
            inc_ref_bits(py, value_bits);
            if let Some(old_bits) = app.variables.insert(var_name.clone(), value_bits) {
                dec_ref_bits(py, old_bits);
            }
            bump_variable_versions_for_reference(app, &var_name);
            let callbacks = collect_trace_callbacks_for_operation(
                app,
                &var_name,
                "write",
                trace_index.as_deref(),
            );
            app.last_error = None;
            inc_ref_bits(py, value_bits);
            (value_bits, callbacks)
        }
    };
    if !trace_callbacks.is_empty()
        && let Err(bits) = invoke_trace_callbacks(
            py,
            handle,
            &trace_var_name,
            trace_index.as_deref(),
            &trace_callbacks,
        )
    {
        if !obj_from_bits(result_bits).is_none() {
            dec_ref_bits(py, result_bits);
        }
        return Err(bits);
    }
    Ok(result_bits)
}

pub(super) fn handle_unset_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "unset expects exactly 1 argument",
        ));
    }
    let var_name = get_string_arg(py, handle, args[1], "unset variable name")?;
    let (trace_var_name, trace_index) = split_array_variable_reference(&var_name);
    let trace_callbacks = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let had_value = if let Some(old_bits) = app.variables.remove(&var_name) {
            dec_ref_bits(py, old_bits);
            true
        } else {
            false
        };
        if had_value {
            bump_variable_versions_for_reference(app, &var_name);
        }
        let callbacks =
            collect_trace_callbacks_for_operation(app, &var_name, "unset", trace_index.as_deref());
        app.last_error = None;
        callbacks
    };
    if !trace_callbacks.is_empty()
        && let Err(bits) = invoke_trace_callbacks(
            py,
            handle,
            &trace_var_name,
            trace_index.as_deref(),
            &trace_callbacks,
        )
    {
        return Err(bits);
    }
    Ok(MoltObject::none().bits())
}

pub(super) fn alloc_trace_info(
    py: &PyToken,
    registrations: Option<&Vec<TkTraceRegistration>>,
) -> Result<u64, u64> {
    let mut info_rows = Vec::new();
    if let Some(registrations) = registrations {
        let mut ordered: Vec<&TkTraceRegistration> = registrations.iter().collect();
        ordered.sort_by_key(|registration| registration.order);
        for registration in ordered {
            let mode_bits = alloc_string_bits(py, registration.mode_name.as_str())?;
            let callback_bits = alloc_string_bits(py, registration.callback_name.as_str())?;
            let pair = [mode_bits, callback_bits];
            let row_bits =
                match alloc_tuple_bits(py, &pair, "failed to allocate trace info row tuple") {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(py, mode_bits);
                        dec_ref_bits(py, callback_bits);
                        for owned_bits in info_rows {
                            dec_ref_bits(py, owned_bits);
                        }
                        return Err(bits);
                    }
                };
            dec_ref_bits(py, mode_bits);
            dec_ref_bits(py, callback_bits);
            info_rows.push(row_bits);
        }
    }
    let out = alloc_tuple_bits(py, info_rows.as_slice(), "failed to allocate trace info");
    for bits in info_rows {
        dec_ref_bits(py, bits);
    }
    out
}

pub(super) fn handle_trace_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "trace requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "trace subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace add expects variable name, mode, and callback",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            let mode_name_raw = get_string_arg(py, handle, args[4], "trace mode")?;
            let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
                Ok(value) => value,
                Err(message) => {
                    return Err(app_tcl_error_locked(py, app, message));
                }
            };
            let callback_name = get_string_arg(py, handle, args[5], "trace callback")?;
            let registrations = app.traces.entry(variable_name).or_default();
            app.next_trace_order = app.next_trace_order.saturating_add(1);
            if app.next_trace_order == 0 {
                app.next_trace_order = 1;
            }
            registrations.push(TkTraceRegistration {
                mode_name,
                callback_name,
                order: app.next_trace_order,
            });
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "remove" => {
            if args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace remove expects variable name, mode, and callback",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            let mode_name_raw = get_string_arg(py, handle, args[4], "trace mode")?;
            let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
                Ok(value) => value,
                Err(message) => {
                    return Err(app_tcl_error_locked(py, app, message));
                }
            };
            let callback_name = get_string_arg(py, handle, args[5], "trace callback")?;
            remove_trace_registration(py, app, &variable_name, &mode_name, &callback_name);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "trace info expects variable name",
                ));
            }
            let subject = get_string_arg(py, handle, args[2], "trace subject")?;
            if subject != "variable" && subject != "array" {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad trace subject \"{subject}\": must be variable or array"),
                ));
            }
            let variable_name = get_string_arg(py, handle, args[3], "trace variable name")?;
            app.last_error = None;
            alloc_trace_info(py, app.traces.get(&variable_name))
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad trace subcommand \"{subcommand}\": must be add, remove, or info"),
        )),
    }
}
