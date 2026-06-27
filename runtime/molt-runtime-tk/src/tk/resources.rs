use super::args::{clear_last_error, get_string_arg, get_text_arg, raise_tcl_for_handle};
use super::parsing::{
    alloc_tuple_from_strings, option_map_query_or_empty, option_map_to_tuple,
    parse_widget_option_name_arg, parse_widget_option_pairs, widget_option_i64_default,
};
use super::state::{
    TkFontState, TkImageState, alloc_string_bits, app_mut_from_registry, app_tcl_error_locked,
    clear_value_map_refs, tk_registry, value_map_set_bits,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new};
use crate::bridge::{dec_ref_bits, inc_ref_bits};
use molt_runtime_core::prelude::{MoltObject, PyToken};
use std::collections::HashMap;

pub(super) fn handle_image_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "image expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "image subcommand")?;
    match subcommand.as_str() {
        "create" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "image create expects an image type",
                ));
            }
            let kind = get_string_arg(py, handle, args[2], "image type")?;
            let explicit_name = if args.len() >= 4 {
                let candidate = get_string_arg(py, handle, args[3], "image name")?;
                (!candidate.starts_with('-')).then_some(candidate)
            } else {
                None
            };
            let option_start = if explicit_name.is_some() { 4 } else { 3 };
            if !(args.len() - option_start).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "image create expects key/value options",
                ));
            }
            let mut option_names = Vec::with_capacity((args.len() - option_start) / 2);
            for idx in (option_start..args.len()).step_by(2) {
                option_names.push(get_string_arg(py, handle, args[idx], "image option name")?);
            }
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let name = if let Some(name) = explicit_name {
                name
            } else {
                let mut id = app.images.len() as i64 + 1;
                let mut generated = format!("pyimage{id}");
                while app.images.contains_key(&generated) {
                    id += 1;
                    generated = format!("pyimage{id}");
                }
                generated
            };
            if let Some(existing) = app.images.get_mut(&name) {
                clear_value_map_refs(py, &mut existing.options);
            }
            let mut options = HashMap::new();
            for (idx, option_name) in option_names.into_iter().enumerate() {
                let value_bits = args[option_start + idx * 2 + 1];
                inc_ref_bits(py, value_bits);
                if let Some(old_bits) = options.insert(option_name, value_bits) {
                    dec_ref_bits(py, old_bits);
                }
            }
            app.images
                .insert(name.clone(), TkImageState { kind, options });
            app.last_error = None;
            alloc_string_bits(py, &name)
        }
        "delete" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            for &bits in &args[2..] {
                let name = get_string_arg(py, handle, bits, "image name")?;
                if let Some(mut image) = app.images.remove(&name) {
                    clear_value_map_refs(py, &mut image.options);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "names" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut names: Vec<String> = app.images.keys().cloned().collect();
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate image names tuple")
        }
        "types" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut kinds: Vec<String> = app
                .images
                .values()
                .map(|image| image.kind.clone())
                .collect();
            kinds.sort_unstable();
            kinds.dedup();
            app.last_error = None;
            alloc_tuple_from_strings(py, kinds.as_slice(), "failed to allocate image types tuple")
        }
        "width" | "height" | "type" => {
            if args.len() != 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!("image {subcommand} expects an image name"),
                ));
            }
            let name = get_string_arg(py, handle, args[2], "image name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let Some(image) = app.images.get(&name) else {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    format!("image \"{name}\" does not exist"),
                ));
            };
            app.last_error = None;
            match subcommand.as_str() {
                "width" => Ok(MoltObject::from_int(widget_option_i64_default(
                    &image.options,
                    "-width",
                    0,
                ))
                .bits()),
                "height" => Ok(MoltObject::from_int(widget_option_i64_default(
                    &image.options,
                    "-height",
                    0,
                ))
                .bits()),
                _ => alloc_string_bits(py, &image.kind),
            }
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

pub(super) fn handle_image_instance_command(
    py: &PyToken,
    handle: i64,
    image_name: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "image command expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "image command subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    let Some(image) = app.images.get_mut(image_name) else {
        return Err(app_tcl_error_locked(
            py,
            app,
            format!("image \"{image_name}\" does not exist"),
        ));
    };
    match subcommand.as_str() {
        "configure" => {
            if args.len() == 2 {
                app.last_error = None;
                return option_map_to_tuple(py, &image.options, "failed to allocate image config");
            }
            if args.len() == 3 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[2], "image option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &image.options, &option_name);
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 2, "image configure options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut image.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "cget" => {
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "image cget expects exactly one option",
                ));
            }
            let option_name =
                parse_widget_option_name_arg(py, handle, args[2], "image option name")?;
            app.last_error = None;
            option_map_query_or_empty(py, &image.options, &option_name)
        }
        "blank" | "copy" | "put" | "write" => {
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "type" => {
            app.last_error = None;
            alloc_string_bits(py, &image.kind)
        }
        "width" => {
            app.last_error = None;
            Ok(MoltObject::from_int(widget_option_i64_default(&image.options, "-width", 0)).bits())
        }
        "height" => {
            app.last_error = None;
            Ok(
                MoltObject::from_int(widget_option_i64_default(&image.options, "-height", 0))
                    .bits(),
            )
        }
        "get" => {
            app.last_error = None;
            alloc_tuple_from_strings(
                py,
                &[String::from("0"), String::from("0"), String::from("0")],
                "failed to allocate image pixel tuple",
            )
        }
        "transparency" => {
            if args.len() >= 3 {
                let op = get_string_arg(py, handle, args[2], "image transparency op")?;
                if op == "get" {
                    app.last_error = None;
                    return Ok(MoltObject::from_bool(false).bits());
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("unknown image subcommand \"{subcommand}\" for image \"{image_name}\""),
        )),
    }
}

pub(super) fn handle_font_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "font expects a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "font subcommand")?;
    match subcommand.as_str() {
        "create" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font create expects a font name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "font name")?;
            if !(args.len() - 3).is_multiple_of(2) {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font create expects key/value options",
                ));
            }
            let option_pairs =
                parse_widget_option_pairs(py, handle, args, 3, "font create options")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            if let Some(existing) = app.fonts.get_mut(&name) {
                clear_value_map_refs(py, &mut existing.options);
            }
            let mut state = TkFontState::default();
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut state.options, option_name, value_bits);
            }
            app.fonts.insert(name.clone(), state);
            app.last_error = None;
            alloc_string_bits(py, &name)
        }
        "delete" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            for &bits in &args[2..] {
                let name = get_string_arg(py, handle, bits, "font name")?;
                if let Some(mut font) = app.fonts.remove(&name) {
                    clear_value_map_refs(py, &mut font.options);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "names" => {
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let mut names: Vec<String> = app.fonts.keys().cloned().collect();
            names.sort_unstable();
            app.last_error = None;
            alloc_tuple_from_strings(py, names.as_slice(), "failed to allocate font names tuple")
        }
        "families" => {
            let families = [
                String::from("TkDefaultFont"),
                String::from("TkTextFont"),
                String::from("TkFixedFont"),
            ];
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            alloc_tuple_from_strings(py, &families, "failed to allocate font families tuple")
        }
        "measure" => {
            if args.len() < 4 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font measure expects name and text",
                ));
            }
            let text = get_text_arg(py, handle, args[args.len() - 1], "font measure text")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            app.last_error = None;
            Ok(MoltObject::from_int((text.chars().count() as i64) * 8).bits())
        }
        "configure" | "actual" | "metrics" => {
            if args.len() < 3 {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    "font configure/actual/metrics expects a font name",
                ));
            }
            let name = get_string_arg(py, handle, args[2], "font name")?;
            let mut registry = tk_registry().lock().unwrap();
            let app = app_mut_from_registry(py, &mut registry, handle)?;
            let font = app.fonts.entry(name).or_default();
            if args.len() == 3 {
                app.last_error = None;
                return option_map_to_tuple(
                    py,
                    &font.options,
                    "failed to allocate font option tuple",
                );
            }
            if args.len() == 4 {
                let option_name =
                    parse_widget_option_name_arg(py, handle, args[3], "font option name")?;
                app.last_error = None;
                return option_map_query_or_empty(py, &font.options, &option_name);
            }
            let option_pairs = parse_widget_option_pairs(py, handle, args, 3, "font options")?;
            for (option_name, value_bits) in option_pairs {
                value_map_set_bits(py, &mut font.options, option_name, value_bits);
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        _ => {
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
    }
}

pub(super) fn command_is_image_instance(
    py: &PyToken,
    handle: i64,
    command: &str,
) -> Result<bool, u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    Ok(app.images.contains_key(command))
}
