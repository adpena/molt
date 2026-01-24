use crate::*;
use crate::PyToken;

unsafe fn class_layout_size(_py: &PyToken<'_>, class_ptr: *mut u8) -> usize {
    let size_name_bits = intern_static_name(_py,
        &runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    if let Some(size_bits) = class_attr_lookup_raw_mro(_py, class_ptr, size_name_bits) {
        if let Some(size) = obj_from_bits(size_bits).as_int() {
            if size > 0 {
                return size as usize;
            }
        }
    }
    8
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

pub(crate) unsafe fn call_class_init_with_args(_py: &PyToken<'_>, class_ptr: *mut u8, args: &[u64]) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let builtins = builtin_classes(_py);
    if issubclass_bits(class_bits, builtins.base_exception) {
        let new_name_bits = intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        let inst_bits = if let Some(new_bits) = class_attr_lookup_raw_mro(_py, class_ptr, new_name_bits)
        {
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
        let init_name_bits = intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
        let Some(init_bits) =
            class_attr_lookup(_py, class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
        else {
            return inst_bits;
        };
        let pos_capacity = args.len() as u64;
        let builder_bits = molt_callargs_new(pos_capacity, 0);
        if builder_bits == 0 {
            return inst_bits;
        }
        for &arg in args {
            let _ = molt_callargs_push_pos(builder_bits, arg);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        return inst_bits;
    }
    if class_bits == builtins.slice {
        match args.len() {
            0 => {
                return raise_exception::<_>(_py,
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
                return raise_exception::<_>(_py,
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
            _ => {
                let obj = obj_from_bits(args[0]);
                let is_bytes_like = obj.as_ptr().is_some_and(|ptr| unsafe {
                    let type_id = object_type_id(ptr);
                    type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY
                });
                if !is_bytes_like {
                    let msg = format!(
                        "decoding to str: need a bytes-like object, {} found",
                        type_name(_py, obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                // TODO(stdlib-compat, owner:runtime, milestone:TC2, priority:P2, status:partial):
                // support encoding/errors args for bytes-like inputs and match CPython's
                // UnicodeDecodeError details.
                return raise_exception::<_>(_py,
                    "NotImplementedError",
                    "str() encoding arguments are not supported yet",
                );
            }
        }
    }
    let inst_bits = alloc_instance_for_class(_py, class_ptr);
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return inst_bits;
    };
    let init_name_bits = intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
    let Some(init_bits) = class_attr_lookup(_py, class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
    else {
        return inst_bits;
    };
    let pos_capacity = args.len() as u64;
    let builder_bits = molt_callargs_new(pos_capacity, 0);
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
    return raise_exception::<_>(_py, "TypeError", &msg);
}

pub(crate) unsafe fn call_builtin_type_if_needed(
    _py: &PyToken<'_>, call_bits: u64,
    call_ptr: *mut u8,
    args: &[u64],
) -> Option<u64> {
    if is_builtin_class_bits(_py, call_bits) {
        return Some(call_class_init_with_args(_py, call_ptr, args));
    }
    None
}

pub(crate) unsafe fn try_call_generator(_py: &PyToken<'_>, func_bits: u64, args: &[u64]) -> Option<u64> {
    let func_obj = obj_from_bits(func_bits);
    let func_ptr = func_obj.as_ptr()?;
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        return None;
    }
    let is_gen = function_attr_bits(_py,
        func_ptr,
        intern_static_name(_py,
            &runtime_state(_py).interned.molt_is_generator,
            b"__molt_is_generator__",
        ),
    )
    .is_some_and(|bits| is_truthy(obj_from_bits(bits)));
    if !is_gen {
        return None;
    }
    let size_bits = function_attr_bits(_py,
        func_ptr,
        intern_static_name(_py,
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

pub(crate) unsafe fn function_attr_bits(_py: &PyToken<'_>, func_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
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
