use super::*;

pub(super) fn handle_focus_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match args.len() {
        1 => {
            let value = app.focus_widget.clone().unwrap_or_default();
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        2 => {
            let target = get_string_arg(py, handle, args[1], "focus target")?;
            app.focus_widget = if target.is_empty() {
                None
            } else {
                Some(target)
            };
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        3 => {
            let op = get_string_arg(py, handle, args[1], "focus option")?;
            let target = get_string_arg(py, handle, args[2], "focus target")?;
            match op.as_str() {
                "-force" => {
                    app.focus_widget = if target.is_empty() {
                        None
                    } else {
                        Some(target)
                    };
                    app.last_error = None;
                    Ok(MoltObject::none().bits())
                }
                "-lastfor" => {
                    if app.focus_widget.is_none() {
                        app.focus_widget = if target.is_empty() {
                            None
                        } else {
                            Some(target.clone())
                        };
                    }
                    let value = app.focus_widget.clone().unwrap_or_default();
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                "-displayof" => {
                    let value = app.focus_widget.clone().unwrap_or(target);
                    app.last_error = None;
                    alloc_string_bits(py, &value)
                }
                _ => Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("bad focus option \"{op}\": must be -displayof, -force, or -lastfor"),
                )),
            }
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            "focus expects no args, a target, or -force/-lastfor target",
        )),
    }
}

pub(super) fn handle_focus_direction_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
    label: &str,
) -> Result<u64, u64> {
    if args.len() != 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} expects a widget target"),
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "focus widget")?;
    clear_last_error(handle);
    alloc_string_bits(py, &widget_path)
}
