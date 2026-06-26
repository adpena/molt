// Text pickle protocol 0/1 encoder, decoder, and stack VM authority.

use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_encode_protocol0(parts_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parts_obj = obj_from_bits(parts_bits);
        let Some(parts_ptr) = parts_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        };
        let parts_type = unsafe { object_type_id(parts_ptr) };
        if parts_type != TYPE_ID_LIST && parts_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        }
        let elems = unsafe { seq_vec_ref(parts_ptr) };
        let mut joined = String::new();
        for &elem_bits in elems.iter() {
            let Some(chunk) = string_obj_to_owned(obj_from_bits(elem_bits)) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "pickle opcode chunks must contain str values",
                );
            };
            joined.push_str(&chunk);
        }
        let bytes_ptr = crate::alloc_bytes(_py, joined.as_bytes());
        if bytes_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(bytes_ptr).bits()
        }
    })
}

#[derive(Clone, Debug)]
enum PickleStackItem {
    Value(u64),
    Mark,
    Global(PickleGlobal),
}

fn pickle_dump_global(out: &mut String, module: &str, name: &str) {
    out.push('c');
    out.push_str(module);
    out.push('\n');
    out.push_str(name);
    out.push('\n');
}

fn pickle_string_repr(_py: &crate::PyToken<'_>, value: &str) -> Result<String, u64> {
    let Some(value_bits) = alloc_string_bits(_py, value) else {
        return Err(MoltObject::none().bits());
    };
    let rendered = format_obj(_py, obj_from_bits(value_bits));
    dec_ref_bits(_py, value_bits);
    Ok(rendered)
}

fn pickle_dump_list_payload(
    _py: &crate::PyToken<'_>,
    values: &[u64],
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    out.push('(');
    out.push('l');
    for &item_bits in values {
        pickle_dump_obj(_py, item_bits, protocol, out)?;
        out.push('a');
    }
    Ok(())
}

fn pickle_dump_obj(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_none() {
        out.push('N');
        return Ok(());
    }
    if let Some(value) = obj.as_bool() {
        if value {
            out.push_str("I01\n");
        } else {
            out.push_str("I00\n");
        }
        return Ok(());
    }
    if let Some(value) = obj.as_int() {
        out.push('I');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    if let Some(value) = obj.as_float() {
        out.push('F');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    let Some(ptr) = obj.as_ptr() else {
        let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
        return Err(pickle_raise(_py, &message));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == crate::TYPE_ID_BIGINT {
        out.push('I');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == TYPE_ID_STRING {
        out.push('S');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTES {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytes payload",
            ));
        };
        pickle_dump_global(out, "_codecs", "encode");
        out.push('(');
        let latin1 = pickle_decode_latin1(raw);
        let latin1_repr = pickle_string_repr(_py, &latin1)?;
        out.push('S');
        out.push_str(&latin1_repr);
        out.push('\n');
        let encoding_repr = pickle_string_repr(_py, "latin1")?;
        out.push('S');
        out.push_str(&encoding_repr);
        out.push('\n');
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTEARRAY {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytearray payload",
            ));
        };
        pickle_dump_global(out, "builtins", "bytearray");
        out.push('(');
        let bytes_ptr = crate::alloc_bytes(_py, raw);
        if bytes_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let dumped = pickle_dump_obj(_py, bytes_bits, protocol, out);
        dec_ref_bits(_py, bytes_bits);
        dumped?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == TYPE_ID_TUPLE {
        out.push('(');
        for &item_bits in unsafe { seq_vec_ref(ptr) }.iter() {
            pickle_dump_obj(_py, item_bits, protocol, out)?;
        }
        out.push('t');
        return Ok(());
    }
    if type_id == TYPE_ID_LIST {
        let values = unsafe { seq_vec_ref(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        return Ok(());
    }
    if type_id == TYPE_ID_DICT {
        out.push('(');
        out.push('d');
        let pairs = unsafe { crate::dict_order(ptr).clone() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            pickle_dump_obj(_py, pairs[idx], protocol, out)?;
            pickle_dump_obj(_py, pairs[idx + 1], protocol, out)?;
            out.push('s');
            idx += 2;
        }
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SET {
        pickle_dump_global(out, "builtins", "set");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_FROZENSET {
        pickle_dump_global(out, "builtins", "frozenset");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SLICE {
        pickle_dump_global(out, "builtins", "slice");
        out.push('(');
        pickle_dump_obj(_py, unsafe { crate::slice_start_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_stop_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_step_bits(ptr) }, protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
    Err(pickle_raise(_py, &message))
}

fn pickle_read_line<'a>(
    _py: &crate::PyToken<'_>,
    text: &'a str,
    idx: &mut usize,
) -> Result<&'a str, u64> {
    let bytes = text.as_bytes();
    if *idx > bytes.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = bytes[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&text[start..end])
}

fn pickle_parse_memo_key(_py: &crate::PyToken<'_>, text: &str) -> Result<i64, u64> {
    text.parse::<i64>()
        .map_err(|_| pickle_raise(_py, "pickle.loads: invalid memo key"))
}

fn pickle_stack_item_to_value(
    _py: &crate::PyToken<'_>,
    item: &PickleStackItem,
) -> Result<u64, u64> {
    match item {
        PickleStackItem::Value(bits) => Ok(*bits),
        PickleStackItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
) -> Result<Vec<PickleStackItem>, u64> {
    let mut out: Vec<PickleStackItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleStackItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_items_to_value_bits(
    _py: &crate::PyToken<'_>,
    items: &[PickleStackItem],
) -> Result<Vec<u64>, u64> {
    let mut out: Vec<u64> = Vec::with_capacity(items.len());
    for item in items {
        out.push(pickle_stack_item_to_value(_py, item)?);
    }
    Ok(out)
}

fn pickle_pop_stack_item(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<PickleStackItem, u64> {
    stack.pop().ok_or_else(|| pickle_raise(_py, message))
}

fn pickle_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<u64, u64> {
    let item = pickle_pop_stack_item(_py, stack, message)?;
    pickle_stack_item_to_value(_py, &item)
}

fn pickle_apply_reduce(
    _py: &crate::PyToken<'_>,
    func_item: PickleStackItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args: Vec<u64> = unsafe { seq_vec_ref(args_ptr).to_vec() };
    match func_item {
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark cannot be called")),
        PickleStackItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 2 {
                let Some(name) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                name
            } else {
                "utf-8".to_string()
            };
            pickle_encode_text(_py, &text, &encoding)
        }
        PickleStackItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
        PickleStackItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_protocol01(obj_bits: u64, protocol_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if protocol != 0 && protocol != 1 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "only pickle protocols 0 and 1 are supported",
            );
        }
        let mut out = String::new();
        if let Err(err_bits) = pickle_dump_obj(_py, obj_bits, protocol, &mut out) {
            return err_bits;
        }
        out.push('.');
        let out_ptr = crate::alloc_bytes(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_protocol01(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle data must be str");
        };
        let bytes = text.as_bytes();
        let mut idx: usize = 0;
        let mut stack: Vec<PickleStackItem> = Vec::new();
        let mut memo: HashMap<i64, PickleStackItem> = HashMap::new();
        while idx < bytes.len() {
            let op = bytes[idx] as char;
            idx += 1;
            match op {
                '.' => break,
                'N' => stack.push(PickleStackItem::Value(MoltObject::none().bits())),
                'I' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if line == "01" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(true).bits()));
                    } else if line == "00" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(false).bits()));
                    } else {
                        let int_bits = match pickle_parse_int_bits(_py, line) {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                        stack.push(PickleStackItem::Value(int_bits));
                    }
                }
                'L' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let int_bits = match pickle_parse_long_line_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(int_bits));
                }
                'F' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let float_bits = match pickle_parse_float_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(float_bits));
                }
                'S' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let parsed = match pickle_parse_string_literal(line) {
                        Ok(value) => value,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let out_ptr = alloc_string(_py, parsed.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                'V' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_ptr = alloc_string(_py, line.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                '(' => stack.push(PickleStackItem::Mark),
                't' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let tuple_ptr = alloc_tuple(_py, values.as_slice());
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(tuple_ptr).bits(),
                    ));
                }
                'l' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(list_ptr).bits(),
                    ));
                }
                'd' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if values.len() % 2 != 0 {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let dict_ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if dict_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(dict_ptr).bits(),
                    ));
                }
                'a' => {
                    let item_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_LIST {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    }
                    let _ = crate::molt_list_append(target_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                's' => {
                    let value_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let key_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, target_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                'c' => {
                    let module = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(global) = pickle_resolve_global(module, name) else {
                        let message =
                            format!("pickle.loads: unsupported global {}.{}", module, name);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(PickleStackItem::Global(global));
                }
                'R' => {
                    let args_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let func_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_stack_item_to_value(_py, &args_item) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_bits = match pickle_apply_reduce(_py, func_item, args_bits) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(out_bits));
                }
                'p' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    memo.insert(key, item.clone());
                    stack.push(item);
                }
                'g' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(item) = memo.get(&key).cloned() else {
                        let message = format!("pickle.loads: memo key {} missing", key);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(item);
                }
                _ => {
                    let message = format!("pickle.loads: unsupported opcode {:?}", op);
                    return pickle_raise(_py, &message);
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_stack_item_to_value(_py, item) {
            Ok(value) => value,
            Err(err_bits) => err_bits,
        }
    })
}
