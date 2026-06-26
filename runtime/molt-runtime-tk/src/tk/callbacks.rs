use super::*;

pub(super) fn next_after_token(next_after_id: &mut u64) -> String {
    *next_after_id = next_after_id.saturating_add(1);
    format!("after#{}", *next_after_id)
}

pub(super) fn after_callback_name_from_token(token: &str) -> String {
    let suffix = token.strip_prefix("after#").unwrap_or(token);
    format!("::__molt_after_callback_{suffix}")
}

pub(super) fn next_callback_command_name(app: &mut TkAppState, prefix: &str) -> String {
    loop {
        app.next_callback_command_id = app.next_callback_command_id.saturating_add(1);
        if app.next_callback_command_id == 0 {
            app.next_callback_command_id = 1;
        }
        let command_name = format!("::__molt_{prefix}_{}", app.next_callback_command_id);
        if !app.callbacks.contains_key(&command_name)
            && !app.filehandler_commands.contains_key(&command_name)
        {
            return command_name;
        }
    }
}

pub(super) fn callback_is_callable(callback_bits: u64) -> bool {
    // Use the shared bridge's decode-free callability oracle (single source of
    // truth shared with Python `callable()`). The previous
    // `to_i64(molt_is_callable(..)) == Some(1)` decode was wrong:
    // `molt_is_callable` returns a Python `bool` (TAG_BOOL) and `as_int()`
    // rejects bools, so the check was always `false` for genuine callables —
    // the root cause of `bind callback must be callable` on plain functions.
    is_callable_bits(callback_bits)
}

pub(super) fn register_callback_command(
    py: &PyToken,
    app: &mut TkAppState,
    command_name: &str,
    callback_bits: u64,
    callback_label: &str,
) -> Result<(), u64> {
    if app.callbacks.contains_key(command_name)
        || app.filehandler_commands.contains_key(command_name)
    {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("{callback_label} name collision for \"{command_name}\""),
        ));
    }
    if let Err(err) = register_tcl_callback_proc(app, command_name) {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to register {callback_label} \"{command_name}\": {err}"),
        ));
    }
    inc_ref_bits(py, callback_bits);
    if let Some(old_bits) = app
        .callbacks
        .insert(command_name.to_string(), callback_bits)
    {
        dec_ref_bits(py, old_bits);
    }
    app.one_shot_callbacks.remove(command_name);
    Ok(())
}

pub(super) fn unregister_callback_command(py: &PyToken, app: &mut TkAppState, command_name: &str) {
    app.one_shot_callbacks.remove(command_name);
    unregister_tcl_callback_proc(app, command_name);
    if let Some(callback_bits) = app.callbacks.remove(command_name) {
        dec_ref_bits(py, callback_bits);
    }
}

pub(super) fn trace_callback_is_referenced(app: &TkAppState, callback_name: &str) -> bool {
    app.traces.values().any(|registrations| {
        registrations
            .iter()
            .any(|registration| registration.callback_name == callback_name)
    })
}

pub(super) fn remove_trace_registration(
    py: &PyToken,
    app: &mut TkAppState,
    variable_name: &str,
    mode_name: &str,
    callback_name: &str,
) {
    if let Some(registrations) = app.traces.get_mut(variable_name) {
        registrations.retain(|registration| {
            !(registration.mode_name == mode_name && registration.callback_name == callback_name)
        });
        if registrations.is_empty() {
            app.traces.remove(variable_name);
        }
    }
    if !trace_callback_is_referenced(app, callback_name) {
        unregister_callback_command(py, app, callback_name);
    }
}

pub(super) fn clear_trace_registrations_for_variable(
    py: &PyToken,
    app: &mut TkAppState,
    variable_name: &str,
) {
    let Some(registrations) = app.traces.remove(variable_name) else {
        return;
    };
    let callbacks: HashSet<String> = registrations
        .into_iter()
        .map(|registration| registration.callback_name)
        .collect();
    for callback_name in callbacks {
        if !trace_callback_is_referenced(app, callback_name.as_str()) {
            unregister_callback_command(py, app, callback_name.as_str());
        }
    }
}

pub(super) fn normalize_bind_add_prefix(py: &PyToken, add_bits: u64) -> Result<String, u64> {
    let add_obj = obj_from_bits(add_bits);
    if add_obj.is_none() {
        return Ok(String::new());
    }
    if add_obj.is_bool() {
        return Ok(if add_obj.as_bool().unwrap_or(false) {
            "+".to_string()
        } else {
            String::new()
        });
    }
    if let Some(value) = to_i64(add_obj) {
        return match value {
            0 => Ok(String::new()),
            1 => Ok("+".to_string()),
            _ => Err(raise_exception_u64(
                py,
                "TypeError",
                "bind add must be one of: None, '', False, True, or '+'",
            )),
        };
    }
    if let Some(value) = string_obj_to_owned(add_obj) {
        return match value.as_str() {
            "" => Ok(String::new()),
            "+" => Ok("+".to_string()),
            _ => Err(raise_exception_u64(
                py,
                "TypeError",
                "bind add must be one of: None, '', False, True, or '+'",
            )),
        };
    }
    Err(raise_exception_u64(
        py,
        "TypeError",
        "bind add must be one of: None, '', False, True, or '+'",
    ))
}

pub(super) fn register_after_command_token(
    app: &mut TkAppState,
    token: &str,
    command_name: &str,
    kind: &str,
) {
    app.after_command_tokens
        .insert(token.to_string(), command_name.to_string());
    app.after_command_kinds
        .insert(token.to_string(), kind.to_string());
}

pub(super) fn unregister_after_command_token(app: &mut TkAppState, token: &str) {
    app.after_command_tokens.remove(token);
    app.after_command_kinds.remove(token);
    app.after_due_at_ms.remove(token);
}

pub(super) fn lookup_after_command_for_token(app: &TkAppState, token: &str) -> Option<String> {
    app.after_command_tokens.get(token).cloned()
}

pub(super) fn lookup_after_kind_for_token(app: &TkAppState, token: &str) -> Option<String> {
    app.after_command_kinds.get(token).cloned()
}

pub(super) fn parse_after_token_id(token: &str) -> Option<u64> {
    token.strip_prefix("after#")?.parse::<u64>().ok()
}

pub(super) fn sort_after_info_tokens(tokens: &mut [String]) {
    tokens.sort_by(
        |left, right| match (parse_after_token_id(left), parse_after_token_id(right)) {
            (Some(left_id), Some(right_id)) => right_id.cmp(&left_id).then_with(|| left.cmp(right)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    );
}

pub(super) fn alloc_after_info_all(py: &PyToken, app: &TkAppState) -> Result<u64, u64> {
    let mut tokens: Vec<String> = app.after_command_tokens.keys().cloned().collect();
    sort_after_info_tokens(&mut tokens);
    alloc_tuple_from_strings(py, tokens.as_slice(), "failed to allocate after info tuple")
}

pub(super) fn alloc_after_info_token(
    py: &PyToken,
    app: &mut TkAppState,
    token: &str,
) -> Result<u64, u64> {
    let Some(command_name) = lookup_after_command_for_token(app, token) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("event \"{token}\" doesn't exist"),
        ));
    };
    let kind = lookup_after_kind_for_token(app, token).unwrap_or_else(|| {
        if command_name.starts_with("::__molt_after_callback_") {
            "timer".to_string()
        } else {
            "idle".to_string()
        }
    });
    let info = [command_name, kind];
    alloc_tuple_from_strings(py, &info, "failed to allocate after info token tuple")
}

pub(super) fn remove_after_events_for_tokens(app: &mut TkAppState, tokens: &HashSet<String>) {
    app.event_queue.retain(|event| match event {
        TkEvent::Callback { token } => !tokens.contains(token),
        TkEvent::Script { token, .. } => !tokens.contains(token),
    });
}

pub(super) fn schedule_after_timer_token(app: &mut TkAppState, token: &str, delay_ms: i64) {
    if delay_ms <= 0 {
        app.after_due_at_ms
            .insert(token.to_string(), app.after_clock_ms);
        return;
    }
    let delay = u64::try_from(delay_ms).unwrap_or(u64::MAX);
    let due_at = app.after_clock_ms.saturating_add(delay);
    app.after_due_at_ms.insert(token.to_string(), due_at);
}

pub(super) fn cleanup_after_tokens(py: &PyToken, app: &mut TkAppState, tokens: &HashSet<String>) {
    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    if let Some(interp) = app.interpreter.as_ref() {
        for token in tokens {
            let _ = interp.eval(("after", "cancel", token.clone()));
        }
    }
    for token in tokens {
        let command_name = lookup_after_command_for_token(app, token);
        unregister_after_command_token(app, token);
        let internal_name = command_name
            .clone()
            .unwrap_or_else(|| after_callback_name_from_token(token));
        if internal_name.starts_with("::__molt_after_callback_") {
            app.one_shot_callbacks.remove(&internal_name);
            if let Some(bits) = app.callbacks.remove(&internal_name) {
                dec_ref_bits(py, bits);
            }
            unregister_tcl_callback_proc(app, &internal_name);
        }
    }
    remove_after_events_for_tokens(app, tokens);
}

pub(super) fn tokens_for_after_command(app: &TkAppState, command_name: &str) -> HashSet<String> {
    app.after_command_tokens
        .iter()
        .filter_map(|(token, mapped)| (mapped == command_name).then_some(token.clone()))
        .collect()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn filehandler_event_name(mask: i64) -> Option<&'static str> {
    match mask {
        TK_FILE_EVENT_READABLE => Some("readable"),
        TK_FILE_EVENT_WRITABLE => Some("writable"),
        TK_FILE_EVENT_EXCEPTION => Some("exception"),
        _ => None,
    }
}

pub(super) fn filehandler_command_name(fd: i64, event_name: &str) -> String {
    format!("::__molt_filehandler_{fd}_{event_name}")
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn filehandler_poll_events(registration: &TkFileHandlerRegistration) -> libc::c_short {
    let mut events: libc::c_short = 0;
    if registration.commands.contains_key(&TK_FILE_EVENT_READABLE) {
        events |= libc::POLLIN;
    }
    if registration.commands.contains_key(&TK_FILE_EVENT_WRITABLE) {
        events |= libc::POLLOUT;
    }
    if registration.commands.contains_key(&TK_FILE_EVENT_EXCEPTION) {
        events |= libc::POLLPRI;
    }
    events
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn filehandler_revents_to_mask(revents: libc::c_short) -> i64 {
    let mut mask = 0_i64;
    if (revents & libc::POLLIN) != 0 || (revents & libc::POLLHUP) != 0 {
        mask |= TK_FILE_EVENT_READABLE;
    }
    if (revents & libc::POLLOUT) != 0 {
        mask |= TK_FILE_EVENT_WRITABLE;
    }
    if (revents & libc::POLLPRI) != 0
        || (revents & libc::POLLERR) != 0
        || (revents & libc::POLLNVAL) != 0
    {
        mask |= TK_FILE_EVENT_EXCEPTION;
    }
    mask
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn next_ready_filehandler_commands(
    py: &PyToken,
    handle: i64,
) -> Result<Vec<String>, u64> {
    let mut pollfds: Vec<libc::pollfd> = Vec::new();
    let mut fd_commands: Vec<Vec<(i64, String)>> = Vec::new();
    {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        for (fd, registration) in &app.filehandlers {
            let Ok(fd_native) = libc::c_int::try_from(*fd) else {
                continue;
            };
            let events = filehandler_poll_events(registration);
            if events == 0 {
                continue;
            }
            let mut commands: Vec<(i64, String)> = registration
                .commands
                .iter()
                .map(|(mask, command_name)| (*mask, command_name.clone()))
                .collect();
            commands.sort_unstable_by(|left, right| left.1.cmp(&right.1));
            pollfds.push(libc::pollfd {
                fd: fd_native,
                events,
                revents: 0,
            });
            fd_commands.push(commands);
        }
    }

    if pollfds.is_empty() {
        return Ok(Vec::new());
    }

    let poll_out = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, 0) };
    if poll_out <= 0 {
        return Ok(Vec::new());
    }

    let mut ready_commands = Vec::new();
    for (idx, pollfd) in pollfds.iter().enumerate() {
        if pollfd.revents == 0 {
            continue;
        }
        let ready_mask = filehandler_revents_to_mask(pollfd.revents);
        if ready_mask == 0 {
            continue;
        }
        for (mask, command_name) in &fd_commands[idx] {
            if (ready_mask & *mask) != 0 {
                ready_commands.push(command_name.clone());
            }
        }
    }
    ready_commands.sort_unstable();
    ready_commands.dedup();
    Ok(ready_commands)
}

#[cfg(any(not(unix), target_arch = "wasm32", feature = "native-tcl"))]
pub(super) fn next_ready_filehandler_commands(
    _py: &PyToken,
    _handle: i64,
) -> Result<Vec<String>, u64> {
    Ok(Vec::new())
}

pub(super) fn clear_filehandler_registration_locked(
    py: &PyToken,
    app: &mut TkAppState,
    fd: i64,
) -> Result<(), u64> {
    let Some(registration) = app.filehandlers.remove(&fd) else {
        return Ok(());
    };
    for (&mask, command_name) in &registration.commands {
        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        let _ = mask;
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        if let Some(event_name) = filehandler_event_name(mask) {
            let clear_result = app_interp_eval_list(
                py,
                app,
                vec![
                    "fileevent".to_string(),
                    fd.to_string(),
                    event_name.to_string(),
                    String::new(),
                ],
            );
            if let Err(bits) = clear_result {
                dec_ref_bits(py, registration.callback_bits);
                dec_ref_bits(py, registration.file_obj_bits);
                return Err(bits);
            }
            unregister_tcl_callback_proc(app, command_name);
        }
        app.filehandler_commands.remove(command_name);
    }
    dec_ref_bits(py, registration.callback_bits);
    dec_ref_bits(py, registration.file_obj_bits);
    Ok(())
}

pub(super) fn rollback_filehandler_registration_locked(
    py: &PyToken,
    app: &mut TkAppState,
    _fd: i64,
    registration: &mut TkFileHandlerRegistration,
) {
    let installed_commands: Vec<(i64, String)> = registration.commands.drain().collect();
    for (mask, command_name) in installed_commands {
        app.filehandler_commands.remove(&command_name);
        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        let _ = mask;
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        if let Some(event_name) = filehandler_event_name(mask) {
            let _ = app_interp_eval_list(
                py,
                app,
                vec![
                    "fileevent".to_string(),
                    _fd.to_string(),
                    event_name.to_string(),
                    String::new(),
                ],
            );
            unregister_tcl_callback_proc(app, &command_name);
        }
    }
    dec_ref_bits(py, registration.callback_bits);
    dec_ref_bits(py, registration.file_obj_bits);
}

pub(super) fn invoke_filehandler_command(
    py: &PyToken,
    handle: i64,
    command_name: &str,
) -> Result<Option<u64>, u64> {
    let (callback_bits, file_obj_bits, mask) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(command) = app.filehandler_commands.get(command_name).copied() else {
            return Ok(None);
        };
        let Some(registration) = app.filehandlers.get(&command.fd) else {
            return Ok(None);
        };
        inc_ref_bits(py, registration.callback_bits);
        inc_ref_bits(py, registration.file_obj_bits);
        (
            registration.callback_bits,
            registration.file_obj_bits,
            command.mask,
        )
    };

    let out_bits = invoke_callback(
        py,
        callback_bits,
        &[file_obj_bits, MoltObject::from_int(mask).bits()],
    );
    dec_ref_bits(py, callback_bits);
    dec_ref_bits(py, file_obj_bits);
    if exception_pending(py) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        set_last_error(handle, "tkinter filehandler callback raised an exception");
        return Err(MoltObject::none().bits());
    }
    clear_last_error(handle);
    Ok(Some(out_bits))
}
