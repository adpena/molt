use std::collections::HashMap;

use molt_obj_model::MoltObject;

use super::functions::alloc_string_bits;
use super::functions_http::urllib_request_attr_optional;
use super::functions_pickle::pickle_resolve_global_bits;
use crate::{
    alloc_string, attr_name_bits_from_bytes, builtin_classes, call_callable0, call_callable1,
    call_class_init_with_args, clear_exception, dec_ref_bits, dict_get_in_place,
    exception_pending, inc_ref_bits, missing_bits, molt_getattr_builtin, obj_from_bits,
    object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64,
    TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_TUPLE,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

fn logging_percent_lookup_mapping_value(
    _py: &crate::PyToken<'_>,
    mapping_ptr: *mut u8,
    key: &str,
) -> Option<u64> {
    let key_ptr = alloc_string(_py, key.as_bytes());
    if key_ptr.is_null() {
        return None;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let value = unsafe { dict_get_in_place(_py, mapping_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    value
}

fn logging_percent_render_str(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_str_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_repr(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_repr_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_value(
    _py: &crate::PyToken<'_>,
    spec: char,
    value_bits: u64,
) -> Option<String> {
    match spec {
        'd' => {
            if let Some(value) = to_i64(obj_from_bits(value_bits)) {
                return Some(value.to_string());
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'f' => {
            if let Some(value) = to_f64(obj_from_bits(value_bits)) {
                return Some(format!("{value:.6}"));
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'r' => logging_percent_render_repr(_py, value_bits),
        _ => logging_percent_render_str(_py, value_bits),
    }
}

fn logging_config_dict_lookup(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
    key: &str,
) -> Result<Option<u64>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    }
    let Some(key_bits) = alloc_string_bits(_py, key) else {
        return Err(MoltObject::none().bits());
    };
    let value = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value)
}

fn logging_config_dict_items(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
) -> Result<Vec<(u64, u64)>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(dict_bits, items_name_bits, missing);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section missing items()",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, iterable_bits) };
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() must produce an iterable of pairs",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() iterable materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    for item_bits in entries {
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        };
        if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be key/value pairs",
            ));
        }
        let key_bits = fields[0];
        let value_bits = fields[1];
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, value_bits);
        pairs.push((key_bits, value_bits));
    }
    dec_ref_bits(_py, list_bits);
    Ok(pairs)
}

fn logging_config_name_list(_py: &crate::PyToken<'_>, seq_bits: u64) -> Result<Vec<String>, u64> {
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, seq_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list must be iterable",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut names: Vec<String> = Vec::new();
    for item_bits in entries {
        let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config handler references must be strings",
            ));
        };
        names.push(name);
    }
    dec_ref_bits(_py, list_bits);
    Ok(names)
}

fn logging_config_call_method1(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    method_name: &[u8],
    arg_bits: u64,
) -> Result<u64, u64> {
    let Some(method_bits) = urllib_request_attr_optional(_py, obj_bits, method_name)? else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "logging object method is missing",
        ));
    };
    let out_bits = unsafe { call_callable1(_py, method_bits, arg_bits) };
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out_bits)
}

fn logging_config_clear_logger_handlers(
    _py: &crate::PyToken<'_>,
    logger_bits: u64,
) -> Result<(), u64> {
    let Some(handlers_bits) = urllib_request_attr_optional(_py, logger_bits, b"handlers")? else {
        return Ok(());
    };
    let Some(handlers_ptr) = obj_from_bits(handlers_bits).as_ptr() else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    let ty = unsafe { object_type_id(handlers_ptr) };
    let snapshot: Vec<u64> = if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
        unsafe { seq_vec_ref(handlers_ptr).to_vec() }
    } else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    dec_ref_bits(_py, handlers_bits);
    for handler_bits in snapshot {
        let out_bits =
            logging_config_call_method1(_py, logger_bits, b"removeHandler", handler_bits)?;
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
    }
    Ok(())
}

fn logging_config_resolve_ext_stream(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
) -> Result<u64, u64> {
    let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
        return Ok(value_bits);
    };
    if text == "ext://sys.stdout" {
        return pickle_resolve_global_bits(_py, "sys", "stdout");
    }
    if text == "ext://sys.stderr" {
        return pickle_resolve_global_bits(_py, "sys", "stderr");
    }
    if text == "ext://sys.stdin" {
        return pickle_resolve_global_bits(_py, "sys", "stdin");
    }
    Err(raise_exception::<u64>(
        _py,
        "ValueError",
        "unsupported logging stream ext target",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_dict(config_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let version_bits = match logging_config_dict_lookup(_py, config_bits, "version") {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                return raise_exception::<_>(_py, "ValueError", "logging config missing version");
            }
            Err(bits) => return bits,
        };
        let Some(version) = to_i64(obj_from_bits(version_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging config version must be int");
        };
        if version != 1 {
            return raise_exception::<_>(_py, "ValueError", "unsupported logging config version");
        }

        let formatter_class_bits = match pickle_resolve_global_bits(_py, "logging", "Formatter") {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let stream_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "StreamHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let file_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "FileHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let get_logger_bits = match pickle_resolve_global_bits(_py, "logging", "getLogger") {
            Ok(bits) => bits,
            Err(bits) => {
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return bits;
            }
        };

        let mut formatter_map: HashMap<String, u64> = HashMap::new();
        let mut handler_map: HashMap<String, u64> = HashMap::new();

        if let Ok(Some(formatters_bits)) =
            logging_config_dict_lookup(_py, config_bits, "formatters")
        {
            let pairs = match logging_config_dict_items(_py, formatters_bits) {
                Ok(items) => items,
                Err(bits) => {
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(formatter_class_ptr) = obj_from_bits(formatter_class_bits).as_ptr() else {
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.Formatter is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter name must be str",
                        );
                    }
                };
                let fmt_bits = match logging_config_dict_lookup(_py, cfg_bits, "format") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => MoltObject::none().bits(),
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let formatter_bits =
                    unsafe { call_class_init_with_args(_py, formatter_class_ptr, &[fmt_bits]) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                formatter_map.insert(name, formatter_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(handlers_bits)) = logging_config_dict_lookup(_py, config_bits, "handlers") {
            let pairs = match logging_config_dict_items(_py, handlers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(stream_handler_class_ptr) = obj_from_bits(stream_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.StreamHandler is invalid");
            };
            let Some(file_handler_class_ptr) = obj_from_bits(file_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.FileHandler is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler name must be str",
                        );
                    }
                };
                let class_bits = match logging_config_dict_lookup(_py, cfg_bits, "class") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "logging handler config missing class",
                        );
                    }
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let class_name = match string_obj_to_owned(obj_from_bits(class_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler class must be str",
                        );
                    }
                };
                let handler_bits = if class_name == "logging.StreamHandler" {
                    let stream_arg_bits = match logging_config_dict_lookup(_py, cfg_bits, "stream")
                    {
                        Ok(Some(bits)) => match logging_config_resolve_ext_stream(_py, bits) {
                            Ok(resolved_bits) => resolved_bits,
                            Err(err_bits) => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return err_bits;
                            }
                        },
                        Ok(None) => MoltObject::none().bits(),
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    unsafe {
                        call_class_init_with_args(_py, stream_handler_class_ptr, &[stream_arg_bits])
                    }
                } else if class_name == "logging.FileHandler" {
                    let filename_bits = match logging_config_dict_lookup(_py, cfg_bits, "filename")
                    {
                        Ok(Some(bits)) => bits,
                        Ok(None) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "logging FileHandler config missing filename",
                            );
                        }
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let mode_bits = match logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        Ok(Some(bits)) => bits,
                        Ok(None) => match alloc_string_bits(_py, "a") {
                            Some(bits) => bits,
                            None => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return MoltObject::none().bits();
                            }
                        },
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let out_bits = unsafe {
                        call_class_init_with_args(
                            _py,
                            file_handler_class_ptr,
                            &[filename_bits, mode_bits],
                        )
                    };
                    if let Ok(None) = logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        dec_ref_bits(_py, mode_bits);
                    }
                    out_bits
                } else {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "unsupported logging handler class for intrinsic dictConfig",
                    );
                };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                if let Ok(Some(formatter_name_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "formatter")
                {
                    let Some(formatter_name) =
                        string_obj_to_owned(obj_from_bits(formatter_name_bits))
                    else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter reference must be str",
                        );
                    };
                    let Some(formatter_bits) = formatter_map.get(&formatter_name).copied() else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown formatter in logging handler config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setFormatter",
                        formatter_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                handler_map.insert(name, handler_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(loggers_bits)) = logging_config_dict_lookup(_py, config_bits, "loggers") {
            let pairs = match logging_config_dict_items(_py, loggers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, handler_bits) in handler_map {
                        dec_ref_bits(_py, handler_bits);
                    }
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            for (name_bits, cfg_bits) in pairs {
                let logger_bits = unsafe { call_callable1(_py, get_logger_bits, name_bits) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Err(bits) = logging_config_clear_logger_handlers(_py, logger_bits) {
                    dec_ref_bits(_py, logger_bits);
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return bits;
                }
                if let Ok(Some(handler_list_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "handlers")
                {
                    let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                        Ok(value) => value,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    for handler_name in handler_names {
                        let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "unknown handler in logger config",
                            );
                        };
                        let out_bits = match logging_config_call_method1(
                            _py,
                            logger_bits,
                            b"addHandler",
                            handler_bits,
                        ) {
                            Ok(bits) => bits,
                            Err(bits) => {
                                dec_ref_bits(_py, logger_bits);
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return bits;
                            }
                        };
                        if !obj_from_bits(out_bits).is_none() {
                            dec_ref_bits(_py, out_bits);
                        }
                    }
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        logger_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                dec_ref_bits(_py, logger_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, handler_bits) in handler_map {
                dec_ref_bits(_py, handler_bits);
            }
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(root_bits)) = logging_config_dict_lookup(_py, config_bits, "root") {
            let root_logger_bits = unsafe { call_callable0(_py, get_logger_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Err(bits) = logging_config_clear_logger_handlers(_py, root_logger_bits) {
                dec_ref_bits(_py, root_logger_bits);
                return bits;
            }
            if let Ok(Some(handler_list_bits)) =
                logging_config_dict_lookup(_py, root_bits, "handlers")
            {
                let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                    Ok(value) => value,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                for handler_name in handler_names {
                    let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                        dec_ref_bits(_py, root_logger_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown handler in root logger config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        root_logger_bits,
                        b"addHandler",
                        handler_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, root_logger_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
            }
            if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, root_bits, "level") {
                let out_bits = match logging_config_call_method1(
                    _py,
                    root_logger_bits,
                    b"setLevel",
                    level_bits,
                ) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
            }
            dec_ref_bits(_py, root_logger_bits);
        } else if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        for (_, handler_bits) in handler_map {
            dec_ref_bits(_py, handler_bits);
        }
        for (_, formatter_bits) in formatter_map {
            dec_ref_bits(_py, formatter_bits);
        }
        dec_ref_bits(_py, get_logger_bits);
        dec_ref_bits(_py, file_handler_class_bits);
        dec_ref_bits(_py, stream_handler_class_bits);
        dec_ref_bits(_py, formatter_class_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_valid_ident(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "logging.config.valid_ident expects str",
            );
        };
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return MoltObject::from_bool(false).bits();
        };
        let first_ok = first == '_' || first.is_ascii_alphabetic();
        if !first_ok {
            return MoltObject::from_bool(false).bits();
        }
        for ch in chars {
            if ch != '_' && !ch.is_ascii_alphanumeric() {
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_file_config(
    config_file_bits: u64,
    defaults_bits: u64,
    disable_existing_loggers_bits: u64,
    encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (
            config_file_bits,
            defaults_bits,
            disable_existing_loggers_bits,
            encoding_bits,
        );
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.fileConfig is not implemented in Molt yet",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_listen(port_bits: u64, verify_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (port_bits, verify_bits);
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.listen is not implemented in Molt yet",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_stop_listening() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_percent_style_format(fmt_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fmt) = string_obj_to_owned(obj_from_bits(fmt_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging format string must be str");
        };
        let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        };
        if unsafe { object_type_id(mapping_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        }

        let chars: Vec<char> = fmt.chars().collect();
        let mut out = String::with_capacity(fmt.len());
        let mut idx = 0usize;

        while idx < chars.len() {
            let ch = chars[idx];
            if ch != '%' {
                out.push(ch);
                idx += 1;
                continue;
            }
            if idx + 1 >= chars.len() {
                out.push('%');
                break;
            }
            if chars[idx + 1] == '%' {
                out.push('%');
                idx += 2;
                continue;
            }
            if chars[idx + 1] != '(' {
                out.push('%');
                idx += 1;
                continue;
            }
            let mut close = idx + 2;
            while close < chars.len() && chars[close] != ')' {
                close += 1;
            }
            if close >= chars.len() || close + 1 >= chars.len() {
                for ch in &chars[idx..] {
                    out.push(*ch);
                }
                break;
            }

            let spec = chars[close + 1];
            let token: String = chars[idx..=close + 1].iter().collect();
            if !matches!(spec, 's' | 'd' | 'r' | 'f') {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            }

            let key: String = chars[idx + 2..close].iter().collect();
            let Some(value_bits) =
                logging_percent_lookup_mapping_value(_py, mapping_ptr, key.as_str())
            else {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            };

            let Some(rendered) = logging_percent_render_value(_py, spec, value_bits) else {
                return MoltObject::none().bits();
            };
            out.push_str(rendered.as_str());
            idx = close + 2;
        }

        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
