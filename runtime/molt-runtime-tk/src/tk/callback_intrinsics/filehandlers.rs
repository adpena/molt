use super::super::args::raise_tcl_for_handle;
use super::super::callbacks::{
    callback_is_callable, clear_filehandler_registration_locked, filehandler_command_name,
    rollback_filehandler_registration_locked,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::dialogs::app_interp_eval_list;
use super::super::native::{register_tcl_callback_proc, unregister_tcl_callback_proc};
use super::super::state::{
    TK_FILE_EVENT_EXCEPTION, TK_FILE_EVENT_READABLE, TK_FILE_EVENT_WRITABLE, TkFileHandlerCommand,
    TkFileHandlerRegistration, TkOperation, app_mut_from_registry, app_tcl_error_locked,
    parse_app_handle, raise_invalid_handle_error, require_tk_operation, tk_registry,
};
use crate::bridge::{inc_ref_bits, raise_exception_u64, to_i64};
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};
use std::collections::HashMap;

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
