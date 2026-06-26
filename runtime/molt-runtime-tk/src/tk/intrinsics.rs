use super::*;

pub extern "C" fn molt_tk_available() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let gate = tk_gate_state(_py, TkOperation::AvailabilityProbe);
        let available = !gate.wasm_unsupported && !gate.backend_unimplemented;
        MoltObject::from_bool(available).bits()
    })
}
pub extern "C" fn molt_tk_app_new(_options_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        let use_tk = option_use_tk(_py, _options_bits);
        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        let use_tk = true;
        if let Err(bits) = require_tk_app_new(_py, use_tk) {
            return bits;
        }
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        let app_state = {
            match build_native_tk_app(_py, use_tk) {
                Ok(app) => app,
                Err(bits) => return bits,
            }
        };
        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        let app_state = TkAppState::default();
        let mut registry = tk_registry().lock().unwrap();
        let mut handle = registry.next_handle;
        while handle <= 0 || registry.apps.contains_key(&handle) {
            handle = if handle == i64::MAX { 1 } else { handle + 1 };
        }
        registry.next_handle = if handle == i64::MAX { 1 } else { handle + 1 };
        registry.apps.insert(handle, app_state);
        MoltObject::from_int(handle).bits()
    })
}
pub extern "C" fn molt_tk_quit(app_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Quit) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Some(app) = registry.apps.get_mut(&handle) else {
            return raise_invalid_handle_error(_py);
        };
        app.quit_requested = true;
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_mainloop(app_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Mainloop) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        loop {
            let should_exit = {
                let mut registry = tk_registry().lock().unwrap();
                let Some(app) = registry.apps.get_mut(&handle) else {
                    return MoltObject::none().bits();
                };
                app.quit_requested
            };
            if should_exit {
                let mut registry = tk_registry().lock().unwrap();
                if let Some(app) = registry.apps.get_mut(&handle) {
                    app.quit_requested = false;
                    app.last_error = None;
                }
                return MoltObject::none().bits();
            }
            let pumped = match pump_tcl_events(_py, handle, 0) {
                Ok(pumped) => pumped,
                Err(bits) => return bits,
            };
            if pumped {
                continue;
            }
            let processed = match dispatch_next_pending_event(_py, handle) {
                Ok(processed) => processed,
                Err(bits) => return bits,
            };
            if processed {
                continue;
            }
            let has_pending = {
                let mut registry = tk_registry().lock().unwrap();
                let Some(app) = registry.apps.get_mut(&handle) else {
                    return MoltObject::none().bits();
                };
                app_has_pending_after_work(app)
            };
            if has_pending {
                {
                    let _gil_release = GilReleaseGuard::new();
                    std::thread::sleep(Duration::from_micros(100));
                }
                continue;
            }
            clear_last_error(handle);
            return MoltObject::none().bits();
        }
    })
}
pub extern "C" fn molt_tk_do_one_event(app_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DoOneEvent) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let flags = match parse_do_one_event_flags(_py, handle, flags_bits) {
            Ok(flags) => flags,
            Err(bits) => return bits,
        };
        let pumped = match pump_tcl_events(_py, handle, flags) {
            Ok(pumped) => pumped,
            Err(bits) => return bits,
        };
        if pumped {
            clear_last_error(handle);
            return MoltObject::from_bool(true).bits();
        }
        let processed = match dispatch_next_pending_event(_py, handle) {
            Ok(processed) => processed,
            Err(bits) => return bits,
        };
        if processed {
            clear_last_error(handle);
            return MoltObject::from_bool(true).bits();
        }
        let dont_wait = (flags & TK_DONT_WAIT_FLAG) != 0;
        if !dont_wait {
            loop {
                let has_pending = {
                    let mut registry = tk_registry().lock().unwrap();
                    let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                        return raise_invalid_handle_error(_py);
                    };
                    app_has_pending_after_work(app)
                };
                if !has_pending {
                    break;
                }
                {
                    let _gil_release = GilReleaseGuard::new();
                    std::thread::sleep(Duration::from_micros(100));
                }
                let progressed = match dispatch_next_pending_event(_py, handle) {
                    Ok(progressed) => progressed,
                    Err(bits) => return bits,
                };
                if progressed {
                    clear_last_error(handle);
                    return MoltObject::from_bool(true).bits();
                }
            }
        }
        clear_last_error(handle);
        MoltObject::from_bool(false).bits()
    })
}
pub extern "C" fn molt_tk_after(app_bits: u64, delay_ms_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::After) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(delay_ms) = to_i64(obj_from_bits(delay_ms_bits)) else {
            return raise_tcl_for_handle(_py, handle, "after delay must be an integer");
        };
        if delay_ms < 0 {
            return raise_tcl_for_handle(_py, handle, "after delay must be non-negative");
        }
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let token = next_after_token(&mut app.next_after_id);
        let callback_name = after_callback_name_from_token(&token);

        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(callback_name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.insert(callback_name.clone());

        if let Err(err) = register_tcl_callback_proc(app, &callback_name) {
            app.one_shot_callbacks.remove(&callback_name);
            if let Some(bits) = app.callbacks.remove(&callback_name) {
                dec_ref_bits(_py, bits);
            }
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter callback command \"{callback_name}\": {err}"),
            );
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                unregister_tcl_callback_proc(app, &callback_name);
                app.one_shot_callbacks.remove(&callback_name);
                if let Some(bits) = app.callbacks.remove(&callback_name) {
                    dec_ref_bits(_py, bits);
                }
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            let api = interp.api;
            let interp_addr = interp.interp_addr;
            let cb_name_clone = callback_name.clone();
            drop(registry);
            // Release GIL during Tcl "after" scheduling eval.
            let after_cmd = [
                TclObj::from("after"),
                TclObj::from(delay_ms),
                TclObj::from(cb_name_clone),
            ];
            let after_token = match eval_tcl_without_gil(api, interp_addr, &after_cmd) {
                Ok(value) => value,
                Err(err) => {
                    let mut registry = tk_registry().lock().unwrap();
                    if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                        unregister_tcl_callback_proc(app, &callback_name);
                        app.one_shot_callbacks.remove(&callback_name);
                        if let Some(bits) = app.callbacks.remove(&callback_name) {
                            dec_ref_bits(_py, bits);
                        }
                        return app_tcl_error_locked(_py, app, format!("tk command failed: {err}"));
                    }
                    return raise_invalid_handle_error(_py);
                }
            };
            {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    register_after_command_token(app, &after_token, &callback_name, "timer");
                    app.last_error = None;
                }
            }
            return match alloc_string_bits(_py, &after_token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        {
            register_after_command_token(app, &token, &callback_name, "timer");
            schedule_after_timer_token(app, &token, delay_ms);
            app.event_queue.push_back(TkEvent::Callback {
                token: token.clone(),
            });
            app.last_error = None;
            drop(registry);
            match alloc_string_bits(_py, &token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            }
        }
    })
}
pub extern "C" fn molt_tk_after_idle(app_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterIdle) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let token = next_after_token(&mut app.next_after_id);
        let callback_name = after_callback_name_from_token(&token);

        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(callback_name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.insert(callback_name.clone());

        if let Err(err) = register_tcl_callback_proc(app, &callback_name) {
            app.one_shot_callbacks.remove(&callback_name);
            if let Some(bits) = app.callbacks.remove(&callback_name) {
                dec_ref_bits(_py, bits);
            }
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter callback command \"{callback_name}\": {err}"),
            );
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                unregister_tcl_callback_proc(app, &callback_name);
                app.one_shot_callbacks.remove(&callback_name);
                if let Some(bits) = app.callbacks.remove(&callback_name) {
                    dec_ref_bits(_py, bits);
                }
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            let api = interp.api;
            let interp_addr = interp.interp_addr;
            let cb_name_clone = callback_name.clone();
            drop(registry);
            // Release GIL during Tcl "after idle" scheduling eval.
            let after_cmd = [
                TclObj::from("after"),
                TclObj::from("idle"),
                TclObj::from(cb_name_clone),
            ];
            let after_token = match eval_tcl_without_gil(api, interp_addr, &after_cmd) {
                Ok(value) => value,
                Err(err) => {
                    let mut registry = tk_registry().lock().unwrap();
                    if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                        unregister_tcl_callback_proc(app, &callback_name);
                        app.one_shot_callbacks.remove(&callback_name);
                        if let Some(bits) = app.callbacks.remove(&callback_name) {
                            dec_ref_bits(_py, bits);
                        }
                        return app_tcl_error_locked(_py, app, format!("tk command failed: {err}"));
                    }
                    return raise_invalid_handle_error(_py);
                }
            };
            {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    register_after_command_token(app, &after_token, &callback_name, "idle");
                    app.last_error = None;
                }
            }
            return match alloc_string_bits(_py, &after_token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        {
            register_after_command_token(app, &token, &callback_name, "idle");
            app.event_queue.push_back(TkEvent::Callback {
                token: token.clone(),
            });
            app.last_error = None;
            drop(registry);
            match alloc_string_bits(_py, &token) {
                Ok(bits) => bits,
                Err(bits) => bits,
            }
        }
    })
}
pub extern "C" fn molt_tk_after_cancel(app_bits: u64, identifier_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterCancel) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let identifier_obj = obj_from_bits(identifier_bits);
        if !is_truthy(_py, identifier_obj) {
            return raise_exception_u64(
                _py,
                "ValueError",
                "id must be a valid identifier returned from after or after_idle",
            );
        }
        let key = match get_text_arg(_py, handle, identifier_bits, "after cancel token") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let mut tokens = HashSet::new();
        if app.after_command_tokens.contains_key(&key) {
            tokens.insert(key.clone());
        } else {
            tokens.extend(tokens_for_after_command(app, &key));
            if tokens.is_empty() && key.starts_with("after#") {
                tokens.insert(key);
            }
        }
        cleanup_after_tokens(_py, app, &tokens);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_after_info(app_bits: u64, identifier_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::AfterInfo) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if obj_from_bits(identifier_bits).is_none() {
            app.last_error = None;
            return match alloc_after_info_all(_py, app) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        let token = match get_text_arg(_py, handle, identifier_bits, "after info token") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        app.last_error = None;
        match alloc_after_info_token(_py, app, token.as_str()) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_call(app_bits: u64, argv_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        // The handle is validated by the single registry lock inside the dispatch
        // path (run_tcl_command / the callback+filehandler resolution); a separate
        // up-front validation lock here is redundant per-call overhead.
        let Some(args) = decode_value_list(obj_from_bits(argv_bits)) else {
            return raise_tcl_for_handle(_py, handle, "tk call argv must be a list or tuple");
        };
        match tk_call_dispatch(_py, handle, &args) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_trace_add(
    app_bits: u64,
    variable_name_bits: u64,
    mode_bits: u64,
    callback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mode_name_raw = match get_string_arg(_py, handle, mode_bits, "trace mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
            Ok(value) => value,
            Err(message) => return raise_tcl_for_handle(_py, handle, message),
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "trace callback must be callable");
        }

        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "trace_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter trace callback command",
            ) {
                return bits;
            }
            let registrations = app.traces.entry(variable_name).or_default();
            app.next_trace_order = app.next_trace_order.saturating_add(1);
            if app.next_trace_order == 0 {
                app.next_trace_order = 1;
            }
            registrations.push(TkTraceRegistration {
                mode_name,
                callback_name: command_name.clone(),
                order: app.next_trace_order,
            });
            app.last_error = None;
            command_name
        };

        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_trace_remove(
    app_bits: u64,
    variable_name_bits: u64,
    mode_bits: u64,
    cbname_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mode_name_raw = match get_string_arg(_py, handle, mode_bits, "trace mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mode_name = match normalize_trace_mode_name(&mode_name_raw) {
            Ok(value) => value,
            Err(message) => return raise_tcl_for_handle(_py, handle, message),
        };
        let callback_name = match get_string_arg(_py, handle, cbname_bits, "trace callback") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        remove_trace_registration(_py, app, &variable_name, &mode_name, &callback_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_trace_info(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        app.last_error = None;
        match alloc_trace_info(_py, app.traces.get(&variable_name)) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_trace_clear(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name =
            match get_string_arg(_py, handle, variable_name_bits, "trace variable name") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        clear_trace_registrations_for_variable(_py, app, &variable_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_tkwait_variable(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let variable_name = match get_string_arg(_py, handle, variable_name_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_variable_target(_py, handle, &variable_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_tkwait_window(app_bits: u64, target_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target = match get_string_arg(_py, handle, target_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_window_target(_py, handle, &target) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_tkwait_visibility(app_bits: u64, target_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target = match get_string_arg(_py, handle, target_bits, "tkwait target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match handle_tkwait_visibility_target(_py, handle, &target) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_bind_callback_register(
    app_bits: u64,
    target_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target_name = match get_string_arg(_py, handle, target_bits, "bind target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec!["bind".to_string(), target_name, sequence, merged_script];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_bind_callback_unregister(
    app_bits: u64,
    target_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let target_name = match get_string_arg(_py, handle, target_bits, "bind target") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = match get_string_arg(_py, handle, command_name_bits, "bind callback id")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let get_bind_argv = vec!["bind".to_string(), target_name.clone(), sequence.clone()];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec!["bind".to_string(), target_name, sequence, replacement];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_widget_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    bind_target_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bind_target = match get_string_arg(_py, handle, bind_target_bits, "widget bind target")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "widget bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "tag_bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "widget_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter widget bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec![
            widget_path,
            "bind".to_string(),
            bind_target,
            sequence,
            merged_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_widget_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    bind_target_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bind_target = match get_string_arg(_py, handle, bind_target_bits, "widget bind target")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "widget bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name =
            match get_string_arg(_py, handle, command_name_bits, "widget bind callback id") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        let get_bind_argv = vec![
            widget_path.clone(),
            "bind".to_string(),
            bind_target.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "bind".to_string(),
            bind_target,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_text_tag_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
    add_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "text widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "text tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "text tag bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "tag_bind callback must be callable");
        }
        let add_prefix = match normalize_bind_add_prefix(_py, add_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "text_tag_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter text tag bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let merged_script = if add_prefix.is_empty() {
            bind_script
        } else {
            format!("{add_prefix}{bind_script}")
        };
        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            merged_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_text_tag_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path = match get_string_arg(_py, handle, widget_path_bits, "text widget path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "text tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence = match get_string_arg(_py, handle, sequence_bits, "text tag bind sequence") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command_name =
            match get_string_arg(_py, handle, command_name_bits, "text tag bind callback id") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        let get_bind_argv = vec![
            widget_path.clone(),
            "tag".to_string(),
            "bind".to_string(),
            tagname.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_treeview_tag_bind_callback_register(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    callback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path =
            match get_string_arg(_py, handle, widget_path_bits, "treeview widget path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "treeview tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence =
            match get_string_arg(_py, handle, sequence_bits, "treeview tag bind sequence") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "tag_bind callback must be callable");
        }

        let command_name = {
            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };
            let command_name = next_callback_command_name(app, "treeview_tag_bind_callback");
            if let Err(bits) = register_callback_command(
                _py,
                app,
                &command_name,
                callback_bits,
                "tkinter treeview tag bind callback command",
            ) {
                return bits;
            }
            app.last_error = None;
            command_name
        };

        let bind_script =
            format!("if {{\"[{command_name} {TK_BIND_SUBST_FORMAT_STR}]\" == \"break\"}} break\n");
        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            bind_script,
        ];
        let bind_result = call_tk_command_from_strings(_py, handle, &set_bind_argv);
        match bind_result {
            Ok(result_bits) => {
                release_result_bits(_py, result_bits);
            }
            Err(bits) => {
                let mut registry = tk_registry().lock().unwrap();
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    unregister_callback_command(_py, app, &command_name);
                }
                return bits;
            }
        }
        match alloc_string_bits(_py, &command_name) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_treeview_tag_bind_callback_unregister(
    app_bits: u64,
    widget_path_bits: u64,
    tagname_bits: u64,
    sequence_bits: u64,
    command_name_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let widget_path =
            match get_string_arg(_py, handle, widget_path_bits, "treeview widget path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let tagname = match get_string_arg(_py, handle, tagname_bits, "treeview tag name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sequence =
            match get_string_arg(_py, handle, sequence_bits, "treeview tag bind sequence") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let command_name = match get_string_arg(
            _py,
            handle,
            command_name_bits,
            "treeview tag bind callback id",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let get_bind_argv = vec![
            widget_path.clone(),
            "tag".to_string(),
            "bind".to_string(),
            tagname.clone(),
            sequence.clone(),
        ];
        let current_script_bits = match call_tk_command_from_strings(_py, handle, &get_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let current_script =
            string_obj_to_owned(obj_from_bits(current_script_bits)).unwrap_or_default();
        release_result_bits(_py, current_script_bits);
        let replacement = remove_bind_script_command_invocations(&current_script, &command_name);

        let set_bind_argv = vec![
            widget_path,
            "tag".to_string(),
            "bind".to_string(),
            tagname,
            sequence,
            replacement,
        ];
        let set_bits = match call_tk_command_from_strings(_py, handle, &set_bind_argv) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        release_result_bits(_py, set_bits);

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        unregister_callback_command(_py, app, &command_name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_bind_command(app_bits: u64, name_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::BindCommand) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_tcl_for_handle(_py, handle, "bind command name must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(err) = register_tcl_callback_proc(app, &name) {
            return app_tcl_error_locked(
                _py,
                app,
                format!("failed to register tkinter command \"{name}\": {err}"),
            );
        }
        inc_ref_bits(_py, callback_bits);
        if let Some(old_bits) = app.callbacks.insert(name.clone(), callback_bits) {
            dec_ref_bits(_py, old_bits);
        }
        app.one_shot_callbacks.remove(&name);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_unbind_command(app_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::UnbindCommand) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_tcl_for_handle(_py, handle, "unbind command name must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Some(callback_bits) = app.callbacks.remove(&name) {
            app.one_shot_callbacks.remove(&name);
            unregister_tcl_callback_proc(app, &name);
            dec_ref_bits(_py, callback_bits);
            app.last_error = None;
            return MoltObject::none().bits();
        }
        if let Some(filehandler) = app.filehandler_commands.get(&name).copied() {
            if let Err(bits) = clear_filehandler_registration_locked(_py, app, filehandler.fd) {
                return bits;
            }
            app.last_error = None;
            return MoltObject::none().bits();
        }
        app_tcl_error_locked(_py, app, format!("invalid command name \"{name}\""))
    })
}
pub extern "C" fn molt_tk_filehandler_create(
    app_bits: u64,
    fd_bits: u64,
    mask_bits: u64,
    callback_bits: u64,
    file_obj_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileHandlerCreate) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_tcl_for_handle(_py, handle, "file descriptor must be an integer");
        };
        if fd < 0 {
            return raise_exception_u64(
                _py,
                "ValueError",
                &format!("file descriptor cannot be a negative integer ({fd})"),
            );
        }
        let Some(mask) = to_i64(obj_from_bits(mask_bits)) else {
            return raise_tcl_for_handle(_py, handle, "filehandler mask must be an integer");
        };
        if !callback_is_callable(callback_bits) {
            return raise_exception_u64(_py, "TypeError", "bad argument list");
        }

        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(bits) = clear_filehandler_registration_locked(_py, app, fd) {
            return bits;
        }

        if mask == 0 {
            app.last_error = None;
            return MoltObject::none().bits();
        }

        let mut registration = TkFileHandlerRegistration {
            callback_bits,
            file_obj_bits,
            commands: HashMap::new(),
        };
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, file_obj_bits);

        for (event_mask, event_name) in [
            (TK_FILE_EVENT_READABLE, "readable"),
            (TK_FILE_EVENT_WRITABLE, "writable"),
            (TK_FILE_EVENT_EXCEPTION, "exception"),
        ] {
            if (mask & event_mask) == 0 {
                #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
                if let Err(bits) = app_interp_eval_list(
                    _py,
                    app,
                    vec![
                        "fileevent".to_string(),
                        fd.to_string(),
                        event_name.to_string(),
                        String::new(),
                    ],
                ) {
                    rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                    return bits;
                }
                continue;
            }

            let command_name = filehandler_command_name(fd, event_name);
            if app.callbacks.contains_key(&command_name) {
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!("filehandler command name collision for \"{command_name}\""),
                );
            }
            #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
            if let Err(err) = register_tcl_callback_proc(app, &command_name) {
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!(
                        "failed to register tkinter filehandler command \"{command_name}\": {err}"
                    ),
                );
            }
            #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
            if let Err(bits) = app_interp_eval_list(
                _py,
                app,
                vec![
                    "fileevent".to_string(),
                    fd.to_string(),
                    event_name.to_string(),
                    command_name.clone(),
                ],
            ) {
                unregister_tcl_callback_proc(app, &command_name);
                rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
                return bits;
            }
            app.filehandler_commands.insert(
                command_name.clone(),
                TkFileHandlerCommand {
                    fd,
                    mask: event_mask,
                },
            );
            registration.commands.insert(event_mask, command_name);
        }

        if registration.commands.is_empty() {
            rollback_filehandler_registration_locked(_py, app, fd, &mut registration);
            app.last_error = None;
            return MoltObject::none().bits();
        }
        app.filehandlers.insert(fd, registration);
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_filehandler_delete(app_bits: u64, fd_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileHandlerDelete) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_tcl_for_handle(_py, handle, "file descriptor must be an integer");
        };
        if fd < 0 {
            return raise_exception_u64(
                _py,
                "ValueError",
                &format!("file descriptor cannot be a negative integer ({fd})"),
            );
        }
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Err(bits) = clear_filehandler_registration_locked(_py, app, fd) {
            return bits;
        }
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_destroy_widget(app_bits: u64, widget_path_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DestroyWidget) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let Some(widget_path) = string_obj_to_owned(obj_from_bits(widget_path_bits)) else {
            return raise_tcl_for_handle(_py, handle, "widget path must be str");
        };
        let mut registry = tk_registry().lock().unwrap();
        if widget_path == "." {
            let Some(mut app) = registry.apps.remove(&handle) else {
                return raise_invalid_handle_error(_py);
            };
            drop_app_state_refs(_py, &mut app);
            return MoltObject::none().bits();
        }
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        {
            let Some(interp) = app.interpreter.as_ref() else {
                return app_tcl_error_locked(_py, app, "tk runtime interpreter is unavailable");
            };
            let api = interp.api;
            let interp_addr = interp.interp_addr;
            let wp = widget_path.clone();
            drop(registry);
            // Release GIL during Tcl "destroy" command.
            let destroy_cmd = [TclObj::from("destroy"), TclObj::from(wp)];
            let destroy_result = eval_tcl_without_gil(api, interp_addr, &destroy_cmd);
            // Single registry lock acquisition for both success and error paths.
            {
                let mut registry = tk_registry().lock().unwrap();
                if let Err(err) = destroy_result {
                    let message = format!("tk command failed: {err}");
                    if let Some(app) = registry.apps.get_mut(&handle) {
                        app.last_error = Some(message.clone());
                    }
                    return raise_tcl_error(_py, &message);
                }
                if let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) {
                    if let Some(widget) = app.widgets.remove(&widget_path) {
                        clear_widget_refs(_py, widget);
                    }
                    app.last_error = None;
                }
            }
            return MoltObject::none().bits();
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        {
            let Some(widget) = app.widgets.remove(&widget_path) else {
                return app_tcl_error_locked(
                    _py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                );
            };
            clear_widget_refs(_py, widget);
            app.last_error = None;
            MoltObject::none().bits()
        }
    })
}
pub extern "C" fn molt_tk_last_error(app_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::LastError) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        if let Some(message) = app.last_error.as_deref() {
            return match alloc_string_bits(_py, message) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_getboolean(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if obj.is_bool() {
            return MoltObject::from_bool(obj.as_bool().unwrap_or(false)).bits();
        }
        if let Some(value) = to_i64(obj) {
            return MoltObject::from_bool(value != 0).bits();
        }
        if let Some(value) = to_f64(obj) {
            return MoltObject::from_bool(value != 0.0).bits();
        }
        if let Some(text) = string_obj_to_owned(obj) {
            if let Some(parsed) = parse_bool_text(&text) {
                return MoltObject::from_bool(parsed).bits();
            }
            return raise_exception_u64(
                _py,
                "ValueError",
                &format!("invalid boolean value \"{text}\""),
            );
        }
        MoltObject::from_bool(is_truthy(_py, obj)).bits()
    })
}
pub extern "C" fn molt_tk_getdouble(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if let Some(value) = to_f64(obj) {
            return MoltObject::from_float(value).bits();
        }
        if let Some(text) = string_obj_to_owned(obj)
            && let Ok(value) = text.trim().parse::<f64>()
        {
            return MoltObject::from_float(value).bits();
        }
        raise_exception_u64(
            _py,
            "ValueError",
            &format!(
                "invalid floating-point value \"{}\"",
                string_obj_to_owned(obj).unwrap_or_else(|| "?".to_string())
            ),
        )
    })
}
pub extern "C" fn molt_tk_splitlist(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        if let Some(items) = decode_value_list(obj) {
            return match alloc_tuple_bits(
                _py,
                items.as_slice(),
                "failed to allocate splitlist tuple",
            ) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        if let Some(text) = string_obj_to_owned(obj) {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return match alloc_tuple_from_strings(
                    _py,
                    &[],
                    "failed to allocate splitlist empty tuple",
                ) {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }
            let mut words = Vec::new();
            for command in parse_tcl_script_commands(trimmed) {
                words.extend(command);
            }
            return match alloc_tuple_from_strings(
                _py,
                words.as_slice(),
                "failed to allocate splitlist tuple",
            ) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        match alloc_tuple_bits(_py, &[value_bits], "failed to allocate splitlist tuple") {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_event_subst_parse(_widget_path_bits: u64, event_args_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(raw_args) = decode_value_list(obj_from_bits(event_args_bits)) else {
            return MoltObject::none().bits();
        };
        let args: Vec<u64> = raw_args.into_iter().map(flatten_event_subst_arg).collect();
        if args.len() != TK_EVENT_SUBST_FIELD_COUNT {
            return MoltObject::none().bits();
        }

        let payload = [
            normalize_event_subst_int_field(args[0]),
            normalize_event_subst_int_field(args[1]),
            normalize_event_subst_bool_field(args[2]),
            normalize_event_subst_int_field(args[3]),
            normalize_event_subst_int_field(args[4]),
            normalize_event_subst_int_field(args[5]),
            normalize_event_subst_int_field(args[6]),
            normalize_event_subst_int_field(args[7]),
            normalize_event_subst_int_field(args[8]),
            normalize_event_subst_int_field(args[9]),
            args[10],
            normalize_event_subst_bool_field(args[11]),
            args[12],
            normalize_event_subst_int_field(args[13]),
            args[14],
            args[15],
            normalize_event_subst_int_field(args[16]),
            normalize_event_subst_int_field(args[17]),
            normalize_event_subst_delta_field(args[18]),
        ];

        match alloc_tuple_bits(
            _py,
            &payload,
            "failed to allocate tkinter event substitution tuple",
        ) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_bind_script_remove_command(
    script_bits: u64,
    command_name_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(script) = string_obj_to_owned(obj_from_bits(script_bits)) else {
            return raise_exception_u64(_py, "TypeError", "bind script must be str");
        };
        let Some(command_name) = string_obj_to_owned(obj_from_bits(command_name_bits)) else {
            return raise_exception_u64(_py, "TypeError", "bind command name must be str");
        };
        let replacement = remove_bind_script_command_invocations(&script, &command_name);
        match alloc_string_bits(_py, &replacement) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_errorinfo_append(app_bits: u64, message_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::Call) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        let message = match get_string_arg(_py, handle, message_bits, "errorinfo message") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut registry = tk_registry().lock().unwrap();
        let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
            return raise_invalid_handle_error(_py);
        };
        let current = app
            .variables
            .get("errorInfo")
            .copied()
            .and_then(|bits| string_obj_to_owned(obj_from_bits(bits)))
            .unwrap_or_default();
        let merged = if current.is_empty() {
            message
        } else if message.starts_with('\n') {
            format!("{current}{message}")
        } else {
            format!("{current}\n{message}")
        };
        let merged_bits = match alloc_string_bits(_py, &merged) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if let Some(old_bits) = app.variables.insert("errorInfo".to_string(), merged_bits) {
            dec_ref_bits(_py, old_bits);
        }
        bump_variable_versions_for_reference(app, "errorInfo");
        app.last_error = None;
        MoltObject::none().bits()
    })
}
pub extern "C" fn molt_tk_dialog_show(
    app_bits: u64,
    master_path_bits: u64,
    title_bits: u64,
    text_bits: u64,
    bitmap_bits: u64,
    default_index_bits: u64,
    strings_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::DialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _master_path = match get_string_arg(_py, handle, master_path_bits, "dialog master path")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _title = match get_string_arg_allow_none(_py, handle, title_bits, "dialog title") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _text = match get_string_arg_allow_none(_py, handle, text_bits, "dialog text") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _bitmap = match get_string_arg_allow_none(_py, handle, bitmap_bits, "dialog bitmap") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(default_index) = to_i64(obj_from_bits(default_index_bits)) else {
            return raise_tcl_for_handle(_py, handle, "dialog default index must be an integer");
        };
        let Some(raw_strings) = decode_value_list(obj_from_bits(strings_bits)) else {
            return raise_tcl_for_handle(_py, handle, "dialog button strings must be a list/tuple");
        };
        let mut button_labels = Vec::with_capacity(raw_strings.len());
        for item_bits in raw_strings {
            let label = match get_string_arg(_py, handle, item_bits, "dialog button label") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            button_labels.push(label);
        }

        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        {
            let mut command = vec![
                "tk_dialog".to_string(),
                _master_path,
                _title,
                _text,
                _bitmap,
                default_index.to_string(),
            ];
            command.extend(button_labels);
            return match tk_dispatch_string_command(_py, handle, &command) {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        {
            let selected = if button_labels.is_empty() {
                0_i64
            } else {
                let mut index = default_index;
                if index < 0 {
                    index = 0;
                }
                let max = (button_labels.len() - 1) as i64;
                if index > max {
                    index = max;
                }
                index
            };
            clear_last_error(handle);
            MoltObject::from_int(selected).bits()
        }
    })
}
pub extern "C" fn molt_tk_commondialog_show(
    app_bits: u64,
    master_path_bits: u64,
    command_bits: u64,
    options_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::CommonDialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "commondialog master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command = match get_string_arg(_py, handle, command_bits, "commondialog command") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        if !commondialog_is_supported_command(command.as_str()) {
            return raise_unsupported_commondialog_command(_py, handle, command.as_str());
        }

        match dispatch_commondialog_via_tk_call(_py, handle, &_master_path, &command, &options) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_messagebox_show(
    app_bits: u64,
    master_path_bits: u64,
    options_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::MessageBoxShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }
        let master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "messagebox master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match dispatch_commondialog_via_tk_call(
            _py,
            handle,
            &master_path,
            "tk_messageBox",
            &options,
        ) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_filedialog_show(
    app_bits: u64,
    master_path_bits: u64,
    command_bits: u64,
    options_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::FileDialogShow) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }
        let master_path = match get_string_arg_allow_none(
            _py,
            handle,
            master_path_bits,
            "filedialog master path",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let command = match get_string_arg(_py, handle, command_bits, "filedialog command") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !filedialog_is_supported_command(command.as_str()) {
            return raise_unsupported_filedialog_command(_py, handle, command.as_str());
        }
        let options = match parse_commondialog_options(_py, handle, options_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match dispatch_commondialog_via_tk_call(_py, handle, &master_path, &command, &options) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
pub extern "C" fn molt_tk_simpledialog_query(
    app_bits: u64,
    parent_path_bits: u64,
    title_bits: u64,
    prompt_bits: u64,
    initial_value_bits: u64,
    query_kind_bits: u64,
    min_value_bits: u64,
    max_value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Err(bits) = require_tk_operation(_py, TkOperation::SimpleDialogQuery) {
            return bits;
        }
        let Ok(handle) = parse_app_handle(_py, app_bits) else {
            return raise_invalid_handle_error(_py);
        };
        {
            let mut registry = tk_registry().lock().unwrap();
            if app_mut_from_registry(_py, &mut registry, handle).is_err() {
                return raise_invalid_handle_error(_py);
            }
        }

        let _parent_path =
            match get_string_arg(_py, handle, parent_path_bits, "simpledialog parent path") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let _title = match get_string_arg_allow_none(_py, handle, title_bits, "simpledialog title")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _prompt = match get_string_arg(_py, handle, prompt_bits, "simpledialog prompt") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let initial_text = match get_string_arg_allow_none(
            _py,
            handle,
            initial_value_bits,
            "simpledialog initial value",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let query_kind =
            match get_string_arg(_py, handle, query_kind_bits, "simpledialog query kind") {
                Ok(value) => value,
                Err(bits) => return bits,
            };

        #[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
        {
            let (int_min, int_max, float_min, float_max) = match query_kind.as_str() {
                "string" => (None, None, None, None),
                "int" => {
                    let min = match parse_optional_i64_arg(
                        _py,
                        handle,
                        min_value_bits,
                        "simpledialog minvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    let max = match parse_optional_i64_arg(
                        _py,
                        handle,
                        max_value_bits,
                        "simpledialog maxvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    (min, max, None, None)
                }
                "float" => {
                    let min = match parse_optional_f64_arg(
                        _py,
                        handle,
                        min_value_bits,
                        "simpledialog minvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    let max = match parse_optional_f64_arg(
                        _py,
                        handle,
                        max_value_bits,
                        "simpledialog maxvalue",
                    ) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                    (None, None, min, max)
                }
                _ => {
                    return raise_tcl_for_handle(
                        _py,
                        handle,
                        "simpledialog query kind must be one of: 'string', 'int', 'float'",
                    );
                }
            };

            let mut registry = tk_registry().lock().unwrap();
            let Ok(app) = app_mut_from_registry(_py, &mut registry, handle) else {
                return raise_invalid_handle_error(_py);
            };

            if !app.tk_loaded {
                if let Err(bits) = app_interp_eval_list(
                    _py,
                    app,
                    vec![
                        "package".to_string(),
                        "require".to_string(),
                        "Tk".to_string(),
                    ],
                ) {
                    return bits;
                }
                app.tk_loaded = true;
            }

            let dialog_token = next_after_token(&mut app.next_after_id).replace('#', "_");
            let dialog_path = format!(".__molt_simpledialog_{handle}_{dialog_token}");
            let body_path = format!("{dialog_path}.body");
            let prompt_widget = format!("{body_path}.prompt");
            let entry_widget = format!("{body_path}.entry");
            let button_row = format!("{dialog_path}.buttons");
            let ok_button = format!("{button_row}.ok");
            let cancel_button = format!("{button_row}.cancel");
            let state_var = format!("::__molt_simpledialog_state_{handle}_{dialog_token}");
            let ok_script = format!("set {state_var} ok");
            let cancel_script = format!("set {state_var} cancel");

            let mut created_dialog = false;

            let run_setup = |app: &mut TkAppState, words: Vec<String>| -> Result<TclObj, u64> {
                app_interp_eval_list(_py, app, words)
            };

            let setup_result = (|| -> Result<(), u64> {
                run_setup(app, vec!["toplevel".to_string(), dialog_path.clone()])?;
                created_dialog = true;
                if !_title.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "title".to_string(),
                            dialog_path.clone(),
                            _title.clone(),
                        ],
                    )?;
                }
                if !_parent_path.is_empty() {
                    run_setup(
                        app,
                        vec![
                            "wm".to_string(),
                            "transient".to_string(),
                            dialog_path.clone(),
                            _parent_path.clone(),
                        ],
                    )?;
                }
                run_setup(
                    app,
                    vec![
                        "wm".to_string(),
                        "resizable".to_string(),
                        dialog_path.clone(),
                        "0".to_string(),
                        "0".to_string(),
                    ],
                )?;
                run_setup(app, vec!["frame".to_string(), body_path.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        body_path.clone(),
                        "-padx".to_string(),
                        "8".to_string(),
                        "-pady".to_string(),
                        "8".to_string(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "label".to_string(),
                        prompt_widget.clone(),
                        "-text".to_string(),
                        _prompt.clone(),
                        "-anchor".to_string(),
                        "w".to_string(),
                        "-justify".to_string(),
                        "left".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        prompt_widget.clone(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(app, vec!["entry".to_string(), entry_widget.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        entry_widget.clone(),
                        "-fill".to_string(),
                        "x".to_string(),
                        "-pady".to_string(),
                        "6".to_string(),
                    ],
                )?;
                run_setup(app, vec!["frame".to_string(), button_row.clone()])?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        button_row.clone(),
                        "-padx".to_string(),
                        "8".to_string(),
                        "-pady".to_string(),
                        "8".to_string(),
                        "-fill".to_string(),
                        "x".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "button".to_string(),
                        ok_button.clone(),
                        "-text".to_string(),
                        "OK".to_string(),
                        "-command".to_string(),
                        ok_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "button".to_string(),
                        cancel_button.clone(),
                        "-text".to_string(),
                        "Cancel".to_string(),
                        "-command".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        ok_button.clone(),
                        "-side".to_string(),
                        "left".to_string(),
                        "-padx".to_string(),
                        "6".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "pack".to_string(),
                        cancel_button.clone(),
                        "-side".to_string(),
                        "left".to_string(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "wm".to_string(),
                        "protocol".to_string(),
                        dialog_path.clone(),
                        "WM_DELETE_WINDOW".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "bind".to_string(),
                        entry_widget.clone(),
                        "<Return>".to_string(),
                        ok_script.clone(),
                    ],
                )?;
                run_setup(
                    app,
                    vec![
                        "bind".to_string(),
                        entry_widget.clone(),
                        "<Escape>".to_string(),
                        cancel_script.clone(),
                    ],
                )?;
                if !initial_text.is_empty() {
                    run_setup(
                        app,
                        vec![
                            entry_widget.clone(),
                            "insert".to_string(),
                            "0".to_string(),
                            initial_text.clone(),
                        ],
                    )?;
                }
                run_setup(app, vec!["focus".to_string(), entry_widget.clone()])?;
                run_setup(
                    app,
                    vec!["grab".to_string(), "set".to_string(), dialog_path.clone()],
                )?;
                run_setup(
                    app,
                    vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                )?;
                Ok(())
            })();

            if let Err(bits) = setup_result {
                if created_dialog {
                    cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                }
                return bits;
            }

            let result_bits = loop {
                if let Err(bits) =
                    app_interp_eval_list(_py, app, vec!["vwait".to_string(), state_var.clone()])
                {
                    cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                    return bits;
                }
                let state = match app_interp_eval_list(
                    _py,
                    app,
                    vec!["set".to_string(), state_var.clone()],
                ) {
                    Ok(value) => value.to_string(),
                    Err(bits) => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                };
                if state == "cancel" {
                    break MoltObject::none().bits();
                }
                if state != "ok" {
                    if let Err(bits) = app_interp_eval_list(
                        _py,
                        app,
                        vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                    ) {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                    continue;
                }

                let value_text = match app_interp_eval_list(
                    _py,
                    app,
                    vec![entry_widget.clone(), "get".to_string()],
                ) {
                    Ok(value) => value.to_string(),
                    Err(bits) => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return bits;
                    }
                };

                match query_kind.as_str() {
                    "string" => match alloc_string_bits(_py, &value_text) {
                        Ok(bits) => break bits,
                        Err(bits) => {
                            cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                            return bits;
                        }
                    },
                    "int" => {
                        let Some(value) = parse_simpledialog_i64(&value_text) else {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        };
                        if int_min.is_some_and(|bound| value < bound)
                            || int_max.is_some_and(|bound| value > bound)
                        {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        }
                        break MoltObject::from_int(value).bits();
                    }
                    "float" => {
                        let Some(value) = parse_simpledialog_f64(&value_text) else {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        };
                        if float_min.is_some_and(|bound| value < bound)
                            || float_max.is_some_and(|bound| value > bound)
                        {
                            if let Err(bits) =
                                app_interp_eval_list(_py, app, vec!["bell".to_string()])
                            {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            if let Err(bits) = app_interp_eval_list(
                                _py,
                                app,
                                vec!["set".to_string(), state_var.clone(), "pending".to_string()],
                            ) {
                                cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                                return bits;
                            }
                            continue;
                        }
                        break MoltObject::from_float(value).bits();
                    }
                    _ => {
                        cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
                        return raise_tcl_for_handle(
                            _py,
                            handle,
                            "simpledialog query kind must be one of: 'string', 'int', 'float'",
                        );
                    }
                }
            };

            cleanup_native_simpledialog(_py, app, &dialog_path, &state_var);
            app.last_error = None;
            return result_bits;
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
        match query_kind.as_str() {
            "string" => {
                clear_last_error(handle);
                match alloc_string_bits(_py, &initial_text) {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                }
            }
            "int" => {
                let value = match parse_simpledialog_i64(&initial_text) {
                    Some(parsed) => parsed,
                    None => {
                        clear_last_error(handle);
                        return MoltObject::none().bits();
                    }
                };
                let min = match parse_optional_i64_arg(
                    _py,
                    handle,
                    min_value_bits,
                    "simpledialog minvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let max = match parse_optional_i64_arg(
                    _py,
                    handle,
                    max_value_bits,
                    "simpledialog maxvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if min.is_some_and(|bound| value < bound) || max.is_some_and(|bound| value > bound)
                {
                    clear_last_error(handle);
                    return MoltObject::none().bits();
                }
                clear_last_error(handle);
                MoltObject::from_int(value).bits()
            }
            "float" => {
                let value = match parse_simpledialog_f64(&initial_text) {
                    Some(parsed) => parsed,
                    None => {
                        clear_last_error(handle);
                        return MoltObject::none().bits();
                    }
                };
                let min = match parse_optional_f64_arg(
                    _py,
                    handle,
                    min_value_bits,
                    "simpledialog minvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let max = match parse_optional_f64_arg(
                    _py,
                    handle,
                    max_value_bits,
                    "simpledialog maxvalue",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if min.is_some_and(|bound| value < bound) || max.is_some_and(|bound| value > bound)
                {
                    clear_last_error(handle);
                    return MoltObject::none().bits();
                }
                clear_last_error(handle);
                MoltObject::from_float(value).bits()
            }
            _ => raise_tcl_for_handle(
                _py,
                handle,
                "simpledialog query kind must be one of: 'string', 'int', 'float'",
            ),
        }
    })
}
