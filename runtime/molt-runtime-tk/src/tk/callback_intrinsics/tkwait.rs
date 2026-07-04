use super::super::args::get_string_arg;
use super::super::commands::{
    handle_tkwait_variable_target, handle_tkwait_visibility_target, handle_tkwait_window_target,
};
use super::super::state::{
    TkOperation, parse_app_handle, raise_invalid_handle_error, require_tk_operation,
};

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
