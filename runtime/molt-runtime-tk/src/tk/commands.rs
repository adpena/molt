use super::*;

pub(super) fn invoke_callback(py: &PyToken, callback_bits: u64, args: &[u64]) -> u64 {
    if args.is_empty() {
        return unsafe { call_callable0(py, callback_bits) };
    }
    unsafe { call_callable_args(py, callback_bits, args) }
}

pub(super) fn run_event_callback(py: &PyToken, handle: i64, event: TkEvent) -> Result<(), u64> {
    match event {
        TkEvent::Callback { token } => {
            let callback_name = after_callback_name_from_token(&token);
            let callback_bits = {
                let mut registry = tk_registry().lock().unwrap();
                let app = app_mut_from_registry(py, &mut registry, handle)?;
                unregister_after_command_token(app, &token);
                app.one_shot_callbacks.remove(&callback_name);
                let Some(bits) = app.callbacks.remove(&callback_name) else {
                    app.last_error = None;
                    return Ok(());
                };
                unregister_tcl_callback_proc(app, &callback_name);
                app.last_error = None;
                bits
            };
            let out_bits = invoke_callback(py, callback_bits, &[]);
            dec_ref_bits(py, callback_bits);
            if exception_pending(py) {
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
                set_last_error(handle, "tkinter callback raised an exception");
                return Err(MoltObject::none().bits());
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            clear_last_error(handle);
            Ok(())
        }
        TkEvent::Script { token, commands } => {
            {
                let mut registry = tk_registry().lock().unwrap();
                let app = app_mut_from_registry(py, &mut registry, handle)?;
                unregister_after_command_token(app, &token);
            }
            if commands.is_empty() {
                clear_last_error(handle);
                return Ok(());
            }
            for words in commands {
                let out_bits = call_tk_command_from_strings(py, handle, &words)?;
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
            }
            clear_last_error(handle);
            Ok(())
        }
    }
}

pub(super) fn lookup_bound_callback(
    py: &PyToken,
    handle: i64,
    name: &str,
) -> Result<Option<u64>, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(bits) = app.callbacks.get(name).copied() {
        inc_ref_bits(py, bits);
        Ok(Some(bits))
    } else {
        Ok(None)
    }
}

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

pub(super) fn handle_expr_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "expr expects an expression argument",
        ));
    }
    if args.len() == 2 {
        let obj = obj_from_bits(args[1]);
        if let Some(i) = to_i64(obj) {
            clear_last_error(handle);
            return Ok(MoltObject::from_int(i).bits());
        }
        if let Some(f) = to_f64(obj) {
            clear_last_error(handle);
            return Ok(MoltObject::from_float(f).bits());
        }
    }
    let mut parts = Vec::with_capacity(args.len() - 1);
    for &bits in &args[1..] {
        let text = get_string_arg(py, handle, bits, "expr argument")?;
        parts.push(text);
    }
    let expression = parts.join(" ");
    let Some(parsed) = parse_expr_literal(&expression) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("invalid expression \"{expression}\""),
        ));
    };
    clear_last_error(handle);
    Ok(match parsed {
        TkExprLiteral::Int(i) => MoltObject::from_int(i).bits(),
        TkExprLiteral::Float(f) => MoltObject::from_float(f).bits(),
    })
}

pub(super) fn handle_loadtk_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 1 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "loadtk expects no arguments",
        ));
    }
    clear_last_error(handle);
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_after_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "after expects at least one argument",
        ));
    }

    if let Some(delay_ms) = to_i64(obj_from_bits(args[1])) {
        if delay_ms < 0 {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                "after delay must be non-negative",
            ));
        }
        if args.len() == 2 {
            let mut remaining = u64::try_from(delay_ms).unwrap_or(u64::MAX);
            while remaining > 0 {
                let _ = pump_tcl_events(py, handle, 0)?;
                let _ = dispatch_next_pending_event(py, handle)?;
                {
                    let _gil_release = GilReleaseGuard::new();
                    std::thread::sleep(Duration::from_micros(100));
                }
                remaining = remaining.saturating_sub(1);
            }
            clear_last_error(handle);
            return Ok(MoltObject::none().bits());
        }
        let mut command_words = Vec::with_capacity(args.len().saturating_sub(2));
        for &bits in &args[2..] {
            command_words.push(get_text_arg(py, handle, bits, "after script part")?);
        }
        if command_words.is_empty() {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                "after delay command form expects delay and command",
            ));
        }
        let command_name = command_words.join(" ");
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let token = next_after_token(&mut app.next_after_id);
        register_after_command_token(app, &token, &command_name, "timer");
        schedule_after_timer_token(app, &token, delay_ms);
        app.event_queue.push_back(TkEvent::Script {
            token: token.clone(),
            commands: vec![command_words],
        });
        app.last_error = None;
        return alloc_string_bits(py, &token);
    }

    let subcommand = get_string_arg(py, handle, args[1], "after subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "idle" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after idle expects a command name",
                ));
            }
            let mut command_words = Vec::with_capacity(args.len().saturating_sub(2));
            for &bits in &args[2..] {
                command_words.push(get_text_arg(py, handle, bits, "after idle script part")?);
            }
            if command_words.is_empty() {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after idle expects a command name",
                ));
            }
            let command_name = command_words.join(" ");
            let token = next_after_token(&mut app.next_after_id);
            register_after_command_token(app, &token, &command_name, "idle");
            app.event_queue.push_back(TkEvent::Script {
                token: token.clone(),
                commands: vec![command_words],
            });
            app.last_error = None;
            alloc_string_bits(py, &token)
        }
        "cancel" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after cancel expects a token or command name",
                ));
            }
            let key = get_string_arg(py, handle, args[2], "after cancel token")?;
            let mut tokens = HashSet::new();
            if app.after_command_tokens.contains_key(&key) {
                tokens.insert(key.clone());
            } else {
                tokens.extend(tokens_for_after_command(app, &key));
                if tokens.is_empty() && key.starts_with("after#") {
                    tokens.insert(key.clone());
                }
            }
            cleanup_after_tokens(py, app, &tokens);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_after_info_all(py, app);
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "after info expects optional token argument",
                ));
            }
            let token = get_string_arg(py, handle, args[2], "after info token")?;
            app.last_error = None;
            alloc_after_info_token(py, app, token.as_str())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad after option \"{subcommand}\": must be cancel, idle, or info"),
        )),
    }
}

pub(super) fn handle_update_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() == 1 {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    if args.len() == 2 {
        let mode = get_string_arg(py, handle, args[1], "update mode")?;
        if mode == "idletasks" {
            clear_last_error(handle);
            return Ok(MoltObject::none().bits());
        }
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        "update expects optional idletasks argument",
    ))
}

pub(super) fn wait_for_tk_condition<F>(py: &PyToken, handle: i64, mut done: F) -> Result<(), u64>
where
    F: FnMut(&TkAppState) -> bool,
{
    loop {
        let is_done = {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            done(app)
        };
        if is_done {
            clear_last_error(handle);
            return Ok(());
        }
        if pump_tcl_events(py, handle, 0)? {
            continue;
        }
        let progressed = dispatch_next_pending_event(py, handle)?;
        if progressed {
            continue;
        }
        {
            let _gil_release = GilReleaseGuard::new();
            std::thread::sleep(Duration::from_micros(100));
        }
    }
}

pub(super) fn tkwait_window_exists_in_app(app: &TkAppState, target: &str) -> bool {
    if target == "." {
        return true;
    }
    app.widgets.contains_key(target)
}

pub(super) fn tkwait_window_exists(registry: &TkRegistry, handle: i64, target: &str) -> bool {
    if target == "." {
        return registry.apps.contains_key(&handle);
    }
    registry
        .apps
        .get(&handle)
        .is_some_and(|app| tkwait_window_exists_in_app(app, target))
}

pub(super) fn tkwait_visibility_reached_in_app(app: &TkAppState, target: &str) -> bool {
    if target == "." {
        return app.wm.state != "withdrawn" && app.wm.state != "iconic";
    }
    app.widgets
        .get(target)
        .is_some_and(|widget| widget.manager.is_some())
}

pub(super) fn handle_tkwait_variable_target(
    py: &PyToken,
    handle: i64,
    variable_name: &str,
) -> Result<u64, u64> {
    let start_version = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        variable_version(app, variable_name)
    };
    wait_for_tk_condition(py, handle, |app| {
        variable_version(app, variable_name) != start_version
    })?;
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_tkwait_window_target(
    py: &PyToken,
    handle: i64,
    target: &str,
) -> Result<u64, u64> {
    let start_exists = {
        let registry = tk_registry().lock().unwrap();
        tkwait_window_exists(&registry, handle, target)
    };
    if !start_exists {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    wait_for_tk_condition(py, handle, |app| !tkwait_window_exists_in_app(app, target))?;
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_tkwait_visibility_target(
    py: &PyToken,
    handle: i64,
    target: &str,
) -> Result<u64, u64> {
    if target != "." {
        let exists_now = {
            let registry = tk_registry().lock().unwrap();
            tkwait_window_exists(&registry, handle, target)
        };
        if !exists_now {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                format!("bad window path name \"{target}\""),
            ));
        }
    }
    wait_for_tk_condition(py, handle, |app| {
        tkwait_visibility_reached_in_app(app, target)
    })?;
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_tkwait_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tkwait expects kind and target",
        ));
    }
    let kind = get_string_arg(py, handle, args[1], "tkwait kind")?;
    let target = get_string_arg(py, handle, args[2], "tkwait target")?;
    match kind.as_str() {
        "variable" => handle_tkwait_variable_target(py, handle, &target),
        "window" => handle_tkwait_window_target(py, handle, &target),
        "visibility" => handle_tkwait_visibility_target(py, handle, &target),
        _ => Err(raise_tcl_for_handle(
            py,
            handle,
            format!("bad tkwait kind \"{kind}\": must be variable, window, or visibility"),
        )),
    }
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
