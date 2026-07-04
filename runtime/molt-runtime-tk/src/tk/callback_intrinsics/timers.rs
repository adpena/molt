use super::super::args::{get_text_arg, raise_tcl_for_handle};
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::super::callbacks::schedule_after_timer_token;
use super::super::callbacks::{
    after_callback_name_from_token, alloc_after_info_all, alloc_after_info_token,
    cleanup_after_tokens, next_after_token, register_after_command_token, tokens_for_after_command,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::native::eval_tcl_without_gil;
use super::super::native::register_tcl_callback_proc;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::native::unregister_tcl_callback_proc;
#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
use super::super::state::TkEvent;
use super::super::state::{
    TkOperation, alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, parse_app_handle,
    raise_invalid_handle_error, require_tk_operation, tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::tcl::TclObj;
use crate::bridge::{dec_ref_bits, inc_ref_bits, is_truthy, raise_exception_u64, to_i64};
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};
use std::collections::HashSet;

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
