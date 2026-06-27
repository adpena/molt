use super::args::{get_string_arg, raise_tcl_for_handle};
use super::state::{alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry};
use molt_runtime_core::prelude::{MoltObject, PyToken};

pub(super) fn handle_grab_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "grab requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "grab subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "set" => {
            if args.len() == 3 {
                let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = false;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            if args.len() == 4 {
                let scope = get_string_arg(py, handle, args[2], "grab scope")?;
                if scope != "-global" {
                    return Err(app_tcl_error_locked(
                        py,
                        app,
                        "grab set scope must be -global",
                    ));
                }
                let widget_path = get_string_arg(py, handle, args[3], "grab widget")?;
                app.grab_widget = Some(widget_path);
                app.grab_is_global = true;
                app.last_error = None;
                return Ok(MoltObject::none().bits());
            }
            Err(app_tcl_error_locked(
                py,
                app,
                "grab set expects widget or -global widget",
            ))
        }
        "release" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab release expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                app.grab_widget = None;
                app.grab_is_global = false;
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "current" => {
            if args.len() != 2 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab current expects no extra arguments",
                ));
            }
            let widget_path = app.grab_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &widget_path)
        }
        "status" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "grab status expects a widget",
                ));
            }
            let widget_path = get_string_arg(py, handle, args[2], "grab widget")?;
            let status = if app.grab_widget.as_deref() == Some(widget_path.as_str()) {
                if app.grab_is_global {
                    "global"
                } else {
                    "local"
                }
            } else {
                ""
            };
            app.last_error = None;
            alloc_string_bits(py, status)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad grab option \"{subcommand}\": must be current, release, set, or status"),
        )),
    }
}
