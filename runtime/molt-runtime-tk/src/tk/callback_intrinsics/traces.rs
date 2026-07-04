use super::super::args::{get_string_arg, raise_tcl_for_handle};
use super::super::callbacks::{
    callback_is_callable, clear_trace_registrations_for_variable, next_callback_command_name,
    register_callback_command, remove_trace_registration,
};
use super::super::state::{
    TkOperation, TkTraceRegistration, alloc_string_bits, app_mut_from_registry, parse_app_handle,
    raise_invalid_handle_error, require_tk_operation, tk_registry,
};
use super::super::trace_commands::{alloc_trace_info, normalize_trace_mode_name};
use crate::bridge::raise_exception_u64;
use molt_runtime_core::prelude::MoltObject;

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
