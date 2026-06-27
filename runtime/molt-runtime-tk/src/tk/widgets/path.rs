use super::super::args::{clear_last_error, get_string_arg, raise_tcl_for_handle};
use super::super::state::{
    app_mut_from_registry, app_tcl_error_locked, clear_widget_refs, drop_app_state_refs,
    raise_invalid_handle_error, tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::super::tcl::get;
use super::super::ttk::handle_ttk_widget_path_command;
use super::super::ttk_treeview::handle_treeview_widget_path_command;
use crate::bridge::{dec_ref_bits, inc_ref_bits};
use molt_runtime_core::prelude::{MoltObject, PyToken};

use super::common::unknown_widget_subcommand_message;
use super::generic::handle_generic_widget_path_command;
use super::listbox::handle_listbox_widget_path_command;
use super::menu::handle_menu_widget_path_command;
use super::panedwindow::handle_panedwindow_widget_path_command;

pub(in crate::tk) fn handle_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget path command requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "widget subcommand")?;
    if let Some(bits) =
        handle_treeview_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) = handle_ttk_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) = handle_menu_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) =
        handle_listbox_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    if let Some(bits) =
        handle_panedwindow_widget_path_command(py, handle, widget_path, &subcommand, args)?
    {
        return Ok(bits);
    }
    match subcommand.as_str() {
        "configure" => {
            if args.len() == 2 {
                clear_last_error(handle);
                return Ok(MoltObject::none().bits());
            }
            if !(args.len() - 2).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "configure expects key/value pairs",
                ));
            }
            let mut option_names = Vec::with_capacity((args.len() - 2) / 2);
            for idx in (2..args.len()).step_by(2) {
                option_names.push(get_string_arg(py, handle, args[idx], "widget option name")?);
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get_mut(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            for (idx, option_name) in option_names.into_iter().enumerate() {
                let value_bits = args[3 + idx * 2];
                inc_ref_bits(py, value_bits);
                if let Some(old_bits) = widget.options.insert(option_name, value_bits) {
                    dec_ref_bits(py, old_bits);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "cget expects exactly one option name",
                ));
            }
            let option_name = get_string_arg(py, handle, args[2], "widget option name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.get(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            let Some(value_bits) = widget.options.get(&option_name).copied() else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown option \"{option_name}\""),
                ));
            };
            inc_ref_bits(py, value_bits);
            app.last_error = None;
            Ok(value_bits)
        }
        "destroy" => {
            if args.len() != 2 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "destroy expects no additional arguments",
                ));
            }
            if widget_path == "." {
                let mut registry = tk_registry().lock().unwrap();
                let Some(mut app) = registry.apps.remove(&handle) else {
                    return Err(raise_invalid_handle_error(py));
                };
                drop_app_state_refs(py, &mut app);
                return Ok(MoltObject::none().bits());
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(widget) = app.widgets.remove(widget_path) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad window path name \"{widget_path}\""),
                ));
            };
            clear_widget_refs(py, widget);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            if let Some(bits) =
                handle_generic_widget_path_command(py, handle, widget_path, &subcommand, args)?
            {
                return Ok(bits);
            }
            Err(raise_tcl_for_handle(
                py,
                handle,
                unknown_widget_subcommand_message(widget_path, &subcommand),
            ))
        }
    }
}
