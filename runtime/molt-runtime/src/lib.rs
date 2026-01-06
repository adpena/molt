//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use crossbeam_deque::{Injector, Stealer, Worker};
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,
    pub ref_count: u32,
    pub poll_fn: u64, // Function pointer for polling
    pub state: i64,   // State machine state
    pub size: usize,  // Total size of allocation
}

struct DataclassDesc {
    name: String,
    field_names: Vec<String>,
    frozen: bool,
    eq: bool,
    repr: bool,
    slots: bool,
    class_bits: u64,
}

struct Buffer2D {
    rows: usize,
    cols: usize,
    data: Vec<i64>,
}

#[repr(C)]
struct MemoryView {
    owner_bits: u64,
    offset: isize,
    len: usize,
    itemsize: usize,
    stride: isize,
    readonly: u8,
    _pad: [u8; 7],
}

struct MoltFileHandle {
    file: Mutex<Option<std::fs::File>>,
    readable: bool,
    writable: bool,
    text: bool,
}

struct Utf8IndexCache {
    block: usize,
    prefix: Vec<i64>,
}

struct Utf8CacheStore {
    entries: HashMap<usize, Arc<Utf8IndexCache>>,
    order: VecDeque<usize>,
}

impl Utf8CacheStore {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: usize) -> Option<Arc<Utf8IndexCache>> {
        self.entries.get(&key).cloned()
    }

    fn insert(&mut self, key: usize, cache: Arc<Utf8IndexCache>) {
        if self.entries.contains_key(&key) {
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > UTF8_CACHE_MAX_ENTRIES {
            if let Some(evict) = self.order.pop_front() {
                self.entries.remove(&evict);
            } else {
                break;
            }
        }
    }

    fn remove(&mut self, key: usize) {
        self.entries.remove(&key);
        self.order.retain(|entry| *entry != key);
    }
}

const TYPE_ID_STRING: u32 = 200;
const TYPE_ID_OBJECT: u32 = 100;
const TYPE_ID_LIST: u32 = 201;
const TYPE_ID_BYTES: u32 = 202;
const TYPE_ID_LIST_BUILDER: u32 = 203;
const TYPE_ID_DICT: u32 = 204;
const TYPE_ID_DICT_BUILDER: u32 = 205;
const TYPE_ID_TUPLE: u32 = 206;
const TYPE_ID_DICT_KEYS_VIEW: u32 = 207;
const TYPE_ID_DICT_VALUES_VIEW: u32 = 208;
const TYPE_ID_DICT_ITEMS_VIEW: u32 = 209;
const TYPE_ID_ITER: u32 = 210;
const TYPE_ID_BYTEARRAY: u32 = 211;
const TYPE_ID_RANGE: u32 = 212;
const TYPE_ID_SLICE: u32 = 213;
const TYPE_ID_EXCEPTION: u32 = 214;
const TYPE_ID_DATACLASS: u32 = 215;
const TYPE_ID_BUFFER2D: u32 = 216;
const TYPE_ID_CONTEXT_MANAGER: u32 = 217;
const TYPE_ID_FILE_HANDLE: u32 = 218;
const TYPE_ID_MEMORYVIEW: u32 = 219;
const TYPE_ID_INTARRAY: u32 = 220;
const TYPE_ID_FUNCTION: u32 = 221;
const TYPE_ID_BOUND_METHOD: u32 = 222;
const TYPE_ID_MODULE: u32 = 223;
const TYPE_ID_TYPE: u32 = 224;
const TYPE_ID_GENERATOR: u32 = 225;
const TYPE_ID_CLASSMETHOD: u32 = 226;
const TYPE_ID_STATICMETHOD: u32 = 227;
const TYPE_ID_PROPERTY: u32 = 228;
const TYPE_ID_SUPER: u32 = 229;
const MAX_SMALL_LIST: usize = 16;
const GEN_SEND_OFFSET: usize = 0;
const GEN_THROW_OFFSET: usize = 8;
const GEN_CLOSED_OFFSET: usize = 16;
const GEN_EXC_DEPTH_OFFSET: usize = 24;
const GEN_CONTROL_SIZE: usize = 32;
const UTF8_CACHE_BLOCK: usize = 4096;
const UTF8_CACHE_MIN_LEN: usize = 16 * 1024;
const UTF8_CACHE_MAX_ENTRIES: usize = 128;
const TYPE_TAG_ANY: i64 = 0;
const TYPE_TAG_INT: i64 = 1;
const TYPE_TAG_FLOAT: i64 = 2;
const TYPE_TAG_BOOL: i64 = 3;
const TYPE_TAG_NONE: i64 = 4;
const TYPE_TAG_STR: i64 = 5;
const TYPE_TAG_BYTES: i64 = 6;
const TYPE_TAG_BYTEARRAY: i64 = 7;
const TYPE_TAG_LIST: i64 = 8;
const TYPE_TAG_TUPLE: i64 = 9;
const TYPE_TAG_DICT: i64 = 10;
const TYPE_TAG_RANGE: i64 = 11;
const TYPE_TAG_SLICE: i64 = 12;
const TYPE_TAG_DATACLASS: i64 = 13;
const TYPE_TAG_BUFFER2D: i64 = 14;
const TYPE_TAG_MEMORYVIEW: i64 = 15;
const TYPE_TAG_INTARRAY: i64 = 16;
const BUILTIN_TAG_OBJECT: i64 = 100;
const BUILTIN_TAG_TYPE: i64 = 101;

thread_local! {
    static PARSE_ARENA: RefCell<TempArena> = RefCell::new(TempArena::new(8 * 1024));
    static CONTEXT_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static EXCEPTION_STACK: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static GENERATOR_EXCEPTION_STACKS: RefCell<HashMap<usize, Vec<u64>>> =
        RefCell::new(HashMap::new());
    static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
}

static LAST_EXCEPTION: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
static MODULE_CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static RAW_OBJECTS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
static BUILTIN_CLASSES: OnceLock<BuiltinClasses> = OnceLock::new();
static INTERN_BASES_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_MRO_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_GET_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_SET_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_DELETE_NAME: OnceLock<u64> = OnceLock::new();

trait ExceptionSentinel {
    fn exception_sentinel() -> Self;
}

impl ExceptionSentinel for u64 {
    fn exception_sentinel() -> Self {
        MoltObject::none().bits()
    }
}

impl ExceptionSentinel for i64 {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for i32 {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for usize {
    fn exception_sentinel() -> Self {
        0
    }
}

impl ExceptionSentinel for bool {
    fn exception_sentinel() -> Self {
        false
    }
}

impl ExceptionSentinel for *mut u8 {
    fn exception_sentinel() -> Self {
        std::ptr::null_mut()
    }
}

impl ExceptionSentinel for () {
    fn exception_sentinel() -> Self {}
}

impl<T> ExceptionSentinel for Option<T> {
    fn exception_sentinel() -> Self {
        None
    }
}

macro_rules! raise {
    ($kind:expr, $message:expr $(,)?) => {
        return raise_exception($kind, $message)
    };
}

fn obj_from_bits(bits: u64) -> MoltObject {
    MoltObject::from_bits(bits)
}

fn to_i64(obj: MoltObject) -> Option<i64> {
    if obj.is_int() {
        return obj.as_int();
    }
    if obj.is_bool() {
        return Some(if obj.as_bool().unwrap_or(false) { 1 } else { 0 });
    }
    None
}

fn to_f64(obj: MoltObject) -> Option<f64> {
    if let Some(val) = obj.as_float() {
        return Some(val);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    None
}

fn is_truthy(obj: MoltObject) -> bool {
    if obj.is_none() {
        return false;
    }
    if let Some(b) = obj.as_bool() {
        return b;
    }
    if let Some(i) = obj.as_int() {
        return i != 0;
    }
    if let Some(f) = obj.as_float() {
        return f != 0.0;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTES {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTEARRAY {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                return memoryview_len(ptr) > 0;
            }
            if type_id == TYPE_ID_LIST {
                return list_len(ptr) > 0;
            }
            if type_id == TYPE_ID_TUPLE {
                return tuple_len(ptr) > 0;
            }
            if type_id == TYPE_ID_INTARRAY {
                return intarray_len(ptr) > 0;
            }
            if type_id == TYPE_ID_DICT {
                return dict_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return false;
                }
                let buf = &*buf_ptr;
                return buf.rows.saturating_mul(buf.cols) > 0;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                return dict_view_len(ptr) > 0;
            }
            if type_id == TYPE_ID_RANGE {
                let len = range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr));
                return len > 0;
            }
            if type_id == TYPE_ID_ITER {
                return true;
            }
            if type_id == TYPE_ID_GENERATOR {
                return true;
            }
            if type_id == TYPE_ID_SLICE {
                return true;
            }
            if type_id == TYPE_ID_DATACLASS {
                return true;
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return true;
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return true;
            }
        }
    }
    false
}

fn type_name(obj: MoltObject) -> &'static str {
    if obj.is_int() {
        return "int";
    }
    if obj.is_float() {
        return "float";
    }
    if obj.is_bool() {
        return "bool";
    }
    if obj.is_none() {
        return "NoneType";
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => "str",
                TYPE_ID_BYTES => "bytes",
                TYPE_ID_BYTEARRAY => "bytearray",
                TYPE_ID_LIST => "list",
                TYPE_ID_TUPLE => "tuple",
                TYPE_ID_DICT => "dict",
                TYPE_ID_RANGE => "range",
                TYPE_ID_SLICE => "slice",
                TYPE_ID_MEMORYVIEW => "memoryview",
                TYPE_ID_INTARRAY => "intarray",
                TYPE_ID_EXCEPTION => "Exception",
                TYPE_ID_DATACLASS => "dataclass",
                TYPE_ID_BUFFER2D => "buffer2d",
                TYPE_ID_CONTEXT_MANAGER => "context_manager",
                TYPE_ID_FILE_HANDLE => "file",
                TYPE_ID_FUNCTION => "function",
                TYPE_ID_BOUND_METHOD => "method",
                TYPE_ID_MODULE => "module",
                TYPE_ID_TYPE => "type",
                TYPE_ID_GENERATOR => "generator",
                TYPE_ID_CLASSMETHOD => "classmethod",
                TYPE_ID_STATICMETHOD => "staticmethod",
                TYPE_ID_PROPERTY => "property",
                TYPE_ID_SUPER => "super",
                _ => "object",
            };
        }
    }
    "object"
}

fn obj_eq(lhs: MoltObject, rhs: MoltObject) -> bool {
    if let (Some(li), Some(ri)) = (lhs.as_int(), rhs.as_int()) {
        return li == ri;
    }
    if let (Some(lb), Some(rb)) = (lhs.as_bool(), rhs.as_bool()) {
        return lb == rb;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if ltype != rtype {
                if (ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTEARRAY)
                    || (ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTES)
                {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    if l_len != r_len {
                        return false;
                    }
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    return l_bytes == r_bytes;
                }
                return false;
            }
            if ltype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_TUPLE {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_LIST {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_DATACLASS {
                let l_desc = dataclass_desc_ptr(lp);
                let r_desc = dataclass_desc_ptr(rp);
                if l_desc.is_null() || r_desc.is_null() {
                    return false;
                }
                let l_desc = &*l_desc;
                let r_desc = &*r_desc;
                if !l_desc.eq || !r_desc.eq {
                    return lp == rp;
                }
                if l_desc.name != r_desc.name || l_desc.field_names != r_desc.field_names {
                    return false;
                }
                let l_vals = dataclass_fields_ref(lp);
                let r_vals = dataclass_fields_ref(rp);
                if l_vals.len() != r_vals.len() {
                    return false;
                }
                for (l_val, r_val) in l_vals.iter().zip(r_vals.iter()) {
                    if !obj_eq(obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
        }
        return lp == rp;
    }
    false
}

fn inc_ref_bits(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { molt_inc_ref(ptr) };
        return;
    }
    if !obj.is_float() {
        return;
    }
    if is_raw_object(bits) {
        unsafe { molt_inc_ref(bits as *mut u8) };
    }
}

fn dec_ref_bits(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { molt_dec_ref(ptr) };
        return;
    }
    if !obj.is_float() {
        return;
    }
    if is_raw_object(bits) {
        unsafe { molt_dec_ref(bits as *mut u8) };
    }
}

fn pending_bits_i64() -> i64 {
    MoltObject::pending().bits() as i64
}

fn alloc_object(total_size: usize, type_id: u32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        let header = ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count = 1;
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

unsafe fn header_from_obj_ptr(ptr: *mut u8) -> *mut MoltHeader {
    ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader
}

unsafe fn object_type_id(ptr: *mut u8) -> u32 {
    (*header_from_obj_ptr(ptr)).type_id
}

unsafe fn object_payload_size(ptr: *mut u8) -> usize {
    (*header_from_obj_ptr(ptr)).size - std::mem::size_of::<MoltHeader>()
}

unsafe fn instance_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    let payload = object_payload_size(ptr);
    ptr.add(payload - std::mem::size_of::<u64>()) as *mut u64
}

unsafe fn instance_dict_bits(ptr: *mut u8) -> u64 {
    *instance_dict_bits_ptr(ptr)
}

unsafe fn instance_set_dict_bits(ptr: *mut u8, bits: u64) {
    *instance_dict_bits_ptr(ptr) = bits;
}

unsafe fn object_class_bits(ptr: *mut u8) -> u64 {
    (*header_from_obj_ptr(ptr)).state as u64
}

unsafe fn object_set_class_bits(ptr: *mut u8, bits: u64) {
    (*header_from_obj_ptr(ptr)).state = bits as i64;
}

unsafe fn string_len(ptr: *mut u8) -> usize {
    *(ptr as *const usize)
}

unsafe fn string_bytes(ptr: *mut u8) -> *const u8 {
    ptr.add(std::mem::size_of::<usize>())
}

unsafe fn bytes_len(ptr: *mut u8) -> usize {
    string_len(ptr)
}

unsafe fn intarray_len(ptr: *mut u8) -> usize {
    *(ptr as *const usize)
}

unsafe fn intarray_data(ptr: *mut u8) -> *const i64 {
    ptr.add(std::mem::size_of::<usize>()) as *const i64
}

unsafe fn intarray_slice(ptr: *mut u8) -> &'static [i64] {
    std::slice::from_raw_parts(intarray_data(ptr), intarray_len(ptr))
}

unsafe fn bytes_data(ptr: *mut u8) -> *const u8 {
    string_bytes(ptr)
}

unsafe fn memoryview_ptr(ptr: *mut u8) -> *mut MemoryView {
    ptr as *mut MemoryView
}

unsafe fn memoryview_owner_bits(ptr: *mut u8) -> u64 {
    (*memoryview_ptr(ptr)).owner_bits
}

unsafe fn memoryview_offset(ptr: *mut u8) -> isize {
    (*memoryview_ptr(ptr)).offset
}

unsafe fn memoryview_len(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).len
}

unsafe fn memoryview_itemsize(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).itemsize
}

unsafe fn memoryview_stride(ptr: *mut u8) -> isize {
    (*memoryview_ptr(ptr)).stride
}

unsafe fn memoryview_readonly(ptr: *mut u8) -> bool {
    (*memoryview_ptr(ptr)).readonly != 0
}

unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        let len = bytes_len(ptr);
        let data = bytes_data(ptr);
        return Some(std::slice::from_raw_parts(data, len));
    }
    None
}

unsafe fn memoryview_bytes_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    if memoryview_itemsize(ptr) != 1 || memoryview_stride(ptr) != 1 {
        return None;
    }
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner.as_ptr()?;
    let base = bytes_like_slice_raw(owner_ptr)?;
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    let len = memoryview_len(ptr);
    if offset > base.len() || offset + len > base.len() {
        return None;
    }
    Some(&base[offset..offset + len])
}

unsafe fn memoryview_collect_bytes(ptr: *mut u8) -> Option<Vec<u8>> {
    if memoryview_itemsize(ptr) != 1 {
        return None;
    }
    let owner_bits = memoryview_owner_bits(ptr);
    let owner = obj_from_bits(owner_bits);
    let owner_ptr = owner.as_ptr()?;
    let base = bytes_like_slice_raw(owner_ptr)?;
    let offset = memoryview_offset(ptr);
    if offset < 0 {
        return None;
    }
    let len = memoryview_len(ptr);
    let stride = memoryview_stride(ptr);
    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        let start = offset + (idx as isize) * stride;
        if start < 0 {
            return None;
        }
        let start = start as usize;
        if start >= base.len() {
            return None;
        }
        out.push(base[start]);
    }
    Some(out)
}

unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_MEMORYVIEW {
        return memoryview_bytes_slice(ptr);
    }
    bytes_like_slice_raw(ptr)
}

unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

unsafe fn seq_vec(ptr: *mut u8) -> &'static mut Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &*vec_ptr
}

unsafe fn list_len(ptr: *mut u8) -> usize {
    seq_vec_ref(ptr).len()
}

unsafe fn tuple_len(ptr: *mut u8) -> usize {
    seq_vec_ref(ptr).len()
}

unsafe fn dict_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

unsafe fn dict_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>)
}

unsafe fn dict_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    let vec_ptr = dict_order_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn dict_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    let vec_ptr = dict_table_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn dict_len(ptr: *mut u8) -> usize {
    dict_order(ptr).len() / 2
}

unsafe fn dict_view_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn dict_view_len(ptr: *mut u8) -> usize {
    let dict_bits = dict_view_dict_bits(ptr);
    let dict_obj = obj_from_bits(dict_bits);
    if let Some(dict_ptr) = dict_obj.as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            return dict_len(dict_ptr);
        }
    }
    0
}

unsafe fn dict_view_entry(ptr: *mut u8, idx: usize) -> Option<(u64, u64)> {
    let dict_bits = dict_view_dict_bits(ptr);
    let dict_obj = obj_from_bits(dict_bits);
    if let Some(dict_ptr) = dict_obj.as_ptr() {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let order = dict_order(dict_ptr);
        let entry = idx * 2;
        if entry + 1 >= order.len() {
            return None;
        }
        return Some((order[entry], order[entry + 1]));
    }
    None
}

unsafe fn iter_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn iter_index(ptr: *mut u8) -> usize {
    *(ptr.add(std::mem::size_of::<u64>()) as *const usize)
}

unsafe fn iter_set_index(ptr: *mut u8, idx: usize) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
}

unsafe fn range_start(ptr: *mut u8) -> i64 {
    *(ptr as *const i64)
}

unsafe fn range_stop(ptr: *mut u8) -> i64 {
    *(ptr.add(std::mem::size_of::<i64>()) as *const i64)
}

unsafe fn range_step(ptr: *mut u8) -> i64 {
    *(ptr.add(2 * std::mem::size_of::<i64>()) as *const i64)
}

unsafe fn slice_start_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn slice_stop_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn slice_step_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn exception_kind_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn exception_msg_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn exception_cause_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn exception_context_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn exception_suppress_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn context_enter_fn(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn context_exit_fn(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
unsafe fn function_fn_ptr(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

#[allow(dead_code)]
unsafe fn function_arity(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
unsafe fn function_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_set_dict_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn bound_method_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn bound_method_self_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn module_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn module_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn class_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_bases_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_bases_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn class_mro_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_mro_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn classmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn staticmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn property_get_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn property_set_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn property_del_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn super_type_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn super_obj_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn dataclass_desc_ptr(ptr: *mut u8) -> *mut DataclassDesc {
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    (*header_ptr).state as usize as *mut DataclassDesc
}

unsafe fn dataclass_fields_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *const *mut Vec<u64>)
}

unsafe fn dataclass_fields_ref(ptr: *mut u8) -> &'static Vec<u64> {
    &*dataclass_fields_ptr(ptr)
}

unsafe fn dataclass_fields_mut(ptr: *mut u8) -> &'static mut Vec<u64> {
    &mut *dataclass_fields_ptr(ptr)
}

unsafe fn dataclass_dict_bits_ptr(ptr: *mut u8) -> *mut u64 {
    ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut u64
}

unsafe fn dataclass_dict_bits(ptr: *mut u8) -> u64 {
    *dataclass_dict_bits_ptr(ptr)
}

unsafe fn dataclass_set_dict_bits(ptr: *mut u8, bits: u64) {
    *dataclass_dict_bits_ptr(ptr) = bits;
}

unsafe fn buffer2d_ptr(ptr: *mut u8) -> *mut Buffer2D {
    *(ptr as *const *mut Buffer2D)
}

unsafe fn file_handle_ptr(ptr: *mut u8) -> *mut MoltFileHandle {
    *(ptr as *const *mut MoltFileHandle)
}

fn range_len_i64(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            return 0;
        }
        let span = stop - start - 1;
        return 1 + span / step;
    }
    if start <= stop {
        return 0;
    }
    let step_abs = -step;
    let span = start - stop - 1;
    1 + span / step_abs
}

fn mix_hash(mut val: u64) -> u64 {
    val ^= val >> 33;
    val = val.wrapping_mul(0xff51afd7ed558ccd);
    val ^= val >> 33;
    val = val.wrapping_mul(0xc4ceb9fe1a85ec53);
    val ^= val >> 33;
    val
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    mix_hash(hash)
}

fn hash_bits(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = obj.as_int() {
        return mix_hash(i as u64);
    }
    if let Some(b) = obj.as_bool() {
        return mix_hash(if b { 1 } else { 0 });
    }
    if obj.is_none() {
        return mix_hash(0x9e3779b97f4a7c15);
    }
    if let Some(f) = obj.as_float() {
        return mix_hash(f.to_bits());
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return hash_bytes(bytes);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes(bytes);
            }
            if type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut hash = 0x345678u64;
                for &elem in elems {
                    hash = hash.wrapping_mul(0x100000001b3);
                    hash ^= hash_bits(elem);
                }
                hash ^= elems.len() as u64;
                return mix_hash(hash);
            }
        }
        return mix_hash(ptr as u64);
    }
    mix_hash(bits)
}

fn dict_table_capacity(entries: usize) -> usize {
    let mut cap = entries.saturating_mul(2).next_power_of_two();
    if cap < 8 {
        cap = 8;
    }
    cap
}

fn dict_insert_entry(order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx * 2];
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn dict_rebuild(order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(order, table, entry_idx);
    }
}

fn dict_find_entry(order: &[u64], table: &[usize], key_bits: u64) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx * 2];
        if obj_eq(obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

fn alloc_bytes_like_with_len(len: usize, type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<usize>() + len;
    let ptr = alloc_object(total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = len;
    }
    ptr
}

fn alloc_string(bytes: &[u8]) -> *mut u8 {
    let ptr = alloc_bytes_like_with_len(bytes.len(), TYPE_ID_STRING);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

fn alloc_bytes_like(bytes: &[u8], type_id: u32) -> *mut u8 {
    let ptr = alloc_bytes_like_with_len(bytes.len(), type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ptr
}

fn concat_bytes_like(left: &[u8], right: &[u8], type_id: u32) -> Option<u64> {
    let total = left.len().checked_add(right.len())?;
    let ptr = alloc_bytes_like_with_len(total, type_id);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(left.as_ptr(), data_ptr, left.len());
        std::ptr::copy_nonoverlapping(right.as_ptr(), data_ptr.add(left.len()), right.len());
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

fn alloc_bytes(bytes: &[u8]) -> *mut u8 {
    alloc_bytes_like(bytes, TYPE_ID_BYTES)
}

fn alloc_bytearray(bytes: &[u8]) -> *mut u8 {
    alloc_bytes_like(bytes, TYPE_ID_BYTEARRAY)
}

fn alloc_intarray(values: &[i64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<usize>()
        + std::mem::size_of_val(values);
    let ptr = alloc_object(total, TYPE_ID_INTARRAY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let len_ptr = ptr as *mut usize;
        *len_ptr = values.len();
        let data_ptr = ptr.add(std::mem::size_of::<usize>()) as *mut i64;
        std::ptr::copy_nonoverlapping(values.as_ptr(), data_ptr, values.len());
    }
    ptr
}

fn alloc_memoryview(
    owner_bits: u64,
    offset: isize,
    len: usize,
    itemsize: usize,
    stride: isize,
    readonly: bool,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr)._pad = [0; 7];
    }
    inc_ref_bits(owner_bits);
    ptr
}

fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    if pattern.len() == 1 {
        dst.fill(pattern[0]);
        return;
    }
    let mut filled = pattern.len().min(dst.len());
    dst[..filled].copy_from_slice(&pattern[..filled]);
    while filled < dst.len() {
        let copy_len = std::cmp::min(filled, dst.len() - filled);
        let (head, tail) = dst.split_at_mut(filled);
        tail[..copy_len].copy_from_slice(&head[..copy_len]);
        filled += copy_len;
    }
}

unsafe fn dict_set_in_place(ptr: *mut u8, key_bits: u64, val_bits: u64) {
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    if let Some(entry_idx) = dict_find_entry(order, table, key_bits) {
        let val_idx = entry_idx * 2 + 1;
        let old_bits = order[val_idx];
        if old_bits != val_bits {
            dec_ref_bits(old_bits);
            inc_ref_bits(val_bits);
            order[val_idx] = val_bits;
        }
        return;
    }

    let new_entries = (order.len() / 2) + 1;
    let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
    if needs_resize {
        let capacity = dict_table_capacity(new_entries);
        dict_rebuild(order, table, capacity);
    }

    order.push(key_bits);
    order.push(val_bits);
    inc_ref_bits(key_bits);
    inc_ref_bits(val_bits);
    let entry_idx = order.len() / 2 - 1;
    dict_insert_entry(order, table, entry_idx);
}

unsafe fn dict_get_in_place(ptr: *mut u8, key_bits: u64) -> Option<u64> {
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    dict_find_entry(order, table, key_bits).map(|idx| order[idx * 2 + 1])
}

fn alloc_list_with_capacity(elems: &[u64], capacity: usize) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_LIST);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

fn alloc_list(elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_list_with_capacity(elems, cap)
}

fn alloc_tuple_with_capacity(elems: &[u64], capacity: usize) -> *mut u8 {
    let cap = capacity.max(elems.len());
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_TUPLE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let mut vec = Vec::with_capacity(cap);
        vec.extend_from_slice(elems);
        for &elem in elems {
            inc_ref_bits(elem);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

fn alloc_tuple(elems: &[u64]) -> *mut u8 {
    let cap = if elems.len() <= MAX_SMALL_LIST {
        MAX_SMALL_LIST
    } else {
        elems.len()
    };
    alloc_tuple_with_capacity(elems, cap)
}

fn alloc_range(start: i64, stop: i64, step: i64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<i64>();
    let ptr = alloc_object(total, TYPE_ID_RANGE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut i64) = start;
        *(ptr.add(std::mem::size_of::<i64>()) as *mut i64) = stop;
        *(ptr.add(2 * std::mem::size_of::<i64>()) as *mut i64) = step;
    }
    ptr
}

fn alloc_slice_obj(start_bits: u64, stop_bits: u64, step_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_SLICE);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = start_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = stop_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = step_bits;
        inc_ref_bits(start_bits);
        inc_ref_bits(stop_bits);
        inc_ref_bits(step_bits);
    }
    ptr
}

fn alloc_exception(kind: &str, message: &str) -> *mut u8 {
    let kind_ptr = alloc_string(kind.as_bytes());
    if kind_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let msg_ptr = alloc_string(message.as_bytes());
    if msg_ptr.is_null() {
        unsafe { molt_dec_ref(kind_ptr) };
        return std::ptr::null_mut();
    }
    let total = std::mem::size_of::<MoltHeader>() + 5 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_EXCEPTION);
    if ptr.is_null() {
        unsafe {
            molt_dec_ref(kind_ptr);
            molt_dec_ref(msg_ptr);
        }
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = MoltObject::from_ptr(kind_ptr).bits();
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = MoltObject::from_ptr(msg_ptr).bits();
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) =
            MoltObject::from_bool(false).bits();
    }
    ptr
}

fn alloc_exception_obj(kind_bits: u64, msg_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 5 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_EXCEPTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = kind_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = msg_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) =
            MoltObject::from_bool(false).bits();
        inc_ref_bits(kind_bits);
        inc_ref_bits(msg_bits);
        inc_ref_bits(MoltObject::none().bits());
        inc_ref_bits(MoltObject::none().bits());
        inc_ref_bits(MoltObject::from_bool(false).bits());
    }
    ptr
}

fn alloc_context_manager(enter_fn: u64, exit_fn: u64, payload_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CONTEXT_MANAGER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = enter_fn;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = exit_fn;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = payload_bits;
        inc_ref_bits(payload_bits);
    }
    ptr
}

fn alloc_function_obj(fn_ptr: u64, arity: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_FUNCTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = arity;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = 0;
    }
    ptr
}

fn alloc_bound_method_obj(func_bits: u64, self_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_BOUND_METHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = self_bits;
        inc_ref_bits(func_bits);
        inc_ref_bits(self_bits);
    }
    ptr
}

fn alloc_module_obj(name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(&[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_MODULE);
    if ptr.is_null() {
        dec_ref_bits(dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        inc_ref_bits(name_bits);
        inc_ref_bits(dict_bits);
    }
    ptr
}

fn alloc_class_obj(name_bits: u64) -> *mut u8 {
    let dict_ptr = alloc_dict_with_pairs(&[]);
    if dict_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let bases_bits = MoltObject::none().bits();
    let mro_bits = MoltObject::none().bits();
    let total = std::mem::size_of::<MoltHeader>() + 4 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_TYPE);
    if ptr.is_null() {
        dec_ref_bits(dict_bits);
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = name_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = dict_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bases_bits;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = mro_bits;
        inc_ref_bits(name_bits);
        inc_ref_bits(dict_bits);
        inc_ref_bits(bases_bits);
        inc_ref_bits(mro_bits);
    }
    ptr
}

fn alloc_classmethod_obj(func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CLASSMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(func_bits);
    }
    ptr
}

fn alloc_staticmethod_obj(func_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_STATICMETHOD);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = func_bits;
        inc_ref_bits(func_bits);
    }
    ptr
}

fn alloc_property_obj(get_bits: u64, set_bits: u64, del_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_PROPERTY);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = get_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = set_bits;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = del_bits;
        inc_ref_bits(get_bits);
        inc_ref_bits(set_bits);
        inc_ref_bits(del_bits);
    }
    ptr
}

fn alloc_super_obj(type_bits: u64, obj_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_SUPER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = type_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = obj_bits;
        inc_ref_bits(type_bits);
        inc_ref_bits(obj_bits);
    }
    ptr
}

fn alloc_file_handle(file: std::fs::File, readable: bool, writable: bool, text: bool) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut MoltFileHandle>();
    let ptr = alloc_object(total, TYPE_ID_FILE_HANDLE);
    if ptr.is_null() {
        return ptr;
    }
    let handle = Box::new(MoltFileHandle {
        file: Mutex::new(Some(file)),
        readable,
        writable,
        text,
    });
    let handle_ptr = Box::into_raw(handle);
    unsafe {
        *(ptr as *mut *mut MoltFileHandle) = handle_ptr;
    }
    ptr
}

extern "C" fn context_null_enter(payload_bits: u64) -> u64 {
    inc_ref_bits(payload_bits);
    payload_bits
}

extern "C" fn context_null_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

extern "C" fn context_closing_enter(payload_bits: u64) -> u64 {
    inc_ref_bits(payload_bits);
    payload_bits
}

extern "C" fn context_closing_exit(payload_bits: u64, _exc_bits: u64) -> u64 {
    close_payload(payload_bits);
    MoltObject::from_bool(false).bits()
}

fn context_stack_push(ctx_bits: u64) {
    CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().push(ctx_bits);
    });
    inc_ref_bits(ctx_bits);
}

fn context_stack_pop(expected_bits: u64) {
    let result = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(bits) = stack.pop() else {
            return Err("context manager stack underflow");
        };
        if bits != expected_bits {
            return Err("context manager stack mismatch");
        }
        Ok(bits)
    });
    match result {
        Ok(bits) => dec_ref_bits(bits),
        Err(msg) => raise!("RuntimeError", msg),
    }
}

unsafe fn context_exit_unchecked(ctx_bits: u64, exc_bits: u64) {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        return;
    };
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_CONTEXT_MANAGER {
        let exit_fn_addr = context_exit_fn(ptr);
        if exit_fn_addr == 0 {
            return;
        }
        let exit_fn =
            std::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(exit_fn_addr as usize);
        exit_fn(context_payload_bits(ptr), exc_bits);
        return;
    }
    if type_id == TYPE_ID_FILE_HANDLE {
        file_handle_exit(ptr, exc_bits);
    }
}

fn context_stack_depth() -> usize {
    CONTEXT_STACK.with(|stack| stack.borrow().len())
}

fn context_stack_unwind_to(depth: usize, exc_bits: u64) {
    let contexts = CONTEXT_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if depth > stack.len() {
            return Err("context manager stack underflow");
        }
        let tail = stack.split_off(depth);
        Ok(tail)
    });
    match contexts {
        Ok(contexts) => {
            for bits in contexts.into_iter().rev() {
                unsafe { context_exit_unchecked(bits, exc_bits) };
                dec_ref_bits(bits);
            }
        }
        Err(msg) => raise!("RuntimeError", msg),
    }
}

fn context_stack_unwind(exc_bits: u64) {
    context_stack_unwind_to(0, exc_bits);
}

fn file_handle_close_ptr(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return false;
        }
        let handle = &*handle_ptr;
        let mut guard = handle.file.lock().unwrap();
        guard.take().is_some()
    }
}

unsafe fn file_handle_enter(ptr: *mut u8) -> u64 {
    let bits = MoltObject::from_ptr(ptr).bits();
    inc_ref_bits(bits);
    bits
}

unsafe fn file_handle_exit(ptr: *mut u8, _exc_bits: u64) -> u64 {
    file_handle_close_ptr(ptr);
    MoltObject::from_bool(false).bits()
}

fn close_payload(payload_bits: u64) {
    let payload = obj_from_bits(payload_bits);
    let Some(ptr) = payload.as_ptr() else {
        raise!("AttributeError", "object has no attribute 'close'");
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
            file_handle_close_ptr(ptr);
            return;
        }
    }
    raise!("AttributeError", "object has no attribute 'close'");
}

fn record_exception(ptr: *mut u8) {
    let cell = LAST_EXCEPTION.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    let mut context_bits: Option<u64> = None;
    if let Some(old_ptr) = guard.take() {
        let old_bits = MoltObject::from_ptr(old_ptr as *mut u8).bits();
        if old_ptr as *mut u8 != ptr {
            context_bits = Some(old_bits);
        }
        dec_ref_bits(old_bits);
    }
    if context_bits.is_none() {
        context_bits = exception_context_active_bits();
    }
    if let Some(ctx_bits) = context_bits {
        let new_bits = MoltObject::from_ptr(ptr).bits();
        if ctx_bits != new_bits {
            let existing = unsafe { exception_context_bits(ptr) };
            if obj_from_bits(existing).is_none() {
                unsafe {
                    inc_ref_bits(ctx_bits);
                    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = ctx_bits;
                }
            }
        }
    }
    *guard = Some(ptr as usize);
    let new_bits = MoltObject::from_ptr(ptr).bits();
    inc_ref_bits(new_bits);
}

fn clear_exception() {
    let cell = LAST_EXCEPTION.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if let Some(old_ptr) = guard.take() {
        let old_bits = MoltObject::from_ptr(old_ptr as *mut u8).bits();
        dec_ref_bits(old_bits);
    }
}

fn exception_pending() -> bool {
    let cell = LAST_EXCEPTION.get_or_init(|| Mutex::new(None));
    let guard = cell.lock().unwrap();
    guard.is_some()
}

fn exception_handler_active() -> bool {
    EXCEPTION_STACK.with(|stack| !stack.borrow().is_empty())
}

fn exception_context_active_bits() -> Option<u64> {
    let active = ACTIVE_EXCEPTION_STACK.with(|stack| {
        let stack = stack.borrow();
        stack.iter().rev().find_map(|bits| {
            if obj_from_bits(*bits).is_none() {
                None
            } else {
                Some(*bits)
            }
        })
    });
    if active.is_some() {
        return active;
    }
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        let stack = stack.borrow();
        stack.iter().rev().find_map(|bits| {
            if obj_from_bits(*bits).is_none() {
                None
            } else {
                Some(*bits)
            }
        })
    })
}

fn exception_context_set(bits: u64) {
    if obj_from_bits(bits).is_none() {
        return;
    }
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(slot) = stack.last_mut() else {
            return;
        };
        if *slot == bits {
            return;
        }
        if !obj_from_bits(*slot).is_none() {
            dec_ref_bits(*slot);
        }
        inc_ref_bits(bits);
        *slot = bits;
    });
}

fn exception_context_align_depth(target: usize) {
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        while stack.len() > target {
            if let Some(bits) = stack.pop() {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(bits);
                }
            }
        }
        while stack.len() < target {
            stack.push(MoltObject::none().bits());
        }
    });
}

fn exception_context_fallback_push(bits: u64) {
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        stack.borrow_mut().push(bits);
    });
}

fn exception_context_fallback_pop() {
    ACTIVE_EXCEPTION_FALLBACK.with(|stack| {
        let _ = stack.borrow_mut().pop();
    });
}

fn generator_raise_active() -> bool {
    GENERATOR_RAISE.with(|flag| flag.get())
}

fn set_generator_raise(active: bool) {
    GENERATOR_RAISE.with(|flag| flag.set(active));
}

fn exception_stack_push() {
    EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(0);
    });
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        stack.borrow_mut().push(MoltObject::none().bits());
    });
}

fn exception_stack_pop() {
    let underflow = EXCEPTION_STACK.with(|stack| stack.borrow_mut().pop().is_none());
    if underflow {
        raise!("RuntimeError", "exception handler stack underflow");
    }
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(bits) = stack.pop() {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(bits);
            }
        }
    });
}

fn exception_stack_depth() -> usize {
    EXCEPTION_STACK.with(|stack| stack.borrow().len())
}

fn exception_stack_set_depth(target: usize) {
    EXCEPTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        while stack.len() > target {
            stack.pop();
        }
        while stack.len() < target {
            stack.push(1);
        }
    });
    exception_context_align_depth(target);
}

fn generator_exception_stack_take(ptr: *mut u8) -> Vec<u64> {
    GENERATOR_EXCEPTION_STACKS
        .with(|map| map.borrow_mut().remove(&(ptr as usize)).unwrap_or_default())
}

fn generator_exception_stack_store(ptr: *mut u8, stack: Vec<u64>) {
    GENERATOR_EXCEPTION_STACKS.with(|map| {
        map.borrow_mut().insert(ptr as usize, stack);
    });
}

fn generator_exception_stack_drop(ptr: *mut u8) {
    GENERATOR_EXCEPTION_STACKS.with(|map| {
        if let Some(stack) = map.borrow_mut().remove(&(ptr as usize)) {
            for bits in stack {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(bits);
                }
            }
        }
    });
}

fn raise_exception<T: ExceptionSentinel>(kind: &str, message: &str) -> T {
    let ptr = alloc_exception(kind, message);
    if !ptr.is_null() {
        record_exception(ptr);
    }
    if !exception_handler_active() {
        let exc_bits = if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        };
        context_stack_unwind(exc_bits);
        eprintln!("{kind}: {message}");
        std::process::exit(1);
    }
    T::exception_sentinel()
}

fn module_cache() -> &'static Mutex<HashMap<String, u64>> {
    MODULE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn raw_objects() -> &'static Mutex<HashSet<usize>> {
    RAW_OBJECTS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn register_raw_object(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    raw_objects().lock().unwrap().insert(ptr as usize);
}

fn unregister_raw_object(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    raw_objects().lock().unwrap().remove(&(ptr as usize));
}

fn is_raw_object(bits: u64) -> bool {
    raw_objects().lock().unwrap().contains(&(bits as usize))
}

fn intern_static_name(slot: &OnceLock<u64>, name: &'static [u8]) -> u64 {
    *slot.get_or_init(|| {
        let ptr = alloc_string(name);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

struct BuiltinClasses {
    object: u64,
    type_obj: u64,
    none_type: u64,
    int: u64,
    float: u64,
    bool: u64,
    str: u64,
    bytes: u64,
    bytearray: u64,
    list: u64,
    tuple: u64,
    dict: u64,
    range: u64,
    slice: u64,
    memoryview: u64,
    function: u64,
    module: u64,
    super_type: u64,
}

fn make_builtin_class(name: &str) -> u64 {
    let name_ptr = alloc_string(name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(name_bits);
    dec_ref_bits(name_bits);
    if class_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(class_ptr).bits()
}

fn builtin_classes() -> &'static BuiltinClasses {
    BUILTIN_CLASSES.get_or_init(|| {
        let object = make_builtin_class("object");
        let type_obj = make_builtin_class("type");
        let none_type = make_builtin_class("NoneType");
        let int = make_builtin_class("int");
        let float = make_builtin_class("float");
        let bool = make_builtin_class("bool");
        let str = make_builtin_class("str");
        let bytes = make_builtin_class("bytes");
        let bytearray = make_builtin_class("bytearray");
        let list = make_builtin_class("list");
        let tuple = make_builtin_class("tuple");
        let dict = make_builtin_class("dict");
        let range = make_builtin_class("range");
        let slice = make_builtin_class("slice");
        let memoryview = make_builtin_class("memoryview");
        let function = make_builtin_class("function");
        let module = make_builtin_class("module");
        let super_type = make_builtin_class("super");

        let _ = molt_class_set_base(object, MoltObject::none().bits());
        let _ = molt_class_set_base(type_obj, object);
        let _ = molt_class_set_base(none_type, object);
        let _ = molt_class_set_base(int, object);
        let _ = molt_class_set_base(float, object);
        let _ = molt_class_set_base(bool, int);
        let _ = molt_class_set_base(str, object);
        let _ = molt_class_set_base(bytes, object);
        let _ = molt_class_set_base(bytearray, object);
        let _ = molt_class_set_base(list, object);
        let _ = molt_class_set_base(tuple, object);
        let _ = molt_class_set_base(dict, object);
        let _ = molt_class_set_base(range, object);
        let _ = molt_class_set_base(slice, object);
        let _ = molt_class_set_base(memoryview, object);
        let _ = molt_class_set_base(function, object);
        let _ = molt_class_set_base(module, object);
        let _ = molt_class_set_base(super_type, object);

        BuiltinClasses {
            object,
            type_obj,
            none_type,
            int,
            float,
            bool,
            str,
            bytes,
            bytearray,
            list,
            tuple,
            dict,
            range,
            slice,
            memoryview,
            function,
            module,
            super_type,
        }
    })
}

fn builtin_type_bits(tag: i64) -> Option<u64> {
    let builtins = builtin_classes();
    match tag {
        TYPE_TAG_INT => Some(builtins.int),
        TYPE_TAG_FLOAT => Some(builtins.float),
        TYPE_TAG_BOOL => Some(builtins.bool),
        TYPE_TAG_NONE => Some(builtins.none_type),
        TYPE_TAG_STR => Some(builtins.str),
        TYPE_TAG_BYTES => Some(builtins.bytes),
        TYPE_TAG_BYTEARRAY => Some(builtins.bytearray),
        TYPE_TAG_LIST => Some(builtins.list),
        TYPE_TAG_TUPLE => Some(builtins.tuple),
        TYPE_TAG_DICT => Some(builtins.dict),
        TYPE_TAG_RANGE => Some(builtins.range),
        TYPE_TAG_SLICE => Some(builtins.slice),
        TYPE_TAG_MEMORYVIEW => Some(builtins.memoryview),
        BUILTIN_TAG_OBJECT => Some(builtins.object),
        BUILTIN_TAG_TYPE => Some(builtins.type_obj),
        _ => None,
    }
}

fn is_builtin_class_bits(bits: u64) -> bool {
    let builtins = builtin_classes();
    bits == builtins.object
        || bits == builtins.type_obj
        || bits == builtins.none_type
        || bits == builtins.int
        || bits == builtins.float
        || bits == builtins.bool
        || bits == builtins.str
        || bits == builtins.bytes
        || bits == builtins.bytearray
        || bits == builtins.list
        || bits == builtins.tuple
        || bits == builtins.dict
        || bits == builtins.range
        || bits == builtins.slice
        || bits == builtins.memoryview
        || bits == builtins.function
        || bits == builtins.module
        || bits == builtins.super_type
}

fn class_name_for_error(class_bits: u64) -> String {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return "<class>".to_string();
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return "<class>".to_string();
        }
        string_obj_to_owned(obj_from_bits(class_name_bits(ptr)))
            .unwrap_or_else(|| "<class>".to_string())
    }
}

unsafe fn class_mro_ref(class_ptr: *mut u8) -> Option<&'static Vec<u64>> {
    let mro_bits = class_mro_bits(class_ptr);
    let mro_obj = obj_from_bits(mro_bits);
    let mro_ptr = mro_obj.as_ptr()?;
    if object_type_id(mro_ptr) != TYPE_ID_TUPLE {
        return None;
    }
    Some(seq_vec_ref(mro_ptr))
}

fn class_mro_vec(class_bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return vec![class_bits];
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return vec![class_bits];
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.clone();
        }
        let mut out = vec![class_bits];
        let bases_bits = class_bases_bits(ptr);
        let bases = class_bases_vec(bases_bits);
        for base in bases {
            out.extend(class_mro_vec(base));
        }
        out
    }
}

fn class_bases_vec(bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() || bits == 0 {
        return Vec::new();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => return vec![bits],
                TYPE_ID_TUPLE => return seq_vec_ref(ptr).clone(),
                _ => {}
            }
        }
    }
    Vec::new()
}

fn type_of_bits(val_bits: u64) -> u64 {
    let builtins = builtin_classes();
    let obj = obj_from_bits(val_bits);
    if obj.is_none() {
        return builtins.none_type;
    }
    if obj.is_bool() {
        return builtins.bool;
    }
    if obj.is_int() {
        return builtins.int;
    }
    if is_raw_object(val_bits) {
        let ptr = val_bits as *mut u8;
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
        return builtins.object;
    }
    if obj.is_float() {
        return builtins.float;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => builtins.str,
                TYPE_ID_BYTES => builtins.bytes,
                TYPE_ID_BYTEARRAY => builtins.bytearray,
                TYPE_ID_LIST => builtins.list,
                TYPE_ID_TUPLE => builtins.tuple,
                TYPE_ID_DICT => builtins.dict,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_FUNCTION => builtins.function,
                TYPE_ID_MODULE => builtins.module,
                TYPE_ID_TYPE => builtins.type_obj,
                TYPE_ID_SUPER => builtins.super_type,
                TYPE_ID_DATACLASS => {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() {
                        let class_bits = (*desc_ptr).class_bits;
                        if class_bits != 0 {
                            return class_bits;
                        }
                    }
                    builtins.object
                }
                TYPE_ID_OBJECT => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.object
                    }
                }
                _ => builtins.object,
            };
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
    }
    builtins.object
}

fn collect_classinfo_isinstance(class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!(
            "TypeError",
            "isinstance() arg 2 must be a type or tuple of types"
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_isinstance(*item, out);
                }
            }
            _ => raise!(
                "TypeError",
                "isinstance() arg 2 must be a type or tuple of types"
            ),
        }
    }
}

fn collect_classinfo_issubclass(class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!(
            "TypeError",
            "issubclass() arg 2 must be a class or tuple of classes"
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_issubclass(*item, out);
                }
            }
            _ => raise!(
                "TypeError",
                "issubclass() arg 2 must be a class or tuple of classes"
            ),
        }
    }
}

fn issubclass_bits(sub_bits: u64, class_bits: u64) -> bool {
    if sub_bits == class_bits {
        return true;
    }
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return false;
        }
        if let Some(mro) = class_mro_ref(ptr) {
            return mro.iter().any(|bit| *bit == class_bits);
        }
    }
    class_mro_vec(sub_bits).iter().any(|bit| *bit == class_bits)
}

fn isinstance_bits(val_bits: u64, class_bits: u64) -> bool {
    let mut classes = Vec::new();
    collect_classinfo_isinstance(class_bits, &mut classes);
    let val_type = type_of_bits(val_bits);
    for class_bits in classes {
        if issubclass_bits(val_type, class_bits) {
            return true;
        }
    }
    false
}

fn alloc_dict_with_pairs(pairs: &[u64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(pairs.len());
        let table = Vec::new();
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(ptr, pair[0], pair[1]);
            }
        }
    }
    ptr
}

#[no_mangle]
pub extern "C" fn molt_alloc(size: usize) -> *mut u8 {
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        let header = ptr as *mut MoltHeader;
        (*header).type_id = TYPE_ID_OBJECT;
        (*header).ref_count = 1;
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        let obj_ptr = ptr.add(std::mem::size_of::<MoltHeader>());
        register_raw_object(obj_ptr);
        obj_ptr
    }
}

// --- List Builder ---

#[no_mangle]
pub extern "C" fn molt_list_builder_new(capacity_hint: usize) -> *mut u8 {
    // Allocate wrapper object
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>(); // Store pointer to Vec
    let ptr = alloc_object(total, TYPE_ID_LIST_BUILDER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let vec = Box::new(Vec::<u64>::with_capacity(capacity_hint));
        let vec_ptr = Box::into_raw(vec);
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_ptr` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_append(builder_ptr: *mut u8, val: u64) {
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_ptr` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_ptr: *mut u8) -> u64 {
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    // Reconstruct Box to drop it later, but we need the data
    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let list_ptr = alloc_list_with_capacity(slice, capacity);

    // Builder object will be cleaned up by GC/Ref counting eventually,
    // but the Vec heap allocation is owned by the Box we just reconstructed.
    // So dropping 'vec' here frees the temporary buffer. Correct.

    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_ptr` is valid and points to a tuple builder.
pub unsafe extern "C" fn molt_tuple_builder_finish(builder_ptr: *mut u8) -> u64 {
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let tuple_ptr = alloc_tuple_with_capacity(slice, capacity);

    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_range_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    let start = match to_i64(obj_from_bits(start_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    let stop = match to_i64(obj_from_bits(stop_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    let step = match to_i64(obj_from_bits(step_bits)) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
    };
    if step == 0 {
        raise!("ValueError", "range() arg 3 must not be zero");
    }
    let ptr = alloc_range(start, stop, step);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    let ptr = alloc_slice_obj(start_bits, stop_bits, step_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dataclass_new(
    name_bits: u64,
    field_names_bits: u64,
    values_bits: u64,
    flags_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let name = match string_obj_to_owned(name_obj) {
        Some(val) => val,
        None => raise!("TypeError", "dataclass name must be a str"),
    };
    let field_names_obj = obj_from_bits(field_names_bits);
    let field_names = match decode_string_list(field_names_obj) {
        Some(val) => val,
        None => raise!(
            "TypeError",
            "dataclass field names must be a list/tuple of str",
        ),
    };
    let values_obj = obj_from_bits(values_bits);
    let values = match decode_value_list(values_obj) {
        Some(val) => val,
        None => raise!("TypeError", "dataclass values must be a list/tuple"),
    };
    if field_names.len() != values.len() {
        raise!("TypeError", "dataclass constructor argument mismatch");
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
    let frozen = (flags & 0x1) != 0;
    let eq = (flags & 0x2) != 0;
    let repr = (flags & 0x4) != 0;
    let slots = (flags & 0x8) != 0;
    let desc = Box::new(DataclassDesc {
        name,
        field_names,
        frozen,
        eq,
        repr,
        slots,
        class_bits: 0,
    });
    let desc_ptr = Box::into_raw(desc);

    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_DATACLASS);
    if ptr.is_null() {
        unsafe { drop(Box::from_raw(desc_ptr)) };
        return MoltObject::none().bits();
    }
    unsafe {
        let mut vec = Vec::with_capacity(values.len());
        vec.extend_from_slice(&values);
        for &val in values.iter() {
            inc_ref_bits(val);
        }
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        dataclass_set_dict_bits(ptr, 0);
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        (*header_ptr).state = desc_ptr as i64;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let idx = match obj_from_bits(index_bits).as_int() {
        Some(val) => val,
        None => raise!("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let fields = dataclass_fields_ref(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                raise!("TypeError", "dataclass field index out of range");
            }
            let val = fields[idx as usize];
            inc_ref_bits(val);
            return val;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let idx = match obj_from_bits(index_bits).as_int() {
        Some(val) => val,
        None => raise!("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let desc_ptr = dataclass_desc_ptr(ptr);
            if !desc_ptr.is_null() && (*desc_ptr).frozen {
                raise!("TypeError", "cannot assign to frozen dataclass field");
            }
            let fields = dataclass_fields_mut(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                raise!("TypeError", "dataclass field index out of range");
            }
            let old_bits = fields[idx as usize];
            if old_bits != val_bits {
                dec_ref_bits(old_bits);
                inc_ref_bits(val_bits);
                fields[idx as usize] = val_bits;
            }
            return obj_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dataclass expects object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DATACLASS {
            raise!("TypeError", "dataclass expects object");
        }
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                raise!("TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                raise!("TypeError", "class must be a type object");
            }
        }
        let desc_ptr = dataclass_desc_ptr(ptr);
        if !desc_ptr.is_null() {
            let old_bits = (*desc_ptr).class_bits;
            if old_bits != 0 {
                dec_ref_bits(old_bits);
            }
            (*desc_ptr).class_bits = class_bits;
            if class_bits != 0 {
                inc_ref_bits(class_bits);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_func_new(fn_ptr: u64, arity: u64) -> u64 {
    let ptr = alloc_function_obj(fn_ptr, arity);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "bound method expects function object");
    };
    unsafe {
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            raise!("TypeError", "bound method expects function object");
        }
    }
    let ptr = alloc_bound_method_obj(func_bits, self_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_module_new(name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "module name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "module name must be str");
        }
    }
    let ptr = alloc_module_obj(name_bits);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let dict_bits = module_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let key_ptr = alloc_string(b"__name__");
                if !key_ptr.is_null() {
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    dict_set_in_place(dict_ptr, key_bits, name_bits);
                    dec_ref_bits(key_bits);
                }
            }
        }
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_class_new(name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "class name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "class name must be str");
        }
    }
    let ptr = alloc_class_obj(name_bits);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_builtin_type(tag_bits: u64) -> u64 {
    let tag = match to_i64(obj_from_bits(tag_bits)) {
        Some(val) => val,
        None => raise!("TypeError", "builtin type tag must be int"),
    };
    let Some(bits) = builtin_type_bits(tag) else {
        raise!("TypeError", "unknown builtin type tag");
    };
    inc_ref_bits(bits);
    bits
}

#[no_mangle]
pub extern "C" fn molt_type_of(val_bits: u64) -> u64 {
    let bits = type_of_bits(val_bits);
    inc_ref_bits(bits);
    bits
}

#[no_mangle]
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    MoltObject::from_bool(isinstance_bits(val_bits, class_bits)).bits()
}

#[no_mangle]
pub extern "C" fn molt_issubclass(sub_bits: u64, class_bits: u64) -> u64 {
    let obj = obj_from_bits(sub_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "issubclass() arg 1 must be a class");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "issubclass() arg 1 must be a class");
        }
    }
    let mut classes = Vec::new();
    collect_classinfo_issubclass(class_bits, &mut classes);
    for class_bits in classes {
        if issubclass_bits(sub_bits, class_bits) {
            return MoltObject::from_bool(true).bits();
        }
    }
    MoltObject::from_bool(false).bits()
}

#[no_mangle]
pub extern "C" fn molt_object_new() -> u64 {
    let obj_ptr = molt_alloc(std::mem::size_of::<u64>());
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
    let class_bits = builtin_classes().object;
    unsafe {
        let _ = molt_object_set_class(obj_ptr, class_bits);
    }
    obj_bits
}

fn c3_merge(mut seqs: Vec<Vec<u64>>) -> Option<Vec<u64>> {
    let mut result = Vec::new();
    loop {
        seqs.retain(|seq| !seq.is_empty());
        if seqs.is_empty() {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for seq in &seqs {
            let head = seq[0];
            let mut in_tail = false;
            for other in &seqs {
                if other.iter().skip(1).any(|val| *val == head) {
                    in_tail = true;
                    break;
                }
            }
            if !in_tail {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for seq in &mut seqs {
            if !seq.is_empty() && seq[0] == cand {
                seq.remove(0);
            }
        }
    }
}

fn compute_mro(class_bits: u64, bases: &[u64]) -> Option<Vec<u64>> {
    let mut seqs = Vec::with_capacity(bases.len() + 1);
    for base in bases {
        seqs.push(class_mro_vec(*base));
    }
    seqs.push(bases.to_vec());
    let mut out = vec![class_bits];
    let merged = c3_merge(seqs)?;
    out.extend(merged);
    Some(out)
}

#[no_mangle]
pub extern "C" fn molt_class_set_base(class_bits: u64, base_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        raise!("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "class must be a type object");
        }
    }
    let mut bases_vec = Vec::new();
    let bases_bits = if obj_from_bits(base_bits).is_none() || base_bits == 0 {
        let tuple_ptr = alloc_tuple(&[]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    } else {
        let base_obj = obj_from_bits(base_bits);
        let Some(base_ptr) = base_obj.as_ptr() else {
            raise!("TypeError", "base must be a type object or tuple of types");
        };
        unsafe {
            match object_type_id(base_ptr) {
                TYPE_ID_TYPE => {
                    bases_vec.push(base_bits);
                    let tuple_ptr = alloc_tuple(&[base_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
                TYPE_ID_TUPLE => {
                    for item in seq_vec_ref(base_ptr).iter() {
                        bases_vec.push(*item);
                    }
                    base_bits
                }
                _ => raise!("TypeError", "base must be a type object or tuple of types"),
            }
        }
    };

    if bases_vec.is_empty() {
        bases_vec = class_bases_vec(bases_bits);
    }
    let mut seen = HashSet::new();
    for base in &bases_vec {
        if !seen.insert(*base) {
            let name = class_name_for_error(*base);
            let msg = format!("duplicate base class {name}");
            raise!("TypeError", &msg);
        }
    }
    for base in bases_vec.iter() {
        let base_obj = obj_from_bits(*base);
        let Some(base_ptr) = base_obj.as_ptr() else {
            raise!("TypeError", "base must be a type object");
        };
        unsafe {
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                raise!("TypeError", "base must be a type object");
            }
            if base_ptr == class_ptr {
                raise!("TypeError", "class cannot inherit from itself");
            }
        }
    }

    let mro = match compute_mro(class_bits, &bases_vec) {
        Some(val) => val,
        None => {
            raise!(
                "TypeError",
                "Cannot create a consistent method resolution order (MRO) for bases"
            );
        }
    };
    let mro_ptr = alloc_tuple(&mro);
    if mro_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let mro_bits = MoltObject::from_ptr(mro_ptr).bits();

    unsafe {
        let old_bases = class_bases_bits(class_ptr);
        let old_mro = class_mro_bits(class_ptr);
        if old_bases != bases_bits {
            dec_ref_bits(old_bases);
            inc_ref_bits(bases_bits);
            class_set_bases_bits(class_ptr, bases_bits);
        }
        if old_mro != mro_bits {
            dec_ref_bits(old_mro);
            inc_ref_bits(mro_bits);
            class_set_mro_bits(class_ptr, mro_bits);
        }
        let dict_bits = class_dict_bits(class_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let bases_name = intern_static_name(&INTERN_BASES_NAME, b"__bases__");
                let mro_name = intern_static_name(&INTERN_MRO_NAME, b"__mro__");
                dict_set_in_place(dict_ptr, bases_name, bases_bits);
                dict_set_in_place(dict_ptr, mro_name, mro_bits);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    let type_obj = obj_from_bits(type_bits);
    let Some(type_ptr) = type_obj.as_ptr() else {
        raise!("TypeError", "super() arg 1 must be a type");
    };
    unsafe {
        if object_type_id(type_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "super() arg 1 must be a type");
        }
    }
    let obj = obj_from_bits(obj_bits);
    if obj.is_none() || obj_bits == 0 {
        raise!(
            "TypeError",
            "super() arg 2 must be an instance or subtype of type"
        );
    }
    let obj_type_bits = if let Some(obj_ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(obj_ptr) == TYPE_ID_TYPE {
                obj_bits
            } else {
                type_of_bits(obj_bits)
            }
        }
    } else {
        type_of_bits(obj_bits)
    };
    if !issubclass_bits(obj_type_bits, type_bits) {
        raise!(
            "TypeError",
            "super() arg 2 must be an instance or subtype of type"
        );
    }
    let ptr = alloc_super_obj(type_bits, obj_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_classmethod_new(func_bits: u64) -> u64 {
    let ptr = alloc_classmethod_obj(func_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_staticmethod_new(func_bits: u64) -> u64 {
    let ptr = alloc_staticmethod_obj(func_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_property_new(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    let ptr = alloc_property_obj(get_bits, set_bits, del_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// # Safety
/// `obj_ptr` must point to a valid Molt object header that can be mutated, and
/// `class_bits` must be either zero or a valid Molt type object.
#[no_mangle]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr: *mut u8, class_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no class");
    }
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).poll_fn != 0 {
        raise!("TypeError", "cannot set class on async object");
    }
    if class_bits != 0 {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            raise!("TypeError", "class must be a type object");
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "class must be a type object");
        }
    }
    let old_bits = object_class_bits(obj_ptr);
    if old_bits != 0 {
        dec_ref_bits(old_bits);
    }
    object_set_class_bits(obj_ptr, class_bits);
    if class_bits != 0 {
        inc_ref_bits(class_bits);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
        Some(val) => val,
        None => raise!("TypeError", "module name must be str"),
    };
    let cache = module_cache();
    let guard = cache.lock().unwrap();
    if let Some(bits) = guard.get(&name) {
        inc_ref_bits(*bits);
        return *bits;
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
        Some(val) => val,
        None => raise!("TypeError", "module name must be str"),
    };
    let cache = module_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(old) = guard.insert(name, module_bits) {
        dec_ref_bits(old);
    }
    inc_ref_bits(module_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        raise!("TypeError", "module attribute access expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            raise!("TypeError", "module attribute access expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => raise!("TypeError", "module dict missing"),
        };
        if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
            inc_ref_bits(val);
            return val;
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr))).unwrap_or_default();
        let attr_name =
            string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_else(|| "<attr>".to_string());
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        raise!("AttributeError", &msg);
    }
}

#[no_mangle]
pub extern "C" fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64 {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        raise!("TypeError", "module attribute set expects module");
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            raise!("TypeError", "module attribute set expects module");
        }
        let dict_bits = module_dict_bits(module_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = match dict_obj.as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => raise!("TypeError", "module dict missing"),
        };
        dict_set_in_place(dict_ptr, attr_bits, val_bits);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_new(kind_bits: u64, msg_bits: u64) -> u64 {
    let kind_obj = obj_from_bits(kind_bits);
    let msg_obj = obj_from_bits(msg_bits);
    if let Some(ptr) = kind_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                raise!("TypeError", "exception kind must be a str");
            }
        }
    } else {
        raise!("TypeError", "exception kind must be a str");
    }
    if let Some(ptr) = msg_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                raise!("TypeError", "exception message must be a str");
            }
        }
    } else {
        raise!("TypeError", "exception message must be a str");
    }
    let ptr = alloc_exception_obj(kind_bits, msg_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    ptr.add(offset) as *mut u64
}

unsafe fn generator_set_slot(ptr: *mut u8, offset: usize, bits: u64) {
    let slot = generator_slot_ptr(ptr, offset);
    let old_bits = *slot;
    dec_ref_bits(old_bits);
    inc_ref_bits(bits);
    *slot = bits;
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    if self_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let slot = self_ptr.add(offset as usize) as *mut u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    if self_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let slot = self_ptr.add(offset as usize) as *mut u64;
    let old_bits = *slot;
    dec_ref_bits(old_bits);
    inc_ref_bits(bits);
    *slot = bits;
    MoltObject::none().bits()
}

unsafe fn generator_closed(ptr: *mut u8) -> bool {
    let bits = *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET);
    obj_from_bits(bits).as_bool().unwrap_or(false)
}

unsafe fn generator_set_closed(ptr: *mut u8, closed: bool) {
    let bits = MoltObject::from_bool(closed).bits();
    generator_set_slot(ptr, GEN_CLOSED_OFFSET, bits);
}

fn generator_done_tuple(value_bits: u64) -> u64 {
    let done_bits = MoltObject::from_bool(true).bits();
    let tuple_ptr = alloc_tuple(&[value_bits, done_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn generator_unpack_pair(bits: u64) -> Option<(u64, bool)> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return None;
        }
        let done = obj_from_bits(elems[1]).as_bool().unwrap_or(false);
        Some((elems[0], done))
    }
}

#[no_mangle]
pub extern "C" fn molt_generator_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    let total_size = std::mem::size_of::<MoltHeader>() + closure_size as usize;
    let ptr = alloc_object(total_size, TYPE_ID_GENERATOR);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let header = header_from_obj_ptr(ptr);
        (*header).poll_fn = poll_fn_addr;
        (*header).state = 0;
        if closure_size as usize >= GEN_CONTROL_SIZE {
            *generator_slot_ptr(ptr, GEN_SEND_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_THROW_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET) = MoltObject::from_bool(false).bits();
            *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET) = MoltObject::from_int(1).bits();
        }
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    let is_gen = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
    MoltObject::from_bool(is_gen).bits()
}

#[no_mangle]
pub extern "C" fn molt_generator_send(gen_bits: u64, send_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        raise!("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            raise!("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return generator_done_tuple(MoltObject::none().bits());
        }
        generator_set_slot(ptr, GEN_SEND_OFFSET, send_bits);
        generator_set_slot(ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            return generator_done_tuple(MoltObject::none().bits());
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = poll_fn(ptr);
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        res as u64
    }
}

#[no_mangle]
pub extern "C" fn molt_generator_throw(gen_bits: u64, exc_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        raise!("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            raise!("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return generator_done_tuple(MoltObject::none().bits());
        }
        generator_set_slot(ptr, GEN_THROW_OFFSET, exc_bits);
        generator_set_slot(ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            return generator_done_tuple(MoltObject::none().bits());
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = poll_fn(ptr);
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        res as u64
    }
}

#[no_mangle]
pub extern "C" fn molt_generator_close(gen_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        raise!("TypeError", "expected generator");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            raise!("TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return MoltObject::none().bits();
        }
        let kind_ptr = alloc_string(b"GeneratorExit");
        if kind_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let msg_ptr = alloc_string(b"");
        if msg_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let kind_bits = MoltObject::from_ptr(kind_ptr).bits();
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let exc_ptr = alloc_exception_obj(kind_bits, msg_bits);
        dec_ref_bits(kind_bits);
        dec_ref_bits(msg_bits);
        if exc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        generator_set_slot(ptr, GEN_THROW_OFFSET, exc_bits);
        dec_ref_bits(exc_bits);
        generator_set_slot(ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            generator_set_closed(ptr, true);
            return MoltObject::none().bits();
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
        let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
        let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
        exception_stack_set_depth(gen_depth);
        let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = poll_fn(ptr) as u64;
        set_generator_raise(prev_raise);
        let pending = exception_pending();
        let exc_bits = if pending {
            let bits = molt_exception_last();
            clear_exception();
            bits
        } else {
            MoltObject::none().bits()
        };
        let new_depth = exception_stack_depth();
        generator_set_slot(
            ptr,
            GEN_EXC_DEPTH_OFFSET,
            MoltObject::from_int(new_depth as i64).bits(),
        );
        exception_context_align_depth(new_depth);
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        exception_stack_set_depth(caller_depth);
        exception_context_fallback_pop();
        if pending {
            let exc_obj = obj_from_bits(exc_bits);
            let is_exit = if let Some(exc_ptr) = exc_obj.as_ptr() {
                if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(exc_ptr)))
                        .unwrap_or_default();
                    kind == "GeneratorExit"
                } else {
                    false
                }
            } else {
                false
            };
            if is_exit {
                dec_ref_bits(exc_bits);
                generator_set_closed(ptr, true);
                return MoltObject::none().bits();
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(exc_bits);
            return raised;
        }
        if let Some((_val, done)) = generator_unpack_pair(res) {
            if !done {
                raise!("RuntimeError", "generator ignored GeneratorExit");
            }
        }
        generator_set_closed(ptr, true);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_kind(exc_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise!("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise!("TypeError", "expected exception object");
        }
        let bits = exception_kind_bits(ptr);
        inc_ref_bits(bits);
        bits
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_message(exc_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise!("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise!("TypeError", "expected exception object");
        }
        let bits = exception_msg_bits(ptr);
        inc_ref_bits(bits);
        bits
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_set_cause(exc_bits: u64, cause_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise!("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise!("TypeError", "expected exception object");
        }
    }
    let cause_obj = obj_from_bits(cause_bits);
    if !cause_obj.is_none() {
        let Some(cause_ptr) = cause_obj.as_ptr() else {
            raise!("TypeError", "exception cause must be an exception or None");
        };
        unsafe {
            if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                raise!("TypeError", "exception cause must be an exception or None");
            }
        }
    }
    unsafe {
        let old_bits = exception_cause_bits(ptr);
        if old_bits != cause_bits {
            dec_ref_bits(old_bits);
            inc_ref_bits(cause_bits);
            *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = cause_bits;
        }
        let suppress_bits = MoltObject::from_bool(true).bits();
        let old_suppress = exception_suppress_bits(ptr);
        if old_suppress != suppress_bits {
            dec_ref_bits(old_suppress);
            inc_ref_bits(suppress_bits);
            *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = suppress_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_context_set(exc_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    if !exc_obj.is_none() {
        let Some(ptr) = exc_obj.as_ptr() else {
            raise!("TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_EXCEPTION {
                raise!("TypeError", "expected exception object");
            }
        }
    }
    exception_context_set(exc_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_set_last(exc_bits: u64) {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise!("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise!("TypeError", "expected exception object");
        }
    }
    record_exception(ptr);
}

#[no_mangle]
pub extern "C" fn molt_exception_last() -> u64 {
    let cell = LAST_EXCEPTION.get_or_init(|| Mutex::new(None));
    let guard = cell.lock().unwrap();
    if let Some(ptr) = *guard {
        let bits = MoltObject::from_ptr(ptr as *mut u8).bits();
        inc_ref_bits(bits);
        bits
    } else {
        MoltObject::none().bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_clear() -> u64 {
    clear_exception();
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_pending() -> u64 {
    if exception_pending() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_push() -> u64 {
    exception_stack_push();
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_exception_pop() -> u64 {
    exception_stack_pop();
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_raise(exc_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise!("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise!("TypeError", "expected exception object");
        }
    }
    record_exception(ptr);
    if !exception_handler_active() && !generator_raise_active() {
        context_stack_unwind(MoltObject::from_ptr(ptr).bits());
        eprintln!("{}", format_exception(ptr));
        std::process::exit(1);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_context_new(enter_fn: u64, exit_fn: u64, payload_bits: u64) -> u64 {
    if enter_fn == 0 || exit_fn == 0 {
        raise!("TypeError", "context manager hooks must be non-null");
    }
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_context_enter(ctx_bits: u64) -> u64 {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        raise!("TypeError", "context manager must be an object");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_CONTEXT_MANAGER {
            let enter_fn_addr = context_enter_fn(ptr);
            if enter_fn_addr == 0 {
                raise!("TypeError", "context manager missing __enter__");
            }
            let enter_fn =
                std::mem::transmute::<usize, extern "C" fn(u64) -> u64>(enter_fn_addr as usize);
            let res = enter_fn(context_payload_bits(ptr));
            context_stack_push(ctx_bits);
            return res;
        }
        if type_id == TYPE_ID_FILE_HANDLE {
            let res = file_handle_enter(ptr);
            context_stack_push(ctx_bits);
            return res;
        }
        raise!("TypeError", "context manager protocol not supported");
    }
}

#[no_mangle]
pub extern "C" fn molt_context_exit(ctx_bits: u64, exc_bits: u64) -> u64 {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        raise!("TypeError", "context manager must be an object");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_CONTEXT_MANAGER {
            let exit_fn_addr = context_exit_fn(ptr);
            if exit_fn_addr == 0 {
                raise!("TypeError", "context manager missing __exit__");
            }
            let exit_fn =
                std::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(exit_fn_addr as usize);
            context_stack_pop(ctx_bits);
            return exit_fn(context_payload_bits(ptr), exc_bits);
        }
        if type_id == TYPE_ID_FILE_HANDLE {
            let res = file_handle_exit(ptr, exc_bits);
            context_stack_pop(ctx_bits);
            return res;
        }
        raise!("TypeError", "context manager protocol not supported");
    }
}

#[no_mangle]
pub extern "C" fn molt_context_unwind(exc_bits: u64) -> u64 {
    context_stack_unwind(exc_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_context_depth() -> u64 {
    MoltObject::from_int(context_stack_depth() as i64).bits()
}

#[no_mangle]
pub extern "C" fn molt_context_unwind_to(depth_bits: u64, exc_bits: u64) -> u64 {
    let depth = match to_i64(obj_from_bits(depth_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "context depth must be a non-negative int"),
    };
    context_stack_unwind_to(depth, exc_bits);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_context_null(payload_bits: u64) -> u64 {
    let enter_fn = context_null_enter as usize as u64;
    let exit_fn = context_null_exit as usize as u64;
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_context_closing(payload_bits: u64) -> u64 {
    let enter_fn = context_closing_enter as usize as u64;
    let exit_fn = context_closing_exit as usize as u64;
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn parse_file_mode(mode: &str) -> Result<(OpenOptions, bool, bool, bool), &'static str> {
    let mut kind: Option<char> = None;
    let mut read = false;
    let mut write = false;
    let mut append = false;
    let mut truncate = false;
    let mut create = false;
    let mut create_new = false;
    let text = !mode.contains('b');

    for ch in mode.chars() {
        match ch {
            'r' | 'w' | 'a' | 'x' => {
                if let Some(prev) = kind {
                    if prev != ch {
                        return Err("mode must include exactly one of 'r', 'w', 'a', or 'x'");
                    }
                } else {
                    kind = Some(ch);
                }
                match ch {
                    'r' => read = true,
                    'w' => {
                        write = true;
                        truncate = true;
                        create = true;
                    }
                    'a' => {
                        write = true;
                        append = true;
                        create = true;
                    }
                    'x' => {
                        write = true;
                        create = true;
                        create_new = true;
                    }
                    _ => {}
                }
            }
            '+' => {
                read = true;
                write = true;
            }
            'b' | 't' => {}
            _ => return Err("invalid mode"),
        }
    }

    if kind.is_none() {
        return Err("mode must include one of 'r', 'w', 'a', or 'x'");
    }

    let mut options = OpenOptions::new();
    options
        .read(read)
        .write(write)
        .append(append)
        .truncate(truncate)
        .create(create);
    if create_new {
        options.create_new(true);
    }
    Ok((options, read, write, text))
}

#[no_mangle]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    let path_obj = obj_from_bits(path_bits);
    let path = match string_obj_to_owned(path_obj) {
        Some(path) => path,
        None => raise!("TypeError", "open path must be a str"),
    };
    let mode_obj = obj_from_bits(mode_bits);
    let mode = if mode_obj.is_none() {
        "r".to_string()
    } else {
        match string_obj_to_owned(mode_obj) {
            Some(mode) => mode,
            None => raise!("TypeError", "open mode must be a str"),
        }
    };
    let (options, readable, writable, text) = match parse_file_mode(&mode) {
        Ok(parsed) => parsed,
        Err(msg) => raise!("ValueError", msg),
    };
    if readable && !has_capability("fs.read") {
        raise!("PermissionError", "missing fs.read capability");
    }
    if writable && !has_capability("fs.write") {
        raise!("PermissionError", "missing fs.write capability");
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(_) => raise!("OSError", "open failed"),
    };
    let ptr = alloc_file_handle(file, readable, writable, text);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        raise!("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            raise!("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            raise!("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        if !handle.readable {
            raise!("PermissionError", "file not readable");
        }
        let mut guard = handle.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            raise!("ValueError", "I/O operation on closed file");
        };
        let mut buf = Vec::new();
        let size_obj = obj_from_bits(size_bits);
        let size = if size_obj.is_none() {
            None
        } else {
            match to_i64(size_obj) {
                Some(val) if val >= 0 => Some(val as usize),
                Some(_) => None,
                None => raise!("TypeError", "read size must be int"),
            }
        };
        match size {
            Some(len) => {
                buf.resize(len, 0);
                let n = match file.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => raise!("OSError", "read failed"),
                };
                buf.truncate(n);
            }
            None => {
                if file.read_to_end(&mut buf).is_err() {
                    raise!("OSError", "read failed");
                }
            }
        }
        if handle.text {
            let text = match String::from_utf8(buf) {
                Ok(text) => text,
                Err(_) => raise!("ValueError", "file decode failed"),
            };
            let out_ptr = alloc_string(text.as_bytes());
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        } else {
            let out_ptr = alloc_bytes(&buf);
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        raise!("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            raise!("TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            raise!("RuntimeError", "file handle missing");
        }
        let handle = &*handle_ptr;
        if !handle.writable {
            raise!("PermissionError", "file not writable");
        }
        let mut guard = handle.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            raise!("ValueError", "I/O operation on closed file");
        };
        let data_obj = obj_from_bits(data_bits);
        let bytes: Vec<u8> = if handle.text {
            let text = match string_obj_to_owned(data_obj) {
                Some(text) => text,
                None => raise!("TypeError", "write expects str for text mode"),
            };
            text.into_bytes()
        } else {
            let Some(data_ptr) = data_obj.as_ptr() else {
                raise!("TypeError", "write expects bytes or bytearray");
            };
            let type_id = object_type_id(data_ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                raise!("TypeError", "write expects bytes or bytearray");
            }
            let len = bytes_len(data_ptr);
            let raw = std::slice::from_raw_parts(bytes_data(data_ptr), len);
            raw.to_vec()
        };
        if file.write_all(&bytes).is_err() {
            raise!("OSError", "write failed");
        }
        MoltObject::from_int(bytes.len() as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        raise!("TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            raise!("TypeError", "expected file handle");
        }
    }
    file_handle_close_ptr(ptr);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bridge_unavailable(msg_bits: u64) -> u64 {
    let msg = format_obj_str(obj_from_bits(msg_bits));
    eprintln!("Molt bridge unavailable: {msg}");
    std::process::exit(1);
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_new(rows_bits: u64, cols_bits: u64, init_bits: u64) -> u64 {
    let rows = match to_i64(obj_from_bits(rows_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "rows must be a non-negative int"),
    };
    let cols = match to_i64(obj_from_bits(cols_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "cols must be a non-negative int"),
    };
    let init = match obj_from_bits(init_bits).as_int() {
        Some(val) => val,
        None => raise!("TypeError", "init must be an int"),
    };
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
    let ptr = alloc_object(total, TYPE_ID_BUFFER2D);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let size = rows.saturating_mul(cols);
    let buf = Box::new(Buffer2D {
        rows,
        cols,
        data: vec![init; size],
    });
    let buf_ptr = Box::into_raw(buf);
    unsafe {
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_get(obj_bits: u64, row_bits: u64, col_bits: u64) -> u64 {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "col must be a non-negative int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &*buf;
            if row >= buf.rows || col >= buf.cols {
                raise!("IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            return MoltObject::from_int(buf.data[idx]).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_set(
    obj_bits: u64,
    row_bits: u64,
    col_bits: u64,
    val_bits: u64,
) -> u64 {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise!("TypeError", "col must be a non-negative int"),
    };
    let val = match obj_from_bits(val_bits).as_int() {
        Some(v) => v,
        None => raise!("TypeError", "value must be an int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &mut *buf;
            if row >= buf.rows || col >= buf.cols {
                raise!("IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            buf.data[idx] = val;
            return MoltObject::none().bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_matmul(a_bits: u64, b_bits: u64) -> u64 {
    let a = obj_from_bits(a_bits);
    let b = obj_from_bits(b_bits);
    let (a_ptr, b_ptr) = match (a.as_ptr(), b.as_ptr()) {
        (Some(ap), Some(bp)) => (ap, bp),
        _ => raise!("TypeError", "matmul expects buffer2d operands"),
    };
    unsafe {
        if object_type_id(a_ptr) != TYPE_ID_BUFFER2D || object_type_id(b_ptr) != TYPE_ID_BUFFER2D {
            raise!("TypeError", "matmul expects buffer2d operands");
        }
        let a_buf = buffer2d_ptr(a_ptr);
        let b_buf = buffer2d_ptr(b_ptr);
        if a_buf.is_null() || b_buf.is_null() {
            return MoltObject::none().bits();
        }
        let a_buf = &*a_buf;
        let b_buf = &*b_buf;
        if a_buf.cols != b_buf.rows {
            raise!("ValueError", "matmul dimension mismatch");
        }
        let rows = a_buf.rows;
        let cols = b_buf.cols;
        let inner = a_buf.cols;
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
        let ptr = alloc_object(total, TYPE_ID_BUFFER2D);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut data = vec![0i64; rows.saturating_mul(cols)];
        for i in 0..rows {
            for j in 0..cols {
                let mut acc = 0i64;
                for k in 0..inner {
                    let left = a_buf.data[i * inner + k];
                    let right = b_buf.data[k * cols + j];
                    acc = acc.wrapping_add(left.wrapping_mul(right));
                }
                data[i * cols + j] = acc;
            }
        }
        let buf = Box::new(Buffer2D { rows, cols, data });
        let buf_ptr = Box::into_raw(buf);
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_new(capacity_hint: usize) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let order = Vec::with_capacity(capacity_hint * 2);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(dict_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_builder_new(capacity_hint: usize) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_DICT_BUILDER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let vec = Vec::with_capacity(capacity_hint * 2);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    ptr
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_ptr` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_append(builder_ptr: *mut u8, key: u64, val: u64) {
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_ptr` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_finish(builder_ptr: *mut u8) -> u64 {
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_dict_with_pairs(vec.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

// --- Channels ---

pub struct MoltChannel {
    pub sender: Sender<i64>,
    pub receiver: Receiver<i64>,
}

pub struct MoltStream {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
}

pub struct MoltWebSocket {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> *mut u8 {
    let capacity = match to_i64(obj_from_bits(capacity_bits)) {
        Some(val) => val,
        None => raise!("TypeError", "channel capacity must be an integer"),
    };
    if capacity < 0 {
        raise!("ValueError", "channel capacity must be non-negative");
    }
    let capacity = capacity as usize;
    let (s, r) = if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    };
    let chan = Box::new(MoltChannel {
        sender: s,
        receiver: r,
    });
    Box::into_raw(chan) as *mut u8
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_ptr: *mut u8, val: i64) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_ptr: *mut u8) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => pending_bits_i64(), // PENDING
    }
}

fn bytes_channel(capacity: usize) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
    if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    }
}

#[no_mangle]
pub extern "C" fn molt_stream_new(capacity: usize) -> *mut u8 {
    let (s, r) = bytes_channel(capacity);
    let stream = Box::new(MoltStream {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
    });
    Box::into_raw(stream) as *mut u8
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_ptr` is valid; `data_ptr` must be readable for `len` bytes.
pub unsafe extern "C" fn molt_stream_send(
    stream_ptr: *mut u8,
    data_ptr: *const u8,
    len: usize,
) -> i64 {
    if stream_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match stream.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_ptr` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_ptr: *mut u8) -> i64 {
    if stream_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    match stream.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(_) => {
            if stream.closed.load(Ordering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_ptr` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_ptr: *mut u8) {
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.closed.store(true, Ordering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `out_left` and `out_right` are valid writable pointers.
pub unsafe extern "C" fn molt_ws_pair(
    capacity: usize,
    out_left: *mut *mut u8,
    out_right: *mut *mut u8,
) -> i32 {
    if out_left.is_null() || out_right.is_null() {
        return 2;
    }
    let (a_tx, a_rx) = bytes_channel(capacity);
    let (b_tx, b_rx) = bytes_channel(capacity);
    let left = Box::new(MoltWebSocket {
        sender: a_tx,
        receiver: b_rx,
        closed: AtomicBool::new(false),
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    let right = Box::new(MoltWebSocket {
        sender: b_tx,
        receiver: a_rx,
        closed: AtomicBool::new(false),
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    *out_left = Box::into_raw(left) as *mut u8;
    *out_right = Box::into_raw(right) as *mut u8;
    0
}

#[no_mangle]
pub extern "C" fn molt_ws_new_with_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    let send_hook = if send_hook == 0 {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<usize, extern "C" fn(*mut u8, *const u8, usize) -> i64>(send_hook)
        })
    };
    let recv_hook = if recv_hook == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8) -> i64>(recv_hook) })
    };
    let close_hook = if close_hook == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8)>(close_hook) })
    };
    let (s, r) = bytes_channel(0);
    let ws = Box::new(MoltWebSocket {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
        send_hook,
        recv_hook,
        close_hook,
        hook_ctx,
    });
    Box::into_raw(ws) as *mut u8
}

type WsConnectHook = extern "C" fn(*const u8, usize) -> *mut u8;

static WS_CONNECT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CAPABILITIES: OnceLock<HashSet<String>> = OnceLock::new();

#[no_mangle]
pub extern "C" fn molt_ws_set_connect_hook(ptr: usize) {
    WS_CONNECT_HOOK.store(ptr, Ordering::Release);
}

fn load_capabilities() -> HashSet<String> {
    let mut set = HashSet::new();
    let caps = std::env::var("MOLT_CAPABILITIES").unwrap_or_default();
    for cap in caps.split(',') {
        let cap = cap.trim();
        if !cap.is_empty() {
            set.insert(cap.to_string());
        }
    }
    set
}

fn has_capability(name: &str) -> bool {
    let caps = CAPABILITIES.get_or_init(load_capabilities);
    caps.contains(name)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len: usize,
    out: *mut *mut u8,
) -> i32 {
    if out.is_null() {
        return 2;
    }
    if !has_capability("websocket:connect") {
        return 6;
    }
    let hook_ptr = WS_CONNECT_HOOK.load(Ordering::Acquire);
    if hook_ptr == 0 {
        // TODO(molt): Provide a host-level connect hook for production sockets.
        return 7;
    }
    let hook: WsConnectHook = std::mem::transmute(hook_ptr);
    let ws_ptr = hook(url_ptr, url_len);
    if ws_ptr.is_null() {
        return 7;
    }
    *out = ws_ptr;
    0
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_ptr` is valid; `data_ptr` must be readable for `len` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_ptr: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ws_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.send_hook {
        return hook(ws.hook_ctx, data_ptr, len);
    }
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match ws.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_ptr` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_ptr: *mut u8) -> i64 {
    if ws_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.recv_hook {
        return hook(ws.hook_ctx);
    }
    match ws.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(_) => {
            if ws.closed.load(Ordering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_ptr` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_ptr: *mut u8) {
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.close_hook {
        hook(ws.hook_ctx);
        return;
    }
    ws.closed.store(true, Ordering::Relaxed);
}

// --- Scheduler ---

pub struct MoltTask {
    pub future_ptr: *mut u8,
}

unsafe impl Send for MoltTask {}

pub struct MoltScheduler {
    injector: Arc<Injector<MoltTask>>,
    stealers: Vec<Stealer<MoltTask>>,
    running: Arc<AtomicBool>,
}

impl MoltScheduler {
    pub fn new() -> Self {
        let num_threads = num_cpus::get();
        let injector = Arc::new(Injector::new());
        let mut workers = Vec::new();
        let mut stealers = Vec::new();
        let running = Arc::new(AtomicBool::new(true));

        for _ in 0..num_threads {
            workers.push(Worker::new_fifo());
        }

        for w in &workers {
            stealers.push(w.stealer());
        }

        for (i, worker) in workers.into_iter().enumerate() {
            let injector_clone = Arc::clone(&injector);
            let stealers_clone = stealers.clone();
            let running_clone = Arc::clone(&running);

            thread::spawn(move || loop {
                if !running_clone.load(Ordering::Relaxed) {
                    break;
                }

                if let Some(task) = worker.pop() {
                    Self::execute_task(task, &injector_clone);
                    continue;
                }

                match injector_clone.steal_batch_and_pop(&worker) {
                    crossbeam_deque::Steal::Success(task) => {
                        Self::execute_task(task, &injector_clone);
                        continue;
                    }
                    crossbeam_deque::Steal::Retry => continue,
                    crossbeam_deque::Steal::Empty => {}
                }

                let mut stolen = false;
                for (j, stealer) in stealers_clone.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    if let crossbeam_deque::Steal::Success(task) =
                        stealer.steal_batch_and_pop(&worker)
                    {
                        Self::execute_task(task, &injector_clone);
                        stolen = true;
                        break;
                    }
                }

                if !stolen {
                    thread::yield_now();
                }
            });
        }

        Self {
            injector,
            stealers,
            running,
        }
    }

    pub fn enqueue(&self, task: MoltTask) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        if self.stealers.is_empty() {
            Self::execute_task(task, &self.injector);
        } else {
            self.injector.push(task);
        }
    }

    fn execute_task(task: MoltTask, injector: &Injector<MoltTask>) {
        unsafe {
            let task_ptr = task.future_ptr;
            let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr != 0 {
                let poll_fn: extern "C" fn(*mut u8) -> i64 =
                    std::mem::transmute(poll_fn_addr as usize);
                let res = poll_fn(task_ptr);
                if res == pending_bits_i64() {
                    injector.push(task);
                }
            }
        }
    }
}

impl Default for MoltScheduler {
    fn default() -> Self {
        Self::new()
    }
}

lazy_static::lazy_static! {
    static ref SCHEDULER: MoltScheduler = MoltScheduler::new();
    static ref UTF8_INDEX_CACHE: Mutex<Utf8CacheStore> = Mutex::new(Utf8CacheStore::new());
}

/// # Safety
/// - `task_ptr` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_ptr: *mut u8) {
    SCHEDULER.enqueue(MoltTask {
        future_ptr: task_ptr,
    });
}

/// # Safety
/// - `task_ptr` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_ptr: *mut u8) -> i64 {
    let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 {
        return 0;
    }
    let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
    loop {
        let res = poll_fn(task_ptr);
        if res == pending_bits_i64() {
            std::thread::yield_now();
            continue;
        }
        return res;
    }
}

/// # Safety
/// - `_obj_ptr` must be a valid pointer if the runtime associates a future with it.
#[no_mangle]
pub unsafe extern "C" fn molt_async_sleep(_obj_ptr: *mut u8) -> i64 {
    0
}

// --- NaN-boxed ops ---

#[no_mangle]
pub extern "C" fn molt_add(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (lhs.as_int(), rhs.as_int()) {
        return MoltObject::from_int(li + ri).bits();
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                if let Some(bits) = concat_bytes_like(l_bytes, r_bytes, TYPE_ID_STRING) {
                    return bits;
                }
                return MoltObject::none().bits();
            }
            if ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTES {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                if let Some(bits) = concat_bytes_like(l_bytes, r_bytes, TYPE_ID_BYTES) {
                    return bits;
                }
                return MoltObject::none().bits();
            }
            if ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                if let Some(bits) = concat_bytes_like(l_bytes, r_bytes, TYPE_ID_BYTEARRAY) {
                    return bits;
                }
                return MoltObject::none().bits();
            }
            if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
                let l_len = list_len(lp);
                let r_len = list_len(rp);
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                let mut combined = Vec::with_capacity(l_len + r_len);
                combined.extend_from_slice(l_elems);
                combined.extend_from_slice(r_elems);
                let ptr = alloc_list(&combined);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return MoltObject::from_float(lf + rf).bits();
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_sub(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (lhs.as_int(), rhs.as_int()) {
        return MoltObject::from_int(li - ri).bits();
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return MoltObject::from_float(lf - rf).bits();
    }
    MoltObject::none().bits()
}

fn repeat_sequence(ptr: *mut u8, count: i64) -> Option<u64> {
    unsafe {
        let type_id = object_type_id(ptr);
        if count <= 0 {
            let out_ptr = match type_id {
                TYPE_ID_LIST => alloc_list(&[]),
                TYPE_ID_TUPLE => alloc_tuple(&[]),
                TYPE_ID_STRING => alloc_string(&[]),
                TYPE_ID_BYTES => alloc_bytes(&[]),
                TYPE_ID_BYTEARRAY => alloc_bytearray(&[]),
                _ => return None,
            };
            if out_ptr.is_null() {
                return None;
            }
            return Some(MoltObject::from_ptr(out_ptr).bits());
        }

        let times = count as usize;
        match type_id {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(ptr);
                let total = elems.len().checked_mul(times)?;
                let mut combined = Vec::with_capacity(total);
                for _ in 0..times {
                    combined.extend_from_slice(elems);
                }
                let out_ptr = alloc_list(&combined);
                if out_ptr.is_null() {
                    return None;
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                let total = elems.len().checked_mul(times)?;
                let mut combined = Vec::with_capacity(total);
                for _ in 0..times {
                    combined.extend_from_slice(elems);
                }
                let out_ptr = alloc_tuple(&combined);
                if out_ptr.is_null() {
                    return None;
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_STRING => {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let total = len.checked_mul(times)?;
                let out_ptr = alloc_bytes_like_with_len(total, TYPE_ID_STRING);
                if out_ptr.is_null() {
                    return None;
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTES => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = len.checked_mul(times)?;
                let out_ptr = alloc_bytes_like_with_len(total, TYPE_ID_BYTES);
                if out_ptr.is_null() {
                    return None;
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTEARRAY => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = len.checked_mul(times)?;
                let out_ptr = alloc_bytes_like_with_len(total, TYPE_ID_BYTEARRAY);
                if out_ptr.is_null() {
                    return None;
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            _ => None,
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_mul(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (lhs.as_int(), rhs.as_int()) {
        return MoltObject::from_int(li * ri).bits();
    }
    if let Some(count) = to_i64(lhs) {
        if let Some(ptr) = rhs.as_ptr() {
            if let Some(bits) = repeat_sequence(ptr, count) {
                return bits;
            }
        }
    }
    if let Some(count) = to_i64(rhs) {
        if let Some(ptr) = lhs.as_ptr() {
            if let Some(bits) = repeat_sequence(ptr, count) {
                return bits;
            }
        }
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return MoltObject::from_float(lf * rf).bits();
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_lt(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (lhs.as_int(), rhs.as_int()) {
        return MoltObject::from_bool(li < ri).bits();
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return MoltObject::from_bool(lf < rf).bits();
    }
    MoltObject::from_bool(false).bits()
}

#[no_mangle]
pub extern "C" fn molt_eq(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    MoltObject::from_bool(obj_eq(lhs, rhs)).bits()
}

#[no_mangle]
pub extern "C" fn molt_is(a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a == b).bits()
}

#[no_mangle]
pub extern "C" fn molt_str_from_obj(val_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_STRING {
                molt_inc_ref(ptr);
                return val_bits;
            }
        }
    }
    let rendered = format_obj_str(obj);
    let ptr = alloc_string(rendered.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_guard_type(val_bits: u64, expected_bits: u64) -> u64 {
    let expected = match to_i64(obj_from_bits(expected_bits)) {
        Some(val) => val,
        None => raise!("TypeError", "guard type tag must be int"),
    };
    if expected == TYPE_TAG_ANY {
        return val_bits;
    }
    let obj = obj_from_bits(val_bits);
    let matches = match expected {
        TYPE_TAG_INT => obj.is_int(),
        TYPE_TAG_FLOAT => obj.is_float(),
        TYPE_TAG_BOOL => obj.is_bool(),
        TYPE_TAG_NONE => obj.is_none(),
        TYPE_TAG_STR => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING }),
        TYPE_TAG_BYTES => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTES }),
        TYPE_TAG_BYTEARRAY => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY }),
        TYPE_TAG_LIST => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_LIST }),
        TYPE_TAG_TUPLE => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE }),
        TYPE_TAG_INTARRAY => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_INTARRAY }),
        TYPE_TAG_DICT => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT }),
        TYPE_TAG_RANGE => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_RANGE }),
        TYPE_TAG_SLICE => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SLICE }),
        TYPE_TAG_DATACLASS => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DATACLASS }),
        TYPE_TAG_BUFFER2D => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BUFFER2D }),
        TYPE_TAG_MEMORYVIEW => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MEMORYVIEW }),
        _ => false,
    };
    if !matches {
        raise!("TypeError", "type guard mismatch");
    }
    val_bits
}

#[no_mangle]
pub extern "C" fn molt_is_truthy(val: u64) -> i64 {
    if is_truthy(obj_from_bits(val)) {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn molt_not(val: u64) -> u64 {
    MoltObject::from_bool(!is_truthy(obj_from_bits(val))).bits()
}

fn vec_sum_result(sum_bits: u64, ok: bool) -> u64 {
    let ok_bits = MoltObject::from_bool(ok).bits();
    let tuple_ptr = alloc_tuple(&[sum_bits, ok_bits]);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn sum_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut sum = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            sum += val;
        } else {
            return None;
        }
    }
    Some(sum)
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm_setzero_si128();
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let vec = _mm_set_epi64x(v1, v0);
        vec_sum = _mm_add_epi64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        sum += val;
    }
    Some(sum)
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm256_setzero_si256();
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        vec_sum = _mm256_add_epi64(vec_sum, vec);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_sum);
    let mut sum = acc + lanes.iter().sum::<i64>();
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        sum += val;
    }
    Some(sum)
}

#[cfg(target_arch = "aarch64")]
unsafe fn sum_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_sum = vdupq_n_s64(0);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        vec_sum = vaddq_s64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        sum += val;
    }
    Some(sum)
}

fn sum_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { sum_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_ints_simd_aarch64(elems, acc) };
        }
    }
    sum_ints_scalar(elems, acc)
}

fn prod_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut prod = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            prod *= val;
        } else {
            return None;
        }
    }
    Some(prod)
}

fn prod_ints_unboxed(elems: &[i64], acc: i64) -> i64 {
    let mut prod = acc;
    if prod == 0 {
        return 0;
    }
    if prod == 1 {
        if let Some(result) = prod_ints_unboxed_trivial(elems) {
            return result;
        }
    }
    for &val in elems {
        if val == 0 {
            return 0;
        }
        prod *= val;
    }
    prod
}

fn prod_ints_unboxed_trivial(_elems: &[i64]) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { prod_ints_unboxed_avx2_trivial(_elems) };
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
unsafe fn prod_ints_unboxed_avx2_trivial(elems: &[i64]) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut idx = 0usize;
    let ones = _mm256_set1_epi64x(1);
    let zeros = _mm256_setzero_si256();
    let mut all_ones = true;
    while idx + 4 <= elems.len() {
        let vec = _mm256_loadu_si256(elems.as_ptr().add(idx) as *const __m256i);
        let eq_zero = _mm256_cmpeq_epi64(vec, zeros);
        if _mm256_movemask_epi8(eq_zero) != 0 {
            return Some(0);
        }
        if all_ones {
            let eq_one = _mm256_cmpeq_epi64(vec, ones);
            if _mm256_movemask_epi8(eq_one) != -1 {
                all_ones = false;
            }
        }
        idx += 4;
    }
    for &val in &elems[idx..] {
        if val == 0 {
            return Some(0);
        }
        if val != 1 {
            all_ones = false;
        }
    }
    if all_ones {
        return Some(1);
    }
    None
}

#[cfg(target_arch = "aarch64")]
unsafe fn prod_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    prod_ints_scalar(elems, acc)
}

fn prod_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { prod_ints_simd_aarch64(elems, acc) };
        }
    }
    prod_ints_scalar(elems, acc)
}

fn min_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut min_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            if val < min_val {
                min_val = val;
            }
        } else {
            return None;
        }
    }
    Some(min_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec_min, vec);
        vec_min = _mm_blendv_epi8(vec_min, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_min);
    let mut min_val = acc.min(lanes[0]).min(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
        }
    }
    Some(min_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec_min, vec);
        vec_min = _mm256_blendv_epi8(vec_min, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_min);
    let mut min_val = acc;
    for lane in lanes {
        if lane < min_val {
            min_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
        }
    }
    Some(min_val)
}

#[cfg(target_arch = "aarch64")]
unsafe fn min_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_min = vdupq_n_s64(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        let mask = vcgtq_s64(vec_min, vec);
        let vec_min_u = vreinterpretq_u64_s64(vec_min);
        let vec_u = vreinterpretq_u64_s64(vec);
        let blended_u = vbslq_u64(mask, vec_u, vec_min_u);
        vec_min = vreinterpretq_s64_u64(blended_u);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_min);
    let mut min_val = acc.min(lanes[0]).min(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
        }
    }
    Some(min_val)
}

fn min_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { min_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { min_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { min_ints_simd_aarch64(elems, acc) };
        }
    }
    min_ints_scalar(elems, acc)
}

fn max_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut max_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            if val > max_val {
                max_val = val;
            }
        } else {
            return None;
        }
    }
    Some(max_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec, vec_max);
        vec_max = _mm_blendv_epi8(vec_max, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_max);
    let mut max_val = acc.max(lanes[0]).max(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
        }
    }
    Some(max_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec, vec_max);
        vec_max = _mm256_blendv_epi8(vec_max, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_max);
    let mut max_val = acc;
    for lane in lanes {
        if lane > max_val {
            max_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
        }
    }
    Some(max_val)
}

#[cfg(target_arch = "aarch64")]
unsafe fn max_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_max = vdupq_n_s64(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        let mask = vcgtq_s64(vec, vec_max);
        let vec_max_u = vreinterpretq_u64_s64(vec_max);
        let vec_u = vreinterpretq_u64_s64(vec);
        let blended_u = vbslq_u64(mask, vec_u, vec_max_u);
        vec_max = vreinterpretq_s64_u64(blended_u);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_max);
    let mut max_val = acc.max(lanes[0]).max(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
        }
    }
    Some(max_val)
}

fn max_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { max_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { max_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { max_ints_simd_aarch64(elems, acc) };
        }
    }
    max_ints_scalar(elems, acc)
}

fn sum_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut sum = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm_setzero_si128();
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        vec_sum = _mm_add_epi64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm256_setzero_si256();
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let v2 = obj2.as_int_unchecked();
        let v3 = obj3.as_int_unchecked();
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        vec_sum = _mm256_add_epi64(vec_sum, vec);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_sum);
    let mut sum = acc + lanes.iter().sum::<i64>();
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "aarch64")]
unsafe fn sum_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_sum = vdupq_n_s64(0);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        vec_sum = vaddq_s64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

fn sum_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { sum_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    sum_ints_trusted_scalar(elems, acc)
}

fn prod_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut prod = acc;
    if prod == 0 {
        return 0;
    }
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val == 0 {
            return 0;
        }
        prod *= val;
    }
    prod
}

#[cfg(target_arch = "aarch64")]
unsafe fn prod_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    prod_ints_trusted_scalar(elems, acc)
}

fn prod_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { prod_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    prod_ints_trusted_scalar(elems, acc)
}

fn min_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut min_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec_min, vec);
        vec_min = _mm_blendv_epi8(vec_min, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_min);
    let mut min_val = acc.min(lanes[0]).min(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let v2 = obj2.as_int_unchecked();
        let v3 = obj3.as_int_unchecked();
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec_min, vec);
        vec_min = _mm256_blendv_epi8(vec_min, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_min);
    let mut min_val = acc;
    for lane in lanes {
        if lane < min_val {
            min_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "aarch64")]
unsafe fn min_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_min = vdupq_n_s64(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        let mask = vcgtq_s64(vec_min, vec);
        let vec_min_u = vreinterpretq_u64_s64(vec_min);
        let vec_u = vreinterpretq_u64_s64(vec);
        let blended_u = vbslq_u64(mask, vec_u, vec_min_u);
        vec_min = vreinterpretq_s64_u64(blended_u);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_min);
    let mut min_val = acc.min(lanes[0]).min(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

fn min_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { min_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { min_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { min_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    min_ints_trusted_scalar(elems, acc)
}

fn max_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut max_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec, vec_max);
        vec_max = _mm_blendv_epi8(vec_max, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_max);
    let mut max_val = acc.max(lanes[0]).max(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let v2 = obj2.as_int_unchecked();
        let v3 = obj3.as_int_unchecked();
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec, vec_max);
        vec_max = _mm256_blendv_epi8(vec_max, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_max);
    let mut max_val = acc;
    for lane in lanes {
        if lane > max_val {
            max_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "aarch64")]
unsafe fn max_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::aarch64::*;
    let mut i = 0usize;
    let mut vec_max = vdupq_n_s64(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let lanes = [v0, v1];
        let vec = vld1q_s64(lanes.as_ptr());
        let mask = vcgtq_s64(vec, vec_max);
        let vec_max_u = vreinterpretq_u64_s64(vec_max);
        let vec_u = vreinterpretq_u64_s64(vec);
        let blended_u = vbslq_u64(mask, vec_u, vec_max_u);
        vec_max = vreinterpretq_s64_u64(blended_u);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    vst1q_s64(lanes.as_mut_ptr(), vec_max);
    let mut max_val = acc.max(lanes[0]).max(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

fn max_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { max_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { max_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { max_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    max_ints_trusted_scalar(elems, acc)
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        if let Some(sum) = sum_ints_checked(elems, acc) {
            return vec_sum_result(MoltObject::from_int(sum).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let sum = sum_ints_trusted(elems, acc);
        vec_sum_result(MoltObject::from_int(sum).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_prod_int(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_INTARRAY {
            let elems = intarray_slice(ptr);
            let prod = prod_ints_unboxed(elems, acc);
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        if let Some(prod) = prod_ints_checked(elems, acc) {
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_prod_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_INTARRAY {
            let elems = intarray_slice(ptr);
            let prod = prod_ints_unboxed(elems, acc);
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let prod = prod_ints_trusted(elems, acc);
        vec_sum_result(MoltObject::from_int(prod).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_min_int(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        if let Some(val) = min_ints_checked(elems, acc) {
            return vec_sum_result(MoltObject::from_int(val).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_min_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let val = min_ints_trusted(elems, acc);
        vec_sum_result(MoltObject::from_int(val).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_max_int(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        if let Some(val) = max_ints_checked(elems, acc) {
            return vec_sum_result(MoltObject::from_int(val).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_max_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let val = max_ints_trusted(elems, acc);
        vec_sum_result(MoltObject::from_int(val).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        if let Some(sum) = sum_ints_checked(slice, acc) {
            return vec_sum_result(MoltObject::from_int(sum).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        let sum = sum_ints_trusted(slice, acc);
        vec_sum_result(MoltObject::from_int(sum).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_prod_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_INTARRAY {
            let elems = intarray_slice(ptr);
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let prod = prod_ints_unboxed(slice, acc);
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        if let Some(prod) = prod_ints_checked(slice, acc) {
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_prod_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_INTARRAY {
            let elems = intarray_slice(ptr);
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let prod = prod_ints_unboxed(slice, acc);
            return vec_sum_result(MoltObject::from_int(prod).bits(), true);
        }
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        let prod = prod_ints_trusted(slice, acc);
        vec_sum_result(MoltObject::from_int(prod).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_min_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        if let Some(val) = min_ints_checked(slice, acc) {
            return vec_sum_result(MoltObject::from_int(val).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_min_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        let val = min_ints_trusted(slice, acc);
        vec_sum_result(MoltObject::from_int(val).bits(), true)
    }
}

#[no_mangle]
pub extern "C" fn molt_vec_max_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        if let Some(val) = max_ints_checked(slice, acc) {
            return vec_sum_result(MoltObject::from_int(val).bits(), true);
        }
    }
    vec_sum_result(MoltObject::from_int(acc).bits(), false)
}

#[no_mangle]
pub extern "C" fn molt_vec_max_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    let acc_obj = obj_from_bits(acc_bits);
    let acc = match acc_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::none().bits(), false),
    };
    let start_obj = obj_from_bits(start_bits);
    let start = match start_obj.as_int() {
        Some(val) => val,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    if start < 0 {
        return vec_sum_result(MoltObject::from_int(acc).bits(), false);
    }
    let seq_obj = obj_from_bits(seq_bits);
    let ptr = match seq_obj.as_ptr() {
        Some(ptr) => ptr,
        None => return vec_sum_result(MoltObject::from_int(acc).bits(), false),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            seq_vec_ref(ptr)
        } else {
            return vec_sum_result(MoltObject::from_int(acc).bits(), false);
        };
        let start_idx = (start as usize).min(elems.len());
        let slice = &elems[start_idx..];
        let val = max_ints_trusted(slice, acc);
        vec_sum_result(MoltObject::from_int(val).bits(), true)
    }
}

enum SliceError {
    Type,
    Value,
}

fn slice_error(err: SliceError) -> u64 {
    match err {
        SliceError::Type => {
            raise!("TypeError", "slice indices must be integers or None");
        }
        SliceError::Value => {
            raise!("ValueError", "slice step cannot be zero");
        }
    }
}

fn decode_slice_bound(obj: MoltObject, len: isize, default: isize) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    if let Some(i) = obj.as_int() {
        let mut idx = i as isize;
        if idx < 0 {
            idx += len;
        }
        if idx < 0 {
            idx = 0;
        }
        if idx > len {
            idx = len;
        }
        return Ok(idx);
    }
    Err(SliceError::Type)
}

fn decode_slice_bound_neg(
    obj: MoltObject,
    len: isize,
    default: isize,
) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    if let Some(i) = obj.as_int() {
        let mut idx = i as isize;
        if idx < 0 {
            idx += len;
        }
        if idx < -1 {
            idx = -1;
        }
        if idx >= len {
            idx = len - 1;
        }
        return Ok(idx);
    }
    Err(SliceError::Type)
}

fn decode_slice_step(obj: MoltObject) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(1);
    }
    if let Some(i) = obj.as_int() {
        let step = i as isize;
        if step == 0 {
            return Err(SliceError::Value);
        }
        return Ok(step);
    }
    Err(SliceError::Type)
}

fn normalize_slice_indices(
    len: isize,
    start_obj: MoltObject,
    stop_obj: MoltObject,
    step_obj: MoltObject,
) -> Result<(isize, isize, isize), SliceError> {
    let step = decode_slice_step(step_obj)?;
    if step > 0 {
        let start = decode_slice_bound(start_obj, len, 0)?;
        let stop = decode_slice_bound(stop_obj, len, len)?;
        return Ok((start, stop, step));
    }
    let start_default = if len == 0 { -1 } else { len - 1 };
    let stop_default = -1;
    let start = decode_slice_bound_neg(start_obj, len, start_default)?;
    let stop = decode_slice_bound_neg(stop_obj, len, stop_default)?;
    Ok((start, stop, step))
}

fn collect_slice_indices(start: isize, stop: isize, step: isize) -> Vec<usize> {
    let mut out = Vec::new();
    if step > 0 {
        let mut i = start;
        while i < stop {
            out.push(i as usize);
            i += step;
        }
    } else {
        let mut i = start;
        while i > stop {
            out.push(i as usize);
            i += step;
        }
    }
    out
}

#[no_mangle]
pub extern "C" fn molt_len(val: u64) -> u64 {
    let obj = obj_from_bits(val);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return MoltObject::from_int(string_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_BYTES {
                return MoltObject::from_int(bytes_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_BYTEARRAY {
                return MoltObject::from_int(bytes_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                return MoltObject::from_int(memoryview_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_LIST {
                return MoltObject::from_int(list_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_TUPLE {
                return MoltObject::from_int(tuple_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_INTARRAY {
                return MoltObject::from_int(intarray_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_DICT {
                return MoltObject::from_int(dict_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                return MoltObject::from_int(dict_view_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_RANGE {
                let len = range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr));
                return MoltObject::from_int(len).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_slice(obj_bits: u64, start_bits: u64, end_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let start_obj = obj_from_bits(start_bits);
    let end_obj = obj_from_bits(end_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let out = alloc_string(&[]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len as usize);
                let slice = &bytes[start as usize..end as usize];
                let out = alloc_string(slice);
                if out.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out).bits();
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let out = alloc_bytes(&[]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                let slice = &bytes[start as usize..end as usize];
                let out = alloc_bytes(slice);
                if out.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out).bits();
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let out = alloc_bytearray(&[]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                let slice = &bytes[start as usize..end as usize];
                let out = alloc_bytearray(slice);
                if out.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out).bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                let len = memoryview_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let base_offset = memoryview_offset(ptr);
                    let stride = memoryview_stride(ptr);
                    let out_ptr = alloc_memoryview(
                        memoryview_owner_bits(ptr),
                        base_offset + start * stride,
                        0,
                        memoryview_itemsize(ptr),
                        stride,
                        memoryview_readonly(ptr),
                    );
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let base_offset = memoryview_offset(ptr);
                let new_offset = base_offset + start * memoryview_stride(ptr);
                let new_len = (end - start) as usize;
                let out_ptr = alloc_memoryview(
                    memoryview_owner_bits(ptr),
                    new_offset,
                    new_len,
                    memoryview_itemsize(ptr),
                    memoryview_stride(ptr),
                    memoryview_readonly(ptr),
                );
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_LIST {
                let len = list_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let out = alloc_list(&[]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                let elems = seq_vec_ref(ptr);
                let slice = &elems[start as usize..end as usize];
                let out = alloc_list(slice);
                if out.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out).bits();
            }
            if type_id == TYPE_ID_TUPLE {
                let len = tuple_len(ptr) as isize;
                let start = match decode_slice_bound(start_obj, len, 0) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                let end = match decode_slice_bound(end_obj, len, len) {
                    Ok(v) => v,
                    Err(err) => return slice_error(err),
                };
                if end < start {
                    let out = alloc_tuple(&[]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                let elems = seq_vec_ref(ptr);
                let slice = &elems[start as usize..end as usize];
                let out = alloc_tuple(slice);
                if out.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_find(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
            {
                return MoltObject::none().bits();
            }
            let hay_len = string_len(hay_ptr);
            let needle_len = string_len(needle_ptr);
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
            let needle_bytes = std::slice::from_raw_parts(string_bytes(needle_ptr), needle_len);
            if needle_bytes.is_empty() {
                return MoltObject::from_int(0).bits();
            }
            let idx = bytes_find_impl(hay_bytes, needle_bytes);
            if idx < 0 {
                return MoltObject::from_int(idx).bits();
            }
            if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                return MoltObject::from_int(idx).bits();
            }
            let char_idx =
                utf8_byte_to_char_index_cached(hay_bytes, idx as usize, Some(hay_ptr as usize));
            return MoltObject::from_int(char_idx).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
            {
                raise!("TypeError", "startswith expects str arguments");
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let ok = if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                hay_bytes.starts_with(needle_bytes)
            } else {
                let hay_str = std::str::from_utf8_unchecked(hay_bytes);
                let needle_str = std::str::from_utf8_unchecked(needle_bytes);
                hay_str.starts_with(needle_str)
            };
            return MoltObject::from_bool(ok).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
            {
                raise!("TypeError", "endswith expects str arguments");
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let ok = if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                hay_bytes.ends_with(needle_bytes)
            } else {
                let hay_str = std::str::from_utf8_unchecked(hay_bytes);
                let needle_str = std::str::from_utf8_unchecked(needle_bytes);
                hay_str.ends_with(needle_str)
            };
            return MoltObject::from_bool(ok).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_count(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
            {
                raise!("TypeError", "count expects str arguments");
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            if needle_bytes.is_empty() {
                let count = utf8_codepoint_count_cached(hay_bytes, Some(hay_ptr as usize)) + 1;
                return MoltObject::from_int(count).bits();
            }
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            return MoltObject::from_int(count).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_join(sep_bits: u64, items_bits: u64) -> u64 {
    let sep = obj_from_bits(sep_bits);
    let items = obj_from_bits(items_bits);
    let sep_ptr = match sep.as_ptr() {
        Some(ptr) => ptr,
        None => return MoltObject::none().bits(),
    };
    unsafe {
        if object_type_id(sep_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "join expects a str separator");
        }
        let elems = match items.as_ptr() {
            Some(ptr) => {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    seq_vec_ref(ptr)
                } else {
                    raise!("TypeError", "join expects a list or tuple of str");
                }
            }
            None => raise!("TypeError", "join expects a list or tuple of str"),
        };
        let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
        let mut total_len = 0usize;
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let elem_ptr = match elem_obj.as_ptr() {
                Some(ptr) => ptr,
                None => raise!("TypeError", "join expects a list or tuple of str"),
            };
            if object_type_id(elem_ptr) != TYPE_ID_STRING {
                raise!("TypeError", "join expects a list or tuple of str");
            }
            total_len += string_len(elem_ptr);
        }
        if !elems.is_empty() {
            total_len = total_len.saturating_add(sep_bytes.len() * (elems.len() - 1));
        }
        let out_ptr = alloc_bytes_like_with_len(total_len, TYPE_ID_STRING);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
        let out_slice = std::slice::from_raw_parts_mut(data_ptr, total_len);
        let mut offset = 0usize;
        for (idx, &elem_bits) in elems.iter().enumerate() {
            if idx > 0 {
                let end = offset + sep_bytes.len();
                out_slice[offset..end].copy_from_slice(sep_bytes);
                offset = end;
            }
            let elem_ptr = obj_from_bits(elem_bits).as_ptr().unwrap();
            let elem_bytes =
                std::slice::from_raw_parts(string_bytes(elem_ptr), string_len(elem_ptr));
            let end = offset + elem_bytes.len();
            out_slice[offset..end].copy_from_slice(elem_bytes);
            offset = end;
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    let spec_obj = obj_from_bits(spec_bits);
    let spec_ptr = match spec_obj.as_ptr() {
        Some(ptr) => ptr,
        None => raise!("TypeError", "format spec must be a str"),
    };
    unsafe {
        if object_type_id(spec_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "format spec must be a str");
        }
        let spec_bytes = std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
        let spec_text = match std::str::from_utf8(spec_bytes) {
            Ok(val) => val,
            Err(_) => raise!("ValueError", "format spec must be valid UTF-8"),
        };
        let spec = match parse_format_spec(spec_text) {
            Ok(val) => val,
            Err(msg) => raise!("ValueError", msg),
        };
        let obj = obj_from_bits(val_bits);
        let rendered = match format_with_spec(obj, &spec) {
            Ok(val) => val,
            Err((kind, msg)) => raise!(kind, msg),
        };
        let out_ptr = alloc_string(rendered.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_bytes_find(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let idx = bytes_find_impl(hay_bytes, needle_bytes);
            return MoltObject::from_int(idx).bits();
        }
    }
    MoltObject::none().bits()
}

fn bytes_find_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return 0;
    }
    if needle_bytes.len() == 1 {
        return memchr(needle_bytes[0], hay_bytes)
            .map(|v| v as i64)
            .unwrap_or(-1);
    }
    if needle_bytes.len() <= 4 {
        return bytes_find_short(hay_bytes, needle_bytes)
            .map(|v| v as i64)
            .unwrap_or(-1);
    }
    memmem::find(hay_bytes, needle_bytes)
        .map(|v| v as i64)
        .unwrap_or(-1)
}

fn bytes_count_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return hay_bytes.len() as i64 + 1;
    }
    if needle_bytes.len() == 1 {
        return memchr::memchr_iter(needle_bytes[0], hay_bytes).count() as i64;
    }
    if needle_bytes.len() <= 4 {
        return bytes_count_short(hay_bytes, needle_bytes);
    }
    let finder = memmem::Finder::new(needle_bytes);
    finder.find_iter(hay_bytes).count() as i64
}

fn bytes_find_short(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let needle_len = needle.len();
    let first = needle[0];
    let mut search = 0usize;
    match needle_len {
        2 => {
            let b1 = needle[1];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 1 < hay.len() && hay[pos + 1] == b1 {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        3 => {
            let b1 = needle[1];
            let b2 = needle[2];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 2 < hay.len() && hay[pos + 1] == b1 && hay[pos + 2] == b2 {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        4 => {
            let b1 = needle[1];
            let b2 = needle[2];
            let b3 = needle[3];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 3 < hay.len()
                    && hay[pos + 1] == b1
                    && hay[pos + 2] == b2
                    && hay[pos + 3] == b3
                {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
        _ => {
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + needle_len <= hay.len() && &hay[pos..pos + needle_len] == needle {
                    return Some(pos);
                }
                search = pos + 1;
            }
        }
    }
    None
}

fn bytes_count_short(hay: &[u8], needle: &[u8]) -> i64 {
    let needle_len = needle.len();
    let first = needle[0];
    let mut count = 0i64;
    let mut search = 0usize;
    match needle_len {
        2 => {
            let b1 = needle[1];
            let mut next_allowed = 0usize;
            for pos in memchr::memchr_iter(first, hay) {
                if pos < next_allowed {
                    continue;
                }
                if pos + 1 < hay.len() && hay[pos + 1] == b1 {
                    count += 1;
                    next_allowed = pos + 2;
                }
            }
        }
        3 => {
            let b1 = needle[1];
            let b2 = needle[2];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 2 < hay.len() && hay[pos + 1] == b1 && hay[pos + 2] == b2 {
                    count += 1;
                    search = pos + 3;
                } else {
                    search = pos + 1;
                }
            }
        }
        4 => {
            let b1 = needle[1];
            let b2 = needle[2];
            let b3 = needle[3];
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + 3 < hay.len()
                    && hay[pos + 1] == b1
                    && hay[pos + 2] == b2
                    && hay[pos + 3] == b3
                {
                    count += 1;
                    search = pos + 4;
                } else {
                    search = pos + 1;
                }
            }
        }
        _ => {
            while let Some(idx) = memchr_fast(first, &hay[search..]) {
                let pos = search + idx;
                if pos + needle_len <= hay.len() && &hay[pos..pos + needle_len] == needle {
                    count += 1;
                    search = pos + needle_len;
                } else {
                    search = pos + 1;
                }
            }
        }
    }
    count
}

fn build_utf8_cache(bytes: &[u8]) -> Utf8IndexCache {
    let mut prefix = Vec::new();
    let mut total = 0i64;
    let mut idx = 0usize;
    prefix.push(0);
    while idx < bytes.len() {
        let end = (idx + UTF8_CACHE_BLOCK).min(bytes.len());
        total += simdutf::count_utf8(&bytes[idx..end]) as i64;
        prefix.push(total);
        idx = end;
    }
    Utf8IndexCache {
        block: UTF8_CACHE_BLOCK,
        prefix,
    }
}

fn utf8_cache_get_or_build(key: usize, bytes: &[u8]) -> Option<Arc<Utf8IndexCache>> {
    if bytes.len() < UTF8_CACHE_MIN_LEN || bytes.is_ascii() {
        return None;
    }
    if let Ok(store) = UTF8_INDEX_CACHE.lock() {
        if let Some(cache) = store.get(key) {
            return Some(cache);
        }
    }
    let cache = Arc::new(build_utf8_cache(bytes));
    if let Ok(mut store) = UTF8_INDEX_CACHE.lock() {
        if let Some(existing) = store.get(key) {
            return Some(existing);
        }
        store.insert(key, cache.clone());
    }
    Some(cache)
}

fn utf8_cache_remove(key: usize) {
    if let Ok(mut store) = UTF8_INDEX_CACHE.lock() {
        store.remove(key);
    }
}

fn utf8_count_prefix_cached(bytes: &[u8], cache: &Utf8IndexCache, prefix_len: usize) -> i64 {
    let prefix_len = prefix_len.min(bytes.len());
    let block_idx = prefix_len / cache.block;
    let mut total = *cache.prefix.get(block_idx).unwrap_or(&0);
    let start = block_idx * cache.block;
    if start < prefix_len {
        total += simdutf::count_utf8(&bytes[start..prefix_len]) as i64;
    }
    total
}

fn utf8_codepoint_count_cached(bytes: &[u8], cache_key: Option<usize>) -> i64 {
    if bytes.is_ascii() {
        return bytes.len() as i64;
    }
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(key, bytes) {
            return *cache.prefix.last().unwrap_or(&0);
        }
    }
    utf8_count_prefix_blocked(bytes, bytes.len())
}

fn utf8_byte_to_char_index_cached(bytes: &[u8], byte_idx: usize, cache_key: Option<usize>) -> i64 {
    if byte_idx == 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return byte_idx.min(bytes.len()) as i64;
    }
    let prefix_len = byte_idx.min(bytes.len());
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(key, bytes) {
            return utf8_count_prefix_cached(bytes, &cache, prefix_len);
        }
    }
    utf8_count_prefix_blocked(bytes, prefix_len)
}

fn utf8_count_prefix_blocked(bytes: &[u8], prefix_len: usize) -> i64 {
    const BLOCK: usize = 4096;
    let mut total = 0i64;
    let mut idx = 0usize;
    while idx + BLOCK <= prefix_len {
        total += simdutf::count_utf8(&bytes[idx..idx + BLOCK]) as i64;
        idx += BLOCK;
    }
    if idx < prefix_len {
        total += simdutf::count_utf8(&bytes[idx..prefix_len]) as i64;
    }
    total
}

fn memchr_fast(needle: u8, hay: &[u8]) -> Option<usize> {
    let (supported, idx) = memchr_simd128(needle, hay);
    if supported {
        return idx;
    }
    memchr(needle, hay)
}

#[cfg(target_arch = "wasm32")]
fn memchr_simd128(needle: u8, hay: &[u8]) -> (bool, Option<usize>) {
    if !std::arch::is_wasm_feature_detected!("simd128") {
        return (false, None);
    }
    unsafe {
        use std::arch::wasm32::*;
        let mut idx = 0usize;
        let needle_vec = u8x16_splat(needle);
        while idx + 16 <= hay.len() {
            let chunk = v128_load(hay.as_ptr().add(idx) as *const v128);
            let mask = u8x16_eq(chunk, needle_vec);
            let bits = u8x16_bitmask(mask) as u32;
            if bits != 0 {
                return (true, Some(idx + bits.trailing_zeros() as usize));
            }
            idx += 16;
        }
        if idx < hay.len() {
            if let Some(tail_idx) = memchr(needle, &hay[idx..]) {
                return (true, Some(idx + tail_idx));
            }
        }
    }
    (true, None)
}

#[cfg(not(target_arch = "wasm32"))]
fn memchr_simd128(_needle: u8, _hay: &[u8]) -> (bool, Option<usize>) {
    (false, None)
}

#[no_mangle]
pub extern "C" fn molt_bytearray_find(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let idx = bytes_find_impl(hay_bytes, needle_bytes);
            return MoltObject::from_int(idx).bits();
        }
    }
    MoltObject::none().bits()
}

fn replace_bytes_impl(hay: &[u8], needle: &[u8], replacement: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() {
        let mut out = Vec::with_capacity(hay.len() + replacement.len() * (hay.len() + 1));
        out.extend_from_slice(replacement);
        for &b in hay {
            out.push(b);
            out.extend_from_slice(replacement);
        }
        return Some(out);
    }
    if needle.len() == 1 {
        let needle_byte = needle[0];
        if replacement.len() == 1 {
            let mut out = hay.to_vec();
            let repl = replacement[0];
            if repl != needle_byte {
                for byte in &mut out {
                    if *byte == needle_byte {
                        *byte = repl;
                    }
                }
            }
            return Some(out);
        }
        let mut count = 0usize;
        let mut offset = 0usize;
        while let Some(idx) = memchr(needle_byte, &hay[offset..]) {
            count += 1;
            offset += idx + 1;
        }
        let extra = replacement.len().saturating_sub(1) * count;
        let mut out = Vec::with_capacity(hay.len().saturating_add(extra));
        let mut start = 0usize;
        let mut search = 0usize;
        while let Some(idx) = memchr(needle_byte, &hay[search..]) {
            let absolute = search + idx;
            out.extend_from_slice(&hay[start..absolute]);
            out.extend_from_slice(replacement);
            start = absolute + 1;
            search = start;
        }
        out.extend_from_slice(&hay[start..]);
        return Some(out);
    }
    let mut out = Vec::with_capacity(hay.len());
    let finder = memmem::Finder::new(needle);
    let mut start = 0usize;
    for idx in finder.find_iter(hay) {
        out.extend_from_slice(&hay[start..idx]);
        out.extend_from_slice(replacement);
        start = idx + needle.len();
    }
    out.extend_from_slice(&hay[start..]);
    Some(out)
}

fn replace_string_impl(
    hay_ptr: *mut u8,
    hay_bytes: &[u8],
    needle_bytes: &[u8],
    replacement_bytes: &[u8],
) -> Option<Vec<u8>> {
    if needle_bytes.is_empty() {
        if hay_bytes.is_ascii() && replacement_bytes.is_ascii() {
            return replace_bytes_impl(hay_bytes, needle_bytes, replacement_bytes);
        }
        let hay_str = unsafe { std::str::from_utf8_unchecked(hay_bytes) };
        let replacement_str = unsafe { std::str::from_utf8_unchecked(replacement_bytes) };
        let codepoints = utf8_codepoint_count_cached(hay_bytes, Some(hay_ptr as usize)) as usize;
        let mut out =
            String::with_capacity(hay_str.len() + replacement_str.len() * (codepoints + 1));
        out.push_str(replacement_str);
        for ch in hay_str.chars() {
            out.push(ch);
            out.push_str(replacement_str);
        }
        return Some(out.into_bytes());
    }
    replace_bytes_impl(hay_bytes, needle_bytes, replacement_bytes)
}

unsafe fn list_push_owned(list_ptr: *mut u8, val_bits: u64) {
    let elems = seq_vec(list_ptr);
    elems.push(val_bits);
}

fn alloc_list_empty_with_capacity(capacity: usize) -> *mut u8 {
    let cap = capacity.max(MAX_SMALL_LIST);
    alloc_list_with_capacity(&[], cap)
}

fn split_bytes_to_list<F>(hay: &[u8], needle: &[u8], mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let mut positions = Vec::new();
    if needle.len() == 1 {
        positions.extend(memchr::memchr_iter(needle[0], hay));
    } else {
        let finder = memmem::Finder::new(needle);
        positions.extend(finder.find_iter(hay));
    }
    let list_ptr = alloc_list_empty_with_capacity(positions.len() + 1);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    for idx in positions {
        let part = &hay[start..idx];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
        start = idx + needle.len();
    }
    let part = &hay[start..];
    let ptr = alloc(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return None;
    }
    unsafe {
        list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
    }
    Some(list_bits)
}

#[no_mangle]
pub extern "C" fn molt_string_split(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
            {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            if needle_bytes.is_empty() {
                raise!("ValueError", "empty separator");
            }
            let list_bits = split_bytes_to_list(hay_bytes, needle_bytes, alloc_string);
            let list_bits = match list_bits {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            return list_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    let replacement = obj_from_bits(replacement_bits);
    if let (Some(hay_ptr), Some(needle_ptr), Some(repl_ptr)) =
        (hay.as_ptr(), needle.as_ptr(), replacement.as_ptr())
    {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING
                || object_type_id(needle_ptr) != TYPE_ID_STRING
                || object_type_id(repl_ptr) != TYPE_ID_STRING
            {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let repl_bytes =
                std::slice::from_raw_parts(string_bytes(repl_ptr), string_len(repl_ptr));
            let out = match replace_string_impl(hay_ptr, hay_bytes, needle_bytes, repl_bytes) {
                Some(out) => out,
                None => return MoltObject::none().bits(),
            };
            let ptr = alloc_string(&out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bytes_split(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            if needle_bytes.is_empty() {
                raise!("ValueError", "empty separator");
            }
            let list_bits = match split_bytes_to_list(hay_bytes, needle_bytes, alloc_bytes) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            return list_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bytearray_split(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let (Some(hay_ptr), Some(needle_ptr)) = (hay.as_ptr(), needle.as_ptr()) {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            if needle_bytes.is_empty() {
                raise!("ValueError", "empty separator");
            }
            let list_bits = match split_bytes_to_list(hay_bytes, needle_bytes, alloc_bytearray) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            return list_bits;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bytes_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    let replacement = obj_from_bits(replacement_bits);
    if let (Some(hay_ptr), Some(needle_ptr), Some(repl_ptr)) =
        (hay.as_ptr(), needle.as_ptr(), replacement.as_ptr())
    {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let repl_bytes = match bytes_like_slice(repl_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let out = match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                Some(out) => out,
                None => return MoltObject::none().bits(),
            };
            let ptr = alloc_bytes(&out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bytearray_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    let replacement = obj_from_bits(replacement_bits);
    if let (Some(hay_ptr), Some(needle_ptr), Some(repl_ptr)) =
        (hay.as_ptr(), needle.as_ptr(), replacement.as_ptr())
    {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let repl_bytes = match bytes_like_slice(repl_ptr) {
                Some(slice) => slice,
                None => return MoltObject::none().bits(),
            };
            let out = match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                Some(out) => out,
                None => return MoltObject::none().bits(),
            };
            let ptr = alloc_bytearray(&out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_intarray_from_seq(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return MoltObject::none().bits();
            };
            let mut out = Vec::with_capacity(elems.len());
            for &elem in elems {
                let val = MoltObject::from_bits(elem);
                if let Some(i) = val.as_int() {
                    out.push(i);
                } else {
                    return MoltObject::none().bits();
                }
            }
            let out_ptr = alloc_intarray(&out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_bytearray_from_obj(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if let Some(slice) = bytes_like_slice(ptr) {
                let out_ptr = alloc_bytearray(slice);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if object_type_id(ptr) == TYPE_ID_MEMORYVIEW {
                let owner_bits = memoryview_owner_bits(ptr);
                let owner = obj_from_bits(owner_bits);
                let owner_ptr = match owner.as_ptr() {
                    Some(ptr) => ptr,
                    None => return MoltObject::none().bits(),
                };
                let base = match bytes_like_slice_raw(owner_ptr) {
                    Some(slice) => slice,
                    None => return MoltObject::none().bits(),
                };
                let offset = memoryview_offset(ptr);
                let len = memoryview_len(ptr);
                let itemsize = memoryview_itemsize(ptr);
                let stride = memoryview_stride(ptr);
                if offset < 0 {
                    return MoltObject::none().bits();
                }
                let mut out = Vec::with_capacity(len.saturating_mul(itemsize));
                for idx in 0..len {
                    let start = offset + (idx as isize) * stride;
                    if start < 0 {
                        return MoltObject::none().bits();
                    }
                    let start = start as usize;
                    let end = start.saturating_add(itemsize);
                    if end > base.len() {
                        return MoltObject::none().bits();
                    }
                    out.extend_from_slice(&base[start..end]);
                }
                let out_ptr = alloc_bytearray(&out);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_memoryview_new(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(ptr) => ptr,
        None => raise!("TypeError", "memoryview expects a bytes-like object"),
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_MEMORYVIEW {
            let owner_bits = memoryview_owner_bits(ptr);
            let offset = memoryview_offset(ptr);
            let len = memoryview_len(ptr);
            let itemsize = memoryview_itemsize(ptr);
            let stride = memoryview_stride(ptr);
            let readonly = memoryview_readonly(ptr);
            let out_ptr = alloc_memoryview(owner_bits, offset, len, itemsize, stride, readonly);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let readonly = type_id == TYPE_ID_BYTES;
            let out_ptr = alloc_memoryview(bits, 0, len, 1, 1, readonly);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
    }
    raise!("TypeError", "memoryview expects a bytes-like object");
}

#[no_mangle]
pub extern "C" fn molt_memoryview_tobytes(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(ptr) => ptr,
        None => raise!("TypeError", "tobytes expects a memoryview"),
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
            raise!("TypeError", "tobytes expects a memoryview");
        }
        let owner_bits = memoryview_owner_bits(ptr);
        let owner = obj_from_bits(owner_bits);
        let owner_ptr = match owner.as_ptr() {
            Some(ptr) => ptr,
            None => return MoltObject::none().bits(),
        };
        let base = match bytes_like_slice_raw(owner_ptr) {
            Some(slice) => slice,
            None => return MoltObject::none().bits(),
        };
        let offset = memoryview_offset(ptr);
        let len = memoryview_len(ptr);
        let itemsize = memoryview_itemsize(ptr);
        let stride = memoryview_stride(ptr);
        if offset < 0 {
            return MoltObject::none().bits();
        }
        let offset = offset as isize;
        let mut out = Vec::with_capacity(len.saturating_mul(itemsize));
        for idx in 0..len {
            let start = offset + (idx as isize) * stride;
            if start < 0 {
                return MoltObject::none().bits();
            }
            let start = start as usize;
            let end = start.saturating_add(itemsize);
            if end > base.len() {
                return MoltObject::none().bits();
            }
            out.extend_from_slice(&base[start..end]);
        }
        let out_ptr = alloc_bytes(&out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[repr(C)]
pub struct BufferExport {
    ptr: u64,
    len: u64,
    readonly: u64,
    stride: i64,
    itemsize: u64,
}

/// # Safety
/// Caller must ensure `out_ptr` is valid and writable.
#[no_mangle]
pub unsafe extern "C" fn molt_buffer_export(obj_bits: u64, out_ptr: *mut BufferExport) -> i32 {
    if out_ptr.is_null() {
        return 1;
    }
    let obj = obj_from_bits(obj_bits);
    let ptr = match obj.as_ptr() {
        Some(ptr) => ptr,
        None => return 1,
    };
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        let data_ptr = bytes_data(ptr) as u64;
        let len = bytes_len(ptr) as u64;
        let readonly = if type_id == TYPE_ID_BYTES { 1 } else { 0 };
        *out_ptr = BufferExport {
            ptr: data_ptr,
            len,
            readonly,
            stride: 1,
            itemsize: 1,
        };
        return 0;
    }
    if type_id == TYPE_ID_MEMORYVIEW {
        let owner_bits = memoryview_owner_bits(ptr);
        let owner = obj_from_bits(owner_bits);
        let owner_ptr = match owner.as_ptr() {
            Some(ptr) => ptr,
            None => return 1,
        };
        let base = match bytes_like_slice_raw(owner_ptr) {
            Some(slice) => slice,
            None => return 1,
        };
        let offset = memoryview_offset(ptr);
        if offset < 0 {
            return 1;
        }
        let offset = offset as usize;
        if offset > base.len() {
            return 1;
        }
        let data_ptr = base.as_ptr().add(offset) as u64;
        let len = memoryview_len(ptr) as u64;
        let readonly = if memoryview_readonly(ptr) { 1 } else { 0 };
        let stride = memoryview_stride(ptr) as i64;
        let itemsize = memoryview_itemsize(ptr) as u64;
        *out_ptr = BufferExport {
            ptr: data_ptr,
            len,
            readonly,
            stride,
            itemsize,
        };
        return 0;
    }
    1
}

#[no_mangle]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let key = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(slice_ptr) = key.as_ptr() {
                    if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                        let len = memoryview_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) =
                            match normalize_slice_indices(len, start_obj, stop_obj, step_obj) {
                                Ok(vals) => vals,
                                Err(err) => return slice_error(err),
                            };
                        let base_offset = memoryview_offset(ptr);
                        let base_stride = memoryview_stride(ptr);
                        let itemsize = memoryview_itemsize(ptr);
                        let new_offset = base_offset + start * base_stride;
                        let new_stride = base_stride * step;
                        let new_len = range_len_i64(start as i64, stop as i64, step as i64);
                        let new_len = new_len.max(0) as usize;
                        let out_ptr = alloc_memoryview(
                            memoryview_owner_bits(ptr),
                            new_offset,
                            new_len,
                            itemsize,
                            new_stride,
                            memoryview_readonly(ptr),
                        );
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
                if let Some(idx) = key.as_int() {
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    let base = match bytes_like_slice_raw(owner_ptr) {
                        Some(slice) => slice,
                        None => return MoltObject::none().bits(),
                    };
                    let len = memoryview_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let offset = memoryview_offset(ptr);
                    let stride = memoryview_stride(ptr);
                    let itemsize = memoryview_itemsize(ptr);
                    let start = offset + (i as isize) * stride;
                    if start < 0 {
                        return MoltObject::none().bits();
                    }
                    let start = start as usize;
                    let end = start.saturating_add(itemsize);
                    if end > base.len() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_int(base[start] as i64).bits();
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_STRING || type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY
            {
                if let Some(slice_ptr) = key.as_ptr() {
                    if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                        let bytes = if type_id == TYPE_ID_STRING {
                            std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
                        } else {
                            std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr))
                        };
                        let len = bytes.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) =
                            match normalize_slice_indices(len, start_obj, stop_obj, step_obj) {
                                Ok(vals) => vals,
                                Err(err) => return slice_error(err),
                            };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                if type_id == TYPE_ID_STRING {
                                    alloc_string(&[])
                                } else if type_id == TYPE_ID_BYTES {
                                    alloc_bytes(&[])
                                } else {
                                    alloc_bytearray(&[])
                                }
                            } else if type_id == TYPE_ID_STRING {
                                alloc_string(&bytes[s..e])
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(&bytes[s..e])
                            } else {
                                alloc_bytearray(&bytes[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(bytes[idx]);
                            }
                            if type_id == TYPE_ID_STRING {
                                alloc_string(&out)
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(&out)
                            } else {
                                alloc_bytearray(&out)
                            }
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_LIST {
                if let Some(slice_ptr) = key.as_ptr() {
                    if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) =
                            match normalize_slice_indices(len, start_obj, stop_obj, step_obj) {
                                Ok(vals) => vals,
                                Err(err) => return slice_error(err),
                            };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_list(&[])
                            } else {
                                alloc_list(&elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_list(out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
                if let Some(idx) = key.as_int() {
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    inc_ref_bits(val);
                    return val;
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_TUPLE {
                if let Some(slice_ptr) = key.as_ptr() {
                    if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) =
                            match normalize_slice_indices(len, start_obj, stop_obj, step_obj) {
                                Ok(vals) => vals,
                                Err(err) => return slice_error(err),
                            };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_tuple(&[])
                            } else {
                                alloc_tuple(&elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_tuple(out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
                if let Some(idx) = key.as_int() {
                    let len = tuple_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    inc_ref_bits(val);
                    return val;
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_RANGE {
                if let Some(idx) = key.as_int() {
                    let start = range_start(ptr);
                    let stop = range_stop(ptr);
                    let step = range_step(ptr);
                    let len = range_len_i64(start, stop, step);
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let val = start + step * i;
                    return MoltObject::from_int(val).bits();
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_DICT {
                if let Some(val) = dict_get_in_place(ptr, key_bits) {
                    inc_ref_bits(val);
                    return val;
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                if let Some(idx) = key.as_int() {
                    let dict_bits = dict_view_dict_bits(ptr);
                    let dict_obj = obj_from_bits(dict_bits);
                    if let Some(dict_ptr) = dict_obj.as_ptr() {
                        if object_type_id(dict_ptr) != TYPE_ID_DICT {
                            return MoltObject::none().bits();
                        }
                        let len = dict_len(dict_ptr) as i64;
                        let mut i = idx;
                        if i < 0 {
                            i += len;
                        }
                        if i < 0 || i >= len {
                            return MoltObject::none().bits();
                        }
                        if type_id == TYPE_ID_DICT_ITEMS_VIEW {
                            let order = dict_order(dict_ptr);
                            let entry = i as usize * 2;
                            let key_bits = order[entry];
                            let val_bits = order[entry + 1];
                            let out = alloc_tuple(&[key_bits, val_bits]);
                            if out.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(out).bits();
                        }
                        let order = dict_order(dict_ptr);
                        let entry = i as usize * 2;
                        let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                            order[entry]
                        } else {
                            order[entry + 1]
                        };
                        inc_ref_bits(val);
                        return val;
                    }
                }
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let key = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_LIST {
                if let Some(idx) = key.as_int() {
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec(ptr);
                    let old_bits = elems[i as usize];
                    if old_bits != val_bits {
                        dec_ref_bits(old_bits);
                        inc_ref_bits(val_bits);
                        elems[i as usize] = val_bits;
                    }
                    return obj_bits;
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_readonly(ptr) {
                    raise!("TypeError", "cannot modify read-only memory");
                }
                if let Some(slice_ptr) = key.as_ptr() {
                    if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                        let owner_bits = memoryview_owner_bits(ptr);
                        let owner = obj_from_bits(owner_bits);
                        let owner_ptr = match owner.as_ptr() {
                            Some(ptr) => ptr,
                            None => return MoltObject::none().bits(),
                        };
                        if object_type_id(owner_ptr) != TYPE_ID_BYTEARRAY {
                            raise!("TypeError", "memoryview is not writable");
                        }
                        if memoryview_itemsize(ptr) != 1 {
                            raise!("TypeError", "memoryview itemsize not supported");
                        }
                        let len = memoryview_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) =
                            match normalize_slice_indices(len, start_obj, stop_obj, step_obj) {
                                Ok(vals) => vals,
                                Err(err) => return slice_error(err),
                            };
                        let elem_count = range_len_i64(start as i64, stop as i64, step as i64);
                        let elem_count = elem_count.max(0) as usize;
                        let src_obj = obj_from_bits(val_bits);
                        let src_bytes = if let Some(src_ptr) = src_obj.as_ptr() {
                            let src_type = object_type_id(src_ptr);
                            if src_type == TYPE_ID_BYTES || src_type == TYPE_ID_BYTEARRAY {
                                let slice = bytes_like_slice_raw(src_ptr).unwrap_or(&[]);
                                slice.to_vec()
                            } else if src_type == TYPE_ID_MEMORYVIEW {
                                if let Some(slice) = memoryview_bytes_slice(src_ptr) {
                                    slice.to_vec()
                                } else {
                                    match memoryview_collect_bytes(src_ptr) {
                                        Some(buf) => buf,
                                        None => return MoltObject::none().bits(),
                                    }
                                }
                            } else {
                                raise!(
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(src_obj)
                                    ),
                                );
                            }
                        } else {
                            raise!(
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(src_obj)
                                ),
                            );
                        };
                        if src_bytes.len() != elem_count {
                            raise!(
                                "ValueError",
                                "memoryview assignment: lvalue and rvalue have different structures",
                            );
                        }
                        let offset = memoryview_offset(ptr);
                        let stride = memoryview_stride(ptr);
                        if offset < 0 {
                            return MoltObject::none().bits();
                        }
                        let base_len = bytes_len(owner_ptr) as isize;
                        let data_ptr = bytes_data(owner_ptr) as *mut u8;
                        let mut pos = offset + start * stride;
                        let step_stride = stride * step;
                        for byte in src_bytes {
                            if pos < 0 || pos >= base_len {
                                return MoltObject::none().bits();
                            }
                            *data_ptr.add(pos as usize) = byte;
                            pos += step_stride;
                        }
                        return obj_bits;
                    }
                }
                if let Some(idx) = key.as_int() {
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    if object_type_id(owner_ptr) != TYPE_ID_BYTEARRAY {
                        raise!("TypeError", "memoryview is not writable");
                    }
                    let len = memoryview_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    let itemsize = memoryview_itemsize(ptr);
                    if itemsize != 1 {
                        raise!("TypeError", "memoryview itemsize not supported");
                    }
                    let offset = memoryview_offset(ptr);
                    let stride = memoryview_stride(ptr);
                    let start = offset + (i as isize) * stride;
                    if start < 0 {
                        return MoltObject::none().bits();
                    }
                    let start = start as usize;
                    let base_len = bytes_len(owner_ptr);
                    if start >= base_len {
                        return MoltObject::none().bits();
                    }
                    let val = obj_from_bits(val_bits);
                    let byte = match val.as_int() {
                        Some(v) if (0..=255).contains(&v) => v as u8,
                        _ => raise!("TypeError", "memoryview item must be int 0-255"),
                    };
                    let data_ptr = bytes_data(owner_ptr) as *mut u8;
                    *data_ptr.add(start) = byte;
                    return obj_bits;
                }
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_DICT {
                dict_set_in_place(ptr, key_bits, val_bits);
                return obj_bits;
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_contains(container_bits: u64, item_bits: u64) -> u64 {
    let container = obj_from_bits(container_bits);
    let item = obj_from_bits(item_bits);
    if let Some(ptr) = container.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_LIST | TYPE_ID_TUPLE => {
                    let elems = seq_vec_ref(ptr);
                    for &elem_bits in elems.iter() {
                        if obj_eq(obj_from_bits(elem_bits), item) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                TYPE_ID_DICT => {
                    let order = dict_order(ptr);
                    let table = dict_table(ptr);
                    let found = dict_find_entry(order, table, item_bits).is_some();
                    return MoltObject::from_bool(found).bits();
                }
                TYPE_ID_STRING => {
                    let Some(item_ptr) = item.as_ptr() else {
                        raise!(
                            "TypeError",
                            &format!(
                                "'in <string>' requires string as left operand, not {}",
                                type_name(item)
                            ),
                        );
                    };
                    if object_type_id(item_ptr) != TYPE_ID_STRING {
                        raise!(
                            "TypeError",
                            &format!(
                                "'in <string>' requires string as left operand, not {}",
                                type_name(item)
                            ),
                        );
                    }
                    let hay_len = string_len(ptr);
                    let needle_len = string_len(item_ptr);
                    let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), hay_len);
                    let needle_bytes =
                        std::slice::from_raw_parts(string_bytes(item_ptr), needle_len);
                    if needle_bytes.is_empty() {
                        return MoltObject::from_bool(true).bits();
                    }
                    let idx = bytes_find_impl(hay_bytes, needle_bytes);
                    return MoltObject::from_bool(idx >= 0).bits();
                }
                TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                    let hay_len = bytes_len(ptr);
                    let hay_bytes = std::slice::from_raw_parts(bytes_data(ptr), hay_len);
                    if let Some(byte) = item.as_int() {
                        if !(0..=255).contains(&byte) {
                            raise!("ValueError", "byte must be in range(0, 256)");
                        }
                        let found = memchr(byte as u8, hay_bytes).is_some();
                        return MoltObject::from_bool(found).bits();
                    }
                    if let Some(item_ptr) = item.as_ptr() {
                        let item_type = object_type_id(item_ptr);
                        if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                            let needle_len = bytes_len(item_ptr);
                            let needle_bytes =
                                std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                            if needle_bytes.is_empty() {
                                return MoltObject::from_bool(true).bits();
                            }
                            let idx = bytes_find_impl(hay_bytes, needle_bytes);
                            return MoltObject::from_bool(idx >= 0).bits();
                        }
                    }
                    raise!(
                        "TypeError",
                        &format!("a bytes-like object is required, not '{}'", type_name(item)),
                    );
                }
                TYPE_ID_RANGE => {
                    let Some(val) = item.as_int() else {
                        return MoltObject::from_bool(false).bits();
                    };
                    let start = range_start(ptr);
                    let stop = range_stop(ptr);
                    let step = range_step(ptr);
                    if step == 0 {
                        return MoltObject::from_bool(false).bits();
                    }
                    let in_range = if step > 0 {
                        val >= start && val < stop
                    } else {
                        val <= start && val > stop
                    };
                    if !in_range {
                        return MoltObject::from_bool(false).bits();
                    }
                    let offset = val - start;
                    let step_abs = if step < 0 { -step } else { step };
                    let aligned = offset.rem_euclid(step_abs) == 0;
                    return MoltObject::from_bool(aligned).bits();
                }
                TYPE_ID_MEMORYVIEW => {
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => raise!(
                            "TypeError",
                            &format!("a bytes-like object is required, not '{}'", type_name(item)),
                        ),
                    };
                    let base = match bytes_like_slice_raw(owner_ptr) {
                        Some(slice) => slice,
                        None => raise!(
                            "TypeError",
                            &format!("a bytes-like object is required, not '{}'", type_name(item)),
                        ),
                    };
                    let offset = memoryview_offset(ptr);
                    let len = memoryview_len(ptr);
                    let itemsize = memoryview_itemsize(ptr);
                    let stride = memoryview_stride(ptr);
                    if offset < 0 {
                        raise!(
                            "TypeError",
                            &format!("a bytes-like object is required, not '{}'", type_name(item)),
                        );
                    }
                    if itemsize != 1 {
                        raise!("TypeError", "memoryview itemsize not supported");
                    }
                    if stride == 1 {
                        let start = offset as usize;
                        let end = start.saturating_add(len);
                        let hay = &base[start.min(base.len())..end.min(base.len())];
                        if let Some(byte) = item.as_int() {
                            if !(0..=255).contains(&byte) {
                                raise!("ValueError", "byte must be in range(0, 256)");
                            }
                            let found = memchr(byte as u8, hay).is_some();
                            return MoltObject::from_bool(found).bits();
                        }
                        if let Some(item_ptr) = item.as_ptr() {
                            let item_type = object_type_id(item_ptr);
                            if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                let needle_len = bytes_len(item_ptr);
                                let needle_bytes =
                                    std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                                if needle_bytes.is_empty() {
                                    return MoltObject::from_bool(true).bits();
                                }
                                let idx = bytes_find_impl(hay, needle_bytes);
                                return MoltObject::from_bool(idx >= 0).bits();
                            }
                        }
                        raise!(
                            "TypeError",
                            &format!("a bytes-like object is required, not '{}'", type_name(item)),
                        );
                    }
                    let mut out = Vec::with_capacity(len);
                    for idx in 0..len {
                        let start = offset + (idx as isize) * stride;
                        if start < 0 {
                            raise!(
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(item)
                                ),
                            );
                        }
                        let start = start as usize;
                        if start >= base.len() {
                            break;
                        }
                        out.push(base[start]);
                    }
                    let hay = out.as_slice();
                    if let Some(byte) = item.as_int() {
                        if !(0..=255).contains(&byte) {
                            raise!("ValueError", "byte must be in range(0, 256)");
                        }
                        let found = memchr(byte as u8, hay).is_some();
                        return MoltObject::from_bool(found).bits();
                    }
                    if let Some(item_ptr) = item.as_ptr() {
                        let item_type = object_type_id(item_ptr);
                        if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                            let needle_len = bytes_len(item_ptr);
                            let needle_bytes =
                                std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                            if needle_bytes.is_empty() {
                                return MoltObject::from_bool(true).bits();
                            }
                            let idx = bytes_find_impl(hay, needle_bytes);
                            return MoltObject::from_bool(idx >= 0).bits();
                        }
                    }
                    raise!(
                        "TypeError",
                        &format!("a bytes-like object is required, not '{}'", type_name(item)),
                    );
                }
                _ => {}
            }
        }
    }
    raise!(
        "TypeError",
        &format!(
            "argument of type '{}' is not iterable",
            type_name(container)
        ),
    );
}

#[no_mangle]
pub extern "C" fn molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    molt_store_index(dict_bits, key_bits, val_bits)
}

#[no_mangle]
pub extern "C" fn molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                if let Some(val) = dict_get_in_place(ptr, key_bits) {
                    inc_ref_bits(val);
                    return val;
                }
                inc_ref_bits(default_bits);
                return default_bits;
            }
        }
    }
    inc_ref_bits(default_bits);
    default_bits
}

#[no_mangle]
pub extern "C" fn molt_dict_pop(
    dict_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> u64 {
    let dict_obj = obj_from_bits(dict_bits);
    let has_default = obj_from_bits(has_default_bits).as_int().unwrap_or(0) != 0;
    if let Some(ptr) = dict_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                let order = dict_order(ptr);
                let table = dict_table(ptr);
                if let Some(entry_idx) = dict_find_entry(order, table, key_bits) {
                    let key_idx = entry_idx * 2;
                    let val_idx = key_idx + 1;
                    let key_val = order[key_idx];
                    let val_val = order[val_idx];
                    inc_ref_bits(val_val);
                    dec_ref_bits(key_val);
                    dec_ref_bits(val_val);
                    order.drain(key_idx..=val_idx);
                    let entries = order.len() / 2;
                    let capacity = dict_table_capacity(entries.max(1));
                    dict_rebuild(order, table, capacity);
                    return val_val;
                }
                if has_default {
                    inc_ref_bits(default_bits);
                    return default_bits;
                }
                return MoltObject::none().bits();
            }
        }
    }
    if has_default {
        inc_ref_bits(default_bits);
        return default_bits;
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_iter(iter_bits: u64) -> u64 {
    if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            let mut target_bits = iter_bits;
            let mut inc_target = true;
            if type_id == TYPE_ID_DICT {
                target_bits = molt_dict_keys(iter_bits);
                if obj_from_bits(target_bits).is_none() {
                    return MoltObject::none().bits();
                }
                inc_target = false;
            }
            if type_id == TYPE_ID_GENERATOR {
                inc_ref_bits(iter_bits);
                return iter_bits;
            }
            if type_id == TYPE_ID_LIST
                || type_id == TYPE_ID_TUPLE
                || type_id == TYPE_ID_DICT
                || type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
                || type_id == TYPE_ID_RANGE
            {
                let total = std::mem::size_of::<MoltHeader>()
                    + std::mem::size_of::<u64>()
                    + std::mem::size_of::<usize>();
                let iter_ptr = alloc_object(total, TYPE_ID_ITER);
                if iter_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                if inc_target {
                    inc_ref_bits(target_bits);
                }
                *(iter_ptr as *mut u64) = target_bits;
                iter_set_index(iter_ptr, 0);
                return MoltObject::from_ptr(iter_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_aiter(obj_bits: u64) -> u64 {
    unsafe {
        let obj = obj_from_bits(obj_bits);
        let name_ptr = alloc_string(b"__aiter__");
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
            dec_ref_bits(name_bits);
            let msg = format!("'{}' object is not async iterable", type_name(obj));
            raise!("TypeError", &msg);
        };
        let Some(call_bits) = attr_lookup_ptr(obj_ptr, name_bits) else {
            dec_ref_bits(name_bits);
            let msg = format!("'{}' object is not async iterable", type_name(obj));
            raise!("TypeError", &msg);
        };
        dec_ref_bits(name_bits);
        let res = call_callable0(call_bits);
        dec_ref_bits(call_bits);
        res
    }
}

#[no_mangle]
pub extern "C" fn molt_iter_next(iter_bits: u64) -> u64 {
    if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_GENERATOR {
                if generator_closed(ptr) {
                    return generator_done_tuple(MoltObject::none().bits());
                }
                generator_set_slot(ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
                let header = header_from_obj_ptr(ptr);
                let poll_fn_addr = (*header).poll_fn;
                if poll_fn_addr == 0 {
                    return generator_done_tuple(MoltObject::none().bits());
                }
                let poll_fn: extern "C" fn(*mut u8) -> i64 =
                    std::mem::transmute(poll_fn_addr as usize);
                let res = poll_fn(ptr);
                return res as u64;
            }
            if object_type_id(ptr) != TYPE_ID_ITER {
                return MoltObject::none().bits();
            }
            let target_bits = iter_target_bits(ptr);
            let target_obj = obj_from_bits(target_bits);
            let idx = iter_index(ptr);
            let (len, next_val, needs_drop) = if let Some(target_ptr) = target_obj.as_ptr() {
                let target_type = object_type_id(target_ptr);
                if target_type == TYPE_ID_LIST || target_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(target_ptr);
                    if idx >= elems.len() {
                        (elems.len(), None, false)
                    } else {
                        (elems.len(), Some(elems[idx]), false)
                    }
                } else if target_type == TYPE_ID_RANGE {
                    let start = range_start(target_ptr);
                    let stop = range_stop(target_ptr);
                    let step = range_step(target_ptr);
                    let len = range_len_i64(start, stop, step) as usize;
                    if idx >= len {
                        (len, None, false)
                    } else {
                        let val = start + step * idx as i64;
                        let bits = MoltObject::from_int(val).bits();
                        (len, Some(bits), false)
                    }
                } else if target_type == TYPE_ID_DICT_KEYS_VIEW
                    || target_type == TYPE_ID_DICT_VALUES_VIEW
                    || target_type == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let len = dict_view_len(target_ptr);
                    if idx >= len {
                        (len, None, false)
                    } else if let Some((key_bits, val_bits)) = dict_view_entry(target_ptr, idx) {
                        if target_type == TYPE_ID_DICT_ITEMS_VIEW {
                            let tuple_ptr = alloc_tuple(&[key_bits, val_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            (len, Some(MoltObject::from_ptr(tuple_ptr).bits()), true)
                        } else if target_type == TYPE_ID_DICT_KEYS_VIEW {
                            (len, Some(key_bits), false)
                        } else {
                            (len, Some(val_bits), false)
                        }
                    } else {
                        (len, None, false)
                    }
                } else {
                    (0, None, false)
                }
            } else {
                (0, None, false)
            };

            if let Some(val_bits) = next_val {
                iter_set_index(ptr, idx + 1);
                let done_bits = MoltObject::from_bool(false).bits();
                let tuple_ptr = alloc_tuple(&[val_bits, done_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                if needs_drop {
                    dec_ref_bits(val_bits);
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            if idx >= len {
                iter_set_index(ptr, len);
            }
            let none_bits = MoltObject::none().bits();
            let done_bits = MoltObject::from_bool(true).bits();
            let tuple_ptr = alloc_tuple(&[none_bits, done_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_anext(obj_bits: u64) -> u64 {
    unsafe {
        let obj = obj_from_bits(obj_bits);
        let name_ptr = alloc_string(b"__anext__");
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
            dec_ref_bits(name_bits);
            let msg = format!("'{}' object is not an async iterator", type_name(obj));
            raise!("TypeError", &msg);
        };
        let Some(call_bits) = attr_lookup_ptr(obj_ptr, name_bits) else {
            dec_ref_bits(name_bits);
            let msg = format!("'{}' object is not an async iterator", type_name(obj));
            raise!("TypeError", &msg);
        };
        dec_ref_bits(name_bits);
        let res = call_callable0(call_bits);
        dec_ref_bits(call_bits);
        res
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_keys(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
                let view_ptr = alloc_object(total, TYPE_ID_DICT_KEYS_VIEW);
                if view_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let bits = MoltObject::from_ptr(ptr).bits();
                inc_ref_bits(bits);
                *(view_ptr as *mut u64) = bits;
                return MoltObject::from_ptr(view_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_values(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
                let view_ptr = alloc_object(total, TYPE_ID_DICT_VALUES_VIEW);
                if view_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let bits = MoltObject::from_ptr(ptr).bits();
                inc_ref_bits(bits);
                *(view_ptr as *mut u64) = bits;
                return MoltObject::from_ptr(view_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_items(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
                let view_ptr = alloc_object(total, TYPE_ID_DICT_ITEMS_VIEW);
                if view_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let bits = MoltObject::from_ptr(ptr).bits();
                inc_ref_bits(bits);
                *(view_ptr as *mut u64) = bits;
                return MoltObject::from_ptr(view_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_append(list_bits: u64, val_bits: u64) -> u64 {
    let obj = obj_from_bits(list_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST {
                let elems = seq_vec(ptr);
                elems.push(val_bits);
                inc_ref_bits(val_bits);
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_pop(list_bits: u64, index_bits: u64) -> u64 {
    let obj = obj_from_bits(list_bits);
    let index_obj = obj_from_bits(index_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST {
                let len = list_len(ptr) as i64;
                if len == 0 {
                    return MoltObject::none().bits();
                }
                let mut idx = if index_obj.is_none() {
                    len - 1
                } else if let Some(i) = index_obj.as_int() {
                    i
                } else {
                    return MoltObject::none().bits();
                };
                if idx < 0 {
                    idx += len;
                }
                if idx < 0 || idx >= len {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec(ptr);
                let idx_usize = idx as usize;
                let value = elems.remove(idx_usize);
                inc_ref_bits(value);
                dec_ref_bits(value);
                return value;
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_extend(list_bits: u64, other_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            let other_obj = obj_from_bits(other_bits);
            if let Some(other_ptr) = other_obj.as_ptr() {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_LIST || other_type == TYPE_ID_TUPLE {
                    let src = seq_vec_ref(other_ptr);
                    for &item in src.iter() {
                        elems.push(item);
                        inc_ref_bits(item);
                    }
                    return MoltObject::none().bits();
                }
                if other_type == TYPE_ID_DICT {
                    let order = dict_order(other_ptr);
                    for idx in (0..order.len()).step_by(2) {
                        let key_bits = order[idx];
                        elems.push(key_bits);
                        inc_ref_bits(key_bits);
                    }
                    return MoltObject::none().bits();
                }
                if other_type == TYPE_ID_DICT_KEYS_VIEW
                    || other_type == TYPE_ID_DICT_VALUES_VIEW
                    || other_type == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let len = dict_view_len(other_ptr);
                    for idx in 0..len {
                        if let Some((key_bits, val_bits)) = dict_view_entry(other_ptr, idx) {
                            if other_type == TYPE_ID_DICT_ITEMS_VIEW {
                                let tuple_ptr = alloc_tuple(&[key_bits, val_bits]);
                                if tuple_ptr.is_null() {
                                    return MoltObject::none().bits();
                                }
                                elems.push(MoltObject::from_ptr(tuple_ptr).bits());
                            } else {
                                let item = if other_type == TYPE_ID_DICT_KEYS_VIEW {
                                    key_bits
                                } else {
                                    val_bits
                                };
                                elems.push(item);
                                inc_ref_bits(item);
                            }
                        }
                    }
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_insert(list_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let index_obj = obj_from_bits(index_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) == TYPE_ID_LIST {
                let len = list_len(list_ptr) as i64;
                let mut idx = index_obj.as_int().unwrap_or_default();
                if idx < 0 {
                    idx += len;
                }
                if idx < 0 {
                    idx = 0;
                }
                if idx > len {
                    idx = len;
                }
                let elems = seq_vec(list_ptr);
                elems.insert(idx as usize, val_bits);
                inc_ref_bits(val_bits);
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_remove(list_bits: u64, val_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) == TYPE_ID_LIST {
                let elems = seq_vec(list_ptr);
                if let Some(pos) = elems
                    .iter()
                    .position(|bits| obj_eq(obj_from_bits(*bits), obj_from_bits(val_bits)))
                {
                    let removed = elems.remove(pos);
                    dec_ref_bits(removed);
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_count(list_bits: u64, val_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let mut count = 0i64;
                for &elem in elems.iter() {
                    if obj_eq(obj_from_bits(elem), obj_from_bits(val_bits)) {
                        count += 1;
                    }
                }
                return MoltObject::from_int(count).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_index(list_bits: u64, val_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                for (idx, elem) in elems.iter().enumerate() {
                    if obj_eq(obj_from_bits(*elem), obj_from_bits(val_bits)) {
                        return MoltObject::from_int(idx as i64).bits();
                    }
                }
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_tuple_count(tuple_bits: u64, val_bits: u64) -> u64 {
    let tuple_obj = obj_from_bits(tuple_bits);
    if let Some(ptr) = tuple_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut count = 0i64;
                for &elem in elems.iter() {
                    if obj_eq(obj_from_bits(elem), obj_from_bits(val_bits)) {
                        count += 1;
                    }
                }
                return MoltObject::from_int(count).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_tuple_index(tuple_bits: u64, val_bits: u64) -> u64 {
    let tuple_obj = obj_from_bits(tuple_bits);
    if let Some(ptr) = tuple_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                for (idx, elem) in elems.iter().enumerate() {
                    if obj_eq(obj_from_bits(*elem), obj_from_bits(val_bits)) {
                        return MoltObject::from_int(idx as i64).bits();
                    }
                }
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_print_obj(val: u64) {
    let obj = obj_from_bits(val);
    if let Some(i) = obj.as_int() {
        println!("{i}");
        return;
    }
    if let Some(f) = obj.as_float() {
        if f.fract() == 0.0 {
            println!("{f:.1}");
        } else {
            println!("{f}");
        }
        return;
    }
    if let Some(b) = obj.as_bool() {
        if b {
            println!("True");
        } else {
            println!("False");
        }
        return;
    }
    if obj.is_none() {
        println!("None");
        return;
    }
    if obj.is_pending() {
        println!("<pending>");
        return;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let s = String::from_utf8_lossy(bytes);
                println!("{s}");
                return;
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let s = format_bytes(bytes);
                println!("{s}");
                return;
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let s = format!("bytearray({})", format_bytes(bytes));
                println!("{s}");
                return;
            }
            if type_id == TYPE_ID_RANGE {
                let start = range_start(ptr);
                let stop = range_stop(ptr);
                let step = range_step(ptr);
                println!("{}", format_range(start, stop, step));
                return;
            }
            if type_id == TYPE_ID_SLICE {
                println!("{}", format_slice(ptr));
                return;
            }
            if type_id == TYPE_ID_EXCEPTION {
                println!("{}", format_exception_message(ptr));
                return;
            }
            if type_id == TYPE_ID_DATACLASS {
                println!("{}", format_dataclass(ptr));
                return;
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    println!("<buffer2d>");
                    return;
                }
                let buf = &*buf_ptr;
                println!("<buffer2d {}x{}>", buf.rows, buf.cols);
                return;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                let len = memoryview_len(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                println!("<memoryview len={len} stride={stride} readonly={readonly}>");
                return;
            }
            if type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("[");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push(']');
                println!("{out}");
                return;
            }
            if type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("(");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                if elems.len() == 1 {
                    out.push(',');
                }
                out.push(')');
                println!("{out}");
                return;
            }
            if type_id == TYPE_ID_DICT {
                let pairs = dict_order(ptr);
                let mut out = String::from("{");
                let mut idx = 0;
                let mut first = true;
                while idx + 1 < pairs.len() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&format_obj(obj_from_bits(pairs[idx])));
                    out.push_str(": ");
                    out.push_str(&format_obj(obj_from_bits(pairs[idx + 1])));
                    idx += 2;
                }
                out.push('}');
                println!("{out}");
                return;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let pairs = dict_order(dict_ptr);
                        let mut out = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                            String::from("dict_keys([")
                        } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                            String::from("dict_values([")
                        } else {
                            String::from("dict_items([")
                        };
                        let mut idx = 0;
                        let mut first = true;
                        while idx + 1 < pairs.len() {
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            if type_id == TYPE_ID_DICT_ITEMS_VIEW {
                                out.push('(');
                                out.push_str(&format_obj(obj_from_bits(pairs[idx])));
                                out.push_str(", ");
                                out.push_str(&format_obj(obj_from_bits(pairs[idx + 1])));
                                out.push(')');
                            } else {
                                let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                                    pairs[idx]
                                } else {
                                    pairs[idx + 1]
                                };
                                out.push_str(&format_obj(obj_from_bits(val)));
                            }
                            idx += 2;
                        }
                        out.push_str("])");
                        println!("{out}");
                        return;
                    }
                }
            }
            if type_id == TYPE_ID_ITER {
                println!("<iter>");
                return;
            }
        }
    }
    println!("<object>");
}

#[no_mangle]
pub extern "C" fn molt_print_newline() {
    println!();
}

fn format_float(f: f64) -> String {
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

fn format_range(start: i64, stop: i64, step: i64) -> String {
    if step == 1 {
        format!("range({start}, {stop})")
    } else {
        format!("range({start}, {stop}, {step})")
    }
}

fn format_slice(ptr: *mut u8) -> String {
    unsafe {
        let start = format_obj(obj_from_bits(slice_start_bits(ptr)));
        let stop = format_obj(obj_from_bits(slice_stop_bits(ptr)));
        let step = format_obj(obj_from_bits(slice_step_bits(ptr)));
        format!("slice({start}, {stop}, {step})")
    }
}

fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let len = string_len(ptr);
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
        Some(String::from_utf8_lossy(bytes).to_string())
    }
}

fn decode_string_list(obj: MoltObject) -> Option<Vec<String>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        let mut out = Vec::with_capacity(elems.len());
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let s = string_obj_to_owned(elem_obj)?;
            out.push(s);
        }
        Some(out)
    }
}

fn decode_value_list(obj: MoltObject) -> Option<Vec<u64>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        Some(elems.to_vec())
    }
}

fn format_exception(ptr: *mut u8) -> String {
    unsafe {
        let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(ptr)))
            .unwrap_or_else(|| "Exception".to_string());
        let message = string_obj_to_owned(obj_from_bits(exception_msg_bits(ptr)))
            .unwrap_or_else(|| "<error>".to_string());
        format!("{kind}: {message}")
    }
}

fn format_exception_message(ptr: *mut u8) -> String {
    unsafe { string_obj_to_owned(obj_from_bits(exception_msg_bits(ptr))).unwrap_or_default() }
}

fn format_dataclass(ptr: *mut u8) -> String {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(ptr);
        if desc_ptr.is_null() {
            return "<dataclass>".to_string();
        }
        let desc = &*desc_ptr;
        if !desc.repr {
            return format!("<{}>", desc.name);
        }
        let fields = dataclass_fields_ref(ptr);
        let mut out = String::new();
        out.push_str(&desc.name);
        out.push('(');
        for (idx, name) in desc.field_names.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(name);
            out.push('=');
            let val = fields
                .get(idx)
                .copied()
                .unwrap_or(MoltObject::none().bits());
            out.push_str(&format_obj(obj_from_bits(val)));
        }
        out.push(')');
        out
    }
}

fn format_obj_str(obj: MoltObject) -> String {
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return String::from_utf8_lossy(bytes).into_owned();
            }
            if type_id == TYPE_ID_EXCEPTION {
                return format_exception_message(ptr);
            }
        }
    }
    format_obj(obj)
}

fn format_obj(obj: MoltObject) -> String {
    if let Some(i) = obj.as_int() {
        return i.to_string();
    }
    if let Some(f) = obj.as_float() {
        return format_float(f);
    }
    if let Some(b) = obj.as_bool() {
        return if b {
            "True".to_string()
        } else {
            "False".to_string()
        };
    }
    if obj.is_none() {
        return "None".to_string();
    }
    if obj.is_pending() {
        return "<pending>".to_string();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let s = String::from_utf8_lossy(bytes);
                return format_string_repr(&s);
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format_bytes(bytes);
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format!("bytearray({})", format_bytes(bytes));
            }
            if type_id == TYPE_ID_RANGE {
                return format_range(range_start(ptr), range_stop(ptr), range_step(ptr));
            }
            if type_id == TYPE_ID_SLICE {
                return format_slice(ptr);
            }
            if type_id == TYPE_ID_EXCEPTION {
                return format_exception(ptr);
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return "<context_manager>".to_string();
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return "<file_handle>".to_string();
            }
            if type_id == TYPE_ID_FUNCTION {
                return "<function>".to_string();
            }
            if type_id == TYPE_ID_BOUND_METHOD {
                return "<bound_method>".to_string();
            }
            if type_id == TYPE_ID_GENERATOR {
                return "<generator>".to_string();
            }
            if type_id == TYPE_ID_MODULE {
                let name =
                    string_obj_to_owned(obj_from_bits(module_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<module>".to_string();
                }
                return format!("<module '{name}'>");
            }
            if type_id == TYPE_ID_TYPE {
                let name =
                    string_obj_to_owned(obj_from_bits(class_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<type>".to_string();
                }
                return format!("<class '{name}'>");
            }
            if type_id == TYPE_ID_CLASSMETHOD {
                return "<classmethod>".to_string();
            }
            if type_id == TYPE_ID_STATICMETHOD {
                return "<staticmethod>".to_string();
            }
            if type_id == TYPE_ID_PROPERTY {
                return "<property>".to_string();
            }
            if type_id == TYPE_ID_SUPER {
                return "<super>".to_string();
            }
            if type_id == TYPE_ID_DATACLASS {
                return format_dataclass(ptr);
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return "<buffer2d>".to_string();
                }
                let buf = &*buf_ptr;
                return format!("<buffer2d {}x{}>", buf.rows, buf.cols);
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                let len = memoryview_len(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                return format!("<memoryview len={len} stride={stride} readonly={readonly}>");
            }
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let mut out = String::from("intarray([");
                for (idx, val) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&val.to_string());
                }
                out.push_str("])");
                return out;
            }
            if type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("[");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push(']');
                return out;
            }
            if type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("(");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                if elems.len() == 1 {
                    out.push(',');
                }
                out.push(')');
                return out;
            }
            if type_id == TYPE_ID_DICT {
                let pairs = dict_order(ptr);
                let mut out = String::from("{");
                let mut idx = 0;
                let mut first = true;
                while idx + 1 < pairs.len() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&format_obj(obj_from_bits(pairs[idx])));
                    out.push_str(": ");
                    out.push_str(&format_obj(obj_from_bits(pairs[idx + 1])));
                    idx += 2;
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let pairs = dict_order(dict_ptr);
                        let mut out = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                            String::from("dict_keys([")
                        } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                            String::from("dict_values([")
                        } else {
                            String::from("dict_items([")
                        };
                        let mut idx = 0;
                        let mut first = true;
                        while idx + 1 < pairs.len() {
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            if type_id == TYPE_ID_DICT_ITEMS_VIEW {
                                out.push('(');
                                out.push_str(&format_obj(obj_from_bits(pairs[idx])));
                                out.push_str(", ");
                                out.push_str(&format_obj(obj_from_bits(pairs[idx + 1])));
                                out.push(')');
                            } else {
                                let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                                    pairs[idx]
                                } else {
                                    pairs[idx + 1]
                                };
                                out.push_str(&format_obj(obj_from_bits(val)));
                            }
                            idx += 2;
                        }
                        out.push_str("])");
                        return out;
                    }
                }
            }
            if type_id == TYPE_ID_ITER {
                return "<iter>".to_string();
            }
        }
    }
    "<object>".to_string()
}

fn format_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("b'");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out.push('\'');
    out
}

fn format_string_repr(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                for b in encoded.as_bytes() {
                    out.push_str(&format!("\\x{:02x}", b));
                }
            }
            _ => out.push(ch),
        }
    }
    out.push('\'');
    out
}

struct FormatSpec {
    fill: char,
    align: Option<char>,
    width: Option<usize>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_format_spec(spec: &str) -> Result<FormatSpec, &'static str> {
    if spec.is_empty() {
        return Ok(FormatSpec {
            fill: ' ',
            align: None,
            width: None,
            precision: None,
            ty: None,
        });
    }
    let mut chars = spec.chars().peekable();
    let mut fill = ' ';
    let mut align = None;
    let mut peeked = chars.clone();
    let first = peeked.next();
    let second = peeked.next();
    if let (Some(c1), Some(c2)) = (first, second) {
        if matches!(c2, '<' | '>' | '^') {
            fill = c1;
            align = Some(c2);
            chars.next();
            chars.next();
        } else if matches!(c1, '<' | '>' | '^') {
            align = Some(c1);
            chars.next();
        }
    } else if let Some(c1) = first {
        if matches!(c1, '<' | '>' | '^') {
            align = Some(c1);
            chars.next();
        }
    }

    if align.is_none() {
        if let Some('0') = chars.peek().copied() {
            fill = '0';
            align = Some('>');
            chars.next();
        }
    }

    let mut width_text = String::new();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            width_text.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    let width = if width_text.is_empty() {
        None
    } else {
        Some(
            width_text
                .parse::<usize>()
                .map_err(|_| "Invalid format width")?,
        )
    };

    let mut precision = None;
    if matches!(chars.peek(), Some('.')) {
        chars.next();
        let mut prec_text = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                prec_text.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if prec_text.is_empty() {
            return Err("Invalid format precision");
        }
        precision = Some(
            prec_text
                .parse::<usize>()
                .map_err(|_| "Invalid format precision")?,
        );
    }

    let remaining: String = chars.collect();
    if remaining.len() > 1 {
        return Err("Invalid format spec");
    }
    let ty = if remaining.is_empty() {
        None
    } else {
        Some(remaining.chars().next().unwrap())
    };

    Ok(FormatSpec {
        fill,
        align,
        width,
        precision,
        ty,
    })
}

fn format_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    let mut text = match spec.ty {
        Some('s') => format_obj_str(obj),
        Some('d') => {
            if let Some(i) = obj.as_int() {
                i.to_string()
            } else if let Some(b) = obj.as_bool() {
                if b {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            } else {
                return Err(("TypeError", "format d requires int"));
            }
        }
        Some('f') => {
            let val = if let Some(f) = obj.as_float() {
                f
            } else if let Some(i) = obj.as_int() {
                i as f64
            } else if let Some(b) = obj.as_bool() {
                if b {
                    1.0
                } else {
                    0.0
                }
            } else {
                return Err(("TypeError", "format f requires float"));
            };
            let prec = spec.precision.unwrap_or(6);
            format!("{:.*}", prec, val)
        }
        Some(_other) => {
            return Err(("ValueError", "unsupported format type"));
        }
        None => {
            if spec.precision.is_some() {
                if let Some(f) = obj.as_float() {
                    let prec = spec.precision.unwrap_or(6);
                    format!("{:.*}", prec, f)
                } else if let Some(i) = obj.as_int() {
                    let prec = spec.precision.unwrap_or(6);
                    format!("{:.*}", prec, i as f64)
                } else {
                    format_obj_str(obj)
                }
            } else {
                format_obj_str(obj)
            }
        }
    };

    let width = match spec.width {
        Some(val) => val,
        None => return Ok(text),
    };
    let len = text.chars().count();
    if len >= width {
        return Ok(text);
    }
    let pad_len = width - len;
    let align = spec
        .align
        .unwrap_or(if matches!(spec.ty, Some('d') | Some('f')) {
            '>'
        } else {
            '<'
        });
    let fill = spec.fill;
    let padding = fill.to_string().repeat(pad_len);
    text = match align {
        '<' => format!("{text}{padding}"),
        '>' => format!("{padding}{text}"),
        '^' => {
            let left = pad_len / 2;
            let right = pad_len - left;
            format!(
                "{}{}{}",
                fill.to_string().repeat(left),
                text,
                fill.to_string().repeat(right)
            )
        }
        _ => return Err(("ValueError", "invalid alignment")),
    };
    Ok(text)
}

// --- Reference Counting ---

/// # Safety
/// Dereferences raw pointer to increment ref count.
#[no_mangle]
pub unsafe extern "C" fn molt_inc_ref(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    (*header_ptr).ref_count += 1;
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[no_mangle]
pub unsafe extern "C" fn molt_dec_ref(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let header = &mut *header_ptr;
    header.ref_count -= 1;
    if header.ref_count == 0 {
        if header.type_id == TYPE_ID_OBJECT {
            unregister_raw_object(ptr);
        }
        match header.type_id {
            TYPE_ID_STRING => {
                utf8_cache_remove(ptr as usize);
            }
            TYPE_ID_LIST => {
                let vec_ptr = seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
            }
            TYPE_ID_TUPLE => {
                let vec_ptr = seq_vec_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
            }
            TYPE_ID_DICT => {
                let order_ptr = dict_order_ptr(ptr);
                let table_ptr = dict_table_ptr(ptr);
                if !order_ptr.is_null() {
                    let order = Box::from_raw(order_ptr);
                    for bits in order.iter() {
                        dec_ref_bits(*bits);
                    }
                }
                if !table_ptr.is_null() {
                    drop(Box::from_raw(table_ptr));
                }
            }
            TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_VALUES_VIEW | TYPE_ID_DICT_ITEMS_VIEW => {
                let dict_bits = dict_view_dict_bits(ptr);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_ITER => {
                let target_bits = iter_target_bits(ptr);
                dec_ref_bits(target_bits);
            }
            TYPE_ID_LIST_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_DICT_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_SLICE => {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                dec_ref_bits(start_bits);
                dec_ref_bits(stop_bits);
                dec_ref_bits(step_bits);
            }
            TYPE_ID_EXCEPTION => {
                let kind_bits = exception_kind_bits(ptr);
                let msg_bits = exception_msg_bits(ptr);
                let cause_bits = exception_cause_bits(ptr);
                let context_bits = exception_context_bits(ptr);
                let suppress_bits = exception_suppress_bits(ptr);
                dec_ref_bits(kind_bits);
                dec_ref_bits(msg_bits);
                dec_ref_bits(cause_bits);
                dec_ref_bits(context_bits);
                dec_ref_bits(suppress_bits);
            }
            TYPE_ID_GENERATOR => {
                generator_exception_stack_drop(ptr);
            }
            TYPE_ID_CONTEXT_MANAGER => {
                let payload_bits = context_payload_bits(ptr);
                dec_ref_bits(payload_bits);
            }
            TYPE_ID_FILE_HANDLE => {
                let handle_ptr = file_handle_ptr(ptr);
                if !handle_ptr.is_null() {
                    let handle_ref = &*handle_ptr;
                    if let Ok(mut guard) = handle_ref.file.lock() {
                        guard.take();
                    }
                    drop(Box::from_raw(handle_ptr));
                }
            }
            TYPE_ID_DATACLASS => {
                let vec_ptr = dataclass_fields_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
                let dict_bits = dataclass_dict_bits(ptr);
                dec_ref_bits(dict_bits);
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0 {
                        dec_ref_bits(class_bits);
                    }
                    drop(Box::from_raw(desc_ptr));
                }
            }
            TYPE_ID_OBJECT => {
                if header.poll_fn == 0 {
                    let payload = header.size - std::mem::size_of::<MoltHeader>();
                    if payload >= std::mem::size_of::<u64>() {
                        let dict_bits = instance_dict_bits(ptr);
                        dec_ref_bits(dict_bits);
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        dec_ref_bits(class_bits);
                    }
                }
            }
            TYPE_ID_BUFFER2D => {
                let buf_ptr = buffer2d_ptr(ptr);
                if !buf_ptr.is_null() {
                    drop(Box::from_raw(buf_ptr));
                }
            }
            TYPE_ID_MEMORYVIEW => {
                let owner_bits = memoryview_owner_bits(ptr);
                dec_ref_bits(owner_bits);
            }
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(ptr);
                let self_bits = bound_method_self_bits(ptr);
                dec_ref_bits(func_bits);
                dec_ref_bits(self_bits);
            }
            TYPE_ID_FUNCTION => {
                let dict_bits = function_dict_bits(ptr);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_MODULE => {
                let name_bits = module_name_bits(ptr);
                let dict_bits = module_dict_bits(ptr);
                dec_ref_bits(name_bits);
                dec_ref_bits(dict_bits);
            }
            TYPE_ID_TYPE => {
                let name_bits = class_name_bits(ptr);
                let dict_bits = class_dict_bits(ptr);
                let bases_bits = class_bases_bits(ptr);
                let mro_bits = class_mro_bits(ptr);
                dec_ref_bits(name_bits);
                dec_ref_bits(dict_bits);
                dec_ref_bits(bases_bits);
                dec_ref_bits(mro_bits);
            }
            TYPE_ID_CLASSMETHOD => {
                let func_bits = classmethod_func_bits(ptr);
                dec_ref_bits(func_bits);
            }
            TYPE_ID_STATICMETHOD => {
                let func_bits = staticmethod_func_bits(ptr);
                dec_ref_bits(func_bits);
            }
            TYPE_ID_PROPERTY => {
                let get_bits = property_get_bits(ptr);
                let set_bits = property_set_bits(ptr);
                let del_bits = property_del_bits(ptr);
                dec_ref_bits(get_bits);
                dec_ref_bits(set_bits);
                dec_ref_bits(del_bits);
            }
            TYPE_ID_SUPER => {
                let type_bits = super_type_bits(ptr);
                let obj_bits = super_obj_bits(ptr);
                dec_ref_bits(type_bits);
                dec_ref_bits(obj_bits);
            }
            _ => {}
        }
        let size = header.size;
        let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
        std::alloc::dealloc(header_ptr as *mut u8, layout);
    }
}

#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        unsafe { molt_inc_ref(ptr) };
    }
}

#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(bits: u64) {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        unsafe { molt_dec_ref(ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static EXIT_CALLED: AtomicBool = AtomicBool::new(false);

    extern "C" fn test_enter(payload_bits: u64) -> u64 {
        payload_bits
    }

    extern "C" fn test_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
        EXIT_CALLED.store(true, Ordering::SeqCst);
        MoltObject::from_bool(false).bits()
    }

    #[test]
    fn context_unwind_runs_exit() {
        EXIT_CALLED.store(false, Ordering::SeqCst);
        let ctx_bits = molt_context_new(
            test_enter as usize as u64,
            test_exit as usize as u64,
            MoltObject::none().bits(),
        );
        let _ = molt_context_enter(ctx_bits);
        let _ = molt_context_unwind(MoltObject::none().bits());
        assert!(EXIT_CALLED.load(Ordering::SeqCst));
        if let Some(ptr) = obj_from_bits(ctx_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
    }

    #[test]
    fn file_handle_close_marks_closed() {
        std::env::set_var("MOLT_CAPABILITIES", "fs.read,fs.write");
        let tmp_dir = std::env::temp_dir();
        let file_name = format!(
            "molt_test_{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = tmp_dir.join(file_name);
        let path_bytes = path.to_string_lossy().into_owned();

        let path_ptr = alloc_string(path_bytes.as_bytes());
        assert!(!path_ptr.is_null());
        let mode_ptr = alloc_string(b"w");
        assert!(!mode_ptr.is_null());
        let handle_bits = molt_file_open(
            MoltObject::from_ptr(path_ptr).bits(),
            MoltObject::from_ptr(mode_ptr).bits(),
        );
        let handle_obj = obj_from_bits(handle_bits);
        let Some(handle_ptr) = handle_obj.as_ptr() else {
            panic!("file handle missing");
        };
        unsafe {
            let fh_ptr = file_handle_ptr(handle_ptr);
            assert!(!fh_ptr.is_null());
            let fh = &*fh_ptr;
            let guard = fh.file.lock().unwrap();
            assert!(guard.is_some());
        }
        let _ = molt_file_close(handle_bits);
        unsafe {
            let fh_ptr = file_handle_ptr(handle_ptr);
            let fh = &*fh_ptr;
            let guard = fh.file.lock().unwrap();
            assert!(guard.is_none());
        }
        if let Some(ptr) = obj_from_bits(handle_bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
        unsafe {
            molt_dec_ref(path_ptr);
            molt_dec_ref(mode_ptr);
        }
        let _ = std::fs::remove_file(path);
    }
}

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len: usize) -> i64 {
    let s = {
        let slice = std::slice::from_raw_parts(ptr, len);
        std::str::from_utf8(slice).unwrap()
    };
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    v.as_i64().unwrap_or(0)
}

fn value_to_object(value: serde_json::Value, arena: &mut TempArena) -> Result<MoltObject, i32> {
    match value {
        serde_json::Value::Null => Ok(MoltObject::none()),
        serde_json::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MoltObject::from_int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(MoltObject::from_float(f))
            } else {
                Err(2)
            }
        }
        serde_json::Value::String(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if map.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = map.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in map.into_iter().enumerate() {
                let key_ptr = alloc_string(key.as_bytes());
                if key_ptr.is_null() {
                    return Err(2);
                }
                let val_obj = value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = MoltObject::from_ptr(key_ptr).bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
    }
}

fn msgpack_value_to_object(value: rmpv::Value, arena: &mut TempArena) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::F32(f) => Ok(MoltObject::from_float(f as f64)),
        rmpv::Value::F64(f) => Ok(MoltObject::from_float(f)),
        rmpv::Value::String(s) => {
            let s = s.as_str().ok_or(2)?;
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = msgpack_value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = msgpack_key_to_object(key)?;
                let val_obj = msgpack_value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn msgpack_key_to_object(value: rmpv::Value) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::String(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_value_to_object(
    value: serde_cbor::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            if i < i64::MIN as i128 || i > i64::MAX as i128 {
                return Err(2);
            }
            Ok(MoltObject::from_int(i as i64))
        }
        serde_cbor::Value::Float(f) => Ok(MoltObject::from_float(f)),
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = cbor_value_to_object(item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(&[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = cbor_key_to_object(key)?;
                let val_obj = cbor_value_to_object(value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_key_to_object(value: serde_cbor::Value) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            let i_val = i;
            if i_val < i64::MIN as i128 || i_val > i64::MAX as i128 {
                Err(2)
            } else {
                Ok(MoltObject::from_int(i_val as i64))
            }
        }
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(&b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

unsafe fn parse_json_scalar(
    ptr: *const u8,
    len: usize,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = std::str::from_utf8(slice).map_err(|_| 1)?;
    let v: serde_json::Value = serde_json::from_str(s).map_err(|_| 1)?;
    value_to_object(v, arena)
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_string_from_bytes(ptr: *const u8, len: usize, out: *mut u64) -> i32 {
    if out.is_null() {
        return 2;
    }
    if ptr.is_null() && len != 0 {
        return 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let obj_ptr = alloc_string(slice);
    if obj_ptr.is_null() {
        return 2;
    }
    *out = MoltObject::from_ptr(obj_ptr).bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_bytes_from_bytes(ptr: *const u8, len: usize, out: *mut u64) -> i32 {
    if out.is_null() {
        return 2;
    }
    if ptr.is_null() && len != 0 {
        return 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let obj_ptr = alloc_bytes(slice);
    if obj_ptr.is_null() {
        return 2;
    }
    *out = MoltObject::from_ptr(obj_ptr).bits();
    0
}

#[no_mangle]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
        Some(key) => key,
        None => return default_bits,
    };
    match std::env::var(key) {
        Ok(val) => {
            let ptr = alloc_string(val.as_bytes());
            if ptr.is_null() {
                default_bits
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
        Err(_) => default_bits,
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_scalar(ptr: *const u8, len: usize, out: *mut u64) -> i32 {
    if out.is_null() {
        return 2;
    }
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = parse_json_scalar(ptr, len, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    ptr: *const u8,
    len: usize,
    out: *mut u64,
) -> i32 {
    if out.is_null() {
        return 2;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let mut cursor = Cursor::new(slice);
    let v = match rmpv::decode::read_value(&mut cursor) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = msgpack_value_to_object(v, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_cbor_parse_scalar(ptr: *const u8, len: usize, out: *mut u64) -> i32 {
    if out.is_null() {
        return 2;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    let v: serde_cbor::Value = match serde_cbor::from_slice(slice) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let obj = PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = cbor_value_to_object(v, &mut arena);
        arena.reset();
        result
    });
    let obj = match obj {
        Ok(val) => val,
        Err(code) => return code,
    };
    *out = obj.bits();
    0
}

// --- Generic ---

fn attr_error(type_label: &str, attr_name: &str) -> i64 {
    let msg = format!("'{}' object has no attribute '{}'", type_label, attr_name);
    raise!("AttributeError", &msg);
}

fn property_no_setter(attr_name: &str, class_ptr: *mut u8) -> i64 {
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no setter");
    raise!("AttributeError", &msg);
}

fn descriptor_no_setter(attr_name: &str, class_ptr: *mut u8) -> i64 {
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    raise!("AttributeError", &msg);
}

fn maybe_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return Some(ptr);
    }
    let ptr = bits as *mut u8;
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>());
        let header = header_ptr as *const MoltHeader;
        if header.is_null() {
            return None;
        }
        if (*header).type_id == TYPE_ID_OBJECT || (*header).type_id == TYPE_ID_GENERATOR {
            return Some(ptr);
        }
    }
    None
}

unsafe fn call_function_obj1(func_bits: u64, arg0_bits: u64) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 1 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
    func(arg0_bits) as u64
}

unsafe fn call_function_obj0(func_bits: u64) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 0 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let func: extern "C" fn() -> i64 = std::mem::transmute(fn_ptr as usize);
    func() as u64
}

unsafe fn call_function_obj2(func_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 2 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
    func(arg0_bits, arg1_bits) as u64
}

unsafe fn call_function_obj3(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 3 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
    func(arg0_bits, arg1_bits, arg2_bits) as u64
}

unsafe fn call_callable0(call_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        raise!("TypeError", "object is not callable");
    };
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => call_function_obj0(call_bits),
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            call_function_obj1(func_bits, self_bits)
        }
        _ => raise!("TypeError", "object is not callable"),
    }
}

unsafe fn module_attr_lookup(ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    let dict_bits = module_dict_bits(ptr);
    let dict_obj = obj_from_bits(dict_bits);
    let dict_ptr = dict_obj.as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    dict_get_in_place(dict_ptr, attr_bits).inspect(|val| inc_ref_bits(*val))
}

unsafe fn instance_bits_for_call(ptr: *mut u8) -> u64 {
    if object_type_id(ptr) == TYPE_ID_OBJECT {
        ptr as u64
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

unsafe fn class_attr_lookup_raw_mro(class_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    if let Some(mro) = class_mro_ref(class_ptr) {
        for class_bits in mro.iter() {
            let class_obj = obj_from_bits(*class_bits);
            let Some(ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            if let Some(val_bits) = dict_get_in_place(dict_ptr, attr_bits) {
                return Some(val_bits);
            }
        }
        return None;
    }
    let mut current_ptr = class_ptr;
    let mut depth = 0usize;
    loop {
        let dict_bits = class_dict_bits(current_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = dict_obj.as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        if let Some(val_bits) = dict_get_in_place(dict_ptr, attr_bits) {
            return Some(val_bits);
        }
        let bases_bits = class_bases_bits(current_ptr);
        let bases = class_bases_vec(bases_bits);
        let Some(next_bits) = bases.first().copied() else {
            return None;
        };
        let next_obj = obj_from_bits(next_bits);
        let next_ptr = next_obj.as_ptr()?;
        if object_type_id(next_ptr) != TYPE_ID_TYPE {
            return None;
        }
        if next_ptr == current_ptr {
            return None;
        }
        current_ptr = next_ptr;
        depth += 1;
        if depth > 64 {
            return None;
        }
    }
}

unsafe fn descriptor_method_bits(val_bits: u64, name_bits: u64) -> Option<u64> {
    let class_bits = type_of_bits(val_bits);
    let class_obj = obj_from_bits(class_bits);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    class_attr_lookup_raw_mro(class_ptr, name_bits)
}

unsafe fn descriptor_has_method(val_bits: u64, name_bits: u64) -> bool {
    descriptor_method_bits(val_bits, name_bits).is_some()
}

unsafe fn descriptor_is_data(val_bits: u64) -> bool {
    let val_obj = obj_from_bits(val_bits);
    let Some(val_ptr) = val_obj.as_ptr() else {
        return false;
    };
    if object_type_id(val_ptr) == TYPE_ID_PROPERTY {
        return true;
    }
    let set_bits = intern_static_name(&INTERN_SET_NAME, b"__set__");
    let del_bits = intern_static_name(&INTERN_DELETE_NAME, b"__delete__");
    descriptor_has_method(val_bits, set_bits) || descriptor_has_method(val_bits, del_bits)
}

unsafe fn descriptor_bind(
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
) -> Option<u64> {
    let val_obj = obj_from_bits(val_bits);
    let Some(val_ptr) = val_obj.as_ptr() else {
        inc_ref_bits(val_bits);
        return Some(val_bits);
    };
    match object_type_id(val_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(inst_ptr) = instance_ptr {
                let inst_bits = instance_bits_for_call(inst_ptr);
                let bound_bits = molt_bound_method_new(val_bits, inst_bits);
                Some(bound_bits)
            } else {
                inc_ref_bits(val_bits);
                Some(val_bits)
            }
        }
        TYPE_ID_CLASSMETHOD => {
            let func_bits = classmethod_func_bits(val_ptr);
            if owner_ptr.is_null() {
                inc_ref_bits(func_bits);
                return Some(func_bits);
            }
            let class_bits = MoltObject::from_ptr(owner_ptr).bits();
            Some(molt_bound_method_new(func_bits, class_bits))
        }
        TYPE_ID_STATICMETHOD => {
            let func_bits = staticmethod_func_bits(val_ptr);
            inc_ref_bits(func_bits);
            Some(func_bits)
        }
        TYPE_ID_PROPERTY => {
            if let Some(inst_ptr) = instance_ptr {
                let get_bits = property_get_bits(val_ptr);
                if obj_from_bits(get_bits).is_none() {
                    raise!("AttributeError", "unreadable property");
                }
                let inst_bits = instance_bits_for_call(inst_ptr);
                let value_bits = call_function_obj1(get_bits, inst_bits);
                Some(value_bits)
            } else {
                inc_ref_bits(val_bits);
                Some(val_bits)
            }
        }
        _ => {
            let get_bits = intern_static_name(&INTERN_GET_NAME, b"__get__");
            if let Some(method_bits) = descriptor_method_bits(val_bits, get_bits) {
                let method_obj = obj_from_bits(method_bits);
                let Some(method_ptr) = method_obj.as_ptr() else {
                    raise!("TypeError", "__get__ must be a function");
                };
                if object_type_id(method_ptr) != TYPE_ID_FUNCTION {
                    raise!("TypeError", "__get__ must be a function");
                }
                let self_bits = MoltObject::from_ptr(val_ptr).bits();
                let inst_bits = instance_ptr
                    .map(|ptr| instance_bits_for_call(ptr))
                    .unwrap_or_else(|| MoltObject::none().bits());
                let owner_bits = if owner_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(owner_ptr).bits()
                };
                let res = call_function_obj3(method_bits, self_bits, inst_bits, owner_bits);
                return Some(res);
            }
            inc_ref_bits(val_bits);
            Some(val_bits)
        }
    }
}

unsafe fn class_attr_lookup(
    class_ptr: *mut u8,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    attr_bits: u64,
) -> Option<u64> {
    let val_bits = class_attr_lookup_raw_mro(class_ptr, attr_bits)?;
    descriptor_bind(val_bits, owner_ptr, instance_ptr)
}

unsafe fn attr_lookup_ptr(obj_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        return module_attr_lookup(obj_ptr, attr_bits);
    }
    if type_id == TYPE_ID_EXCEPTION {
        let name = string_obj_to_owned(obj_from_bits(attr_bits));
        let attr_name = name.as_deref()?;
        match attr_name {
            "__cause__" => {
                let bits = exception_cause_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__context__" => {
                let bits = exception_context_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "__suppress_context__" => {
                let bits = exception_suppress_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            _ => {}
        }
    }
    if type_id == TYPE_ID_TYPE {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__name__" {
                let bits = class_name_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
        }
        return class_attr_lookup(obj_ptr, obj_ptr, None, attr_bits);
    }
    if type_id == TYPE_ID_SUPER {
        let start_bits = super_type_bits(obj_ptr);
        let obj_bits = super_obj_bits(obj_ptr);
        let obj = obj_from_bits(obj_bits);
        let obj_type_bits = if let Some(obj_ptr) = obj.as_ptr() {
            if object_type_id(obj_ptr) == TYPE_ID_TYPE {
                obj_bits
            } else {
                type_of_bits(obj_bits)
            }
        } else {
            type_of_bits(obj_bits)
        };
        let Some(obj_type_ptr) = obj_from_bits(obj_type_bits).as_ptr() else {
            return None;
        };
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let mro_storage: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(obj_type_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(obj_type_bits))
        };
        let mut instance_ptr = None;
        let mut owner_ptr = obj_type_ptr;
        if let Some(raw_ptr) = obj.as_ptr() {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                owner_ptr = raw_ptr;
            } else {
                instance_ptr = Some(raw_ptr);
            }
        }
        let mut found_start = false;
        for class_bits in mro_storage.iter() {
            if !found_start {
                if *class_bits == start_bits {
                    found_start = true;
                }
                continue;
            }
            let class_obj = obj_from_bits(*class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            if let Some(val_bits) = dict_get_in_place(dict_ptr, attr_bits) {
                return descriptor_bind(val_bits, owner_ptr, instance_ptr);
            }
        }
        return None;
    }
    if type_id == TYPE_ID_FUNCTION {
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                        inc_ref_bits(val);
                        return Some(val);
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let slots = (*desc_ptr).slots;
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(val_bits) {
                                return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                            }
                        }
                    }
                }
            }
            if !slots {
                let dict_bits = dataclass_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                            inc_ref_bits(val);
                            return Some(val);
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                        }
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_OBJECT {
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if descriptor_is_data(val_bits) {
                            return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                        }
                    }
                }
            }
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(dict_ptr, attr_bits) {
                        inc_ref_bits(val);
                        return Some(val);
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                    }
                }
            }
        }
        return None;
    }
    None
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len: usize,
) -> i64 {
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let attr_ptr = alloc_string(slice);
    if attr_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
    let found = attr_lookup_ptr(obj_ptr, attr_bits);
    dec_ref_bits(attr_bits);
    if let Some(val) = found {
        return val as i64;
    }
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() && (*desc_ptr).slots {
            let name = &(*desc_ptr).name;
            let type_label = if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            };
            return attr_error(type_label, attr_name);
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        return attr_error(type_label, attr_name);
    }
    if type_id == TYPE_ID_TYPE {
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
        raise!("AttributeError", &msg);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len: usize,
    val_bits: u64,
) -> i64 {
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let module_bits = MoltObject::from_ptr(obj_ptr).bits();
        let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
        dec_ref_bits(attr_bits);
        return res as i64;
    }
    if type_id == TYPE_ID_TYPE {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(class_bits) {
            raise!("TypeError", "cannot set attributes on builtin type");
        }
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("type", attr_name);
    }
    if type_id == TYPE_ID_EXCEPTION {
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
        dec_ref_bits(attr_bits);
        if name == "__cause__" || name == "__context__" {
            let val_obj = obj_from_bits(val_bits);
            if !val_obj.is_none() {
                let Some(val_ptr) = val_obj.as_ptr() else {
                    raise!(
                        "TypeError",
                        if name == "__cause__" {
                            "exception cause must be an exception or None"
                        } else {
                            "exception context must be an exception or None"
                        }
                    );
                };
                unsafe {
                    if object_type_id(val_ptr) != TYPE_ID_EXCEPTION {
                        raise!(
                            "TypeError",
                            if name == "__cause__" {
                                "exception cause must be an exception or None"
                            } else {
                                "exception context must be an exception or None"
                            }
                        );
                    }
                }
            }
            unsafe {
                let slot = if name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if old_bits != val_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(val_bits);
                    *slot = val_bits;
                }
                if name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(true).bits();
                    let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *suppress_slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(old_bits);
                        inc_ref_bits(suppress_bits);
                        *suppress_slot = suppress_bits;
                    }
                }
            }
            return MoltObject::none().bits() as i64;
        }
        if name == "__suppress_context__" {
            let suppress = is_truthy(obj_from_bits(val_bits));
            let suppress_bits = MoltObject::from_bool(suppress).bits();
            unsafe {
                let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != suppress_bits {
                    dec_ref_bits(old_bits);
                    inc_ref_bits(suppress_bits);
                    *slot = suppress_bits;
                }
            }
            return MoltObject::none().bits() as i64;
        }
        return attr_error("exception", attr_name);
    }
    if type_id == TYPE_ID_FUNCTION {
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let mut dict_bits = function_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            function_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("function", attr_name);
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() && (*desc_ptr).frozen {
            raise!("TypeError", "cannot assign to frozen dataclass field");
        }
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(desc_bits) {
                                let desc_obj = obj_from_bits(desc_bits);
                                if let Some(desc_ptr) = desc_obj.as_ptr() {
                                    if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                        let set_bits = property_set_bits(desc_ptr);
                                        if obj_from_bits(set_bits).is_none() {
                                            dec_ref_bits(attr_bits);
                                            return property_no_setter(attr_name, class_ptr);
                                        }
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                        dec_ref_bits(attr_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                }
                                let set_bits = intern_static_name(&INTERN_SET_NAME, b"__set__");
                                if let Some(method_bits) =
                                    descriptor_method_bits(desc_bits, set_bits)
                                {
                                    let method_obj = obj_from_bits(method_bits);
                                    let Some(method_ptr) = method_obj.as_ptr() else {
                                        raise!("TypeError", "__set__ must be a function");
                                    };
                                    if object_type_id(method_ptr) != TYPE_ID_FUNCTION {
                                        raise!("TypeError", "__set__ must be a function");
                                    }
                                    let self_bits = desc_bits;
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj3(
                                        method_bits,
                                        self_bits,
                                        inst_bits,
                                        val_bits,
                                    );
                                    dec_ref_bits(attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                                dec_ref_bits(attr_bits);
                                return descriptor_no_setter(attr_name, class_ptr);
                            }
                        }
                    }
                }
            }
            if (*desc_ptr).slots {
                dec_ref_bits(attr_bits);
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(type_label, attr_name);
            }
        }
        let mut dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            dataclass_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        return attr_error(type_label, attr_name);
    }
    if type_id == TYPE_ID_OBJECT {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error("object", attr_name);
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error("object", attr_name);
        }
        let attr_ptr = alloc_string(slice);
        if attr_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if descriptor_is_data(desc_bits) {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr() {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let set_bits = property_set_bits(desc_ptr);
                                    if obj_from_bits(set_bits).is_none() {
                                        dec_ref_bits(attr_bits);
                                        return property_no_setter(attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj2(set_bits, inst_bits, val_bits);
                                    dec_ref_bits(attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let set_bits = intern_static_name(&INTERN_SET_NAME, b"__set__");
                            if let Some(method_bits) = descriptor_method_bits(desc_bits, set_bits) {
                                let method_obj = obj_from_bits(method_bits);
                                let Some(method_ptr) = method_obj.as_ptr() else {
                                    raise!("TypeError", "__set__ must be a function");
                                };
                                if object_type_id(method_ptr) != TYPE_ID_FUNCTION {
                                    raise!("TypeError", "__set__ must be a function");
                                }
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ =
                                    call_function_obj3(method_bits, self_bits, inst_bits, val_bits);
                                dec_ref_bits(attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            dec_ref_bits(attr_bits);
                            return descriptor_no_setter(attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        let mut dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(&[]);
            if dict_ptr.is_null() {
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            instance_set_dict_bits(obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("object", attr_name);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len: usize,
) -> i64 {
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_get_attr_generic(ptr, attr_name_ptr, attr_name_len);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len: usize,
    val_bits: u64,
) -> i64 {
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_set_attr_generic(ptr, attr_name_ptr, attr_name_len, val_bits);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "attribute name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "attribute name must be str");
        }
        let attr_name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if let Some(val) = attr_lookup_ptr(obj_ptr, name_bits) {
                return val;
            }
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                if !desc_ptr.is_null() && (*desc_ptr).slots {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error(type_label, &attr_name) as u64;
                }
                let type_label = if !desc_ptr.is_null() {
                    let name = &(*desc_ptr).name;
                    if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    }
                } else {
                    "dataclass"
                };
                return attr_error(type_label, &attr_name) as u64;
            }
            if type_id == TYPE_ID_TYPE {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                raise!("AttributeError", &msg);
            }
            return attr_error(type_name(MoltObject::from_ptr(obj_ptr)), &attr_name) as u64;
        }
        let obj = obj_from_bits(obj_bits);
        attr_error(type_name(obj), &attr_name) as u64
    }
}

#[no_mangle]
pub extern "C" fn molt_get_attr_name_default(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "attribute name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "attribute name must be str");
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if let Some(val) = attr_lookup_ptr(obj_ptr, name_bits) {
                return val;
            }
            return default_bits;
        }
    }
    default_bits
}

#[no_mangle]
pub extern "C" fn molt_has_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "attribute name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "attribute name must be str");
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            if attr_lookup_ptr(obj_ptr, name_bits).is_some() {
                return MoltObject::from_bool(true).bits();
            }
            return MoltObject::from_bool(false).bits();
        }
    }
    MoltObject::from_bool(false).bits()
}

#[no_mangle]
pub extern "C" fn molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "attribute name must be str");
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "attribute name must be str");
        }
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            let bytes = string_bytes(name_ptr);
            let len = string_len(name_ptr);
            return molt_set_attr_generic(obj_ptr, bytes, len, val_bits) as u64;
        }
    }
    let obj = obj_from_bits(obj_bits);
    let name =
        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
    attr_error(type_name(obj), &name) as u64
}
mod arena;
use arena::TempArena;
