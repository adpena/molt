use super::super::args::{clear_last_error, get_string_arg, raise_tcl_for_handle};
use super::super::dispatch::{
    app_has_pending_after_work, dispatch_next_pending_event, parse_do_one_event_flags,
    tk_call_dispatch,
};
use super::super::native::pump_tcl_events;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::native::{build_native_tk_app, eval_tcl_without_gil, option_use_tk};
use super::super::parsing::{
    alloc_tuple_bits, alloc_tuple_from_strings, parse_bool_text, parse_tcl_script_commands,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::state::raise_tcl_error;
use super::super::state::{
    TK_DONT_WAIT_FLAG, TkAppState, TkOperation, alloc_string_bits, app_mut_from_registry,
    app_tcl_error_locked, clear_widget_refs, drop_app_state_refs, parse_app_handle,
    raise_invalid_handle_error, require_tk_app_new, require_tk_operation, tk_gate_state,
    tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::tcl::TclObj;
use super::super::trace_commands::bump_variable_versions_for_reference;
use crate::bridge::{
    dec_ref_bits, decode_value_list, is_truthy, raise_exception_u64, string_obj_to_owned, to_f64,
    to_i64,
};
use molt_runtime_core::prelude::{GilReleaseGuard, MoltObject, obj_from_bits};
use std::time::Duration;

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_available() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let gate = tk_gate_state(_py, TkOperation::AvailabilityProbe);
        let available = !gate.wasm_unsupported && !gate.backend_unimplemented;
        MoltObject::from_bool(available).bits()
    })
}
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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
