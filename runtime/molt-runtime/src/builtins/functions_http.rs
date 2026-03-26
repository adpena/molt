use molt_obj_model::MoltObject;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::builtins::platform::env_state_get;
use crate::{
    TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_TUPLE,
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bytes_like_slice,
    call_callable0, call_callable1, call_callable2,
    call_class_init_with_args, clear_exception, dec_ref_bits,
    exception_kind_bits, exception_pending, format_obj,
    inc_ref_bits, is_truthy, maybe_ptr_from_bits, missing_bits,
    molt_exception_last, molt_getattr_builtin, molt_is_callable, molt_iter,
    molt_list_insert, molt_module_import, molt_object_setattr,
    obj_from_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64,
};

struct MoltUrllibResponse {
    body: Vec<u8>,
    pos: usize,
    closed: bool,
    url: String,
    code: i64,
    reason: String,
    headers: Vec<(String, String)>,
    header_joined: HashMap<String, String>,
    headers_dict_cache: Option<u64>,
    headers_list_cache: Option<u64>,
}

struct UrllibHttpRequest {
    host: String,
    port: u16,
    path: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Option<f64>,
}

#[derive(Clone)]
struct MoltHttpClientConnection {
    host: String,
    port: u16,
    timeout: Option<f64>,
    method: Option<String>,
    url: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    buffer: Vec<Vec<u8>>,
    skip_host: bool,
    skip_accept_encoding: bool,
}

struct MoltHttpClientConnectionRuntime {
    next_handle: u64,
    connections: HashMap<u64, MoltHttpClientConnection>,
}

#[derive(Clone, Default)]
struct MoltHttpMessage {
    headers: Vec<(String, String)>,
    index: HashMap<String, Vec<usize>>,
    items_list_cache: Option<u64>,
}

struct MoltHttpMessageRuntime {
    next_handle: u64,
    messages: HashMap<u64, MoltHttpMessage>,
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

static URLLIB_RESPONSE_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltUrllibResponse>>> =
    OnceLock::new();
static URLLIB_RESPONSE_NEXT: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT_CONNECTION_RUNTIME: OnceLock<Mutex<MoltHttpClientConnectionRuntime>> =
    OnceLock::new();
static HTTP_MESSAGE_RUNTIME: OnceLock<Mutex<MoltHttpMessageRuntime>> = OnceLock::new();
static COOKIEJAR_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltCookieJar>>> = OnceLock::new();
static COOKIEJAR_NEXT: AtomicU64 = AtomicU64::new(1);

static SOCKETSERVER_RUNTIME: OnceLock<Mutex<MoltSocketServerRuntime>> = OnceLock::new();

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

const PICKLE_PROTO_3: i64 = 3;
const PICKLE_PROTO_4: i64 = 4;
const PICKLE_PROTO_5: i64 = 5;

const PICKLE_OP_PROTO: u8 = 0x80;
const PICKLE_OP_STOP: u8 = b'.';
const PICKLE_OP_POP: u8 = b'0';
const PICKLE_OP_POP_MARK: u8 = b'1';
const PICKLE_OP_MARK: u8 = b'(';
const PICKLE_OP_NONE: u8 = b'N';
const PICKLE_OP_NEWTRUE: u8 = 0x88;
const PICKLE_OP_NEWFALSE: u8 = 0x89;
const PICKLE_OP_INT: u8 = b'I';
const PICKLE_OP_LONG: u8 = b'L';
const PICKLE_OP_BININT: u8 = b'J';
const PICKLE_OP_BININT1: u8 = b'K';
const PICKLE_OP_BININT2: u8 = b'M';
const PICKLE_OP_LONG1: u8 = 0x8a;
const PICKLE_OP_LONG4: u8 = 0x8b;
const PICKLE_OP_FLOAT: u8 = b'F';
const PICKLE_OP_BINFLOAT: u8 = b'G';
const PICKLE_OP_STRING: u8 = b'S';
const PICKLE_OP_BINUNICODE: u8 = b'X';
const PICKLE_OP_SHORT_BINUNICODE: u8 = 0x8c;
const PICKLE_OP_UNICODE: u8 = b'V';
const PICKLE_OP_BINBYTES: u8 = b'B';
const PICKLE_OP_SHORT_BINBYTES: u8 = b'C';
const PICKLE_OP_BINBYTES8: u8 = 0x8e;
const PICKLE_OP_BYTEARRAY8: u8 = 0x96;
const PICKLE_OP_EMPTY_TUPLE: u8 = b')';
const PICKLE_OP_TUPLE: u8 = b't';
const PICKLE_OP_TUPLE1: u8 = 0x85;
const PICKLE_OP_TUPLE2: u8 = 0x86;
const PICKLE_OP_TUPLE3: u8 = 0x87;
const PICKLE_OP_EMPTY_LIST: u8 = b']';
const PICKLE_OP_LIST: u8 = b'l';
const PICKLE_OP_APPEND: u8 = b'a';
const PICKLE_OP_APPENDS: u8 = b'e';
const PICKLE_OP_EMPTY_DICT: u8 = b'}';
const PICKLE_OP_DICT: u8 = b'd';
const PICKLE_OP_SETITEM: u8 = b's';
const PICKLE_OP_SETITEMS: u8 = b'u';
const PICKLE_OP_EMPTY_SET: u8 = 0x8f;
const PICKLE_OP_ADDITEMS: u8 = 0x90;
const PICKLE_OP_FROZENSET: u8 = 0x91;
const PICKLE_OP_GLOBAL: u8 = b'c';
const PICKLE_OP_STACK_GLOBAL: u8 = 0x93;
const PICKLE_OP_REDUCE: u8 = b'R';
const PICKLE_OP_BUILD: u8 = b'b';
const PICKLE_OP_NEWOBJ: u8 = 0x81;
const PICKLE_OP_NEWOBJ_EX: u8 = 0x92;
const PICKLE_OP_PUT: u8 = b'p';
const PICKLE_OP_BINPUT: u8 = b'q';
const PICKLE_OP_LONG_BINPUT: u8 = b'r';
const PICKLE_OP_GET: u8 = b'g';
const PICKLE_OP_BINGET: u8 = b'h';
const PICKLE_OP_LONG_BINGET: u8 = b'j';
const PICKLE_OP_MEMOIZE: u8 = 0x94;
const PICKLE_OP_PERSID: u8 = b'P';
const PICKLE_OP_BINPERSID: u8 = b'Q';
const PICKLE_OP_EXT1: u8 = 0x82;
const PICKLE_OP_EXT2: u8 = 0x83;
const PICKLE_OP_EXT4: u8 = 0x84;
const PICKLE_OP_FRAME: u8 = 0x95;
const PICKLE_OP_NEXT_BUFFER: u8 = 0x97;
const PICKLE_OP_READONLY_BUFFER: u8 = 0x98;

const PICKLE_RECURSION_LIMIT: usize = 1_000;

#[derive(Clone, Debug)]
enum PickleVmItem {
    Value(u64),
    Global(PickleGlobal),
    Mark,
}

struct PickleDumpState {
    protocol: i64,
    out: Vec<u8>,
    memo: HashMap<u64, u32>,
    next_memo: u32,
    depth: usize,
    persistent_id_bits: Option<u64>,
    buffer_callback_bits: Option<u64>,
    dispatch_table_bits: Option<u64>,
}

impl PickleDumpState {
    fn new(
        protocol: i64,
        persistent_id_bits: Option<u64>,
        buffer_callback_bits: Option<u64>,
        dispatch_table_bits: Option<u64>,
    ) -> Self {
        Self {
            protocol,
            out: Vec::with_capacity(256),
            memo: HashMap::new(),
            next_memo: 0,
            depth: 0,
            persistent_id_bits,
            buffer_callback_bits,
            dispatch_table_bits,
        }
    }

    fn push(&mut self, op: u8) {
        self.out.push(op);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
}

fn pickle_option_callable_bits(
    _py: &crate::PyToken<'_>,
    maybe_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(maybe_bits).is_none() {
        return Ok(None);
    }
    if !is_truthy(_py, obj_from_bits(molt_is_callable(maybe_bits))) {
        let message = format!("pickle {name} must be callable");
        return Err(raise_exception::<u64>(_py, "TypeError", &message));
    }
    Ok(Some(maybe_bits))
}

fn pickle_input_to_bytes(_py: &crate::PyToken<'_>, data_bits: u64) -> Result<Vec<u8>, u64> {
    if let Some(ptr) = obj_from_bits(data_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        return Ok(raw.to_vec());
    }
    if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
        return Ok(text.into_bytes());
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "pickle data must be bytes, bytearray, or str",
    ))
}

fn pickle_read_u8(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u8, u64> {
    if *idx >= data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let byte = data[*idx];
    *idx += 1;
    Ok(byte)
}

fn pickle_read_exact<'a>(
    data: &'a [u8],
    idx: &mut usize,
    n: usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if data.len().saturating_sub(*idx) < n {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let end = start + n;
    *idx = end;
    Ok(&data[start..end])
}

fn pickle_read_line_bytes<'a>(
    data: &'a [u8],
    idx: &mut usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if *idx > data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = data[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&data[start..end])
}

fn pickle_read_u16_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u16, u64> {
    let raw = pickle_read_exact(data, idx, 2, _py)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn pickle_read_u32_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u32, u64> {
    let raw = pickle_read_exact(data, idx, 4, _py)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn pickle_read_u64_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn pickle_parse_long_bytes_bits(_py: &crate::PyToken<'_>, raw: &[u8]) -> Result<u64, u64> {
    if raw.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if raw.len() > 8 {
        return Err(pickle_raise(
            _py,
            "pickle.loads: LONG payload exceeds Molt int range",
        ));
    }
    let negative = (raw[raw.len() - 1] & 0x80) != 0;
    let mut bytes = if negative { [0xff; 8] } else { [0u8; 8] };
    bytes[..raw.len()].copy_from_slice(raw);
    Ok(MoltObject::from_int(i64::from_le_bytes(bytes)).bits())
}

fn pickle_read_f64_be(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<f64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(f64::from_bits(u64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ])))
}

fn pickle_decode_utf8(_py: &crate::PyToken<'_>, raw: &[u8], ctx: &str) -> Result<String, u64> {

    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINPUT);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINPUT);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_emit_memo_get(state: &mut PickleDumpState, index: u32) {
    if index <= u8::MAX as u32 {

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

fn pickle_buffer_value_to_memoryview(
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

fn pickle_next_external_buffer_bits(
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
                            Ok(v) => v as usize,
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
                        Ok(v) => v as usize,
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
                            crate::set_add_in_place(_py, set_ptr, bits);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

fn shlex_is_safe(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(


        let placeholder_len = textwrap_char_len(&options.placeholder);
        loop {
            let can_append_placeholder = cur_line
                .last()
                .map(|last| {
                    !textwrap_chunk_is_whitespace(last) && cur_len + placeholder_len <= width
                })
                .unwrap_or(false);
            if can_append_placeholder {
                cur_line.push(options.placeholder.clone());
                lines.push(format!("{indent}{}", cur_line.concat()));
                break;
            }
            if let Some(last) = cur_line.pop() {
                cur_len -= textwrap_char_len(&last);
                continue;
            }
            if let Some(prev_line) = lines.last_mut() {
                let trimmed = prev_line.trim_end_matches(char::is_whitespace).to_string();
                if textwrap_char_len(&trimmed) + placeholder_len <= options.width {
                    *prev_line = trimmed + &options.placeholder;
                    return Ok(lines);
                }
            }
            let placeholder_lstrip = options.placeholder.trim_start_matches(char::is_whitespace);
            lines.push(format!("{indent}{placeholder_lstrip}"));
            break;
        }
        break;
    }

    Ok(lines)
}

fn textwrap_wrap_impl(text: &str, options: &TextWrapOptions) -> Result<Vec<String>, String> {
    let munged = textwrap_munge_whitespace(text, options);
    let mut chunks = textwrap_split_chunks(&munged, options.break_on_hyphens);
    if options.fix_sentence_endings {
        textwrap_fix_sentence_endings(&mut chunks);
    }
    textwrap_wrap_chunks(chunks, options)
}

fn textwrap_line_is_space(line: &str) -> bool {
    !line.is_empty() && line.chars().all(char::is_whitespace)
}

fn textwrap_splitlines_keepends(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut line_start = 0usize;
    let mut iter = text.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        let mut end = idx + ch.len_utf8();
        let is_break = match ch {
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => true,
            '\r' => {
                if let Some((next_idx, next_ch)) = iter.peek().copied()
                    && next_ch == '\n'
                {
                    end = next_idx + next_ch.len_utf8();
                    iter.next();
                }
                true
            }
            _ => false,
        };
        if is_break {
            out.push(text[line_start..end].to_string());
            line_start = end;
        }
    }
    if line_start < text.len() {
        out.push(text[line_start..].to_string());
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn textwrap_parse_options_ex(
    _py: &crate::PyToken<'_>,
    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> Result<TextWrapOptions, u64> {
    let Some(width) = to_i64(obj_from_bits(width_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "width must be int",
        ));
    };
    let Some(initial_indent) = string_obj_to_owned(obj_from_bits(initial_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "initial_indent must be str",
        ));
    };
    let Some(subsequent_indent) = string_obj_to_owned(obj_from_bits(subsequent_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "subsequent_indent must be str",
        ));
    };
    let Some(tabsize) = to_i64(obj_from_bits(tabsize_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "tabsize must be int",
        ));
    };
    let Some(max_lines_placeholder_ptr) = obj_from_bits(max_lines_placeholder_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    };
    if unsafe { object_type_id(max_lines_placeholder_ptr) } != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_placeholder = unsafe { seq_vec_ref(max_lines_placeholder_ptr) };
    if max_lines_placeholder.len() != 2 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_bits = max_lines_placeholder[0];
    let placeholder_bits = max_lines_placeholder[1];

    let max_lines = if obj_from_bits(max_lines_bits).is_none() {
        None
    } else {
        let Some(value) = to_i64(obj_from_bits(max_lines_bits)) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "max_lines must be int or None",
            ));
        };
        Some(value)
    };
    let Some(placeholder) = string_obj_to_owned(obj_from_bits(placeholder_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "placeholder must be str",
        ));
    };
    Ok(TextWrapOptions {
        width,
        initial_indent,
        subsequent_indent,
        expand_tabs: is_truthy(_py, obj_from_bits(expand_tabs_bits)),
        replace_whitespace: is_truthy(_py, obj_from_bits(replace_whitespace_bits)),
        fix_sentence_endings: is_truthy(_py, obj_from_bits(fix_sentence_endings_bits)),
        break_long_words: is_truthy(_py, obj_from_bits(break_long_words_bits)),
        drop_whitespace: is_truthy(_py, obj_from_bits(drop_whitespace_bits)),
        break_on_hyphens: is_truthy(_py, obj_from_bits(break_on_hyphens_bits)),
        tabsize,
        max_lines,
        placeholder,
    })
}

fn textwrap_indent_with_predicate(
    _py: &crate::PyToken<'_>,
    text: &str,
    prefix: &str,
    predicate_bits: Option<u64>,
) -> u64 {
    let mut out = String::with_capacity(text.len().saturating_add(prefix.len() * 4));
    for line in textwrap_splitlines_keepends(text) {
        let should_prefix = if let Some(predicate) = predicate_bits {
            let Some(line_bits) = alloc_string_bits(_py, &line) else {
                return MoltObject::none().bits();
            };
            let result_bits = unsafe { call_callable1(_py, predicate, line_bits) };
            dec_ref_bits(_py, line_bits);
            if exception_pending(_py) {
                if !obj_from_bits(result_bits).is_none() {
                    dec_ref_bits(_py, result_bits);
                }
                return MoltObject::none().bits();
            }
            let truthy = is_truthy(_py, obj_from_bits(result_bits));
            if !obj_from_bits(result_bits).is_none() {
                dec_ref_bits(_py, result_bits);
            }
            truthy
        } else {
            !textwrap_line_is_space(&line)
        };
        if should_prefix {
            out.push_str(prefix);
        }
        out.push_str(&line);
    }
    let out_ptr = alloc_string(_py, out.as_bytes());
    if out_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(out_ptr).bits()
    }
}

// ─── textwrap.dedent ────────────────────────────────────────────────────────

fn textwrap_dedent_impl(text: &str) -> String {
    // CPython textwrap.dedent: remove common leading whitespace from all lines.
    let mut margin: Option<&str> = None;
    let lines: Vec<&str> = text.split('\n').collect();
    for &line in &lines {
        let stripped = line.trim_start();
        if stripped.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - stripped.len()];
        if let Some(m) = margin {
            // Find common prefix between margin and indent
            let common_len = m
                .chars()
                .zip(indent.chars())
                .take_while(|(a, b)| a == b)
                .count();
            // Need byte length of common prefix
            let byte_len = m
                .char_indices()
                .nth(common_len)
                .map(|(i, _)| i)
                .unwrap_or(m.len());
            margin = Some(&m[..byte_len]);
        } else {
            margin = Some(indent);
        }
    }
    let margin = margin.unwrap_or("");
    if margin.is_empty() {
        return text.to_string();
    }
    let margin_len = margin.len();
    let mut result = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.trim_start().is_empty() {
            // Whitespace-only line: strip all leading whitespace
            result.push_str(line.trim_start());
        } else if line.len() >= margin_len && &line[..margin_len] == margin {
            result.push_str(&line[margin_len..]);
        } else {
            result.push_str(line);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_dedent(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let result = textwrap_dedent_impl(&text);
        let out_ptr = alloc_string(_py, result.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_shorten(
    text_bits: u64,
    width_bits: u64,
    placeholder_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let placeholder = if obj_from_bits(placeholder_bits).is_none() {
            " [...]".to_string()
        } else {
            string_obj_to_owned(obj_from_bits(placeholder_bits))
                .unwrap_or_else(|| " [...]".to_string())
        };
        // Collapse whitespace and truncate
        let collapsed: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
        if (collapsed.len() as i64) <= width {
            let out_ptr = alloc_string(_py, collapsed.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let ph_len = placeholder.len() as i64;
        let max_text = width - ph_len;
        if max_text < 0 {
            let out_ptr = alloc_string(_py, placeholder.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        // Find last space before max_text
        let mut truncate_at = max_text as usize;
        if truncate_at < collapsed.len() {
            // Find last space at or before truncate_at
            if let Some(pos) = collapsed[..truncate_at].rfind(' ') {
                truncate_at = pos;
            }
        }
        let result = format!("{}{}", &collapsed[..truncate_at].trim_end(), placeholder);
        let out_ptr = alloc_string(_py, result.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ─── logging filter intrinsics ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_filter_check(filter_name_bits: u64, record_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let filter_name = string_obj_to_owned(obj_from_bits(filter_name_bits)).unwrap_or_default();
        let record_name = string_obj_to_owned(obj_from_bits(record_name_bits)).unwrap_or_default();
        let result = filter_name.is_empty()
            || record_name == filter_name
            || record_name.starts_with(&format!("{}.", filter_name));
        MoltObject::from_int(if result { 1 } else { 0 }).bits()
    })
}

// ─── logging file handler intrinsics ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_file_handler_emit(
    msg_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(msg) = string_obj_to_owned(obj_from_bits(msg_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "msg must be str");
        };
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "filename must be str");
        };
        let mode = string_obj_to_owned(obj_from_bits(mode_bits)).unwrap_or_else(|| "a".to_string());
        let _encoding = string_obj_to_owned(obj_from_bits(encoding_bits));

        use std::fs::OpenOptions;
        use std::io::Write;
        let open_result = if mode.contains('w') {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&filename)
        } else {
            OpenOptions::new().append(true).create(true).open(&filename)
        };
        match open_result {
            Ok(mut f) => {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.write_all(b"\n");
            }
            Err(e) => {
                return raise_exception::<_>(
                    _py,
                    "IOError",
                    &format!("cannot open {}: {}", filename, e),
                );
            }
        }
        MoltObject::none().bits()
    })
}

// ─── copy.replace intrinsic ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_replace(obj_bits: u64, changes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // copy.replace creates a modified shallow copy.
        // For Molt's supported types, apply changes dict on top of a shallow copy.
        let _ = changes_bits; // changes are applied Python-side
        crate::builtins::copy_mod::molt_copy_copy(obj_bits)
    })
}

// ─── pprint format/isreadable/isrecursive with context ──────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_format_object(
    obj_bits: u64,
    max_depth_bits: u64,
    level_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        use std::collections::HashSet;
        let max_depth = crate::builtins::pprint_ext::i64_from_bits_default(max_depth_bits, -1);
        let level = crate::builtins::pprint_ext::i64_from_bits_default(level_bits, 0);
        let mut seen = HashSet::new();
        let (repr, readable, recursive) = crate::builtins::pprint_ext::safe_repr_inner(
            _py, obj_bits, &mut seen, level, max_depth, -1,
        );
        // Return a tuple (repr_str, readable_bool, recursive_bool)
        let repr_ptr = alloc_string(_py, repr.as_bytes());
        if repr_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let repr_bits = MoltObject::from_ptr(repr_ptr).bits();
        let readable_bits = MoltObject::from_int(if readable { 1 } else { 0 }).bits();
        let recursive_bits = MoltObject::from_int(if recursive { 1 } else { 0 }).bits();
        let tup_ptr = crate::alloc_tuple(_py, &[repr_bits, readable_bits, recursive_bits]);
        if tup_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tup_ptr).bits()
    })
}

#[derive(Clone)]
struct PkgutilModuleInfo {
    module_finder: String,
    name: String,
    ispkg: bool,
}

fn pkgutil_join(base: &str, name: &str) -> String {
    if base.is_empty() {
        return name.to_string();
    }
    Path::new(base).join(name).to_string_lossy().into_owned()
}

fn pkgutil_iter_modules_in_path(path: &str, prefix: &str) -> Vec<PkgutilModuleInfo> {
    let entries = match fs::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(_) => return Vec::new(),
    };

    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut yielded: HashSet<String> = HashSet::new();
    let mut results: Vec<PkgutilModuleInfo> = Vec::new();
    for entry in names {
        if entry == "__pycache__" {
            continue;
        }
        let full = pkgutil_join(path, &entry);
        if !entry.contains('.') {
            if let Ok(dir_entries) = fs::read_dir(&full) {
                let mut ispkg = false;
                for item in dir_entries.flatten() {
                    if item.file_name().to_string_lossy() == "__init__.py" {
                        ispkg = true;
                        break;
                    }
                }
                if ispkg && yielded.insert(entry.clone()) {
                    results.push(PkgutilModuleInfo {
                        module_finder: path.to_string(),
                        name: format!("{prefix}{entry}"),
                        ispkg: true,
                    });
                }
            }
            continue;
        }
        if !entry.ends_with(".py") {
            continue;
        }
        let modname = &entry[..entry.len().saturating_sub(3)];
        if modname.is_empty() || modname == "__init__" || modname.contains('.') {
            continue;
        }
        if yielded.insert(modname.to_string()) {
            results.push(PkgutilModuleInfo {
                module_finder: path.to_string(),
                name: format!("{prefix}{modname}"),
                ispkg: false,
            });
        }
    }
    results
}

fn pkgutil_iter_modules_impl(paths: &[String], prefix: &str) -> Vec<PkgutilModuleInfo> {
    let mut yielded: HashSet<String> = HashSet::new();
    let mut out: Vec<PkgutilModuleInfo> = Vec::new();
    for path in paths {
        for info in pkgutil_iter_modules_in_path(path, prefix) {
            if yielded.insert(info.name.clone()) {
                out.push(info);
            }
        }
    }
    out
}

fn pkgutil_walk_packages_impl(paths: &[String], prefix: &str) -> Vec<PkgutilModuleInfo> {
    let mut out: Vec<PkgutilModuleInfo> = Vec::new();
    let infos = pkgutil_iter_modules_impl(paths, prefix);
    for info in infos {
        out.push(info.clone());
        if !info.ispkg {
            continue;
        }
        let mut pkg_name = info.name.clone();
        if !prefix.is_empty() && pkg_name.starts_with(prefix) {
            pkg_name = pkg_name[prefix.len()..].to_string();
        }
        let subdir = pkgutil_join(&info.module_finder, &pkg_name);
        let subprefix = format!("{}.", info.name);
        let nested = pkgutil_walk_packages_impl(&[subdir], &subprefix);
        out.extend(nested);
    }
    out
}

fn alloc_pkgutil_module_info_list(_py: &crate::PyToken<'_>, values: &[PkgutilModuleInfo]) -> u64 {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(values.len());
    for entry in values {
        let finder_ptr = alloc_string(_py, entry.module_finder.as_bytes());
        if finder_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, entry.name.as_bytes());
        if name_ptr.is_null() {
            let finder_bits = MoltObject::from_ptr(finder_ptr).bits();
            dec_ref_bits(_py, finder_bits);
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let finder_bits = MoltObject::from_ptr(finder_ptr).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let ispkg_bits = MoltObject::from_bool(entry.ispkg).bits();
        let tuple_ptr = alloc_tuple(_py, &[finder_bits, name_bits, ispkg_bits]);
        dec_ref_bits(_py, finder_bits);
        dec_ref_bits(_py, name_bits);
        if tuple_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, tuple_bits.as_slice(), tuple_bits.len());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn compileall_compile_file_impl(fullname: &str) -> bool {
    let mut handle = match fs::File::open(fullname) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    let mut one = [0u8; 1];
    handle.read(&mut one).is_ok()
}

fn compileall_compile_dir_impl(dir: &str, maxlevels: i64) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut success = true;
    for entry in names {
        if entry == "__pycache__" {
            continue;
        }
        let full = pkgutil_join(dir, &entry);
        if entry.ends_with(".py") {
            if !compileall_compile_file_impl(&full) {
                success = false;
            }
            continue;
        }
        if maxlevels <= 0 {
            continue;
        }
        if fs::read_dir(&full).is_err() {
            continue;
        }
        if !compileall_compile_dir_impl(&full, maxlevels - 1) {
            success = false;
        }
    }
    success
}

static EMAIL_MSGID_NEXT: AtomicU64 = AtomicU64::new(1);

fn email_message_default() -> MoltEmailMessage {
    MoltEmailMessage {
        headers: Vec::new(),
        body: String::new(),
        content_type: "text/plain".to_string(),
        filename: None,
        parts: Vec::new(),
        multipart_subtype: None,
    }
}

fn email_header_get(headers: &[(String, String)], name: &str) -> Option<String> {
    for (header_name, value) in headers.iter().rev() {
        if header_name.eq_ignore_ascii_case(name) {
            return Some(value.clone());
        }
    }
    None
}

fn email_fold_header(name: &str, value: &str) -> String {
    let prefix = format!("{name}: ");
    if prefix.len() + value.len() <= 78 {
        return format!("{prefix}{value}");
    }
    let mut out = prefix;
    let mut remaining = value.trim();
    let mut first = true;
    while !remaining.is_empty() {
        let max_len = if first { 72 } else { 74 };
        let take = remaining
            .char_indices()
            .take_while(|(idx, _)| *idx < max_len)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or_else(|| remaining.len().min(max_len));
        let (chunk, rest) = remaining.split_at(take);
        if !first {
            out.push(' ');
        }
        out.push_str(chunk.trim_end());
        if !rest.is_empty() {
            out.push('\n');
            first = false;
        }
        remaining = rest.trim_start();
    }
    out
}

fn email_serialize_message(message: &MoltEmailMessage) -> String {
    let mut out = String::new();
    for (name, value) in &message.headers {
        out.push_str(&email_fold_header(name, value));
        out.push('\n');
    }
    if message.parts.is_empty() {
        out.push_str(&format!("Content-Type: {}\n", message.content_type));
        if let Some(filename) = &message.filename {
            out.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}\"\n",
                filename
            ));
        }
        out.push('\n');
        out.push_str(&message.body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    let subtype = message
        .multipart_subtype
        .as_deref()
        .unwrap_or("mixed")
        .to_string();
    let boundary = "==MOLT_BOUNDARY==";
    out.push_str(&format!(
        "Content-Type: multipart/{}; boundary=\"{}\"\n\n",
        subtype, boundary
    ));
    for part in &message.parts {
        out.push_str(&format!("--{}\n", boundary));
        out.push_str(&format!("Content-Type: {}\n", part.content_type));
        if let Some(filename) = &part.filename {
            out.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}\"\n",
                filename
            ));
        }
        out.push('\n');
        out.push_str(&part.body);
        if !part.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&format!("--{}--\n", boundary));
    out
}

fn email_parse_simple_message(raw: &str) -> MoltEmailMessage {
    let mut message = email_message_default();
    let normalized = raw.replace("\r\n", "\n");
    let mut split = normalized.splitn(2, "\n\n");
    let header_block = split.next().unwrap_or_default();
    let body_block = split.next().unwrap_or_default();
    let mut last_header: Option<usize> = None;
    for line in header_block.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(idx) = last_header
                && let Some((_, value)) = message.headers.get_mut(idx)
            {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim().to_string();
        let value = line[colon + 1..].trim().to_string();
        if name.eq_ignore_ascii_case("content-type") {
            let base = value
                .split(';')
                .next()
                .unwrap_or(value.as_str())
                .trim()
                .to_string();
            message.content_type = if base.is_empty() {
                "text/plain".to_string()
            } else {
                base
            };
            continue;
        }
        message.headers.push((name, value));
        last_header = Some(message.headers.len().saturating_sub(1));
    }
    message.body = body_block.to_string();
    message
}

fn email_month_number(token: &str) -> Option<i64> {
    match token.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn email_month_name(month: i64) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    }
}

fn email_weekday_mon0(year: i64, month: i64, day: i64) -> i64 {
    // Sakamoto algorithm (returns 0=Sunday..6=Saturday).
    let t = [0i64, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    let m_index = usize::try_from(month.saturating_sub(1))
        .unwrap_or(0)
        .min(t.len().saturating_sub(1));
    let sun0 = (y + y / 4 - y / 100 + y / 400 + t[m_index] + day).rem_euclid(7);
    // Convert Sunday=0..Saturday=6 to Monday=0..Sunday=6.
    (sun0 + 6).rem_euclid(7)
}

fn email_weekday_name_mon0(mon0: i64) -> &'static str {
    match mon0 {
        0 => "Mon",
        1 => "Tue",
        2 => "Wed",
        3 => "Thu",
        4 => "Fri",
        5 => "Sat",
        6 => "Sun",
        _ => "Mon",
    }
}

fn email_parse_datetime_like(value: &str) -> Option<(i64, i64, i64, i64, i64, i64, i64)> {
    let mut text = value.trim();
    if let Some(comma) = text.find(',') {
        text = text[comma + 1..].trim();
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let day = parts[0].parse::<i64>().ok()?;
    let month = email_month_number(parts[1])?;
    let year = parts[2].parse::<i64>().ok()?;
    let mut time_iter = parts[3].split(':');
    let hour = time_iter.next()?.parse::<i64>().ok()?;
    let minute = time_iter.next()?.parse::<i64>().ok()?;
    let second = time_iter.next()?.parse::<i64>().ok()?;
    let tz = parts[4];
    if tz.len() != 5 {
        return None;
    }
    let sign = match &tz[0..1] {
        "+" => 1i64,
        "-" => -1i64,
        _ => return None,
    };
    let tz_hours = tz[1..3].parse::<i64>().ok()?;
    let tz_minutes = tz[3..5].parse::<i64>().ok()?;
    let offset = sign * (tz_hours * 3600 + tz_minutes * 60);
    Some((year, month, day, hour, minute, second, offset))
}

fn email_utils_format_datetime_impl(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> String {
    let wday = email_weekday_mon0(year, month, day);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} +0000",
        email_weekday_name_mon0(wday),
        day,
        email_month_name(month),
        year,
        hour,
        minute,
        second
    )
}

fn email_utils_parse_addresses(values: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for value in values {
        for token in value.split(',') {
            let entry = token.trim();
            if entry.is_empty() {
                continue;
            }
            if let (Some(start), Some(end)) = (entry.rfind('<'), entry.rfind('>'))
                && start < end
            {
                let name = entry[..start].trim().trim_matches('"').to_string();
                let addr = entry[start + 1..end].trim().to_string();
                out.push((name, addr));
                continue;
            }
            out.push((String::new(), entry.to_string()));
        }
    }
    out
}

fn email_header_encode_word_impl(text: &str, charset: Option<&str>) -> Result<String, String> {
    let active = charset.unwrap_or("utf-8");
    let lower = active.to_ascii_lowercase();
    if text.is_ascii() && (charset.is_none() || lower == "ascii" || lower == "us-ascii") {
        return Ok(text.to_string());
    }
    match lower.as_str() {
        "utf-8" | "utf8" => {
            let encoded = urllib_base64_encode(text.as_bytes());
            Ok(format!("=?utf-8?b?{}?=", encoded))
        }
        "ascii" | "us-ascii" => {
            if text.is_ascii() {
                Ok(text.to_string())
            } else {
                Err("non-ASCII header text with ASCII charset".to_string())
            }
        }
        _ => Err("unsupported email header charset".to_string()),
    }
}

fn email_address_addr_spec_impl(username: &str, domain: &str) -> String {
    if !username.is_empty() && !domain.is_empty() {
        format!("{username}@{domain}")
    } else if !domain.is_empty() {
        format!("@{domain}")
    } else {
        username.to_string()
    }
}

fn email_address_format_impl(display_name: &str, username: &str, domain: &str) -> String {
    let addr_spec = email_address_addr_spec_impl(username, domain);
    if !display_name.is_empty() && !addr_spec.is_empty() {
        format!("{display_name} <{addr_spec}>")
    } else if !display_name.is_empty() {
        display_name.to_string()
    } else {
        addr_spec
    }
}

fn email_get_int_attr(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<i64, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        let name_text = std::str::from_utf8(name).unwrap_or("attribute");
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            &format!("datetime object missing {name_text}"),
        ));
    }
    let Some(value) = to_i64(obj_from_bits(value_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "datetime field must be int",
        ));
    };
    Ok(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let id = email_message_register(email_message_default());
        email_message_bits_from_id(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_from_bytes(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = if let Some(ptr) = obj_from_bits(data_bits).as_ptr() {
            if let Some(bytes) = unsafe { bytes_like_slice(ptr) } {
                String::from_utf8_lossy(bytes).into_owned()
            } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
                text
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "message_from_bytes argument must be bytes-like",
                );
            }
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
            text
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "message_from_bytes argument must be bytes-like",
            );
        };
        let id = email_message_register(email_parse_simple_message(&raw));
        email_message_bits_from_id(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_set(
    message_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header value must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        message.headers.push((name, value));
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_get(message_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if let Some(value) = email_header_get(&message.headers, &name) {
            let value_ptr = alloc_string(_py, value.as_bytes());
            if value_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(value_ptr).bits()
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_set_content(message_bits: u64, content_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(content) = string_obj_to_owned(obj_from_bits(content_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "content must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        message.body = content;
        message.content_type = "text/plain".to_string();
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_add_alternative(
    message_bits: u64,
    content_bits: u64,
    subtype_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(content) = string_obj_to_owned(obj_from_bits(content_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "alternative content must be str");
        };
        let Some(subtype) = string_obj_to_owned(obj_from_bits(subtype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "alternative subtype must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if message.parts.is_empty() {
            let mut first = email_message_default();
            first.content_type = "text/plain".to_string();
            first.body = message.body.clone();
            message.parts.push(first);
            message.body.clear();
        }
        let mut alt = email_message_default();
        alt.content_type = format!("text/{}", subtype);
        alt.body = content;
        message.parts.push(alt);
        message.content_type = "multipart/alternative".to_string();
        message.multipart_subtype = Some("alternative".to_string());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_add_attachment(
    message_bits: u64,
    data_bits: u64,
    maintype_bits: u64,
    subtype_bits: u64,
    filename_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let payload = if let Some(ptr) = obj_from_bits(data_bits).as_ptr() {
            if let Some(bytes) = unsafe { bytes_like_slice(ptr) } {
                String::from_utf8_lossy(bytes).into_owned()
            } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
                text
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attachment payload must be bytes-like or str",
                );
            }
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
            text
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "attachment payload must be bytes-like or str",
            );
        };
        let Some(maintype) = string_obj_to_owned(obj_from_bits(maintype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maintype must be str");
        };
        let Some(subtype) = string_obj_to_owned(obj_from_bits(subtype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "subtype must be str");
        };
        let filename = if obj_from_bits(filename_bits).is_none() {
            None
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "filename must be str or None");
            };
            Some(value)
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if message.parts.is_empty() {
            let mut first = email_message_default();
            first.content_type = "text/plain".to_string();
            first.body = message.body.clone();
            message.parts.push(first);
            message.body.clear();
        }
        let mut part = email_message_default();
        part.content_type = format!("{}/{}", maintype, subtype);
        part.body = payload;
        part.filename = filename;
        message.parts.push(part);
        message.content_type = "multipart/mixed".to_string();
        message.multipart_subtype = Some("mixed".to_string());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_is_multipart(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        MoltObject::from_bool(!message.parts.is_empty()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_payload(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let (body, parts) = {
            let registry = email_message_registry()
                .lock()
                .expect("email message registry lock poisoned");
            let Some(message) = registry.get(&id) else {
                return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
            };
            (message.body.clone(), message.parts.clone())
        };
        if parts.is_empty() {
            let body_ptr = alloc_string(_py, body.as_bytes());
            if body_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(body_ptr).bits();
        }
        let mut handles: Vec<u64> = Vec::with_capacity(parts.len());
        for part in parts {
            let handle = email_message_register(part);
            handles.push(email_message_bits_from_id(_py, handle));
        }
        let list_ptr = alloc_list_with_capacity(_py, handles.as_slice(), handles.len());
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_content(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let out_ptr = alloc_string(_py, message.body.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_content_type(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let out_ptr = alloc_string(_py, message.content_type.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_filename(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if let Some(filename) = &message.filename {
            let out_ptr = alloc_string(_py, filename.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_as_string(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let rendered = email_serialize_message(message);
        let out_ptr = alloc_string(_py, rendered.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_items(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let mut pair_bits: Vec<u64> = Vec::with_capacity(message.headers.len());
        for (name, value) in &message.headers {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let value_ptr = alloc_string(_py, value.as_bytes());
            if value_ptr.is_null() {
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                dec_ref_bits(_py, name_bits);
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[name_bits, value_bits]);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, value_bits);
            if tuple_ptr.is_null() {
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list_with_capacity(_py, pair_bits.as_slice(), pair_bits.len());
        for bits in pair_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_drop(message_bits: u64) {
    crate::with_gil_entry!(_py, {
        let Ok(id) = email_message_id_from_bits(_py, message_bits) else {
            return;
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        registry.remove(&id);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_make_msgid(domain_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let domain = if obj_from_bits(domain_bits).is_none() {
            "localhost".to_string()
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "domain must be str or None");
            };
            value
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let seq = EMAIL_MSGID_NEXT.fetch_add(1, Ordering::Relaxed);
        let out = format!("<{}.{}@{}>", now, seq, domain);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_getaddresses(values_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let values = match iterable_to_string_vec(_py, values_bits) {
            Ok(v) => v,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let pairs = email_utils_parse_addresses(values.as_slice());
        let mut out_bits: Vec<u64> = Vec::with_capacity(pairs.len());
        for (name, addr) in pairs {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let addr_ptr = alloc_string(_py, addr.as_bytes());
            if addr_ptr.is_null() {
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                dec_ref_bits(_py, name_bits);
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let addr_bits = MoltObject::from_ptr(addr_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[name_bits, addr_bits]);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, addr_bits);
            if tuple_ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list_with_capacity(_py, out_bits.as_slice(), out_bits.len());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_parsedate_tz(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "date value must be str");
        };
        let Some((year, month, day, hour, minute, second, offset)) =
            email_parse_datetime_like(value.as_str())
        else {
            return MoltObject::none().bits();
        };
        // Match CPython email.utils.parsedate_tz behavior: slots 6/7 default to
        // (weekday=0, yearday=1) rather than computed calendar values.
        let wday = 0i64;
        let yday = 1i64;
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(year).bits(),
                MoltObject::from_int(month).bits(),
                MoltObject::from_int(day).bits(),
                MoltObject::from_int(hour).bits(),
                MoltObject::from_int(minute).bits(),
                MoltObject::from_int(second).bits(),
                MoltObject::from_int(wday).bits(),
                MoltObject::from_int(yday).bits(),
                MoltObject::from_int(-1).bits(),
                MoltObject::from_int(offset).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_format_datetime(dt_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let year = match email_get_int_attr(_py, dt_bits, b"year") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let month = match email_get_int_attr(_py, dt_bits, b"month") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let day = match email_get_int_attr(_py, dt_bits, b"day") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let hour = match email_get_int_attr(_py, dt_bits, b"hour") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let minute = match email_get_int_attr(_py, dt_bits, b"minute") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let second = match email_get_int_attr(_py, dt_bits, b"second") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = email_utils_format_datetime_impl(year, month, day, hour, minute, second);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_parsedate_to_datetime(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "date value must be str");
        };
        let Some((year, month, day, hour, minute, second, offset)) =
            email_parse_datetime_like(value.as_str())
        else {
            return raise_exception::<_>(_py, "ValueError", "invalid date value");
        };
        if offset != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "non-UTC email date offsets are not yet supported",
            );
        }
        let module_name_ptr = alloc_string(_py, b"datetime");
        if module_name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
        let module_bits = crate::molt_module_import(module_name_bits);
        dec_ref_bits(_py, module_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(datetime_name_bits) = attr_name_bits_from_bytes(_py, b"datetime") else {
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        };
        let Some(timezone_name_bits) = attr_name_bits_from_bytes(_py, b"timezone") else {
            dec_ref_bits(_py, datetime_name_bits);
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let datetime_class_bits = molt_getattr_builtin(module_bits, datetime_name_bits, missing);
        let timezone_class_bits = molt_getattr_builtin(module_bits, timezone_name_bits, missing);
        dec_ref_bits(_py, datetime_name_bits);
        dec_ref_bits(_py, timezone_name_bits);
        dec_ref_bits(_py, module_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if datetime_class_bits == missing || timezone_class_bits == missing {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "datetime module is missing required classes",
            );
        }
        let Some(utc_name_bits) = attr_name_bits_from_bytes(_py, b"utc") else {
            dec_ref_bits(_py, datetime_class_bits);
            dec_ref_bits(_py, timezone_class_bits);
            return MoltObject::none().bits();
        };
        let utc_bits = molt_getattr_builtin(timezone_class_bits, utc_name_bits, missing);
        dec_ref_bits(_py, utc_name_bits);
        dec_ref_bits(_py, timezone_class_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, datetime_class_bits);
            return MoltObject::none().bits();
        }
        if utc_bits == missing {
            dec_ref_bits(_py, datetime_class_bits);
            return raise_exception::<_>(_py, "RuntimeError", "datetime.timezone.utc missing");
        }
        let Some(datetime_class_ptr) = obj_from_bits(datetime_class_bits).as_ptr() else {
            dec_ref_bits(_py, utc_bits);
            dec_ref_bits(_py, datetime_class_bits);
            return raise_exception::<_>(_py, "TypeError", "datetime class is invalid");
        };
        let out_bits = unsafe {
            call_class_init_with_args(
                _py,
                datetime_class_ptr,
                &[
                    MoltObject::from_int(year).bits(),
                    MoltObject::from_int(month).bits(),
                    MoltObject::from_int(day).bits(),
                    MoltObject::from_int(hour).bits(),
                    MoltObject::from_int(minute).bits(),
                    MoltObject::from_int(second).bits(),
                    MoltObject::from_int(0).bits(),
                    utc_bits,
                ],
            )
        };
        dec_ref_bits(_py, utc_bits);
        dec_ref_bits(_py, datetime_class_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_policy_new(name_bits: u64, utf8_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "policy name must be str");
        };
        let utf8 = is_truthy(_py, obj_from_bits(utf8_bits));
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_obj_bits = MoltObject::from_ptr(name_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[name_obj_bits, MoltObject::from_bool(utf8).bits()]);
        dec_ref_bits(_py, name_obj_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_headerregistry_value(name_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
        let out_ptr = alloc_string(_py, value.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_header_encode_word(text_bits: u64, charset_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header text must be str");
        };
        let charset = if obj_from_bits(charset_bits).is_none() {
            None
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(charset_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "charset must be str or None");
            };
            Some(value)
        };
        let encoded = match email_header_encode_word_impl(text.as_str(), charset.as_deref()) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "RuntimeError", &msg),
        };
        let out_ptr = alloc_string(_py, encoded.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_address_addr_spec(username_bits: u64, domain_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(username) = string_obj_to_owned(obj_from_bits(username_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "username must be str");
        };
        let Some(domain) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "domain must be str");
        };
        let out = email_address_addr_spec_impl(username.as_str(), domain.as_str());
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_address_format(
    display_name_bits: u64,
    username_bits: u64,
    domain_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(display_name) = string_obj_to_owned(obj_from_bits(display_name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "display_name must be str");
        };
        let Some(username) = string_obj_to_owned(obj_from_bits(username_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "username must be str");
        };
        let Some(domain) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "domain must be str");
        };
        let out =
            email_address_format_impl(display_name.as_str(), username.as_str(), domain.as_str());
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_quote(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.quote argument must be str");
        };
        let out = shlex_quote_impl(&text);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split(text_bits: u64, whitespace_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split argument must be str");
        };
        let Some(whitespace) = string_obj_to_owned(obj_from_bits(whitespace_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split whitespace must be str");
        };
        let parts = match shlex_split_impl(&text, &whitespace, true, false, "#", true, "") {
            Ok(parts) => parts,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &parts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split_ex(
    text_bits: u64,
    whitespace_bits: u64,
    posix_bits: u64,
    comments_bits: u64,
    whitespace_split_bits: u64,
    commenters_bits: u64,
    punctuation_chars_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split argument must be str");
        };
        let Some(whitespace) = string_obj_to_owned(obj_from_bits(whitespace_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split whitespace must be str");
        };
        let Some(commenters) = string_obj_to_owned(obj_from_bits(commenters_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split commenters must be str");
        };
        let Some(punctuation_chars) = string_obj_to_owned(obj_from_bits(punctuation_chars_bits))
        else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "shlex.split punctuation_chars must be str",
            );
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let comments = is_truthy(_py, obj_from_bits(comments_bits));
        let whitespace_split = is_truthy(_py, obj_from_bits(whitespace_split_bits));
        let parts = match shlex_split_impl(
            &text,
            &whitespace,
            posix,
            comments,
            &commenters,
            whitespace_split,
            &punctuation_chars,
        ) {
            Ok(parts) => parts,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &parts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_join(words_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match iterable_to_string_vec(_py, words_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let out = shlex_join_impl(&parts);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_this_payload() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(s_bits) = alloc_string_bits(_py, THIS_ENCODED) else {
            return MoltObject::none().bits();
        };

        let mut pairs: Vec<u64> = Vec::with_capacity(52 * 2);
        let mut owned_pairs: Vec<u64> = Vec::with_capacity(52 * 2);
        for base in [b'A', b'a'] {
            for idx in 0u8..26u8 {
                let key = [(base + idx) as char];
                let value = [(base + ((idx + 13) % 26)) as char];
                let key_text: String = key.into_iter().collect();
                let value_text: String = value.into_iter().collect();
                let Some(key_bits) = alloc_string_bits(_py, &key_text) else {
                    dec_ref_bits(_py, s_bits);
                    for bits in owned_pairs {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                let Some(value_bits) = alloc_string_bits(_py, &value_text) else {
                    dec_ref_bits(_py, s_bits);
                    dec_ref_bits(_py, key_bits);
                    for bits in owned_pairs {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                pairs.push(key_bits);
                pairs.push(value_bits);
                owned_pairs.push(key_bits);
                owned_pairs.push(value_bits);
            }
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            dec_ref_bits(_py, s_bits);
            for bits in owned_pairs {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_pairs {
            dec_ref_bits(_py, bits);
        }

        let zen_text = this_build_rot13_text();
        let Some(zen_bits) = alloc_string_bits(_py, &zen_text) else {
            dec_ref_bits(_py, s_bits);
            dec_ref_bits(_py, dict_bits);
            return MoltObject::none().bits();
        };

        let payload_ptr = alloc_tuple(
            _py,
            &[
                s_bits,
                dict_bits,
                zen_bits,
                MoltObject::from_int(97).bits(),
                MoltObject::from_int(25).bits(),
            ],
        );
        dec_ref_bits(_py, s_bits);
        dec_ref_bits(_py, dict_bits);
        dec_ref_bits(_py, zen_bits);
        if payload_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(payload_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_encode(data_bits: u64, quotetabs_bits: u64, header_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "encodestring") {
            Ok(data) => data,
            Err(bits) => return bits,
        };
        let quotetabs = is_truthy(_py, obj_from_bits(quotetabs_bits));
        let header = is_truthy(_py, obj_from_bits(header_bits));
        let out = quopri_encode_impl(data.as_slice(), quotetabs, header);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_decode(data_bits: u64, header_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "decodestring") {
            Ok(data) => data,
            Err(bits) => return bits,
        };
        let header = is_truthy(_py, obj_from_bits(header_bits));
        let out = quopri_decode_impl(data.as_slice(), header);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_needs_quoting(
    c_bits: u64,
    quotetabs_bits: u64,
    header_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "needsquoting") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        let quotetabs = is_truthy(_py, obj_from_bits(quotetabs_bits));
        let header = is_truthy(_py, obj_from_bits(header_bits));
        MoltObject::from_bool(quopri_needs_quoting(byte, quotetabs, header)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_quote(c_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "quote") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        let mut out = Vec::with_capacity(3);
        quopri_quote_byte(byte, &mut out);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_ishex(c_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "ishex") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(quopri_is_hex(byte)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_unhex(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let bytes = match quopri_expect_bytes_like(_py, s_bits, "unhex") {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        if bytes.is_empty() {
            return MoltObject::from_int(0).bits();
        }
        let mut out = 0i64;
        for byte in bytes {
            let value = match byte {
                b'0'..=b'9' => i64::from(byte - b'0'),
                b'a'..=b'f' => i64::from(byte - b'a' + 10),
                b'A'..=b'F' => i64::from(byte - b'A' + 10),
                _ => return raise_exception::<_>(_py, "ValueError", "quopri unhex expects hex"),
            };
            out = out.saturating_mul(16).saturating_add(value);
        }
        MoltObject::from_int(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_check(octet_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let octet = match email_quopri_expect_int_octet(_py, octet_bits, "header_check") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut mapped = String::new();
        email_quopri_push_header_mapped(octet, &mut mapped);
        let same = mapped.len() == 1 && mapped.as_bytes()[0] == octet;
        MoltObject::from_bool(!same).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_check(octet_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let octet = match email_quopri_expect_int_octet(_py, octet_bits, "body_check") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(!email_quopri_body_safe(octet)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_length(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "email.quoprimime.header_length")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut total = 0i64;
        for byte in data {
            total += if email_quopri_header_safe(byte) || byte == b' ' {
                1
            } else {
                3
            };
        }
        MoltObject::from_int(total).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_length(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "email.quoprimime.body_length") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut total = 0i64;
        for byte in data {
            total += if email_quopri_body_safe(byte) { 1 } else { 3 };
        }
        MoltObject::from_int(total).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_quote(c_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let c = match email_quopri_expect_string(_py, c_bits, "quote") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut it = c.chars();
        let Some(ch) = it.next() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "ord() expected a character, but string of length 0 found",
            );
        };
        if it.next().is_some() {
            let msg = format!(
                "ord() expected a character, but string of length {} found",
                c.chars().count()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if (ch as u32) > 255 {
            return raise_exception::<_>(_py, "IndexError", "list index out of range");
        }
        let mut out = String::with_capacity(3);
        email_quopri_push_escape(ch as u8, &mut out);
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_unquote(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match email_quopri_expect_string(_py, s_bits, "unquote") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 3 || chars[0] != '=' {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "invalid literal for int() with base 16",
            );
        }
        let Some(ch) = email_quopri_decode_hex_pair(chars[1], chars[2]) else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "invalid literal for int() with base 16",
            );
        };
        let out: String = [ch].into_iter().collect();
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_encode(
    header_bytes_bits: u64,
    charset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let header_bytes = match quopri_expect_bytes_like(
            _py,
            header_bytes_bits,
            "email.quoprimime.header_encode",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let charset = match email_quopri_expect_string(_py, charset_bits, "header_encode charset") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if header_bytes.is_empty() {
            return email_quopri_alloc_str(_py, "");
        }
        let mut encoded = String::with_capacity(header_bytes.len() * 3);
        for byte in header_bytes {
            email_quopri_push_header_mapped(byte, &mut encoded);
        }
        let out = format!("=?{charset}?q?{encoded}?=");
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_decode(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match email_quopri_expect_string(_py, s_bits, "header_decode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let replaced = s.replace('_', " ");
        let chars: Vec<char> = replaced.chars().collect();
        let mut out = String::with_capacity(replaced.len());
        let mut idx = 0usize;
        while idx < chars.len() {
            if chars[idx] == '='
                && idx + 2 < chars.len()
                && email_quopri_is_hex_char(chars[idx + 1])
                && email_quopri_is_hex_char(chars[idx + 2])
                && let Some(ch) = email_quopri_decode_hex_pair(chars[idx + 1], chars[idx + 2])
            {
                out.push(ch);
                idx += 3;
                continue;
            }
            out.push(chars[idx]);
            idx += 1;
        }
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_encode(
    body_bits: u64,
    maxlinelen_bits: u64,
    eol_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let body = match email_quopri_expect_string(_py, body_bits, "body_encode body") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let maxlinelen = match to_i64(obj_from_bits(maxlinelen_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "maxlinelen must be int"),
        };
        if maxlinelen < 4 {
            return raise_exception::<_>(_py, "ValueError", "maxlinelen must be at least 4");
        }
        let eol = match email_quopri_expect_string(_py, eol_bits, "body_encode eol") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if body.is_empty() {
            return email_quopri_alloc_str(_py, body.as_str());
        }

        let mut quoted = String::with_capacity(body.len() + 8);
        for ch in body.chars() {
            let code = ch as u32;
            if code <= 255 {
                let byte = code as u8;
                if matches!(byte, b'\r' | b'\n') {
                    quoted.push(ch);
                } else {
                    email_quopri_push_body_mapped(byte, &mut quoted);
                }
            } else {
                quoted.push(ch);
            }
        }

        let soft_break = format!("={eol}");
        let maxlinelen1 = (maxlinelen as usize) - 1;
        let mut encoded_lines: Vec<String> = Vec::new();
        for line in email_quopri_splitlines(quoted.as_str()) {
            let chars: Vec<char> = line.chars().collect();
            let mut start = 0usize;
            let laststart = (chars.len() as isize) - 1 - (maxlinelen as isize);
            while (start as isize) <= laststart {
                let stop = start + maxlinelen1;
                if chars[stop - 2] == '=' {
                    encoded_lines.push(chars[start..stop - 1].iter().collect());
                    start = stop - 2;
                } else if chars[stop - 1] == '=' {
                    encoded_lines.push(chars[start..stop].iter().collect());
                    start = stop - 1;
                } else {
                    let mut segment: String = chars[start..stop].iter().collect();
                    segment.push('=');
                    encoded_lines.push(segment);
                    start = stop;
                }
            }

            if !chars.is_empty() && matches!(chars[chars.len() - 1], ' ' | '\t') {
                let room = (start as isize) - laststart;
                let mut q = String::new();
                if room >= 3 {
                    email_quopri_push_escape(chars[chars.len() - 1] as u8, &mut q);
                } else if room == 2 {
                    q.push(chars[chars.len() - 1]);
                    q.push_str(soft_break.as_str());
                } else {
                    q.push_str(soft_break.as_str());
                    email_quopri_push_escape(chars[chars.len() - 1] as u8, &mut q);
                }
                let mut segment: String = chars[start..chars.len() - 1].iter().collect();
                segment.push_str(q.as_str());
                encoded_lines.push(segment);
            } else {
                encoded_lines.push(chars[start..].iter().collect());
            }
        }

        if matches!(quoted.chars().last(), Some('\r' | '\n')) {
            encoded_lines.push(String::new());
        }

        let out = encoded_lines.join(eol.as_str());
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_decode(encoded_bits: u64, eol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let encoded = match email_quopri_expect_string(_py, encoded_bits, "decode encoded") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let eol = match email_quopri_expect_string(_py, eol_bits, "decode eol") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if encoded.is_empty() {
            return email_quopri_alloc_str(_py, encoded.as_str());
        }

        let mut decoded = String::new();
        for line in email_quopri_splitlines(encoded.as_str()) {
            let line = line.trim_end_matches(char::is_whitespace);
            if line.is_empty() {
                decoded.push_str(eol.as_str());
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            let mut idx = 0usize;
            let n = chars.len();
            while idx < n {
                let c = chars[idx];
                if c != '=' {
                    decoded.push(c);
                    idx += 1;
                } else if idx + 1 == n {
                    idx += 1;
                    continue;
                } else if idx + 2 < n
                    && email_quopri_is_hex_char(chars[idx + 1])
                    && email_quopri_is_hex_char(chars[idx + 2])
                {
                    if let Some(ch) = email_quopri_decode_hex_pair(chars[idx + 1], chars[idx + 2]) {
                        decoded.push(ch);
                        idx += 3;
                    } else {
                        decoded.push(c);
                        idx += 1;
                    }
                } else {
                    decoded.push(c);
                    idx += 1;
                }
                if idx == n {
                    decoded.push_str(eol.as_str());
                }
            }
        }

        if !encoded.ends_with('\r')
            && !encoded.ends_with('\n')
            && !eol.is_empty()
            && decoded.ends_with(eol.as_str())
        {
            let trim = decoded.len() - eol.len();
            decoded.truncate(trim);
        }
        email_quopri_alloc_str(_py, decoded.as_str())
    })
}

fn opcode_num_popped_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                 // CACHE
        1 => Some(1),                 // POP_TOP
        2 => Some(0),                 // PUSH_NULL
        3 => Some(1),                 // INTERPRETER_EXIT
        4 => Some(1 + 1),             // END_FOR
        5 => Some(2),                 // END_SEND
        9 => Some(0),                 // NOP
        11 => Some(1),                // UNARY_NEGATIVE
        12 => Some(1),                // UNARY_NOT
        15 => Some(1),                // UNARY_INVERT
        17 => Some(0),                // RESERVED
        25 => Some(2),                // BINARY_SUBSCR
        26 => Some(3),                // BINARY_SLICE
        27 => Some(4),                // STORE_SLICE
        30 => Some(1),                // GET_LEN
        31 => Some(1),                // MATCH_MAPPING
        32 => Some(1),                // MATCH_SEQUENCE
        33 => Some(2),                // MATCH_KEYS
        35 => Some(1),                // PUSH_EXC_INFO
        36 => Some(2),                // CHECK_EXC_MATCH
        37 => Some(2),                // CHECK_EG_MATCH
        49 => Some(4),                // WITH_EXCEPT_START
        50 => Some(1),                // GET_AITER
        51 => Some(1),                // GET_ANEXT
        52 => Some(1),                // BEFORE_ASYNC_WITH
        53 => Some(1),                // BEFORE_WITH
        54 => Some(2),                // END_ASYNC_FOR
        55 => Some(3),                // CLEANUP_THROW
        60 => Some(3),                // STORE_SUBSCR
        61 => Some(2),                // DELETE_SUBSCR
        68 => Some(1),                // GET_ITER
        69 => Some(1),                // GET_YIELD_FROM_ITER
        71 => Some(0),                // LOAD_BUILD_CLASS
        74 => Some(0),                // LOAD_ASSERTION_ERROR
        75 => Some(0),                // RETURN_GENERATOR
        83 => Some(1),                // RETURN_VALUE
        85 => Some(0),                // SETUP_ANNOTATIONS
        87 => Some(0),                // LOAD_LOCALS
        89 => Some(1),                // POP_EXCEPT
        90 => Some(1),                // STORE_NAME
        91 => Some(0),                // DELETE_NAME
        92 => Some(1),                // UNPACK_SEQUENCE
        93 => Some(1),                // FOR_ITER
        94 => Some(1),                // UNPACK_EX
        95 => Some(2),                // STORE_ATTR
        96 => Some(1),                // DELETE_ATTR
        97 => Some(1),                // STORE_GLOBAL
        98 => Some(0),                // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),  // SWAP
        100 => Some(0),               // LOAD_CONST
        101 => Some(0),               // LOAD_NAME
        102 => Some(oparg),           // BUILD_TUPLE
        103 => Some(oparg),           // BUILD_LIST
        104 => Some(oparg),           // BUILD_SET
        105 => Some(oparg * 2),       // BUILD_MAP
        106 => Some(1),               // LOAD_ATTR
        107 => Some(2),               // COMPARE_OP
        108 => Some(2),               // IMPORT_NAME
        109 => Some(1),               // IMPORT_FROM
        110 => Some(0),               // JUMP_FORWARD
        114 => Some(1),               // POP_JUMP_IF_FALSE
        115 => Some(1),               // POP_JUMP_IF_TRUE
        116 => Some(0),               // LOAD_GLOBAL
        117 => Some(2),               // IS_OP
        118 => Some(2),               // CONTAINS_OP
        119 => Some(oparg + 1),       // RERAISE
        120 => Some((oparg - 1) + 1), // COPY
        121 => Some(0),               // RETURN_CONST
        122 => Some(2),               // BINARY_OP
        123 => Some(2),               // SEND
        124 => Some(0),               // LOAD_FAST
        125 => Some(1),               // STORE_FAST
        126 => Some(0),               // DELETE_FAST
        127 => Some(0),               // LOAD_FAST_CHECK
        128 => Some(1),               // POP_JUMP_IF_NOT_NONE
        129 => Some(1),               // POP_JUMP_IF_NONE
        130 => Some(oparg),           // RAISE_VARARGS
        131 => Some(1),               // GET_AWAITABLE
        132 => Some(
            (if (oparg & 0x01) != 0 { 1 } else { 0 })
                + (if (oparg & 0x02) != 0 { 1 } else { 0 })
                + (if (oparg & 0x04) != 0 { 1 } else { 0 })
                + (if (oparg & 0x08) != 0 { 1 } else { 0 })
                + 1,
        ), // MAKE_FUNCTION
        133 => Some((if oparg == 3 { 1 } else { 0 }) + 2), // BUILD_SLICE
        134 => Some(0),               // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),               // MAKE_CELL
        136 => Some(0),               // LOAD_CLOSURE
        137 => Some(0),               // LOAD_DEREF
        138 => Some(1),               // STORE_DEREF
        139 => Some(0),               // DELETE_DEREF
        140 => Some(0),               // JUMP_BACKWARD
        141 => Some(3),               // LOAD_SUPER_ATTR
        142 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 3), // CALL_FUNCTION_EX
        143 => Some(0),               // LOAD_FAST_AND_CLEAR
        144 => Some(0),               // EXTENDED_ARG
        145 => Some((oparg - 1) + 2), // LIST_APPEND
        146 => Some((oparg - 1) + 2), // SET_ADD
        147 => Some(2),               // MAP_ADD
        149 => Some(0),               // COPY_FREE_VARS
        150 => Some(1),               // YIELD_VALUE
        151 => Some(0),               // RESUME
        152 => Some(3),               // MATCH_CLASS
        155 => Some((if (oparg & 0x04) == 0x04 { 1 } else { 0 }) + 1), // FORMAT_VALUE
        156 => Some(oparg + 1),       // BUILD_CONST_KEY_MAP
        157 => Some(oparg),           // BUILD_STRING
        162 => Some((oparg - 1) + 2), // LIST_EXTEND
        163 => Some((oparg - 1) + 2), // SET_UPDATE
        164 => Some(1),               // DICT_MERGE
        165 => Some(1),               // DICT_UPDATE
        171 => Some(oparg + 2),       // CALL
        172 => Some(0),               // KW_NAMES
        173 => Some(1),               // CALL_INTRINSIC_1
        174 => Some(2),               // CALL_INTRINSIC_2
        175 => Some(1),               // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),               // LOAD_FROM_DICT_OR_DEREF
        237 => Some(3),               // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0),               // INSTRUMENTED_RESUME
        241 => Some(0),               // INSTRUMENTED_CALL
        242 => Some(1),               // INSTRUMENTED_RETURN_VALUE
        243 => Some(1),               // INSTRUMENTED_YIELD_VALUE
        244 => Some(0),               // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0),               // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0),               // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0),               // INSTRUMENTED_RETURN_CONST
        248 => Some(0),               // INSTRUMENTED_FOR_ITER
        249 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(2),               // INSTRUMENTED_END_FOR
        252 => Some(2),               // INSTRUMENTED_END_SEND
        253 => Some(0),               // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}

fn opcode_num_pushed_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                                            // CACHE
        1 => Some(0),                                            // POP_TOP
        2 => Some(1),                                            // PUSH_NULL
        3 => Some(0),                                            // INTERPRETER_EXIT
        4 => Some(0),                                            // END_FOR
        5 => Some(1),                                            // END_SEND
        9 => Some(0),                                            // NOP
        11 => Some(1),                                           // UNARY_NEGATIVE
        12 => Some(1),                                           // UNARY_NOT
        15 => Some(1),                                           // UNARY_INVERT
        17 => Some(0),                                           // RESERVED
        25 => Some(1),                                           // BINARY_SUBSCR
        26 => Some(1),                                           // BINARY_SLICE
        27 => Some(0),                                           // STORE_SLICE
        30 => Some(2),                                           // GET_LEN
        31 => Some(2),                                           // MATCH_MAPPING
        32 => Some(2),                                           // MATCH_SEQUENCE
        33 => Some(3),                                           // MATCH_KEYS
        35 => Some(2),                                           // PUSH_EXC_INFO
        36 => Some(2),                                           // CHECK_EXC_MATCH
        37 => Some(2),                                           // CHECK_EG_MATCH
        49 => Some(5),                                           // WITH_EXCEPT_START
        50 => Some(1),                                           // GET_AITER
        51 => Some(2),                                           // GET_ANEXT
        52 => Some(2),                                           // BEFORE_ASYNC_WITH
        53 => Some(2),                                           // BEFORE_WITH
        54 => Some(0),                                           // END_ASYNC_FOR
        55 => Some(2),                                           // CLEANUP_THROW
        60 => Some(0),                                           // STORE_SUBSCR
        61 => Some(0),                                           // DELETE_SUBSCR
        68 => Some(1),                                           // GET_ITER
        69 => Some(1),                                           // GET_YIELD_FROM_ITER
        71 => Some(1),                                           // LOAD_BUILD_CLASS
        74 => Some(1),                                           // LOAD_ASSERTION_ERROR
        75 => Some(0),                                           // RETURN_GENERATOR
        83 => Some(0),                                           // RETURN_VALUE
        85 => Some(0),                                           // SETUP_ANNOTATIONS
        87 => Some(1),                                           // LOAD_LOCALS
        89 => Some(0),                                           // POP_EXCEPT
        90 => Some(0),                                           // STORE_NAME
        91 => Some(0),                                           // DELETE_NAME
        92 => Some(oparg),                                       // UNPACK_SEQUENCE
        93 => Some(2),                                           // FOR_ITER
        94 => Some((oparg & 0xFF) + (oparg >> 8) + 1),           // UNPACK_EX
        95 => Some(0),                                           // STORE_ATTR
        96 => Some(0),                                           // DELETE_ATTR
        97 => Some(0),                                           // STORE_GLOBAL
        98 => Some(0),                                           // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),                             // SWAP
        100 => Some(1),                                          // LOAD_CONST
        101 => Some(1),                                          // LOAD_NAME
        102 => Some(1),                                          // BUILD_TUPLE
        103 => Some(1),                                          // BUILD_LIST
        104 => Some(1),                                          // BUILD_SET
        105 => Some(1),                                          // BUILD_MAP
        106 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_ATTR
        107 => Some(1),                                          // COMPARE_OP
        108 => Some(1),                                          // IMPORT_NAME
        109 => Some(2),                                          // IMPORT_FROM
        110 => Some(0),                                          // JUMP_FORWARD
        114 => Some(0),                                          // POP_JUMP_IF_FALSE
        115 => Some(0),                                          // POP_JUMP_IF_TRUE
        116 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_GLOBAL
        117 => Some(1),                                          // IS_OP
        118 => Some(1),                                          // CONTAINS_OP
        119 => Some(oparg),                                      // RERAISE
        120 => Some((oparg - 1) + 2),                            // COPY
        121 => Some(0),                                          // RETURN_CONST
        122 => Some(1),                                          // BINARY_OP
        123 => Some(2),                                          // SEND
        124 => Some(1),                                          // LOAD_FAST
        125 => Some(0),                                          // STORE_FAST
        126 => Some(0),                                          // DELETE_FAST
        127 => Some(1),                                          // LOAD_FAST_CHECK
        128 => Some(0),                                          // POP_JUMP_IF_NOT_NONE
        129 => Some(0),                                          // POP_JUMP_IF_NONE
        130 => Some(0),                                          // RAISE_VARARGS
        131 => Some(1),                                          // GET_AWAITABLE
        132 => Some(1),                                          // MAKE_FUNCTION
        133 => Some(1),                                          // BUILD_SLICE
        134 => Some(0),                                          // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),                                          // MAKE_CELL
        136 => Some(1),                                          // LOAD_CLOSURE
        137 => Some(1),                                          // LOAD_DEREF
        138 => Some(0),                                          // STORE_DEREF
        139 => Some(0),                                          // DELETE_DEREF
        140 => Some(0),                                          // JUMP_BACKWARD
        141 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_SUPER_ATTR
        142 => Some(1),                                          // CALL_FUNCTION_EX
        143 => Some(1),                                          // LOAD_FAST_AND_CLEAR
        144 => Some(0),                                          // EXTENDED_ARG
        145 => Some((oparg - 1) + 1),                            // LIST_APPEND
        146 => Some((oparg - 1) + 1),                            // SET_ADD
        147 => Some(0),                                          // MAP_ADD
        149 => Some(0),                                          // COPY_FREE_VARS
        150 => Some(1),                                          // YIELD_VALUE
        151 => Some(0),                                          // RESUME
        152 => Some(1),                                          // MATCH_CLASS
        155 => Some(1),                                          // FORMAT_VALUE
        156 => Some(1),                                          // BUILD_CONST_KEY_MAP
        157 => Some(1),                                          // BUILD_STRING
        162 => Some((oparg - 1) + 1),                            // LIST_EXTEND
        163 => Some((oparg - 1) + 1),                            // SET_UPDATE
        164 => Some(0),                                          // DICT_MERGE
        165 => Some(0),                                          // DICT_UPDATE
        171 => Some(1),                                          // CALL
        172 => Some(0),                                          // KW_NAMES
        173 => Some(1),                                          // CALL_INTRINSIC_1
        174 => Some(1),                                          // CALL_INTRINSIC_2
        175 => Some(1),                                          // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),                                          // LOAD_FROM_DICT_OR_DEREF
        237 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),                                          // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0), // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0), // INSTRUMENTED_RESUME
        241 => Some(0), // INSTRUMENTED_CALL
        242 => Some(0), // INSTRUMENTED_RETURN_VALUE
        243 => Some(1), // INSTRUMENTED_YIELD_VALUE
        244 => Some(0), // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0), // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0), // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0), // INSTRUMENTED_RETURN_CONST
        248 => Some(0), // INSTRUMENTED_FOR_ITER
        249 => Some(0), // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0), // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(0), // INSTRUMENTED_END_FOR
        252 => Some(1), // INSTRUMENTED_END_SEND
        253 => Some(0), // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}

fn opcode_is_noarg_pseudo_312(opcode: i64) -> bool {
    matches!(opcode, 256..=259)
}

fn opcode_stack_effect_pseudo_312(opcode: i64) -> Option<i64> {
    match opcode {
        256 => Some(1),  // SETUP_FINALLY (max jump/non-jump)
        257 => Some(2),  // SETUP_CLEANUP (max jump/non-jump)
        258 => Some(1),  // SETUP_WITH (max jump/non-jump)
        259 => Some(0),  // POP_BLOCK
        260 => Some(0),  // JUMP
        261 => Some(0),  // JUMP_NO_INTERRUPT
        262 => Some(1),  // LOAD_METHOD
        263 => Some(-1), // LOAD_SUPER_METHOD
        264 => Some(-1), // LOAD_ZERO_SUPER_METHOD
        265 => Some(-1), // LOAD_ZERO_SUPER_ATTR
        266 => Some(-1), // STORE_FAST_MAYBE_NULL
        _ => None,
    }
}

#[inline]
fn opcode_is_noarg_312(opcode: i64) -> bool {
    opcode < 90 || opcode_is_noarg_pseudo_312(opcode)
}

#[inline]
fn opcode_stack_effect_core_312(opcode: i64, oparg: i64) -> Option<i64> {
    if let Some(effect) = opcode_stack_effect_pseudo_312(opcode) {
        return Some(effect);
    }
    let popped = opcode_num_popped_312(opcode, oparg)?;
    let pushed = opcode_num_pushed_312(opcode, oparg)?;
    if popped < 0 || pushed < 0 {
        return None;
    }
    pushed.checked_sub(popped)
}

fn token_payload_json_value_to_bits(
    _py: &crate::PyToken<'_>,
    value: &JsonValue,
) -> Result<u64, u64> {
    match value {
        JsonValue::Null => Ok(MoltObject::none().bits()),
        JsonValue::Bool(flag) => Ok(MoltObject::from_bool(*flag).bits()),
        JsonValue::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Ok(MoltObject::from_int(integer).bits());
            }
            if let Some(integer) = number.as_u64() {
                let Ok(integer_i64) = i64::try_from(integer) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "token payload number is out of range",
                    ));
                };
                return Ok(MoltObject::from_int(integer_i64).bits());
            }
            if let Some(float_value) = number.as_f64() {
                return Ok(MoltObject::from_float(float_value).bits());
            }
            Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "token payload number is invalid",
            ))
        }
        JsonValue::String(text) => {
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        JsonValue::Array(items) => {
            let mut item_bits: Vec<u64> = Vec::with_capacity(items.len());
            for item in items {
                let bits = match token_payload_json_value_to_bits(_py, item) {
                    Ok(bits) => bits,
                    Err(err_bits) => {
                        for owned in item_bits {
                            dec_ref_bits(_py, owned);
                        }
                        return Err(err_bits);
                    }
                };
                item_bits.push(bits);
            }
            let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            if list_ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(list_ptr).bits())
            }
        }
        JsonValue::Object(entries) => {
            let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            for (key, item) in entries {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    for owned in owned_bits {
                        dec_ref_bits(_py, owned);
                    }
                    return Err(MoltObject::none().bits());
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let value_bits = match token_payload_json_value_to_bits(_py, item) {
                    Ok(bits) => bits,
                    Err(err_bits) => {
                        dec_ref_bits(_py, key_bits);
                        for owned in owned_bits {
                            dec_ref_bits(_py, owned);
                        }
                        return Err(err_bits);
                    }
                };
                pairs.push(key_bits);
                pairs.push(value_bits);
                owned_bits.push(key_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            if dict_ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(dict_ptr).bits())
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_PAYLOAD_312_JSON)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, { email_quopri_alloc_str(_py, TOKEN_PAYLOAD_312_JSON) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312() -> u64 {
    crate::with_gil_entry!(_py, {
        let parsed: JsonValue = match serde_json::from_str(TOKEN_PAYLOAD_312_JSON) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid token payload json: {err}");
                return raise_exception::<u64>(_py, "RuntimeError", msg.as_str());
            }
        };
        match token_payload_json_value_to_bits(_py, &parsed) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_metadata_payload_314_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_METADATA_PAYLOAD_314_JSON)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_get_specialization_stats() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_stack_effect(opcode_bits: u64, oparg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let opcode_obj = obj_from_bits(opcode_bits);
        let Some(opcode) = to_i64(opcode_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, opcode_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let oparg_obj = obj_from_bits(oparg_bits);
        let opcode_noarg = opcode_is_noarg_312(opcode);
        if oparg_obj.is_none() {
            if opcode_noarg {
                return match opcode_stack_effect_core_312(opcode, 0) {
                    Some(effect) => MoltObject::from_int(effect).bits(),
                    None => raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg"),
                };
            }
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode requires oparg but oparg was not specified",
            );
        }
        if opcode_noarg {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode does not permit oparg but oparg was specified",
            );
        }

        let Some(oparg) = to_i64(oparg_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, oparg_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let Some(effect) = opcode_stack_effect_core_312(opcode, oparg) else {
            return raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg");
        };
        MoltObject::from_int(effect).bits()
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ArgparseOptionalKind {
    Value,
    StoreTrue,
}

#[derive(Clone)]
struct ArgparseOptionalSpec {
    flag: String,
    dest: String,
    kind: ArgparseOptionalKind,
    required: bool,
    default: JsonValue,
}

#[derive(Clone)]
struct ArgparseSubparsersSpec {
    dest: String,
    required: bool,
    parsers: HashMap<String, ArgparseSpec>,
}

#[derive(Clone)]
struct ArgparseSpec {
    optionals: Vec<ArgparseOptionalSpec>,
    positionals: Vec<String>,
    subparsers: Option<ArgparseSubparsersSpec>,
}

fn argparse_choice_list(parsers: &HashMap<String, ArgparseSpec>) -> String {
    let mut keys: Vec<&str> = parsers.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.join(", ")
}

fn argparse_decode_spec(value: &JsonValue) -> Result<ArgparseSpec, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "argparse spec must be a JSON object".to_string())?;

    let mut optionals: Vec<ArgparseOptionalSpec> = Vec::new();
    if let Some(raw_optionals) = obj.get("optionals") {
        let items = raw_optionals
            .as_array()
            .ok_or_else(|| "argparse optionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse optional spec must be object".to_string())?;
            let flag = item_obj
                .get("flag")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string flag".to_string())?
                .to_string();
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string dest".to_string())?
                .to_string();
            let kind = item_obj
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string kind".to_string())?;
            let parsed_kind = match kind {
                "value" => ArgparseOptionalKind::Value,
                "store_true" => ArgparseOptionalKind::StoreTrue,
                _ => return Err(format!("unsupported argparse optional kind: {kind}")),
            };
            let required = item_obj
                .get("required")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let default = item_obj.get("default").cloned().unwrap_or_else(|| {
                if parsed_kind == ArgparseOptionalKind::StoreTrue {
                    JsonValue::Bool(false)
                } else {
                    JsonValue::Null
                }
            });
            optionals.push(ArgparseOptionalSpec {
                flag,
                dest,
                kind: parsed_kind,
                required,
                default,
            });
        }
    }

    let mut positionals: Vec<String> = Vec::new();
    if let Some(raw_positionals) = obj.get("positionals") {
        let items = raw_positionals
            .as_array()
            .ok_or_else(|| "argparse positionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse positional spec must be object".to_string())?;
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse positional spec missing string dest".to_string())?
                .to_string();
            positionals.push(dest);
        }
    }

    let subparsers = if let Some(raw_subparsers) = obj.get("subparsers") {
        let sp_obj = raw_subparsers
            .as_object()
            .ok_or_else(|| "argparse subparsers spec must be object".to_string())?;
        let dest = sp_obj
            .get("dest")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "argparse subparsers spec missing string dest".to_string())?
            .to_string();
        let required = sp_obj
            .get("required")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let parsers_obj = sp_obj
            .get("parsers")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| "argparse subparsers spec missing parsers object".to_string())?;
        let mut parsers: HashMap<String, ArgparseSpec> = HashMap::new();
        for (name, parser_spec) in parsers_obj {
            let parsed = argparse_decode_spec(parser_spec)?;
            parsers.insert(name.clone(), parsed);
        }
        Some(ArgparseSubparsersSpec {
            dest,
            required,
            parsers,
        })
    } else {
        None
    };

    Ok(ArgparseSpec {
        optionals,
        positionals,
        subparsers,
    })
}

fn argparse_parse_with_spec(
    spec: &ArgparseSpec,
    argv: &[String],
) -> Result<JsonMap<String, JsonValue>, String> {
    let mut out: JsonMap<String, JsonValue> = JsonMap::new();
    let mut optional_dest_seen: HashSet<String> = HashSet::new();
    for opt in &spec.optionals {
        out.insert(opt.dest.clone(), opt.default.clone());
    }

    let mut pos_index = 0usize;
    let mut index = 0usize;

    while index < argv.len() {
        let token = &argv[index];
        if token.starts_with('-') && token != "-" {
            let Some(opt) = spec.optionals.iter().find(|entry| entry.flag == *token) else {
                return Err(format!("unrecognized arguments: {token}"));
            };
            optional_dest_seen.insert(opt.dest.clone());
            match opt.kind {
                ArgparseOptionalKind::StoreTrue => {
                    out.insert(opt.dest.clone(), JsonValue::Bool(true));
                    index += 1;
                }
                ArgparseOptionalKind::Value => {
                    if index + 1 >= argv.len() {
                        return Err(format!("argument {}: expected one argument", opt.flag));
                    }
                    let value = argv[index + 1].clone();
                    out.insert(opt.dest.clone(), JsonValue::String(value));
                    index += 2;
                }
            }
            continue;
        }

        if pos_index < spec.positionals.len() {
            let dest = spec.positionals[pos_index].clone();
            out.insert(dest, JsonValue::String(token.clone()));
            pos_index += 1;
            index += 1;
            continue;
        }

        if let Some(subparsers) = &spec.subparsers {
            if let Some(child_spec) = subparsers.parsers.get(token) {
                out.insert(subparsers.dest.clone(), JsonValue::String(token.clone()));
                let child = argparse_parse_with_spec(child_spec, &argv[index + 1..])?;
                for (key, value) in child {
                    out.insert(key, value);
                }
                break;
            }
            let choices = argparse_choice_list(&subparsers.parsers);
            return Err(format!(
                "argument {}: invalid choice: '{}' (choose from {})",
                subparsers.dest, token, choices
            ));
        }

        return Err(format!("unrecognized arguments: {token}"));
    }

    if pos_index < spec.positionals.len() {
        let missing = spec.positionals[pos_index..].join(", ");
        return Err(format!("the following arguments are required: {missing}"));
    }

    for opt in &spec.optionals {
        if opt.required && !optional_dest_seen.contains(&opt.dest) {
            return Err(format!(
                "the following arguments are required: {}",
                opt.flag
            ));
        }
    }

    if let Some(subparsers) = &spec.subparsers
        && subparsers.required
        && !out.contains_key(&subparsers.dest)
    {
        return Err(format!(
            "the following arguments are required: {}",
            subparsers.dest
        ));
    }

    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parse(spec_json_bits: u64, argv_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(spec_json) = string_obj_to_owned(obj_from_bits(spec_json_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "argparse spec_json must be str");
        };
        let argv = match iterable_to_string_vec(_py, argv_bits) {
            Ok(values) => values,
            Err(bits) => return bits,
        };

        let spec_value: JsonValue = match serde_json::from_str(spec_json.as_str()) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid argparse spec json: {err}");
                return raise_exception::<_>(_py, "ValueError", msg.as_str());
            }
        };
        let spec = match argparse_decode_spec(&spec_value) {
            Ok(spec) => spec,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let parsed = match argparse_parse_with_spec(&spec, argv.as_slice()) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let payload = match serde_json::to_string(&JsonValue::Object(parsed)) {
            Ok(payload) => payload,
            Err(err) => {
                let msg = format!("argparse payload encode failed: {err}");
                return raise_exception::<_>(_py, "RuntimeError", msg.as_str());
            }
        };
        let out_ptr = alloc_string(_py, payload.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatchcase(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_impl(&name, &pat)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name, &pat)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_text(&name);
            let pat_norm = fnmatch_normcase_text(&pat);
            return MoltObject::from_bool(fnmatch_match_impl(&name_norm, &pat_norm)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_bytes(&name);
            let pat_norm = fnmatch_normcase_bytes(&pat);
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name_norm, &pat_norm)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_filter(names_bits: u64, pat_bits: u64, invert_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let pat_str = string_obj_to_owned(obj_from_bits(pat_bits));
        let pat_bytes = if pat_str.is_none() {
            fnmatch_bytes_from_bits(pat_bits)
        } else {
            None
        };
        if pat_str.is_none() && pat_bytes.is_none() {
            return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
        }
        let invert = is_truthy(_py, obj_from_bits(invert_bits));
        let iter_bits = molt_iter(names_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut out_bits: Vec<u64> = Vec::new();
        loop {
            let (item_bits, done) = match iter_next_pair(_py, iter_bits) {
                Ok(value) => value,
                Err(bits) => {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return bits;
                }
            };
            if done {
                break;
            }
            if let Some(pat) = &pat_str {
                let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected str item");
                };
                let name_norm = fnmatch_normcase_text(&name);
                let pat_norm = fnmatch_normcase_text(pat);
                let matched = fnmatch_match_impl(&name_norm, &pat_norm);
                if matched != invert {
                    inc_ref_bits(_py, item_bits);
                    out_bits.push(item_bits);
                }
            } else if let Some(pat) = &pat_bytes {
                let Some(name) = fnmatch_bytes_from_bits(item_bits) else {
                    if string_obj_to_owned(obj_from_bits(item_bits)).is_some() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot use a string pattern on a bytes-like object",
                        );
                    }
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected bytes item");
                };
                let name_norm = fnmatch_normcase_bytes(&name);
                let pat_norm = fnmatch_normcase_bytes(pat);
                let matched = fnmatch_match_bytes_impl(&name_norm, &pat_norm);
                if matched != invert {
                    let ptr = alloc_bytes(_py, &name);
                    if ptr.is_null() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    out_bits.push(MoltObject::from_ptr(ptr).bits());
                }
            }
        }
        let list_ptr = alloc_list_with_capacity(_py, out_bits.as_slice(), out_bits.len());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_translate(pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "expected str pattern");
        };
        let out = fnmatch_translate_impl(&pat);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

fn bisect_normalize_bounds(
    _py: &crate::PyToken<'_>,
    seq_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
) -> Result<(i64, i64), u64> {
    let lo_err = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(_py, obj_from_bits(lo_bits))
    );
    let Some(lo) = index_i64_with_overflow(_py, lo_bits, lo_err.as_str(), None) else {
        return Err(MoltObject::none().bits());
    };
    if lo < 0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "lo must be non-negative",
        ));
    }

    let seq_len_bits = crate::molt_len(seq_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(seq_len) = to_i64(obj_from_bits(seq_len_bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "object has no usable length for bisect",
        ));
    };
    if !obj_from_bits(seq_len_bits).is_none() {
        dec_ref_bits(_py, seq_len_bits);
    }

    let hi = if obj_from_bits(hi_bits).is_none() {
        seq_len
    } else {
        let hi_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(hi_bits))
        );
        let Some(value) = index_i64_with_overflow(_py, hi_bits, hi_err.as_str(), None) else {
            return Err(MoltObject::none().bits());
        };
        value
    };
    Ok((lo, hi))
}

fn bisect_find_index(
    _py: &crate::PyToken<'_>,
    seq_bits: u64,
    x_bits: u64,
    mut lo: i64,
    mut hi: i64,
    key_bits: u64,
    left: bool,
) -> Result<i64, u64> {
    while lo < hi {
        let mid = (lo + hi) / 2;
        let mid_bits = MoltObject::from_int(mid).bits();
        let item_bits = molt_getitem_method(seq_bits, mid_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }

        let mut key_result_bits = item_bits;
        let mut release_key = false;
        if !obj_from_bits(key_bits).is_none() {
            key_result_bits = unsafe { call_callable1(_py, key_bits, item_bits) };
            if exception_pending(_py) {
                if !obj_from_bits(item_bits).is_none() {
                    dec_ref_bits(_py, item_bits);
                }
                return Err(MoltObject::none().bits());
            }
            release_key = true;
        }

        let lt_bits = if left {
            crate::molt_lt(key_result_bits, x_bits)
        } else {
            crate::molt_lt(x_bits, key_result_bits)

