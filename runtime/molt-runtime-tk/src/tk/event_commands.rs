use super::args::{clear_last_error, get_string_arg, get_text_arg, raise_tcl_for_handle};
use super::dispatch::tk_call_dispatch;
use super::parsing::{
    alloc_tuple_from_strings, parse_bool_text, parse_tcl_script_commands, parse_treeview_tags,
    tk_widget_class_name,
};
use super::state::{
    TkAppState, TkTreeviewState, alloc_string_bits, app_mut_from_registry, app_tcl_error_locked,
    tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new};
use crate::bridge::{dec_ref_bits, decode_value_list, string_obj_to_owned, to_f64, to_i64};
use molt_runtime_core::prelude::{MoltObject, PyToken, obj_from_bits};
use std::collections::HashMap;

pub(super) fn default_bindtags_for_target(app: &TkAppState, target_name: &str) -> Vec<String> {
    if target_name == "." {
        return vec![".".to_string(), "Tk".to_string(), "all".to_string()];
    }
    if target_name == "all" {
        return vec!["all".to_string()];
    }
    if let Some(widget) = app.widgets.get(target_name) {
        return vec![
            target_name.to_string(),
            tk_widget_class_name(&widget.widget_command),
            ".".to_string(),
            "all".to_string(),
        ];
    }
    vec![target_name.to_string()]
}

pub(super) fn handle_bind_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 || args.len() > 4 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "bind expects target, optional sequence, optional script",
        ));
    }
    let target_name = get_string_arg(py, handle, args[1], "bind target")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;

    if args.len() == 2 {
        let mut sequences: Vec<String> = app
            .bind_scripts
            .get(&target_name)
            .map(|scripts| scripts.keys().cloned().collect())
            .unwrap_or_default();
        sequences.sort_unstable();
        app.last_error = None;
        return alloc_tuple_from_strings(py, sequences.as_slice(), "failed to allocate bind tuple");
    }

    let sequence = get_string_arg(py, handle, args[2], "bind sequence")?;
    if args.len() == 3 {
        let script = app
            .bind_scripts
            .get(&target_name)
            .and_then(|scripts| scripts.get(&sequence))
            .cloned()
            .unwrap_or_default();
        app.last_error = None;
        return alloc_string_bits(py, &script);
    }

    let script = get_string_arg(py, handle, args[3], "bind script")?;
    let scripts = app.bind_scripts.entry(target_name).or_default();
    if script.is_empty() {
        scripts.remove(&sequence);
    } else if script.starts_with('+') {
        let merged = if let Some(previous) = scripts.get(&sequence) {
            if previous.trim().is_empty() {
                script
            } else {
                format!("{previous}\n{script}")
            }
        } else {
            script
        };
        scripts.insert(sequence, merged);
    } else {
        scripts.insert(sequence, script);
    }
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

pub(super) fn handle_bindtags_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() != 2 && args.len() != 3 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "bindtags expects target and optional tag list",
        ));
    }
    let target_name = get_string_arg(py, handle, args[1], "bindtags target")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if args.len() == 2 {
        let tags = app
            .bindtags
            .get(&target_name)
            .cloned()
            .unwrap_or_else(|| default_bindtags_for_target(app, &target_name));
        app.last_error = None;
        return alloc_tuple_from_strings(py, tags.as_slice(), "failed to allocate bindtags tuple");
    }

    let tag_values = if let Some(raw) = decode_value_list(obj_from_bits(args[2])) {
        let mut tags = Vec::with_capacity(raw.len());
        for tag_bits in raw {
            tags.push(get_string_arg(py, handle, tag_bits, "bindtags tag")?);
        }
        tags
    } else {
        vec![get_string_arg(py, handle, args[2], "bindtags tag list")?]
    };
    app.bindtags.insert(target_name, tag_values);
    app.last_error = None;
    Ok(MoltObject::none().bits())
}

pub(super) fn parse_event_generate_options(
    py: &PyToken,
    handle: i64,
    args: &[u64],
    start_index: usize,
) -> Result<HashMap<String, String>, u64> {
    let mut options = HashMap::new();
    if start_index >= args.len() {
        return Ok(options);
    }
    let tail_len = args.len() - start_index;
    if !tail_len.is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "event generate option list must contain key/value pairs",
        ));
    }
    let mut index = start_index;
    while index < args.len() {
        let name = get_string_arg(py, handle, args[index], "event option name")?;
        let value = get_text_arg(py, handle, args[index + 1], "event option value")?;
        options.insert(name.to_ascii_lowercase(), value);
        index += 2;
    }
    Ok(options)
}

pub(super) fn event_generate_type_name(sequence: &str) -> String {
    if sequence.starts_with("<<") && sequence.ends_with(">>") && sequence.len() >= 4 {
        return "VirtualEvent".to_string();
    }
    if sequence.starts_with('<') && sequence.ends_with('>') && sequence.len() >= 2 {
        let inner = &sequence[1..sequence.len() - 1];
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    sequence.to_string()
}

pub(super) fn event_generate_placeholder_value(
    placeholder: &str,
    target_path: &str,
    sequence: &str,
    options: &HashMap<String, String>,
) -> Option<String> {
    let fallback_xy = options
        .get("-x")
        .cloned()
        .or_else(|| options.get("-rootx").cloned())
        .unwrap_or_else(|| "0".to_string());
    let fallback_yy = options
        .get("-y")
        .cloned()
        .or_else(|| options.get("-rooty").cloned())
        .unwrap_or_else(|| "0".to_string());
    let value = match placeholder {
        "%#" => options
            .get("-serial")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%b" => options
            .get("-button")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%f" => options
            .get("-focus")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%h" => options
            .get("-height")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%k" => options
            .get("-keycode")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%s" => options
            .get("-state")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%t" => options
            .get("-time")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%w" => options
            .get("-width")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%x" => options
            .get("-x")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%y" => options
            .get("-y")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%A" => options
            .get("-char")
            .cloned()
            .or_else(|| options.get("-data").cloned())
            .unwrap_or_default(),
        "%E" => options
            .get("-sendevent")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        "%K" => options.get("-keysym").cloned().unwrap_or_default(),
        "%N" => options
            .get("-keysym_num")
            .cloned()
            .or_else(|| options.get("-keycode").cloned())
            .unwrap_or_else(|| "0".to_string()),
        "%W" => target_path.to_string(),
        "%T" => event_generate_type_name(sequence),
        "%X" => options.get("-rootx").cloned().unwrap_or(fallback_xy),
        "%Y" => options.get("-rooty").cloned().unwrap_or(fallback_yy),
        "%D" => options
            .get("-delta")
            .cloned()
            .unwrap_or_else(|| "0".to_string()),
        _ => return None,
    };
    Some(value)
}

pub(super) fn parse_bind_script_commands(script: &str) -> Vec<Vec<String>> {
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let extracted = if trimmed.starts_with("if ") {
        if let Some(open_idx) = trimmed.find('[') {
            if let Some(close_rel) = trimmed[open_idx + 1..].find(']') {
                trimmed[open_idx + 1..open_idx + 1 + close_rel].trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let command = extracted.trim_start_matches('+').trim();
    if command.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with("if ") {
        return parse_tcl_script_commands(command)
            .into_iter()
            .next()
            .map(|words| vec![words])
            .unwrap_or_default();
    }
    parse_tcl_script_commands(command)
}

pub(super) const TK_EVENT_SUBST_FIELD_COUNT: usize = 19;

pub(super) fn flatten_event_subst_arg(mut value_bits: u64) -> u64 {
    for _ in 0..8 {
        let Some(values) = decode_value_list(obj_from_bits(value_bits)) else {
            break;
        };
        if values.len() != 1 {
            break;
        }
        value_bits = values[0];
    }
    value_bits
}

pub(super) fn parse_event_subst_i64(value_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(value_bits);
    if let Some(value) = to_i64(obj) {
        return Some(value);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return text.trim().parse::<i64>().ok();
    }
    if let Some(value) = to_f64(obj)
        && value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return Some(value as i64);
    }
    None
}

pub(super) fn normalize_event_subst_int_field(value_bits: u64) -> u64 {
    parse_event_subst_i64(value_bits)
        .map(MoltObject::from_int)
        .map(MoltObject::bits)
        .unwrap_or(value_bits)
}

pub(super) fn normalize_event_subst_bool_field(value_bits: u64) -> u64 {
    let obj = obj_from_bits(value_bits);
    let parsed = if obj.is_bool() {
        obj.as_bool()
    } else if let Some(value) = to_i64(obj) {
        Some(value != 0)
    } else if let Some(text) = string_obj_to_owned(obj) {
        parse_bool_text(&text)
    } else {
        to_f64(obj).map(|value| value != 0.0)
    };
    parsed
        .map(MoltObject::from_bool)
        .map(MoltObject::bits)
        .unwrap_or_else(|| MoltObject::none().bits())
}

pub(super) fn event_subst_value_is_empty(value_bits: u64) -> bool {
    let obj = obj_from_bits(value_bits);
    if obj.is_none() {
        return true;
    }
    string_obj_to_owned(obj).is_some_and(|value| value.is_empty())
}

pub(super) fn normalize_event_subst_delta_field(value_bits: u64) -> u64 {
    if let Some(value) = parse_event_subst_i64(value_bits) {
        return MoltObject::from_int(value).bits();
    }
    if event_subst_value_is_empty(value_bits) {
        return MoltObject::from_int(0).bits();
    }
    value_bits
}

pub(super) fn bind_script_line_invokes_command(line: &str, command_name: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    let normalized = trimmed.trim_start_matches('+').trim_start();
    if normalized.starts_with(command_name)
        && normalized[command_name.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    {
        return true;
    }

    let wrapped_prefix = format!("[{command_name} ");
    let wrapped_exact = format!("[{command_name}]");
    if normalized.starts_with("if ")
        && (normalized.contains(&wrapped_prefix) || normalized.contains(&wrapped_exact))
    {
        return true;
    }

    parse_bind_script_commands(normalized)
        .into_iter()
        .any(|words| {
            let Some(first) = words.first() else {
                return false;
            };
            let first = first.trim_start_matches('+');
            if first == command_name {
                return true;
            }
            first == "if"
                && words.iter().any(|word| {
                    word.contains(wrapped_prefix.as_str()) || word.contains(wrapped_exact.as_str())
                })
        })
}

pub(super) fn remove_bind_script_command_invocations(script: &str, command_name: &str) -> String {
    if script.is_empty() || command_name.is_empty() {
        return script.to_string();
    }
    let mut out = String::with_capacity(script.len());
    for segment in script.split_inclusive('\n') {
        let (line, ending) = match segment.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (segment, ""),
        };
        let parse_line = line.strip_suffix('\r').unwrap_or(line);
        if bind_script_line_invokes_command(parse_line, command_name) {
            continue;
        }
        out.push_str(line);
        out.push_str(ending);
    }
    if out.trim().is_empty() {
        return String::new();
    }
    out
}

pub(super) fn event_generate_binding_sequences(app: &TkAppState, sequence: &str) -> Vec<String> {
    let mut sequences = vec![sequence.to_string()];
    if !(sequence.starts_with("<<") && sequence.ends_with(">>")) {
        for (virtual_name, physical_sequences) in &app.virtual_events {
            if physical_sequences.iter().any(|name| name == sequence)
                && !sequences.iter().any(|name| name == virtual_name)
            {
                sequences.push(virtual_name.clone());
            }
        }
    }
    sequences
}

pub(super) fn build_event_generate_commands(
    app: &TkAppState,
    target_path: &str,
    sequence: &str,
    binding_sequences: &[String],
    options: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let tags = app
        .bindtags
        .get(target_path)
        .cloned()
        .unwrap_or_else(|| default_bindtags_for_target(app, target_path));

    let mut out = Vec::new();
    for tag in tags {
        let Some(bindings) = app.bind_scripts.get(&tag) else {
            continue;
        };
        for binding_sequence in binding_sequences {
            let Some(script) = bindings.get(binding_sequence) else {
                continue;
            };
            for mut words in parse_bind_script_commands(script) {
                if words.is_empty() {
                    continue;
                }
                for word in &mut words {
                    if let Some(substituted) =
                        event_generate_placeholder_value(word, target_path, sequence, options)
                    {
                        *word = substituted;
                    }
                }
                out.push(words);
            }
        }
    }
    out
}

pub(super) fn treeview_event_target_item(
    treeview: &TkTreeviewState,
    options: &HashMap<String, String>,
) -> Option<String> {
    if let Some(item) = options
        .get("-item")
        .or_else(|| options.get("-iid"))
        .filter(|candidate| !candidate.is_empty())
        && treeview.items.contains_key(item.as_str())
    {
        return Some(item.clone());
    }
    if let Some(focus) = treeview
        .focus
        .as_deref()
        .filter(|candidate| treeview.items.contains_key(*candidate))
    {
        return Some(focus.to_string());
    }
    treeview
        .selection
        .iter()
        .find(|candidate| treeview.items.contains_key(candidate.as_str()))
        .cloned()
}

pub(super) fn build_treeview_tag_event_commands(
    app: &TkAppState,
    target_path: &str,
    sequence: &str,
    binding_sequences: &[String],
    options: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let Some(treeview) = app
        .widgets
        .get(target_path)
        .and_then(|widget| widget.treeview.as_ref())
    else {
        return Vec::new();
    };
    let Some(item_id) = treeview_event_target_item(treeview, options) else {
        return Vec::new();
    };
    let Some(item) = treeview.items.get(&item_id) else {
        return Vec::new();
    };
    let item_tags = parse_treeview_tags(item);
    if item_tags.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for tag_name in item_tags {
        let Some(tag_state) = treeview.tags.get(&tag_name) else {
            continue;
        };
        for binding_sequence in binding_sequences {
            let Some(script) = tag_state.bindings.get(binding_sequence) else {
                continue;
            };
            for mut words in parse_bind_script_commands(script) {
                if words.is_empty() {
                    continue;
                }
                for word in &mut words {
                    if let Some(substituted) =
                        event_generate_placeholder_value(word, target_path, sequence, options)
                    {
                        *word = substituted;
                    }
                }
                out.push(words);
            }
        }
    }
    out
}

pub(super) fn handle_event_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "event requires a subcommand",
        ));
    }
    let subcommand = get_string_arg(py, handle, args[1], "event subcommand")?;
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    match subcommand.as_str() {
        "add" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event add expects virtual event and sequences",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            let sequences = app.virtual_events.entry(virtual_name).or_default();
            for &sequence_bits in &args[3..] {
                let sequence = get_string_arg(py, handle, sequence_bits, "event sequence")?;
                if !sequences.iter().any(|existing| existing == &sequence) {
                    sequences.push(sequence);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "delete" => {
            if args.len() < 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event delete expects virtual event name",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            if args.len() == 3 {
                app.virtual_events.remove(&virtual_name);
            } else if let Some(sequences) = app.virtual_events.get_mut(&virtual_name) {
                for &sequence_bits in &args[3..] {
                    let sequence = get_string_arg(py, handle, sequence_bits, "event sequence")?;
                    sequences.retain(|existing| existing != &sequence);
                }
                if sequences.is_empty() {
                    app.virtual_events.remove(&virtual_name);
                }
            }
            app.last_error = None;
            Ok(MoltObject::none().bits())
        }
        "generate" => {
            if args.len() < 4 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event generate expects widget path and sequence",
                ));
            }
            let target_path = get_string_arg(py, handle, args[2], "event target widget")?;
            let sequence = get_string_arg(py, handle, args[3], "event sequence")?;
            let options = parse_event_generate_options(py, handle, args, 4)?;
            let binding_sequences = event_generate_binding_sequences(app, &sequence);
            let mut command_lines = build_event_generate_commands(
                app,
                &target_path,
                &sequence,
                &binding_sequences,
                &options,
            );
            let mut tree_tag_command_lines = build_treeview_tag_event_commands(
                app,
                &target_path,
                &sequence,
                &binding_sequences,
                &options,
            );
            command_lines.append(&mut tree_tag_command_lines);
            app.last_error = None;
            drop(registry);

            for words in command_lines {
                let mut argv = Vec::with_capacity(words.len());
                for word in &words {
                    match alloc_string_bits(py, word) {
                        Ok(bits) => argv.push(bits),
                        Err(bits) => {
                            for owned in argv {
                                dec_ref_bits(py, owned);
                            }
                            return Err(bits);
                        }
                    }
                }
                let dispatch_out = tk_call_dispatch(py, handle, &argv);
                for owned in argv {
                    dec_ref_bits(py, owned);
                }
                let out_bits = dispatch_out?;
                let should_break = string_obj_to_owned(obj_from_bits(out_bits))
                    .is_some_and(|value| value == "break");
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(py, out_bits);
                }
                if should_break {
                    break;
                }
            }
            clear_last_error(handle);
            Ok(MoltObject::none().bits())
        }
        "info" => {
            if args.len() == 2 {
                let mut names: Vec<String> = app.virtual_events.keys().cloned().collect();
                names.sort_unstable();
                app.last_error = None;
                return alloc_tuple_from_strings(
                    py,
                    names.as_slice(),
                    "failed to allocate event info tuple",
                );
            }
            if args.len() != 3 {
                return Err(app_tcl_error_locked(
                    py,
                    app,
                    "event info expects optional virtual event name",
                ));
            }
            let virtual_name = get_string_arg(py, handle, args[2], "virtual event name")?;
            let sequences = app
                .virtual_events
                .get(&virtual_name)
                .cloned()
                .unwrap_or_default();
            app.last_error = None;
            alloc_tuple_from_strings(py, sequences.as_slice(), "failed to allocate event tuple")
        }
        _ => Err(app_tcl_error_locked(
            py,
            app,
            format!("bad event option \"{subcommand}\": must be add, delete, generate, or info"),
        )),
    }
}
