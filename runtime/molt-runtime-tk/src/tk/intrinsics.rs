use super::args::{
    clear_last_error, get_string_arg, get_string_arg_allow_none, parse_optional_f64_arg,
    parse_optional_i64_arg, raise_tcl_for_handle,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::callbacks::next_after_token;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::dialogs::{
    app_interp_eval_list, cleanup_native_simpledialog, tk_dispatch_string_command,
};
use super::dialogs::{
    commondialog_is_supported_command, dispatch_commondialog_via_tk_call,
    filedialog_is_supported_command, parse_commondialog_options, parse_simpledialog_f64,
    parse_simpledialog_i64, raise_unsupported_commondialog_command,
    raise_unsupported_filedialog_command,
};
use super::dispatch::{
    app_has_pending_after_work, dispatch_next_pending_event, parse_do_one_event_flags,
    tk_call_dispatch,
};
use super::native::pump_tcl_events;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::native::{build_native_tk_app, eval_tcl_without_gil, option_use_tk};
use super::parsing::{
    alloc_tuple_bits, alloc_tuple_from_strings, parse_bool_text, parse_tcl_script_commands,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::state::raise_tcl_error;
use super::state::{
    TK_DONT_WAIT_FLAG, TkAppState, TkOperation, alloc_string_bits, app_mut_from_registry,
    app_tcl_error_locked, clear_widget_refs, drop_app_state_refs, parse_app_handle,
    raise_invalid_handle_error, require_tk_app_new, require_tk_operation, tk_gate_state,
    tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::TclObj;
use super::trace_commands::bump_variable_versions_for_reference;
use crate::bridge::{
    dec_ref_bits, decode_value_list, is_truthy, raise_exception_u64, string_obj_to_owned, to_f64,
    to_i64,
};
use molt_runtime_core::prelude::{GilReleaseGuard, MoltObject, obj_from_bits};
use std::time::Duration;

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
