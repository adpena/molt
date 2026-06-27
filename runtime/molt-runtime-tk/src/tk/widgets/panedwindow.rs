use super::super::args::get_string_arg;
use super::super::parsing::{
    alloc_tuple_from_strings, option_map_to_tuple, parse_simple_end_or_int_index,
    parse_simple_end_or_int_index_bits, parse_widget_option_name_arg, parse_widget_option_pairs,
};
use super::super::state::{
    TkAppState, TkWidgetState, app_mut_from_registry, app_tcl_error_locked, clear_value_map_refs,
    tk_registry, value_map_set_bits,
};
use super::common::{
    alloc_empty_string_bits, alloc_widget_coord_bits, unknown_widget_subcommand_message,
};
use crate::bridge::inc_ref_bits;
use molt_runtime_core::prelude::{MoltObject, PyToken};
#[cfg(test)]
use std::collections::HashMap;

pub(in crate::tk) fn handle_panedwindow_widget_path_command(
    py: &PyToken,
    handle: i64,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if !is_panedwindow_widget_subcommand(subcommand) {
        return Ok(None);
    }

    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(widget) = app.widgets.get_mut(widget_path) else {
        return Ok(None);
    };
    if widget.widget_command != "panedwindow" {
        return Ok(None);
    }

    match subcommand {
        "add" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow add expects child path and optional key/value pairs",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            panedwindow_add_child(widget, child.clone());
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
            panedwindow_apply_pane_options(py, widget, child, option_pairs);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "insert" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow insert expects index, child path, and optional key/value pairs",
                ));
            }
            let Some(index) =
                parse_simple_end_or_int_index_bits(args[2], widget.pane_children.len())
            else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow insert index must be an integer or end",
                ));
            };
            let child = get_string_arg(py, handle, args[3], "panedwindow child path")?;
            panedwindow_insert_child(widget, index, child.clone());
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 4, "panedwindow pane options")?;
            panedwindow_apply_pane_options(py, widget, child, option_pairs);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "forget" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow forget expects exactly one child path",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            panedwindow_forget_child(py, widget, &child);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "index" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "index expects exactly one index argument",
                ));
            }
            let token = get_string_arg(py, handle, args[2], "panedwindow index")?;
            if let Some(position) = widget.pane_children.iter().position(|item| item == &token) {
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(position as i64).bits()));
            }
            if let Some(index) =
                parse_simple_end_or_int_index(token.as_str(), widget.pane_children.len())
            {
                app.last_error = None;
                return Ok(Some(MoltObject::from_int(index as i64).bits()));
            }
            Err(app_tcl_error_locked(
                py,
                app,
                format!("bad panedwindow index \"{token}\""),
            ))
        }
        "panes" => {
            app.last_error = None;
            alloc_tuple_from_strings(
                py,
                widget.pane_children.as_slice(),
                "failed to allocate panedwindow panes tuple",
            )
            .map(Some)
        }
        "panecget" => {
            if args.len() != 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow panecget expects child and option",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            let option_name =
                parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
            if let Some(bits) = widget
                .pane_child_options
                .get(&child)
                .and_then(|options| options.get(&option_name))
                .copied()
            {
                inc_ref_bits(py, bits);
                app.last_error = None;
                return Ok(Some(bits));
            }
            app.last_error = None;
            alloc_empty_string_bits(py).map(Some)
        }
        "paneconfigure" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "panedwindow paneconfigure expects child and optional key/value options",
                ));
            }
            let child = get_string_arg(py, handle, args[2], "panedwindow child path")?;
            if !panedwindow_has_child(widget, &child) {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("unknown pane \"{child}\""),
                ));
            }
            if args.len() == 3 {
                let options = widget
                    .pane_child_options
                    .get(&child)
                    .cloned()
                    .unwrap_or_default();
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &options,
                    "failed to allocate panedwindow paneconfigure tuple",
                )
                .map(Some);
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "pane option name")?;
                if let Some(bits) = widget
                    .pane_child_options
                    .get(&child)
                    .and_then(|options| options.get(&option_name))
                    .copied()
                {
                    inc_ref_bits(py, bits);
                    app.last_error = None;
                    return Ok(Some(bits));
                }
                app.last_error = None;
                return alloc_empty_string_bits(py).map(Some);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "panedwindow pane options")?;
            panedwindow_apply_pane_options(py, widget, child, option_pairs);
            app.last_error = None;
            Ok(Some(MoltObject::none().bits()))
        }
        "proxy" => handle_panedwindow_coord_subcommand(py, handle, app, widget_path, "proxy", args),
        "sash" => handle_panedwindow_coord_subcommand(py, handle, app, widget_path, "sash", args),
        _ => Ok(None),
    }
}

fn is_panedwindow_widget_subcommand(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "add"
            | "insert"
            | "forget"
            | "index"
            | "panes"
            | "panecget"
            | "paneconfigure"
            | "proxy"
            | "sash"
    )
}

fn handle_panedwindow_coord_subcommand(
    py: &PyToken,
    handle: i64,
    app: &mut TkAppState,
    widget_path: &str,
    subcommand: &str,
    args: &[u64],
) -> Result<Option<u64>, u64> {
    if args.len() >= 3 {
        let op = get_string_arg(py, handle, args[2], &format!("{subcommand} subcommand"))?;
        if op == "coord" {
            app.last_error = None;
            return alloc_widget_coord_bits(py).map(Some);
        }
        return Err(app_tcl_error_locked(
            py,
            app,
            unknown_widget_subcommand_message(widget_path, &format!("{subcommand} {op}")),
        ));
    }
    app.last_error = None;
    Ok(Some(MoltObject::none().bits()))
}

fn panedwindow_has_child(widget: &TkWidgetState, child: &str) -> bool {
    widget
        .pane_children
        .iter()
        .any(|existing| existing == child)
}

fn panedwindow_add_child(widget: &mut TkWidgetState, child: String) {
    if !panedwindow_has_child(widget, &child) {
        widget.pane_children.push(child);
    }
}

fn panedwindow_insert_child(widget: &mut TkWidgetState, index: usize, child: String) {
    widget.pane_children.retain(|existing| existing != &child);
    let insert_index = index.min(widget.pane_children.len());
    widget.pane_children.insert(insert_index, child);
}

fn panedwindow_forget_child(py: &PyToken, widget: &mut TkWidgetState, child: &str) {
    widget.pane_children.retain(|existing| existing != child);
    if let Some(mut options) = widget.pane_child_options.remove(child) {
        clear_value_map_refs(py, &mut options);
    }
}

fn panedwindow_apply_pane_options(
    py: &PyToken,
    widget: &mut TkWidgetState,
    child: String,
    option_pairs: Vec<(String, u64)>,
) {
    let pane_options = widget.pane_child_options.entry(child).or_default();
    for (option_name, value_bits) in option_pairs {
        value_map_set_bits(py, pane_options, option_name, value_bits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panedwindow_widget() -> TkWidgetState {
        TkWidgetState {
            widget_command: "panedwindow".to_string(),
            ..TkWidgetState::default()
        }
    }

    #[test]
    fn panedwindow_child_order_is_unique_and_reorderable() {
        let mut widget = panedwindow_widget();

        panedwindow_add_child(&mut widget, ".a".to_string());
        panedwindow_add_child(&mut widget, ".b".to_string());
        panedwindow_add_child(&mut widget, ".a".to_string());
        panedwindow_insert_child(&mut widget, 0, ".b".to_string());

        assert_eq!(
            widget.pane_children,
            vec![".b".to_string(), ".a".to_string()]
        );
    }

    #[test]
    fn panedwindow_forget_removes_child_and_option_authority() {
        let py = PyToken::new();
        let mut widget = panedwindow_widget();
        panedwindow_add_child(&mut widget, ".a".to_string());
        widget
            .pane_child_options
            .insert(".a".to_string(), HashMap::new());

        panedwindow_forget_child(&py, &mut widget, ".a");

        assert!(widget.pane_children.is_empty());
        assert!(!widget.pane_child_options.contains_key(".a"));
    }
}
