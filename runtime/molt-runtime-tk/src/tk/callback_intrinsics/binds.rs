use super::super::args::{get_string_arg, raise_tcl_for_handle};
use super::super::callbacks::{
    callback_is_callable, clear_filehandler_registration_locked, next_callback_command_name,
    normalize_bind_add_prefix, register_callback_command, unregister_callback_command,
};
use super::super::event_commands::remove_bind_script_command_invocations;
use super::super::native::{register_tcl_callback_proc, unregister_tcl_callback_proc};
use super::super::state::{
    TK_BIND_SUBST_FORMAT_STR, TkOperation, alloc_string_bits, app_mut_from_registry,
    app_tcl_error_locked, parse_app_handle, raise_invalid_handle_error, require_tk_operation,
    tk_registry,
};
use super::super::trace_commands::{call_tk_command_from_strings, release_result_bits};
use crate::bridge::{dec_ref_bits, inc_ref_bits, raise_exception_u64, string_obj_to_owned};
use molt_runtime_core::prelude::{MoltObject, obj_from_bits};

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
