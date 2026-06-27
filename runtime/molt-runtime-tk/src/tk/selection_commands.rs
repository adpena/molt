use super::args::{get_string_arg, raise_tcl_for_handle};
use super::state::{alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::new;
use molt_runtime_core::prelude::{MoltObject, PyToken};

pub(super) fn handle_clipboard_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "clipboard requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "clipboard subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.clipboard_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "append" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "clipboard append expects a string payload",
                ));
            }
            let mut payload = String::new();
            let mut idx = 2;
            while idx < args.len() {
                let token = get_string_arg(py, handle, args[idx], "clipboard token")?;
                if token == "--" && idx + 1 < args.len() {
                    payload = get_string_arg(py, handle, args[idx + 1], "clipboard payload")?;
                    break;
                }
                payload = token;
                idx += 1;
            }
            app.clipboard_text.push_str(&payload);
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            app.last_error = None;
            alloc_string_bits(py, &app.clipboard_text)
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad clipboard option \"{subcommand}\": must be append, clear, or get"),
        )),
    }
}

pub(super) fn handle_selection_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "selection requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "selection subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "clear" => {
            app.selection_text.clear();
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "get" => {
            let value = if app.selection_text.is_empty() {
                app.clipboard_text.clone()
            } else {
                app.selection_text.clone()
            };
            app.last_error = None;
            alloc_string_bits(py, &value)
        }
        "own" => {
            if args.len() == 2 {
                app.last_error = None;
                return alloc_string_bits(py, app.selection_owner.as_deref().unwrap_or(""));
            }
            let mut owner: Option<String> = None;
            for &bits in &args[2..] {
                let token = get_string_arg(py, handle, bits, "selection own argument")?;
                if token.starts_with('-') {
                    continue;
                }
                owner = Some(token);
            }
            app.selection_owner = owner;
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "handle" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad selection option \"{subcommand}\": must be clear, get, handle, or own"),
        )),
    }
}
