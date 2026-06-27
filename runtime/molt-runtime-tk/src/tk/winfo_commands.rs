use super::args::{get_string_arg, get_text_arg, raise_tcl_for_handle};
use super::parsing::{
    alloc_int_tuple2_bits, alloc_tuple_bits, alloc_tuple_from_strings, parse_i64_arg,
    parse_winfo_rgb_components, tk_widget_class_name, widget_option_i64_default,
};
use super::state::{alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, tk_registry};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new};
use super::widgets::common::alloc_empty_string_bits;
use molt_runtime_core::prelude::{MoltObject, PyToken};

pub(super) fn handle_winfo_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "winfo requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "winfo subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "children" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo children expects a widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let children: Vec<String> = if path == "." {
                let mut names: Vec<String> = app.widgets.keys().cloned().collect();
                names.sort_unstable();
                names
            } else {
                Vec::new()
            };
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                children.as_slice(),
                "failed to allocate children",
            );
        }
        "exists" | "ismapped" | "viewable" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo exists/ismapped/viewable expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let exists = path == "." || app.widgets.contains_key(&path);
            let value = if subcommand == "exists" {
                exists
            } else if path == "." {
                true
            } else {
                app.widgets
                    .get(&path)
                    .is_some_and(|widget| widget.manager.is_some())
            };
            app.last_error = None;
            return Ok(MoltObject::from_bool(value).bits());
        }
        "manager" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo manager expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                "wm".to_string()
            } else {
                app.widgets
                    .get(&path)
                    .and_then(|widget| widget.manager.clone())
                    .unwrap_or_default()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "class" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo class expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let class_name = if path == "." {
                "Tk".to_string()
            } else if let Some(widget) = app.widgets.get(&path) {
                tk_widget_class_name(&widget.widget_command)
            } else {
                String::new()
            };
            app.last_error = None;
            return alloc_string_bits(py, &class_name);
        }
        "name" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo name expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let name = if path == "." {
                "tk".to_string()
            } else {
                path.trim_start_matches('.')
                    .trim_start_matches('!')
                    .to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "parent" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo parent expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let parent = if path == "." {
                String::new()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &parent);
        }
        "toplevel" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo toplevel expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ".");
        }
        "id" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo id expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let id = if path == "." {
                1
            } else {
                (path
                    .bytes()
                    .fold(17_u64, |acc, b| acc.wrapping_mul(33).wrapping_add(b as u64))
                    % 1_000_000) as i64
                    + 2
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "width" | "reqwidth" | "height" | "reqheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo width/height/reqwidth/reqheight expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let value = if path == "." {
                if subcommand.ends_with("width") {
                    200
                } else {
                    160
                }
            } else if let Some(widget) = app.widgets.get(&path) {
                if subcommand.ends_with("width") {
                    widget_option_i64_default(&widget.options, "-width", 200)
                } else {
                    widget_option_i64_default(&widget.options, "-height", 160)
                }
            } else {
                0
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "x" | "y" | "rootx" | "rooty" | "pointerx" | "pointery" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo coordinate query expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        "screenwidth" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "screenheight" => {
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "pointerxy" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pointerxy expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_int_tuple2_bits(py, 0, 0, "failed to allocate pointerxy tuple");
        }
        "rgb" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo rgb expects widget path and color",
                ));
            }
            let color = get_string_arg(py, handle, args[3], "winfo color")?;
            let (r, g, b) = parse_winfo_rgb_components(&color);
            let elems = vec![
                MoltObject::from_int(r).bits(),
                MoltObject::from_int(g).bits(),
                MoltObject::from_int(b).bits(),
            ];
            app.last_error = None;
            return alloc_tuple_bits(py, elems.as_slice(), "failed to allocate winfo rgb tuple");
        }
        "atom" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atom expects atom name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "atom name")?;
            let id = if let Some(id) = app.atoms_by_name.get(&name).copied() {
                id
            } else {
                app.next_atom_id = app.next_atom_id.saturating_add(1);
                let id = app.next_atom_id;
                app.atoms_by_name.insert(name.clone(), id);
                app.atoms_by_id.insert(id, name);
                id
            };
            app.last_error = None;
            return Ok(MoltObject::from_int(id).bits());
        }
        "atomname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo atomname expects atom id",
                ));
            }
            let atom_id = parse_i64_arg(py, handle, args[2], "atom id")?;
            let name = app.atoms_by_id.get(&atom_id).cloned().unwrap_or_default();
            app.last_error = None;
            return alloc_string_bits(py, &name);
        }
        "containing" => {
            if args.len() != 4 && args.len() != 6 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo containing expects root coordinates with optional -displayof",
                ));
            }
            let value = if let Some(first) = app.widgets.keys().next() {
                first.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "cells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo cells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(256).bits());
        }
        "colormapfull" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo colormapfull expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_bool(false).bits());
        }
        "depth" | "screendepth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo depth/screendepth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(24).bits());
        }
        "fpixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo fpixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo fpixels distance")?;
            let value = distance.trim().parse::<f64>().unwrap_or(0.0);
            app.last_error = None;
            return Ok(MoltObject::from_float(value).bits());
        }
        "pixels" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pixels expects widget path and distance",
                ));
            }
            let distance = get_text_arg(py, handle, args[3], "winfo pixels distance")?;
            let value = distance
                .trim()
                .parse::<f64>()
                .map(|v| v.round() as i64)
                .unwrap_or(0);
            app.last_error = None;
            return Ok(MoltObject::from_int(value).bits());
        }
        "geometry" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo geometry expects widget path",
                ));
            }
            let path = get_string_arg(py, handle, args[2], "winfo widget path")?;
            let (width, height) = if path == "." {
                (200, 160)
            } else if let Some(widget) = app.widgets.get(&path) {
                (
                    widget_option_i64_default(&widget.options, "-width", 200),
                    widget_option_i64_default(&widget.options, "-height", 160),
                )
            } else {
                (0, 0)
            };
            app.last_error = None;
            return alloc_string_bits(py, &format!("{width}x{height}+0+0"));
        }
        "interps" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo interps expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_tuple_from_strings(
                py,
                &[String::from("molt")],
                "failed to allocate winfo interps",
            );
        }
        "pathname" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo pathname expects window id",
                ));
            }
            let window_id = parse_i64_arg(py, handle, args[2], "winfo window id")?;
            let value = if window_id <= 1 {
                ".".to_string()
            } else if let Some(path) = app.widgets.keys().next() {
                path.clone()
            } else {
                ".".to_string()
            };
            app.last_error = None;
            return alloc_string_bits(py, &value);
        }
        "screen" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screen expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, ":0.0");
        }
        "screencells" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screencells expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(16_777_216).bits());
        }
        "screenmmheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(270).bits());
        }
        "screenmmwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo screenmmwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(340).bits());
        }
        "screenvisual" | "visual" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visual/screenvisual expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "truecolor");
        }
        "server" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo server expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "MoltTk");
        }
        "visualid" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo visualid expects widget path",
                ));
            }
            app.last_error = None;
            return alloc_string_bits(py, "0x00000021");
        }
        "vrootheight" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootheight expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(768).bits());
        }
        "vrootwidth" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootwidth expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(1024).bits());
        }
        "vrootx" | "vrooty" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "winfo vrootx/vrooty expects widget path",
                ));
            }
            app.last_error = None;
            return Ok(MoltObject::from_int(0).bits());
        }
        _ => {}
    }
    app.last_error = None;
    alloc_empty_string_bits(py)
}
