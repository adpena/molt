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
