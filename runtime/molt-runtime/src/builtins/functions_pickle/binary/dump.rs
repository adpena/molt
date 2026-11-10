// Binary pickle object serialization (dump) VM.

use super::*;
use crate::BuiltinExc;

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
    let code_bits = match code_bits {
        Ok(value) => value,
        Err(()) => {
            dec_ref_bits(_py, registry_bits);
            return Err(MoltObject::none().bits());
        }
    };
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
    let Some(module_bits) = pickle_attr_optional(_py, obj_bits, crate::NameId::DunderModule)?
    else {
        return Ok(false);
    };
    let Some(name_bits) = pickle_attr_optional(_py, obj_bits, crate::NameId::DunderName)? else {
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
    let emitted = pickle_emit_global_names(
        _py,
        state,
        (module_bits, module_name.as_str()),
        (name_bits, attr_name.as_str()),
    );
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    emitted.map(|()| true)
}

/// The tail of CPython's save_global (`Modules/_pickle.c`): an extension code
/// from protocol 2, else STACK_GLOBAL from protocol 4, whose module and name
/// strings go through save() and so take memo entries, else GLOBAL.
fn pickle_emit_global_names(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    (module_bits, module_name): (u64, &str),
    (name_bits, global_name): (u64, &str),
) -> Result<(), u64> {
    if state.protocol >= 2
        && let Some(code) = pickle_lookup_extension_code(_py, module_name, global_name)?
    {
        if code <= u8::MAX as i64 {
            state.push(PICKLE_OP_EXT1);
            state.push(code as u8);
            return Ok(());
        }
        if code <= u16::MAX as i64 {
            state.push(PICKLE_OP_EXT2);
            state.extend(&(code as u16).to_le_bytes());
            return Ok(());
        }
        if code <= u32::MAX as i64 {
            state.push(PICKLE_OP_EXT4);
            state.extend(&(code as u32).to_le_bytes());
            return Ok(());
        }
    }
    if state.protocol >= PICKLE_PROTO_4 {
        pickle_dump_obj_binary(_py, state, module_bits, true)?;
        pickle_dump_obj_binary(_py, state, name_bits, true)?;
        state.push(PICKLE_OP_STACK_GLOBAL);
        return Ok(());
    }
    pickle_emit_global_opcode(state, module_name, global_name);
    Ok(())
}

fn pickle_dump_unicode_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    text: &str,
) -> Result<(), u64> {
    let raw = text.as_bytes();
    if raw.len() <= u8::MAX as usize && state.protocol >= PICKLE_PROTO_4 {
        state.write_payload(&[PICKLE_OP_SHORT_BINUNICODE, raw.len() as u8], raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.write_payload(
            &pickle_payload_header_u32(PICKLE_OP_BINUNICODE, raw.len()),
            raw,
        );
        return Ok(());
    }
    state.write_payload(&pickle_payload_header_u64(0x8d, raw.len()), raw);
    Ok(())
}

fn pickle_payload_header_u32(op: u8, len: usize) -> [u8; 5] {
    let mut header = [op, 0, 0, 0, 0];
    header[1..].copy_from_slice(&(len as u32).to_le_bytes());
    header
}

fn pickle_payload_header_u64(op: u8, len: usize) -> [u8; 9] {
    let mut header = [op, 0, 0, 0, 0, 0, 0, 0, 0];
    header[1..].copy_from_slice(&(len as u64).to_le_bytes());
    header
}

fn pickle_dump_bytes_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if raw.len() <= u8::MAX as usize {
        state.write_payload(&[PICKLE_OP_SHORT_BINBYTES, raw.len() as u8], raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.write_payload(
            &pickle_payload_header_u32(PICKLE_OP_BINBYTES, raw.len()),
            raw,
        );
        return Ok(());
    }
    state.write_payload(
        &pickle_payload_header_u64(PICKLE_OP_BINBYTES8, raw.len()),
        raw,
    );
    Ok(())
}

fn pickle_dump_bytearray_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if state.protocol >= PICKLE_PROTO_5 {
        state.write_payload(
            &pickle_payload_header_u64(PICKLE_OP_BYTEARRAY8, raw.len()),
            raw,
        );
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
    pickle_dump_long_bytes(state, &pickle_long_bytes_from_i64(value));
}

/// LONG1 or LONG4 over a minimal little-endian two's-complement payload, as
/// CPython's save_long writes any int outside int32 from protocol 2
/// (`Modules/_pickle.c`).
fn pickle_dump_long_bytes(state: &mut PickleDumpState, raw: &[u8]) {
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_LONG1);
        state.push(raw.len() as u8);
        state.extend(raw);
    } else {
        state.push(PICKLE_OP_LONG4);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
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
    Err(PickleError::Pickling.raise(_py, &msg))
}

pub(crate) fn pickle_buffer_value_to_memoryview(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    let view_bits = crate::molt_memoryview_new_attached_body(_py, value_bits);
    if exception_pending(_py) {
        let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
        return Err(PickleError::Unpickling.raise(_py, &msg));
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
    if let Some(raw_method_bits) = pickle_attr_optional(_py, item_bits, crate::NameId::Raw)? {
        let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
        dec_ref_bits(_py, raw_method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return pickle_buffer_value_to_memoryview(_py, raw_bits, "out-of-band buffer");
    }
    Err(PickleError::Unpickling.raise(
        _py,
        "pickle.loads: out-of-band buffer must be bytes-like or expose raw()",
    ))
}

pub(crate) fn pickle_next_external_buffer_bits(
    _py: &crate::PyToken<'_>,
    buffers_iterator: Option<&molt_runtime_core::OwnedRuntimeValue<'_>>,
) -> Result<u64, u64> {
    let Some(iterator) = buffers_iterator else {
        return Err(PickleError::Unpickling
            .raise(_py, "pickle.loads: NEXT_BUFFER requires buffers argument"));
    };
    let Some(item_owner) = molt_runtime_core::iter_next_owned(_py.core_token(), iterator)
        .map_err(|_| MoltObject::none().bits())?
    else {
        return Err(
            PickleError::Unpickling.raise(_py, "pickle.loads: not enough out-of-band buffers")
        );
    };
    let item_bits = item_owner.bits();
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
    let marker_bits =
        match pickle_attr_optional(_py, obj_bits, crate::NameId::DunderMoltPickleBuffer)? {
            Some(bits) => bits,
            None => return Ok(None),
        };
    let is_marker = is_truthy(_py, obj_from_bits(marker_bits));
    dec_ref_bits(_py, marker_bits);
    if !is_marker {
        return Ok(None);
    }
    let raw_method_bits =
        pickle_attr_required(_py, PickleError::Pickling, obj_bits, crate::NameId::Raw)?;
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
    let reducer_bits = match reducer_bits {
        Ok(value) => value,
        Err(()) => {
            return Err(MoltObject::none().bits());
        }
    };
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
    if let Some(reduce_ex_bits) =
        pickle_attr_optional(_py, obj_bits, crate::NameId::DunderReduceEx)?
    {
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
    if let Some(reduce_bits) = pickle_attr_optional(_py, obj_bits, crate::NameId::DunderReduce)? {
        let out_bits = unsafe { call_callable0(_py, reduce_bits) };
        dec_ref_bits(_py, reduce_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    Ok(None)
}

/// CPython's batch loop for exact dicts and sets: MARK, up to
/// PICKLE_BATCH_SIZE items, `close`, repeated while the last batch was full,
/// so a count that is a multiple of the batch size ends with an empty batch.
fn pickle_dump_full_batches(
    state: &mut PickleDumpState,
    count: usize,
    close: u8,
    mut dump_at: impl FnMut(&mut PickleDumpState, usize) -> Result<(), u64>,
) -> Result<(), u64> {
    let mut at = 0usize;
    loop {
        state.push(PICKLE_OP_MARK);
        let mut written = 0usize;
        while at < count && written < PICKLE_BATCH_SIZE {
            dump_at(state, at)?;
            at += 1;
            written += 1;
        }
        state.push(close);
        if written != PICKLE_BATCH_SIZE {
            return Ok(());
        }
    }
}

/// A dictitems element's key and value; anything but a 2-tuple is the
/// TypeError CPython's batch_dict raises.
fn pickle_dict_item_pair(_py: &crate::PyToken<'_>, item_bits: u64) -> Result<[u64; 2], u64> {
    let refuse = || {
        raise_exception(
            _py,
            BuiltinExc::TypeError,
            "dict items iterator must return 2-tuples",
        )
    };
    let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
        return Err(refuse());
    };
    if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
        return Err(refuse());
    }
    let Some(fields) = (unsafe {
        crate::object::seq_access::snapshot(_py, item_ptr, "sequence snapshot allocation failed")
    }) else {
        return Err(MoltObject::none().bits());
    };
    match fields[..] {
        [key, value] => Ok([key, value]),
        _ => Err(refuse()),
    }
}

fn pickle_dump_iterator_item(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    item_bits: u64,
    dict_items: bool,
) -> Result<(), u64> {
    if dict_items {
        let [key, value] = pickle_dict_item_pair(_py, item_bits)?;
        pickle_dump_obj_binary(_py, state, key, true)?;
        pickle_dump_obj_binary(_py, state, value, true)
    } else {
        pickle_dump_obj_binary(_py, state, item_bits, true)
    }
}

// save_reduce already admitted these slots with the canonical PyIter_Check
// predicate. Consume that exact iterator; __iter__ is not part of this path.
fn pickle_dump_items_from_iterator(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    iterator_bits: u64,
    dict_items: bool,
) -> Result<(), u64> {
    let iterator_owner =
        molt_runtime_core::OwnedRuntimeValue::retain(_py.core_token(), iterator_bits);
    let next = || {
        molt_runtime_core::iter_next_owned(_py.core_token(), &iterator_owner)
            .map_err(|_| MoltObject::none().bits())
    };
    let (single, batch) = if dict_items {
        (PICKLE_OP_SETITEM, PICKLE_OP_SETITEMS)
    } else {
        (PICKLE_OP_APPEND, PICKLE_OP_APPENDS)
    };
    // CPython's batch_list and batch_dict (`Modules/_pickle.c`): nothing for an
    // empty iterator; a lone item at a batch start takes APPEND or SETITEM;
    // otherwise MARK, up to PICKLE_BATCH_SIZE items and APPENDS or SETITEMS,
    // repeated while a batch fills. The first item of a batch is validated
    // before the next one is fetched; later items just before they are saved.
    loop {
        let Some(first) = next()? else {
            break;
        };
        if dict_items {
            pickle_dict_item_pair(_py, first.bits())?;
        }
        let Some(second) = next()? else {
            pickle_dump_iterator_item(_py, state, first.bits(), dict_items)?;
            state.push(single);
            break;
        };
        state.push(PICKLE_OP_MARK);
        pickle_dump_iterator_item(_py, state, first.bits(), dict_items)?;
        drop(first);
        let mut written = 1usize;
        let mut pending = Some(second);
        while let Some(item) = pending.take() {
            pickle_dump_iterator_item(_py, state, item.bits(), dict_items)?;
            written += 1;
            if written == PICKLE_BATCH_SIZE {
                break;
            }
            pending = next()?;
        }
        state.push(batch);
        if written != PICKLE_BATCH_SIZE {
            break;
        }
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
        return Err(PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple"));
    };
    let reduce_type = unsafe { object_type_id(ptr) };
    if reduce_type == TYPE_ID_STRING {
        let Some(global_name) = string_obj_to_owned(obj_from_bits(reduce_bits)) else {
            return Err(
                PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple")
            );
        };
        let Some(obj_bits) = obj_bits else {
            return Err(
                PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple")
            );
        };
        let Some(module_bits) = pickle_attr_optional(_py, obj_bits, crate::NameId::DunderModule)?
        else {
            return Err(
                PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple")
            );
        };
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            dec_ref_bits(_py, module_bits);
            return Err(
                PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple")
            );
        };
        let resolved_bits =
            match pickle_resolve_global_bits(_py, module_name.as_str(), global_name.as_str()) {
                Ok(bits) => bits,
                Err(err_bits) => {
                    dec_ref_bits(_py, module_bits);
                    return Err(err_bits);
                }
            };
        let matches = resolved_bits == obj_bits;
        if !obj_from_bits(resolved_bits).is_none() {
            dec_ref_bits(_py, resolved_bits);
        }
        if !matches {
            dec_ref_bits(_py, module_bits);
            let obj_type = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!(
                "Can't pickle {obj_type}: it's not the same object as {}.{}",
                module_name, global_name
            );
            return Err(PickleError::Pickling.raise(_py, &msg));
        }
        let emitted = pickle_emit_global_names(
            _py,
            state,
            (module_bits, module_name.as_str()),
            (reduce_bits, global_name.as_str()),
        );
        dec_ref_bits(_py, module_bits);
        emitted?;
        let _ = pickle_memo_store_if_absent(state, obj_bits);
        return Ok(());
    }
    if reduce_type != TYPE_ID_TUPLE {
        return Err(PickleError::Pickling.raise(_py, "__reduce__ must return a string or tuple"));
    }
    let Some(fields) = (unsafe {
        crate::object::seq_access::snapshot(_py, ptr, "sequence snapshot allocation failed")
    }) else {
        return Err(MoltObject::none().bits());
    };
    if !(2..=6).contains(&fields.len()) {
        return Err(PickleError::Pickling.raise(
            _py,
            "tuple returned by __reduce__ must contain 2 through 6 elements",
        ));
    }
    let callable_bits = fields[0];
    let callable_check = molt_is_callable_attached_body(_py, callable_bits);
    if !is_truthy(_py, obj_from_bits(callable_check)) {
        return Err(PickleError::Pickling.raise(
            _py,
            "first item of the tuple returned by __reduce__ must be callable",
        ));
    }
    let args_bits = fields[1];
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(PickleError::Pickling.raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(PickleError::Pickling.raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    }
    for (index, position) in [(3, "fourth"), (4, "fifth")] {
        if let Some(&iterator) = fields.get(index)
            && !obj_from_bits(iterator).is_none()
        {
            let valid = unsafe { crate::is_iterator_bits(_py, iterator) }
                .map_err(|_| MoltObject::none().bits())?;
            if !valid {
                let value_type = type_name(_py, obj_from_bits(iterator));
                let msg = format!(
                    "{position} element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
                );
                return Err(PickleError::Pickling.raise(_py, &msg));
            }
        }
    }
    if fields.len() >= 6 && !obj_from_bits(fields[5]).is_none() {
        let setter_check = molt_is_callable_attached_body(_py, fields[5]);
        if !is_truthy(_py, obj_from_bits(setter_check)) {
            let value_type = type_name(_py, obj_from_bits(fields[5]));
            let msg = format!(
                "sixth element of the tuple returned by __reduce__ must be a function, not {value_type}"
            );
            return Err(PickleError::Pickling.raise(_py, &msg));
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
                    return Err(PickleError::Pickling
                        .raise(_py, "pickle reducer state_setter requires object context"));
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
        pickle_dump_items_from_iterator(_py, state, fields[3], false)?;
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        pickle_dump_items_from_iterator(_py, state, fields[4], true)?;
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
        return Err(PickleError::Pickling.raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(PickleError::Pickling.raise(_py, &msg));
    }
    Ok(())
}

fn pickle_require_dict_bits(_py: &crate::PyToken<'_>, bits: u64, context: &str) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(PickleError::Pickling.raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(PickleError::Pickling.raise(_py, &msg));
    }
    Ok(())
}

fn pickle_default_newobj_args(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<(u64, Option<u64>), u64> {
    if let Some(getnewargs_ex_bits) =
        pickle_attr_optional(_py, obj_bits, crate::NameId::DunderGetnewargsEx)?
    {
        let out_bits = unsafe { call_callable0(_py, getnewargs_ex_bits) };
        dec_ref_bits(_py, getnewargs_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(tuple_ptr) = obj_from_bits(out_bits).as_ptr() else {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            return Err(PickleError::Pickling.raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        };
        if unsafe { object_type_id(tuple_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, out_bits);
            return Err(PickleError::Pickling.raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let Some(fields) = (unsafe {
            crate::object::seq_access::snapshot(
                _py,
                tuple_ptr,
                "sequence snapshot allocation failed",
            )
        }) else {
            dec_ref_bits(_py, out_bits);
            return Err(MoltObject::none().bits());
        };
        if fields.len() != 2 {
            dec_ref_bits(_py, out_bits);
            return Err(PickleError::Pickling.raise(
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

    if let Some(getnewargs_bits) =
        pickle_attr_optional(_py, obj_bits, crate::NameId::DunderGetnewargs)?
    {
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

fn pickle_default_instance_state(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    if let Some(getstate_bits) = pickle_attr_optional(_py, obj_bits, crate::NameId::DunderGetstate)?
    {
        let state_bits = unsafe { call_callable0(_py, getstate_bits) };
        dec_ref_bits(_py, getstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(state_bits));
    }
    if unsafe { crate::object::object_has_class_shape(ptr) } {
        let state = crate::object::ops_builtins::molt_object_getstate_attached_body(_py, obj_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state);
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(state));
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
    if type_id != crate::TYPE_ID_OBJECT || !unsafe { crate::object::object_has_class_shape(ptr) } {
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
                return Err(PickleError::Pickling.raise(_py, "pickle.dumps: kwargs must be dict"));
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                return Err(PickleError::Pickling.raise(_py, "pickle.dumps: kwargs must be dict"));
            }
            if unsafe { crate::dict_len(dict_ptr) == 0 } {
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
        if let Some(state_bits) = pickle_default_instance_state(_py, obj_bits, ptr)? {
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
    // CPython's save() starts at an opcode boundary, where a full frame commits.
    state.opcode_boundary();
    if state.depth >= PICKLE_RECURSION_LIMIT {
        return Err(
            PickleError::Pickling.raise(_py, "pickle.dumps: maximum recursion depth exceeded")
        );
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
            return Err(PickleError::Pickling.raise(_py, &msg));
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == crate::TYPE_ID_BIGINT {
            let big = unsafe { crate::builtins::numbers::bigint_ref(ptr) };
            match num_traits::ToPrimitive::to_i64(big) {
                Some(value) => pickle_dump_int_binary(state, value),
                None => pickle_dump_long_bytes(state, &big.to_signed_bytes_le()),
            }
            return Ok(());
        }
        if type_id == TYPE_ID_STRING {
            let text = string_obj_to_owned(obj).ok_or_else(|| {
                PickleError::Pickling.raise(_py, "pickle.dumps: string conversion failed")
            })?;
            pickle_dump_unicode_binary(_py, state, text.as_str())?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTES {
            let raw = unsafe { bytes_like_slice(ptr) }.ok_or_else(|| {
                PickleError::Pickling.raise(_py, "pickle.dumps: bytes conversion failed")
            })?;
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
            let raw = unsafe { bytes_like_slice(ptr) }.ok_or_else(|| {
                PickleError::Pickling.raise(_py, "pickle.dumps: bytearray conversion failed")
            })?;
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
                return Err(PickleError::Pickling
                    .raise(_py, "pickle.dumps: PickleBuffer.raw() must be bytes-like"));
            };
            let raw = unsafe { bytes_like_slice(payload_ptr) }.ok_or_else(|| {
                PickleError::Pickling
                    .raise(_py, "pickle.dumps: PickleBuffer.raw() must be bytes-like")
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
            let Some(values) = (unsafe {
                crate::object::seq_access::snapshot(_py, ptr, "sequence snapshot allocation failed")
            }) else {
                return Err(MoltObject::none().bits());
            };
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
                    for &entry in values.iter() {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_TUPLE);
                }
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_LIST && unsafe { crate::object::iterable::builtin_receiver(_py, ptr) }
        {
            state.push(PICKLE_OP_EMPTY_LIST);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let Some(values) = (unsafe {
                crate::object::seq_access::snapshot(_py, ptr, "sequence snapshot allocation failed")
            }) else {
                return Err(MoltObject::none().bits());
            };
            // CPython's batch_list_exact: a one-item list takes APPEND; any
            // other nonempty list takes MARK ... APPENDS per PICKLE_BATCH_SIZE.
            if let [only] = values[..] {
                pickle_dump_obj_binary(_py, state, only, true)?;
                state.push(PICKLE_OP_APPEND);
            } else {
                for chunk in values.chunks(PICKLE_BATCH_SIZE) {
                    state.push(PICKLE_OP_MARK);
                    for &entry in chunk {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_APPENDS);
                }
            }
            return Ok(());
        }
        if type_id == TYPE_ID_DICT {
            state.push(PICKLE_OP_EMPTY_DICT);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let pairs = unsafe {
                crate::object::ops_dict::dict_snapshot(
                    _py,
                    ptr,
                    crate::object::ops_dict::DictSnapshotKind::Entries,
                )
            }
            .ok_or_else(|| MoltObject::none().bits())?;
            // CPython's batch_dict_exact: a one-entry dict takes SETITEM;
            // otherwise MARK ... SETITEMS per PICKLE_BATCH_SIZE, repeated while
            // a batch fills, so an exact multiple ends with an empty batch.
            let (entries, _) = pairs.as_chunks::<2>();
            if let [[key, value]] = entries {
                pickle_dump_obj_binary(_py, state, *key, true)?;
                pickle_dump_obj_binary(_py, state, *value, true)?;
                state.push(PICKLE_OP_SETITEM);
            } else if !entries.is_empty() {
                pickle_dump_full_batches(state, entries.len(), PICKLE_OP_SETITEMS, |state, at| {
                    let [key, value] = entries[at];
                    pickle_dump_obj_binary(_py, state, key, true)?;
                    pickle_dump_obj_binary(_py, state, value, true)
                })?;
            }
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_EMPTY_SET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                let values = unsafe { crate::object::ops_set::set_snapshot(_py, ptr) }
                    .ok_or_else(|| MoltObject::none().bits())?;
                // CPython's save_set: MARK ... ADDITEMS per PICKLE_BATCH_SIZE,
                // repeated while a batch fills.
                if !values.is_empty() {
                    pickle_dump_full_batches(
                        state,
                        values.len(),
                        PICKLE_OP_ADDITEMS,
                        |state, at| pickle_dump_obj_binary(_py, state, values[at], true),
                    )?;
                }
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "set");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::object::ops_set::set_snapshot(_py, ptr) }
                .ok_or_else(|| MoltObject::none().bits())?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for &entry in values.iter() {
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
                let values = unsafe { crate::object::ops_set::set_snapshot(_py, ptr) }
                    .ok_or_else(|| MoltObject::none().bits())?;
                for &entry in values.iter() {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_FROZENSET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "frozenset");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::object::ops_set::set_snapshot(_py, ptr) }
                .ok_or_else(|| MoltObject::none().bits())?;
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for &entry in values.iter() {
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
        Err(raise_exception::<u64>(_py, BuiltinExc::TypeError, &message))
    })();
    state.depth = state.depth.saturating_sub(1);
    result
}
