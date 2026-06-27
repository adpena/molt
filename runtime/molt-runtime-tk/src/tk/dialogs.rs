use super::args::{clear_last_error, get_string_arg, get_text_arg, raise_tcl_for_handle};
use super::dispatch::tk_call_dispatch;
use super::parsing::{
    alloc_tuple_from_strings, parse_bool_arg, parse_i64_arg, parse_widget_option_pairs,
};
use super::state::alloc_string_bits;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::state::{TkAppState, app_mut_from_registry, app_tcl_error_locked, tk_registry};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::TclObj;
use crate::bridge::{dec_ref_bits, decode_value_list, dict_order, object_type_id};
use molt_runtime_core::prelude::{MoltObject, PyToken, obj_from_bits};
use molt_runtime_core::type_ids::TYPE_ID_DICT;

pub(super) fn normalize_commondialog_option_name(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

pub(super) fn parse_commondialog_options(
    py: &PyToken,
    handle: i64,
    options_bits: u64,
) -> Result<Vec<(String, u64)>, u64> {
    let options_obj = obj_from_bits(options_bits);
    if options_obj.is_none() {
        return Ok(Vec::new());
    }

    if let Some(dict_ptr) = options_obj.as_ptr()
        && object_type_id(dict_ptr) == TYPE_ID_DICT
    {
        let entries = dict_order(dict_ptr);
        let mut options = Vec::with_capacity(entries.len() / 2);
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name = get_string_arg(py, handle, pair[0], "commondialog option name")?;
            let value_bits = pair[1];
            if obj_from_bits(value_bits).is_none() {
                continue;
            }
            options.push((normalize_commondialog_option_name(&name), value_bits));
        }
        return Ok(options);
    }

    let Some(raw_items) = decode_value_list(options_obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "commondialog options must be a dict or list/tuple",
        ));
    };
    if !raw_items.len().is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "commondialog option list must contain key/value pairs",
        ));
    }

    let mut options = Vec::with_capacity(raw_items.len() / 2);
    for idx in (0..raw_items.len()).step_by(2) {
        let name = get_string_arg(py, handle, raw_items[idx], "commondialog option name")?;
        let value_bits = raw_items[idx + 1];
        if obj_from_bits(value_bits).is_none() {
            continue;
        }
        options.push((normalize_commondialog_option_name(&name), value_bits));
    }
    Ok(options)
}

pub(super) fn commondialog_option_value_bits(options: &[(String, u64)], key: &str) -> Option<u64> {
    options
        .iter()
        .rev()
        .find_map(|(name, bits)| name.eq_ignore_ascii_case(key).then_some(*bits))
}

pub(super) fn commondialog_is_supported_command(command: &str) -> bool {
    matches!(
        command,
        "tk_messageBox"
            | "tk_getOpenFile"
            | "tk_getSaveFile"
            | "tk_chooseDirectory"
            | "tk_chooseColor"
    )
}

pub(super) fn commondialog_supports_parent(command: &str) -> bool {
    commondialog_is_supported_command(command)
}

pub(super) fn commondialog_allowed_options(command: &str) -> &'static [&'static str] {
    match command {
        "tk_messageBox" => &[
            "-command", "-default", "-detail", "-icon", "-message", "-parent", "-title", "-type",
        ],
        "tk_getOpenFile" => &[
            "-defaultextension",
            "-filetypes",
            "-initialdir",
            "-initialfile",
            "-multiple",
            "-parent",
            "-title",
            "-typevariable",
        ],
        "tk_getSaveFile" => &[
            "-confirmoverwrite",
            "-defaultextension",
            "-filetypes",
            "-initialdir",
            "-initialfile",
            "-parent",
            "-title",
            "-typevariable",
        ],
        "tk_chooseDirectory" => &["-initialdir", "-mustexist", "-parent", "-title"],
        "tk_chooseColor" => &["-initialcolor", "-parent", "-title"],
        _ => &[],
    }
}

pub(super) fn validate_commondialog_options(
    py: &PyToken,
    handle: i64,
    command: &str,
    options: &[(String, u64)],
) -> Result<(), u64> {
    let allowed = commondialog_allowed_options(command);
    for (option_name, _) in options {
        let is_allowed = allowed
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(option_name));
        if !is_allowed {
            return Err(raise_tcl_for_handle(
                py,
                handle,
                format!("unknown option \"{option_name}\" for {command}"),
            ));
        }
    }
    Ok(())
}

pub(super) fn raise_unsupported_commondialog_command(
    py: &PyToken,
    handle: i64,
    command: &str,
) -> u64 {
    raise_tcl_for_handle(
        py,
        handle,
        format!("unsupported commondialog command \"{command}\""),
    )
}

pub(super) fn commondialog_option_text(
    py: &PyToken,
    handle: i64,
    options: &[(String, u64)],
    key: &str,
    label: &str,
) -> Result<Option<String>, u64> {
    let Some(value_bits) = commondialog_option_value_bits(options, key) else {
        return Ok(None);
    };
    Ok(Some(get_text_arg(py, handle, value_bits, label)?))
}

pub(super) fn commondialog_option_bool(
    py: &PyToken,
    handle: i64,
    options: &[(String, u64)],
    key: &str,
    label: &str,
) -> Result<Option<bool>, u64> {
    let Some(value_bits) = commondialog_option_value_bits(options, key) else {
        return Ok(None);
    };
    Ok(Some(parse_bool_arg(py, handle, value_bits, label)?))
}

pub(super) fn messagebox_buttons_for_type(dialog_type: &str) -> Option<&'static [&'static str]> {
    match dialog_type {
        "ok" => Some(&["ok"]),
        "okcancel" => Some(&["ok", "cancel"]),
        "yesno" => Some(&["yes", "no"]),
        "yesnocancel" => Some(&["yes", "no", "cancel"]),
        "retrycancel" => Some(&["retry", "cancel"]),
        "abortretryignore" => Some(&["abort", "retry", "ignore"]),
        _ => None,
    }
}

pub(super) fn normalize_dialog_choice_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(super) fn resolve_messagebox_selection(
    dialog_type_raw: &str,
    default_raw: Option<&str>,
) -> Result<String, String> {
    let dialog_type = normalize_dialog_choice_name(dialog_type_raw);
    let Some(buttons) = messagebox_buttons_for_type(&dialog_type) else {
        return Err(format!(
            "bad -type value \"{dialog_type_raw}\": must be abortretryignore, ok, okcancel, retrycancel, yesno, or yesnocancel"
        ));
    };
    if let Some(default_name_raw) = default_raw {
        let default_name = normalize_dialog_choice_name(default_name_raw);
        if buttons.iter().any(|candidate| *candidate == default_name) {
            return Ok(default_name);
        }
        return Err(format!(
            "bad -default value \"{default_name_raw}\" for dialog type \"{dialog_type}\""
        ));
    }
    Ok(buttons[0].to_string())
}

pub(super) fn messagebox_icon_is_supported(icon: &str) -> bool {
    matches!(
        normalize_dialog_choice_name(icon).as_str(),
        "error" | "info" | "question" | "warning"
    )
}

pub(super) fn join_dialog_path(initial_dir: &str, initial_file: &str) -> String {
    if initial_file.is_empty() {
        return initial_dir.to_string();
    }
    if initial_dir.is_empty() {
        return initial_file.to_string();
    }
    if initial_dir.ends_with('/') || initial_dir.ends_with('\\') {
        return format!("{initial_dir}{initial_file}");
    }
    if initial_dir.ends_with(':') {
        return format!("{initial_dir}\\{initial_file}");
    }
    let sep = if initial_dir.contains('\\') && !initial_dir.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{initial_dir}{sep}{initial_file}")
}

pub(super) fn apply_default_extension(path: &str, default_extension: &str) -> String {
    let trimmed_ext = default_extension.trim();
    if path.is_empty() || trimmed_ext.is_empty() {
        return path.to_string();
    }
    if std::path::Path::new(path).extension().is_some() {
        return path.to_string();
    }
    if trimmed_ext.starts_with('.') {
        format!("{path}{trimmed_ext}")
    } else {
        format!("{path}.{trimmed_ext}")
    }
}

pub(super) fn normalize_color_literal(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && trimmed.len() == 4 {
        let mut chars = trimmed.chars();
        let _ = chars.next();
        let red = chars.next()?;
        let green = chars.next()?;
        let blue = chars.next()?;
        if !(red.is_ascii_hexdigit() && green.is_ascii_hexdigit() && blue.is_ascii_hexdigit()) {
            return None;
        }
        return Some(format!("#{}{}{}{}{}{}", red, red, green, green, blue, blue));
    }
    if trimmed.starts_with('#') && trimmed.len() == 7 {
        if !trimmed[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        return Some(trimmed.to_string());
    }
    Some(trimmed.to_string())
}

pub(super) fn parse_commondialog_command_options(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<Vec<(String, u64)>, u64> {
    parse_widget_option_pairs(py, handle, args, 1, "commondialog options")
}

pub(super) fn headless_commondialog_result(
    py: &PyToken,
    handle: i64,
    command: &str,
    options: &[(String, u64)],
) -> Result<u64, u64> {
    match command {
        "tk_messageBox" => {
            let dialog_type =
                commondialog_option_text(py, handle, options, "-type", "messagebox type option")?
                    .unwrap_or_else(|| "ok".to_string());
            let default_choice = commondialog_option_text(
                py,
                handle,
                options,
                "-default",
                "messagebox default option",
            )?;
            if let Some(icon_name) =
                commondialog_option_text(py, handle, options, "-icon", "messagebox icon option")?
                && !messagebox_icon_is_supported(icon_name.as_str())
            {
                return Err(raise_tcl_for_handle(
                    py,
                    handle,
                    format!(
                        "bad -icon value \"{icon_name}\": must be error, info, question, or warning"
                    ),
                ));
            }
            let selection = resolve_messagebox_selection(&dialog_type, default_choice.as_deref())
                .map_err(|message| raise_tcl_for_handle(py, handle, message))?;
            clear_last_error(handle);
            alloc_string_bits(py, &selection)
        }
        "tk_getOpenFile" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "filedialog initialdir option",
            )?
            .unwrap_or_default();
            let initial_file = commondialog_option_text(
                py,
                handle,
                options,
                "-initialfile",
                "filedialog initialfile option",
            )?
            .unwrap_or_default();
            let default_extension = commondialog_option_text(
                py,
                handle,
                options,
                "-defaultextension",
                "filedialog defaultextension option",
            )?
            .unwrap_or_default();
            let selected = apply_default_extension(
                join_dialog_path(initial_dir.as_str(), initial_file.as_str()).as_str(),
                default_extension.as_str(),
            );
            let multiple = commondialog_option_bool(
                py,
                handle,
                options,
                "-multiple",
                "filedialog multiple option",
            )?
            .unwrap_or(false);
            clear_last_error(handle);
            if multiple {
                let values = if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![selected]
                };
                alloc_tuple_from_strings(
                    py,
                    values.as_slice(),
                    "failed to allocate open-file selection tuple",
                )
            } else {
                alloc_string_bits(py, &selected)
            }
        }
        "tk_getSaveFile" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "filedialog initialdir option",
            )?
            .unwrap_or_default();
            let initial_file = commondialog_option_text(
                py,
                handle,
                options,
                "-initialfile",
                "filedialog initialfile option",
            )?
            .unwrap_or_default();
            let default_extension = commondialog_option_text(
                py,
                handle,
                options,
                "-defaultextension",
                "filedialog defaultextension option",
            )?
            .unwrap_or_default();
            let selected = apply_default_extension(
                join_dialog_path(initial_dir.as_str(), initial_file.as_str()).as_str(),
                default_extension.as_str(),
            );
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        "tk_chooseDirectory" => {
            let initial_dir = commondialog_option_text(
                py,
                handle,
                options,
                "-initialdir",
                "directory dialog initialdir option",
            )?
            .unwrap_or_default();
            let must_exist = commondialog_option_bool(
                py,
                handle,
                options,
                "-mustexist",
                "directory dialog mustexist option",
            )?
            .unwrap_or(false);
            let selected = if must_exist
                && !initial_dir.is_empty()
                && !std::path::Path::new(initial_dir.as_str()).is_dir()
            {
                String::new()
            } else {
                initial_dir
            };
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        "tk_chooseColor" => {
            let initial_color = commondialog_option_text(
                py,
                handle,
                options,
                "-initialcolor",
                "color chooser initialcolor option",
            )?;
            let selected = if let Some(color_name) = initial_color.as_deref() {
                let Some(normalized) = normalize_color_literal(color_name) else {
                    return Err(raise_tcl_for_handle(
                        py,
                        handle,
                        format!("invalid color name \"{color_name}\""),
                    ));
                };
                normalized
            } else {
                String::new()
            };
            clear_last_error(handle);
            alloc_string_bits(py, &selected)
        }
        _ => Err(raise_unsupported_commondialog_command(py, handle, command)),
    }
}

pub(super) fn handle_headless_commondialog_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    let command = get_string_arg(py, handle, args[0], "commondialog command")?;
    if !commondialog_is_supported_command(command.as_str()) {
        return Err(raise_unsupported_commondialog_command(
            py,
            handle,
            command.as_str(),
        ));
    }
    let options = parse_commondialog_command_options(py, handle, args)?;
    validate_commondialog_options(py, handle, command.as_str(), &options)?;
    headless_commondialog_result(py, handle, command.as_str(), &options)
}

pub(super) fn clamp_dialog_selection(default_index: i64, button_count: usize) -> i64 {
    if button_count == 0 {
        return 0;
    }
    let max_index = (button_count - 1) as i64;
    default_index.clamp(0, max_index)
}

pub(super) fn handle_headless_tk_dialog_command(
    py: &PyToken,
    handle: i64,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 6 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "tk_dialog expects window, title, text, bitmap, default, and optional button labels",
        ));
    }
    let default_index = parse_i64_arg(py, handle, args[5], "tk_dialog default index")?;
    let selected = clamp_dialog_selection(default_index, args.len().saturating_sub(6));
    clear_last_error(handle);
    Ok(MoltObject::from_int(selected).bits())
}

pub(super) fn filedialog_is_supported_command(command: &str) -> bool {
    matches!(
        command,
        "tk_getOpenFile" | "tk_getSaveFile" | "tk_chooseDirectory"
    )
}

pub(super) fn raise_unsupported_filedialog_command(
    py: &PyToken,
    handle: i64,
    command: &str,
) -> u64 {
    raise_tcl_for_handle(
        py,
        handle,
        format!("unsupported filedialog command \"{command}\""),
    )
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn ensure_native_tk_loaded_for_commondialog(
    py: &PyToken,
    handle: i64,
) -> Result<(), u64> {
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if app.tk_loaded {
        return Ok(());
    }
    let Some(interp) = app.interpreter.as_ref() else {
        return Err(app_tcl_error_locked(
            py,
            app,
            "tk runtime interpreter is unavailable",
        ));
    };
    match interp.eval(("package", "require", "Tk")) {
        Ok(_) => {
            app.tk_loaded = true;
            Ok(())
        }
        Err(err) => Err(app_tcl_error_locked(
            py,
            app,
            format!("failed to load Tk package: {err}"),
        )),
    }
}

#[cfg(any(target_arch = "wasm32", not(feature = "native-tcl")))]
pub(super) fn ensure_native_tk_loaded_for_commondialog(
    _py: &PyToken,
    _handle: i64,
) -> Result<(), u64> {
    Ok(())
}

pub(super) fn dispatch_commondialog_via_tk_call(
    py: &PyToken,
    handle: i64,
    master_path: &str,
    command: &str,
    options: &[(String, u64)],
) -> Result<u64, u64> {
    validate_commondialog_options(py, handle, command, options)?;
    ensure_native_tk_loaded_for_commondialog(py, handle)?;

    let inject_parent = !master_path.is_empty()
        && commondialog_supports_parent(command)
        && commondialog_option_value_bits(options, "-parent").is_none();
    let mut argv = Vec::with_capacity(1 + options.len() * 2 + usize::from(inject_parent) * 2);
    let mut allocated = Vec::with_capacity(1 + options.len() + usize::from(inject_parent) * 2);

    let alloc_and_push =
        |value: &str, allocated: &mut Vec<u64>, argv: &mut Vec<u64>| -> Result<(), u64> {
            let bits = alloc_string_bits(py, value)?;
            allocated.push(bits);
            argv.push(bits);
            Ok(())
        };

    if let Err(bits) = alloc_and_push(command, &mut allocated, &mut argv) {
        for owned_bits in allocated {
            dec_ref_bits(py, owned_bits);
        }
        return Err(bits);
    }

    if inject_parent {
        if let Err(bits) = alloc_and_push("-parent", &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
        if let Err(bits) = alloc_and_push(master_path, &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
    }

    for (name, value_bits) in options {
        if let Err(bits) = alloc_and_push(name, &mut allocated, &mut argv) {
            for owned_bits in allocated {
                dec_ref_bits(py, owned_bits);
            }
            return Err(bits);
        }
        argv.push(*value_bits);
    }

    let out = tk_call_dispatch(py, handle, &argv);
    for bits in allocated {
        dec_ref_bits(py, bits);
    }
    out
}

pub(super) fn parse_simpledialog_i64(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok()
}

pub(super) fn parse_simpledialog_f64(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn app_interp_eval_list(
    py: &PyToken,
    app: &mut TkAppState,
    words: Vec<String>,
) -> Result<TclObj, u64> {
    let eval_result = {
        let Some(interp) = app.interpreter.as_ref() else {
            return Err(app_tcl_error_locked(
                py,
                app,
                "tk runtime interpreter is unavailable",
            ));
        };
        interp.eval(TclObj::new_list(words.into_iter().map(TclObj::from)))
    };
    eval_result.map_err(|err| app_tcl_error_locked(py, app, format!("tk command failed: {err}")))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn cleanup_native_simpledialog(
    py: &PyToken,
    app: &mut TkAppState,
    dialog_path: &str,
    state_var: &str,
) {
    let _ = app_interp_eval_list(
        py,
        app,
        vec![
            "grab".to_string(),
            "release".to_string(),
            dialog_path.to_string(),
        ],
    );
    let _ = app_interp_eval_list(
        py,
        app,
        vec!["destroy".to_string(), dialog_path.to_string()],
    );
    let _ = app_interp_eval_list(py, app, vec!["unset".to_string(), state_var.to_string()]);
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tk_dispatch_string_command(
    py: &PyToken,
    handle: i64,
    args: &[String],
) -> Result<u64, u64> {
    let mut arg_bits = Vec::with_capacity(args.len());
    for arg in args {
        match alloc_string_bits(py, arg) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                for allocated in arg_bits {
                    dec_ref_bits(py, allocated);
                }
                return Err(bits);
            }
        }
    }
    let out = tk_call_dispatch(py, handle, &arg_bits);
    for allocated in arg_bits {
        dec_ref_bits(py, allocated);
    }
    out
}
