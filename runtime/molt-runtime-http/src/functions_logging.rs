use std::collections::HashMap;

use molt_obj_model::MoltObject;

use molt_runtime_core::{ErrorIndicatorSet, OwnedRuntimeValue, obj_from_bits};

use crate::bridge::{
    alloc_string, alloc_string_bits, attr_name_bits_from_bytes, attr_optional, builtin_classes,
    call_callable0, call_callable1, call_class_init_with_args, clear_exception, dec_ref_bits,
    dict_get_in_place, exception_pending, missing_bits, molt_getattr_builtin, object_type_id,
    raise_exception, resolve_global_bits, seq_snapshot, string_obj_to_owned, to_f64, to_i64,
};

fn logging_percent_lookup_mapping_value(
    _py: &molt_runtime_core::CoreGilToken,
    mapping_ptr: *mut u8,
    key: &str,
) -> Result<Option<u64>, ErrorIndicatorSet> {
    let key_ptr = alloc_string(_py, key.as_bytes());
    if key_ptr.is_null() {
        return Err(ErrorIndicatorSet);
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let value = unsafe { dict_get_in_place(_py, mapping_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    value
}

fn logging_percent_render_str(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
) -> Option<String> {
    let rendered_bits = crate::bridge::molt_str_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_repr(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
) -> Option<String> {
    let rendered_bits = crate::bridge::molt_repr_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_value(
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if unsafe { object_type_id(dict_ptr) } != crate::bridge::type_id_dict() {
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
    value.map_err(|ErrorIndicatorSet| MoltObject::none().bits())
}

fn logging_config_dict_items<'py>(
    _py: &'py molt_runtime_core::CoreGilToken,
    dict_bits: u64,
) -> Result<Vec<(OwnedRuntimeValue<'py>, OwnedRuntimeValue<'py>)>, u64> {
    let _dict = OwnedRuntimeValue::retain(_py, dict_bits);
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != crate::bridge::type_id_dict() {
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
    let iterable_bits = call_callable0(_py, items_method_bits);
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let list_bits = call_callable1(_py, builtin_classes(_py, "list"), iterable_bits);
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
    if unsafe { object_type_id(list_ptr) } != crate::bridge::type_id_list() {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() iterable materialization failed",
        ));
    }
    let entries = unsafe { seq_snapshot(list_ptr) };
    let mut pairs = Vec::new();
    for item_bits in entries.iter().copied() {
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        };
        if unsafe { object_type_id(item_ptr) } != crate::bridge::type_id_tuple() {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        }
        let fields = unsafe { seq_snapshot(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be key/value pairs",
            ));
        }
        let key_bits = fields[0];
        let value_bits = fields[1];
        pairs.push((
            OwnedRuntimeValue::retain(_py, key_bits),
            OwnedRuntimeValue::retain(_py, value_bits),
        ));
    }
    dec_ref_bits(_py, list_bits);
    Ok(pairs)
}

fn logging_config_name_list(
    _py: &molt_runtime_core::CoreGilToken,
    seq_bits: u64,
) -> Result<Vec<String>, u64> {
    let list_bits = call_callable1(_py, builtin_classes(_py, "list"), seq_bits);
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
    if unsafe { object_type_id(list_ptr) } != crate::bridge::type_id_list() {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list materialization failed",
        ));
    }
    let entries = unsafe { seq_snapshot(list_ptr) };
    let mut names: Vec<String> = Vec::new();
    for item_bits in entries.iter().copied() {
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

fn logging_config_call_method1<'py>(
    _py: &'py molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    method_name: &[u8],
    arg_bits: u64,
) -> Result<OwnedRuntimeValue<'py>, u64> {
    let Some(method_bits) = attr_optional(_py, obj_bits, method_name)? else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "logging object method is missing",
        ));
    };
    let out_bits = call_callable1(_py, method_bits, arg_bits);
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(unsafe { OwnedRuntimeValue::from_owned_bits(_py, out_bits) })
}

fn logging_config_clear_logger_handlers(
    _py: &molt_runtime_core::CoreGilToken,
    logger_bits: u64,
) -> Result<(), u64> {
    let Some(handlers_bits) = attr_optional(_py, logger_bits, b"handlers")? else {
        return Ok(());
    };
    let Some(handlers_ptr) = obj_from_bits(handlers_bits).as_ptr() else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    let ty = unsafe { object_type_id(handlers_ptr) };
    let snapshot = if ty == crate::bridge::type_id_list() || ty == crate::bridge::type_id_tuple() {
        unsafe { seq_snapshot(handlers_ptr) }
    } else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    dec_ref_bits(_py, handlers_bits);
    for handler_bits in snapshot.iter().copied() {
        drop(logging_config_call_method1(
            _py,
            logger_bits,
            b"removeHandler",
            handler_bits,
        )?);
    }
    Ok(())
}

fn logging_config_resolve_ext_stream<'py>(
    _py: &'py molt_runtime_core::CoreGilToken,
    value: OwnedRuntimeValue<'py>,
) -> Result<OwnedRuntimeValue<'py>, u64> {
    let Some(text) = string_obj_to_owned(obj_from_bits(value.bits())) else {
        return Ok(value);
    };
    if text == "ext://sys.stdout" {
        return resolve_global_bits(_py, "sys", "stdout")
            .map(|bits| unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) });
    }
    if text == "ext://sys.stderr" {
        return resolve_global_bits(_py, "sys", "stderr")
            .map(|bits| unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) });
    }
    if text == "ext://sys.stdin" {
        return resolve_global_bits(_py, "sys", "stdin")
            .map(|bits| unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) });
    }
    Err(raise_exception::<u64>(
        _py,
        "ValueError",
        "unsupported logging stream ext target",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_dict(config_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let configure = || -> Result<(), u64> {
            let version_bits = logging_config_dict_lookup(_py, config_bits, "version")?
                .ok_or_else(|| {
                    raise_exception::<u64>(_py, "ValueError", "logging config missing version")
                })?;
            let version = to_i64(obj_from_bits(version_bits)).ok_or_else(|| {
                raise_exception::<u64>(_py, "TypeError", "logging config version must be int")
            })?;
            if version != 1 {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "unsupported logging config version",
                ));
            }
            let formatter_class = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    resolve_global_bits(_py, "logging", "Formatter")?,
                )
            };
            let stream_handler_class = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    resolve_global_bits(_py, "logging", "StreamHandler")?,
                )
            };
            let file_handler_class = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    resolve_global_bits(_py, "logging", "FileHandler")?,
                )
            };
            let get_logger = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    resolve_global_bits(_py, "logging", "getLogger")?,
                )
            };
            let mut formatter_map: HashMap<String, OwnedRuntimeValue<'_>> = HashMap::new();
            let mut handler_map: HashMap<String, OwnedRuntimeValue<'_>> = HashMap::new();

            if let Some(formatters_bits) =
                logging_config_dict_lookup(_py, config_bits, "formatters")?
            {
                let pairs = logging_config_dict_items(_py, formatters_bits)?;
                if obj_from_bits(formatter_class.bits()).as_ptr().is_none() {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "logging.Formatter is invalid",
                    ));
                }
                for (name, cfg) in pairs {
                    let name =
                        string_obj_to_owned(obj_from_bits(name.bits())).ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "logging formatter name must be str",
                            )
                        })?;
                    let fmt = OwnedRuntimeValue::retain(
                        _py,
                        logging_config_dict_lookup(_py, cfg.bits(), "format")?
                            .unwrap_or_else(|| MoltObject::none().bits()),
                    );
                    let bits =
                        call_class_init_with_args(_py, formatter_class.bits(), &[fmt.bits()]);
                    if exception_pending(_py) {
                        return Err(MoltObject::none().bits());
                    }
                    formatter_map.insert(name, unsafe {
                        OwnedRuntimeValue::from_owned_bits(_py, bits)
                    });
                }
            }
            if let Some(handlers_bits) = logging_config_dict_lookup(_py, config_bits, "handlers")? {
                let pairs = logging_config_dict_items(_py, handlers_bits)?;
                if obj_from_bits(stream_handler_class.bits())
                    .as_ptr()
                    .is_none()
                {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "logging.StreamHandler is invalid",
                    ));
                }
                if obj_from_bits(file_handler_class.bits()).as_ptr().is_none() {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "logging.FileHandler is invalid",
                    ));
                }
                for (name, cfg) in pairs {
                    let name =
                        string_obj_to_owned(obj_from_bits(name.bits())).ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "logging handler name must be str",
                            )
                        })?;
                    let class_bits = logging_config_dict_lookup(_py, cfg.bits(), "class")?
                        .ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "ValueError",
                                "logging handler config missing class",
                            )
                        })?;
                    let class_name =
                        string_obj_to_owned(obj_from_bits(class_bits)).ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "logging handler class must be str",
                            )
                        })?;
                    let bits = if class_name == "logging.StreamHandler" {
                        let stream = match logging_config_dict_lookup(_py, cfg.bits(), "stream")? {
                            Some(bits) => logging_config_resolve_ext_stream(
                                _py,
                                OwnedRuntimeValue::retain(_py, bits),
                            )?,
                            None => unsafe {
                                OwnedRuntimeValue::from_owned_bits(_py, MoltObject::none().bits())
                            },
                        };
                        call_class_init_with_args(
                            _py,
                            stream_handler_class.bits(),
                            &[stream.bits()],
                        )
                    } else if class_name == "logging.FileHandler" {
                        let filename_bits =
                            logging_config_dict_lookup(_py, cfg.bits(), "filename")?.ok_or_else(
                                || {
                                    raise_exception::<u64>(
                                        _py,
                                        "ValueError",
                                        "logging FileHandler config missing filename",
                                    )
                                },
                            )?;
                        let filename = OwnedRuntimeValue::retain(_py, filename_bits);
                        let mode = match logging_config_dict_lookup(_py, cfg.bits(), "mode")? {
                            Some(bits) => OwnedRuntimeValue::retain(_py, bits),
                            None => unsafe {
                                OwnedRuntimeValue::from_owned_bits(
                                    _py,
                                    alloc_string_bits(_py, "a")
                                        .ok_or_else(|| MoltObject::none().bits())?,
                                )
                            },
                        };
                        call_class_init_with_args(
                            _py,
                            file_handler_class.bits(),
                            &[filename.bits(), mode.bits()],
                        )
                    } else {
                        return Err(raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "unsupported logging handler class for intrinsic dictConfig",
                        ));
                    };
                    if exception_pending(_py) {
                        return Err(MoltObject::none().bits());
                    }
                    let handler = unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) };
                    if let Some(level_bits) = logging_config_dict_lookup(_py, cfg.bits(), "level")?
                    {
                        let level = OwnedRuntimeValue::retain(_py, level_bits);
                        drop(logging_config_call_method1(
                            _py,
                            handler.bits(),
                            b"setLevel",
                            level.bits(),
                        )?);
                    }
                    if let Some(formatter_name_bits) =
                        logging_config_dict_lookup(_py, cfg.bits(), "formatter")?
                    {
                        let formatter_name = string_obj_to_owned(obj_from_bits(
                            formatter_name_bits,
                        ))
                        .ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "logging formatter reference must be str",
                            )
                        })?;
                        let formatter = formatter_map.get(&formatter_name).ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "ValueError",
                                "unknown formatter in logging handler config",
                            )
                        })?;
                        drop(logging_config_call_method1(
                            _py,
                            handler.bits(),
                            b"setFormatter",
                            formatter.bits(),
                        )?);
                    }
                    handler_map.insert(name, handler);
                }
            }
            if let Some(loggers_bits) = logging_config_dict_lookup(_py, config_bits, "loggers")? {
                for (name, cfg) in logging_config_dict_items(_py, loggers_bits)? {
                    let bits = call_callable1(_py, get_logger.bits(), name.bits());
                    if exception_pending(_py) {
                        return Err(MoltObject::none().bits());
                    }
                    let logger = unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) };
                    logging_config_clear_logger_handlers(_py, logger.bits())?;
                    if let Some(handler_list_bits) =
                        logging_config_dict_lookup(_py, cfg.bits(), "handlers")?
                    {
                        let handler_list = OwnedRuntimeValue::retain(_py, handler_list_bits);
                        for name in logging_config_name_list(_py, handler_list.bits())? {
                            let handler = handler_map.get(&name).ok_or_else(|| {
                                raise_exception::<u64>(
                                    _py,
                                    "ValueError",
                                    "unknown handler in logger config",
                                )
                            })?;
                            drop(logging_config_call_method1(
                                _py,
                                logger.bits(),
                                b"addHandler",
                                handler.bits(),
                            )?);
                        }
                    }
                    if let Some(level_bits) = logging_config_dict_lookup(_py, cfg.bits(), "level")?
                    {
                        let level = OwnedRuntimeValue::retain(_py, level_bits);
                        drop(logging_config_call_method1(
                            _py,
                            logger.bits(),
                            b"setLevel",
                            level.bits(),
                        )?);
                    }
                }
            }
            if let Some(root_bits) = logging_config_dict_lookup(_py, config_bits, "root")? {
                let root = OwnedRuntimeValue::retain(_py, root_bits);
                let bits = call_callable0(_py, get_logger.bits());
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                let logger = unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) };
                logging_config_clear_logger_handlers(_py, logger.bits())?;
                if let Some(handler_list_bits) =
                    logging_config_dict_lookup(_py, root.bits(), "handlers")?
                {
                    let handler_list = OwnedRuntimeValue::retain(_py, handler_list_bits);
                    for name in logging_config_name_list(_py, handler_list.bits())? {
                        let handler = handler_map.get(&name).ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                "ValueError",
                                "unknown handler in root logger config",
                            )
                        })?;
                        drop(logging_config_call_method1(
                            _py,
                            logger.bits(),
                            b"addHandler",
                            handler.bits(),
                        )?);
                    }
                }
                if let Some(level_bits) = logging_config_dict_lookup(_py, root.bits(), "level")? {
                    let level = OwnedRuntimeValue::retain(_py, level_bits);
                    drop(logging_config_call_method1(
                        _py,
                        logger.bits(),
                        b"setLevel",
                        level.bits(),
                    )?);
                }
            }
            Ok(())
        };
        match configure() {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_valid_ident(value_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, MoltObject::none().bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_percent_style_format(fmt_bits: u64, mapping_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(fmt) = string_obj_to_owned(obj_from_bits(fmt_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging format string must be str");
        };
        let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        };
        if unsafe { object_type_id(mapping_ptr) } != crate::bridge::type_id_dict() {
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
            let value_bits =
                match logging_percent_lookup_mapping_value(_py, mapping_ptr, key.as_str()) {
                    Ok(Some(value)) => value,
                    Ok(None) => {
                        out.push_str(token.as_str());
                        idx = close + 2;
                        continue;
                    }
                    Err(ErrorIndicatorSet) => return MoltObject::none().bits(),
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
