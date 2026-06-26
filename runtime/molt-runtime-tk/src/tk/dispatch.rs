use super::*;
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::widget_create::{handle_widget_create_command, is_widget_constructor_command};
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::window_commands::{
    command_is_image_instance, handle_clipboard_command, handle_focus_command,
    handle_focus_direction_command, handle_font_command, handle_geometry_command,
    handle_grab_command, handle_image_command, handle_image_instance_command, handle_option_command,
    handle_raise_or_lower_command, handle_rename_command, handle_selection_command,
    handle_send_command, handle_tix_command, handle_tix_form_command,
    handle_tix_set_silent_command, handle_tk_global_command, handle_winfo_command,
    handle_wm_command,
};

pub(super) fn handle_eval_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "eval expects a script argument",
        ));
    }
    let mut script_parts = Vec::with_capacity(args.len() - 1);
    for &bits in &args[1..] {
        script_parts.push(get_string_arg(py, handle, bits, "eval script segment")?);
    }
    let script = script_parts.join(" ");
    let commands = parse_tcl_script_commands(&script);
    if commands.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut last_out = MoltObject::none().bits();
    for words in commands {
        let out = call_tk_command_from_strings(py, handle, &words)?;
        if !obj_from_bits(last_out).is_none() {
            dec_ref_bits(py, last_out);
        }
        last_out = out;
    }
    Ok(last_out)
}

pub(super) fn handle_source_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "source expects exactly one filename argument",
        ));
    }
    let filename = get_string_arg(py, handle, args[1], "source filename")?;
    let script = std::fs::read_to_string(&filename).map_err(|err| {
        raise_tcl_for_handle(py, handle, format!("could not read source file: {err}"))
    })?;
    let commands = parse_tcl_script_commands(&script);
    if commands.is_empty() {
        clear_last_error(handle);
        return Ok(MoltObject::none().bits());
    }
    let mut last_out = MoltObject::none().bits();
    for words in commands {
        let out = call_tk_command_from_strings(py, handle, &words)?;
        if !obj_from_bits(last_out).is_none() {
            dec_ref_bits(py, last_out);
        }
        last_out = out;
    }
    Ok(last_out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn run_tcl_rename_and_sync_callbacks(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() != 3 {
        return run_tcl_command(py, handle, args);
    }
    let old_name = get_string_arg(py, handle, args[1], "rename old command name")?;
    let new_name = get_string_arg(py, handle, args[2], "rename new command name")?;
    let out = run_tcl_command(py, handle, args)?;

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(callback_bits) = app.callbacks.remove(&old_name) else {
        app.last_error = None;
        return Ok(out);
    };
    let was_one_shot = app.one_shot_callbacks.remove(&old_name);
    if new_name.is_empty() {
        dec_ref_bits(py, callback_bits);
        app.last_error = None;
        return Ok(out);
    }
    if let Some(old_bits) = app.callbacks.insert(new_name.clone(), callback_bits) {
        dec_ref_bits(py, old_bits);
    }
    if was_one_shot {
        app.one_shot_callbacks.insert(new_name);
    } else {
        app.one_shot_callbacks.remove(&new_name);
    }
    app.last_error = None;
    Ok(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn run_tcl_after_and_sync_callbacks(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    let out = run_tcl_command(py, handle, args)?;
    if args.len() != 3 {
        return Ok(out);
    }
    let Some(subcommand) = string_obj_to_owned(obj_from_bits(args[1])) else {
        return Ok(out);
    };
    if subcommand != "cancel" {
        return Ok(out);
    }
    let Some(key) = string_obj_to_owned(obj_from_bits(args[2])) else {
        return Ok(out);
    };

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let mut tokens = HashSet::new();
    if app.after_command_tokens.contains_key(&key) {
        tokens.insert(key.clone());
    } else {
        tokens.extend(tokens_for_after_command(app, &key));
        if tokens.is_empty() && key.starts_with("after#") {
            tokens.insert(key);
        }
    }
    cleanup_after_tokens(py, app, &tokens);
    app.last_error = None;
    Ok(out)
}

/// Tcl commands whose evaluation may run the event loop and therefore fire
/// registered procs (bound callbacks, `after`/`after_idle` handlers, traces).
/// After such a command, `::__molt_pending_callbacks` must be drained so the
/// Python-side callbacks actually run, matching CPython where the event loop
/// invokes the command directly.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn is_event_pumping_command(command: &str) -> bool {
    matches!(command, "update" | "tkwait" | "vwait" | "grab")
}

/// Run a Tcl command that pumps the event loop, then repeatedly drain and
/// dispatch any pending Python callbacks it queued. Draining loops because a
/// dispatched callback may itself schedule further idle work that the same
/// `update` already executed (and thus already enqueued).
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn run_tcl_command_and_drain_callbacks(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    let out = run_tcl_command(py, handle, args)?;
    // Bound the drain so a callback that re-queues itself synchronously cannot
    // wedge the call; each iteration dispatches at least one queued proc.
    for _ in 0..MAX_PENDING_CALLBACK_DRAIN_ROUNDS {
        let pending_callbacks = take_pending_tcl_callbacks(py, handle)?;
        if pending_callbacks.is_empty() {
            break;
        }
        for callback_argv in pending_callbacks {
            dispatch_named_callback_from_strings(py, handle, callback_argv)?;
        }
    }
    Ok(out)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) const MAX_PENDING_CALLBACK_DRAIN_ROUNDS: usize = 1024;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn native_loadtk_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 1 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "loadtk expects no arguments",
        ));
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if app.tk_loaded {
        app.last_error = None;
        return Ok(MoltObject::none().bits());
    }
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    match interp.eval(("package", "require", "Tk")) {
        Ok(_) => {
            app.tk_loaded = true;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        Err(err) => Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to load Tk package: {err}"),
        )),
    }
}

pub(super) fn handle_tk_popup_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 4 && args.len() != 5 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tk_popup expects menu path, x, y, and optional entry index",
        ));
    }
    let menu_path = get_string_arg(py, handle, args[1], "tk_popup menu path")?;
    let x = parse_i64_arg(py, handle, args[2], "tk_popup x")?;
    let y = parse_i64_arg(py, handle, args[3], "tk_popup y")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(&menu_path) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("bad window path name \"{menu_path}\""),
        ));
    };
    if widget.widget_command != "menu" {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("widget \"{menu_path}\" is not a menu"),
        ));
    }
    widget.menu_posted_at = Some((x, y));
    if args.len() == 5 {
        widget.menu_active_index = parse_menu_existing_index_bits(
            args[4],
            widget.menu_entries.len(),
            widget.menu_active_index,
        );
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

/// Single-lock resolution of a `tk.call` command on the native (libtcl) path.
/// One registry lock acquisition gathers everything the hot path needs:
/// whether the command names a bound Python callback, whether it names a file
/// handler, and (otherwise) the interpreter context for a direct Tcl eval. This
/// replaces the prior 3 separate lock acquisitions (lookup_bound_callback +
/// invoke_filehandler_command's lock + run_tcl_command's Phase-2 lock).
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) enum NativeDispatch {
    Callback(u64),
    FileHandler,
    TclCommand {
        api: &'static TclApi,
        interp_addr: usize,
        types: TclTypePtrs,
    },
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn resolve_native_dispatch(
    py: &PyToken,
    handle: i64,
    command: &str,
) -> Result<NativeDispatch, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(bits) = app.callbacks.get(command).copied() {
        inc_ref_bits(py, bits);
        return Ok(NativeDispatch::Callback(bits));
    }
    if app.filehandler_commands.contains_key(command) {
        return Ok(NativeDispatch::FileHandler);
    }
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    if let Err(err) = interp.ensure_owner_thread() {
        return Err(app_tcl_error_locked(py, app, err));
    }
    Ok(NativeDispatch::TclCommand {
        api: interp.api,
        interp_addr: interp.interp_addr,
        types: interp.types,
    })
}

pub(super) fn tk_call_dispatch(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.is_empty() {
        return Err(raise_tcl_for_handle(py, handle, "empty tkinter command"));
    }
    let command = get_string_arg(py, handle, args[0], "command name")?;

    #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
    {
        // Commands with their own callback-sync semantics keep their dedicated
        // paths (they re-lock as needed; they are not on the hot per-call path).
        if command == "rename" {
            return run_tcl_rename_and_sync_callbacks(py, handle, args);
        }
        if command == "after" {
            return run_tcl_after_and_sync_callbacks(py, handle, args);
        }
        if command == "loadtk" {
            return native_loadtk_command(py, handle, args);
        }
        // Single-lock resolution for the common path.
        match resolve_native_dispatch(py, handle, &command)? {
            NativeDispatch::Callback(callback_bits) => {
                let out_bits = invoke_callback(py, callback_bits, &args[1..]);
                dec_ref_bits(py, callback_bits);
                if exception_pending(py) {
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(py, out_bits);
                    }
                    set_last_error(handle, "bound tkinter command raised an exception");
                    return Err(MoltObject::none().bits());
                }
                clear_last_error(handle);
                return Ok(out_bits);
            }
            NativeDispatch::FileHandler => {
                if let Some(out_bits) = invoke_filehandler_command(py, handle, &command)? {
                    return Ok(out_bits);
                }
                // Fall through to a normal Tcl eval if the handler vanished.
            }
            NativeDispatch::TclCommand {
                api,
                interp_addr,
                types,
            } => {
                if is_event_pumping_command(&command) {
                    return run_tcl_command_and_drain_callbacks(py, handle, args);
                }
                return run_tcl_command_with_ctx(py, handle, args, api, interp_addr, types);
            }
        }
        run_tcl_command(py, handle, args)
    }

    #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
    if let Some(callback_bits) = lookup_bound_callback(py, handle, &command)? {
        let out_bits = invoke_callback(py, callback_bits, &args[1..]);
        dec_ref_bits(py, callback_bits);
        if exception_pending(py) {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            set_last_error(handle, "bound tkinter command raised an exception");
            return Err(MoltObject::none().bits());
        }
        clear_last_error(handle);
        return Ok(out_bits);
    }
    #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
    if let Some(out_bits) = invoke_filehandler_command(py, handle, &command)? {
        return Ok(out_bits);
    }

    #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
    {
        match command.as_str() {
            "tk_messageBox" | "tk_getOpenFile" | "tk_getSaveFile" | "tk_chooseDirectory"
            | "tk_chooseColor" => handle_headless_commondialog_command(py, handle, args),
            "tk_popup" => handle_tk_popup_command(py, handle, args),
            "tk_dialog" => handle_headless_tk_dialog_command(py, handle, args),
            "set" => handle_set_command(py, handle, args),
            "unset" => handle_unset_command(py, handle, args),
            "loadtk" => handle_loadtk_command(py, handle, args),
            "after" => handle_after_command(py, handle, args),
            "update" => handle_update_command(py, handle, args),
            "tkwait" => handle_tkwait_command(py, handle, args),
            "trace" => handle_trace_command(py, handle, args),
            "rename" => handle_rename_command(py, handle, args),
            "bind" => handle_bind_command(py, handle, args),
            "bindtags" => handle_bindtags_command(py, handle, args),
            "event" => handle_event_command(py, handle, args),
            "option" => handle_option_command(py, handle, args),
            "send" => handle_send_command(py, handle, args),
            "focus" => handle_focus_command(py, handle, args),
            "tk_focusNext" => handle_focus_direction_command(py, handle, args, "tk_focusNext"),
            "tk_focusPrev" => handle_focus_direction_command(py, handle, args, "tk_focusPrev"),
            "tk_strictMotif" | "tk_bisque" | "tk_setPalette" => {
                handle_tk_global_command(py, handle, args)
            }
            "tk_focusFollowsMouse" => {
                if args.len() != 1 {
                    Err(raise_tcl_for_handle(
                        py,
                        handle,
                        "tk_focusFollowsMouse expects no arguments",
                    ))
                } else {
                    clear_last_error(handle);
                    Ok(MoltObject::none().bits())
                }
            }
            "grab" => handle_grab_command(py, handle, args),
            "clipboard" => handle_clipboard_command(py, handle, args),
            "selection" => handle_selection_command(py, handle, args),
            "bell" => {
                clear_last_error(handle);
                Ok(MoltObject::none().bits())
            }
            "wm" => handle_wm_command(py, handle, args),
            "winfo" => handle_winfo_command(py, handle, args),
            "image" => handle_image_command(py, handle, args),
            "font" => handle_font_command(py, handle, args),
            "tix" => handle_tix_command(py, handle, args),
            "tixForm" => handle_tix_form_command(py, handle, args),
            "tixSetSilent" => handle_tix_set_silent_command(py, handle, args),
            "pack" => handle_geometry_command(py, handle, "pack", args),
            "grid" => handle_geometry_command(py, handle, "grid", args),
            "place" => handle_geometry_command(py, handle, "place", args),
            "raise" | "lower" => handle_raise_or_lower_command(py, handle, &command, args),
            "eval" => handle_eval_command(py, handle, args),
            "source" => handle_source_command(py, handle, args),
            "expr" => handle_expr_command(py, handle, args),
            "ttk::style" => handle_ttk_style_command(py, handle, args),
            "ttk::notebook::enableTraversal" => {
                handle_ttk_notebook_enable_traversal(py, handle, args)
            }
            _ => {
                if command.starts_with('.') {
                    return handle_widget_path_command(py, handle, &command, args);
                }
                if command_is_image_instance(py, handle, &command)? {
                    return handle_image_instance_command(py, handle, &command, args);
                }
                if args.len() >= 2
                    && is_widget_constructor_command(command.as_str())
                    && let Some(path) = string_obj_to_owned(obj_from_bits(args[1]))
                    && path.starts_with('.')
                {
                    return handle_widget_create_command(py, handle, &command, args);
                }
                Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("unknown tkinter command \"{command}\""),
                ))
            }
        }
    }
}

pub(super) fn parse_do_one_event_flags(
    py: &PyToken,
    handle: i64,
    flags_bits: u64,
) -> Result<i32, u64> {
    let flags_obj = obj_from_bits(flags_bits);
    if flags_obj.is_none() {
        return Ok(0);
    }
    let Some(raw_flags) = to_i64(flags_obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "dooneevent flags must be an integer",
        ));
    };
    let Ok(flags) = i32::try_from(raw_flags) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "dooneevent flags are out of range",
        ));
    };
    Ok(flags)
}

pub(super) fn event_token(event: &TkEvent) -> &str {
    match event {
        TkEvent::Callback { token } => token.as_str(),
        TkEvent::Script { token, .. } => token.as_str(),
    }
}

pub(super) fn event_is_idle(app: &TkAppState, token: &str) -> bool {
    app.after_command_kinds
        .get(token)
        .is_some_and(|kind| kind == "idle")
}

pub(super) fn event_is_due(app: &TkAppState, token: &str) -> bool {
    app.after_due_at_ms
        .get(token)
        .is_none_or(|due_at| *due_at <= app.after_clock_ms)
}

pub(super) fn pop_next_ready_event(app: &mut TkAppState) -> Option<TkEvent> {
    app.after_clock_ms = app.after_clock_ms.saturating_add(1);
    let mut ready_idle_index: Option<usize> = None;
    let mut ready_non_idle_index: Option<usize> = None;

    for idx in 0..app.event_queue.len() {
        let Some(event) = app.event_queue.get(idx) else {
            continue;
        };
        let token = event_token(event);
        if !event_is_due(app, token) {
            continue;
        }
        if event_is_idle(app, token) {
            if ready_idle_index.is_none() {
                ready_idle_index = Some(idx);
            }
        } else {
            ready_non_idle_index = Some(idx);
            break;
        }
    }

    if let Some(idx) = ready_non_idle_index.or(ready_idle_index) {
        return app.event_queue.remove(idx);
    }
    None
}

pub(super) fn app_has_pending_after_work(app: &TkAppState) -> bool {
    !app.event_queue.is_empty() || !app.after_due_at_ms.is_empty()
}

pub(super) fn dispatch_next_pending_event(py: &PyToken, handle: i64) -> Result<bool, u64> {
    let ready_filehandler_commands = next_ready_filehandler_commands(py, handle)?;
    for command_name in ready_filehandler_commands {
        if let Some(out_bits) = invoke_filehandler_command(py, handle, &command_name)? {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(py, out_bits);
            }
            clear_last_error(handle);
            return Ok(true);
        }
    }

    let event = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        pop_next_ready_event(app)
    };
    let Some(event) = event else {
        return Ok(false);
    };
    run_event_callback(py, handle, event)?;
    Ok(true)
}
