use crate::builtins::exceptions::{
    molt_exception_init, molt_exception_new_bound, molt_exceptiongroup_init,
};
use crate::PyToken;
use crate::*;

fn str_codec_arg(_py: &PyToken<'_>, bits: u64, arg_name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("str() argument '{arg_name}' must be str, not {type_name}");
        return raise_exception::<Option<String>>(_py, "TypeError", &msg);
    };
    Some(text)
}

unsafe fn class_layout_size(_py: &PyToken<'_>, class_ptr: *mut u8) -> usize {
    let size_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    let mut size = 0usize;
    if let Some(size_bits) = class_attr_lookup_raw_mro(_py, class_ptr, size_name_bits) {
        if let Some(val) = obj_from_bits(size_bits).as_int() {
            if val > 0 {
                size = val as usize;
            }
        }
    }
    if size == 0 {
        size = 8;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let builtins = builtin_classes(_py);
    if issubclass_bits(class_bits, builtins.int) && size < 16 {
        size = 16;
    }
    if issubclass_bits(class_bits, builtins.dict) && size < 16 {
        size = 16;
    }
    size
}

pub(crate) unsafe fn alloc_instance_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let size = class_layout_size(_py, class_ptr);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(_py, total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    object_set_class_bits(_py, obj_ptr, class_bits);
    inc_ref_bits(_py, class_bits);
    MoltObject::from_ptr(obj_ptr).bits()
}

pub(crate) unsafe fn alloc_instance_for_class_no_pool(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let size = class_layout_size(_py, class_ptr);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed(_py, total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    object_set_class_bits(_py, obj_ptr, class_bits);
    inc_ref_bits(_py, class_bits);
    MoltObject::from_ptr(obj_ptr).bits()
}

unsafe fn alloc_dataclass_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> Option<u64> {
    let Some(field_names_name) =
        attr_name_bits_from_bytes(_py, b"__molt_dataclass_field_names__")
    else {
        return None;
    };
    let field_names_bits = class_attr_lookup_raw_mro(_py, class_ptr, field_names_name);
    dec_ref_bits(_py, field_names_name);
    let Some(field_names_bits) = field_names_bits else {
        return None;
    };
    let Some(field_names_ptr) = obj_from_bits(field_names_bits).as_ptr() else {
        return Some(raise_exception::<_>(
            _py,
            "TypeError",
            "dataclass field names must be a list/tuple of str",
        ));
    };
    let field_count = match object_type_id(field_names_ptr) {
        TYPE_ID_TUPLE => tuple_len(field_names_ptr),
        TYPE_ID_LIST => list_len(field_names_ptr),
        _ => {
            return Some(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass field names must be a list/tuple of str",
            ))
        }
    };
    let missing = missing_bits(_py);
    let mut values = Vec::with_capacity(field_count);
    values.resize(field_count, missing);
    let values_ptr = alloc_tuple(_py, &values);
    if values_ptr.is_null() {
        return Some(MoltObject::none().bits());
    }
    let values_bits = MoltObject::from_ptr(values_ptr).bits();
    let flags_bits = if let Some(flags_name) =
        attr_name_bits_from_bytes(_py, b"__molt_dataclass_flags__")
    {
        let bits = class_attr_lookup_raw_mro(_py, class_ptr, flags_name)
            .unwrap_or_else(|| MoltObject::from_int(0).bits());
        dec_ref_bits(_py, flags_name);
        bits
    } else {
        MoltObject::from_int(0).bits()
    };
    let name_bits = class_name_bits(class_ptr);
    let inst_bits = molt_dataclass_new(name_bits, field_names_bits, values_bits, flags_bits);
    dec_ref_bits(_py, values_bits);
    if exception_pending(_py) {
        return Some(MoltObject::none().bits());
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let _ = molt_dataclass_set_class(inst_bits, class_bits);
    if exception_pending(_py) {
        return Some(MoltObject::none().bits());
    }
    Some(inst_bits)
}

pub(crate) unsafe fn call_class_init_with_args(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    args: &[u64],
) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let builtins = builtin_classes(_py);
    if class_bits == builtins.none_type {
        if !args.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "NoneType takes no arguments");
        }
        return MoltObject::none().bits();
    }
    if class_bits == builtins.not_implemented_type {
        if !args.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "NotImplementedType takes no arguments");
        }
        return not_implemented_bits(_py);
    }
    if class_bits == builtins.ellipsis_type {
        if !args.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "ellipsis takes no arguments");
        }
        return ellipsis_bits(_py);
    }
    if issubclass_bits(class_bits, builtins.base_exception) {
        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let inst_bits =
            if let Some(new_bits) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits) {
                let mut tuple_new = false;
                if let Some(new_ptr) = obj_from_bits(new_bits).as_ptr() {
                    if object_type_id(new_ptr) == TYPE_ID_FUNCTION
                        && function_fn_ptr(new_ptr) == fn_addr!(molt_exception_new_bound)
                    {
                        tuple_new = true;
                    }
                }
                let inst_bits = if tuple_new {
                    let args_ptr = alloc_tuple(_py, args);
                    if args_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let args_bits = MoltObject::from_ptr(args_ptr).bits();
                    let builder_bits = molt_callargs_new(2, 0);
                    if builder_bits == 0 {
                        dec_ref_bits(_py, args_bits);
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(builder_bits, class_bits);
                    let _ = molt_callargs_push_pos(builder_bits, args_bits);
                    let inst_bits = molt_call_bind(new_bits, builder_bits);
                    dec_ref_bits(_py, args_bits);
                    inst_bits
                } else {
                    let builder_bits = molt_callargs_new(args.len() as u64 + 1, 0);
                    if builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(builder_bits, class_bits);
                    for &arg in args {
                        let _ = molt_callargs_push_pos(builder_bits, arg);
                    }
                    molt_call_bind(new_bits, builder_bits)
                };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !isinstance_bits(_py, inst_bits, class_bits) {
                    return inst_bits;
                }
                inst_bits
            } else {
                let args_ptr = alloc_tuple(_py, args);
                if args_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let args_bits = MoltObject::from_ptr(args_ptr).bits();
                let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
                dec_ref_bits(_py, args_bits);
                if exc_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(exc_ptr).bits()
            };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return inst_bits;
        };
        let init_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
        let Some(init_bits) =
            class_attr_lookup(_py, class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
        else {
            return inst_bits;
        };
        let mut tuple_init = false;
        if let Some(init_ptr) = obj_from_bits(init_bits).as_ptr() {
            if object_type_id(init_ptr) == TYPE_ID_FUNCTION {
                let fn_ptr = function_fn_ptr(init_ptr);
                if fn_ptr == fn_addr!(molt_exception_init)
                    || fn_ptr == fn_addr!(molt_exceptiongroup_init)
                {
                    tuple_init = true;
                }
            }
        }
        if tuple_init {
            let args_ptr = alloc_tuple(_py, args);
            if args_ptr.is_null() {
                return inst_bits;
            }
            let args_bits = MoltObject::from_ptr(args_ptr).bits();
            let builder_bits = molt_callargs_new(2, 0);
            if builder_bits == 0 {
                dec_ref_bits(_py, args_bits);
                return inst_bits;
            }
            let _ = molt_callargs_push_pos(builder_bits, inst_bits);
            let _ = molt_callargs_push_pos(builder_bits, args_bits);
            let _ = molt_call_bind(init_bits, builder_bits);
            dec_ref_bits(_py, args_bits);
        } else {
            let pos_capacity = args.len() as u64;
            let builder_bits = molt_callargs_new(pos_capacity, 0);
            if builder_bits == 0 {
                return inst_bits;
            }
            for &arg in args {
                let _ = molt_callargs_push_pos(builder_bits, arg);
            }
            let _ = molt_call_bind(init_bits, builder_bits);
        }
        return inst_bits;
    }
    if class_bits == builtins.slice {
        match args.len() {
            0 => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "slice expected at least 1 argument, got 0",
                );
            }
            1 => {
                return molt_slice_new(
                    MoltObject::none().bits(),
                    args[0],
                    MoltObject::none().bits(),
                );
            }
            2 => {
                return molt_slice_new(args[0], args[1], MoltObject::none().bits());
            }
            3 => {
                return molt_slice_new(args[0], args[1], args[2]);
            }
            _ => {
                let msg = format!("slice expected at most 3 arguments, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.list {
        match args.len() {
            0 => {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => {
                let Some(bits) = list_from_iter_bits(_py, args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("list expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.tuple {
        match args.len() {
            0 => {
                let ptr = alloc_tuple(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => {
                let Some(bits) = tuple_from_iter_bits(_py, args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("tuple expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.dict {
        match args.len() {
            0 => return molt_dict_new(0),
            1 => return molt_dict_from_obj(args[0]),
            _ => {
                let msg = format!("dict expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.module {
        match args.len() {
            0 => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module() missing required argument 'name' (pos 1)",
                );
            }
            1 => return molt_module_new(args[0]),
            2 => {
                let mod_bits = molt_module_new(args[0]);
                if obj_from_bits(mod_bits).is_none() {
                    return mod_bits;
                }
                let Some(doc_name_bits) = attr_name_bits_from_bytes(_py, b"__doc__") else {
                    return mod_bits;
                };
                let _ = molt_module_set_attr(mod_bits, doc_name_bits, args[1]);
                dec_ref_bits(_py, doc_name_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return mod_bits;
            }
            _ => {
                let msg = format!("module expected at most 2 arguments, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.set {
        match args.len() {
            0 => return molt_set_new(0),
            1 => {
                let set_bits = molt_set_new(0);
                if obj_from_bits(set_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let _ = molt_set_update(set_bits, args[0]);
                if exception_pending(_py) {
                    dec_ref_bits(_py, set_bits);
                    return MoltObject::none().bits();
                }
                return set_bits;
            }
            _ => {
                let msg = format!("set expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.frozenset {
        match args.len() {
            0 => return molt_frozenset_new(0),
            1 => {
                let Some(bits) = frozenset_from_iter_bits(_py, args[0]) else {
                    return MoltObject::none().bits();
                };
                return bits;
            }
            _ => {
                let msg = format!("frozenset expected at most 1 argument, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.range {
        match args.len() {
            0 => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "range expected at least 1 argument, got 0",
                );
            }
            1 => {
                let start_bits = MoltObject::from_int(0).bits();
                let step_bits = MoltObject::from_int(1).bits();
                return molt_range_new(start_bits, args[0], step_bits);
            }
            2 => {
                let step_bits = MoltObject::from_int(1).bits();
                return molt_range_new(args[0], args[1], step_bits);
            }
            3 => {
                return molt_range_new(args[0], args[1], args[2]);
            }
            _ => {
                let msg = format!("range expected at most 3 arguments, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.bytes {
        match args.len() {
            0 => {
                let ptr = alloc_bytes(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_bytes_from_obj(args[0]),
            2 => return molt_bytes_from_str(args[0], args[1], MoltObject::none().bits()),
            3 => return molt_bytes_from_str(args[0], args[1], args[2]),
            _ => {
                let msg = format!("bytes() takes at most 3 arguments ({} given)", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.bytearray {
        match args.len() {
            0 => {
                let ptr = alloc_bytearray(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_bytearray_from_obj(args[0]),
            2 => return molt_bytearray_from_str(args[0], args[1], MoltObject::none().bits()),
            3 => return molt_bytearray_from_str(args[0], args[1], args[2]),
            _ => {
                let msg = format!(
                    "bytearray() takes at most 3 arguments ({} given)",
                    args.len()
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    if class_bits == builtins.str {
        match args.len() {
            0 => {
                let ptr = alloc_string(_py, b"");
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            1 => return molt_str_from_obj(args[0]),
            2 | 3 => {
                let obj = obj_from_bits(args[0]);
                let Some(ptr) = obj.as_ptr() else {
                    let msg = format!(
                        "decoding to str: need a bytes-like object, {} found",
                        type_name(_py, obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                };
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "decoding str is not supported");
                }
                if type_id != TYPE_ID_BYTES
                    && type_id != TYPE_ID_BYTEARRAY
                    && type_id != TYPE_ID_MEMORYVIEW
                {
                    let msg = format!(
                        "decoding to str: need a bytes-like object, {} found",
                        type_name(_py, obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let encoding = match str_codec_arg(_py, args[1], "encoding") {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                let errors = if args.len() == 3 {
                    match str_codec_arg(_py, args[2], "errors") {
                        Some(val) => val,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    "strict".to_string()
                };
                let bytes_bits = if type_id == TYPE_ID_BYTES {
                    inc_ref_bits(_py, args[0]);
                    args[0]
                } else {
                    let bits = molt_bytes_from_obj(args[0]);
                    if obj_from_bits(bits).is_none() {
                        return MoltObject::none().bits();
                    }
                    bits
                };
                let bytes_obj = obj_from_bits(bytes_bits);
                let out_bits = if let Some(bytes_ptr) = bytes_obj.as_ptr() {
                    let bytes = unsafe { bytes_like_slice(bytes_ptr) }.unwrap_or(&[]);
                    match decode_bytes_text(&encoding, &errors, bytes) {
                        Ok((text_bytes, _label)) => {
                            let ptr = alloc_string(_py, &text_bytes);
                            if ptr.is_null() {
                                MoltObject::none().bits()
                            } else {
                                MoltObject::from_ptr(ptr).bits()
                            }
                        }
                        Err(DecodeTextError::UnknownEncoding(name)) => {
                            let msg = format!("unknown encoding: {name}");
                            raise_exception::<_>(_py, "LookupError", &msg)
                        }
                        Err(DecodeTextError::UnknownErrorHandler(name)) => {
                            let msg = format!("unknown error handler name '{name}'");
                            raise_exception::<_>(_py, "LookupError", &msg)
                        }
                        Err(DecodeTextError::Failure(
                            DecodeFailure::Byte { pos, message, .. },
                            label,
                        )) => raise_unicode_decode_error(
                            _py,
                            &label,
                            bytes_bits,
                            pos,
                            pos + 1,
                            message,
                        ),
                        Err(DecodeTextError::Failure(
                            DecodeFailure::Range {
                                start,
                                end,
                                message,
                            },
                            label,
                        )) => {
                            let end_exclusive = end.saturating_add(1);
                            raise_unicode_decode_error(
                                _py,
                                &label,
                                bytes_bits,
                                start,
                                end_exclusive,
                                message,
                            )
                        }
                        Err(DecodeTextError::Failure(
                            DecodeFailure::UnknownErrorHandler(name),
                            _label,
                        )) => {
                            let msg = format!("unknown error handler name '{name}'");
                            raise_exception::<_>(_py, "LookupError", &msg)
                        }
                    }
                } else {
                    MoltObject::none().bits()
                };
                dec_ref_bits(_py, bytes_bits);
                return out_bits;
            }
            _ => {
                let msg = format!("str expected at most 3 arguments, got {}", args.len());
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let new_name_bits = intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
    let mut default_new = false;
    let inst_bits = if let Some(new_bits) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits)
    {
        if let Some(new_ptr) = obj_from_bits(new_bits).as_ptr() {
            let new_func_bits = if object_type_id(new_ptr) == TYPE_ID_BOUND_METHOD {
                bound_method_func_bits(new_ptr)
            } else {
                new_bits
            };
            if let Some(new_func_ptr) = obj_from_bits(new_func_bits).as_ptr() {
                if object_type_id(new_func_ptr) == TYPE_ID_FUNCTION
                    && function_fn_ptr(new_func_ptr) == fn_addr!(molt_object_new_bound)
                {
                    default_new = true;
                }
            }
        }
        let inst_bits = if default_new {
            if let Some(inst_bits) = alloc_dataclass_for_class(_py, class_ptr) {
                inst_bits
            } else {
                let builder_bits = molt_callargs_new(args.len() as u64 + 1, 0);
                if builder_bits == 0 {
                    return MoltObject::none().bits();
                }
                let _ = molt_callargs_push_pos(builder_bits, class_bits);
                for &arg in args {
                    let _ = molt_callargs_push_pos(builder_bits, arg);
                }
                let inst_bits = molt_call_bind(new_bits, builder_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !isinstance_bits(_py, inst_bits, class_bits) {
                    return inst_bits;
                }
                inst_bits
            }
        } else {
            let builder_bits = molt_callargs_new(args.len() as u64 + 1, 0);
            if builder_bits == 0 {
                return MoltObject::none().bits();
            }
            let _ = molt_callargs_push_pos(builder_bits, class_bits);
            for &arg in args {
                let _ = molt_callargs_push_pos(builder_bits, arg);
            }
            let inst_bits = molt_call_bind(new_bits, builder_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !isinstance_bits(_py, inst_bits, class_bits) {
                return inst_bits;
            }
            inst_bits
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        inst_bits
    } else {
        alloc_instance_for_class(_py, class_ptr)
    };
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return inst_bits;
    };
    let init_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
    let Some(init_bits) =
        class_attr_lookup(_py, class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
    else {
        return inst_bits;
    };
    if default_new && !args.is_empty() {
        if let Some(init_ptr) = obj_from_bits(init_bits).as_ptr() {
            let init_func_bits = if object_type_id(init_ptr) == TYPE_ID_BOUND_METHOD {
                bound_method_func_bits(init_ptr)
            } else {
                init_bits
            };
            if let Some(init_func_ptr) = obj_from_bits(init_func_bits).as_ptr() {
                if object_type_id(init_func_ptr) == TYPE_ID_FUNCTION
                    && function_fn_ptr(init_func_ptr) == fn_addr!(molt_object_init)
                {
                    let class_name = class_name_for_error(class_bits);
                    let msg = format!("{class_name}() takes no arguments");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
    }
    let builder_bits = molt_callargs_new(args.len() as u64, 0);
    if builder_bits == 0 {
        return inst_bits;
    }
    for &arg in args {
        let _ = molt_callargs_push_pos(builder_bits, arg);
    }
    let _ = molt_call_bind(init_bits, builder_bits);
    inst_bits
}

pub(crate) fn raise_not_callable(_py: &PyToken<'_>, obj: MoltObject) -> u64 {
    let msg = format!("'{}' object is not callable", type_name(_py, obj));
    raise_exception::<_>(_py, "TypeError", &msg)
}

pub(crate) unsafe fn call_builtin_type_if_needed(
    _py: &PyToken<'_>,
    call_bits: u64,
    call_ptr: *mut u8,
    args: &[u64],
) -> Option<u64> {
    if is_builtin_class_bits(_py, call_bits) {
        return Some(call_class_init_with_args(_py, call_ptr, args));
    }
    None
}

pub(crate) unsafe fn try_call_generator(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> Option<u64> {
    let func_obj = obj_from_bits(func_bits);
    let func_ptr = func_obj.as_ptr()?;
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        return None;
    }
    let is_gen = function_attr_bits(
        _py,
        func_ptr,
        intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_is_generator,
            b"__molt_is_generator__",
        ),
    )
    .is_some_and(|bits| is_truthy(_py, obj_from_bits(bits)));
    if !is_gen {
        return None;
    }
    let size_bits = function_attr_bits(
        _py,
        func_ptr,
        intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_closure_size,
            b"__molt_closure_size__",
        ),
    )
    .unwrap_or_else(|| MoltObject::none().bits());
    let Some(size_val) = obj_from_bits(size_bits).as_int() else {
        return raise_exception::<_>(_py, "TypeError", "call expects function object");
    };
    if size_val < 0 {
        return raise_exception::<_>(_py, "TypeError", "closure size must be non-negative");
    }
    let closure_size = size_val as usize;
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let mut payload: Vec<u64> =
        Vec::with_capacity(args.len() + if closure_bits != 0 { 1 } else { 0 });
    if closure_bits != 0 {
        payload.push(closure_bits);
    }
    payload.extend(args.iter().copied());
    let base = GEN_CONTROL_SIZE;
    let needed = base + payload.len() * std::mem::size_of::<u64>();
    if closure_size < needed {
        return raise_exception::<_>(_py, "TypeError", "call expects function object");
    }
    let obj_bits = molt_generator_new(fn_ptr, closure_size as u64);
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return Some(MoltObject::none().bits());
    };
    let mut offset = base;
    for val_bits in payload {
        let slot = obj_ptr.add(offset) as *mut u64;
        *slot = val_bits;
        inc_ref_bits(_py, val_bits);
        offset += std::mem::size_of::<u64>();
    }
    Some(obj_bits)
}

pub(crate) unsafe fn function_attr_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    let dict_bits = function_dict_bits(func_ptr);
    if dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    dict_get_in_place(_py, dict_ptr, attr_bits)
}
