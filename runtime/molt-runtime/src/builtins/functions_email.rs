use molt_obj_model::MoltObject;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::{
    TYPE_ID_LIST, TYPE_ID_TUPLE,
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bytes_like_slice,
    call_callable0, call_callable2,
    clear_exception, dec_ref_bits, exception_pending, format_obj,
    inc_ref_bits, is_truthy, maybe_ptr_from_bits, missing_bits,
    molt_exception_last, molt_getattr_builtin, molt_is_callable,
    obj_from_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_i64, type_of_bits,
};
use memchr::memmem;

struct MoltEmailMessage {
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
    filename: Option<String>,
    parts: Vec<MoltEmailMessage>,
    multipart_subtype: Option<String>,
}

    index: HashMap<String, Vec<usize>>,
    items_list_cache: Option<u64>,
}


#[derive(Clone)]
struct MoltCookieEntry {
    name: String,
    value: String,
    domain: String,
    path: String,
}

#[derive(Clone, Default)]
struct MoltCookieJar {
    cookies: Vec<MoltCookieEntry>,
}

struct MoltSocketServerPending {
    request: Vec<u8>,
    response: Option<Vec<u8>>,
}

struct MoltSocketServerRuntime {
    next_request_id: u64,
    pending_by_server: HashMap<u64, VecDeque<u64>>,
    pending_requests: HashMap<u64, MoltSocketServerPending>,
    request_server: HashMap<u64, u64>,
    closed_servers: HashSet<u64>,
}

const THIS_ENCODED: &str = concat!(
    "Gur Mra bs Clguba, ol Gvz Crgref\n\n",
    "Ornhgvshy vf orggre guna htyl.\n",
    "Rkcyvpvg vf orggre guna vzcyvpvg.\n",
    "Fvzcyr vf orggre guna pbzcyrk.\n",
    "Pbzcyrk vf orggre guna pbzcyvpngrq.\n",
    "Syng vf orggre guna arfgrq.\n",
    "Fcnefr vf orggre guna qrafr.\n",
    "Ernqnovyvgl pbhagf.\n",
    "Fcrpvny pnfrf nera'g fcrpvny rabhtu gb oernx gur ehyrf.\n",
    "Nygubhtu cenpgvpnyvgl orngf chevgl.\n",
    "Reebef fubhyq arire cnff fvyragyl.\n",
    "Hayrff rkcyvpvgyl fvyraprq.\n",
    "Va gur snpr bs nzovthvgl, ershfr gur grzcgngvba gb thrff.\n",
    "Gurer fubhyq or bar-- naq cersrenoyl bayl bar --boivbhf jnl gb qb vg.\n",
    "Nygubhtu gung jnl znl abg or boivbhf ng svefg hayrff lbh'er Qhgpu.\n",
    "Abj vf orggre guna arire.\n",
    "Nygubhtu arire vf bsgra orggre guna *evtug* abj.\n",
    "Vs gur vzcyrzragngvba vf uneq gb rkcynva, vg'f n onq vqrn.\n",
    "Vs gur vzcyrzragngvba vf rnfl gb rkcynva, vg znl or n tbbq vqrn.\n",
    "Anzrfcnprf ner bar ubaxvat terng vqrn -- yrg'f qb zber bs gubfr!",
);

#[inline]
fn this_rot13_char(ch: char) -> char {
    match ch {
        'A'..='Z' => {
            let base = b'A';
            let idx = ch as u8 - base;
            (base + ((idx + 13) % 26)) as char
        }
        'a'..='z' => {
            let base = b'a';
            let idx = ch as u8 - base;
            (base + ((idx + 13) % 26)) as char
        }
        _ => ch,
    }
}

fn this_build_rot13_text() -> String {
    THIS_ENCODED.chars().map(this_rot13_char).collect()
}

const QUOPRI_ESCAPE: u8 = b'=';
const QUOPRI_MAX_LINE_SIZE: usize = 76;
const QUOPRI_HEX: &[u8; 16] = b"0123456789ABCDEF";
const OPCODE_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/opcode_payload_312.json");
const OPCODE_METADATA_PAYLOAD_314_JSON: &str =
    include_str!("../intrinsics/data/opcode_metadata_payload_314.json");
const TOKEN_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/token_payload_312.json");

#[inline]
fn quopri_needs_quoting(byte: u8, quotetabs: bool, header: bool) -> bool {
    if matches!(byte, b' ' | b'\t') {
        return quotetabs;
    }
    if byte == b'_' {
        return header;
    }
    byte == QUOPRI_ESCAPE || !(b' '..=b'~').contains(&byte)
}

#[inline]
fn quopri_quote_byte(byte: u8, out: &mut Vec<u8>) {
    out.push(QUOPRI_ESCAPE);
    out.push(QUOPRI_HEX[(byte >> 4) as usize]);
    out.push(QUOPRI_HEX[(byte & 0x0F) as usize]);
}

#[inline]
fn quopri_write_chunk(chunk: &[u8], line_end: &[u8], out: &mut Vec<u8>) {
    if let Some(last) = chunk.last()
        && matches!(*last, b' ' | b'\t')
    {
        out.extend_from_slice(&chunk[..chunk.len() - 1]);
        quopri_quote_byte(*last, out);
        out.extend_from_slice(line_end);
        return;
    }
    if chunk == b"." {
        quopri_quote_byte(b'.', out);
        out.extend_from_slice(line_end);
        return;
    }
    out.extend_from_slice(chunk);
    out.extend_from_slice(line_end);
}

fn quopri_encode_impl(data: &[u8], quotetabs: bool, header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + (data.len() / 20));
    let mut idx = 0usize;
    while idx < data.len() {
        let start = idx;
        while idx < data.len() && data[idx] != b'\n' {
            idx += 1;
        }
        let line = &data[start..idx];
        let line_end: &[u8] = if idx < data.len() && data[idx] == b'\n' {
            idx += 1;
            b"\n"
        } else {
            b""
        };

        let mut encoded = Vec::with_capacity(line.len() * 3);
        for byte in line {
            if quopri_needs_quoting(*byte, quotetabs, header) {
                quopri_quote_byte(*byte, &mut encoded);
            } else if header && *byte == b' ' {
                encoded.push(b'_');
            } else {
                encoded.push(*byte);
            }
        }

        let mut cursor = 0usize;
        while encoded.len().saturating_sub(cursor) > QUOPRI_MAX_LINE_SIZE {
            let end = cursor + QUOPRI_MAX_LINE_SIZE - 1;
            quopri_write_chunk(&encoded[cursor..end], b"=\n", &mut out);
            cursor = end;
        }
        quopri_write_chunk(&encoded[cursor..], line_end, &mut out);
    }
    out
}

#[inline]
fn quopri_is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

#[inline]
fn quopri_unhex_pair(hi: u8, lo: u8) -> Option<u8> {
    let hi = match hi {
        b'0'..=b'9' => hi - b'0',
        b'a'..=b'f' => hi - b'a' + 10,
        b'A'..=b'F' => hi - b'A' + 10,
        _ => return None,
    };
    let lo = match lo {
        b'0'..=b'9' => lo - b'0',
        b'a'..=b'f' => lo - b'a' + 10,
        b'A'..=b'F' => lo - b'A' + 10,
        _ => return None,
    };
    Some((hi << 4) | lo)
}

fn quopri_decode_impl(data: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut new = Vec::with_capacity(64);
    let mut idx = 0usize;
    while idx < data.len() {
        let start = idx;
        while idx < data.len() && data[idx] != b'\n' {
            idx += 1;
        }
        let mut n = idx - start;
        let line = &data[start..idx];
        let mut partial = true;
        if idx < data.len() && data[idx] == b'\n' {
            idx += 1;
            partial = false;
            while n > 0 && matches!(line[n - 1], b' ' | b'\t' | b'\r') {
                n -= 1;
            }
        }

        let mut i = 0usize;
        while i < n {
            let c = line[i];
            if c == b'_' && header {
                new.push(b' ');
                i += 1;
            } else if c != QUOPRI_ESCAPE {
                new.push(c);
                i += 1;
            } else if i + 1 == n && !partial {
                partial = true;
                break;
            } else if i + 1 < n && line[i + 1] == QUOPRI_ESCAPE {
                new.push(QUOPRI_ESCAPE);
                i += 2;
            } else if i + 2 < n && quopri_is_hex(line[i + 1]) && quopri_is_hex(line[i + 2]) {
                if let Some(decoded) = quopri_unhex_pair(line[i + 1], line[i + 2]) {
                    new.push(decoded);
                    i += 3;
                } else {
                    new.push(c);
                    i += 1;
                }
            } else {
                new.push(c);
                i += 1;
            }
        }
        if !partial {
            out.extend_from_slice(new.as_slice());
            out.push(b'\n');
            new.clear();
        }
    }
    if !new.is_empty() {
        out.extend_from_slice(new.as_slice());
    }
    out
}

fn quopri_expect_bytes_like(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("quopri {arg_name} expects bytes-like");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
        let msg = format!("quopri {arg_name} expects bytes-like");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    Ok(raw.to_vec())
}

fn quopri_expect_single_byte(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<u8, u64> {
    let bytes = quopri_expect_bytes_like(_py, bits, arg_name)?;
    if bytes.len() != 1 {
        let msg = format!("quopri {arg_name} expects single-byte bytes");
        return Err(raise_exception::<u64>(_py, "ValueError", &msg));
    }
    Ok(bytes[0])
}

#[inline]
fn email_quopri_header_safe(byte: u8) -> bool {
    matches!(byte, b'-' | b'!' | b'*' | b'+' | b'/')
        || byte.is_ascii_alphabetic()
        || byte.is_ascii_digit()
}

#[inline]
fn email_quopri_body_safe(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
            | b',' | b'-' | b'.' | b'/' | b'0'..=b'9' | b':' | b';' | b'<' | b'>' | b'?'
            | b'@' | b'A'..=b'Z' | b'[' | b'\\' | b']' | b'^' | b'_' | b'`' | b'a'..=b'z'
            | b'{' | b'|' | b'}' | b'~' | b'\t'
    )
}

#[inline]
fn email_quopri_push_escape(byte: u8, out: &mut String) {
    out.push('=');
    out.push(QUOPRI_HEX[(byte >> 4) as usize] as char);
    out.push(QUOPRI_HEX[(byte & 0x0F) as usize] as char);
}

#[inline]
fn email_quopri_push_header_mapped(byte: u8, out: &mut String) {
    if email_quopri_header_safe(byte) {
        out.push(byte as char);
    } else if byte == b' ' {
        out.push('_');
    } else {
        email_quopri_push_escape(byte, out);
    }
}

#[inline]
fn email_quopri_push_body_mapped(byte: u8, out: &mut String) {
    if email_quopri_body_safe(byte) {
        out.push(byte as char);
    } else {
        email_quopri_push_escape(byte, out);
    }
}

fn email_quopri_expect_int_octet(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<u8, u64> {
    let value = match to_i64(obj_from_bits(bits)) {
        Some(value) => value,
        None => {
            let msg = format!("email.quoprimime {arg_name} expects int");
            return Err(raise_exception::<u64>(_py, "TypeError", &msg));
        }
    };
    if !(0..=255).contains(&value) {
        let msg = format!("email.quoprimime {arg_name} out of range");
        return Err(raise_exception::<u64>(_py, "ValueError", &msg));
    }
    Ok(value as u8)
}

fn email_quopri_expect_string(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(value),
        None => {
            let msg = format!("email.quoprimime {arg_name} expects str");
            Err(raise_exception::<u64>(_py, "TypeError", &msg))
        }
    }
}

fn email_quopri_alloc_str(_py: &crate::PyToken<'_>, value: &str) -> u64 {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn email_quopri_splitlines(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\n' => {
                lines.push(value[start..idx].to_string());
                idx += 1;
                start = idx;
            }
            b'\r' => {
                lines.push(value[start..idx].to_string());
                idx += 1;
                if idx < bytes.len() && bytes[idx] == b'\n' {
                    idx += 1;
                }
                start = idx;
            }
            _ => idx += 1,
        }
    }

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
        let tuple_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), slot_state_bits]);
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

fn pickle_dump_obj_binary(
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

fn pickle_apply_dict_state(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    dict_state_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(dict_state_bits).is_none() {
        return Ok(());
    }
    let Some(state_ptr) = obj_from_bits(dict_state_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    };
    if unsafe { object_type_id(state_ptr) } != TYPE_ID_DICT {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    }

    // Use setattr for each state entry. This correctly routes values to typed
    // field slots (TYPE_ID_OBJECT), dataclass descriptor fields
    // (TYPE_ID_DATACLASS), or __dict__ for fully dynamic instances.
    let pairs = unsafe { crate::dict_order(state_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let key_bits = pairs[idx];
        let value_bits = pairs[idx + 1];
        idx += 2;
        let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(())
}

fn pickle_vm_item_to_bits(_py: &crate::PyToken<'_>, item: &PickleVmItem) -> Result<u64, u64> {
    match item {
        PickleVmItem::Value(bits) => Ok(*bits),
        PickleVmItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleVmItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_vm_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<Vec<PickleVmItem>, u64> {
    let mut out: Vec<PickleVmItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleVmItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_vm_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<u64, u64> {
    let item = stack
        .pop()
        .ok_or_else(|| pickle_raise(_py, "pickle.loads: stack underflow"))?;
    pickle_vm_item_to_bits(_py, &item)
}

fn pickle_decode_8bit_string(
    _py: &crate::PyToken<'_>,
    raw: &[u8],
    encoding: &str,
    _errors: &str,
) -> Result<u64, u64> {
    if encoding.eq_ignore_ascii_case("bytes") {
        let ptr = crate::alloc_bytes(_py, raw);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(ptr).bits());
    }
    let decoded = if encoding.eq_ignore_ascii_case("latin1")
        || encoding.eq_ignore_ascii_case("latin-1")
    {
        raw.iter().map(|&b| char::from(b)).collect::<String>()
    } else {
        String::from_utf8(raw.to_vec())
            .map_err(|_| pickle_raise(_py, "pickle.loads: unable to decode 8-bit string payload"))?
    };
    let ptr = alloc_string(_py, decoded.as_bytes());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn pickle_resolve_global_bits(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<u64, u64> {
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        return Err(MoltObject::none().bits());
    };
    let imported_bits = crate::molt_module_import(module_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    };
    let value_bits = crate::molt_object_getattribute(imported_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits)
}

fn pickle_resolve_global_with_hook(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    if let Some(callback_bits) = find_class_bits {
        let Some(module_bits) = alloc_string_bits(_py, module) else {
            return Err(MoltObject::none().bits());
        };
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, module_bits);
            return Err(MoltObject::none().bits());
        };
        let out_bits = unsafe { call_callable2(_py, callback_bits, module_bits, name_bits) };
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(out_bits);
    }
    pickle_resolve_global_bits(_py, module, name)
}

fn pickle_lookup_extension_bits(
    _py: &crate::PyToken<'_>,
    code: i64,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    let copyreg_bits = pickle_resolve_global_bits(_py, "copyreg", "_inverted_registry")?;
    let Some(dict_ptr) = obj_from_bits(copyreg_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    }
    let code_bits = MoltObject::from_int(code).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, dict_ptr, code_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, copyreg_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(entry_bits) = entry_bits else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: unknown extension code"));
    };
    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    if unsafe { object_type_id(entry_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let fields = unsafe { seq_vec_ref(entry_ptr) };
    if fields.len() != 2 {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let Some(module) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    let Some(name) = string_obj_to_owned(obj_from_bits(fields[1])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    dec_ref_bits(_py, copyreg_bits);
    pickle_resolve_global_with_hook(_py, &module, &name, find_class_bits)
}

fn pickle_apply_newobj(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    args_bits: u64,
    kwargs_bits: Option<u64>,
) -> Result<u64, u64> {
    let new_bits = pickle_attr_required(_py, cls_bits, b"__new__")?;
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let kw_len = if let Some(kw_bits) = kwargs_bits {
        let Some(kw_ptr) = obj_from_bits(kw_bits).as_ptr() else {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        };
        if unsafe { object_type_id(kw_ptr) } != TYPE_ID_DICT {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        }
        unsafe { crate::dict_order(kw_ptr).len() / 2 }
    } else {
        0
    };
    let builder_bits = crate::molt_callargs_new((args.len() + 1) as u64, kw_len as u64);
    let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, cls_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, new_bits);
        return Err(MoltObject::none().bits());
    }
    for arg in args {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return Err(MoltObject::none().bits());
        }
    }
    if let Some(kw_bits) = kwargs_bits {
        let kw_ptr = obj_from_bits(kw_bits).as_ptr().expect("checked above");
        let pairs = unsafe { crate::dict_order(kw_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let val_bits = pairs[idx + 1];
            let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, key_bits, val_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    let out_bits = crate::molt_call_bind(new_bits, builder_bits);
    dec_ref_bits(_py, new_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    // Initialize typed field slots to the missing sentinel so that
    // uninitialized fields (from __new__ without __init__) are properly
    // recognized as absent by hasattr/getattr.
    pickle_init_missing_fields(_py, out_bits);
    Ok(out_bits)
}

/// Initialize all typed field slots (and dataclass field values) to the missing
/// sentinel. Called after NEWOBJ to ensure fields not populated by BUILD are
/// correctly absent.
fn pickle_init_missing_fields(_py: &crate::PyToken<'_>, inst_bits: u64) {
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return;
    };
    let type_id = unsafe { object_type_id(inst_ptr) };
    let missing = missing_bits(_py);

    if type_id == crate::TYPE_ID_OBJECT {
        // Initialize typed field offsets to missing.
        let class_bits = unsafe { object_class_bits(inst_ptr) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
            return;
        }
        let cd_bits = unsafe { crate::class_dict_bits(class_ptr) };
        let Some(cd_ptr) = obj_from_bits(cd_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(cd_ptr) } != TYPE_ID_DICT {
            return;
        }
        let Some(offsets_name) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
            return;
        };
        let offsets_bits = unsafe { crate::dict_get_in_place(_py, cd_ptr, offsets_name) };
        dec_ref_bits(_py, offsets_name);
        if exception_pending(_py) {
            clear_exception(_py);
            return;
        }
        let Some(offsets_bits) = offsets_bits else {
            return;
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
            return;
        }
        let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let offset_bits = pairs[idx + 1];
            idx += 2;
            if let Some(offset) = to_i64(obj_from_bits(offset_bits)).filter(|&v| v >= 0) {
                unsafe {
                    let slot = inst_ptr.add(offset as usize) as *mut u64;
                    let old = *slot;
                    if old != missing {
                        inc_ref_bits(_py, missing);
                        if obj_from_bits(old).as_ptr().is_some() {
                            dec_ref_bits(_py, old);
                        }
                        *slot = missing;
                    }
                }
            }
        }
    } else if type_id == crate::TYPE_ID_DATACLASS {
        // Initialize dataclass field values to missing.
        let desc_ptr = unsafe { crate::dataclass_desc_ptr(inst_ptr) };
        if desc_ptr.is_null() {
            return;
        }
        let fields = unsafe { crate::dataclass_fields_mut(inst_ptr) };
        for val in fields.iter_mut() {
            if *val != missing {
                inc_ref_bits(_py, missing);
                if obj_from_bits(*val).as_ptr().is_some() {
                    dec_ref_bits(_py, *val);
                }
                *val = missing;
            }
        }
    }
}

fn pickle_apply_build(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    state_bits: u64,
) -> Result<u64, u64> {
    if obj_from_bits(state_bits).is_none() {
        return Ok(inst_bits);
    }
    if let Some(setstate_bits) = pickle_attr_optional(_py, inst_bits, b"__setstate__")? {
        let _ = unsafe { call_callable1(_py, setstate_bits, state_bits) };
        dec_ref_bits(_py, setstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(inst_bits);
    }
    let mut dict_state_bits = state_bits;
    let mut slot_state_bits: Option<u64> = None;
    if let Some(state_ptr) = obj_from_bits(state_bits).as_ptr()
        && unsafe { object_type_id(state_ptr) } == TYPE_ID_TUPLE
    {
        let fields = unsafe { seq_vec_ref(state_ptr) };
        if fields.len() == 2 {
            dict_state_bits = fields[0];
            slot_state_bits = Some(fields[1]);
        }
    }
    pickle_apply_dict_state(_py, inst_bits, dict_state_bits)?;
    if let Some(slot_bits) = slot_state_bits
        && !obj_from_bits(slot_bits).is_none()
    {
        let Some(slot_ptr) = obj_from_bits(slot_bits).as_ptr() else {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        };
        if unsafe { object_type_id(slot_ptr) } != TYPE_ID_DICT {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        }
        let pairs = unsafe { crate::dict_order(slot_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let value_bits = pairs[idx + 1];
            let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    Ok(inst_bits)
}

fn pickle_apply_reduce_vm(
    _py: &crate::PyToken<'_>,
    callable: PickleVmItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = match callable {
        PickleVmItem::Mark => {
            return Err(pickle_raise(_py, "pickle.loads: mark cannot be called"));
        }
        PickleVmItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 1 {
                "utf-8".to_string()
            } else {
                let Some(enc) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                enc
            };
            pickle_encode_text(_py, &text, &encoding)?
        }
        PickleVmItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
        PickleVmItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
    };
    Ok(out_bits)
}

fn pickle_apply_reduce_bits(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_memo_set(
    _py: &crate::PyToken<'_>,
    memo: &mut Vec<Option<PickleVmItem>>,
    index: usize,
    item: PickleVmItem,
) {
    if memo.len() <= index {
        memo.resize(index + 1, None);
    }
    memo[index] = Some(item);
}

fn pickle_memo_get(
    _py: &crate::PyToken<'_>,
    memo: &[Option<PickleVmItem>],
    index: usize,
) -> Result<PickleVmItem, u64> {
    if let Some(Some(item)) = memo.get(index) {
        return Ok(item.clone());
    }
    let msg = format!("pickle.loads: memo key {} missing", index);
    Err(pickle_raise(_py, &msg))
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

