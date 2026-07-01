use super::args::{get_string_arg, get_text_arg, raise_tcl_for_handle};
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::callbacks::schedule_after_timer_token;
use super::callbacks::{
    after_callback_name_from_token, alloc_after_info_all, alloc_after_info_token,
    callback_is_callable, cleanup_after_tokens, clear_filehandler_registration_locked,
    clear_trace_registrations_for_variable, filehandler_command_name, next_after_token,
    next_callback_command_name, normalize_bind_add_prefix, register_after_command_token,
    register_callback_command, remove_trace_registration, rollback_filehandler_registration_locked,
    tokens_for_after_command, unregister_callback_command,
};
use super::commands::{
    handle_tkwait_variable_target, handle_tkwait_visibility_target, handle_tkwait_window_target,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::dialogs::app_interp_eval_list;
use super::event_commands::{
    TK_EVENT_SUBST_FIELD_COUNT, flatten_event_subst_arg, normalize_event_subst_bool_field,
    normalize_event_subst_delta_field, normalize_event_subst_int_field,
    remove_bind_script_command_invocations,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::native::eval_tcl_without_gil;
use super::native::{register_tcl_callback_proc, unregister_tcl_callback_proc};
use super::parsing::alloc_tuple_bits;
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::state::TkEvent;
use super::state::{
    TK_BIND_SUBST_FORMAT_STR, TK_FILE_EVENT_EXCEPTION, TK_FILE_EVENT_READABLE,
    TK_FILE_EVENT_WRITABLE, TkFileHandlerCommand, TkFileHandlerRegistration, TkOperation,
    TkTraceRegistration, alloc_string_bits, app_mut_from_registry, app_tcl_error_locked,
    parse_app_handle, raise_invalid_handle_error, require_tk_operation, tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::TclObj;
use super::trace_commands::{
    alloc_trace_info, call_tk_command_from_strings, normalize_trace_mode_name, release_result_bits,
};
use crate::bridge::{
    dec_ref_bits, decode_value_list, inc_ref_bits, is_truthy, raise_exception_u64,
    string_obj_to_owned, to_i64,
};
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};
use std::collections::{HashMap, HashSet};

#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
