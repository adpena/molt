// Binary pickle dumps/loads C-ABI entrypoints and multiprocessing codec.

use super::*;
use crate::PyToken;

#[inline]
fn pickle_payload_len(_py: &PyToken<'_>, len: u64) -> Result<usize, u64> {
    usize::try_from(len).map_err(|_| {
        raise_exception::<u64>(
            _py,
            "OverflowError",
            "pickle payload length exceeds the active address space",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_core(
    obj_bits: u64,
    protocol_bits: u64,
    _fix_imports_bits: u64,
    persistent_id_bits: u64,
    buffer_callback_bits: u64,
    dispatch_table_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if !(-1..=PICKLE_PROTO_5).contains(&protocol) {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "pickle protocol must be in range -1..5",
            );
        }
        let actual_protocol = if protocol < 0 {
            PICKLE_PROTO_5
        } else {
            protocol
        };
        if actual_protocol <= 1 {
            return molt_pickle_dumps_protocol01(
                obj_bits,
                MoltObject::from_int(actual_protocol).bits(),
            );
        }
        let persistent_id =
            match pickle_option_callable_bits(_py, persistent_id_bits, "persistent_id") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let buffer_callback =
            match pickle_option_callable_bits(_py, buffer_callback_bits, "buffer_callback") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let dispatch_table = if obj_from_bits(dispatch_table_bits).is_none() {
            None
        } else {
            Some(dispatch_table_bits)
        };
        let mut state = PickleDumpState::new(
            actual_protocol,
            persistent_id,
            buffer_callback,
            dispatch_table,
        );
        if state.buffer_callback_bits.is_some() && actual_protocol < PICKLE_PROTO_5 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "buffer_callback requires protocol 5 or higher",
            );
        }
        pickle_emit_proto_header(&mut state);
        if let Err(err_bits) = pickle_dump_obj_binary(_py, &mut state, obj_bits, true) {
            return err_bits;
        }
        state.push(PICKLE_OP_STOP);
        let out_ptr = crate::alloc_bytes(_py, state.out.as_slice());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_core(
    data_bits: u64,
    _fix_imports_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    persistent_load_bits: u64,
    find_class_bits: u64,
    buffers_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let encoding = if let Some(text) = string_obj_to_owned(obj_from_bits(encoding_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle encoding must be str");
        };
        let errors = if let Some(text) = string_obj_to_owned(obj_from_bits(errors_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle errors must be str");
        };
        let persistent_load =
            match pickle_option_callable_bits(_py, persistent_load_bits, "persistent_load") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let find_class = match pickle_option_callable_bits(_py, find_class_bits, "find_class") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let data = match pickle_input_to_bytes(_py, data_bits) {
            Ok(bytes) => bytes,
            Err(err_bits) => return err_bits,
        };
        let buffers_iter = if obj_from_bits(buffers_bits).is_none() {
            None
        } else {
            let iter_bits = molt_iter(buffers_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            Some(iter_bits)
        };
        if data.first().is_none_or(|op| *op != PICKLE_OP_PROTO) {
            let text = match String::from_utf8(data) {
                Ok(value) => value,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "pickle.loads: protocol 0/1 payload must be UTF-8",
                    );
                }
            };
            let text_ptr = alloc_string(_py, text.as_bytes());
            if text_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let text_bits = MoltObject::from_ptr(text_ptr).bits();
            let out_bits = molt_pickle_loads_protocol01(text_bits);
            dec_ref_bits(_py, text_bits);
            return out_bits;
        }

        let mut idx: usize = 0;
        let mut stack: Vec<PickleVmItem> = Vec::new();
        let mut memo: Vec<Option<PickleVmItem>> = Vec::new();
        while idx < data.len() {
            let op = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                Ok(value) => value,
                Err(err_bits) => return err_bits,
            };
            match op {
                PICKLE_OP_STOP => break,
                PICKLE_OP_POP => {
                    if stack.pop().is_none() {
                        return pickle_raise(_py, "pickle.loads: stack underflow");
                    }
                }
                PICKLE_OP_POP_MARK => {
                    let mut found_mark = false;
                    while let Some(item) = stack.pop() {
                        if matches!(item, PickleVmItem::Mark) {
                            found_mark = true;
                            break;
                        }
                    }
                    if !found_mark {
                        return pickle_raise(_py, "pickle.loads: mark not found");
                    }
                }
                PICKLE_OP_PROTO => {
                    let version = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if version > PICKLE_PROTO_5 as u8 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unsupported pickle protocol",
                        );
                    }
                }
                PICKLE_OP_FRAME => {
                    if pickle_read_u64_le(data.as_slice(), &mut idx, _py).is_err() {
                        return MoltObject::none().bits();
                    }
                }
                PICKLE_OP_NEXT_BUFFER => {
                    let bits = match pickle_next_external_buffer_bits(_py, buffers_iter) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_READONLY_BUFFER => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let view_bits =
                        match pickle_buffer_value_to_memoryview(_py, value_bits, "READONLY_BUFFER")
                        {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                    let readonly_bits = if let Some(toreadonly_bits) =
                        match pickle_attr_optional(_py, view_bits, b"toreadonly") {
                            Ok(bits) => bits,
                            Err(err_bits) => return err_bits,
                        } {
                        let out_bits = unsafe { call_callable0(_py, toreadonly_bits) };
                        dec_ref_bits(_py, toreadonly_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        out_bits
                    } else {
                        view_bits
                    };
                    stack.push(PickleVmItem::Value(readonly_bits));
                }
                PICKLE_OP_MARK => stack.push(PickleVmItem::Mark),
                PICKLE_OP_NONE => stack.push(PickleVmItem::Value(MoltObject::none().bits())),
                PICKLE_OP_NEWTRUE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(true).bits()))
                }
                PICKLE_OP_NEWFALSE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(false).bits()))
                }
                PICKLE_OP_INT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid INT payload"),
                    };
                    let bits = match pickle_parse_int_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BININT => {
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, 4, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let value = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                    stack.push(PickleVmItem::Value(
                        MoltObject::from_int(value as i64).bits(),
                    ));
                }
                PICKLE_OP_BININT1 => {
                    let value = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_BININT2 => {
                    let value = match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_LONG => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid LONG payload"),
                    };
                    let bits = match pickle_parse_long_line_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_LONG1 | PICKLE_OP_LONG4 => {
                    let size = if op == PICKLE_OP_LONG1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_parse_long_bytes_bits(_py, raw) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_FLOAT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid FLOAT payload"),
                    };
                    let bits = match pickle_parse_float_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BINFLOAT => {
                    let value = match pickle_read_f64_be(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_float(value).bits()));
                }
                PICKLE_OP_STRING => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid STRING payload"),
                    };
                    let parsed = match pickle_parse_string_literal(text) {
                        Ok(v) => v,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let ptr = alloc_string(_py, parsed.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_UNICODE => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, line, "UNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BINUNICODE | PICKLE_OP_SHORT_BINUNICODE => {
                    let size = if op == PICKLE_OP_SHORT_BINUNICODE {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, raw, "BINUNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SHORT_BINBYTES | PICKLE_OP_BINBYTES | PICKLE_OP_BINBYTES8 => {
                    let size = match op {
                        PICKLE_OP_SHORT_BINBYTES => {
                            match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        PICKLE_OP_BINBYTES => {
                            match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        _ => match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => match pickle_payload_len(_py, v) {
                                Ok(v) => v,
                                Err(err_bits) => return err_bits,
                            },
                            Err(err_bits) => return err_bits,
                        },
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = crate::alloc_bytes(_py, raw);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BYTEARRAY8 => {
                    let size = match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => match pickle_payload_len(_py, v) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        },
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bytes_ptr = crate::alloc_bytes(_py, raw);
                    if bytes_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                    let out_bits =
                        pickle_call_with_args(_py, builtin_classes(_py).bytearray, &[bytes_bits]);
                    dec_ref_bits(_py, bytes_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(out_bits));
                }
                PICKLE_OP_EMPTY_TUPLE => {
                    let ptr = alloc_tuple(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE1 | PICKLE_OP_TUPLE2 | PICKLE_OP_TUPLE3 => {
                    let needed = if op == PICKLE_OP_TUPLE1 {
                        1
                    } else if op == PICKLE_OP_TUPLE2 {
                        2
                    } else {
                        3
                    };
                    let mut items: Vec<u64> = Vec::with_capacity(needed);
                    for _ in 0..needed {
                        let bits = match pickle_vm_pop_value(_py, &mut stack) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        items.push(bits);
                    }
                    items.reverse();
                    let ptr = alloc_tuple(_py, items.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_tuple(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_EMPTY_LIST => {
                    let ptr = alloc_list_with_capacity(_py, &[], 0);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_LIST => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_APPEND => {
                    let item_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let _ = crate::molt_list_append(list_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_APPENDS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        let _ = crate::molt_list_append(list_bits, bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_EMPTY_DICT => {
                    let ptr = alloc_dict_with_pairs(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_DICT => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if !values.len().is_multiple_of(2) {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SETITEM => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let key_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_SETITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    }
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if !values.len().is_multiple_of(2) {
                        return pickle_raise(
                            _py,
                            "pickle.loads: setitems has odd number of values",
                        );
                    }
                    let mut pair_idx = 0usize;
                    while pair_idx + 1 < values.len() {
                        unsafe {
                            crate::dict_set_in_place(
                                _py,
                                dict_ptr,
                                values[pair_idx],
                                values[pair_idx + 1],
                            );
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        pair_idx += 2;
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_EMPTY_SET => {
                    let bits = crate::molt_set_new(0);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_ADDITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let set_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(set_ptr) = obj_from_bits(set_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: additems target is not set");
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        unsafe {
                            crate::set_add_in_place(
                                _py,
                                set_ptr,
                                bits,
                                crate::HashContext::SetElement,
                            );
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(set_bits));
                }
                PICKLE_OP_FROZENSET => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let list_bits = MoltObject::from_ptr(list_ptr).bits();
                    let tuple_ptr = alloc_tuple(_py, &[list_bits]);
                    dec_ref_bits(_py, list_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    let out_bits =
                        pickle_apply_reduce_bits(_py, builtin_classes(_py).frozenset, args_bits);
                    dec_ref_bits(_py, args_bits);
                    match out_bits {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_GLOBAL => {
                    let module = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_text = match pickle_decode_utf8(_py, module, "GLOBAL module") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name_text = match pickle_decode_utf8(_py, name, "GLOBAL name") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    if let Some(global) = pickle_resolve_global(&module_text, &name_text) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(
                            _py,
                            &module_text,
                            &name_text,
                            find_class,
                        ) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_STACK_GLOBAL => {
                    let name_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(module) = string_obj_to_owned(obj_from_bits(module_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL module must be str");
                    };
                    let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL name must be str");
                    };
                    if let Some(global) = pickle_resolve_global(&module, &name) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(_py, &module, &name, find_class) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_REDUCE => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let callable_item = match stack.pop() {
                        Some(v) => v,
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    match pickle_apply_reduce_vm(_py, callable_item, args_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, None) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ_EX => {
                    let kwargs_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, Some(kwargs_bits)) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BUILD => {
                    let state_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let inst_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_build(_py, inst_bits, state_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PUT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid PUT payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_BINPUT => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_LONG_BINPUT => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_MEMOIZE => {
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    memo.push(Some(item));
                }
                PICKLE_OP_GET => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid GET payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BINGET => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_LONG_BINGET => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PERSID => {
                    let pid_line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let pid_text = match pickle_decode_utf8(_py, pid_line, "PERSID payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(pid_bits) = alloc_string_bits(_py, pid_text.as_str()) else {
                        return MoltObject::none().bits();
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        dec_ref_bits(_py, pid_bits);
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    dec_ref_bits(_py, pid_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_BINPERSID => {
                    let pid_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_EXT1 | PICKLE_OP_EXT2 | PICKLE_OP_EXT4 => {
                    let code = if op == PICKLE_OP_EXT1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else if op == PICKLE_OP_EXT2 {
                        match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    match pickle_lookup_extension_bits(_py, code, find_class) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                // Python 2 string opcodes.
                b'U' => {
                    let size = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                b'T' => {
                    let size = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                _ => {
                    let msg = format!("pickle.loads: unsupported opcode 0x{op:02x}");
                    return pickle_raise(_py, msg.as_str());
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_vm_item_to_bits(_py, item) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_multiprocessing_codec_dumps(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let protocol_bits = MoltObject::from_int(PICKLE_PROTO_5).bits();
        let true_bits = MoltObject::from_bool(true).bits();
        let none_bits = MoltObject::none().bits();
        molt_pickle_dumps_core(
            obj_bits,
            protocol_bits,
            true_bits,
            none_bits,
            none_bits,
            none_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_multiprocessing_codec_loads(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let true_bits = MoltObject::from_bool(true).bits();
        let none_bits = MoltObject::none().bits();
        let encoding_ptr = alloc_string(_py, b"ASCII");
        if encoding_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let errors_ptr = alloc_string(_py, b"strict");
        if errors_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(encoding_ptr).bits());
            return MoltObject::none().bits();
        }
        let encoding_bits = MoltObject::from_ptr(encoding_ptr).bits();
        let errors_bits = MoltObject::from_ptr(errors_ptr).bits();
        let out_bits = molt_pickle_loads_core(
            data_bits,
            true_bits,
            encoding_bits,
            errors_bits,
            none_bits,
            none_bits,
            none_bits,
        );
        dec_ref_bits(_py, encoding_bits);
        dec_ref_bits(_py, errors_bits);
        out_bits
    })
}
