// Binary pickle object serialization (dump) VM.

use super::*;

fn pickle_lookup_extension_code(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<Option<i64>, u64> {
    let registry_bits = pickle_resolve_global_bits(_py, "copyreg", "_extension_registry")?;
    let Some(registry_ptr) = obj_from_bits(registry_bits).as_ptr() else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    if unsafe { object_type_id(registry_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    }
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_ptr = alloc_tuple(_py, &[module_bits, name_bits]);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    let Some(key_ptr) = (!key_ptr.is_null()).then_some(key_ptr) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let code_bits = unsafe { dict_get_in_place(_py, registry_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(code_bits) = code_bits else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    let Some(code) = to_i64(obj_from_bits(code_bits)) else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    dec_ref_bits(_py, registry_bits);
    if code <= 0 {
        return Ok(None);
    }
    Ok(Some(code))
}

fn pickle_emit_global_ref(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
        return Ok(false);
    };
    let Some(name_bits) = pickle_attr_optional(_py, obj_bits, b"__name__")? else {
        dec_ref_bits(_py, module_bits);
        return Ok(false);
    };
    let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    if state.protocol >= 2
        && let Some(code) = pickle_lookup_extension_code(_py, &module_name, &attr_name)?
    {
        if code <= u8::MAX as i64 {
            state.push(PICKLE_OP_EXT1);
            state.push(code as u8);
            return Ok(true);
        }
        if code <= u16::MAX as i64 {
            state.push(PICKLE_OP_EXT2);
            state.extend(&(code as u16).to_le_bytes());
            return Ok(true);
        }
        if code <= u32::MAX as i64 {
            state.push(PICKLE_OP_EXT4);
            state.extend(&(code as u32).to_le_bytes());
            return Ok(true);
        }
    }
    if state.protocol >= PICKLE_PROTO_4 {
        pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
        pickle_dump_unicode_binary(_py, state, attr_name.as_str())?;
        state.push(PICKLE_OP_STACK_GLOBAL);
        return Ok(true);
    }
    pickle_emit_global_opcode(state, module_name.as_str(), attr_name.as_str());
    Ok(true)
}

fn pickle_dump_unicode_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    text: &str,
) -> Result<(), u64> {
    let raw = text.as_bytes();
    if raw.len() <= u8::MAX as usize && state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_SHORT_BINUNICODE);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINUNICODE);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(0x8d);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytes_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_SHORT_BINBYTES);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINBYTES);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(PICKLE_OP_BINBYTES8);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytearray_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if state.protocol >= PICKLE_PROTO_5 {
        state.push(PICKLE_OP_BYTEARRAY8);
        pickle_emit_u64_le(state, raw.len() as u64);
        state.extend(raw);
        return Ok(());
    }
    // Protocols 2-4: bytearray(bytes(...)) reduce path.
    pickle_emit_global_opcode(state, "builtins", "bytearray");
    let bytes_ptr = crate::alloc_bytes(_py, raw);
    if bytes_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
    let dumped = pickle_dump_obj_binary(_py, state, bytes_bits, true);
    dec_ref_bits(_py, bytes_bits);
    dumped?;
    state.push(PICKLE_OP_TUPLE1);
    state.push(PICKLE_OP_REDUCE);
    Ok(())
}

fn pickle_long_bytes_from_i64(value: i64) -> Vec<u8> {
    let mut raw = value.to_le_bytes().to_vec();
    while raw.len() > 1 {
        let last = raw[raw.len() - 1];
        let prev = raw[raw.len() - 2];
        let drop_zero = last == 0x00 && (prev & 0x80) == 0;
        let drop_ff = last == 0xff && (prev & 0x80) != 0;
        if drop_zero || drop_ff {
            raw.pop();
        } else {
            break;
        }
    }
    raw
}

fn pickle_dump_int_binary(state: &mut PickleDumpState, value: i64) {
    if (0..=u8::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT1);
        state.push(value as u8);
        return;
    }
    if (0..=u16::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT2);
        state.extend(&(value as u16).to_le_bytes());
        return;
    }
    if (i32::MIN as i64..=i32::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT);
        state.extend(&(value as i32).to_le_bytes());
        return;
    }
    let raw = pickle_long_bytes_from_i64(value);
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_LONG1);
        state.push(raw.len() as u8);
        state.extend(raw.as_slice());
    } else {
        state.push(PICKLE_OP_LONG4);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw.as_slice());
    }
}

fn pickle_dump_float_binary(state: &mut PickleDumpState, value: f64) {
    state.push(PICKLE_OP_BINFLOAT);
    state.extend(&value.to_bits().to_be_bytes());
}

fn pickle_dump_maybe_persistent(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.persistent_id_bits else {
        return Ok(false);
    };
    let pid_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(pid_bits).is_none() {
        return Ok(false);
    }
    if state.protocol == 0
        && let Some(pid_text) = string_obj_to_owned(obj_from_bits(pid_bits))
    {
        state.push(PICKLE_OP_PERSID);
        state.extend(pid_text.as_bytes());
        state.push(b'\n');
        return Ok(true);
    }
    pickle_dump_obj_binary(_py, state, pid_bits, false)?;
    state.push(PICKLE_OP_BINPERSID);
    Ok(true)
}

fn pickle_buffer_value_to_bytes(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        let out_ptr = crate::alloc_bytes(_py, raw);
        if out_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(out_ptr).bits());
    }
    let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
    Err(pickle_raise(_py, &msg))
}

pub(crate) fn pickle_buffer_value_to_memoryview(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    let view_bits = crate::molt_memoryview_new(value_bits);
    if exception_pending(_py) {
        let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(view_bits)
}

fn pickle_external_buffer_to_memoryview(
    _py: &crate::PyToken<'_>,
    item_bits: u64,
) -> Result<u64, u64> {
    if let Ok(bits) = pickle_buffer_value_to_memoryview(_py, item_bits, "out-of-band buffer") {
        return Ok(bits);
    }
    if let Some(raw_method_bits) = pickle_attr_optional(_py, item_bits, b"raw")? {
        let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
        dec_ref_bits(_py, raw_method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return pickle_buffer_value_to_memoryview(_py, raw_bits, "out-of-band buffer");
    }
    Err(pickle_raise(
        _py,
        "pickle.loads: out-of-band buffer must be bytes-like or expose raw()",
    ))
}

pub(crate) fn pickle_next_external_buffer_bits(
    _py: &crate::PyToken<'_>,
    buffers_iter_bits: Option<u64>,
) -> Result<u64, u64> {
    let Some(iter_bits) = buffers_iter_bits else {
        return Err(pickle_raise(
            _py,
            "pickle.loads: NEXT_BUFFER requires buffers argument",
        ));
    };
    let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
    if done {
        return Err(pickle_raise(
            _py,
            "pickle.loads: not enough out-of-band buffers",
        ));
    }
    pickle_external_buffer_to_memoryview(_py, item_bits)
}

fn pickle_dump_maybe_out_of_band_buffer(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    readonly: bool,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.buffer_callback_bits else {
        return Ok(false);
    };
    if state.protocol < PICKLE_PROTO_5 {
        return Ok(false);
    }
    let callback_result_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let in_band = is_truthy(_py, obj_from_bits(callback_result_bits));
    if !obj_from_bits(callback_result_bits).is_none() {
        dec_ref_bits(_py, callback_result_bits);
    }
    if in_band {
        return Ok(false);
    }
    state.push(PICKLE_OP_NEXT_BUFFER);
    if readonly {
        state.push(PICKLE_OP_READONLY_BUFFER);
    }
    // Do NOT memo out-of-band buffers — each reference must emit its own
    // NEXT_BUFFER opcode so every buffer slot is consumed during loads.
    Ok(true)
}

fn pickle_extract_picklebuffer_payload(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<Option<(u64, bool)>, u64> {
    let marker_bits = match pickle_attr_optional(_py, obj_bits, b"__molt_pickle_buffer__")? {
        Some(bits) => bits,
        None => return Ok(None),
    };
    let is_marker = is_truthy(_py, obj_from_bits(marker_bits));
    dec_ref_bits(_py, marker_bits);
    if !is_marker {
        return Ok(None);
    }
    let raw_method_bits = pickle_attr_required(_py, obj_bits, b"raw")?;
    let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
    dec_ref_bits(_py, raw_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let readonly = if let Some(raw_ptr) = obj_from_bits(raw_bits).as_ptr() {
        let raw_type = unsafe { object_type_id(raw_ptr) };
        if raw_type == crate::TYPE_ID_BYTEARRAY {
            false
        } else if raw_type == crate::TYPE_ID_MEMORYVIEW {
            if unsafe { crate::memoryview_released(raw_ptr) } {
                return Err(crate::raise_released_memoryview(_py));
            }
            unsafe { crate::memoryview_readonly(raw_ptr) }
        } else {
            true
        }
    } else {
        true
    };
    let payload_bits = pickle_buffer_value_to_bytes(_py, raw_bits, "PickleBuffer.raw() payload");
    if !obj_from_bits(raw_bits).is_none() {
        dec_ref_bits(_py, raw_bits);
    }
    payload_bits.map(|bits| Some((bits, readonly)))
}

fn pickle_dispatch_reducer_from_table(
    _py: &crate::PyToken<'_>,
    dispatch_table_bits: u64,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    let Some(ptr) = obj_from_bits(dispatch_table_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }
    let type_bits = type_of_bits(_py, obj_bits);
    let reducer_bits = unsafe { dict_get_in_place(_py, ptr, type_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(reducer_bits) = reducer_bits else {
        return Ok(None);
    };
    let out_bits = unsafe { call_callable1(_py, reducer_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(out_bits))
}

fn pickle_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    if let Some(dispatch_bits) = state.dispatch_table_bits
        && let Some(reduced) = pickle_dispatch_reducer_from_table(_py, dispatch_bits, obj_bits)?
    {
        return Ok(Some(reduced));
    }
    if let Some(reduce_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce_ex__")? {
        let out_bits = unsafe {
            call_callable1(
                _py,
                reduce_ex_bits,
                MoltObject::from_int(state.protocol).bits(),
            )
        };
        dec_ref_bits(_py, reduce_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    if let Some(reduce_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce__")? {
        let out_bits = unsafe { call_callable0(_py, reduce_bits) };
        dec_ref_bits(_py, reduce_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    Ok(None)
}

fn pickle_dump_items_from_iterable(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    values_bits: u64,
    dict_items: bool,
    iterator_error_prefix: &str,
) -> Result<(), u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        let value_type = type_name(_py, obj_from_bits(values_bits));
        let msg = format!("{iterator_error_prefix}{value_type}");
        return Err(pickle_raise(_py, &msg));
    }
    state.push(PICKLE_OP_MARK);
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        if dict_items {
            let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            };
            if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            let fields = unsafe { seq_vec_ref(item_ptr) };
            if fields.len() != 2 {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            pickle_dump_obj_binary(_py, state, fields[0], true)?;
            pickle_dump_obj_binary(_py, state, fields[1], true)?;
        } else {
            pickle_dump_obj_binary(_py, state, item_bits, true)?;
        }
    }
    if dict_items {
        state.push(PICKLE_OP_SETITEMS);
    } else {
        state.push(PICKLE_OP_APPENDS);
    }
    Ok(())
}

fn pickle_dump_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    reduce_bits: u64,
    obj_bits: Option<u64>,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(reduce_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    };
    let reduce_type = unsafe { object_type_id(ptr) };
    if reduce_type == TYPE_ID_STRING {
        let Some(global_name) = string_obj_to_owned(obj_from_bits(reduce_bits)) else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(obj_bits) = obj_bits else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            dec_ref_bits(_py, module_bits);
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        dec_ref_bits(_py, module_bits);
        let resolved_bits =
            pickle_resolve_global_bits(_py, module_name.as_str(), global_name.as_str())?;
        let matches = resolved_bits == obj_bits;
        if !obj_from_bits(resolved_bits).is_none() {
            dec_ref_bits(_py, resolved_bits);
        }
        if !matches {
            let obj_type = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!(
                "Can't pickle {obj_type}: it's not the same object as {}.{}",
                module_name, global_name
            );
            return Err(pickle_raise(_py, &msg));
        }
        if state.protocol >= PICKLE_PROTO_4 {
            pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
            pickle_dump_unicode_binary(_py, state, global_name.as_str())?;
            state.push(PICKLE_OP_STACK_GLOBAL);
        } else {
            pickle_emit_global_opcode(state, module_name.as_str(), global_name.as_str());
        }
        let _ = pickle_memo_store_if_absent(state, obj_bits);
        return Ok(());
    }
    if reduce_type != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if !(2..=6).contains(&fields.len()) {
        return Err(pickle_raise(
            _py,
            "tuple returned by __reduce__ must contain 2 through 6 elements",
        ));
    }
    let callable_bits = fields[0];
    let callable_check = molt_is_callable(callable_bits);
    if !is_truthy(_py, obj_from_bits(callable_check)) {
        return Err(pickle_raise(
            _py,
            "first item of the tuple returned by __reduce__ must be callable",
        ));
    }
    let args_bits = fields[1];
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        let iter_bits = molt_iter(fields[3]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[3]));
            let msg = format!(
                "fourth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        let iter_bits = molt_iter(fields[4]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[4]));
            let msg = format!(
                "fifth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 6 && !obj_from_bits(fields[5]).is_none() {
        let setter_check = molt_is_callable(fields[5]);
        if !is_truthy(_py, obj_from_bits(setter_check)) {
            let value_type = type_name(_py, obj_from_bits(fields[5]));
            let msg = format!(
                "sixth element of the tuple returned by __reduce__ must be a function, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
    }
    pickle_dump_obj_binary(_py, state, callable_bits, true)?;
    pickle_dump_obj_binary(_py, state, args_bits, true)?;
    state.push(PICKLE_OP_REDUCE);
    if let Some(bits) = obj_bits {
        let _ = pickle_memo_store_if_absent(state, bits);
    }
    let state_bits = if fields.len() >= 3 {
        Some(fields[2])
    } else {
        None
    };
    let state_setter_bits = if fields.len() >= 6 {
        Some(fields[5])
    } else {
        None
    };
    if let Some(state_bits) = state_bits
        && !obj_from_bits(state_bits).is_none()
    {
        if let Some(state_setter_bits) = state_setter_bits {
            if !obj_from_bits(state_setter_bits).is_none() {
                let Some(obj_bits) = obj_bits else {
                    return Err(pickle_raise(
                        _py,
                        "pickle reducer state_setter requires object context",
                    ));
                };
                pickle_dump_obj_binary(_py, state, state_setter_bits, true)?;
                pickle_dump_obj_binary(_py, state, obj_bits, true)?;
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
                state.push(PICKLE_OP_POP);
            } else {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
        } else {
            pickle_dump_obj_binary(_py, state, state_bits, true)?;
            state.push(PICKLE_OP_BUILD);
        }
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[3],
            false,
            "fourth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[4],
            true,
            "fifth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    Ok(())
}

fn pickle_empty_tuple_bits(_py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let tuple_ptr = alloc_tuple(_py, &[]);
    if tuple_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn pickle_require_tuple_bits(
    _py: &crate::PyToken<'_>,
    bits: u64,
    context: &str,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_require_dict_bits(_py: &crate::PyToken<'_>, bits: u64, context: &str) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_default_newobj_args(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<(u64, Option<u64>), u64> {
    if let Some(getnewargs_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs_ex__")? {
        let out_bits = unsafe { call_callable0(_py, getnewargs_ex_bits) };
        dec_ref_bits(_py, getnewargs_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(tuple_ptr) = obj_from_bits(out_bits).as_ptr() else {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        };
        if unsafe { object_type_id(tuple_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let fields = unsafe { seq_vec_ref(tuple_ptr).to_vec() };
        if fields.len() != 2 {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let args_bits = fields[0];
        let kwargs_bits = fields[1];
        pickle_require_tuple_bits(_py, args_bits, "__getnewargs_ex__ args")?;
        pickle_require_dict_bits(_py, kwargs_bits, "__getnewargs_ex__ kwargs")?;
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, kwargs_bits);
        dec_ref_bits(_py, out_bits);
        return Ok((args_bits, Some(kwargs_bits)));
    }

    if let Some(getnewargs_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs__")? {
        let args_bits = unsafe { call_callable0(_py, getnewargs_bits) };
        dec_ref_bits(_py, getnewargs_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if let Err(err_bits) = pickle_require_tuple_bits(_py, args_bits, "__getnewargs__ value") {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return Err(err_bits);
        }
        return Ok((args_bits, None));
    }

    Ok((pickle_empty_tuple_bits(_py)?, None))
}

fn pickle_dataclass_state_bits(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Result<Option<u64>, u64> {
    let desc_ptr = unsafe { crate::dataclass_desc_ptr(ptr) };
    if desc_ptr.is_null() {
        return Ok(None);
    }

    if unsafe { (*desc_ptr).slots } {
        let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
        if slot_state_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
        let mut wrote_any = false;
        let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
        let field_names = unsafe { &(*desc_ptr).field_names };
        for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
            let Some(name_bits) = alloc_string_bits(_py, name) else {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
            }
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
        }
        if !wrote_any {
            dec_ref_bits(_py, slot_state_bits);
            return Ok(None);
        }
        let dict_state_bits = if unsafe { (*desc_ptr).allows_dict } {
            let dict_bits = unsafe { crate::dataclass_dict_bits(ptr) };
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
                && !unsafe { crate::dict_order(dict_ptr).is_empty() }
            {
                inc_ref_bits(_py, dict_bits);
                dict_bits
            } else {
                MoltObject::none().bits()
            }
        } else {
            MoltObject::none().bits()
        };
        let tuple_ptr = alloc_tuple(_py, &[dict_state_bits, slot_state_bits]);
        if !obj_from_bits(dict_state_bits).is_none() {
            dec_ref_bits(_py, dict_state_bits);
        }
        dec_ref_bits(_py, slot_state_bits);
        if tuple_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()));
    }

    if !unsafe { (*desc_ptr).slots } {
        let dict_bits = unsafe { crate::dataclass_dict_bits(ptr) };
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            inc_ref_bits(_py, dict_bits);
            return Ok(Some(dict_bits));
        }
    }

    let state_ptr = alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;

    let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
    let field_names = unsafe { &(*desc_ptr).field_names };
    for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        };
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }

    let extra_bits = unsafe { crate::dataclass_dict_bits(ptr) };
    if extra_bits != 0
        && let Some(extra_ptr) = obj_from_bits(extra_bits).as_ptr()
        && unsafe { object_type_id(extra_ptr) } == TYPE_ID_DICT
    {
        let pairs = unsafe { crate::dict_order(extra_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            unsafe {
                crate::dict_set_in_place(_py, state_ptr, pairs[idx], pairs[idx + 1]);
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
            idx += 2;
        }
    }

    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return Ok(None);
    }
    Ok(Some(state_bits))
}

fn pickle_object_slot_state_bits(
    _py: &crate::PyToken<'_>,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
        return Ok(None);
    }

    let class_dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let Some(class_dict_ptr) = obj_from_bits(class_dict_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_dict_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let Some(offsets_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
        return Err(MoltObject::none().bits());
    };
    let offsets_bits = unsafe { dict_get_in_place(_py, class_dict_ptr, offsets_name_bits) };
    dec_ref_bits(_py, offsets_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(offsets_bits) = offsets_bits else {
        return Ok(None);
    };
    let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
    if slot_state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
    let mut wrote_any = false;
    let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let name_bits = pairs[idx];
        let offset_bits = pairs[idx + 1];
        idx += 2;
        let Some(offset) = to_i64(obj_from_bits(offset_bits)) else {
            continue;
        };
        if offset < 0 {
            continue;
        }
        let value_bits = unsafe { crate::object_field_get_ptr_raw(_py, ptr, offset as usize) };
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        if value_bits == missing_bits(_py) {
            dec_ref_bits(_py, value_bits);
            continue;
        }
        unsafe {
            crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, value_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, slot_state_bits);
        return Ok(None);
    }
    Ok(Some(slot_state_bits))
}

fn pickle_object_state_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let mut dict_state_bits: Option<u64> = None;
    // Try the fast path first: trailing __dict__ slot.
    let dict_bits = unsafe { crate::instance_dict_bits(ptr) };
    if dict_bits != 0
        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
        && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        && !unsafe { crate::dict_order(dict_ptr).is_empty() }
    {
        inc_ref_bits(_py, dict_bits);
        dict_state_bits = Some(dict_bits);
    }
    // Fall back to getattr(__dict__) when the trailing slot is empty/missing.
    // The compiler may store attributes in a dict accessible only through getattr.
    if dict_state_bits.is_none()
        && !exception_pending(_py)
        && let Some(dict_name_bits) = attr_name_bits_from_bytes(_py, b"__dict__")
    {
        let missing = missing_bits(_py);
        let attr_dict_bits = molt_getattr_builtin(obj_bits, dict_name_bits, missing);
        dec_ref_bits(_py, dict_name_bits);
        if !exception_pending(_py)
            && attr_dict_bits != missing
            && let Some(dict_ptr) = obj_from_bits(attr_dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            // attr_dict_bits already carries a reference from getattr.
            dict_state_bits = Some(attr_dict_bits);
        } else if attr_dict_bits != missing && !obj_from_bits(attr_dict_bits).is_none() {
            dec_ref_bits(_py, attr_dict_bits);
        }
        // Clear AttributeError if __dict__ wasn't found.
        if exception_pending(_py) {
            clear_exception(_py);
        }
    }

    let slot_state_bits = pickle_object_slot_state_bits(_py, ptr)?;
    let Some(slot_state_bits) = slot_state_bits else {
        return Ok(dict_state_bits);
    };

    let dict_or_none_bits = dict_state_bits.unwrap_or(MoltObject::none().bits());
    let tuple_ptr = alloc_tuple(_py, &[dict_or_none_bits, slot_state_bits]);
    if let Some(bits) = dict_state_bits {
        dec_ref_bits(_py, bits);
    }
    dec_ref_bits(_py, slot_state_bits);
    if tuple_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()))
}

fn pickle_default_instance_state(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<Option<u64>, u64> {
    if let Some(getstate_bits) = pickle_attr_optional(_py, obj_bits, b"__getstate__")? {
        let state_bits = unsafe { call_callable0(_py, getstate_bits) };
        dec_ref_bits(_py, getstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(state_bits));
    }
    if type_id == crate::TYPE_ID_DATACLASS {
        return pickle_dataclass_state_bits(_py, ptr);
    }
    if type_id == crate::TYPE_ID_OBJECT {
        return pickle_object_state_bits(_py, obj_bits, ptr);
    }
    Ok(None)
}

fn pickle_dump_default_instance(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<bool, u64> {
    if type_id != crate::TYPE_ID_OBJECT && type_id != crate::TYPE_ID_DATACLASS {
        return Ok(false);
    }
    let cls_bits = unsafe { object_class_bits(ptr) };
    if cls_bits == 0 || obj_from_bits(cls_bits).as_ptr().is_none() {
        return Ok(false);
    }

    let (args_bits, kwargs_bits) = pickle_default_newobj_args(_py, obj_bits)?;
    let result = (|| -> Result<(), u64> {
        let mut kwargs_effective = kwargs_bits;
        if let Some(bits) = kwargs_effective {
            let Some(dict_ptr) = obj_from_bits(bits).as_ptr() else {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            }
            if unsafe { crate::dict_order(dict_ptr).is_empty() } {
                kwargs_effective = None;
            }
        }

        if let Some(kwargs_bits) = kwargs_effective {
            if state.protocol >= PICKLE_PROTO_4 {
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_NEWOBJ_EX);
            } else {
                pickle_emit_global_opcode(state, "copyreg", "__newobj_ex__");
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_TUPLE3);
                state.push(PICKLE_OP_REDUCE);
            }
        } else {
            pickle_dump_obj_binary(_py, state, cls_bits, true)?;
            pickle_dump_obj_binary(_py, state, args_bits, true)?;
            state.push(PICKLE_OP_NEWOBJ);
        }

        let _ = pickle_memo_store_if_absent(state, obj_bits);
        if let Some(state_bits) = pickle_default_instance_state(_py, obj_bits, ptr, type_id)? {
            if !obj_from_bits(state_bits).is_none() {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
            if !obj_from_bits(state_bits).is_none() {
                dec_ref_bits(_py, state_bits);
            }
        }
        Ok(())
    })();

    if !obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, args_bits);
    }
    if let Some(bits) = kwargs_bits
        && !obj_from_bits(bits).is_none()
    {
        dec_ref_bits(_py, bits);
    }
    result.map(|()| true)
}

pub(crate) fn pickle_dump_obj_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    allow_persistent_id: bool,
) -> Result<(), u64> {
    if state.depth >= PICKLE_RECURSION_LIMIT {
        return Err(pickle_raise(
            _py,
            "pickle.dumps: maximum recursion depth exceeded",
        ));
    }
    state.depth += 1;
    let result = (|| -> Result<(), u64> {
        if allow_persistent_id && pickle_dump_maybe_persistent(_py, state, obj_bits)? {
            return Ok(());
        }
        if let Some(index) = pickle_memo_lookup(state, obj_bits) {
            pickle_emit_memo_get(state, index);
            return Ok(());
        }
        let obj = obj_from_bits(obj_bits);
        if obj.is_none() {
            state.push(PICKLE_OP_NONE);
            return Ok(());
        }
        if let Some(value) = obj.as_bool() {
            state.push(if value {
                PICKLE_OP_NEWTRUE
            } else {
                PICKLE_OP_NEWFALSE
            });
            return Ok(());
        }
        if let Some(value) = obj.as_int() {
            pickle_dump_int_binary(state, value);
            return Ok(());
        }
        if let Some(value) = obj.as_float() {
            pickle_dump_float_binary(state, value);
            return Ok(());
        }
        let Some(ptr) = obj.as_ptr() else {
            let type_name = type_name(_py, obj);
            let msg = format!("pickle.dumps: unsupported type: {type_name}");
            return Err(pickle_raise(_py, &msg));
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_STRING {
            let text = string_obj_to_owned(obj)
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: string conversion failed"))?;
            pickle_dump_unicode_binary(_py, state, text.as_str())?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTES {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytes conversion failed"))?;
            if state.protocol < PICKLE_PROTO_3 {
                pickle_emit_global_opcode(state, "_codecs", "encode");
                let latin1 = pickle_decode_latin1(raw);
                pickle_dump_unicode_binary(_py, state, &latin1)?;
                pickle_dump_unicode_binary(_py, state, "latin1")?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
            } else {
                pickle_dump_bytes_binary(_py, state, raw)?;
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTEARRAY {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytearray conversion failed"))?;
            pickle_dump_bytearray_binary(_py, state, raw)?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some((payload_bits, readonly)) = pickle_extract_picklebuffer_payload(_py, obj_bits)?
        {
            if pickle_dump_maybe_out_of_band_buffer(_py, state, obj_bits, readonly)? {
                if !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(_py, payload_bits);
                }
                return Ok(());
            }
            let Some(payload_ptr) = obj_from_bits(payload_bits).as_ptr() else {
                return Err(pickle_raise(
                    _py,
                    "pickle.dumps: PickleBuffer.raw() must be bytes-like",
                ));
            };
            let raw = unsafe { bytes_like_slice(payload_ptr) }.ok_or_else(|| {
                pickle_raise(_py, "pickle.dumps: PickleBuffer.raw() must be bytes-like")
            })?;
            if readonly {
                pickle_dump_bytes_binary(_py, state, raw)?;
            } else {
                pickle_dump_bytearray_binary(_py, state, raw)?;
            }
            if !obj_from_bits(payload_bits).is_none() {
                dec_ref_bits(_py, payload_bits);
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_TUPLE {
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            match values.len() {
                0 => state.push(PICKLE_OP_EMPTY_TUPLE),
                1 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    state.push(PICKLE_OP_TUPLE1);
                }
                2 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    state.push(PICKLE_OP_TUPLE2);
                }
                3 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    pickle_dump_obj_binary(_py, state, values[2], true)?;
                    state.push(PICKLE_OP_TUPLE3);
                }
                _ => {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_TUPLE);
                }
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_LIST {
            state.push(PICKLE_OP_EMPTY_LIST);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            return Ok(());
        }
        if type_id == TYPE_ID_DICT {
            state.push(PICKLE_OP_EMPTY_DICT);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let pairs = unsafe { crate::dict_order(ptr).to_vec() };
            if !pairs.is_empty() {
                state.push(PICKLE_OP_MARK);
                let mut idx = 0usize;
                while idx + 1 < pairs.len() {
                    pickle_dump_obj_binary(_py, state, pairs[idx], true)?;
                    pickle_dump_obj_binary(_py, state, pairs[idx + 1], true)?;
                    idx += 2;
                }
                state.push(PICKLE_OP_SETITEMS);
            }
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_EMPTY_SET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                if !values.is_empty() {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_ADDITEMS);
                }
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "set");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_FROZENSET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_MARK);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_FROZENSET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "frozenset");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SLICE {
            pickle_emit_global_opcode(state, "builtins", "slice");
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_start_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_stop_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_step_bits(ptr) }, true)?;
            state.push(PICKLE_OP_TUPLE3);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if pickle_emit_global_ref(_py, state, obj_bits)? {
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some(reduce_bits) = pickle_reduce_value(_py, state, obj_bits)? {
            let dumped = pickle_dump_reduce_value(_py, state, reduce_bits, Some(obj_bits));
            if !obj_from_bits(reduce_bits).is_none() {
                dec_ref_bits(_py, reduce_bits);
            }
            return dumped;
        }
        if pickle_dump_default_instance(_py, state, obj_bits, ptr, type_id)? {
            return Ok(());
        }
        let type_name = type_name(_py, obj_from_bits(obj_bits));
        let message = format!("cannot pickle '{type_name}' object");
        Err(raise_exception::<u64>(_py, "TypeError", &message))
    })();
    state.depth = state.depth.saturating_sub(1);
    result
}
