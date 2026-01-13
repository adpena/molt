//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use crossbeam_deque::{Injector, Stealer, Worker};
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::BinaryHeap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    #[link_name = "molt_call_indirect1"]
    fn molt_call_indirect1(func_idx: u64, arg0: u64) -> i64;
}

#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,
    pub ref_count: AtomicU32,
    pub poll_fn: u64, // Function pointer for polling
    pub state: i64,   // State machine state
    pub size: usize,  // Total size of allocation
    pub flags: u64,   // Header flags (object metadata)
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
    ndim: u8,
    _pad: [u8; 6],
    format_bits: u64,
    shape_ptr: *mut Vec<isize>,
    strides_ptr: *mut Vec<isize>,
}

struct MoltFileHandle {
    file: Mutex<Option<std::fs::File>>,
    readable: bool,
    writable: bool,
    text: bool,
}

struct Utf8IndexCache {
    offsets: Vec<usize>,
    prefix: Vec<i64>,
}

struct Utf8CountCache {
    needle: Vec<u8>,
    count: i64,
    prefix: Vec<i64>,
    hay_len: usize,
}

struct Utf8CountCacheEntry {
    key: usize,
    cache: Arc<Utf8CountCache>,
}

struct AttrNameCacheEntry {
    bytes: Vec<u8>,
    bits: u64,
}

#[derive(Clone)]
struct DescriptorCacheEntry {
    class_bits: u64,
    attr_bits: u64,
    version: u64,
    data_desc_bits: Option<u64>,
    class_attr_bits: Option<u64>,
}

struct Utf8CountCacheStore {
    entries: HashMap<usize, Arc<Utf8CountCache>>,
    order: VecDeque<usize>,
    capacity: usize,
}

impl Utf8CountCacheStore {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&self, key: usize) -> Option<Arc<Utf8CountCache>> {
        self.entries.get(&key).cloned()
    }

    fn insert(&mut self, key: usize, cache: Arc<Utf8CountCache>) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(key) {
            entry.insert(cache);
            return;
        }
        self.entries.insert(key, cache);
        self.order.push_back(key);
        while self.entries.len() > self.capacity {
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
const TYPE_ID_SET: u32 = 230;
const TYPE_ID_SET_BUILDER: u32 = 231;
const TYPE_ID_FROZENSET: u32 = 232;
const TYPE_ID_BIGINT: u32 = 233;
const TYPE_ID_ENUMERATE: u32 = 234;
const TYPE_ID_CALLARGS: u32 = 235;
const TYPE_ID_NOT_IMPLEMENTED: u32 = 236;

const INLINE_INT_MIN_I128: i128 = -(1_i128 << 46);
const INLINE_INT_MAX_I128: i128 = (1_i128 << 46) - 1;
const MAX_SMALL_LIST: usize = 16;
const FUNC_DEFAULT_NONE: i64 = 1;
const FUNC_DEFAULT_DICT_POP: i64 = 2;
const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
const GEN_SEND_OFFSET: usize = 0;
const GEN_THROW_OFFSET: usize = 8;
const GEN_CLOSED_OFFSET: usize = 16;
const GEN_EXC_DEPTH_OFFSET: usize = 24;
const GEN_CONTROL_SIZE: usize = 32;
const UTF8_CACHE_BLOCK: usize = 4096;
const UTF8_CACHE_MIN_LEN: usize = 16 * 1024;
const UTF8_COUNT_PREFIX_MIN_LEN: usize = UTF8_CACHE_BLOCK;
const UTF8_CACHE_MAX_ENTRIES: usize = 128;
const UTF8_COUNT_CACHE_SHARDS: usize = 8;
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
const TYPE_TAG_SET: i64 = 17;
const TYPE_TAG_FROZENSET: i64 = 18;
const BUILTIN_TAG_OBJECT: i64 = 100;
const BUILTIN_TAG_TYPE: i64 = 101;
const DEFAULT_RECURSION_LIMIT: usize = 1000;

thread_local! {
    static PARSE_ARENA: RefCell<TempArena> = RefCell::new(TempArena::new(8 * 1024));
    static CONTEXT_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static EXCEPTION_STACK: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static FRAME_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static RECURSION_LIMIT: Cell<usize> = const { Cell::new(DEFAULT_RECURSION_LIMIT) };
    static RECURSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static GENERATOR_EXCEPTION_STACKS: RefCell<HashMap<usize, Vec<u64>>> =
        RefCell::new(HashMap::new());
    static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
}

static LAST_EXCEPTION: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
static MODULE_CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static EXCEPTION_TYPE_CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static BUILTIN_CLASSES: OnceLock<BuiltinClasses> = OnceLock::new();
static INTERN_BASES_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_MRO_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_GET_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_SET_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_DELETE_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_SET_NAME_METHOD: OnceLock<u64> = OnceLock::new();
static INTERN_GETATTR_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_GETATTRIBUTE_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_CALL_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_INIT_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_SETATTR_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_FIELD_OFFSETS_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_LAYOUT_SIZE: OnceLock<u64> = OnceLock::new();
static INTERN_FLOAT_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_INDEX_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_INT_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_ROUND_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_TRUNC_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_REPR_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_STR_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_QUALNAME_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_NAME_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_ARG_NAMES: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_POSONLY: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_KWONLY_NAMES: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_VARARG: OnceLock<u64> = OnceLock::new();
static INTERN_MOLT_VARKW: OnceLock<u64> = OnceLock::new();
static INTERN_DEFAULTS_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_KWDEFAULTS_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_LT_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_LE_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_GT_NAME: OnceLock<u64> = OnceLock::new();
static INTERN_GE_NAME: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_KEYS: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_VALUES: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_ITEMS: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_GET: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_POP: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_CLEAR: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_COPY: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_POPITEM: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_SETDEFAULT: OnceLock<u64> = OnceLock::new();
static DICT_METHOD_UPDATE: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_APPEND: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_EXTEND: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_INSERT: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_REMOVE: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_POP: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_CLEAR: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_COPY: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_REVERSE: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_COUNT: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_INDEX: OnceLock<u64> = OnceLock::new();
static LIST_METHOD_SORT: OnceLock<u64> = OnceLock::new();
static PROFILE_ENABLED: OnceLock<bool> = OnceLock::new();
static MOLT_MISSING: OnceLock<u64> = OnceLock::new();
static MOLT_NOT_IMPLEMENTED: OnceLock<u64> = OnceLock::new();
static CALL_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
static STRING_COUNT_CACHE_HIT: AtomicU64 = AtomicU64::new(0);
static STRING_COUNT_CACHE_MISS: AtomicU64 = AtomicU64::new(0);
static STRUCT_FIELD_STORE_COUNT: AtomicU64 = AtomicU64::new(0);
static ATTR_LOOKUP_COUNT: AtomicU64 = AtomicU64::new(0);
static LAYOUT_GUARD_COUNT: AtomicU64 = AtomicU64::new(0);
static LAYOUT_GUARD_FAIL: AtomicU64 = AtomicU64::new(0);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_POLL_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_WAKEUP_COUNT: AtomicU64 = AtomicU64::new(0);
static ASYNC_SLEEP_REGISTER_COUNT: AtomicU64 = AtomicU64::new(0);
static OBJECT_POOL: OnceLock<Mutex<Vec<Vec<usize>>>> = OnceLock::new();

const OBJECT_POOL_MAX_BYTES: usize = 1024;
const OBJECT_POOL_BUCKET_LIMIT: usize = 4096;
const OBJECT_POOL_TLS_BUCKET_LIMIT: usize = 1024;
const OBJECT_POOL_BUCKETS: usize = OBJECT_POOL_MAX_BYTES / 8 + 1;
const HEADER_FLAG_HAS_PTRS: u64 = 1;
const HEADER_FLAG_SKIP_CLASS_DECREF: u64 = 1 << 1;

thread_local! {
    static OBJECT_POOL_TLS: RefCell<Vec<Vec<usize>>> =
        RefCell::new(vec![Vec::new(); OBJECT_POOL_BUCKETS]);
}

fn profile_enabled() -> bool {
    *PROFILE_ENABLED.get_or_init(|| {
        std::env::var("MOLT_PROFILE")
            .map(|val| !val.is_empty() && val != "0")
            .unwrap_or(false)
    })
}

#[no_mangle]
pub extern "C" fn molt_profile_enabled() -> u64 {
    if profile_enabled() {
        1
    } else {
        0
    }
}

fn profile_hit(counter: &AtomicU64) {
    if profile_enabled() {
        counter.fetch_add(1, AtomicOrdering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn molt_profile_struct_field_store() {
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
}

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

fn bigint_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = maybe_ptr_from_bits(bits)?;
    unsafe {
        if object_type_id(ptr) == TYPE_ID_BIGINT {
            Some(ptr)
        } else {
            None
        }
    }
}

fn to_bigint(obj: MoltObject) -> Option<BigInt> {
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    let ptr = bigint_ptr_from_bits(obj.bits())?;
    Some(unsafe { bigint_ref(ptr).clone() })
}

fn bigint_to_inline(value: &BigInt) -> Option<i64> {
    let val = value.to_i64()?;
    if (val as i128) >= INLINE_INT_MIN_I128 && (val as i128) <= INLINE_INT_MAX_I128 {
        Some(val)
    } else {
        None
    }
}

fn compare_bigint_float(big: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        if f.is_sign_positive() {
            return Some(Ordering::Less);
        }
        return Some(Ordering::Greater);
    }
    if let Some(big_f) = big.to_f64() {
        return big_f.partial_cmp(&f);
    }
    if big.is_negative() {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

fn index_i64_from_obj(obj_bits: u64, err: &str) -> i64 {
    let obj = obj_from_bits(obj_bits);
    if let Some(i) = to_i64(obj) {
        return i;
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        unsafe {
            let index_name_bits = intern_static_name(&INTERN_INDEX_NAME, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, index_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    return i;
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise!("TypeError", &msg);
            }
        }
    }
    raise!("TypeError", err)
}

fn index_bigint_from_obj(obj_bits: u64, err: &str) -> Option<BigInt> {
    let obj = obj_from_bits(obj_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj_bits) {
        return Some(unsafe { bigint_ref(ptr).clone() });
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        unsafe {
            let index_name_bits = intern_static_name(&INTERN_INDEX_NAME, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, index_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    return Some(BigInt::from(i));
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr).clone();
                    dec_ref_bits(res_bits);
                    return Some(big);
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise_exception::<u64>("TypeError", &msg);
                return None;
            }
        }
    }
    raise_exception::<u64>("TypeError", err);
    None
}

fn to_f64(obj: MoltObject) -> Option<f64> {
    if let Some(val) = obj.as_float() {
        return Some(val);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_f64();
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
            if type_id == TYPE_ID_BIGINT {
                return !bigint_ref(ptr).is_zero();
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
            if type_id == TYPE_ID_SET {
                return set_len(ptr) > 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return set_len(ptr) > 0;
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
            if type_id == TYPE_ID_ENUMERATE {
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
                TYPE_ID_DICT_KEYS_VIEW => "dict_keys",
                TYPE_ID_DICT_VALUES_VIEW => "dict_values",
                TYPE_ID_DICT_ITEMS_VIEW => "dict_items",
                TYPE_ID_SET => "set",
                TYPE_ID_FROZENSET => "frozenset",
                TYPE_ID_BIGINT => "int",
                TYPE_ID_RANGE => "range",
                TYPE_ID_SLICE => "slice",
                TYPE_ID_MEMORYVIEW => "memoryview",
                TYPE_ID_INTARRAY => "intarray",
                TYPE_ID_NOT_IMPLEMENTED => "NotImplementedType",
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
                TYPE_ID_ENUMERATE => "enumerate",
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
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return li == ri;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if lhs.is_float() || rhs.is_float() {
        if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
            return lf == rf;
        }
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return l_big == r_big;
    }
    if let (Some(lp), Some(rp)) = (
        maybe_ptr_from_bits(lhs.bits()),
        maybe_ptr_from_bits(rhs.bits()),
    ) {
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
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    let l_elems = set_order(lp);
                    let r_elems = set_order(rp);
                    if l_elems.len() != r_elems.len() {
                        return false;
                    }
                    let r_table = set_table(rp);
                    for key_bits in l_elems.iter().copied() {
                        if set_find_entry(r_elems, r_table, key_bits).is_none() {
                            return false;
                        }
                    }
                    return true;
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
            if ltype == TYPE_ID_DICT {
                let l_pairs = dict_order(lp);
                let r_pairs = dict_order(rp);
                if l_pairs.len() != r_pairs.len() {
                    return false;
                }
                let r_table = dict_table(rp);
                let entries = l_pairs.len() / 2;
                for entry_idx in 0..entries {
                    let key_bits = l_pairs[entry_idx * 2];
                    let val_bits = l_pairs[entry_idx * 2 + 1];
                    let Some(r_entry_idx) = dict_find_entry(r_pairs, r_table, key_bits) else {
                        return false;
                    };
                    let r_val_bits = r_pairs[r_entry_idx * 2 + 1];
                    if !obj_eq(obj_from_bits(val_bits), obj_from_bits(r_val_bits)) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_SET || ltype == TYPE_ID_FROZENSET {
                let l_elems = set_order(lp);
                let r_elems = set_order(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                let r_table = set_table(rp);
                for key_bits in l_elems.iter().copied() {
                    if set_find_entry(r_elems, r_table, key_bits).is_none() {
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
    }
}

fn dec_ref_bits(bits: u64) {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe { molt_dec_ref(ptr) };
    }
}

fn pending_bits_i64() -> i64 {
    MoltObject::pending().bits() as i64
}

#[inline]
unsafe fn call_poll_fn(poll_fn_addr: u64, task_ptr: *mut u8) -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        let bits = MoltObject::from_ptr(task_ptr).bits();
        return molt_call_indirect1(poll_fn_addr, bits);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let poll_fn: extern "C" fn(u64) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        poll_fn(task_ptr as u64)
    }
}

fn inline_int_from_i128(val: i128) -> Option<i64> {
    if (INLINE_INT_MIN_I128..=INLINE_INT_MAX_I128).contains(&val) {
        Some(val as i64)
    } else {
        None
    }
}

fn bigint_bits(value: BigInt) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<BigInt>();
    let ptr = alloc_object(total, TYPE_ID_BIGINT);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut BigInt, value);
    }
    MoltObject::from_ptr(ptr).bits()
}

fn int_bits_from_i128(val: i128) -> u64 {
    if let Some(i) = inline_int_from_i128(val) {
        MoltObject::from_int(i).bits()
    } else {
        bigint_bits(BigInt::from(val))
    }
}

fn object_pool() -> &'static Mutex<Vec<Vec<usize>>> {
    OBJECT_POOL.get_or_init(|| Mutex::new(vec![Vec::new(); OBJECT_POOL_BUCKETS]))
}

fn object_pool_index(total_size: usize) -> Option<usize> {
    if total_size == 0 || total_size > OBJECT_POOL_MAX_BYTES || !total_size.is_multiple_of(8) {
        return None;
    }
    Some(total_size / 8)
}

fn object_pool_take(total_size: usize) -> Option<*mut u8> {
    let idx = object_pool_index(total_size)?;
    let from_tls = OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        pool.get_mut(idx).and_then(|bucket| bucket.pop())
    });
    if let Some(addr) = from_tls {
        return Some(addr as *mut u8);
    }
    let mut guard = object_pool().lock().unwrap();
    guard
        .get_mut(idx)
        .and_then(|bucket| bucket.pop())
        .map(|addr| addr as *mut u8)
}

fn object_pool_put(total_size: usize, header_ptr: *mut u8) -> bool {
    if header_ptr.is_null() {
        return false;
    }
    let Some(idx) = object_pool_index(total_size) else {
        return false;
    };
    unsafe {
        std::ptr::write_bytes(header_ptr, 0, total_size);
    }
    let stored_tls = OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        let bucket = &mut pool[idx];
        if bucket.len() >= OBJECT_POOL_TLS_BUCKET_LIMIT {
            return false;
        }
        bucket.push(header_ptr as usize);
        true
    });
    if stored_tls {
        return true;
    }
    let mut guard = object_pool().lock().unwrap();
    let bucket = &mut guard[idx];
    if bucket.len() >= OBJECT_POOL_BUCKET_LIMIT {
        return false;
    }
    bucket.push(header_ptr as usize);
    true
}

fn alloc_object_zeroed_with_pool(total_size: usize, type_id: u32) -> *mut u8 {
    let header_ptr = if type_id == TYPE_ID_OBJECT {
        object_pool_take(total_size)
    } else {
        None
    };
    let header_ptr = header_ptr.unwrap_or_else(|| {
        let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
        unsafe { std::alloc::alloc_zeroed(layout) }
    });
    if header_ptr.is_null() {
        return std::ptr::null_mut();
    }
    profile_hit(&ALLOC_COUNT);
    unsafe {
        let header = header_ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        (*header).flags = 0;
        header_ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

fn alloc_object(total_size: usize, type_id: u32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        profile_hit(&ALLOC_COUNT);
        let header = ptr as *mut MoltHeader;
        (*header).type_id = type_id;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        (*header).flags = 0;
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

unsafe fn object_mark_has_ptrs(ptr: *mut u8) {
    (*header_from_obj_ptr(ptr)).flags |= HEADER_FLAG_HAS_PTRS;
}

// Intentionally no helper for flags to keep dead-code warnings clean.

unsafe fn bigint_ref(ptr: *mut u8) -> &'static BigInt {
    &*(ptr as *const BigInt)
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

unsafe fn memoryview_ndim(ptr: *mut u8) -> usize {
    (*memoryview_ptr(ptr)).ndim as usize
}

unsafe fn memoryview_format_bits(ptr: *mut u8) -> u64 {
    (*memoryview_ptr(ptr)).format_bits
}

unsafe fn memoryview_shape_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    (*memoryview_ptr(ptr)).shape_ptr
}

unsafe fn memoryview_strides_ptr(ptr: *mut u8) -> *mut Vec<isize> {
    (*memoryview_ptr(ptr)).strides_ptr
}

unsafe fn memoryview_shape(ptr: *mut u8) -> Option<&'static [isize]> {
    let shape_ptr = memoryview_shape_ptr(ptr);
    if shape_ptr.is_null() {
        return None;
    }
    Some(&*shape_ptr)
}

unsafe fn memoryview_strides(ptr: *mut u8) -> Option<&'static [isize]> {
    let strides_ptr = memoryview_strides_ptr(ptr);
    if strides_ptr.is_null() {
        return None;
    }
    Some(&*strides_ptr)
}

unsafe fn memoryview_nbytes(ptr: *mut u8) -> usize {
    memoryview_len(ptr).saturating_mul(memoryview_itemsize(ptr))
}

fn tuple_from_isize_slice(values: &[isize]) -> u64 {
    let mut elems = Vec::with_capacity(values.len());
    for &val in values {
        elems.push(MoltObject::from_int(val as i64).bits());
    }
    let ptr = alloc_tuple(&elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
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

unsafe fn set_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

unsafe fn set_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>)
}

unsafe fn set_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    let vec_ptr = set_order_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn set_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    let vec_ptr = set_table_ptr(ptr);
    &mut *vec_ptr
}

unsafe fn set_len(ptr: *mut u8) -> usize {
    set_order(ptr).len()
}

fn is_set_like_type(type_id: u32) -> bool {
    type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET
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

unsafe fn enumerate_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn enumerate_index_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn enumerate_set_index_bits(ptr: *mut u8, idx_bits: u64) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = idx_bits;
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

unsafe fn exception_trace_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn context_enter_fn(ptr: *mut u8) -> *const () {
    *(ptr as *const *const ())
}

unsafe fn context_exit_fn(ptr: *mut u8) -> *const () {
    *(ptr.add(std::mem::size_of::<*const ()>()) as *const *const ())
}

unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *const u64)
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

unsafe fn function_name_bits(ptr: *mut u8) -> u64 {
    let dict_bits = function_dict_bits(ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let qual_bits = intern_static_name(&INTERN_QUALNAME_NAME, b"__qualname__");
                if let Some(bits) = dict_get_in_place(dict_ptr, qual_bits) {
                    return bits;
                }
                let name_bits = intern_static_name(&INTERN_NAME_NAME, b"__name__");
                if let Some(bits) = dict_get_in_place(dict_ptr, name_bits) {
                    return bits;
                }
            }
        }
    }
    MoltObject::none().bits()
}

unsafe fn function_set_dict_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn function_closure_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn function_set_closure_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    if bits != 0 {
        inc_ref_bits(bits);
    }
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

unsafe fn class_layout_version_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn class_set_layout_version_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

unsafe fn class_bump_layout_version(ptr: *mut u8) {
    let current = class_layout_version_bits(ptr);
    class_set_layout_version_bits(ptr, current.wrapping_add(1));
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

fn hash_float(val: f64) -> u64 {
    if val.is_nan() {
        return mix_hash(0x7ff0_0000_0000_0001);
    }
    if val == 0.0 {
        return mix_hash(0);
    }
    if val.fract() == 0.0 {
        let int_val = val as i64;
        if (int_val as f64) == val {
            return mix_hash(int_val as u64);
        }
    }
    mix_hash(val.to_bits())
}

fn hash_frozenset(ptr: *mut u8) -> u64 {
    let elems = unsafe { set_order(ptr) };
    let mut hash = 0u64;
    for &elem in elems.iter() {
        hash ^= hash_bits(elem).wrapping_mul(0x9e3779b97f4a7c15);
    }
    hash ^= elems.len() as u64;
    mix_hash(hash)
}

fn hash_bytes_cached(ptr: *mut u8, bytes: &[u8]) -> u64 {
    let header = unsafe { header_from_obj_ptr(ptr) };
    let cached = unsafe { (*header).state as u64 };
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let hash = hash_bytes(bytes);
    unsafe {
        (*header).state = hash.wrapping_add(1) as i64;
    }
    hash
}

fn hash_unhashable(obj: MoltObject) -> u64 {
    let name = type_name(obj);
    let msg = format!("unhashable type: '{name}'");
    raise!("TypeError", &msg);
}

fn is_unhashable_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_BYTEARRAY
            | TYPE_ID_MEMORYVIEW
            | TYPE_ID_LIST_BUILDER
            | TYPE_ID_DICT_BUILDER
            | TYPE_ID_SET_BUILDER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_CALLARGS
    )
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
        return hash_float(f);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                return hash_unhashable(obj);
            }
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return hash_bytes_cached(ptr, bytes);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes_cached(ptr, bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                let bytes = bigint_ref(ptr).to_signed_bytes_le();
                return hash_bytes(&bytes);
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
            if type_id == TYPE_ID_FROZENSET {
                return hash_frozenset(ptr);
            }
        }
        return mix_hash(ptr as u64);
    }
    mix_hash(bits)
}

fn ensure_hashable(key_bits: u64) -> bool {
    let obj = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                let name = type_name(obj);
                let msg = format!("unhashable type: '{name}'");
                raise!("TypeError", &msg);
            }
        }
    }
    true
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

fn set_table_capacity(entries: usize) -> usize {
    dict_table_capacity(entries)
}

fn set_insert_entry(order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx];
    let mut slot = (hash_bits(key_bits) as usize) & mask;
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn set_rebuild(order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(order, table, entry_idx);
    }
}

fn set_find_entry(order: &[u64], table: &[usize], key_bits: u64) -> Option<usize> {
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
        let entry_key = order[entry_idx];
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
    format_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<MemoryView>();
    let ptr = alloc_object(total, TYPE_ID_MEMORYVIEW);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let shape = Box::new(vec![len as isize]);
        let strides = Box::new(vec![stride]);
        let mv_ptr = memoryview_ptr(ptr);
        (*mv_ptr).owner_bits = owner_bits;
        (*mv_ptr).offset = offset;
        (*mv_ptr).len = len;
        (*mv_ptr).itemsize = itemsize;
        (*mv_ptr).stride = stride;
        (*mv_ptr).readonly = if readonly { 1 } else { 0 };
        (*mv_ptr).ndim = 1;
        (*mv_ptr)._pad = [0; 6];
        (*mv_ptr).format_bits = format_bits;
        (*mv_ptr).shape_ptr = Box::into_raw(shape);
        (*mv_ptr).strides_ptr = Box::into_raw(strides);
    }
    inc_ref_bits(owner_bits);
    inc_ref_bits(format_bits);
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
    if !ensure_hashable(key_bits) {
        return;
    }
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

unsafe fn set_add_in_place(ptr: *mut u8, key_bits: u64) {
    if !ensure_hashable(key_bits) {
        return;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    if set_find_entry(order, table, key_bits).is_some() {
        return;
    }

    let new_entries = order.len() + 1;
    let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
    if needs_resize {
        let capacity = set_table_capacity(new_entries);
        set_rebuild(order, table, capacity);
    }

    order.push(key_bits);
    inc_ref_bits(key_bits);
    let entry_idx = order.len() - 1;
    set_insert_entry(order, table, entry_idx);
}

unsafe fn dict_get_in_place(ptr: *mut u8, key_bits: u64) -> Option<u64> {
    if !ensure_hashable(key_bits) {
        return None;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    dict_find_entry(order, table, key_bits).map(|idx| order[idx * 2 + 1])
}

unsafe fn set_del_in_place(ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(key_bits) {
        return false;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    let Some(entry_idx) = set_find_entry(order, table, key_bits) else {
        return false;
    };
    let key_val = order[entry_idx];
    dec_ref_bits(key_val);
    order.remove(entry_idx);
    let entries = order.len();
    let capacity = set_table_capacity(entries.max(1));
    set_rebuild(order, table, capacity);
    true
}

unsafe fn set_replace_entries(ptr: *mut u8, entries: &[u64]) {
    let order = set_order(ptr);
    for entry in order.iter().copied() {
        dec_ref_bits(entry);
    }
    order.clear();
    for entry in entries {
        inc_ref_bits(*entry);
        order.push(*entry);
    }
    let table = set_table(ptr);
    let capacity = set_table_capacity(order.len().max(1));
    set_rebuild(order, table, capacity);
}

unsafe fn dict_del_in_place(ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(key_bits) {
        return false;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    let Some(entry_idx) = dict_find_entry(order, table, key_bits) else {
        return false;
    };
    let key_idx = entry_idx * 2;
    let val_idx = key_idx + 1;
    let key_val = order[key_idx];
    let val_val = order[val_idx];
    dec_ref_bits(key_val);
    dec_ref_bits(val_val);
    order.drain(key_idx..=val_idx);
    let entries = order.len() / 2;
    let capacity = dict_table_capacity(entries.max(1));
    dict_rebuild(order, table, capacity);
    true
}

unsafe fn dict_clear_in_place(ptr: *mut u8) {
    let order = dict_order(ptr);
    for pair in order.chunks_exact(2) {
        dec_ref_bits(pair[0]);
        dec_ref_bits(pair[1]);
    }
    order.clear();
    let table = dict_table(ptr);
    table.clear();
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
    let total = std::mem::size_of::<MoltHeader>() + 6 * std::mem::size_of::<u64>();
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
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
    }
    ptr
}

fn alloc_exception_obj(kind_bits: u64, msg_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 6 * std::mem::size_of::<u64>();
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
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
        inc_ref_bits(kind_bits);
        inc_ref_bits(msg_bits);
        inc_ref_bits(MoltObject::none().bits());
        inc_ref_bits(MoltObject::none().bits());
        inc_ref_bits(MoltObject::from_bool(false).bits());
        inc_ref_bits(MoltObject::none().bits());
    }
    ptr
}

fn alloc_context_manager(enter_fn: *const (), exit_fn: *const (), payload_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + 2 * std::mem::size_of::<*const ()>()
        + std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_CONTEXT_MANAGER);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut *const ()) = enter_fn;
        *(ptr.add(std::mem::size_of::<*const ()>()) as *mut *const ()) = exit_fn;
        *(ptr.add(2 * std::mem::size_of::<*const ()>()) as *mut u64) = payload_bits;
        inc_ref_bits(payload_bits);
    }
    ptr
}

fn alloc_function_obj(fn_ptr: u64, arity: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 4 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_FUNCTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = arity;
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = 0;
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = 0;
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
    let total = std::mem::size_of::<MoltHeader>() + 5 * std::mem::size_of::<u64>();
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
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = 0;
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
        if exit_fn_addr.is_null() {
            return;
        }
        let exit_fn =
            std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
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
    let trace_bits = unsafe { exception_trace_bits(ptr) };
    if let Some(new_bits) = frame_stack_trace_bits() {
        if new_bits != trace_bits {
            if !obj_from_bits(trace_bits).is_none() {
                dec_ref_bits(trace_bits);
            }
            unsafe {
                *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = new_bits;
            }
        } else {
            dec_ref_bits(new_bits);
        }
    } else if !obj_from_bits(trace_bits).is_none() {
        dec_ref_bits(trace_bits);
        unsafe {
            *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = MoltObject::none().bits();
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

fn frame_stack_push(name_bits: u64) {
    inc_ref_bits(name_bits);
    FRAME_STACK.with(|stack| {
        stack.borrow_mut().push(name_bits);
    });
}

fn frame_stack_pop() {
    FRAME_STACK.with(|stack| {
        if let Some(bits) = stack.borrow_mut().pop() {
            dec_ref_bits(bits);
        }
    });
}

fn frame_stack_trace_bits() -> Option<u64> {
    let names = FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        let none_bits = MoltObject::none().bits();
        stack
            .iter()
            .copied()
            .filter(|bits| *bits != none_bits)
            .collect::<Vec<_>>()
    });
    if names.is_empty() {
        return None;
    }
    let tuple_ptr = alloc_tuple(&names);
    if tuple_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

fn recursion_limit_get() -> usize {
    RECURSION_LIMIT.with(|limit| limit.get())
}

fn recursion_limit_set(limit: usize) {
    RECURSION_LIMIT.with(|cell| cell.set(limit));
}

fn recursion_guard_enter() -> bool {
    let limit = recursion_limit_get();
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

fn recursion_guard_exit() {
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
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

fn exception_type_cache() -> &'static Mutex<HashMap<String, u64>> {
    EXCEPTION_TYPE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn exception_type_bits(kind_bits: u64) -> u64 {
    let builtins = builtin_classes();
    let name =
        string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "Exception".to_string());
    if name == "Exception" {
        return builtins.exception;
    }
    if name == "BaseException" {
        return builtins.base_exception;
    }
    let fallback = if matches!(
        name.as_str(),
        "SystemExit" | "KeyboardInterrupt" | "GeneratorExit"
    ) {
        builtins.base_exception
    } else {
        builtins.exception
    };
    let cache = exception_type_cache();
    if let Some(bits) = cache.lock().unwrap().get(&name).copied() {
        return bits;
    }

    let name_ptr = alloc_string(name.as_bytes());
    if name_ptr.is_null() {
        return fallback;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(name_bits);
    dec_ref_bits(name_bits);
    if class_ptr.is_null() {
        return fallback;
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let _ = molt_class_set_base(class_bits, fallback);

    let mut cache = exception_type_cache().lock().unwrap();
    if let Some(bits) = cache.get(&name).copied() {
        dec_ref_bits(class_bits);
        return bits;
    }
    inc_ref_bits(class_bits);
    cache.insert(name, class_bits);
    class_bits
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

fn builtin_func_bits(slot: &OnceLock<u64>, fn_ptr: u64, arity: u64) -> u64 {
    builtin_func_bits_with_default(slot, fn_ptr, arity, 0)
}

fn builtin_func_bits_with_default(
    slot: &OnceLock<u64>,
    fn_ptr: u64,
    arity: u64,
    default_kind: i64,
) -> u64 {
    *slot.get_or_init(|| {
        let ptr = alloc_function_obj(fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            if default_kind != 0 {
                let bits = MoltObject::from_int(default_kind).bits();
                unsafe {
                    function_set_dict_bits(ptr, bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn missing_bits() -> u64 {
    *MOLT_MISSING.get_or_init(|| {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(total_size, TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn not_implemented_bits() -> u64 {
    *MOLT_NOT_IMPLEMENTED.get_or_init(|| {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(total_size, TYPE_ID_NOT_IMPLEMENTED);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn is_not_implemented_bits(bits: u64) -> bool {
    if let Some(ptr) = maybe_ptr_from_bits(bits) {
        unsafe { object_type_id(ptr) == TYPE_ID_NOT_IMPLEMENTED }
    } else {
        false
    }
}

fn dict_method_bits(name: &str) -> Option<u64> {
    match name {
        "keys" => Some(builtin_func_bits(
            &DICT_METHOD_KEYS,
            dict_keys_method as usize as u64,
            1,
        )),
        "values" => Some(builtin_func_bits(
            &DICT_METHOD_VALUES,
            dict_values_method as usize as u64,
            1,
        )),
        "items" => Some(builtin_func_bits(
            &DICT_METHOD_ITEMS,
            dict_items_method as usize as u64,
            1,
        )),
        "get" => Some(builtin_func_bits_with_default(
            &DICT_METHOD_GET,
            dict_get_method as usize as u64,
            3,
            FUNC_DEFAULT_NONE,
        )),
        "pop" => Some(builtin_func_bits_with_default(
            &DICT_METHOD_POP,
            dict_pop_method as usize as u64,
            4,
            FUNC_DEFAULT_DICT_POP,
        )),
        "clear" => Some(builtin_func_bits(
            &DICT_METHOD_CLEAR,
            dict_clear_method as usize as u64,
            1,
        )),
        "copy" => Some(builtin_func_bits(
            &DICT_METHOD_COPY,
            dict_copy_method as usize as u64,
            1,
        )),
        "popitem" => Some(builtin_func_bits(
            &DICT_METHOD_POPITEM,
            dict_popitem_method as usize as u64,
            1,
        )),
        "setdefault" => Some(builtin_func_bits_with_default(
            &DICT_METHOD_SETDEFAULT,
            dict_setdefault_method as usize as u64,
            3,
            FUNC_DEFAULT_NONE,
        )),
        "update" => Some(builtin_func_bits_with_default(
            &DICT_METHOD_UPDATE,
            dict_update_method as usize as u64,
            2,
            FUNC_DEFAULT_DICT_UPDATE,
        )),
        _ => None,
    }
}

fn list_method_bits(name: &str) -> Option<u64> {
    match name {
        "append" => Some(builtin_func_bits(
            &LIST_METHOD_APPEND,
            molt_list_append as usize as u64,
            2,
        )),
        "extend" => Some(builtin_func_bits(
            &LIST_METHOD_EXTEND,
            molt_list_extend as usize as u64,
            2,
        )),
        "insert" => Some(builtin_func_bits(
            &LIST_METHOD_INSERT,
            molt_list_insert as usize as u64,
            3,
        )),
        "remove" => Some(builtin_func_bits(
            &LIST_METHOD_REMOVE,
            molt_list_remove as usize as u64,
            2,
        )),
        "pop" => Some(builtin_func_bits_with_default(
            &LIST_METHOD_POP,
            molt_list_pop as usize as u64,
            2,
            FUNC_DEFAULT_NONE,
        )),
        "clear" => Some(builtin_func_bits(
            &LIST_METHOD_CLEAR,
            molt_list_clear as usize as u64,
            1,
        )),
        "copy" => Some(builtin_func_bits(
            &LIST_METHOD_COPY,
            molt_list_copy as usize as u64,
            1,
        )),
        "reverse" => Some(builtin_func_bits(
            &LIST_METHOD_REVERSE,
            molt_list_reverse as usize as u64,
            1,
        )),
        "count" => Some(builtin_func_bits(
            &LIST_METHOD_COUNT,
            molt_list_count as usize as u64,
            2,
        )),
        "index" => Some(builtin_func_bits(
            &LIST_METHOD_INDEX,
            molt_list_index as usize as u64,
            2,
        )),
        "sort" => Some(builtin_func_bits(
            &LIST_METHOD_SORT,
            molt_list_sort as usize as u64,
            3,
        )),
        _ => None,
    }
}

struct BuiltinClasses {
    object: u64,
    type_obj: u64,
    none_type: u64,
    not_implemented_type: u64,
    base_exception: u64,
    exception: u64,
    int: u64,
    float: u64,
    bool: u64,
    str: u64,
    bytes: u64,
    bytearray: u64,
    list: u64,
    tuple: u64,
    dict: u64,
    set: u64,
    frozenset: u64,
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
        let not_implemented_type = make_builtin_class("NotImplementedType");
        let base_exception = make_builtin_class("BaseException");
        let exception = make_builtin_class("Exception");
        let int = make_builtin_class("int");
        let float = make_builtin_class("float");
        let bool = make_builtin_class("bool");
        let str = make_builtin_class("str");
        let bytes = make_builtin_class("bytes");
        let bytearray = make_builtin_class("bytearray");
        let list = make_builtin_class("list");
        let tuple = make_builtin_class("tuple");
        let dict = make_builtin_class("dict");
        let set = make_builtin_class("set");
        let frozenset = make_builtin_class("frozenset");
        let range = make_builtin_class("range");
        let slice = make_builtin_class("slice");
        let memoryview = make_builtin_class("memoryview");
        let function = make_builtin_class("function");
        let module = make_builtin_class("module");
        let super_type = make_builtin_class("super");

        let _ = molt_class_set_base(object, MoltObject::none().bits());
        let _ = molt_class_set_base(type_obj, object);
        let _ = molt_class_set_base(none_type, object);
        let _ = molt_class_set_base(not_implemented_type, object);
        let _ = molt_class_set_base(base_exception, object);
        let _ = molt_class_set_base(exception, base_exception);
        let _ = molt_class_set_base(int, object);
        let _ = molt_class_set_base(float, object);
        let _ = molt_class_set_base(bool, int);
        let _ = molt_class_set_base(str, object);
        let _ = molt_class_set_base(bytes, object);
        let _ = molt_class_set_base(bytearray, object);
        let _ = molt_class_set_base(list, object);
        let _ = molt_class_set_base(tuple, object);
        let _ = molt_class_set_base(dict, object);
        let _ = molt_class_set_base(set, object);
        let _ = molt_class_set_base(frozenset, object);
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
            not_implemented_type,
            base_exception,
            exception,
            int,
            float,
            bool,
            str,
            bytes,
            bytearray,
            list,
            tuple,
            dict,
            set,
            frozenset,
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
        TYPE_TAG_SET => Some(builtins.set),
        TYPE_TAG_FROZENSET => Some(builtins.frozenset),
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
        || bits == builtins.not_implemented_type
        || bits == builtins.base_exception
        || bits == builtins.exception
        || bits == builtins.int
        || bits == builtins.float
        || bits == builtins.bool
        || bits == builtins.str
        || bits == builtins.bytes
        || bits == builtins.bytearray
        || bits == builtins.list
        || bits == builtins.tuple
        || bits == builtins.dict
        || bits == builtins.set
        || bits == builtins.frozenset
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
                TYPE_ID_SET => builtins.set,
                TYPE_ID_FROZENSET => builtins.frozenset,
                TYPE_ID_BIGINT => builtins.int,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_NOT_IMPLEMENTED => builtins.not_implemented_type,
                TYPE_ID_EXCEPTION => exception_type_bits(exception_kind_bits(ptr)),
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
            return mro.contains(&class_bits);
        }
    }
    class_mro_vec(sub_bits).contains(&class_bits)
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

fn alloc_set_like_with_entries(entries: &[u64], type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(entries.len());
        let mut table = Vec::new();
        if !entries.is_empty() {
            table.resize(set_table_capacity(entries.len()), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for &entry in entries {
            set_add_in_place(ptr, entry);
        }
    }
    ptr
}

fn alloc_set_with_entries(entries: &[u64]) -> *mut u8 {
    alloc_set_like_with_entries(entries, TYPE_ID_SET)
}

#[no_mangle]
pub extern "C" fn molt_alloc(size_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                raise!("TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                raise!("TypeError", "class must be a type object");
            }
            object_set_class_bits(obj_ptr, class_bits);
            inc_ref_bits(class_bits);
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class_trusted(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            object_set_class_bits(obj_ptr, class_bits);
            inc_ref_bits(class_bits);
        }
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_alloc_class_static(size_bits: u64, class_bits: u64) -> u64 {
    let size = usize_from_bits(size_bits);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        if class_bits != 0 {
            object_set_class_bits(obj_ptr, class_bits);
        }
        let header = header_from_obj_ptr(obj_ptr);
        (*header).flags |= HEADER_FLAG_SKIP_CLASS_DECREF;
    }
    MoltObject::from_ptr(obj_ptr).bits()
}

// --- List Builder ---

#[no_mangle]
pub extern "C" fn molt_list_builder_new(capacity_bits: u64) -> u64 {
    // Allocate wrapper object
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>(); // Store pointer to Vec
    let ptr = alloc_object(total, TYPE_ID_LIST_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Box::new(Vec::<u64>::with_capacity(capacity_hint));
        let vec_ptr = Box::into_raw(vec);
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

struct CallArgs {
    pos: Vec<u64>,
    kw_names: Vec<u64>,
    kw_values: Vec<u64>,
}

unsafe fn callargs_ptr(ptr: *mut u8) -> *mut CallArgs {
    *(ptr as *mut *mut CallArgs)
}

#[no_mangle]
pub extern "C" fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut CallArgs>();
    let ptr = alloc_object(total, TYPE_ID_CALLARGS);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let pos_capacity = usize_from_bits(pos_capacity_bits);
        let kw_capacity = usize_from_bits(kw_capacity_bits);
        let args = Box::new(CallArgs {
            pos: Vec::with_capacity(pos_capacity),
            kw_names: Vec::with_capacity(kw_capacity),
            kw_values: Vec::with_capacity(kw_capacity),
        });
        let args_ptr = Box::into_raw(args);
        *(ptr as *mut *mut CallArgs) = args_ptr;
    }
    bits_from_ptr(ptr)
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new` and
/// remain owned by the caller for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args_ptr = callargs_ptr(builder_ptr);
    if args_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args = &mut *args_ptr;
    args.pos.push(val);
    MoltObject::none().bits()
}

unsafe fn callargs_push_kw(builder_ptr: *mut u8, name_bits: u64, val_bits: u64) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        raise!("TypeError", "keywords must be strings");
    };
    if object_type_id(name_ptr) != TYPE_ID_STRING {
        raise!("TypeError", "keywords must be strings");
    }
    let args_ptr = callargs_ptr(builder_ptr);
    if args_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args = &mut *args_ptr;
    for existing in args.kw_names.iter().copied() {
        if obj_eq(obj_from_bits(existing), name_obj) {
            let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
            let msg = format!("got multiple values for keyword argument '{name}'");
            raise!("TypeError", &msg);
        }
    }
    args.kw_names.push(name_bits);
    args.kw_values.push(val_bits);
    MoltObject::none().bits()
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
/// `name_bits` must reference a Molt string object.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_push_kw(
    builder_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    callargs_push_kw(builder_ptr, name_bits, val_bits)
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let iter_bits = molt_iter(iterable_bits);
    if obj_from_bits(iter_bits).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return MoltObject::none().bits();
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return MoltObject::none().bits();
        }
        let done_bits = elems[1];
        if is_truthy(obj_from_bits(done_bits)) {
            break;
        }
        let val_bits = elems[0];
        let res = molt_callargs_push_pos(builder_bits, val_bits);
        if obj_from_bits(res).is_none() && exception_pending() {
            return res;
        }
    }
    MoltObject::none().bits()
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_expand_kwstar(builder_bits: u64, mapping_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let mapping_obj = obj_from_bits(mapping_bits);
    let Some(mapping_ptr) = mapping_obj.as_ptr() else {
        raise!("TypeError", "argument after ** must be a mapping");
    };
    if object_type_id(mapping_ptr) == TYPE_ID_DICT {
        let order = dict_order(mapping_ptr);
        for idx in (0..order.len()).step_by(2) {
            let key_bits = order[idx];
            let val_bits = order[idx + 1];
            let res = callargs_push_kw(builder_ptr, key_bits, val_bits);
            if obj_from_bits(res).is_none() && exception_pending() {
                return res;
            }
        }
        return MoltObject::none().bits();
    }
    let Some(keys_bits) = attr_name_bits_from_bytes(b"keys") else {
        raise!("TypeError", "argument after ** must be a mapping");
    };
    let keys_method_bits = attr_lookup_ptr(mapping_ptr, keys_bits);
    dec_ref_bits(keys_bits);
    let Some(keys_method_bits) = keys_method_bits else {
        raise!("TypeError", "argument after ** must be a mapping");
    };
    let keys_iterable = call_callable0(keys_method_bits);
    let iter_bits = molt_iter(keys_iterable);
    if obj_from_bits(iter_bits).is_none() {
        raise!("TypeError", "argument after ** must be a mapping");
    }
    let Some(getitem_bits) = attr_name_bits_from_bytes(b"__getitem__") else {
        raise!("TypeError", "argument after ** must be a mapping");
    };
    let getitem_method_bits = attr_lookup_ptr(mapping_ptr, getitem_bits);
    dec_ref_bits(getitem_bits);
    let Some(getitem_method_bits) = getitem_method_bits else {
        raise!("TypeError", "argument after ** must be a mapping");
    };
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return MoltObject::none().bits();
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return MoltObject::none().bits();
        }
        let done_bits = elems[1];
        if is_truthy(obj_from_bits(done_bits)) {
            break;
        }
        let key_bits = elems[0];
        let key_obj = obj_from_bits(key_bits);
        let Some(key_ptr) = key_obj.as_ptr() else {
            raise!("TypeError", "keywords must be strings");
        };
        if object_type_id(key_ptr) != TYPE_ID_STRING {
            raise!("TypeError", "keywords must be strings");
        }
        let val_bits = call_callable1(getitem_method_bits, key_bits);
        let res = callargs_push_kw(builder_ptr, key_bits, val_bits);
        if obj_from_bits(res).is_none() && exception_pending() {
            return res;
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_append(builder_bits: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
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
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
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
/// Caller must ensure `builder_bits` is valid and points to a tuple builder.
pub unsafe extern "C" fn molt_tuple_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
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
pub extern "C" fn molt_func_new_closure(fn_ptr: u64, arity: u64, closure_bits: u64) -> u64 {
    let ptr = alloc_function_obj(fn_ptr, arity);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        function_set_closure_bits(ptr, closure_bits);
    }
    MoltObject::from_ptr(ptr).bits()
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
    let obj_bits = molt_alloc(std::mem::size_of::<u64>() as u64);
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    let class_bits = builtin_classes().object;
    unsafe {
        let _ = molt_object_set_class(bits_from_ptr(obj_ptr), class_bits);
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
        let mut updated = false;
        if old_bases != bases_bits {
            dec_ref_bits(old_bases);
            inc_ref_bits(bases_bits);
            class_set_bases_bits(class_ptr, bases_bits);
            updated = true;
        }
        if old_mro != mro_bits {
            dec_ref_bits(old_mro);
            inc_ref_bits(mro_bits);
            class_set_mro_bits(class_ptr, mro_bits);
            updated = true;
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
        if updated {
            class_bump_layout_version(class_ptr);
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        raise!("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "class must be a type object");
        }
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return MoltObject::none().bits();
        }
        let entries = dict_order(dict_ptr).clone();
        let set_name_bits = intern_static_name(&INTERN_SET_NAME_METHOD, b"__set_name__");
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name_bits = pair[0];
            let val_bits = pair[1];
            let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
                continue;
            };
            if let Some(set_name) = attr_lookup_ptr(val_ptr, set_name_bits) {
                let _ = call_callable2(set_name, class_bits, name_bits);
                dec_ref_bits(set_name);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_class_layout_version(class_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        raise!("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "class must be a type object");
        }
        MoltObject::from_int(class_layout_version_bits(class_ptr) as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_class_set_layout_version(class_bits: u64, version_bits: u64) -> u64 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        raise!("TypeError", "class must be a type object");
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            raise!("TypeError", "class must be a type object");
        }
        let version = match to_i64(obj_from_bits(version_bits)) {
            Some(val) if val >= 0 => val as u64,
            _ => raise!("TypeError", "layout version must be int"),
        };
        class_set_layout_version_bits(class_ptr, version);
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
/// `obj_ptr_bits` must point to a valid Molt object header that can be mutated, and
/// `class_bits` must be either zero or a valid Molt type object.
#[no_mangle]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr_bits: u64, class_bits: u64) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
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
    let skip_class_ref = ((*header).flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0;
    let old_bits = object_class_bits(obj_ptr);
    if old_bits != 0 && !skip_class_ref {
        dec_ref_bits(old_bits);
    }
    object_set_class_bits(obj_ptr, class_bits);
    if class_bits != 0 && !skip_class_ref {
        inc_ref_bits(class_bits);
    }
    MoltObject::none().bits()
}

fn resolve_obj_ptr(bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        return Some(ptr);
    }
    None
}

fn resolve_task_ptr(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return Some(ptr);
    }
    if !obj.is_float() {
        return None;
    }
    let high = bits >> 48;
    if high == 0 || high == 0xffff {
        let ptr = ptr_from_bits(bits);
        let addr = ptr as usize;
        if addr < 4096 || (addr & 0x7) != 0 {
            return None;
        }
        return Some(ptr);
    }
    None
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
unsafe fn object_field_get_ptr_raw(obj_ptr: *mut u8, offset: usize) -> u64 {
    if obj_ptr.is_null() {
        raise!("TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *const u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
unsafe fn object_field_set_ptr_raw(obj_ptr: *mut u8, offset: usize, val_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        raise!("TypeError", "object field access on non-object");
    }
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
    let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
    if new_is_ptr {
        object_mark_has_ptrs(obj_ptr);
    }
    if !old_is_ptr && !new_is_ptr {
        *slot = val_bits;
        return MoltObject::none().bits();
    }
    if old_bits != val_bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(val_bits);
        *slot = val_bits;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
/// Intended for initializing freshly allocated objects with immediate values.
unsafe fn object_field_init_ptr_raw(obj_ptr: *mut u8, offset: usize, val_bits: u64) -> u64 {
    if obj_ptr.is_null() {
        raise!("TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    debug_assert!(
        old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
        "object_field_init used on slot with pointer contents"
    );
    if obj_from_bits(val_bits).as_ptr().is_some() {
        object_mark_has_ptrs(obj_ptr);
    }
    *slot = val_bits;
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get_ptr(obj_ptr_bits: u64, offset_bits: u64) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    object_field_get_ptr_raw(obj_ptr, offset)
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set_ptr(
    obj_ptr_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    object_field_set_ptr_raw(obj_ptr, offset, val_bits)
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init_ptr(
    obj_ptr_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    object_field_init_ptr_raw(obj_ptr, offset, val_bits)
}

unsafe fn guard_layout_match(obj_ptr: *mut u8, class_bits: u64, expected_version: u64) -> bool {
    profile_hit(&LAYOUT_GUARD_COUNT);
    if obj_ptr.is_null() {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).type_id != TYPE_ID_OBJECT {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let obj_class_bits = object_class_bits(obj_ptr);
    if obj_class_bits == 0 || obj_class_bits != class_bits {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    };
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    let version = class_layout_version_bits(class_ptr);
    let expected = match to_i64(obj_from_bits(expected_version)) {
        Some(val) if val >= 0 => val as u64,
        _ => {
            profile_hit(&LAYOUT_GUARD_FAIL);
            return false;
        }
    };
    if version != expected {
        profile_hit(&LAYOUT_GUARD_FAIL);
        return false;
    }
    true
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with a class.
#[no_mangle]
pub unsafe extern "C" fn molt_guard_layout_ptr(
    obj_ptr_bits: u64,
    class_bits: u64,
    expected_version: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    MoltObject::from_bool(guard_layout_match(obj_ptr, class_bits, expected_version)).bits()
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_get_ptr(
    obj_ptr_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_get_ptr_raw(obj_ptr, offset);
    }
    molt_get_attr_ptr(obj_ptr_bits, attr_name_bits, attr_name_len_bits) as u64
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_set_ptr(
    obj_ptr_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_set_ptr_raw(obj_ptr, offset, val_bits);
    }
    molt_set_attr_ptr(obj_ptr_bits, attr_name_bits, attr_name_len_bits, val_bits) as u64
}

/// # Safety
/// `obj_ptr_bits` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_init_ptr(
    obj_ptr_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let offset = usize_from_bits(offset_bits);
    if guard_layout_match(obj_ptr, class_bits, expected_version) {
        return object_field_init_ptr_raw(obj_ptr, offset, val_bits);
    }
    molt_set_attr_ptr(obj_ptr_bits, attr_name_bits, attr_name_len_bits, val_bits) as u64
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get(obj_bits: u64, offset_bits: u64) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        raise!("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    let slot = obj_ptr.add(offset) as *const u64;
    let bits = *slot;
    inc_ref_bits(bits);
    bits
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        raise!("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    profile_hit(&STRUCT_FIELD_STORE_COUNT);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
    let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
    if new_is_ptr {
        object_mark_has_ptrs(obj_ptr);
    }
    if !old_is_ptr && !new_is_ptr {
        *slot = val_bits;
        return MoltObject::none().bits();
    }
    if old_bits != val_bits {
        dec_ref_bits(old_bits);
        inc_ref_bits(val_bits);
        *slot = val_bits;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        raise!("TypeError", "object field access on non-object");
    };
    let offset = usize_from_bits(offset_bits);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    debug_assert!(
        old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
        "object_field_init used on slot with pointer contents"
    );
    if obj_from_bits(val_bits).as_ptr().is_some() {
        object_mark_has_ptrs(obj_ptr);
    }
    *slot = val_bits;
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
    let mut msg_bits = msg_bits;
    let mut converted = false;
    if let Some(ptr) = msg_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                msg_bits = molt_str_from_obj(msg_bits);
                converted = true;
            }
        }
    } else {
        msg_bits = molt_str_from_obj(msg_bits);
        converted = true;
    }
    if obj_from_bits(msg_bits).is_none() {
        return MoltObject::none().bits();
    }
    let ptr = alloc_exception_obj(kind_bits, msg_bits);
    let out = if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    };
    if converted {
        dec_ref_bits(msg_bits);
    }
    out
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
pub unsafe extern "C" fn molt_closure_load(self_bits: u64, offset: u64) -> u64 {
    let self_ptr = ptr_from_bits(self_bits);
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
pub unsafe extern "C" fn molt_closure_store(self_bits: u64, offset: u64, bits: u64) -> u64 {
    let self_ptr = ptr_from_bits(self_bits);
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
pub extern "C" fn molt_is_bound_method(obj_bits: u64) -> u64 {
    let is_bound = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BOUND_METHOD });
    MoltObject::from_bool(is_bound).bits()
}

#[no_mangle]
pub extern "C" fn molt_is_function_obj(obj_bits: u64) -> u64 {
    let is_func = maybe_ptr_from_bits(obj_bits)
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION });
    MoltObject::from_bool(is_func).bits()
}

#[no_mangle]
pub extern "C" fn molt_is_callable(obj_bits: u64) -> u64 {
    let is_callable = maybe_ptr_from_bits(obj_bits).is_some_and(|ptr| unsafe {
        match object_type_id(ptr) {
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE => true,
            TYPE_ID_OBJECT => {
                let call_bits = intern_static_name(&INTERN_CALL_NAME, b"__call__");
                let dict_bits = instance_dict_bits(ptr);
                if dict_bits != 0 {
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT
                            && dict_get_in_place(dict_ptr, call_bits).is_some()
                        {
                            return true;
                        }
                    }
                }
                let class_bits = object_class_bits(ptr);
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            return class_attr_lookup_raw_mro(class_ptr, call_bits).is_some();
                        }
                    }
                }
                false
            }
            TYPE_ID_DATACLASS => {
                let call_bits = intern_static_name(&INTERN_CALL_NAME, b"__call__");
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(ptr);
                    if dict_bits != 0 {
                        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                            if object_type_id(dict_ptr) == TYPE_ID_DICT
                                && dict_get_in_place(dict_ptr, call_bits).is_some()
                            {
                                return true;
                            }
                        }
                    }
                }
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0 {
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                return class_attr_lookup_raw_mro(class_ptr, call_bits).is_some();
                            }
                        }
                    }
                }
                false
            }
            _ => false,
        }
    });
    MoltObject::from_bool(is_callable).bits()
}

#[no_mangle]
pub extern "C" fn molt_function_default_kind(func_bits: u64) -> i64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return 0;
        }
        let dict_bits = function_dict_bits(ptr);
        if dict_bits == 0 {
            return 0;
        }
        obj_from_bits(dict_bits).as_int().unwrap_or(0)
    }
}

#[no_mangle]
pub extern "C" fn molt_function_closure_bits(func_bits: u64) -> u64 {
    let obj = obj_from_bits(func_bits);
    let Some(ptr) = obj.as_ptr() else {
        return 0;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return 0;
        }
        function_closure_bits(ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_call_arity_error(expected: i64, got: i64) -> u64 {
    let msg = format!("call arity mismatch (expected {expected}, got {got})");
    raise!("TypeError", &msg);
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
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = call_poll_fn(poll_fn_addr, ptr);
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
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = call_poll_fn(poll_fn_addr, ptr);
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
        let prev_raise = generator_raise_active();
        set_generator_raise(true);
        let res = call_poll_fn(poll_fn_addr, ptr) as u64;
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
pub extern "C" fn molt_context_new(
    enter_fn: *const (),
    exit_fn: *const (),
    payload_bits: u64,
) -> u64 {
    if enter_fn.is_null() || exit_fn.is_null() {
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
            if enter_fn_addr.is_null() {
                raise!("TypeError", "context manager missing __enter__");
            }
            let enter_fn =
                std::mem::transmute::<*const (), extern "C" fn(u64) -> u64>(enter_fn_addr);
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
            if exit_fn_addr.is_null() {
                raise!("TypeError", "context manager missing __exit__");
            }
            let exit_fn =
                std::mem::transmute::<*const (), extern "C" fn(u64, u64) -> u64>(exit_fn_addr);
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
    let enter_fn = context_null_enter as *const ();
    let exit_fn = context_null_exit as *const ();
    let ptr = alloc_context_manager(enter_fn, exit_fn, payload_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_context_closing(payload_bits: u64) -> u64 {
    let enter_fn = context_closing_enter as *const ();
    let exit_fn = context_closing_exit as *const ();
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
pub extern "C" fn molt_dict_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
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

enum DictSeqError {
    NotIterable,
    BadLen(usize),
    Exception,
}

fn dict_pair_from_item(item_bits: u64) -> Result<(u64, u64), DictSeqError> {
    let item_obj = obj_from_bits(item_bits);
    if let Some(item_ptr) = item_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(item_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(item_ptr);
                if elems.len() != 2 {
                    return Err(DictSeqError::BadLen(elems.len()));
                }
                return Ok((elems[0], elems[1]));
            }
        }
    }
    let iter_bits = molt_iter(item_bits);
    if obj_from_bits(iter_bits).is_none() {
        return Err(DictSeqError::NotIterable);
    }
    let mut elems = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending() {
            return Err(DictSeqError::Exception);
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return Err(DictSeqError::Exception);
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(DictSeqError::Exception);
            }
            let pair_elems = seq_vec_ref(pair_ptr);
            if pair_elems.len() < 2 {
                return Err(DictSeqError::Exception);
            }
            let done_bits = pair_elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            elems.push(pair_elems[0]);
        }
    }
    if elems.len() != 2 {
        return Err(DictSeqError::BadLen(elems.len()));
    }
    Ok((elems[0], elems[1]))
}

#[no_mangle]
pub extern "C" fn molt_dict_from_obj(obj_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let mut iter_bits = obj_bits;
    let mut capacity = 0usize;
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_DICT {
                capacity = dict_len(ptr);
                iter_bits = molt_dict_items(obj_bits);
                if obj_from_bits(iter_bits).is_none() {
                    return MoltObject::none().bits();
                }
            }
        }
    }
    let dict_bits = molt_dict_new(capacity as u64);
    if obj_from_bits(dict_bits).is_none() {
        return MoltObject::none().bits();
    }
    let Some(dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
        return MoltObject::none().bits();
    };
    let source_bits = iter_bits;
    let iter = molt_iter(iter_bits);
    if obj_from_bits(iter).is_none() {
        let mapping_obj = obj_from_bits(source_bits);
        let Some(mapping_ptr) = mapping_obj.as_ptr() else {
            raise!("TypeError", "dict() argument must be a mapping or iterable");
        };
        let Some(keys_bits) = attr_name_bits_from_bytes(b"keys") else {
            raise!("TypeError", "dict() argument must be a mapping or iterable");
        };
        unsafe {
            let keys_method_bits = attr_lookup_ptr(mapping_ptr, keys_bits);
            dec_ref_bits(keys_bits);
            let Some(keys_method_bits) = keys_method_bits else {
                raise!("TypeError", "dict() argument must be a mapping or iterable");
            };
            let keys_iterable = call_callable0(keys_method_bits);
            let keys_iter = molt_iter(keys_iterable);
            if obj_from_bits(keys_iter).is_none() {
                raise!("TypeError", "dict() argument must be a mapping or iterable");
            }
            let Some(getitem_bits) = attr_name_bits_from_bytes(b"__getitem__") else {
                raise!("TypeError", "dict() argument must be a mapping or iterable");
            };
            let getitem_method_bits = attr_lookup_ptr(mapping_ptr, getitem_bits);
            dec_ref_bits(getitem_bits);
            let Some(getitem_method_bits) = getitem_method_bits else {
                raise!("TypeError", "dict() argument must be a mapping or iterable");
            };
            loop {
                let pair_bits = molt_iter_next(keys_iter);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let val_bits = call_callable1(getitem_method_bits, key_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                dict_set_in_place(dict_ptr, key_bits, val_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
            }
        }
        return dict_bits;
    }
    let mut elem_index = 0usize;
    loop {
        let pair_bits = molt_iter_next(iter);
        if exception_pending() {
            return MoltObject::none().bits();
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let item_bits = elems[0];
            match dict_pair_from_item(item_bits) {
                Ok((key, val)) => {
                    dict_set_in_place(dict_ptr, key, val);
                }
                Err(DictSeqError::NotIterable) => {
                    let msg = format!(
                        "cannot convert dictionary update sequence element #{elem_index} to a sequence"
                    );
                    raise!("TypeError", &msg);
                }
                Err(DictSeqError::BadLen(len)) => {
                    let msg = format!(
                        "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                    );
                    raise!("ValueError", &msg);
                }
                Err(DictSeqError::Exception) => {
                    return MoltObject::none().bits();
                }
            }
        }
        elem_index += 1;
    }
    dict_bits
}

#[no_mangle]
pub extern "C" fn molt_set_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_SET);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let order = Vec::with_capacity(capacity_hint);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(set_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_frozenset_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_FROZENSET);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let order = Vec::with_capacity(capacity_hint);
        let mut table = Vec::new();
        if capacity_hint > 0 {
            table.resize(set_table_capacity(capacity_hint), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_DICT_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint * 2);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_append(builder_bits: u64, key: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
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
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
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

// --- Set Builder ---

#[no_mangle]
pub extern "C" fn molt_set_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_SET_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_append(builder_bits: u64, key: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_set_with_entries(vec.as_slice());
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
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> u64 {
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
    bits_from_ptr(Box::into_raw(chan) as *mut u8)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_bits: u64, val: i64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_bits: u64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
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
pub extern "C" fn molt_stream_new(capacity_bits: u64) -> u64 {
    let capacity = usize_from_bits(capacity_bits);
    let (s, r) = bytes_channel(capacity);
    let stream = Box::new(MoltStream {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
    });
    bits_from_ptr(Box::into_raw(stream) as *mut u8)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_bits` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_stream_send(stream_bits: u64, data_bits: u64, len_bits: u64) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    let data_ptr = ptr_from_const_bits(data_bits);
    let len = usize_from_bits(len_bits);
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
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_bits: u64) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
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
            if stream.closed.load(AtomicOrdering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_bits: u64) {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.closed.store(true, AtomicOrdering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `out_left` and `out_right` are valid writable pointers.
pub unsafe extern "C" fn molt_ws_pair(
    capacity_bits: u64,
    out_left_bits: u64,
    out_right_bits: u64,
) -> i32 {
    let out_left = ptr_from_bits(out_left_bits) as *mut u64;
    let out_right = ptr_from_bits(out_right_bits) as *mut u64;
    if out_left.is_null() || out_right.is_null() {
        return 2;
    }
    let capacity = usize_from_bits(capacity_bits);
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
    *out_left = bits_from_ptr(Box::into_raw(left) as *mut u8);
    *out_right = bits_from_ptr(Box::into_raw(right) as *mut u8);
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
    WS_CONNECT_HOOK.store(ptr, AtomicOrdering::Release);
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
pub unsafe extern "C" fn molt_ws_connect(url_bits: u64, url_len_bits: u64, out_bits: u64) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    if out.is_null() {
        return 2;
    }
    let url_ptr = ptr_from_const_bits(url_bits);
    let url_len = usize_from_bits(url_len_bits);
    if !has_capability("websocket:connect") {
        return 6;
    }
    let hook_ptr = WS_CONNECT_HOOK.load(AtomicOrdering::Acquire);
    if hook_ptr == 0 {
        // TODO(molt): Provide a host-level connect hook for production sockets.
        return 7;
    }
    let hook: WsConnectHook = std::mem::transmute(hook_ptr);
    let ws_ptr = hook(url_ptr, url_len);
    if ws_ptr.is_null() {
        return 7;
    }
    *out = bits_from_ptr(ws_ptr);
    0
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_bits` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_bits: u64, data_bits: u64, len_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
    let data_ptr = ptr_from_const_bits(data_bits);
    let len = usize_from_bits(len_bits);
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
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
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
            if ws.closed.load(AtomicOrdering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_bits: u64) {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.close_hook {
        hook(ws.hook_ctx);
        return;
    }
    ws.closed.store(true, AtomicOrdering::Relaxed);
}

// --- Scheduler ---

struct AsyncHangProbe {
    threshold: usize,
    pending_counts: Mutex<HashMap<usize, usize>>,
}

impl AsyncHangProbe {
    fn new(threshold: usize) -> Self {
        Self {
            threshold,
            pending_counts: Mutex::new(HashMap::new()),
        }
    }
}

static ASYNC_HANG_PROBE: OnceLock<Option<AsyncHangProbe>> = OnceLock::new();

fn async_hang_probe() -> Option<&'static AsyncHangProbe> {
    ASYNC_HANG_PROBE
        .get_or_init(|| {
            let value = std::env::var("MOLT_ASYNC_HANG_PROBE").ok()?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return None;
            }
            let threshold = match trimmed.parse::<usize>() {
                Ok(0) => return None,
                Ok(val) => val,
                Err(_) => 100_000,
            };
            Some(AsyncHangProbe::new(threshold))
        })
        .as_ref()
}

struct CancelTokenEntry {
    parent: u64,
    cancelled: bool,
    refs: u64,
}

static CANCEL_TOKENS: OnceLock<Mutex<HashMap<u64, CancelTokenEntry>>> = OnceLock::new();
static NEXT_CANCEL_TOKEN_ID: AtomicU64 = AtomicU64::new(2);
static TASK_TOKENS: OnceLock<Mutex<HashMap<usize, u64>>> = OnceLock::new();

thread_local! {
    static CURRENT_TASK: Cell<usize> = const { Cell::new(0) };
    static CURRENT_TOKEN: Cell<u64> = const { Cell::new(1) };
}

fn cancel_tokens() -> &'static Mutex<HashMap<u64, CancelTokenEntry>> {
    CANCEL_TOKENS.get_or_init(|| {
        let mut map = HashMap::new();
        map.insert(
            1,
            CancelTokenEntry {
                parent: 0,
                cancelled: false,
                refs: 1,
            },
        );
        Mutex::new(map)
    })
}

fn task_tokens() -> &'static Mutex<HashMap<usize, u64>> {
    TASK_TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn token_id_from_bits(bits: u64) -> Option<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Some(0);
    }
    obj.as_int()
        .and_then(|val| if val >= 0 { Some(val as u64) } else { None })
}

fn current_token_id() -> u64 {
    CURRENT_TOKEN.with(|cell| cell.get())
}

fn set_current_token(id: u64) -> u64 {
    retain_token(id);
    let prev = CURRENT_TOKEN.with(|cell| {
        let prev = cell.get();
        cell.set(id);
        prev
    });
    release_token(prev);
    prev
}

fn retain_token(id: u64) {
    if id == 0 || id == 1 {
        return;
    }
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.refs = entry.refs.saturating_add(1);
    }
}

fn release_token(id: u64) {
    if id == 0 || id == 1 {
        return;
    }
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.refs = entry.refs.saturating_sub(1);
        if entry.refs == 0 {
            map.remove(&id);
        }
    }
}

fn register_task_token(task_ptr: *mut u8, token: u64) {
    let task_key = task_ptr as usize;
    let mut map = task_tokens().lock().unwrap();
    if let Some(old) = map.insert(task_key, token) {
        release_token(old);
    }
    retain_token(token);
}

fn ensure_task_token(task_ptr: *mut u8, fallback: u64) -> u64 {
    let task_key = task_ptr as usize;
    let mut map = task_tokens().lock().unwrap();
    if let Some(token) = map.get(&task_key).copied() {
        return token;
    }
    map.insert(task_key, fallback);
    retain_token(fallback);
    fallback
}

fn clear_task_token(task_ptr: *mut u8) {
    let task_key = task_ptr as usize;
    if let Some(token) = task_tokens().lock().unwrap().remove(&task_key) {
        release_token(token);
    }
}

fn token_is_cancelled(id: u64) -> bool {
    if id == 0 {
        return false;
    }
    let map = cancel_tokens().lock().unwrap();
    let mut current = id;
    let mut depth = 0;
    while current != 0 && depth < 64 {
        let Some(entry) = map.get(&current) else {
            return false;
        };
        if entry.cancelled {
            return true;
        }
        current = entry.parent;
        depth += 1;
    }
    false
}

fn record_async_poll(task_ptr: *mut u8, pending: bool, site: &str) {
    profile_hit(&ASYNC_POLL_COUNT);
    if pending {
        profile_hit(&ASYNC_PENDING_COUNT);
    }
    let Some(probe) = async_hang_probe() else {
        return;
    };
    if task_ptr.is_null() {
        return;
    }
    if !pending {
        probe
            .pending_counts
            .lock()
            .unwrap()
            .remove(&(task_ptr as usize));
        return;
    }
    let mut counts = probe.pending_counts.lock().unwrap();
    let count = counts.entry(task_ptr as usize).or_insert(0);
    *count += 1;
    if *count != probe.threshold && *count % probe.threshold != 0 {
        return;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        eprintln!(
            "Molt async hang probe: site={} polls={} ptr=0x{:x} type={} state={} poll=0x{:x}",
            site,
            count,
            task_ptr as usize,
            (*header).type_id,
            (*header).state,
            (*header).poll_fn
        );
    }
}

pub struct MoltTask {
    pub future_ptr: *mut u8,
}

#[derive(Copy, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct SleepEntry {
    deadline: Instant,
    task_ptr: usize,
    gen: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl PartialEq for SleepEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.gen == other.gen && self.task_ptr == other.task_ptr
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Eq for SleepEntry {}

#[cfg(not(target_arch = "wasm32"))]
impl PartialOrd for SleepEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Ord for SleepEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.gen.cmp(&self.gen))
    }
}

struct SleepState {
    #[cfg(not(target_arch = "wasm32"))]
    heap: BinaryHeap<SleepEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    tasks: HashMap<usize, u64>,
    #[cfg(not(target_arch = "wasm32"))]
    next_gen: u64,
    blocking: HashMap<usize, Instant>,
}

struct SleepQueue {
    inner: Mutex<SleepState>,
    #[cfg(not(target_arch = "wasm32"))]
    cv: Condvar,
}

impl SleepQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(SleepState {
                #[cfg(not(target_arch = "wasm32"))]
                heap: BinaryHeap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                tasks: HashMap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                next_gen: 0,
                blocking: HashMap::new(),
            }),
            #[cfg(not(target_arch = "wasm32"))]
            cv: Condvar::new(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn register_scheduler(&self, task_ptr: *mut u8, deadline: Instant) {
        let mut guard = self.inner.lock().unwrap();
        let task_key = task_ptr as usize;
        let gen = guard.next_gen;
        guard.next_gen += 1;
        guard.tasks.insert(task_key, gen);
        profile_hit(&ASYNC_SLEEP_REGISTER_COUNT);
        guard.heap.push(SleepEntry {
            deadline,
            task_ptr: task_key,
            gen,
        });
        self.cv.notify_one();
    }

    fn register_blocking(&self, task_ptr: *mut u8, deadline: Instant) {
        let mut guard = self.inner.lock().unwrap();
        profile_hit(&ASYNC_SLEEP_REGISTER_COUNT);
        guard.blocking.insert(task_ptr as usize, deadline);
    }

    fn take_blocking_deadline(&self, task_ptr: *mut u8) -> Option<Instant> {
        let mut guard = self.inner.lock().unwrap();
        guard.blocking.remove(&(task_ptr as usize))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn is_scheduled(&self, task_ptr: *mut u8) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.tasks.contains_key(&(task_ptr as usize))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sleep_worker(queue: Arc<SleepQueue>) {
    loop {
        let task_ptr = {
            let mut guard = queue.inner.lock().unwrap();
            loop {
                match guard.heap.peek() {
                    Some(entry) => {
                        let key = entry.task_ptr;
                        if guard.tasks.get(&key) != Some(&entry.gen) {
                            guard.heap.pop();
                            continue;
                        }
                        let now = Instant::now();
                        if entry.deadline <= now {
                            let entry = guard.heap.pop().unwrap();
                            guard.tasks.remove(&key);
                            break entry.task_ptr as *mut u8;
                        }
                        let wait = entry.deadline.saturating_duration_since(now);
                        let (next_guard, _) = queue.cv.wait_timeout(guard, wait).unwrap();
                        guard = next_guard;
                    }
                    None => {
                        guard = queue.cv.wait(guard).unwrap();
                    }
                }
            }
        };
        profile_hit(&ASYNC_WAKEUP_COUNT);
        SCHEDULER.enqueue(MoltTask {
            future_ptr: task_ptr,
        });
    }
}

static START_TIME: OnceLock<Instant> = OnceLock::new();

fn monotonic_now_secs() -> f64 {
    START_TIME.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn instant_from_monotonic_secs(secs: f64) -> Instant {
    let start = START_TIME.get_or_init(Instant::now);
    if !secs.is_finite() || secs <= 0.0 {
        return *start;
    }
    *start + Duration::from_secs_f64(secs)
}

unsafe impl Send for MoltTask {}

pub struct MoltScheduler {
    injector: Arc<Injector<MoltTask>>,
    stealers: Vec<Stealer<MoltTask>>,
    running: Arc<AtomicBool>,
}

impl MoltScheduler {
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let num_threads = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let num_threads = num_cpus::get().max(1);
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
                if !running_clone.load(AtomicOrdering::Relaxed) {
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
        if !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        if self.stealers.is_empty() {
            Self::execute_task(task, &self.injector);
        } else {
            self.injector.push(task);
        }
    }

    fn execute_task(task: MoltTask, injector: &Injector<MoltTask>) {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = injector;
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = (*header).poll_fn;
                if poll_fn_addr != 0 {
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr as usize);
                        prev
                    });
                    let token = ensure_task_token(task_ptr, current_token_id());
                    let prev_token = set_current_token(token);
                    loop {
                        let res = call_poll_fn(poll_fn_addr, task_ptr);
                        let pending = res == pending_bits_i64();
                        record_async_poll(task_ptr, pending, "scheduler");
                        if pending {
                            if let Some(deadline) = SLEEP_QUEUE.take_blocking_deadline(task_ptr) {
                                let now = Instant::now();
                                if deadline > now {
                                    std::thread::sleep(deadline - now);
                                }
                            } else {
                                std::thread::yield_now();
                            }
                            continue;
                        }
                        clear_task_token(task_ptr);
                        break;
                    }
                    set_current_token(prev_token);
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
                }
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = (*header).poll_fn;
                if poll_fn_addr != 0 {
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr as usize);
                        prev
                    });
                    let token = ensure_task_token(task_ptr, current_token_id());
                    let prev_token = set_current_token(token);
                    let res = call_poll_fn(poll_fn_addr, task_ptr);
                    let pending = res == pending_bits_i64();
                    record_async_poll(task_ptr, pending, "scheduler");
                    if pending {
                        if !SLEEP_QUEUE.is_scheduled(task_ptr) {
                            injector.push(task);
                        }
                    } else {
                        clear_task_token(task_ptr);
                    }
                    set_current_token(prev_token);
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
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
    static ref SLEEP_QUEUE: Arc<SleepQueue> = {
        let queue = Arc::new(SleepQueue::new());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let worker_queue = Arc::clone(&queue);
            thread::spawn(move || sleep_worker(worker_queue));
        }
        queue
    };
    static ref UTF8_INDEX_CACHE: Mutex<Utf8CacheStore> = Mutex::new(Utf8CacheStore::new());
    static ref UTF8_COUNT_CACHE: Vec<Mutex<Utf8CountCacheStore>> = {
        let per_shard = (UTF8_CACHE_MAX_ENTRIES / UTF8_COUNT_CACHE_SHARDS).max(1);
        (0..UTF8_COUNT_CACHE_SHARDS)
            .map(|_| Mutex::new(Utf8CountCacheStore::new(per_shard)))
            .collect()
    };
}

thread_local! {
    static ATTR_NAME_TLS: RefCell<Option<AttrNameCacheEntry>> = const { RefCell::new(None) };
    static DESCRIPTOR_CACHE_TLS: RefCell<Option<DescriptorCacheEntry>> = const { RefCell::new(None) };
    static UTF8_COUNT_TLS: RefCell<Option<Utf8CountCacheEntry>> = const { RefCell::new(None) };
    static BLOCK_ON_TASK: Cell<usize> = const { Cell::new(0) };
}

/// # Safety
/// `parent_bits` must be either `None` or an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_new(parent_bits: u64) -> u64 {
    cancel_tokens();
    let parent_id = match token_id_from_bits(parent_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => raise!("TypeError", "cancel token parent must be int or None"),
    };
    let id = NEXT_CANCEL_TOKEN_ID.fetch_add(1, AtomicOrdering::Relaxed);
    let mut map = cancel_tokens().lock().unwrap();
    map.insert(
        id,
        CancelTokenEntry {
            parent: parent_id,
            cancelled: false,
            refs: 1,
        },
    );
    MoltObject::from_int(id as i64).bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_clone(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => raise!("TypeError", "cancel token id must be int"),
    };
    retain_token(id);
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_drop(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => raise!("TypeError", "cancel token id must be int"),
    };
    release_token(id);
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_cancel(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => raise!("TypeError", "cancel token id must be int"),
    };
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.cancelled = true;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_is_cancelled(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(id) => id,
        None => raise!("TypeError", "cancel token id must be int"),
    };
    MoltObject::from_bool(token_is_cancelled(id)).bits()
}

/// # Safety
/// `token_bits` must be an integer token id or `None`.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_set_current(token_bits: u64) -> u64 {
    let id = match token_id_from_bits(token_bits) {
        Some(0) => 1,
        Some(id) => id,
        None => raise!("TypeError", "cancel token id must be int"),
    };
    let prev = set_current_token(id);
    CURRENT_TASK.with(|cell| {
        let task = cell.get();
        if task != 0 {
            register_task_token(task as *mut u8, id);
        }
    });
    MoltObject::from_int(prev as i64).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_get_current() -> u64 {
    cancel_tokens();
    MoltObject::from_int(current_token_id() as i64).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancelled() -> u64 {
    cancel_tokens();
    MoltObject::from_bool(token_is_cancelled(current_token_id())).bits()
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_current() -> u64 {
    cancel_tokens();
    let id = current_token_id();
    let mut map = cancel_tokens().lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.cancelled = true;
    }
    MoltObject::none().bits()
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        raise!("TypeError", "object is not awaitable");
    };
    cancel_tokens();
    let token = current_token_id();
    register_task_token(task_ptr, token);
    SCHEDULER.enqueue(MoltTask {
        future_ptr: task_ptr,
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn is_block_on_task(task_ptr: *mut u8) -> bool {
    BLOCK_ON_TASK.with(|cell| cell.get() == task_ptr as usize)
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_bits: u64) -> i64 {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        raise!("TypeError", "object is not awaitable");
    };
    cancel_tokens();
    let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 {
        return 0;
    }
    let prev_task = CURRENT_TASK.with(|cell| {
        let prev = cell.get();
        cell.set(task_ptr as usize);
        prev
    });
    let token = ensure_task_token(task_ptr, current_token_id());
    let prev_token = set_current_token(token);
    BLOCK_ON_TASK.with(|cell| cell.set(task_ptr as usize));
    let result = loop {
        let res = call_poll_fn(poll_fn_addr, task_ptr);
        let pending = res == pending_bits_i64();
        record_async_poll(task_ptr, pending, "block_on");
        if pending {
            if let Some(deadline) = SLEEP_QUEUE.take_blocking_deadline(task_ptr) {
                let now = Instant::now();
                if deadline > now {
                    std::thread::sleep(deadline - now);
                }
            } else {
                std::thread::yield_now();
            }
            continue;
        }
        break res;
    };
    BLOCK_ON_TASK.with(|cell| cell.set(0));
    set_current_token(prev_token);
    CURRENT_TASK.with(|cell| cell.set(prev_task));
    clear_task_token(task_ptr);
    result
}

#[no_mangle]
pub extern "C" fn molt_future_poll_fn(future_bits: u64) -> u64 {
    let obj = obj_from_bits(future_bits);
    let Some(ptr) = obj.as_ptr() else {
        if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
            eprintln!(
                "Molt awaitable debug: bits=0x{:x} type={}",
                future_bits,
                type_name(obj)
            );
        }
        raise_exception::<()>("TypeError", "object is not awaitable");
        return 0;
    };
    unsafe {
        let header = header_from_obj_ptr(ptr);
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                let mut class_name = None;
                if object_type_id(ptr) == TYPE_ID_OBJECT {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_name = Some(class_name_for_error(class_bits));
                    }
                }
                eprintln!(
                    "Molt awaitable debug: bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                    future_bits,
                    type_name(obj),
                    class_name.as_deref().unwrap_or("-"),
                    (*header).state,
                    (*header).size
                );
            }
            raise_exception::<()>("TypeError", "object is not awaitable");
            return 0;
        }
        poll_fn_addr
    }
}

#[no_mangle]
pub extern "C" fn molt_future_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    let total_size = std::mem::size_of::<MoltHeader>() + closure_size as usize;
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let header = ptr as *mut MoltHeader;
        (*header).type_id = TYPE_ID_OBJECT;
        (*header).ref_count.store(1, AtomicOrdering::Relaxed);
        (*header).poll_fn = poll_fn_addr;
        (*header).state = 0;
        (*header).size = total_size;
        (*header).flags = 0;
        let obj_ptr = ptr.add(std::mem::size_of::<MoltHeader>());
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
            eprintln!(
                "Molt future init debug: bits=0x{:x} poll=0x{:x} size={}",
                obj_bits,
                poll_fn_addr,
                (*header).size
            );
        }
        obj_bits
    }
}

#[no_mangle]
pub extern "C" fn molt_async_sleep_new(delay_bits: u64, result_bits: u64) -> u64 {
    let obj_bits = molt_future_new(
        molt_async_sleep as usize as u64,
        (2 * std::mem::size_of::<u64>()) as u64,
    );
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr = delay_bits;
        *payload_ptr.add(1) = result_bits;
        inc_ref_bits(delay_bits);
        inc_ref_bits(result_bits);
    }
    obj_bits
}

/// # Safety
/// - `obj_bits` must be a valid pointer if the runtime associates a future with it.
#[no_mangle]
pub unsafe extern "C" fn molt_async_sleep(obj_bits: u64) -> i64 {
    let _obj_ptr = ptr_from_bits(obj_bits);
    if _obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(_obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_len = payload_bytes / std::mem::size_of::<u64>();
    let payload_ptr = _obj_ptr as *mut u64;

    if (*header).state == 0 {
        let delay_secs = if payload_len >= 1 {
            let delay_bits = *payload_ptr;
            let float_bits = molt_float_from_obj(delay_bits);
            let delay_obj = obj_from_bits(float_bits);
            delay_obj.as_float().unwrap_or(0.0)
        } else {
            0.0
        };
        let delay_secs = if delay_secs.is_finite() && delay_secs > 0.0 {
            delay_secs
        } else {
            0.0
        };
        if payload_len >= 1 {
            let deadline = monotonic_now_secs() + delay_secs;
            *payload_ptr = MoltObject::from_float(deadline).bits();
        }
        (*header).state = 1;
        return pending_bits_i64();
    }

    if payload_len >= 1 {
        let deadline_obj = obj_from_bits(*payload_ptr);
        if let Some(deadline) = to_f64(deadline_obj) {
            if deadline.is_finite() && monotonic_now_secs() < deadline {
                return pending_bits_i64();
            }
        }
    }

    let result_bits = if payload_len >= 2 {
        *payload_ptr.add(1)
    } else {
        MoltObject::none().bits()
    };
    inc_ref_bits(result_bits);
    result_bits as i64
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt future allocated with payload slots.
#[no_mangle]
pub unsafe extern "C" fn molt_anext_default_poll(obj_bits: u64) -> i64 {
    let _obj_ptr = ptr_from_bits(obj_bits);
    if _obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(_obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return MoltObject::none().bits() as i64;
    }
    let payload_ptr = _obj_ptr as *mut u64;
    let iter_bits = *payload_ptr;
    let default_bits = *payload_ptr.add(1);
    if (*header).state == 0 {
        let await_bits = molt_anext(iter_bits);
        inc_ref_bits(await_bits);
        *payload_ptr.add(2) = await_bits;
        (*header).state = 1;
    }
    let await_bits = *payload_ptr.add(2);
    let Some(await_ptr) = maybe_ptr_from_bits(await_bits) else {
        return MoltObject::none().bits() as i64;
    };
    let await_header = header_from_obj_ptr(await_ptr);
    let poll_fn_addr = (*await_header).poll_fn;
    if poll_fn_addr == 0 {
        return MoltObject::none().bits() as i64;
    }
    let res = call_poll_fn(poll_fn_addr, await_ptr);
    if res == pending_bits_i64() {
        return res;
    }
    if exception_pending() {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        dec_ref_bits(kind_bits);
        if kind.as_deref() == Some("StopAsyncIteration") {
            molt_exception_clear();
            dec_ref_bits(exc_bits);
            inc_ref_bits(default_bits);
            return default_bits as i64;
        }
        dec_ref_bits(exc_bits);
    }
    res
}

/// # Safety
/// - `task_bits` must be a valid Molt task pointer.
/// - `future_bits` must point to a valid Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_sleep_register(task_bits: u64, future_bits: u64) -> u64 {
    let task_ptr = ptr_from_bits(task_bits);
    let future_ptr = ptr_from_bits(future_bits);
    if task_ptr.is_null() || future_ptr.is_null() {
        return 0;
    }
    let header = header_from_obj_ptr(future_ptr);
    if (*header).poll_fn != molt_async_sleep as usize as u64 {
        return 0;
    }
    if (*header).state == 0 {
        return 0;
    }
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < std::mem::size_of::<u64>() {
        return 0;
    }
    let payload_ptr = future_ptr as *mut u64;
    let deadline_obj = obj_from_bits(*payload_ptr);
    let Some(deadline_secs) = to_f64(deadline_obj) else {
        return 0;
    };
    if !deadline_secs.is_finite() {
        return 0;
    }
    let deadline = instant_from_monotonic_secs(deadline_secs);
    if deadline <= Instant::now() {
        return 0;
    }
    #[cfg(target_arch = "wasm32")]
    {
        SLEEP_QUEUE.register_blocking(task_ptr, deadline);
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_block_on_task(task_ptr) {
            SLEEP_QUEUE.register_blocking(task_ptr, deadline);
        } else {
            SLEEP_QUEUE.register_scheduler(task_ptr, deadline);
        }
        1
    }
}

// --- NaN-boxed ops ---

#[no_mangle]
pub extern "C" fn molt_add(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        let res = li as i128 + ri as i128;
        return int_bits_from_i128(res);
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
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big + r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        return MoltObject::from_float(lf + rf).bits();
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_sub(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        let res = li as i128 - ri as i128;
        return int_bits_from_i128(res);
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big - r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        return MoltObject::from_float(lf - rf).bits();
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if is_set_like_type(ltype) && is_set_like_type(rtype) {
                return set_like_difference(lp, rp, ltype);
            }
        }
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
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        let res = li as i128 * ri as i128;
        return int_bits_from_i128(res);
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
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big * r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        return MoltObject::from_float(lf * rf).bits();
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_div(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        if rf == 0.0 {
            raise!("ZeroDivisionError", "division by zero");
        }
        return MoltObject::from_float(lf / rf).bits();
    }
    if bigint_ptr_from_bits(a).is_some() || bigint_ptr_from_bits(b).is_some() {
        raise!("OverflowError", "int too large to convert to float");
    }
    raise!("TypeError", "unsupported operand type(s) for /");
}

#[no_mangle]
pub extern "C" fn molt_floordiv(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if ri == 0 {
            raise!("ZeroDivisionError", "integer division or modulo by zero");
        }
        return MoltObject::from_int(li.div_euclid(ri)).bits();
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        if r_big.is_zero() {
            raise!("ZeroDivisionError", "integer division or modulo by zero");
        }
        let res = l_big.div_floor(&r_big);
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        if rf == 0.0 {
            raise!("ZeroDivisionError", "float floor division by zero");
        }
        return MoltObject::from_float((lf / rf).floor()).bits();
    }
    raise!("TypeError", "unsupported operand type(s) for //");
}

#[no_mangle]
pub extern "C" fn molt_mod(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if ri == 0 {
            raise!("ZeroDivisionError", "integer division or modulo by zero");
        }
        let mut rem = li % ri;
        if rem != 0 && (rem > 0) != (ri > 0) {
            rem += ri;
        }
        return MoltObject::from_int(rem).bits();
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        if r_big.is_zero() {
            raise!("ZeroDivisionError", "integer division or modulo by zero");
        }
        let res = l_big.mod_floor(&r_big);
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        if rf == 0.0 {
            raise!("ZeroDivisionError", "float modulo");
        }
        let mut rem = lf % rf;
        if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
            rem += rf;
        }
        return MoltObject::from_float(rem).bits();
    }
    raise!("TypeError", "unsupported operand type(s) for %");
}

fn pow_i64_checked(base: i64, exp: i64) -> Option<i64> {
    if exp < 0 {
        return None;
    }
    let mut result: i128 = 1;
    let mut base_val: i128 = base as i128;
    let mut exp_val = exp as u64;
    let max = (1i128 << 46) - 1;
    let min = -(1i128 << 46);
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = result.saturating_mul(base_val);
            if result > max || result < min {
                return None;
            }
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = base_val.saturating_mul(base_val);
            if base_val > max || base_val < min {
                return None;
            }
        }
    }
    Some(result as i64)
}

fn mod_py_i128(value: i128, modulus: i128) -> i128 {
    let mut rem = value % modulus;
    if rem != 0 && (rem > 0) != (modulus > 0) {
        rem += modulus;
    }
    rem
}

fn mod_pow_i128(mut base: i128, exp: i64, modulus: i128) -> i128 {
    let mut result: i128 = 1;
    base = mod_py_i128(base, modulus);
    let mut exp_val = exp as u64;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = mod_py_i128(result * base, modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base = mod_py_i128(base * base, modulus);
        }
    }
    mod_py_i128(result, modulus)
}

fn egcd_i128(a: i128, b: i128) -> (i128, i128, i128) {
    if b == 0 {
        return (a, 1, 0);
    }
    let (g, x, y) = egcd_i128(b, a % b);
    (g, y, x - (a / b) * y)
}

fn mod_inverse_i128(value: i128, modulus: i128) -> Option<i128> {
    let (g, x, _) = egcd_i128(value, modulus);
    if g == 1 || g == -1 {
        Some(mod_py_i128(x, modulus))
    } else {
        None
    }
}

fn mod_pow_bigint(base: &BigInt, exp: u64, modulus: &BigInt) -> BigInt {
    let mut result = BigInt::from(1);
    let mut base_val = base.mod_floor(modulus);
    let mut exp_val = exp;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = (result * &base_val).mod_floor(modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = (&base_val * &base_val).mod_floor(modulus);
        }
    }
    result
}

fn egcd_bigint(a: BigInt, b: BigInt) -> (BigInt, BigInt, BigInt) {
    if b.is_zero() {
        return (a, BigInt::from(1), BigInt::from(0));
    }
    let (q, r) = a.div_mod_floor(&b);
    let (g, x, y) = egcd_bigint(b, r);
    (g, y.clone(), x - q * y)
}

fn mod_inverse_bigint(value: BigInt, modulus: &BigInt) -> Option<BigInt> {
    let (g, x, _) = egcd_bigint(value, modulus.clone());
    if g == BigInt::from(1) || g == BigInt::from(-1) {
        Some(x.mod_floor(modulus))
    } else {
        None
    }
}

#[no_mangle]
pub extern "C" fn molt_pow(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if ri >= 0 {
            if let Some(res) = pow_i64_checked(li, ri) {
                return MoltObject::from_int(res).bits();
            }
            let res = BigInt::from(li).pow(ri as u32);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(res);
        }
        let lf = li as f64;
        let rf = ri as f64;
        if lf == 0.0 && rf < 0.0 {
            raise!(
                "ZeroDivisionError",
                "0.0 cannot be raised to a negative power"
            );
        }
        return MoltObject::from_float(lf.powf(rf)).bits();
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        if let Some(exp) = r_big.to_u64() {
            let res = l_big.pow(exp as u32);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(res);
        }
        if r_big.is_negative() {
            if let Some(lf) = l_big.to_f64() {
                let rf = r_big.to_f64().unwrap_or(f64::NEG_INFINITY);
                if lf == 0.0 && rf < 0.0 {
                    raise!(
                        "ZeroDivisionError",
                        "0.0 cannot be raised to a negative power"
                    );
                }
                return MoltObject::from_float(lf.powf(rf)).bits();
            }
        }
        raise!("OverflowError", "exponent too large");
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        if lf == 0.0 && rf < 0.0 {
            raise!(
                "ZeroDivisionError",
                "0.0 cannot be raised to a negative power"
            );
        }
        return MoltObject::from_float(lf.powf(rf)).bits();
    }
    raise!("TypeError", "unsupported operand type(s) for **");
}

#[no_mangle]
pub extern "C" fn molt_pow_mod(a: u64, b: u64, m: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    let mod_obj = obj_from_bits(m);
    if let (Some(li), Some(ri), Some(mi)) = (to_i64(lhs), to_i64(rhs), to_i64(mod_obj)) {
        let (base, exp, modulus) = (li as i128, ri, mi as i128);
        if modulus == 0 {
            raise!("ValueError", "pow() 3rd argument cannot be 0");
        }
        let result = if exp < 0 {
            let mod_abs = modulus.abs();
            let base_mod = mod_py_i128(base, mod_abs);
            let Some(inv) = mod_inverse_i128(base_mod, mod_abs) else {
                raise!("ValueError", "base is not invertible for the given modulus");
            };
            let inv_mod = mod_py_i128(inv, modulus);
            mod_pow_i128(inv_mod, -exp, modulus)
        } else {
            mod_pow_i128(base, exp, modulus)
        };
        return MoltObject::from_int(result as i64).bits();
    }
    if let (Some(base), Some(exp), Some(modulus)) =
        (to_bigint(lhs), to_bigint(rhs), to_bigint(mod_obj))
    {
        if modulus.is_zero() {
            raise!("ValueError", "pow() 3rd argument cannot be 0");
        }
        let result = if exp.is_negative() {
            let mod_abs = modulus.abs();
            let base_mod = base.mod_floor(&mod_abs);
            let Some(inv) = mod_inverse_bigint(base_mod, &mod_abs) else {
                raise!("ValueError", "base is not invertible for the given modulus");
            };
            let inv_mod = inv.mod_floor(&modulus);
            let neg_exp = -exp;
            if neg_exp.to_u64().is_none() {
                raise!("OverflowError", "exponent too large");
            }
            let exp_u64 = neg_exp.to_u64().unwrap();
            mod_pow_bigint(&inv_mod, exp_u64, &modulus)
        } else {
            if exp.to_u64().is_none() {
                raise!("OverflowError", "exponent too large");
            }
            let exp_u64 = exp.to_u64().unwrap();
            mod_pow_bigint(&base, exp_u64, &modulus)
        };
        if let Some(i) = bigint_to_inline(&result) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(result);
    }
    raise!(
        "TypeError",
        "pow() 3rd argument not allowed unless all arguments are integers",
    );
}

fn round_half_even(val: f64) -> f64 {
    if !val.is_finite() {
        return val;
    }
    let floor = val.floor();
    let ceil = val.ceil();
    let diff_floor = (val - floor).abs();
    let diff_ceil = (ceil - val).abs();
    if diff_floor < diff_ceil {
        return floor;
    }
    if diff_ceil < diff_floor {
        return ceil;
    }
    if floor.abs() > i64::MAX as f64 {
        return floor;
    }
    let floor_int = floor as i64;
    if floor_int & 1 == 0 {
        floor
    } else {
        ceil
    }
}

fn bigint_from_f64_trunc(val: f64) -> BigInt {
    if val == 0.0 {
        return BigInt::from(0);
    }
    let bits = val.to_bits();
    let sign = if (bits >> 63) != 0 { -1 } else { 1 };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (mantissa, exp) = if exp_bits == 0 {
        (frac_bits, 1 - 1023 - 52)
    } else {
        ((1u64 << 52) | frac_bits, exp_bits - 1023 - 52)
    };
    let mut big = BigInt::from(mantissa);
    if exp >= 0 {
        big <<= exp as usize;
    } else {
        big >>= (-exp) as usize;
    }
    if sign < 0 {
        -big
    } else {
        big
    }
}

fn round_float_ndigits(val: f64, ndigits: i64) -> f64 {
    if !val.is_finite() {
        return val;
    }
    if ndigits == 0 {
        return round_half_even(val);
    }
    if ndigits > 0 {
        if ndigits > 308 {
            return val;
        }
        let formatted = format!("{:.*}", ndigits as usize, val);
        return formatted.parse::<f64>().unwrap_or(val);
    }
    let factor = 10f64.powi((-ndigits) as i32);
    if !factor.is_finite() {
        return if val.is_sign_negative() { -0.0 } else { 0.0 };
    }
    if factor == 0.0 {
        return val;
    }
    let scaled = val / factor;
    round_half_even(scaled) * factor
}

#[no_mangle]
pub extern "C" fn molt_round(val_bits: u64, ndigits_bits: u64, has_ndigits_bits: u64) -> u64 {
    let val = obj_from_bits(val_bits);
    let has_ndigits = to_i64(obj_from_bits(has_ndigits_bits)).unwrap_or(0) != 0;
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        if !has_ndigits {
            return val_bits;
        }
        let ndigits_obj = obj_from_bits(ndigits_bits);
        if ndigits_obj.is_none() {
            return val_bits;
        }
        let ndigits = index_i64_from_obj(ndigits_bits, "round() ndigits must be int");
        if ndigits >= 0 {
            return val_bits;
        }
        let exp = (-ndigits) as u32;
        let value = unsafe { bigint_ref(ptr).clone() };
        let pow = BigInt::from(10).pow(exp);
        if pow.is_zero() {
            return val_bits;
        }
        let div = value.div_floor(&pow);
        let rem = value.mod_floor(&pow);
        let twice = &rem * 2;
        let mut rounded = div;
        if twice > pow || (twice == pow && !rounded.is_even()) {
            if value.is_negative() {
                rounded -= 1;
            } else {
                rounded += 1;
            }
        }
        let result = rounded * pow;
        if let Some(i) = bigint_to_inline(&result) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(result);
    }
    if !val.is_int() && !val.is_bool() && !val.is_float() {
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let round_name_bits = intern_static_name(&INTERN_ROUND_NAME, b"__round__");
                if let Some(call_bits) = attr_lookup_ptr(ptr, round_name_bits) {
                    let ndigits_obj = obj_from_bits(ndigits_bits);
                    let want_arg = has_ndigits && !ndigits_obj.is_none();
                    let arity = callable_arity(call_bits).unwrap_or(0);
                    let res_bits = if arity <= 1 {
                        if want_arg {
                            call_callable1(call_bits, ndigits_bits)
                        } else {
                            call_callable0(call_bits)
                        }
                    } else {
                        let arg_bits = if want_arg {
                            ndigits_bits
                        } else {
                            MoltObject::none().bits()
                        };
                        call_callable1(call_bits, arg_bits)
                    };
                    dec_ref_bits(call_bits);
                    return res_bits;
                }
            }
        }
    }
    if let Some(i) = to_i64(val) {
        if !has_ndigits {
            return MoltObject::from_int(i).bits();
        }
        let ndigits_obj = obj_from_bits(ndigits_bits);
        if ndigits_obj.is_none() {
            return MoltObject::from_int(i).bits();
        }
        let Some(ndigits) = to_i64(ndigits_obj) else {
            raise!("TypeError", "round() ndigits must be int");
        };
        if ndigits >= 0 {
            return MoltObject::from_int(i).bits();
        }
        let exp = (-ndigits) as u32;
        if exp > 38 {
            return MoltObject::from_int(0).bits();
        }
        let pow = 10_i128.pow(exp);
        let value = i as i128;
        if pow == 0 {
            return MoltObject::from_int(i).bits();
        }
        let div = value / pow;
        let rem = value % pow;
        let abs_rem = rem.abs();
        let twice = abs_rem.saturating_mul(2);
        let mut rounded = div;
        if twice > pow || (twice == pow && (div & 1) != 0) {
            rounded += if value >= 0 { 1 } else { -1 };
        }
        let result = rounded.saturating_mul(pow);
        return MoltObject::from_int(result as i64).bits();
    }
    if let Some(f) = to_f64(val) {
        if !has_ndigits {
            if f.is_nan() {
                raise!("ValueError", "cannot convert float NaN to integer");
            }
            if f.is_infinite() {
                raise!("OverflowError", "cannot convert float infinity to integer");
            }
            let rounded = round_half_even(f);
            let big = bigint_from_f64_trunc(rounded);
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(big);
        }
        let ndigits_obj = obj_from_bits(ndigits_bits);
        if ndigits_obj.is_none() {
            if f.is_nan() {
                raise!("ValueError", "cannot convert float NaN to integer");
            }
            if f.is_infinite() {
                raise!("OverflowError", "cannot convert float infinity to integer");
            }
            let rounded = round_half_even(f);
            let big = bigint_from_f64_trunc(rounded);
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(big);
        }
        let Some(ndigits) = to_i64(ndigits_obj) else {
            raise!("TypeError", "round() ndigits must be int");
        };
        let rounded = round_float_ndigits(f, ndigits);
        return MoltObject::from_float(rounded).bits();
    }
    raise!("TypeError", "round() expects a real number");
}

#[no_mangle]
pub extern "C" fn molt_trunc(val_bits: u64) -> u64 {
    let val = obj_from_bits(val_bits);
    if let Some(i) = to_i64(val) {
        return MoltObject::from_int(i).bits();
    }
    if bigint_ptr_from_bits(val_bits).is_some() {
        return val_bits;
    }
    if let Some(f) = to_f64(val) {
        if f.is_nan() {
            raise!("ValueError", "cannot convert float NaN to integer");
        }
        if f.is_infinite() {
            raise!("OverflowError", "cannot convert float infinity to integer");
        }
        let big = bigint_from_f64_trunc(f);
        if let Some(i) = bigint_to_inline(&big) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(big);
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let trunc_name_bits = intern_static_name(&INTERN_TRUNC_NAME, b"__trunc__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, trunc_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                return res_bits;
            }
        }
    }
    raise!("TypeError", "trunc() expects a real number");
}

fn set_like_result_type_id(type_id: u32) -> u32 {
    if type_id == TYPE_ID_FROZENSET {
        TYPE_ID_FROZENSET
    } else {
        TYPE_ID_SET
    }
}

unsafe fn set_like_new_bits(type_id: u32, capacity: usize) -> u64 {
    if type_id == TYPE_ID_FROZENSET {
        molt_frozenset_new(capacity as u64)
    } else {
        molt_set_new(capacity as u64)
    }
}

unsafe fn set_like_union(lhs_ptr: *mut u8, rhs_ptr: *mut u8, result_type_id: u32) -> u64 {
    let l_elems = set_order(lhs_ptr);
    let r_elems = set_order(rhs_ptr);
    let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
    let res_ptr = obj_from_bits(res_bits)
        .as_ptr()
        .unwrap_or(std::ptr::null_mut());
    if res_ptr.is_null() {
        return MoltObject::none().bits();
    }
    for &entry in l_elems.iter() {
        set_add_in_place(res_ptr, entry);
    }
    for &entry in r_elems.iter() {
        set_add_in_place(res_ptr, entry);
    }
    res_bits
}

unsafe fn set_like_intersection(lhs_ptr: *mut u8, rhs_ptr: *mut u8, result_type_id: u32) -> u64 {
    let l_elems = set_order(lhs_ptr);
    let r_elems = set_order(rhs_ptr);
    let (probe_elems, probe_table, output) = if l_elems.len() <= r_elems.len() {
        (r_elems, set_table(rhs_ptr), l_elems)
    } else {
        (l_elems, set_table(lhs_ptr), r_elems)
    };
    let res_bits = set_like_new_bits(result_type_id, output.len());
    let res_ptr = obj_from_bits(res_bits)
        .as_ptr()
        .unwrap_or(std::ptr::null_mut());
    if res_ptr.is_null() {
        return MoltObject::none().bits();
    }
    for &entry in output.iter() {
        if set_find_entry(probe_elems, probe_table, entry).is_some() {
            set_add_in_place(res_ptr, entry);
        }
    }
    res_bits
}

unsafe fn set_like_difference(lhs_ptr: *mut u8, rhs_ptr: *mut u8, result_type_id: u32) -> u64 {
    let l_elems = set_order(lhs_ptr);
    let r_elems = set_order(rhs_ptr);
    let r_table = set_table(rhs_ptr);
    let res_bits = set_like_new_bits(result_type_id, l_elems.len());
    let res_ptr = obj_from_bits(res_bits)
        .as_ptr()
        .unwrap_or(std::ptr::null_mut());
    if res_ptr.is_null() {
        return MoltObject::none().bits();
    }
    for &entry in l_elems.iter() {
        if set_find_entry(r_elems, r_table, entry).is_none() {
            set_add_in_place(res_ptr, entry);
        }
    }
    res_bits
}

unsafe fn set_like_symdiff(lhs_ptr: *mut u8, rhs_ptr: *mut u8, result_type_id: u32) -> u64 {
    let l_elems = set_order(lhs_ptr);
    let r_elems = set_order(rhs_ptr);
    let l_table = set_table(lhs_ptr);
    let r_table = set_table(rhs_ptr);
    let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
    let res_ptr = obj_from_bits(res_bits)
        .as_ptr()
        .unwrap_or(std::ptr::null_mut());
    if res_ptr.is_null() {
        return MoltObject::none().bits();
    }
    for &entry in l_elems.iter() {
        if set_find_entry(r_elems, r_table, entry).is_none() {
            set_add_in_place(res_ptr, entry);
        }
    }
    for &entry in r_elems.iter() {
        if set_find_entry(l_elems, l_table, entry).is_none() {
            set_add_in_place(res_ptr, entry);
        }
    }
    res_bits
}

fn binary_type_error(lhs: MoltObject, rhs: MoltObject, op: &str) -> u64 {
    let msg = format!(
        "unsupported operand type(s) for {op}: '{}' and '{}'",
        type_name(lhs),
        type_name(rhs)
    );
    raise!("TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_bit_or(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if lhs.is_bool() && rhs.is_bool() {
            return MoltObject::from_bool((li != 0) | (ri != 0)).bits();
        }
        let res = li | ri;
        if inline_int_from_i128(res as i128).is_some() {
            return MoltObject::from_int(res).bits();
        }
        return bigint_bits(BigInt::from(li) | BigInt::from(ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big | r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if is_set_like_type(ltype) && is_set_like_type(rtype) {
                return set_like_union(lp, rp, set_like_result_type_id(ltype));
            }
        }
    }
    binary_type_error(lhs, rhs, "|")
}

#[no_mangle]
pub extern "C" fn molt_bit_and(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if lhs.is_bool() && rhs.is_bool() {
            return MoltObject::from_bool((li != 0) & (ri != 0)).bits();
        }
        let res = li & ri;
        if inline_int_from_i128(res as i128).is_some() {
            return MoltObject::from_int(res).bits();
        }
        return bigint_bits(BigInt::from(li) & BigInt::from(ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big & r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if is_set_like_type(ltype) && is_set_like_type(rtype) {
                return set_like_intersection(lp, rp, set_like_result_type_id(ltype));
            }
        }
    }
    binary_type_error(lhs, rhs, "&")
}

#[no_mangle]
pub extern "C" fn molt_bit_xor(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if lhs.is_bool() && rhs.is_bool() {
            return MoltObject::from_bool((li != 0) ^ (ri != 0)).bits();
        }
        let res = li ^ ri;
        if inline_int_from_i128(res as i128).is_some() {
            return MoltObject::from_int(res).bits();
        }
        return bigint_bits(BigInt::from(li) ^ BigInt::from(ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        let res = l_big ^ r_big;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if is_set_like_type(ltype) && is_set_like_type(rtype) {
                return set_like_symdiff(lp, rp, set_like_result_type_id(ltype));
            }
        }
    }
    binary_type_error(lhs, rhs, "^")
}

#[no_mangle]
pub extern "C" fn molt_lshift(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    let shift = index_i64_from_obj(b, "shift count must be int");
    if shift < 0 {
        raise!("ValueError", "negative shift count");
    }
    let shift_u = shift as u32;
    if let Some(value) = to_i64(lhs) {
        if shift_u >= 63 {
            return bigint_bits(BigInt::from(value) << shift_u);
        }
        let res = value << shift_u;
        if inline_int_from_i128(res as i128).is_some() {
            return MoltObject::from_int(res).bits();
        }
        return bigint_bits(BigInt::from(value) << shift_u);
    }
    if let Some(value) = to_bigint(lhs) {
        let res = value << shift_u;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    binary_type_error(lhs, rhs, "<<")
}

#[no_mangle]
pub extern "C" fn molt_rshift(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    let shift = index_i64_from_obj(b, "shift count must be int");
    if shift < 0 {
        raise!("ValueError", "negative shift count");
    }
    let shift_u = shift as u32;
    if let Some(value) = to_i64(lhs) {
        let res = if shift_u >= 63 {
            if value >= 0 {
                0
            } else {
                -1
            }
        } else {
            value >> shift_u
        };
        return MoltObject::from_int(res).bits();
    }
    if let Some(value) = to_bigint(lhs) {
        let res = value >> shift_u;
        if let Some(i) = bigint_to_inline(&res) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(res);
    }
    binary_type_error(lhs, rhs, ">>")
}

#[no_mangle]
pub extern "C" fn molt_matmul(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            if object_type_id(lp) == TYPE_ID_BUFFER2D && object_type_id(rp) == TYPE_ID_BUFFER2D {
                return molt_buffer2d_matmul(a, b);
            }
        }
    }
    binary_type_error(lhs, rhs, "@")
}

fn compare_type_error(lhs: MoltObject, rhs: MoltObject, op: &str) -> u64 {
    let msg = format!(
        "'{}' not supported between instances of '{}' and '{}'",
        op,
        type_name(lhs),
        type_name(rhs),
    );
    raise!("TypeError", &msg);
}

#[derive(Clone, Copy)]
enum CompareOutcome {
    Ordered(Ordering),
    Unordered,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
enum CompareBoolOutcome {
    True,
    False,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

fn is_number(obj: MoltObject) -> bool {
    to_i64(obj).is_some() || obj.is_float() || bigint_ptr_from_bits(obj.bits()).is_some()
}

fn compare_numbers_outcome(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if let Some(ordering) = compare_numbers(lhs, rhs) {
        return CompareOutcome::Ordered(ordering);
    }
    if is_number(lhs) && is_number(rhs) {
        return CompareOutcome::Unordered;
    }
    CompareOutcome::NotComparable
}

unsafe fn compare_string_bytes(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    let l_len = string_len(lhs_ptr);
    let r_len = string_len(rhs_ptr);
    let l_bytes = std::slice::from_raw_parts(string_bytes(lhs_ptr), l_len);
    let r_bytes = std::slice::from_raw_parts(string_bytes(rhs_ptr), r_len);
    l_bytes.cmp(r_bytes)
}

unsafe fn compare_bytes_like(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    let l_len = bytes_len(lhs_ptr);
    let r_len = bytes_len(rhs_ptr);
    let l_bytes = std::slice::from_raw_parts(bytes_data(lhs_ptr), l_len);
    let r_bytes = std::slice::from_raw_parts(bytes_data(rhs_ptr), r_len);
    l_bytes.cmp(r_bytes)
}

unsafe fn compare_sequence(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> CompareOutcome {
    let lhs = seq_vec_ref(lhs_ptr);
    let rhs = seq_vec_ref(rhs_ptr);
    let common = lhs.len().min(rhs.len());
    for idx in 0..common {
        let l_bits = lhs[idx];
        let r_bits = rhs[idx];
        if obj_eq(obj_from_bits(l_bits), obj_from_bits(r_bits)) {
            continue;
        }
        return compare_objects(obj_from_bits(l_bits), obj_from_bits(r_bits));
    }
    CompareOutcome::Ordered(lhs.len().cmp(&rhs.len()))
}

fn compare_objects_builtin(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return CompareOutcome::NotComparable;
    };
    unsafe {
        let ltype = object_type_id(lhs_ptr);
        let rtype = object_type_id(rhs_ptr);
        if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
            return CompareOutcome::Ordered(compare_string_bytes(lhs_ptr, rhs_ptr));
        }
        if (ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY)
            && (rtype == TYPE_ID_BYTES || rtype == TYPE_ID_BYTEARRAY)
        {
            return CompareOutcome::Ordered(compare_bytes_like(lhs_ptr, rhs_ptr));
        }
        if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
            return compare_sequence(lhs_ptr, rhs_ptr);
        }
        if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
            return compare_sequence(lhs_ptr, rhs_ptr);
        }
    }
    CompareOutcome::NotComparable
}

fn ordering_matches(ordering: Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Lt => ordering == Ordering::Less,
        CompareOp::Le => ordering != Ordering::Greater,
        CompareOp::Gt => ordering == Ordering::Greater,
        CompareOp::Ge => ordering != Ordering::Less,
    }
}

fn compare_builtin_bool(lhs: MoltObject, rhs: MoltObject, op: CompareOp) -> CompareBoolOutcome {
    match compare_objects_builtin(lhs, rhs) {
        CompareOutcome::Ordered(ordering) => {
            if ordering_matches(ordering, op) {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            }
        }
        CompareOutcome::Unordered => CompareBoolOutcome::False,
        CompareOutcome::NotComparable => CompareBoolOutcome::NotComparable,
        CompareOutcome::Error => CompareBoolOutcome::Error,
    }
}

fn rich_compare_bool(
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareBoolOutcome {
    unsafe {
        if let Some(lhs_ptr) = lhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr(lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(call_bits, rhs.bits());
                dec_ref_bits(call_bits);
                if exception_pending() {
                    dec_ref_bits(res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(res_bits) {
                    dec_ref_bits(res_bits);
                } else {
                    let truthy = is_truthy(obj_from_bits(res_bits));
                    dec_ref_bits(res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_pending() {
                return CompareBoolOutcome::Error;
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr(rhs_ptr, reverse_name_bits) {
                let res_bits = call_callable1(call_bits, lhs.bits());
                dec_ref_bits(call_bits);
                if exception_pending() {
                    dec_ref_bits(res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(res_bits) {
                    dec_ref_bits(res_bits);
                } else {
                    let truthy = is_truthy(obj_from_bits(res_bits));
                    dec_ref_bits(res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_pending() {
                return CompareBoolOutcome::Error;
            }
        }
    }
    CompareBoolOutcome::NotComparable
}

fn rich_compare_order(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    let lt_name_bits = intern_static_name(&INTERN_LT_NAME, b"__lt__");
    let gt_name_bits = intern_static_name(&INTERN_GT_NAME, b"__gt__");
    match rich_compare_bool(lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
        CompareBoolOutcome::False => {}
        CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => return CompareOutcome::Error,
    }
    match rich_compare_bool(rhs, lhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
        CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
        CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => CompareOutcome::Error,
    }
}

fn compare_objects(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    match compare_objects_builtin(lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    rich_compare_order(lhs, rhs)
}

fn compare_numbers(lhs: MoltObject, rhs: MoltObject) -> Option<Ordering> {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return Some(li.cmp(&ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return Some(l_big.cmp(&r_big));
    }
    if let Some(ptr) = bigint_ptr_from_bits(lhs.bits()) {
        if let Some(f) = to_f64(rhs) {
            return compare_bigint_float(unsafe { bigint_ref(ptr) }, f);
        }
    }
    if let Some(ptr) = bigint_ptr_from_bits(rhs.bits()) {
        if let Some(f) = to_f64(lhs) {
            return compare_bigint_float(unsafe { bigint_ref(ptr) }, f).map(Ordering::reverse);
        }
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return lf.partial_cmp(&rf);
    }
    None
}

fn float_pair_from_obj(lhs: MoltObject, rhs: MoltObject) -> Option<(f64, f64)> {
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return Some((lf, rf));
    }
    if (lhs.is_float() || rhs.is_float())
        && (bigint_ptr_from_bits(lhs.bits()).is_some()
            || bigint_ptr_from_bits(rhs.bits()).is_some())
    {
        raise!("OverflowError", "int too large to convert to float");
    }
    None
}

#[no_mangle]
pub extern "C" fn molt_lt(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    match compare_builtin_bool(lhs, rhs, CompareOp::Lt) {
        CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => return MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => {}
    }
    let lt_name_bits = intern_static_name(&INTERN_LT_NAME, b"__lt__");
    let gt_name_bits = intern_static_name(&INTERN_GT_NAME, b"__gt__");
    match rich_compare_bool(lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => compare_type_error(lhs, rhs, "<"),
    }
}

#[no_mangle]
pub extern "C" fn molt_le(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    match compare_builtin_bool(lhs, rhs, CompareOp::Le) {
        CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => return MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => {}
    }
    let le_name_bits = intern_static_name(&INTERN_LE_NAME, b"__le__");
    let ge_name_bits = intern_static_name(&INTERN_GE_NAME, b"__ge__");
    match rich_compare_bool(lhs, rhs, le_name_bits, ge_name_bits) {
        CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => compare_type_error(lhs, rhs, "<="),
    }
}

#[no_mangle]
pub extern "C" fn molt_gt(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    match compare_builtin_bool(lhs, rhs, CompareOp::Gt) {
        CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => return MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => {}
    }
    let gt_name_bits = intern_static_name(&INTERN_GT_NAME, b"__gt__");
    let lt_name_bits = intern_static_name(&INTERN_LT_NAME, b"__lt__");
    match rich_compare_bool(lhs, rhs, gt_name_bits, lt_name_bits) {
        CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => compare_type_error(lhs, rhs, ">"),
    }
}

#[no_mangle]
pub extern "C" fn molt_ge(a: u64, b: u64) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    match compare_builtin_bool(lhs, rhs, CompareOp::Ge) {
        CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => return MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => {}
    }
    let ge_name_bits = intern_static_name(&INTERN_GE_NAME, b"__ge__");
    let le_name_bits = intern_static_name(&INTERN_LE_NAME, b"__le__");
    match rich_compare_bool(lhs, rhs, ge_name_bits, le_name_bits) {
        CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
        CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
        CompareBoolOutcome::Error => MoltObject::none().bits(),
        CompareBoolOutcome::NotComparable => compare_type_error(lhs, rhs, ">="),
    }
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
pub extern "C" fn molt_repr_from_obj(val_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    let rendered = format_obj(obj);
    let ptr = alloc_string(rendered.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn ascii_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let code = ch as u32;
            if code <= 0xff {
                out.push_str(&format!("\\x{:02x}", code));
            } else if code <= 0xffff {
                out.push_str(&format!("\\u{:04x}", code));
            } else {
                out.push_str(&format!("\\U{:08x}", code));
            }
        }
    }
    out
}

#[no_mangle]
pub extern "C" fn molt_ascii_from_obj(val_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    let rendered = format_obj(obj);
    let escaped = ascii_escape(&rendered);
    let ptr = alloc_string(escaped.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn format_int_base(value: &BigInt, base: u32, prefix: &str, upper: bool) -> String {
    let negative = value.is_negative();
    let mut abs_val = if negative { -value } else { value.clone() };
    if abs_val.is_zero() {
        abs_val = BigInt::from(0);
    }
    let mut digits = abs_val.to_str_radix(base);
    if upper {
        digits = digits.to_uppercase();
    }
    if negative {
        format!("-{prefix}{digits}")
    } else {
        format!("{prefix}{digits}")
    }
}

#[no_mangle]
pub extern "C" fn molt_bin_builtin(val_bits: u64) -> u64 {
    let type_name = class_name_for_error(type_of_bits(val_bits));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let Some(value) = index_bigint_from_obj(val_bits, &msg) else {
        return MoltObject::none().bits();
    };
    let text = format_int_base(&value, 2, "0b", false);
    let ptr = alloc_string(text.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_oct_builtin(val_bits: u64) -> u64 {
    let type_name = class_name_for_error(type_of_bits(val_bits));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let Some(value) = index_bigint_from_obj(val_bits, &msg) else {
        return MoltObject::none().bits();
    };
    let text = format_int_base(&value, 8, "0o", false);
    let ptr = alloc_string(text.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_hex_builtin(val_bits: u64) -> u64 {
    let type_name = class_name_for_error(type_of_bits(val_bits));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let Some(value) = index_bigint_from_obj(val_bits, &msg) else {
        return MoltObject::none().bits();
    };
    let text = format_int_base(&value, 16, "0x", false);
    let ptr = alloc_string(text.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn parse_float_from_bytes(bytes: &[u8]) -> Result<f64, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let trimmed = text.trim();
    trimmed.parse::<f64>().map_err(|_| ())
}

fn parse_int_from_str(text: &str, base: i64) -> Result<(BigInt, i64), ()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    let mut sign = 1i32;
    let mut digits = trimmed;
    if let Some(rest) = digits.strip_prefix('+') {
        digits = rest;
    } else if let Some(rest) = digits.strip_prefix('-') {
        digits = rest;
        sign = -1;
    }
    let mut base_val = base;
    if base_val == 0 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            base_val = 16;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            base_val = 8;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            base_val = 2;
            digits = rest;
        } else {
            base_val = 10;
        }
    } else if base_val == 16 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            digits = rest;
        }
    } else if base_val == 8 {
        if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            digits = rest;
        }
    } else if base_val == 2 {
        if let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            digits = rest;
        }
    }
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err(());
    }
    let parsed = BigInt::parse_bytes(digits.as_bytes(), base_val as u32).ok_or(())?;
    let parsed = if sign < 0 { -parsed } else { parsed };
    Ok((parsed, base_val))
}

/// # Safety
/// - `ptr_bits` must be null or valid for `len_bits` bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_bigint_from_str(ptr_bits: u64, len_bits: u64) -> u64 {
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let text = match std::str::from_utf8(bytes) {
        Ok(val) => val,
        Err(_) => raise!("ValueError", "invalid literal for int()"),
    };
    let (parsed, _base_used) = match parse_int_from_str(text, 10) {
        Ok(val) => val,
        Err(_) => raise!("ValueError", "invalid literal for int()"),
    };
    if let Some(i) = bigint_to_inline(&parsed) {
        return MoltObject::from_int(i).bits();
    }
    bigint_bits(parsed)
}

#[no_mangle]
pub extern "C" fn molt_float_from_obj(val_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    if obj.is_float() {
        return val_bits;
    }
    if let Some(i) = to_i64(obj) {
        return MoltObject::from_float(i as f64).bits();
    }
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        let big = unsafe { bigint_ref(ptr) };
        if let Some(val) = big.to_f64() {
            return MoltObject::from_float(val).bits();
        }
        raise!("OverflowError", "int too large to convert to float");
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                if let Ok(parsed) = parse_float_from_bytes(bytes) {
                    return MoltObject::from_float(parsed).bits();
                }
                let rendered = String::from_utf8_lossy(bytes);
                let msg = format!("could not convert string to float: '{rendered}'");
                raise!("ValueError", &msg);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                if let Ok(parsed) = parse_float_from_bytes(bytes) {
                    return MoltObject::from_float(parsed).bits();
                }
                let rendered = String::from_utf8_lossy(bytes);
                let msg = format!("could not convert string to float: '{rendered}'");
                raise!("ValueError", &msg);
            }
            let float_name_bits = intern_static_name(&INTERN_FLOAT_NAME, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, float_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if res_obj.is_float() {
                    return res_bits;
                }
                let owner = class_name_for_error(type_of_bits(val_bits));
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                raise!("TypeError", &msg);
            }
            let index_name_bits = intern_static_name(&INTERN_INDEX_NAME, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, index_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    return MoltObject::from_float(i as f64).bits();
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise!("TypeError", &msg);
            }
        }
    }
    raise!("TypeError", "float() argument must be a string or a number");
}

#[no_mangle]
pub extern "C" fn molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    let has_base = to_i64(obj_from_bits(has_base_bits)).unwrap_or(0) != 0;
    let base_val = if has_base {
        let base = index_i64_from_obj(base_bits, "int() base must be int");
        if base != 0 && !(2..=36).contains(&base) {
            raise!("ValueError", "base must be 0 or between 2 and 36");
        }
        base
    } else {
        10
    };
    let invalid_literal = |base: i64, literal: &str| -> u64 {
        let msg = format!("invalid literal for int() with base {base}: '{literal}'");
        raise!("ValueError", &msg);
    };
    if has_base {
        let Some(ptr) = maybe_ptr_from_bits(val_bits) else {
            raise!(
                "TypeError",
                "int() can't convert non-string with explicit base"
            );
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_STRING && type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY
            {
                raise!(
                    "TypeError",
                    "int() can't convert non-string with explicit base"
                );
            }
        }
    }
    if !has_base {
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
        }
        if let Some(f) = to_f64(obj) {
            if f.is_nan() {
                raise!("ValueError", "cannot convert float NaN to integer");
            }
            if f.is_infinite() {
                raise!("OverflowError", "cannot convert float infinity to integer");
            }
            let big = bigint_from_f64_trunc(f);
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(big);
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let text = match std::str::from_utf8(bytes) {
                    Ok(val) => val,
                    Err(_) => return invalid_literal(base_val, "<bytes>"),
                };
                let base = if has_base { base_val } else { 10 };
                let (parsed, _base_used) = match parse_int_from_str(text, base) {
                    Ok(val) => val,
                    Err(_) => return invalid_literal(base, text),
                };
                if let Some(i) = bigint_to_inline(&parsed) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(parsed);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = String::from_utf8_lossy(bytes);
                let base = if has_base { base_val } else { 10 };
                let (parsed, _base_used) = match parse_int_from_str(&text, base) {
                    Ok(val) => val,
                    Err(_) => return invalid_literal(base, &format!("b'{text}'")),
                };
                if let Some(i) = bigint_to_inline(&parsed) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(parsed);
            }
            if !has_base {
                let int_name_bits = intern_static_name(&INTERN_INT_NAME, b"__int__");
                if let Some(call_bits) = attr_lookup_ptr(ptr, int_name_bits) {
                    let res_bits = call_callable0(call_bits);
                    dec_ref_bits(call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        return MoltObject::from_int(i).bits();
                    }
                    if bigint_ptr_from_bits(res_bits).is_some() {
                        return res_bits;
                    }
                    let res_type = class_name_for_error(type_of_bits(res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(res_bits);
                    }
                    let msg = format!("__int__ returned non-int (type {res_type})");
                    raise!("TypeError", &msg);
                }
                let index_name_bits = intern_static_name(&INTERN_INDEX_NAME, b"__index__");
                if let Some(call_bits) = attr_lookup_ptr(ptr, index_name_bits) {
                    let res_bits = call_callable0(call_bits);
                    dec_ref_bits(call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        return MoltObject::from_int(i).bits();
                    }
                    if bigint_ptr_from_bits(res_bits).is_some() {
                        return res_bits;
                    }
                    let res_type = class_name_for_error(type_of_bits(res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(res_bits);
                    }
                    let msg = format!("__index__ returned non-int (type {res_type})");
                    raise!("TypeError", &msg);
                }
            }
        }
    }
    if has_base {
        raise!("ValueError", "invalid literal for int()");
    }
    raise!("TypeError", "int() argument must be a string or a number");
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
        TYPE_TAG_INT => obj.is_int() || bigint_ptr_from_bits(val_bits).is_some(),
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
        TYPE_TAG_SET => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET }),
        TYPE_TAG_FROZENSET => obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FROZENSET }),
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

#[no_mangle]
pub extern "C" fn molt_profile_dump() {
    if !profile_enabled() {
        return;
    }
    let call_dispatch = CALL_DISPATCH_COUNT.load(AtomicOrdering::Relaxed);
    let cache_hit = STRING_COUNT_CACHE_HIT.load(AtomicOrdering::Relaxed);
    let cache_miss = STRING_COUNT_CACHE_MISS.load(AtomicOrdering::Relaxed);
    let struct_stores = STRUCT_FIELD_STORE_COUNT.load(AtomicOrdering::Relaxed);
    let attr_lookups = ATTR_LOOKUP_COUNT.load(AtomicOrdering::Relaxed);
    let layout_guard = LAYOUT_GUARD_COUNT.load(AtomicOrdering::Relaxed);
    let layout_guard_fail = LAYOUT_GUARD_FAIL.load(AtomicOrdering::Relaxed);
    let allocs = ALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let async_polls = ASYNC_POLL_COUNT.load(AtomicOrdering::Relaxed);
    let async_pending = ASYNC_PENDING_COUNT.load(AtomicOrdering::Relaxed);
    let async_wakeups = ASYNC_WAKEUP_COUNT.load(AtomicOrdering::Relaxed);
    let async_sleep_reg = ASYNC_SLEEP_REGISTER_COUNT.load(AtomicOrdering::Relaxed);
    eprintln!(
        "molt_profile call_dispatch={} string_count_cache_hit={} string_count_cache_miss={} struct_field_store={} attr_lookup={} layout_guard={} layout_guard_fail={} alloc_count={} async_polls={} async_pending={} async_wakeups={} async_sleep_register={}",
        call_dispatch,
        cache_hit,
        cache_miss,
        struct_stores,
        attr_lookups,
        layout_guard,
        layout_guard_fail,
        allocs,
        async_polls,
        async_pending,
        async_wakeups,
        async_sleep_reg
    );
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
            if type_id == TYPE_ID_SET {
                return MoltObject::from_int(set_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_FROZENSET {
                return MoltObject::from_int(set_len(ptr) as i64).bits();
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
            if let Some(name_bits) = attr_name_bits_from_bytes(b"__len__") {
                let call_bits = attr_lookup_ptr(ptr, name_bits);
                dec_ref_bits(name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(call_bits);
                    dec_ref_bits(call_bits);
                    if exception_pending() {
                        return MoltObject::none().bits();
                    }
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        if i < 0 {
                            raise!("ValueError", "__len__() should return >= 0");
                        }
                        return MoltObject::from_int(i).bits();
                    }
                    if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                        let big = bigint_ref(big_ptr);
                        if big.is_negative() {
                            raise!("ValueError", "__len__() should return >= 0");
                        }
                        let Some(len) = big.to_usize() else {
                            raise!(
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer"
                            );
                        };
                        if len > i64::MAX as usize {
                            raise!(
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer"
                            );
                        }
                        return MoltObject::from_int(len as i64).bits();
                    }
                    let res_type = class_name_for_error(type_of_bits(res_bits));
                    let msg = format!("'{}' object cannot be interpreted as an integer", res_type);
                    raise!("TypeError", &msg);
                }
            }
        }
    }
    let type_name = class_name_for_error(type_of_bits(val));
    let msg = format!("object of type '{type_name}' has no len()");
    raise!("TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_id(val: u64) -> u64 {
    MoltObject::from_int(val as i64).bits()
}

#[no_mangle]
pub extern "C" fn molt_ord(val: u64) -> u64 {
    let obj = obj_from_bits(val);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                let Ok(s) = std::str::from_utf8(bytes) else {
                    return MoltObject::none().bits();
                };
                let char_count = s.chars().count();
                if char_count != 1 {
                    let msg = format!(
                        "ord() expected a character, but string of length {char_count} found"
                    );
                    raise!("TypeError", &msg);
                }
                let ch = s.chars().next().unwrap_or('\0');
                return MoltObject::from_int(ch as i64).bits();
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                if len != 1 {
                    let msg =
                        format!("ord() expected a character, but string of length {len} found");
                    raise!("TypeError", &msg);
                }
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return MoltObject::from_int(bytes[0] as i64).bits();
            }
        }
    }
    let type_name = class_name_for_error(type_of_bits(val));
    let msg = format!("ord() expected string of length 1, but {type_name} found");
    raise!("TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_chr(val: u64) -> u64 {
    let type_name = class_name_for_error(type_of_bits(val));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let Some(value) = index_bigint_from_obj(val, &msg) else {
        return MoltObject::none().bits();
    };
    if value.is_negative() || value > BigInt::from(0x10FFFF) {
        raise!("ValueError", "chr() arg not in range(0x110000)");
    }
    let Some(code) = value.to_u32() else {
        raise!("ValueError", "chr() arg not in range(0x110000)");
    };
    let Some(ch) = std::char::from_u32(code) else {
        raise!("ValueError", "chr() arg not in range(0x110000)");
    };
    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    let out = alloc_string(s.as_bytes());
    if out.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out).bits()
}

#[no_mangle]
pub extern "C" fn molt_missing() -> u64 {
    let bits = missing_bits();
    inc_ref_bits(bits);
    bits
}

#[no_mangle]
pub extern "C" fn molt_not_implemented() -> u64 {
    not_implemented_bits()
}

#[no_mangle]
pub extern "C" fn molt_getrecursionlimit() -> u64 {
    MoltObject::from_int(recursion_limit_get() as i64).bits()
}

#[no_mangle]
pub extern "C" fn molt_setrecursionlimit(limit_bits: u64) -> u64 {
    let obj = obj_from_bits(limit_bits);
    let limit = if let Some(value) = to_i64(obj) {
        if value < 1 {
            raise!(
                "ValueError",
                "recursion limit must be greater or equal than 1"
            );
        }
        value as usize
    } else if let Some(big_ptr) = bigint_ptr_from_bits(limit_bits) {
        let big = unsafe { bigint_ref(big_ptr) };
        if big.is_negative() {
            raise!(
                "ValueError",
                "recursion limit must be greater or equal than 1"
            );
        }
        let Some(value) = big.to_usize() else {
            raise!(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer"
            );
        };
        value
    } else {
        let type_name = class_name_for_error(type_of_bits(limit_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        raise!("TypeError", &msg);
    };
    let depth = RECURSION_DEPTH.with(|depth| depth.get());
    if limit <= depth {
        let msg = format!(
            "cannot set the recursion limit to {limit} at the recursion depth {depth}: the limit is too low"
        );
        raise!("RecursionError", &msg);
    }
    recursion_limit_set(limit);
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_recursion_guard_enter() -> i64 {
    if recursion_guard_enter() {
        1
    } else {
        raise_exception::<i64>("RecursionError", "maximum recursion depth exceeded")
    }
}

#[no_mangle]
pub extern "C" fn molt_recursion_guard_exit() {
    recursion_guard_exit();
}

#[no_mangle]
pub extern "C" fn molt_repr_builtin(val_bits: u64) -> u64 {
    molt_repr_from_obj(val_bits)
}

#[no_mangle]
pub extern "C" fn molt_callable_builtin(val_bits: u64) -> u64 {
    molt_is_callable(val_bits)
}

#[no_mangle]
pub extern "C" fn molt_round_builtin(val_bits: u64, ndigits_bits: u64) -> u64 {
    let missing = missing_bits();
    let has_ndigits = ndigits_bits != missing;
    let has_ndigits_bits = MoltObject::from_bool(has_ndigits).bits();
    let ndigits = if has_ndigits {
        ndigits_bits
    } else {
        MoltObject::none().bits()
    };
    molt_round(val_bits, ndigits, has_ndigits_bits)
}

#[no_mangle]
pub extern "C" fn molt_enumerate_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    let missing = missing_bits();
    let has_start = start_bits != missing;
    let start = if has_start {
        start_bits
    } else {
        MoltObject::from_int(0).bits()
    };
    let has_start_bits = MoltObject::from_bool(has_start).bits();
    molt_enumerate(iter_bits, start, has_start_bits)
}

#[no_mangle]
pub extern "C" fn molt_next_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    let missing = missing_bits();
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        raise!("TypeError", "object is not an iterator");
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            raise!("TypeError", "object is not an iterator");
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            raise!("TypeError", "object is not an iterator");
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        if is_truthy(obj_from_bits(done_bits)) {
            if default_bits != missing {
                inc_ref_bits(default_bits);
                return default_bits;
            }
            if obj_from_bits(val_bits).is_none() {
                raise!("StopIteration", "");
            }
            let msg_bits = molt_str_from_obj(val_bits);
            let msg = string_obj_to_owned(obj_from_bits(msg_bits)).unwrap_or_default();
            dec_ref_bits(msg_bits);
            raise!("StopIteration", &msg);
        }
        inc_ref_bits(val_bits);
        val_bits
    }
}

#[no_mangle]
pub extern "C" fn molt_any_builtin(iter_bits: u64) -> u64 {
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            raise!("TypeError", "object is not an iterator");
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                raise!("TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                return MoltObject::from_bool(false).bits();
            }
            if is_truthy(obj_from_bits(val_bits)) {
                return MoltObject::from_bool(true).bits();
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_all_builtin(iter_bits: u64) -> u64 {
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            raise!("TypeError", "object is not an iterator");
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                raise!("TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                return MoltObject::from_bool(true).bits();
            }
            if !is_truthy(obj_from_bits(val_bits)) {
                return MoltObject::from_bool(false).bits();
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_abs_builtin(val_bits: u64) -> u64 {
    let obj = obj_from_bits(val_bits);
    if let Some(i) = to_i64(obj) {
        return int_bits_from_i128((i as i128).abs());
    }
    if let Some(big) = to_bigint(obj) {
        let abs_val = big.abs();
        if let Some(i) = bigint_to_inline(&abs_val) {
            return MoltObject::from_int(i).bits();
        }
        return bigint_bits(abs_val);
    }
    if let Some(f) = to_f64(obj) {
        return MoltObject::from_float(f.abs()).bits();
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        if let Some(name_bits) = attr_name_bits_from_bytes(b"__abs__") {
            unsafe {
                let call_bits = attr_lookup_ptr(ptr, name_bits);
                dec_ref_bits(name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(call_bits);
                    dec_ref_bits(call_bits);
                    return res_bits;
                }
            }
        }
    }
    let type_name = class_name_for_error(type_of_bits(val_bits));
    let msg = format!("bad operand type for abs(): '{type_name}'");
    raise!("TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_divmod_builtin(a_bits: u64, b_bits: u64) -> u64 {
    let lhs = obj_from_bits(a_bits);
    let rhs = obj_from_bits(b_bits);
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        if ri == 0 {
            raise!("ZeroDivisionError", "integer division or modulo by zero");
        }
        let li128 = li as i128;
        let ri128 = ri as i128;
        let mut rem = li128 % ri128;
        if rem != 0 && (rem > 0) != (ri128 > 0) {
            rem += ri128;
        }
        let quot = (li128 - rem) / ri128;
        let q_bits = int_bits_from_i128(quot);
        let r_bits = int_bits_from_i128(rem);
        let tuple_ptr = alloc_tuple(&[q_bits, r_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(tuple_ptr).bits();
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        if r_big.is_zero() {
            raise!("ZeroDivisionError", "division by zero");
        }
        let quot = l_big.div_floor(&r_big);
        let rem = l_big.mod_floor(&r_big);
        let q_bits = if let Some(i) = bigint_to_inline(&quot) {
            MoltObject::from_int(i).bits()
        } else {
            bigint_bits(quot)
        };
        let r_bits = if let Some(i) = bigint_to_inline(&rem) {
            MoltObject::from_int(i).bits()
        } else {
            bigint_bits(rem)
        };
        let tuple_ptr = alloc_tuple(&[q_bits, r_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(tuple_ptr).bits();
    }
    if let Some((lf, rf)) = float_pair_from_obj(lhs, rhs) {
        if rf == 0.0 {
            raise!("ZeroDivisionError", "float divmod()");
        }
        let quot = (lf / rf).floor();
        let mut rem = lf % rf;
        if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
            rem += rf;
        }
        let q_bits = MoltObject::from_float(quot).bits();
        let r_bits = MoltObject::from_float(rem).bits();
        let tuple_ptr = alloc_tuple(&[q_bits, r_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(tuple_ptr).bits();
    }
    let left = class_name_for_error(type_of_bits(a_bits));
    let right = class_name_for_error(type_of_bits(b_bits));
    let msg = format!("unsupported operand type(s) for divmod(): '{left}' and '{right}'");
    raise!("TypeError", &msg);
}

#[inline]
fn minmax_compare(best_key_bits: u64, cand_key_bits: u64) -> CompareOutcome {
    compare_objects(obj_from_bits(cand_key_bits), obj_from_bits(best_key_bits))
}

fn molt_minmax_builtin(
    args_bits: u64,
    key_bits: u64,
    default_bits: u64,
    want_max: bool,
    name: &str,
) -> u64 {
    let missing = missing_bits();
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        let msg = format!("{name} expected at least 1 argument, got 0");
        raise!("TypeError", &msg);
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{name} expected at least 1 argument, got 0");
            raise!("TypeError", &msg);
        }
        let args = seq_vec_ref(args_ptr);
        if args.is_empty() {
            let msg = format!("{name} expected at least 1 argument, got 0");
            raise!("TypeError", &msg);
        }
        let has_default = default_bits != missing;
        if args.len() > 1 && has_default {
            let msg =
                format!("Cannot specify a default for {name}() with multiple positional arguments");
            raise!("TypeError", &msg);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let mut best_bits;
        let mut best_key_bits: u64;
        if args.len() == 1 {
            let iter_bits = molt_iter(args[0]);
            if obj_from_bits(iter_bits).is_none() {
                raise!("TypeError", "object is not iterable");
            }
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                raise!("TypeError", "object is not an iterator");
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                raise!("TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                if has_default {
                    inc_ref_bits(default_bits);
                    return default_bits;
                }
                let msg = format!("{name}() iterable argument is empty");
                raise!("ValueError", &msg);
            }
            best_bits = val_bits;
            if use_key {
                best_key_bits = call_callable1(key_bits, best_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
            } else {
                best_key_bits = best_bits;
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    raise!("TypeError", "object is not an iterator");
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    raise!("TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    raise!("TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    if use_key {
                        dec_ref_bits(best_key_bits);
                    }
                    inc_ref_bits(best_bits);
                    return best_bits;
                }
                let cand_key_bits = if use_key {
                    let res_bits = call_callable1(key_bits, val_bits);
                    if exception_pending() {
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                let replace = match minmax_compare(best_key_bits, cand_key_bits) {
                    CompareOutcome::Ordered(ordering) => {
                        if want_max {
                            ordering == Ordering::Greater
                        } else {
                            ordering == Ordering::Less
                        }
                    }
                    CompareOutcome::Unordered => false,
                    CompareOutcome::NotComparable => {
                        if use_key {
                            dec_ref_bits(best_key_bits);
                            dec_ref_bits(cand_key_bits);
                        }
                        return compare_type_error(
                            obj_from_bits(cand_key_bits),
                            obj_from_bits(best_key_bits),
                            if want_max { ">" } else { "<" },
                        );
                    }
                    CompareOutcome::Error => {
                        if use_key {
                            dec_ref_bits(best_key_bits);
                            dec_ref_bits(cand_key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if replace {
                    if use_key {
                        dec_ref_bits(best_key_bits);
                    }
                    best_bits = val_bits;
                    best_key_bits = cand_key_bits;
                } else if use_key {
                    dec_ref_bits(cand_key_bits);
                }
            }
        }
        best_bits = args[0];
        if use_key {
            best_key_bits = call_callable1(key_bits, best_bits);
            if exception_pending() {
                return MoltObject::none().bits();
            }
        } else {
            best_key_bits = best_bits;
        }
        for &val_bits in args.iter().skip(1) {
            let cand_key_bits = if use_key {
                let res_bits = call_callable1(key_bits, val_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                res_bits
            } else {
                val_bits
            };
            let replace = match minmax_compare(best_key_bits, cand_key_bits) {
                CompareOutcome::Ordered(ordering) => {
                    if want_max {
                        ordering == Ordering::Greater
                    } else {
                        ordering == Ordering::Less
                    }
                }
                CompareOutcome::Unordered => false,
                CompareOutcome::NotComparable => {
                    if use_key {
                        dec_ref_bits(best_key_bits);
                        dec_ref_bits(cand_key_bits);
                    }
                    return compare_type_error(
                        obj_from_bits(cand_key_bits),
                        obj_from_bits(best_key_bits),
                        if want_max { ">" } else { "<" },
                    );
                }
                CompareOutcome::Error => {
                    if use_key {
                        dec_ref_bits(best_key_bits);
                        dec_ref_bits(cand_key_bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if replace {
                if use_key {
                    dec_ref_bits(best_key_bits);
                }
                best_bits = val_bits;
                best_key_bits = cand_key_bits;
            } else if use_key {
                dec_ref_bits(cand_key_bits);
            }
        }
        if use_key {
            dec_ref_bits(best_key_bits);
        }
        inc_ref_bits(best_bits);
        best_bits
    }
}

#[no_mangle]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    molt_minmax_builtin(args_bits, key_bits, default_bits, false, "min")
}

#[no_mangle]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    molt_minmax_builtin(args_bits, key_bits, default_bits, true, "max")
}

struct SortItem {
    key_bits: u64,
    value_bits: u64,
}

enum SortError {
    NotComparable(u64, u64),
    Exception,
}

#[no_mangle]
pub extern "C" fn molt_sorted_builtin(iter_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    let use_key = !obj_from_bits(key_bits).is_none();
    let reverse = is_truthy(obj_from_bits(reverse_bits));
    let mut items: Vec<SortItem> = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            if use_key {
                for item in items.drain(..) {
                    dec_ref_bits(item.key_bits);
                }
            }
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(item.key_bits);
                    }
                }
                raise!("TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(item.key_bits);
                    }
                }
                raise!("TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let key_val_bits = if use_key {
                let res_bits = call_callable1(key_bits, val_bits);
                if exception_pending() {
                    for item in items.drain(..) {
                        dec_ref_bits(item.key_bits);
                    }
                    return MoltObject::none().bits();
                }
                res_bits
            } else {
                val_bits
            };
            items.push(SortItem {
                key_bits: key_val_bits,
                value_bits: val_bits,
            });
        }
    }
    let mut error: Option<SortError> = None;
    items.sort_by(|left, right| {
        if error.is_some() {
            return Ordering::Equal;
        }
        let outcome = compare_objects(obj_from_bits(left.key_bits), obj_from_bits(right.key_bits));
        match outcome {
            CompareOutcome::Ordered(ordering) => {
                if reverse {
                    ordering.reverse()
                } else {
                    ordering
                }
            }
            CompareOutcome::Unordered => Ordering::Equal,
            CompareOutcome::NotComparable => {
                error = Some(SortError::NotComparable(left.key_bits, right.key_bits));
                Ordering::Equal
            }
            CompareOutcome::Error => {
                error = Some(SortError::Exception);
                Ordering::Equal
            }
        }
    });
    if let Some(error) = error {
        if use_key {
            for item in items.drain(..) {
                dec_ref_bits(item.key_bits);
            }
        }
        match error {
            SortError::NotComparable(left_bits, right_bits) => {
                let msg = format!(
                    "'<' not supported between instances of '{}' and '{}'",
                    type_name(obj_from_bits(left_bits)),
                    type_name(obj_from_bits(right_bits)),
                );
                raise!("TypeError", &msg);
            }
            SortError::Exception => {
                return MoltObject::none().bits();
            }
        }
    }
    let mut out: Vec<u64> = Vec::with_capacity(items.len());
    for item in items.iter() {
        out.push(item.value_bits);
    }
    if use_key {
        for item in items.drain(..) {
            dec_ref_bits(item.key_bits);
        }
    }
    let list_ptr = alloc_list(&out);
    if list_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(list_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_sum_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    let start_obj = obj_from_bits(start_bits);
    if let Some(ptr) = start_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                raise!(
                    "TypeError",
                    "sum() can't sum strings [use ''.join(seq) instead]"
                );
            }
            if type_id == TYPE_ID_BYTES {
                raise!(
                    "TypeError",
                    "sum() can't sum bytes [use b''.join(seq) instead]"
                );
            }
            if type_id == TYPE_ID_BYTEARRAY {
                raise!(
                    "TypeError",
                    "sum() can't sum bytearray [use b''.join(seq) instead]"
                );
            }
        }
    }
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    let mut total_bits = start_bits;
    let mut total_owned = false;
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            raise!("TypeError", "object is not an iterator");
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                raise!("TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                if !total_owned {
                    inc_ref_bits(total_bits);
                }
                return total_bits;
            }
            let next_bits = molt_add(total_bits, val_bits);
            if obj_from_bits(next_bits).is_none() {
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                return binary_type_error(obj_from_bits(total_bits), obj_from_bits(val_bits), "+");
            }
            total_bits = next_bits;
            total_owned = true;
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    let missing = missing_bits();
    if default_bits == missing {
        return molt_get_attr_name(obj_bits, name_bits);
    }
    molt_get_attr_name_default(obj_bits, name_bits, default_bits)
}

#[no_mangle]
pub extern "C" fn molt_anext_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    let missing = missing_bits();
    if default_bits == missing {
        return molt_anext(iter_bits);
    }
    let obj_bits = molt_alloc(3 * std::mem::size_of::<u64>() as u64);
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let header = header_from_obj_ptr(obj_ptr);
        (*header).poll_fn = molt_anext_default_poll as usize as u64;
        (*header).state = 0;
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr = iter_bits;
        inc_ref_bits(iter_bits);
        *payload_ptr.add(1) = default_bits;
        inc_ref_bits(default_bits);
        *payload_ptr.add(2) = MoltObject::none().bits();
    }
    obj_bits
}

#[no_mangle]
pub extern "C" fn molt_print_builtin(args_bits: u64) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        raise!("TypeError", "print expects a tuple");
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            raise!("TypeError", "print expects a tuple");
        }
        let elems = seq_vec_ref(args_ptr);
        if elems.is_empty() {
            molt_print_newline();
            return MoltObject::none().bits();
        }
        if elems.len() == 1 {
            molt_print_obj(elems[0]);
            return MoltObject::none().bits();
        }
        let mut parts = Vec::with_capacity(elems.len());
        for &val_bits in elems {
            let str_bits = molt_str_from_obj(val_bits);
            parts.push(str_bits);
        }
        let tuple_ptr = alloc_tuple(&parts);
        if tuple_ptr.is_null() {
            for part in parts {
                dec_ref_bits(part);
            }
            return MoltObject::none().bits();
        }
        let sep_ptr = alloc_string(b" ");
        if sep_ptr.is_null() {
            for part in parts {
                dec_ref_bits(part);
            }
            dec_ref_bits(MoltObject::from_ptr(tuple_ptr).bits());
            return MoltObject::none().bits();
        }
        let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let joined_bits = molt_string_join(sep_bits, tuple_bits);
        molt_print_obj(joined_bits);
        for part in parts {
            dec_ref_bits(part);
        }
        dec_ref_bits(sep_bits);
        dec_ref_bits(tuple_bits);
        dec_ref_bits(joined_bits);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_super_builtin(type_bits: u64, obj_bits: u64) -> u64 {
    molt_super_new(type_bits, obj_bits)
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
                        memoryview_format_bits(ptr),
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
                    memoryview_format_bits(ptr),
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
            if let Some(cache) = utf8_count_cache_lookup(hay_ptr as usize, needle_bytes) {
                return MoltObject::from_int(cache.count).bits();
            }
            profile_hit(&STRING_COUNT_CACHE_MISS);
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            utf8_count_cache_store(hay_ptr as usize, hay_bytes, needle_bytes, count, Vec::new());
            return MoltObject::from_int(count).bits();
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_string_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
    let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
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
            let total_chars = utf8_codepoint_count_cached(hay_bytes, Some(hay_ptr as usize));
            let mut start = if has_start {
                index_i64_from_obj(start_bits, "count() start must be int")
            } else {
                0
            };
            let mut end = if has_end {
                index_i64_from_obj(end_bits, "count() end must be int")
            } else {
                total_chars
            };
            if start < 0 {
                start += total_chars;
            }
            if end < 0 {
                end += total_chars;
            }
            if start < 0 {
                start = 0;
            }
            if end < 0 {
                end = 0;
            }
            if start > total_chars {
                start = total_chars;
            }
            if end > total_chars {
                end = total_chars;
            }
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if needle_bytes.is_empty() {
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let start_byte =
                utf8_char_to_byte_index_cached(hay_bytes, start, Some(hay_ptr as usize));
            let end_byte = utf8_char_to_byte_index_cached(hay_bytes, end, Some(hay_ptr as usize))
                .min(hay_bytes.len());
            if let Some(cache) = utf8_count_cache_lookup(hay_ptr as usize, needle_bytes) {
                let cache = utf8_count_cache_upgrade_prefix(hay_ptr as usize, &cache, hay_bytes);
                let count = utf8_count_cache_count_slice(&cache, hay_bytes, start_byte, end_byte);
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start_byte..end_byte];
            let count = bytes_count_impl(slice, needle_bytes);
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
        struct StringPart {
            bits: u64,
            data: *const u8,
            len: usize,
        }
        let mut parts = Vec::with_capacity(elems.len());
        let mut all_same = true;
        let mut first_bits = 0u64;
        let mut first_data = std::ptr::null();
        let mut first_len = 0usize;
        for (idx, &elem_bits) in elems.iter().enumerate() {
            let elem_obj = obj_from_bits(elem_bits);
            let elem_ptr = match elem_obj.as_ptr() {
                Some(ptr) => ptr,
                None => raise!("TypeError", "join expects a list or tuple of str"),
            };
            if object_type_id(elem_ptr) != TYPE_ID_STRING {
                raise!("TypeError", "join expects a list or tuple of str");
            }
            let len = string_len(elem_ptr);
            total_len += len;
            let data = string_bytes(elem_ptr);
            if idx == 0 {
                first_bits = elem_bits;
                first_data = data;
                first_len = len;
            } else if elem_bits != first_bits {
                all_same = false;
            }
            parts.push(StringPart {
                bits: elem_bits,
                data,
                len,
            });
        }
        if !parts.is_empty() {
            let sep_total = sep_bytes
                .len()
                .saturating_mul(parts.len().saturating_sub(1));
            total_len = total_len.saturating_add(sep_total);
        }
        if parts.len() == 1 {
            inc_ref_bits(parts[0].bits);
            return parts[0].bits;
        }
        let out_ptr = alloc_bytes_like_with_len(total_len, TYPE_ID_STRING);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut cursor = out_ptr.add(std::mem::size_of::<usize>());
        if all_same && parts.len() > 1 {
            let sep_len = sep_bytes.len();
            let elem_len = first_len;
            if elem_len > 0 {
                std::ptr::copy_nonoverlapping(first_data, cursor, elem_len);
                cursor = cursor.add(elem_len);
            }
            let pattern_len = sep_len.saturating_add(elem_len);
            let total_pattern_bytes = pattern_len.saturating_mul(parts.len() - 1);
            if total_pattern_bytes > 0 {
                if sep_len > 0 {
                    std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_len);
                }
                if elem_len > 0 {
                    std::ptr::copy_nonoverlapping(first_data, cursor.add(sep_len), elem_len);
                }
                let pattern_start = cursor;
                let mut filled = pattern_len;
                while filled < total_pattern_bytes {
                    let copy_len = (total_pattern_bytes - filled).min(filled);
                    std::ptr::copy_nonoverlapping(
                        pattern_start,
                        pattern_start.add(filled),
                        copy_len,
                    );
                    filled += copy_len;
                }
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        for (idx, part) in parts.iter().enumerate() {
            if idx > 0 {
                std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_bytes.len());
                cursor = cursor.add(sep_bytes.len());
            }
            std::ptr::copy_nonoverlapping(part.data, cursor, part.len);
            cursor = cursor.add(part.len);
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
    let mut offsets = Vec::new();
    let mut prefix = Vec::new();
    let mut total = 0i64;
    let mut idx = 0usize;
    offsets.push(0);
    prefix.push(0);
    while idx < bytes.len() {
        let mut end = (idx + UTF8_CACHE_BLOCK).min(bytes.len());
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end += 1;
        }
        total += count_utf8_bytes(&bytes[idx..end]);
        offsets.push(end);
        prefix.push(total);
        idx = end;
    }
    Utf8IndexCache { offsets, prefix }
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
    utf8_count_cache_remove(key);
    utf8_count_cache_tls_remove(key);
}

fn utf8_count_cache_shard(key: usize) -> usize {
    let mut x = key as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as usize) & (UTF8_COUNT_CACHE_SHARDS - 1)
}

fn utf8_count_cache_remove(key: usize) {
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = UTF8_COUNT_CACHE.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.remove(key);
        }
    }
}

fn utf8_count_cache_lookup(key: usize, needle: &[u8]) -> Option<Arc<Utf8CountCache>> {
    if let Some(cache) = UTF8_COUNT_TLS.with(|cell| {
        cell.borrow().as_ref().and_then(|entry| {
            if entry.key == key && entry.cache.needle == needle {
                Some(entry.cache.clone())
            } else {
                None
            }
        })
    }) {
        profile_hit(&STRING_COUNT_CACHE_HIT);
        return Some(cache);
    }
    let shard = utf8_count_cache_shard(key);
    let store = UTF8_COUNT_CACHE.get(shard)?.lock().ok()?;
    let cache = store.get(key)?;
    if cache.needle == needle {
        profile_hit(&STRING_COUNT_CACHE_HIT);
        return Some(cache);
    }
    None
}

fn build_utf8_count_prefix(hay_bytes: &[u8], needle: &[u8]) -> Vec<i64> {
    if hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN || needle.is_empty() {
        return Vec::new();
    }
    let blocks = hay_bytes.len().div_ceil(UTF8_CACHE_BLOCK);
    let mut prefix = vec![0i64; blocks + 1];
    let mut count = 0i64;
    let mut idx = 1usize;
    let mut next_boundary = UTF8_CACHE_BLOCK.min(hay_bytes.len());
    let finder = memmem::Finder::new(needle);
    for pos in finder.find_iter(hay_bytes) {
        while pos >= next_boundary && idx < prefix.len() {
            prefix[idx] = count;
            idx += 1;
            next_boundary = (next_boundary + UTF8_CACHE_BLOCK).min(hay_bytes.len());
        }
        count += 1;
    }
    while idx < prefix.len() {
        prefix[idx] = count;
        idx += 1;
    }
    prefix
}

fn utf8_count_cache_store(
    key: usize,
    hay_bytes: &[u8],
    needle: &[u8],
    count: i64,
    prefix: Vec<i64>,
) {
    let cache = Arc::new(Utf8CountCache {
        needle: needle.to_vec(),
        count,
        prefix,
        hay_len: hay_bytes.len(),
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = UTF8_COUNT_CACHE.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.insert(key, cache.clone());
        }
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry { key, cache });
    });
}

fn utf8_count_cache_upgrade_prefix(
    key: usize,
    cache: &Arc<Utf8CountCache>,
    hay_bytes: &[u8],
) -> Arc<Utf8CountCache> {
    if !cache.prefix.is_empty()
        || cache.hay_len != hay_bytes.len()
        || hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN
        || cache.needle.is_empty()
    {
        return cache.clone();
    }
    let prefix = build_utf8_count_prefix(hay_bytes, &cache.needle);
    if prefix.is_empty() {
        return cache.clone();
    }
    let upgraded = Arc::new(Utf8CountCache {
        needle: cache.needle.clone(),
        count: cache.count,
        prefix,
        hay_len: cache.hay_len,
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = UTF8_COUNT_CACHE.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.insert(key, upgraded.clone());
        }
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry {
            key,
            cache: upgraded.clone(),
        });
    });
    upgraded
}

fn utf8_count_cache_tls_remove(key: usize) {
    UTF8_COUNT_TLS.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.as_ref().is_some_and(|entry| entry.key == key) {
            *guard = None;
        }
    });
}

fn count_matches_range(
    hay_bytes: &[u8],
    needle: &[u8],
    window_start: usize,
    window_end: usize,
    start_min: usize,
    start_max: usize,
) -> i64 {
    if window_end <= window_start || start_min > start_max {
        return 0;
    }
    let finder = memmem::Finder::new(needle);
    let mut count = 0i64;
    for pos in finder.find_iter(&hay_bytes[window_start..window_end]) {
        let abs = window_start + pos;
        if abs < start_min {
            continue;
        }
        if abs > start_max {
            break;
        }
        count += 1;
    }
    count
}

fn utf8_count_cache_count_slice(
    cache: &Utf8CountCache,
    hay_bytes: &[u8],
    start: usize,
    end: usize,
) -> i64 {
    let needle = &cache.needle;
    let needle_len = needle.len();
    if needle_len == 0 || end <= start {
        return 0;
    }
    if end - start < needle_len {
        return 0;
    }
    if cache.prefix.is_empty() || cache.hay_len != hay_bytes.len() {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let end_limit = end - needle_len;
    let block = UTF8_CACHE_BLOCK;
    let start_block = start / block;
    let end_block = end_limit / block;
    if start_block == end_block {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let mut total = 0i64;
    let block_end = ((start_block + 1) * block).min(hay_bytes.len());
    let left_scan_end = (block_end + needle_len - 1).min(end);
    let left_max = (block_end.saturating_sub(1)).min(end_limit);
    total += count_matches_range(hay_bytes, needle, start, left_scan_end, start, left_max);
    if end_block > start_block + 1 {
        total += cache.prefix[end_block] - cache.prefix[start_block + 1];
    }
    let right_block_start = (end_block * block).min(hay_bytes.len());
    if right_block_start <= end_limit {
        total += count_matches_range(
            hay_bytes,
            needle,
            right_block_start,
            end,
            right_block_start,
            end_limit,
        );
    }
    total
}

fn utf8_count_prefix_cached(bytes: &[u8], cache: &Utf8IndexCache, prefix_len: usize) -> i64 {
    let prefix_len = prefix_len.min(bytes.len());
    let block_idx = match cache.offsets.binary_search(&prefix_len) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let mut total = *cache.prefix.get(block_idx).unwrap_or(&0);
    let start = *cache.offsets.get(block_idx).unwrap_or(&0);
    if start < prefix_len {
        total += count_utf8_bytes(&bytes[start..prefix_len]);
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

fn utf8_char_width(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn utf8_char_to_byte_index_scan(bytes: &[u8], target: usize) -> usize {
    let mut idx = 0usize;
    let mut count = 0usize;
    while idx < bytes.len() && count < target {
        let width = utf8_char_width(bytes[idx]);
        idx = idx.saturating_add(width);
        count = count.saturating_add(1);
    }
    idx.min(bytes.len())
}

fn utf8_char_to_byte_index_cached(bytes: &[u8], char_idx: i64, cache_key: Option<usize>) -> usize {
    if char_idx <= 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return (char_idx as usize).min(bytes.len());
    }
    let total = utf8_codepoint_count_cached(bytes, cache_key);
    if char_idx >= total {
        return bytes.len();
    }
    let target = char_idx as usize;
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(key, bytes) {
            let mut lo = 0usize;
            let mut hi = cache.prefix.len().saturating_sub(1);
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if (cache.prefix.get(mid).copied().unwrap_or(0) as usize) <= target {
                    lo = mid;
                } else {
                    hi = mid.saturating_sub(1);
                }
            }
            let mut count = cache.prefix.get(lo).copied().unwrap_or(0) as usize;
            let mut idx = cache.offsets.get(lo).copied().unwrap_or(0);
            while idx < bytes.len() && count < target {
                let width = utf8_char_width(bytes[idx]);
                idx = idx.saturating_add(width);
                count = count.saturating_add(1);
            }
            return idx.min(bytes.len());
        }
    }
    utf8_char_to_byte_index_scan(bytes, target)
}

fn utf8_count_prefix_blocked(bytes: &[u8], prefix_len: usize) -> i64 {
    const BLOCK: usize = 4096;
    let mut total = 0i64;
    let mut idx = 0usize;
    while idx + BLOCK <= prefix_len {
        total += count_utf8_bytes(&bytes[idx..idx + BLOCK]);
        idx += BLOCK;
    }
    if idx < prefix_len {
        total += count_utf8_bytes(&bytes[idx..prefix_len]);
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

#[cfg(not(target_arch = "wasm32"))]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    simdutf::count_utf8(bytes) as i64
}

#[cfg(target_arch = "wasm32")]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    let mut count = 0i64;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        let width = if b < 0x80 {
            1
        } else if b < 0xE0 {
            2
        } else if b < 0xF0 {
            3
        } else {
            4
        };
        idx = idx.saturating_add(width);
        count += 1;
    }
    count
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
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

#[cfg(all(target_arch = "wasm32", not(target_os = "unknown")))]
fn memchr_simd128(_needle: u8, _hay: &[u8]) -> (bool, Option<usize>) {
    (false, None)
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
    if needle == replacement {
        return Some(hay.to_vec());
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
    if needle.len() == replacement.len() {
        let mut out = hay.to_vec();
        if needle.len() == 2 {
            let n0 = needle[0];
            let n1 = needle[1];
            let r0 = replacement[0];
            let r1 = replacement[1];
            let mut i = 0usize;
            while i + 1 < hay.len() {
                if hay[i] == n0 && hay[i + 1] == n1 {
                    out[i] = r0;
                    out[i + 1] = r1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            return Some(out);
        }
        let finder = memmem::Finder::new(needle);
        for idx in finder.find_iter(hay) {
            out[idx..idx + needle.len()].copy_from_slice(replacement);
        }
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

const SPLIT_CACHE_MAX_ENTRIES: usize = 8;
const SPLIT_CACHE_MAX_LEN: usize = 32;

struct SplitTokenCacheEntry {
    bits: u64,
    len: usize,
}

fn split_cache_lookup(cache: &[SplitTokenCacheEntry], part: &[u8]) -> Option<u64> {
    if part.len() > SPLIT_CACHE_MAX_LEN {
        return None;
    }
    for entry in cache {
        if entry.len != part.len() {
            continue;
        }
        let obj = obj_from_bits(entry.bits);
        let Some(ptr) = obj.as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                continue;
            }
            let bytes = std::slice::from_raw_parts(string_bytes(ptr), entry.len);
            if bytes == part {
                return Some(entry.bits);
            }
        }
    }
    None
}

fn split_cache_store(cache: &mut Vec<SplitTokenCacheEntry>, bits: u64, len: usize) {
    if len > SPLIT_CACHE_MAX_LEN || cache.len() >= SPLIT_CACHE_MAX_ENTRIES {
        return;
    }
    cache.push(SplitTokenCacheEntry { bits, len });
}

fn split_string_push_part(
    cache: &mut Vec<SplitTokenCacheEntry>,
    list_ptr: *mut u8,
    list_bits: u64,
    part: &[u8],
) -> bool {
    if let Some(bits) = split_cache_lookup(cache, part) {
        inc_ref_bits(bits);
        unsafe {
            list_push_owned(list_ptr, bits);
        }
        return true;
    }
    let ptr = alloc_string(part);
    if ptr.is_null() {
        dec_ref_bits(list_bits);
        return false;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    unsafe {
        list_push_owned(list_ptr, bits);
    }
    split_cache_store(cache, bits, part.len());
    true
}

fn split_string_bytes_to_list(hay: &[u8], needle: &[u8]) -> Option<u64> {
    let mut cache = Vec::new();
    if needle.len() == 1 {
        let count = memchr::memchr_iter(needle[0], hay).count();
        let list_ptr = alloc_list_empty_with_capacity(count + 1);
        if list_ptr.is_null() {
            return None;
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let mut start = 0usize;
        for idx in memchr::memchr_iter(needle[0], hay) {
            let part = &hay[start..idx];
            if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
                return None;
            }
            start = idx + needle.len();
        }
        let part = &hay[start..];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        return Some(list_bits);
    }
    let mut indices = Vec::new();
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        indices.push(idx);
    }
    let list_ptr = alloc_list_empty_with_capacity(indices.len() + 1);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    for idx in indices {
        let part = &hay[start..idx];
        if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
            return None;
        }
        start = idx + needle.len();
    }
    let part = &hay[start..];
    if !split_string_push_part(&mut cache, list_ptr, list_bits, part) {
        return None;
    }
    Some(list_bits)
}

fn split_bytes_to_list<F>(hay: &[u8], needle: &[u8], mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    if needle.len() == 1 {
        let count = memchr::memchr_iter(needle[0], hay).count();
        let list_ptr = alloc_list_empty_with_capacity(count + 1);
        if list_ptr.is_null() {
            return None;
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let mut start = 0usize;
        for idx in memchr::memchr_iter(needle[0], hay) {
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
        return Some(list_bits);
    }
    let mut indices = Vec::new();
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        indices.push(idx);
    }
    let list_ptr = alloc_list_empty_with_capacity(indices.len() + 1);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start = 0usize;
    for idx in indices {
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

fn split_bytes_whitespace_to_list<F>(hay: &[u8], mut alloc: F) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let mut start: Option<usize> = None;
    for (idx, byte) in hay.iter().enumerate() {
        if byte.is_ascii_whitespace() {
            if let Some(s) = start {
                let part = &hay[s..idx];
                let ptr = alloc(part);
                if ptr.is_null() {
                    dec_ref_bits(list_bits);
                    return None;
                }
                unsafe {
                    list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
                }
                start = None;
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        let part = &hay[s..];
        let ptr = alloc(part);
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

fn split_string_whitespace_to_list(hay: &[u8]) -> Option<u64> {
    let Ok(hay_str) = std::str::from_utf8(hay) else {
        return None;
    };
    let list_ptr = alloc_list_empty_with_capacity(4);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    for part in hay_str.split_whitespace() {
        let ptr = alloc_string(part.as_bytes());
        if ptr.is_null() {
            dec_ref_bits(list_bits);
            return None;
        }
        unsafe {
            list_push_owned(list_ptr, MoltObject::from_ptr(ptr).bits());
        }
    }
    Some(list_bits)
}

#[no_mangle]
pub extern "C" fn molt_string_split(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let Some(hay_ptr) = hay.as_ptr() {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            if needle.is_none() {
                let list_bits = split_string_whitespace_to_list(hay_bytes);
                return list_bits.unwrap_or_else(|| MoltObject::none().bits());
            }
            let Some(needle_ptr) = needle.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            if needle_bytes.is_empty() {
                raise!("ValueError", "empty separator");
            }
            let list_bits = split_string_bytes_to_list(hay_bytes, needle_bytes);
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
pub extern "C" fn molt_string_lower(hay_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != TYPE_ID_STRING {
            return MoltObject::none().bits();
        }
        let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
        let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
            return MoltObject::none().bits();
        };
        let lowered = hay_str.to_lowercase();
        let ptr = alloc_string(lowered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_string_upper(hay_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != TYPE_ID_STRING {
            return MoltObject::none().bits();
        }
        let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
        let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
            return MoltObject::none().bits();
        };
        let uppered = hay_str.to_uppercase();
        let ptr = alloc_string(uppered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_string_capitalize(hay_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != TYPE_ID_STRING {
            return MoltObject::none().bits();
        }
        let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
        let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
            return MoltObject::none().bits();
        };
        let mut out = String::with_capacity(hay_str.len());
        let mut chars = hay_str.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
        let ptr = alloc_string(out.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_string_strip(hay_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != TYPE_ID_STRING {
            return MoltObject::none().bits();
        }
        let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
        let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
            return MoltObject::none().bits();
        };
        let trimmed = hay_str.trim();
        let ptr = alloc_string(trimmed.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_bytes_split(hay_bits: u64, needle_bits: u64) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let needle = obj_from_bits(needle_bits);
    if let Some(hay_ptr) = hay.as_ptr() {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if needle.is_none() {
                let list_bits = split_bytes_whitespace_to_list(hay_bytes, alloc_bytes);
                return list_bits.unwrap_or_else(|| MoltObject::none().bits());
            }
            let Some(needle_ptr) = needle.as_ptr() else {
                return MoltObject::none().bits();
            };
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
    if let Some(hay_ptr) = hay.as_ptr() {
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if needle.is_none() {
                let list_bits = split_bytes_whitespace_to_list(hay_bytes, alloc_bytearray);
                return list_bits.unwrap_or_else(|| MoltObject::none().bits());
            }
            let Some(needle_ptr) = needle.as_ptr() else {
                return MoltObject::none().bits();
            };
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
pub extern "C" fn molt_tuple_from_list(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE {
                inc_ref_bits(bits);
                return bits;
            }
            if type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                let out_ptr = alloc_tuple(elems);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[derive(Clone, Copy)]
enum BytesCtorKind {
    Bytes,
    Bytearray,
}

impl BytesCtorKind {
    fn name(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes",
            BytesCtorKind::Bytearray => "bytearray",
        }
    }

    fn ctor_label(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes()",
            BytesCtorKind::Bytearray => "bytearray()",
        }
    }

    fn range_error(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes must be in range(0, 256)",
            BytesCtorKind::Bytearray => "byte must be in range(0, 256)",
        }
    }

    fn non_iterable_message(self, type_name: &str) -> String {
        format!("cannot convert '{}' object to {}", type_name, self.name())
    }

    fn arg_type_message(self, arg: &str, type_name: &str) -> String {
        format!(
            "{} argument '{}' must be str, not {}",
            self.ctor_label(),
            arg,
            type_name
        )
    }
}

#[derive(Clone, Copy)]
enum EncodingKind {
    Utf8,
    Latin1,
    Ascii,
    Utf16,
    Utf16LE,
    Utf16BE,
    Utf32,
    Utf32LE,
    Utf32BE,
}

impl EncodingKind {
    fn name(self) -> &'static str {
        match self {
            EncodingKind::Utf8 => "utf-8",
            EncodingKind::Latin1 => "latin-1",
            EncodingKind::Ascii => "ascii",
            EncodingKind::Utf16 => "utf-16",
            EncodingKind::Utf16LE => "utf-16-le",
            EncodingKind::Utf16BE => "utf-16-be",
            EncodingKind::Utf32 => "utf-32",
            EncodingKind::Utf32LE => "utf-32-le",
            EncodingKind::Utf32BE => "utf-32-be",
        }
    }

    fn ordinal_limit(self) -> u32 {
        match self {
            EncodingKind::Ascii => 128,
            EncodingKind::Latin1 => 256,
            EncodingKind::Utf8
            | EncodingKind::Utf16
            | EncodingKind::Utf16LE
            | EncodingKind::Utf16BE
            | EncodingKind::Utf32
            | EncodingKind::Utf32LE
            | EncodingKind::Utf32BE => u32::MAX,
        }
    }
}

enum EncodeError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    InvalidChar {
        encoding: &'static str,
        ch: char,
        pos: usize,
        limit: u32,
    },
}

fn normalize_encoding(name: &str) -> Option<EncodingKind> {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Some(EncodingKind::Utf8),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => Some(EncodingKind::Latin1),
        "ascii" => Some(EncodingKind::Ascii),
        "utf-16" | "utf16" => Some(EncodingKind::Utf16),
        "utf-16le" | "utf-16-le" | "utf16le" => Some(EncodingKind::Utf16LE),
        "utf-16be" | "utf-16-be" | "utf16be" => Some(EncodingKind::Utf16BE),
        "utf-32" | "utf32" => Some(EncodingKind::Utf32),
        "utf-32le" | "utf-32-le" | "utf32le" => Some(EncodingKind::Utf32LE),
        "utf-32be" | "utf-32-be" | "utf32be" => Some(EncodingKind::Utf32BE),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

fn native_endian() -> Endian {
    if cfg!(target_endian = "big") {
        Endian::Big
    } else {
        Endian::Little
    }
}

fn push_u16(out: &mut Vec<u8>, val: u16, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

fn push_u32(out: &mut Vec<u8>, val: u32, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

fn encode_utf16(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(2) + if with_bom { 2 } else { 0 });
    if with_bom {
        push_u16(&mut out, 0xFEFF, endian);
    }
    for code in text.encode_utf16() {
        push_u16(&mut out, code, endian);
    }
    out
}

fn encode_utf32(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(4) + if with_bom { 4 } else { 0 });
    if with_bom {
        push_u32(&mut out, 0x0000_FEFF, endian);
    }
    for ch in text.chars() {
        push_u32(&mut out, ch as u32, endian);
    }
    out
}

fn unicode_escape(ch: char) -> String {
    let code = ch as u32;
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn encode_string_with_errors(
    text: &str,
    encoding: &str,
    errors: Option<&str>,
) -> Result<Vec<u8>, EncodeError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(EncodeError::UnknownEncoding(encoding.to_string()));
    };
    match kind {
        EncodingKind::Utf8 => Ok(text.as_bytes().to_vec()),
        EncodingKind::Utf16 => Ok(encode_utf16(text, native_endian(), true)),
        EncodingKind::Utf16LE => Ok(encode_utf16(text, Endian::Little, false)),
        EncodingKind::Utf16BE => Ok(encode_utf16(text, Endian::Big, false)),
        EncodingKind::Utf32 => Ok(encode_utf32(text, native_endian(), true)),
        EncodingKind::Utf32LE => Ok(encode_utf32(text, Endian::Little, false)),
        EncodingKind::Utf32BE => Ok(encode_utf32(text, Endian::Big, false)),
        EncodingKind::Latin1 | EncodingKind::Ascii => {
            let limit = kind.ordinal_limit();
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let code = ch as u32;
                if code < limit {
                    out.push(code as u8);
                    continue;
                }
                match errors.unwrap_or("strict") {
                    "ignore" => continue,
                    "replace" => out.push(b'?'),
                    "strict" => {
                        return Err(EncodeError::InvalidChar {
                            encoding: kind.name(),
                            ch,
                            pos: idx,
                            limit,
                        });
                    }
                    "surrogateescape" | "surrogatepass" => {
                        return Err(EncodeError::InvalidChar {
                            encoding: kind.name(),
                            ch,
                            pos: idx,
                            limit,
                        });
                    }
                    other => {
                        return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                    }
                }
            }
            Ok(out)
        }
    }
}

fn bytes_from_count(len: usize, type_id: u32) -> u64 {
    let ptr = alloc_bytes_like_with_len(len, type_id);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::write_bytes(data_ptr, 0, len);
    }
    MoltObject::from_ptr(ptr).bits()
}

fn bytes_item_to_u8(bits: u64, kind: BytesCtorKind) -> Option<u8> {
    let type_name = class_name_for_error(type_of_bits(bits));
    let msg = format!("'{}' object cannot be interpreted as an integer", type_name);
    let val = index_i64_from_obj(bits, &msg);
    if exception_pending() {
        return None;
    }
    if !(0..=255).contains(&val) {
        raise!("ValueError", kind.range_error());
    }
    Some(val as u8)
}

fn bytes_collect_from_iter(iter_bits: u64, kind: BytesCtorKind) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending() {
            return None;
        }
        let pair_ptr = obj_from_bits(pair_bits).as_ptr()?;
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return None;
            }
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let val_bits = elems[0];
            let byte = bytes_item_to_u8(val_bits, kind)?;
            out.push(byte);
        }
    }
    Some(out)
}

fn bytes_from_obj_impl(bits: u64, kind: BytesCtorKind) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = to_i64(obj) {
        if i < 0 {
            raise!("ValueError", "negative count");
        }
        let len = match usize::try_from(i) {
            Ok(len) => len,
            Err(_) => {
                raise!(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer"
                );
            }
        };
        let type_id = match kind {
            BytesCtorKind::Bytes => TYPE_ID_BYTES,
            BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
        };
        return bytes_from_count(len, type_id);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                raise!("TypeError", "string argument without an encoding");
            }
            if type_id == TYPE_ID_BYTES && matches!(kind, BytesCtorKind::Bytes) {
                inc_ref_bits(bits);
                return bits;
            }
            if let Some(slice) = bytes_like_slice(ptr) {
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(slice),
                    BytesCtorKind::Bytearray => alloc_bytearray(slice),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(out) = memoryview_collect_bytes(ptr) {
                    let out_ptr = match kind {
                        BytesCtorKind::Bytes => alloc_bytes(&out),
                        BytesCtorKind::Bytearray => alloc_bytearray(&out),
                    };
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if type_id == TYPE_ID_BIGINT {
                let big = bigint_ref(ptr);
                if big.is_negative() {
                    raise!("ValueError", "negative count");
                }
                let Some(len) = big.to_usize() else {
                    raise!(
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer"
                    );
                };
                let type_id = match kind {
                    BytesCtorKind::Bytes => TYPE_ID_BYTES,
                    BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                };
                return bytes_from_count(len, type_id);
            }
            let index_name_bits = intern_static_name(&INTERN_INDEX_NAME, b"__index__");
            let call_bits = attr_lookup_ptr(ptr, index_name_bits);
            dec_ref_bits(index_name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if i < 0 {
                        raise!("ValueError", "negative count");
                    }
                    let len = match usize::try_from(i) {
                        Ok(len) => len,
                        Err(_) => {
                            raise!(
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer"
                            );
                        }
                    };
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(len, type_id);
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr);
                    if big.is_negative() {
                        raise!("ValueError", "negative count");
                    }
                    let Some(len) = big.to_usize() else {
                        raise!(
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer"
                        );
                    };
                    dec_ref_bits(res_bits);
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(len, type_id);
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise!("TypeError", &msg);
            }
        }
    }
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        let type_name = class_name_for_error(type_of_bits(bits));
        let msg = kind.non_iterable_message(&type_name);
        raise!("TypeError", &msg);
    }
    let Some(out) = bytes_collect_from_iter(iter_bits, kind) else {
        return MoltObject::none().bits();
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(&out),
        BytesCtorKind::Bytearray => alloc_bytearray(&out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

fn bytes_from_str_impl(
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    kind: BytesCtorKind,
) -> u64 {
    let encoding_obj = obj_from_bits(encoding_bits);
    let errors_obj = obj_from_bits(errors_bits);
    let encoding = if encoding_obj.is_none() {
        None
    } else {
        let Some(encoding) = string_obj_to_owned(encoding_obj) else {
            let type_name = class_name_for_error(type_of_bits(encoding_bits));
            let msg = kind.arg_type_message("encoding", &type_name);
            raise!("TypeError", &msg);
        };
        Some(encoding)
    };
    let errors = if errors_obj.is_none() {
        None
    } else {
        let Some(errors) = string_obj_to_owned(errors_obj) else {
            let type_name = class_name_for_error(type_of_bits(errors_bits));
            let msg = kind.arg_type_message("errors", &type_name);
            raise!("TypeError", &msg);
        };
        Some(errors)
    };
    let src_obj = obj_from_bits(src_bits);
    let Some(src_ptr) = src_obj.as_ptr() else {
        if encoding.is_some() {
            raise!("TypeError", "encoding without a string argument");
        }
        if errors.is_some() {
            raise!("TypeError", "errors without a string argument");
        }
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(src_ptr) != TYPE_ID_STRING {
            if encoding.is_some() {
                raise!("TypeError", "encoding without a string argument");
            }
            if errors.is_some() {
                raise!("TypeError", "errors without a string argument");
            }
            return MoltObject::none().bits();
        }
    }
    let Some(encoding) = encoding else {
        raise!("TypeError", "string argument without an encoding");
    };
    let text = string_obj_to_owned(src_obj).unwrap_or_default();
    let out = match encode_string_with_errors(&text, &encoding, errors.as_deref()) {
        Ok(bytes) => bytes,
        Err(EncodeError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            raise!("LookupError", &msg);
        }
        Err(EncodeError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            raise!("LookupError", &msg);
        }
        Err(EncodeError::InvalidChar {
            encoding,
            ch,
            pos,
            limit,
        }) => {
            let escaped = unicode_escape(ch);
            let msg = format!(
                "'{encoding}' codec can't encode character '{escaped}' in position {pos}: ordinal not in range({limit})"
            );
            raise!("UnicodeEncodeError", &msg);
        }
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(&out),
        BytesCtorKind::Bytearray => alloc_bytearray(&out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_bytes_from_obj(bits: u64) -> u64 {
    bytes_from_obj_impl(bits, BytesCtorKind::Bytes)
}

#[no_mangle]
pub extern "C" fn molt_bytearray_from_obj(bits: u64) -> u64 {
    bytes_from_obj_impl(bits, BytesCtorKind::Bytearray)
}

#[no_mangle]
pub extern "C" fn molt_bytes_from_str(src_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    bytes_from_str_impl(src_bits, encoding_bits, errors_bits, BytesCtorKind::Bytes)
}

#[no_mangle]
pub extern "C" fn molt_bytearray_from_str(
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    bytes_from_str_impl(
        src_bits,
        encoding_bits,
        errors_bits,
        BytesCtorKind::Bytearray,
    )
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
            let format_bits = memoryview_format_bits(ptr);
            let out_ptr = alloc_memoryview(
                owner_bits,
                offset,
                len,
                itemsize,
                stride,
                readonly,
                format_bits,
            );
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let readonly = type_id == TYPE_ID_BYTES;
            let format_ptr = alloc_string(b"B");
            if format_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let format_bits = MoltObject::from_ptr(format_ptr).bits();
            let out_ptr = alloc_memoryview(bits, 0, len, 1, 1, readonly, format_bits);
            dec_ref_bits(format_bits);
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
                            memoryview_format_bits(ptr),
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
                if let Some(idx) = key.as_int() {
                    if type_id == TYPE_ID_STRING {
                        let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                        let Ok(text) = std::str::from_utf8(bytes) else {
                            return MoltObject::none().bits();
                        };
                        let mut i = idx;
                        let len = text.chars().count() as i64;
                        if i < 0 {
                            i += len;
                        }
                        if i < 0 || i >= len {
                            return MoltObject::none().bits();
                        }
                        let ch = match text.chars().nth(i as usize) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        let mut buf = [0u8; 4];
                        let out = ch.encode_utf8(&mut buf);
                        let out_ptr = alloc_string(out.as_bytes());
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
                    let len = bytes.len() as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_int(bytes[i as usize] as i64).bits();
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
            if let Some(name_bits) = attr_name_bits_from_bytes(b"__getitem__") {
                if let Some(call_bits) = attr_lookup_ptr(ptr, name_bits) {
                    dec_ref_bits(name_bits);
                    exception_stack_push();
                    let res = call_callable1(call_bits, key_bits);
                    dec_ref_bits(call_bits);
                    if exception_pending() {
                        exception_stack_pop();
                        return MoltObject::none().bits();
                    }
                    exception_stack_pop();
                    return res;
                }
                dec_ref_bits(name_bits);
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
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                return obj_bits;
            }
            if let Some(name_bits) = attr_name_bits_from_bytes(b"__setitem__") {
                if let Some(call_bits) = attr_lookup_ptr(ptr, name_bits) {
                    dec_ref_bits(name_bits);
                    exception_stack_push();
                    let _ = call_callable2(call_bits, key_bits, val_bits);
                    dec_ref_bits(call_bits);
                    if exception_pending() {
                        exception_stack_pop();
                        return MoltObject::none().bits();
                    }
                    exception_stack_pop();
                    return obj_bits;
                }
                dec_ref_bits(name_bits);
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
                    if !ensure_hashable(item_bits) {
                        return MoltObject::none().bits();
                    }
                    let order = dict_order(ptr);
                    let table = dict_table(ptr);
                    let found = dict_find_entry(order, table, item_bits).is_some();
                    return MoltObject::from_bool(found).bits();
                }
                TYPE_ID_SET | TYPE_ID_FROZENSET => {
                    if !ensure_hashable(item_bits) {
                        return MoltObject::none().bits();
                    }
                    let order = set_order(ptr);
                    let table = set_table(ptr);
                    let found = set_find_entry(order, table, item_bits).is_some();
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

extern "C" fn dict_keys_method(self_bits: u64) -> i64 {
    molt_dict_keys(self_bits) as i64
}

extern "C" fn dict_values_method(self_bits: u64) -> i64 {
    molt_dict_values(self_bits) as i64
}

extern "C" fn dict_items_method(self_bits: u64) -> i64 {
    molt_dict_items(self_bits) as i64
}

extern "C" fn dict_get_method(self_bits: u64, key_bits: u64, default_bits: u64) -> i64 {
    molt_dict_get(self_bits, key_bits, default_bits) as i64
}

extern "C" fn dict_pop_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> i64 {
    molt_dict_pop(self_bits, key_bits, default_bits, has_default_bits) as i64
}

extern "C" fn dict_clear_method(self_bits: u64) -> i64 {
    let obj = obj_from_bits(self_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.clear expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.clear expects dict");
        }
        dict_clear_in_place(ptr);
    }
    MoltObject::none().bits() as i64
}

extern "C" fn dict_copy_method(self_bits: u64) -> i64 {
    let obj = obj_from_bits(self_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.copy expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.copy expects dict");
        }
        let pairs = dict_order(ptr).clone();
        let out_ptr = alloc_dict_with_pairs(pairs.as_slice());
        if out_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        MoltObject::from_ptr(out_ptr).bits() as i64
    }
}

extern "C" fn dict_popitem_method(self_bits: u64) -> i64 {
    let obj = obj_from_bits(self_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.popitem expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.popitem expects dict");
        }
        let order = dict_order(ptr);
        if order.len() < 2 {
            raise!("KeyError", "popitem(): dictionary is empty");
        }
        let key_bits = order[order.len() - 2];
        let val_bits = order[order.len() - 1];
        let item_ptr = alloc_tuple(&[key_bits, val_bits]);
        if item_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        dec_ref_bits(key_bits);
        dec_ref_bits(val_bits);
        order.truncate(order.len() - 2);
        let entries = order.len() / 2;
        let table = dict_table(ptr);
        let capacity = dict_table_capacity(entries.max(1));
        dict_rebuild(order, table, capacity);
        MoltObject::from_ptr(item_ptr).bits() as i64
    }
}

extern "C" fn dict_setdefault_method(self_bits: u64, key_bits: u64, default_bits: u64) -> i64 {
    molt_dict_setdefault(self_bits, key_bits, default_bits) as i64
}

extern "C" fn dict_update_method(self_bits: u64, other_bits: u64) -> i64 {
    if other_bits == missing_bits() {
        return MoltObject::none().bits() as i64;
    }
    molt_dict_update(self_bits, other_bits) as i64
}

#[no_mangle]
pub extern "C" fn molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    if !ensure_hashable(key_bits) {
        return MoltObject::none().bits();
    }
    molt_store_index(dict_bits, key_bits, val_bits)
}

#[no_mangle]
pub extern "C" fn molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.get expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.get expects dict");
        }
        if !ensure_hashable(key_bits) {
            return MoltObject::none().bits();
        }
        if let Some(val) = dict_get_in_place(ptr, key_bits) {
            inc_ref_bits(val);
            return val;
        }
        inc_ref_bits(default_bits);
        default_bits
    }
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
    let Some(ptr) = dict_obj.as_ptr() else {
        raise!("TypeError", "dict.pop expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.pop expects dict");
        }
        if !ensure_hashable(key_bits) {
            return MoltObject::none().bits();
        }
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
    }
    raise!("KeyError", "dict.pop missing key");
}

#[no_mangle]
pub extern "C" fn molt_dict_setdefault(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(ptr) = dict_obj.as_ptr() else {
        raise!("TypeError", "dict.setdefault expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.setdefault expects dict");
        }
        if !ensure_hashable(key_bits) {
            return MoltObject::none().bits();
        }
        if let Some(val) = dict_get_in_place(ptr, key_bits) {
            inc_ref_bits(val);
            return val;
        }
        dict_set_in_place(ptr, key_bits, default_bits);
        if exception_pending() {
            return MoltObject::none().bits();
        }
        inc_ref_bits(default_bits);
        default_bits
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_update(dict_bits: u64, other_bits: u64) -> u64 {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        raise!("TypeError", "dict.update expects dict");
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.update expects dict");
        }
        let mut iter_bits = other_bits;
        let other_obj = obj_from_bits(other_bits);
        if let Some(ptr) = other_obj.as_ptr() {
            if object_type_id(ptr) == TYPE_ID_DICT {
                iter_bits = molt_dict_items(other_bits);
                if obj_from_bits(iter_bits).is_none() {
                    return MoltObject::none().bits();
                }
            }
        }
        let source_bits = iter_bits;
        let iter = molt_iter(iter_bits);
        if obj_from_bits(iter).is_none() {
            let mapping_obj = obj_from_bits(source_bits);
            let Some(mapping_ptr) = mapping_obj.as_ptr() else {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            };
            let Some(keys_bits) = attr_name_bits_from_bytes(b"keys") else {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            };
            let keys_method_bits = attr_lookup_ptr(mapping_ptr, keys_bits);
            dec_ref_bits(keys_bits);
            let Some(keys_method_bits) = keys_method_bits else {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            };
            let keys_iterable = call_callable0(keys_method_bits);
            let keys_iter = molt_iter(keys_iterable);
            if obj_from_bits(keys_iter).is_none() {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            }
            let Some(getitem_bits) = attr_name_bits_from_bytes(b"__getitem__") else {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            };
            let getitem_method_bits = attr_lookup_ptr(mapping_ptr, getitem_bits);
            dec_ref_bits(getitem_bits);
            let Some(getitem_method_bits) = getitem_method_bits else {
                raise!("TypeError", "dict.update expects a mapping or iterable");
            };
            loop {
                let pair_bits = molt_iter_next(keys_iter);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let val_bits = call_callable1(getitem_method_bits, key_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                dict_set_in_place(dict_ptr, key_bits, val_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
            }
            return MoltObject::none().bits();
        }
        let mut elem_index = 0usize;
        loop {
            let pair_bits = molt_iter_next(iter);
            if exception_pending() {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let item_bits = elems[0];
            match dict_pair_from_item(item_bits) {
                Ok((key, val)) => {
                    dict_set_in_place(dict_ptr, key, val);
                    if exception_pending() {
                        return MoltObject::none().bits();
                    }
                }
                Err(DictSeqError::NotIterable) => {
                    let msg = format!(
                        "cannot convert dictionary update sequence element #{elem_index} to a sequence"
                    );
                    raise!("TypeError", &msg);
                }
                Err(DictSeqError::BadLen(len)) => {
                    let msg = format!(
                        "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                    );
                    raise!("ValueError", &msg);
                }
                Err(DictSeqError::Exception) => {
                    return MoltObject::none().bits();
                }
            }
            elem_index += 1;
        }
        MoltObject::none().bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_clear(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.clear expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.clear expects dict");
        }
        dict_clear_in_place(ptr);
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_copy(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.copy expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.copy expects dict");
        }
        let pairs = dict_order(ptr).clone();
        let out_ptr = alloc_dict_with_pairs(pairs.as_slice());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_popitem(dict_bits: u64) -> u64 {
    let obj = obj_from_bits(dict_bits);
    let Some(ptr) = obj.as_ptr() else {
        raise!("TypeError", "dict.popitem expects dict");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.popitem expects dict");
        }
        let order = dict_order(ptr);
        if order.len() < 2 {
            raise!("KeyError", "popitem(): dictionary is empty");
        }
        let key_bits = order[order.len() - 2];
        let val_bits = order[order.len() - 1];
        let item_ptr = alloc_tuple(&[key_bits, val_bits]);
        if item_ptr.is_null() {
            return MoltObject::none().bits();
        }
        dec_ref_bits(key_bits);
        dec_ref_bits(val_bits);
        order.truncate(order.len() - 2);
        let entries = order.len() / 2;
        let table = dict_table(ptr);
        let capacity = dict_table_capacity(entries.max(1));
        dict_rebuild(order, table, capacity);
        MoltObject::from_ptr(item_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_update_kwstar(dict_bits: u64, mapping_bits: u64) -> u64 {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        raise!("TypeError", "dict.update expects dict");
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            raise!("TypeError", "dict.update expects dict");
        }
        let mapping_obj = obj_from_bits(mapping_bits);
        let Some(mapping_ptr) = mapping_obj.as_ptr() else {
            raise!("TypeError", "argument after ** must be a mapping");
        };
        if object_type_id(mapping_ptr) == TYPE_ID_DICT {
            let order = dict_order(mapping_ptr);
            for idx in (0..order.len()).step_by(2) {
                let key_bits = order[idx];
                let val_bits = order[idx + 1];
                let key_obj = obj_from_bits(key_bits);
                let Some(key_ptr) = key_obj.as_ptr() else {
                    raise!("TypeError", "keywords must be strings");
                };
                if object_type_id(key_ptr) != TYPE_ID_STRING {
                    raise!("TypeError", "keywords must be strings");
                }
                dict_set_in_place(dict_ptr, key_bits, val_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
            }
            return MoltObject::none().bits();
        }
        let Some(keys_bits) = attr_name_bits_from_bytes(b"keys") else {
            raise!("TypeError", "argument after ** must be a mapping");
        };
        let keys_method_bits = attr_lookup_ptr(mapping_ptr, keys_bits);
        dec_ref_bits(keys_bits);
        let Some(keys_method_bits) = keys_method_bits else {
            raise!("TypeError", "argument after ** must be a mapping");
        };
        let keys_iterable = call_callable0(keys_method_bits);
        let iter_bits = molt_iter(keys_iterable);
        if obj_from_bits(iter_bits).is_none() {
            raise!("TypeError", "argument after ** must be a mapping");
        }
        let Some(getitem_bits) = attr_name_bits_from_bytes(b"__getitem__") else {
            raise!("TypeError", "argument after ** must be a mapping");
        };
        let getitem_method_bits = attr_lookup_ptr(mapping_ptr, getitem_bits);
        dec_ref_bits(getitem_bits);
        let Some(getitem_method_bits) = getitem_method_bits else {
            raise!("TypeError", "argument after ** must be a mapping");
        };
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(obj_from_bits(done_bits)) {
                break;
            }
            let key_bits = elems[0];
            let key_obj = obj_from_bits(key_bits);
            let Some(key_ptr) = key_obj.as_ptr() else {
                raise!("TypeError", "keywords must be strings");
            };
            if object_type_id(key_ptr) != TYPE_ID_STRING {
                raise!("TypeError", "keywords must be strings");
            }
            let val_bits = call_callable1(getitem_method_bits, key_bits);
            dict_set_in_place(dict_ptr, key_bits, val_bits);
            if exception_pending() {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_set_add(set_bits: u64, key_bits: u64) -> u64 {
    if !ensure_hashable(key_bits) {
        return MoltObject::none().bits();
    }
    let obj = obj_from_bits(set_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                set_add_in_place(ptr, key_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_frozenset_add(set_bits: u64, key_bits: u64) -> u64 {
    if !ensure_hashable(key_bits) {
        return MoltObject::none().bits();
    }
    let obj = obj_from_bits(set_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_FROZENSET {
                set_add_in_place(ptr, key_bits);
                if exception_pending() {
                    return MoltObject::none().bits();
                }
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_discard(set_bits: u64, key_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                set_del_in_place(ptr, key_bits);
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_remove(set_bits: u64, key_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                if set_del_in_place(ptr, key_bits) {
                    return MoltObject::none().bits();
                }
                raise!("KeyError", "set.remove(x): x not in set");
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_pop(set_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                let order = set_order(ptr);
                if order.is_empty() {
                    raise!("KeyError", "pop from an empty set");
                }
                let key_bits = order.pop().unwrap_or_else(|| MoltObject::none().bits());
                let entries = order.len();
                let table = set_table(ptr);
                let capacity = set_table_capacity(entries.max(1));
                set_rebuild(order, table, capacity);
                inc_ref_bits(key_bits);
                return key_bits;
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_update(set_bits: u64, other_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    let other = obj_from_bits(other_bits);
    if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
        unsafe {
            if object_type_id(set_ptr) == TYPE_ID_SET {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                    let entries = set_order(other_ptr);
                    for entry in entries.iter().copied() {
                        set_add_in_place(set_ptr, entry);
                    }
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_intersection_update(set_bits: u64, other_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    let other = obj_from_bits(other_bits);
    if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
        unsafe {
            if object_type_id(set_ptr) == TYPE_ID_SET {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                    let other_order = set_order(other_ptr);
                    let other_table = set_table(other_ptr);
                    let set_entries = set_order(set_ptr).clone();
                    let mut new_entries = Vec::with_capacity(set_entries.len());
                    for entry in set_entries {
                        if set_find_entry(other_order, other_table, entry).is_some() {
                            new_entries.push(entry);
                        }
                    }
                    set_replace_entries(set_ptr, &new_entries);
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    let other = obj_from_bits(other_bits);
    if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
        unsafe {
            if object_type_id(set_ptr) == TYPE_ID_SET {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                    let other_order = set_order(other_ptr);
                    let other_table = set_table(other_ptr);
                    let set_entries = set_order(set_ptr).clone();
                    let mut new_entries = Vec::with_capacity(set_entries.len());
                    for entry in set_entries {
                        if set_find_entry(other_order, other_table, entry).is_none() {
                            new_entries.push(entry);
                        }
                    }
                    set_replace_entries(set_ptr, &new_entries);
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_set_symdiff_update(set_bits: u64, other_bits: u64) -> u64 {
    let obj = obj_from_bits(set_bits);
    let other = obj_from_bits(other_bits);
    if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
        unsafe {
            if object_type_id(set_ptr) == TYPE_ID_SET {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                    let other_order = set_order(other_ptr);
                    let other_table = set_table(other_ptr);
                    let set_entries = set_order(set_ptr).clone();
                    let set_table_ptr = set_table(set_ptr);
                    let mut new_entries = Vec::with_capacity(set_entries.len() + other_order.len());
                    for entry in &set_entries {
                        if set_find_entry(other_order, other_table, *entry).is_none() {
                            new_entries.push(*entry);
                        }
                    }
                    for entry in other_order.iter().copied() {
                        if set_find_entry(set_entries.as_slice(), set_table_ptr, entry).is_none() {
                            new_entries.push(entry);
                        }
                    }
                    set_replace_entries(set_ptr, &new_entries);
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_enumerate(iterable_bits: u64, start_bits: u64, has_start_bits: u64) -> u64 {
    let has_start = is_truthy(obj_from_bits(has_start_bits));
    let iter_bits = molt_iter(iterable_bits);
    if obj_from_bits(iter_bits).is_none() {
        raise!("TypeError", "object is not iterable");
    }
    let index_bits = if has_start {
        let start_obj = obj_from_bits(start_bits);
        let mut is_int_like = start_obj.is_int() || start_obj.is_bool();
        if !is_int_like {
            if let Some(ptr) = start_obj.as_ptr() {
                unsafe {
                    is_int_like = object_type_id(ptr) == TYPE_ID_BIGINT;
                }
            }
        }
        if !is_int_like {
            raise!("TypeError", "enumerate() start must be an int");
        }
        start_bits
    } else {
        MoltObject::from_int(0).bits()
    };
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let enum_ptr = alloc_object(total, TYPE_ID_ENUMERATE);
    if enum_ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        *(enum_ptr as *mut u64) = iter_bits;
        *(enum_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = index_bits;
    }
    inc_ref_bits(index_bits);
    MoltObject::from_ptr(enum_ptr).bits()
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
            if type_id == TYPE_ID_ENUMERATE {
                inc_ref_bits(iter_bits);
                return iter_bits;
            }
            if type_id == TYPE_ID_ITER {
                inc_ref_bits(iter_bits);
                return iter_bits;
            }
            if type_id == TYPE_ID_LIST
                || type_id == TYPE_ID_TUPLE
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
                || type_id == TYPE_ID_DICT
                || type_id == TYPE_ID_SET
                || type_id == TYPE_ID_FROZENSET
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
            if let Some(name_bits) = attr_name_bits_from_bytes(b"__iter__") {
                if let Some(call_bits) = attr_lookup_ptr(ptr, name_bits) {
                    dec_ref_bits(name_bits);
                    let res = call_callable0(call_bits);
                    dec_ref_bits(call_bits);
                    if let Some(res_ptr) = maybe_ptr_from_bits(res) {
                        let res_type = object_type_id(res_ptr);
                        if res_type == TYPE_ID_LIST
                            || res_type == TYPE_ID_TUPLE
                            || res_type == TYPE_ID_STRING
                            || res_type == TYPE_ID_BYTES
                            || res_type == TYPE_ID_BYTEARRAY
                            || res_type == TYPE_ID_DICT
                            || res_type == TYPE_ID_SET
                            || res_type == TYPE_ID_FROZENSET
                            || res_type == TYPE_ID_DICT_KEYS_VIEW
                            || res_type == TYPE_ID_DICT_VALUES_VIEW
                            || res_type == TYPE_ID_DICT_ITEMS_VIEW
                            || res_type == TYPE_ID_RANGE
                        {
                            let total = std::mem::size_of::<MoltHeader>()
                                + std::mem::size_of::<u64>()
                                + std::mem::size_of::<usize>();
                            let iter_ptr = alloc_object(total, TYPE_ID_ITER);
                            if iter_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            inc_ref_bits(res);
                            *(iter_ptr as *mut u64) = res;
                            iter_set_index(iter_ptr, 0);
                            return MoltObject::from_ptr(iter_ptr).bits();
                        }
                    }
                    return res;
                }
                dec_ref_bits(name_bits);
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_aiter(obj_bits: u64) -> u64 {
    unsafe {
        let obj = obj_from_bits(obj_bits);
        let Some(name_bits) = attr_name_bits_from_bytes(b"__aiter__") else {
            return MoltObject::none().bits();
        };
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
                let res = call_poll_fn(poll_fn_addr, ptr);
                return res as u64;
            }
            if object_type_id(ptr) == TYPE_ID_ENUMERATE {
                let iter_bits = enumerate_target_bits(ptr);
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    return pair_bits;
                }
                let idx_bits = enumerate_index_bits(ptr);
                let item_ptr = alloc_tuple(&[idx_bits, val_bits]);
                if item_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let item_bits = MoltObject::from_ptr(item_ptr).bits();
                let done_false = MoltObject::from_bool(false).bits();
                let out_ptr = alloc_tuple(&[item_bits, done_false]);
                if out_ptr.is_null() {
                    dec_ref_bits(item_bits);
                    return MoltObject::none().bits();
                }
                dec_ref_bits(item_bits);
                let next_bits = molt_add(idx_bits, MoltObject::from_int(1).bits());
                if obj_from_bits(next_bits).is_none() {
                    return MoltObject::none().bits();
                }
                dec_ref_bits(idx_bits);
                enumerate_set_index_bits(ptr, next_bits);
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if object_type_id(ptr) != TYPE_ID_ITER {
                if let Some(name_bits) = attr_name_bits_from_bytes(b"__next__") {
                    if let Some(call_bits) = attr_lookup_ptr(ptr, name_bits) {
                        dec_ref_bits(name_bits);
                        exception_stack_push();
                        let val_bits = call_callable0(call_bits);
                        dec_ref_bits(call_bits);
                        if exception_pending() {
                            let exc_bits = molt_exception_last();
                            let kind_bits = molt_exception_kind(exc_bits);
                            let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                            dec_ref_bits(kind_bits);
                            if kind.as_deref() == Some("StopIteration") {
                                molt_exception_clear();
                                dec_ref_bits(exc_bits);
                                exception_stack_pop();
                                return generator_done_tuple(MoltObject::none().bits());
                            }
                            dec_ref_bits(exc_bits);
                            exception_stack_pop();
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop();
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(&[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    dec_ref_bits(name_bits);
                }
                return MoltObject::none().bits();
            }
            let target_bits = iter_target_bits(ptr);
            let target_obj = obj_from_bits(target_bits);
            let idx = iter_index(ptr);
            if let Some(target_ptr) = target_obj.as_ptr() {
                let target_type = object_type_id(target_ptr);
                if target_type == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(
                        string_bytes(target_ptr),
                        string_len(target_ptr),
                    );
                    if idx >= bytes.len() {
                        let none_bits = MoltObject::none().bits();
                        let done_bits = MoltObject::from_bool(true).bits();
                        let tuple_ptr = alloc_tuple(&[none_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    let tail = &bytes[idx..];
                    let Ok(text) = std::str::from_utf8(tail) else {
                        return MoltObject::none().bits();
                    };
                    let Some(ch) = text.chars().next() else {
                        let none_bits = MoltObject::none().bits();
                        let done_bits = MoltObject::from_bool(true).bits();
                        let tuple_ptr = alloc_tuple(&[none_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    };
                    let mut buf = [0u8; 4];
                    let out = ch.encode_utf8(&mut buf);
                    let out_ptr = alloc_string(out.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let val_bits = MoltObject::from_ptr(out_ptr).bits();
                    let next_idx = idx + ch.len_utf8();
                    iter_set_index(ptr, next_idx);
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(&[val_bits, done_bits]);
                    if tuple_ptr.is_null() {
                        dec_ref_bits(val_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(val_bits);
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                if target_type == TYPE_ID_BYTES || target_type == TYPE_ID_BYTEARRAY {
                    let bytes =
                        std::slice::from_raw_parts(bytes_data(target_ptr), bytes_len(target_ptr));
                    if idx >= bytes.len() {
                        let none_bits = MoltObject::none().bits();
                        let done_bits = MoltObject::from_bool(true).bits();
                        let tuple_ptr = alloc_tuple(&[none_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    let val_bits = MoltObject::from_int(bytes[idx] as i64).bits();
                    iter_set_index(ptr, idx + 1);
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(&[val_bits, done_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
            }
            let (len, next_val, needs_drop) = if let Some(target_ptr) = target_obj.as_ptr() {
                let target_type = object_type_id(target_ptr);
                if target_type == TYPE_ID_LIST || target_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(target_ptr);
                    if idx >= elems.len() {
                        (elems.len(), None, false)
                    } else {
                        (elems.len(), Some(elems[idx]), false)
                    }
                } else if target_type == TYPE_ID_SET || target_type == TYPE_ID_FROZENSET {
                    let elems = set_order(target_ptr);
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
        let Some(name_bits) = attr_name_bits_from_bytes(b"__anext__") else {
            return MoltObject::none().bits();
        };
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
            let list_elems = seq_vec(list_ptr);
            let other_obj = obj_from_bits(other_bits);
            if let Some(other_ptr) = other_obj.as_ptr() {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_LIST || other_type == TYPE_ID_TUPLE {
                    let src = seq_vec_ref(other_ptr);
                    for &item in src.iter() {
                        list_elems.push(item);
                        inc_ref_bits(item);
                    }
                    return MoltObject::none().bits();
                }
                if other_type == TYPE_ID_DICT {
                    let order = dict_order(other_ptr);
                    for idx in (0..order.len()).step_by(2) {
                        let key_bits = order[idx];
                        list_elems.push(key_bits);
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
                                list_elems.push(MoltObject::from_ptr(tuple_ptr).bits());
                            } else {
                                let item = if other_type == TYPE_ID_DICT_KEYS_VIEW {
                                    key_bits
                                } else {
                                    val_bits
                                };
                                list_elems.push(item);
                                inc_ref_bits(item);
                            }
                        }
                    }
                    return MoltObject::none().bits();
                }
            }
            let iter_bits = molt_iter(other_bits);
            if obj_from_bits(iter_bits).is_none() {
                raise!("TypeError", "object is not iterable");
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let pair_elems = seq_vec_ref(pair_ptr);
                if pair_elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = pair_elems[1];
                if is_truthy(obj_from_bits(done_bits)) {
                    break;
                }
                let val_bits = pair_elems[0];
                list_elems.push(val_bits);
                inc_ref_bits(val_bits);
            }
            return MoltObject::none().bits();
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
pub extern "C" fn molt_list_clear(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) == TYPE_ID_LIST {
                let elems = seq_vec(list_ptr);
                for &elem in elems.iter() {
                    dec_ref_bits(elem);
                }
                elems.clear();
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_copy(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) == TYPE_ID_LIST {
                let elems = seq_vec_ref(list_ptr);
                let out_ptr = alloc_list(elems.as_slice());
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
pub extern "C" fn molt_list_reverse(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) == TYPE_ID_LIST {
                let elems = seq_vec(list_ptr);
                elems.reverse();
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_list_sort(list_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    if let Some(list_ptr) = list_obj.as_ptr() {
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let use_key = !obj_from_bits(key_bits).is_none();
            let reverse = is_truthy(obj_from_bits(reverse_bits));
            let elems = seq_vec_ref(list_ptr);
            let mut items: Vec<SortItem> = Vec::with_capacity(elems.len());
            for &val_bits in elems.iter() {
                let key_val_bits = if use_key {
                    let res_bits = call_callable1(key_bits, val_bits);
                    if exception_pending() {
                        dec_ref_bits(res_bits);
                        for item in items.drain(..) {
                            dec_ref_bits(item.key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                items.push(SortItem {
                    key_bits: key_val_bits,
                    value_bits: val_bits,
                });
            }
            let mut error: Option<SortError> = None;
            items.sort_by(|left, right| {
                if error.is_some() {
                    return Ordering::Equal;
                }
                let outcome =
                    compare_objects(obj_from_bits(left.key_bits), obj_from_bits(right.key_bits));
                match outcome {
                    CompareOutcome::Ordered(ordering) => {
                        if reverse {
                            ordering.reverse()
                        } else {
                            ordering
                        }
                    }
                    CompareOutcome::Unordered => Ordering::Equal,
                    CompareOutcome::NotComparable => {
                        error = Some(SortError::NotComparable(left.key_bits, right.key_bits));
                        Ordering::Equal
                    }
                    CompareOutcome::Error => {
                        error = Some(SortError::Exception);
                        Ordering::Equal
                    }
                }
            });
            if let Some(error) = error {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(item.key_bits);
                    }
                }
                match error {
                    SortError::NotComparable(left_bits, right_bits) => {
                        let msg = format!(
                            "'<' not supported between instances of '{}' and '{}'",
                            type_name(obj_from_bits(left_bits)),
                            type_name(obj_from_bits(right_bits)),
                        );
                        raise!("TypeError", &msg);
                    }
                    SortError::Exception => {
                        return MoltObject::none().bits();
                    }
                }
            }
            let mut new_elems: Vec<u64> = Vec::with_capacity(items.len());
            for item in items.iter() {
                new_elems.push(item.value_bits);
            }
            if use_key {
                for item in items.drain(..) {
                    dec_ref_bits(item.key_bits);
                }
            }
            let elems_mut = seq_vec(list_ptr);
            *elems_mut = new_elems;
            return MoltObject::none().bits();
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
        println!("{}", format_float(f));
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
    if let Some(ptr) = maybe_ptr_from_bits(val) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let s = String::from_utf8_lossy(bytes);
                println!("{s}");
                return;
            }
            if type_id == TYPE_ID_BIGINT {
                println!("{}", bigint_ref(ptr));
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
            if type_id == TYPE_ID_SET {
                let elems = set_order(ptr);
                if elems.is_empty() {
                    println!("set()");
                    return;
                }
                let mut out = String::from("{");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push('}');
                println!("{out}");
                return;
            }
            if type_id == TYPE_ID_FROZENSET {
                let elems = set_order(ptr);
                if elems.is_empty() {
                    println!("frozenset()");
                    return;
                }
                let mut out = String::from("frozenset({");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push_str("})");
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
    let rendered = format_obj_str(obj);
    println!("{rendered}");
}

#[no_mangle]
pub extern "C" fn molt_print_newline() {
    println!();
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        if f.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
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
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
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
            let str_name_bits = intern_static_name(&INTERN_STR_NAME, b"__str__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, str_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(res_bits);
                    return rendered;
                }
                dec_ref_bits(res_bits);
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
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let s = String::from_utf8_lossy(bytes);
                return format_string_repr(&s);
            }
            if type_id == TYPE_ID_BIGINT {
                return bigint_ref(ptr).to_string();
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
            if type_id == TYPE_ID_NOT_IMPLEMENTED {
                return "NotImplemented".to_string();
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
            if type_id == TYPE_ID_SET {
                let elems = set_order(ptr);
                if elems.is_empty() {
                    return "set()".to_string();
                }
                let mut out = String::from("{");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_FROZENSET {
                let elems = set_order(ptr);
                if elems.is_empty() {
                    return "frozenset()".to_string();
                }
                let mut out = String::from("frozenset({");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(obj_from_bits(*elem)));
                }
                out.push_str("})");
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
            let repr_name_bits = intern_static_name(&INTERN_REPR_NAME, b"__repr__");
            if let Some(call_bits) = attr_lookup_ptr(ptr, repr_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(res_bits);
                    return rendered;
                }
                dec_ref_bits(res_bits);
                return "<object>".to_string();
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
    sign: Option<char>,
    alternate: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_format_spec(spec: &str) -> Result<FormatSpec, &'static str> {
    if spec.is_empty() {
        return Ok(FormatSpec {
            fill: ' ',
            align: None,
            sign: None,
            alternate: false,
            width: None,
            grouping: None,
            precision: None,
            ty: None,
        });
    }
    let mut chars = spec.chars().peekable();
    let mut fill = ' ';
    let mut align = None;
    let mut sign = None;
    let mut alternate = false;
    let mut grouping = None;
    let mut peeked = chars.clone();
    let first = peeked.next();
    let second = peeked.next();
    if let (Some(c1), Some(c2)) = (first, second) {
        if matches!(c2, '<' | '>' | '^' | '=') {
            fill = c1;
            align = Some(c2);
            chars.next();
            chars.next();
        } else if matches!(c1, '<' | '>' | '^' | '=') {
            align = Some(c1);
            chars.next();
        }
    } else if let Some(c1) = first {
        if matches!(c1, '<' | '>' | '^' | '=') {
            align = Some(c1);
            chars.next();
        }
    }

    if let Some(ch) = chars.peek().copied() {
        if matches!(ch, '+' | '-' | ' ') {
            sign = Some(ch);
            chars.next();
        }
    }

    if matches!(chars.peek(), Some('#')) {
        alternate = true;
        chars.next();
    }

    if align.is_none() && matches!(chars.peek(), Some('0')) {
        fill = '0';
        align = Some('=');
        chars.next();
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

    if let Some(ch) = chars.peek().copied() {
        if ch == ',' || ch == '_' {
            grouping = Some(ch);
            chars.next();
        }
    }

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
        sign,
        alternate,
        width,
        grouping,
        precision,
        ty,
    })
}

fn apply_grouping(text: &str, group: usize, sep: char) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / group);
    for (count, ch) in text.chars().rev().enumerate() {
        if count > 0 && count.is_multiple_of(group) {
            out.push(sep);
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn apply_alignment(prefix: &str, body: &str, spec: &FormatSpec, default_align: char) -> String {
    let text = format!("{prefix}{body}");
    let width = match spec.width {
        Some(val) => val,
        None => return text,
    };
    let len = text.chars().count();
    if len >= width {
        return text;
    }
    let pad_len = width - len;
    let align = spec.align.unwrap_or(default_align);
    let fill = spec.fill;
    if align == '=' {
        let padding = fill.to_string().repeat(pad_len);
        return format!("{prefix}{padding}{body}");
    }
    let padding = fill.to_string().repeat(pad_len);
    match align {
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
        _ => text,
    }
}

fn trim_float_trailing(text: &str, alternate: bool) -> String {
    if alternate {
        return text.to_string();
    }
    let exp_pos = text.find(['e', 'E']).unwrap_or(text.len());
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut end = mantissa.len();
    if let Some(dot) = mantissa.find('.') {
        let bytes = mantissa.as_bytes();
        while end > dot + 1 && bytes[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot + 1 {
            end = dot;
        }
    }
    let trimmed = &mantissa[..end];
    format!("{trimmed}{exp}")
}

fn normalize_exponent(text: &str, upper: bool) -> String {
    let (exp_pos, exp_char) = if let Some(pos) = text.find('e') {
        (pos, 'e')
    } else if let Some(pos) = text.find('E') {
        (pos, 'E')
    } else {
        return text.to_string();
    };
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut exp_text = &exp[1..];
    let mut sign = '+';
    if let Some(first) = exp_text.chars().next() {
        if first == '+' || first == '-' {
            sign = first;
            exp_text = &exp_text[1..];
        }
    }
    let digits = if exp_text.is_empty() { "0" } else { exp_text };
    let mut padded = String::from(digits);
    if padded.len() == 1 {
        padded.insert(0, '0');
    }
    let exp_out = if upper { 'E' } else { exp_char };
    format!("{mantissa}{exp_out}{sign}{padded}")
}

fn format_string_with_spec(text: String, spec: &FormatSpec) -> String {
    let mut out = text;
    if let Some(prec) = spec.precision {
        out = out.chars().take(prec).collect();
    }
    apply_alignment("", &out, spec, '<')
}

fn format_int_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    if spec.precision.is_some() {
        return Err(("ValueError", "precision not allowed in integer format"));
    }
    let ty = spec.ty.unwrap_or('d');
    let mut value = if let Some(i) = obj.as_int() {
        BigInt::from(i)
    } else if let Some(b) = obj.as_bool() {
        BigInt::from(if b { 1 } else { 0 })
    } else if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        unsafe { bigint_ref(ptr).clone() }
    } else {
        return Err(("TypeError", "format requires int"));
    };
    if ty == 'c' {
        if value.is_negative() {
            return Err(("ValueError", "format c requires non-negative int"));
        }
        let code = value
            .to_u32()
            .ok_or(("ValueError", "format c out of range"))?;
        let ch = std::char::from_u32(code).ok_or(("ValueError", "format c out of range"))?;
        return Ok(format_string_with_spec(ch.to_string(), spec));
    }
    let base = match ty {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        'd' | 'n' => 10,
        _ => return Err(("ValueError", "unsupported int format type")),
    };
    let negative = value.is_negative();
    if negative {
        value = -value;
    }
    let mut digits = value.to_str_radix(base);
    if ty == 'X' {
        digits = digits.to_uppercase();
    }
    if let Some(sep) = spec.grouping {
        let group = match base {
            2 | 16 => 4,
            8 => 3,
            _ => 3,
        };
        digits = apply_grouping(&digits, group, sep);
    }
    let mut prefix = String::new();
    if negative {
        prefix.push('-');
    } else if let Some(sign) = spec.sign {
        if sign == '+' || sign == ' ' {
            prefix.push(sign);
        }
    }
    if spec.alternate {
        match ty {
            'b' => prefix.push_str("0b"),
            'o' => prefix.push_str("0o"),
            'x' => prefix.push_str("0x"),
            'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    Ok(apply_alignment(&prefix, &digits, spec, '>'))
}

fn format_float_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
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
        return Err(("TypeError", "format requires float"));
    };
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let ty = spec.ty.unwrap_or('g');
    let upper = matches!(ty, 'F' | 'E' | 'G');
    if val.is_nan() {
        let text = if upper { "NAN" } else { "nan" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    if val.is_infinite() {
        let text = if upper { "INF" } else { "inf" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    let mut prefix = String::new();
    if val.is_sign_negative() {
        prefix.push('-');
    } else if let Some(sign) = spec.sign {
        if sign == '+' || sign == ' ' {
            prefix.push(sign);
        }
    }
    let abs_val = val.abs();
    let prec = spec.precision.unwrap_or(6);
    let mut body = if use_default {
        format_float(abs_val)
    } else {
        match ty {
            'f' | 'F' => format!("{:.*}", prec, abs_val),
            'e' | 'E' => format!("{:.*e}", prec, abs_val),
            'g' | 'G' => {
                let digits = if prec == 0 { 1 } else { prec };
                if abs_val == 0.0 {
                    "0".to_string()
                } else {
                    let exp = abs_val.log10().floor() as i32;
                    if exp < -4 || exp >= digits as i32 {
                        let text = format!("{:.*e}", digits - 1, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    } else {
                        let frac = (digits as i32 - 1 - exp).max(0) as usize;
                        let text = format!("{:.*}", frac, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    }
                }
            }
            '%' => {
                let scaled = abs_val * 100.0;
                format!("{:.*}", prec, scaled)
            }
            _ => return Err(("ValueError", "unsupported float format type")),
        }
    };
    body = normalize_exponent(&body, upper);
    if upper {
        body = body.replace('e', "E");
    }
    if spec.alternate && !body.contains('.') && !body.contains('E') && !body.contains('e') {
        body.push('.');
    }
    if let Some(sep) = spec.grouping {
        if !body.contains('e') && !body.contains('E') {
            let mut parts = body.splitn(2, '.');
            let int_part = parts.next().unwrap_or("");
            let frac_part = parts.next();
            let grouped = apply_grouping(int_part, 3, sep);
            body = if let Some(frac) = frac_part {
                format!("{grouped}.{frac}")
            } else {
                grouped
            };
        }
    }
    if ty == '%' {
        body.push('%');
    }
    Ok(apply_alignment(&prefix, &body, spec, '>'))
}

fn format_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    match spec.ty {
        Some('s') => Ok(format_string_with_spec(format_obj_str(obj), spec)),
        Some('d') | Some('b') | Some('o') | Some('x') | Some('X') | Some('n') | Some('c') => {
            format_int_with_spec(obj, spec)
        }
        Some('f') | Some('F') | Some('e') | Some('E') | Some('g') | Some('G') | Some('%') => {
            format_float_with_spec(obj, spec)
        }
        Some(_) => Err(("ValueError", "unsupported format type")),
        None => {
            if obj.as_float().is_some() {
                format_float_with_spec(obj, spec)
            } else if obj.as_int().is_some()
                || obj.as_bool().is_some()
                || bigint_ptr_from_bits(obj.bits()).is_some()
            {
                format_int_with_spec(obj, spec)
            } else {
                Ok(format_string_with_spec(format_obj_str(obj), spec))
            }
        }
    }
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
    (*header_ptr)
        .ref_count
        .fetch_add(1, AtomicOrdering::Relaxed);
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
    if header.type_id == TYPE_ID_NOT_IMPLEMENTED {
        return;
    }
    if header.ref_count.fetch_sub(1, AtomicOrdering::AcqRel) == 1 {
        std::sync::atomic::fence(AtomicOrdering::Acquire);
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
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let order_ptr = set_order_ptr(ptr);
                let table_ptr = set_table_ptr(ptr);
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
            TYPE_ID_ENUMERATE => {
                let target_bits = enumerate_target_bits(ptr);
                let index_bits = enumerate_index_bits(ptr);
                dec_ref_bits(target_bits);
                dec_ref_bits(index_bits);
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
            TYPE_ID_SET_BUILDER => {
                let vec_ptr = *(ptr as *mut *mut Vec<u64>);
                if !vec_ptr.is_null() {
                    drop(Box::from_raw(vec_ptr));
                }
            }
            TYPE_ID_CALLARGS => {
                let args_ptr = callargs_ptr(ptr);
                if !args_ptr.is_null() {
                    drop(Box::from_raw(args_ptr));
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
                let trace_bits = exception_trace_bits(ptr);
                dec_ref_bits(kind_bits);
                dec_ref_bits(msg_bits);
                dec_ref_bits(cause_bits);
                dec_ref_bits(context_bits);
                dec_ref_bits(suppress_bits);
                dec_ref_bits(trace_bits);
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
                    let has_ptrs = (header.flags & HEADER_FLAG_HAS_PTRS) != 0;
                    let payload = header
                        .size
                        .saturating_sub(std::mem::size_of::<MoltHeader>());
                    if has_ptrs {
                        let slots = payload / std::mem::size_of::<u64>();
                        let payload_ptr = ptr as *mut u64;
                        for idx in 0..slots {
                            dec_ref_bits(*payload_ptr.add(idx));
                        }
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 && (header.flags & HEADER_FLAG_SKIP_CLASS_DECREF) == 0 {
                        dec_ref_bits(class_bits);
                    }
                } else {
                    let payload = header
                        .size
                        .saturating_sub(std::mem::size_of::<MoltHeader>());
                    let slots = payload / std::mem::size_of::<u64>();
                    let payload_ptr = ptr as *mut u64;
                    for idx in 0..slots {
                        dec_ref_bits(*payload_ptr.add(idx));
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
                let format_bits = memoryview_format_bits(ptr);
                let shape_ptr = memoryview_shape_ptr(ptr);
                let strides_ptr = memoryview_strides_ptr(ptr);
                dec_ref_bits(owner_bits);
                dec_ref_bits(format_bits);
                if !shape_ptr.is_null() {
                    drop(Box::from_raw(shape_ptr));
                }
                if !strides_ptr.is_null() {
                    drop(Box::from_raw(strides_ptr));
                }
            }
            TYPE_ID_BIGINT => {
                std::ptr::drop_in_place(ptr as *mut BigInt);
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
        if header.type_id == TYPE_ID_OBJECT
            && header.poll_fn == 0
            && object_pool_put(size, header_ptr as *mut u8)
        {
            return;
        }
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
pub extern "C" fn molt_handle_resolve(bits: u64) -> u64 {
    resolve_obj_ptr(bits).map_or(0, |ptr| ptr as u64)
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
    use std::sync::atomic::AtomicBool;

    static EXIT_CALLED: AtomicBool = AtomicBool::new(false);

    extern "C" fn test_enter(payload_bits: u64) -> u64 {
        payload_bits
    }

    extern "C" fn test_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
        EXIT_CALLED.store(true, AtomicOrdering::SeqCst);
        MoltObject::from_bool(false).bits()
    }

    #[test]
    fn context_unwind_runs_exit() {
        EXIT_CALLED.store(false, AtomicOrdering::SeqCst);
        let ctx_bits = molt_context_new(
            test_enter as *const (),
            test_exit as *const (),
            MoltObject::none().bits(),
        );
        let _ = molt_context_enter(ctx_bits);
        let _ = molt_context_unwind(MoltObject::none().bits());
        assert!(EXIT_CALLED.load(AtomicOrdering::SeqCst));
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
pub unsafe extern "C" fn molt_json_parse_int(ptr_bits: u64, len_bits: u64) -> i64 {
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
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
pub unsafe extern "C" fn molt_string_from_bytes(
    ptr_bits: u64,
    len_bits: u64,
    out_bits: u64,
) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if ptr.is_null() && len != 0 {
        return 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    if std::str::from_utf8(slice).is_err() {
        return 1;
    }
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
pub unsafe extern "C" fn molt_bytes_from_bytes(ptr_bits: u64, len_bits: u64, out_bits: u64) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
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
pub unsafe extern "C" fn molt_json_parse_scalar(
    ptr_bits: u64,
    len_bits: u64,
    out_bits: u64,
) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
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
    ptr_bits: u64,
    len_bits: u64,
    out_bits: u64,
) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
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
pub unsafe extern "C" fn molt_cbor_parse_scalar(
    ptr_bits: u64,
    len_bits: u64,
    out_bits: u64,
) -> i32 {
    let out = ptr_from_bits(out_bits) as *mut u64;
    let ptr = ptr_from_const_bits(ptr_bits);
    let len = usize_from_bits(len_bits);
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

fn property_no_deleter(attr_name: &str, class_ptr: *mut u8) -> i64 {
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no deleter");
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

fn descriptor_no_deleter(attr_name: &str, class_ptr: *mut u8) -> i64 {
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
    obj.as_ptr()
}

#[inline]
fn usize_from_bits(bits: u64) -> usize {
    debug_assert!(bits <= usize::MAX as u64);
    bits as usize
}

#[inline]
fn ptr_from_bits(bits: u64) -> *mut u8 {
    bits as usize as *mut u8
}

#[inline]
fn ptr_from_const_bits(bits: u64) -> *const u8 {
    bits as usize as *const u8
}

#[inline]
fn bits_from_ptr(ptr: *mut u8) -> u64 {
    ptr as usize as u64
}

#[inline]
fn bits_from_const_ptr(ptr: *const u8) -> u64 {
    ptr as usize as u64
}

unsafe fn call_function_obj1(func_bits: u64, arg0_bits: u64) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
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
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(closure_bits, arg0_bits) as u64
    } else {
        let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(arg0_bits) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj0(func_bits: u64) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
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
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(closure_bits) as u64
    } else {
        let func: extern "C" fn() -> i64 = std::mem::transmute(fn_ptr as usize);
        func() as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj2(func_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
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
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(closure_bits, arg0_bits, arg1_bits) as u64
    } else {
        let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(arg0_bits, arg1_bits) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj3(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
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
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(closure_bits, arg0_bits, arg1_bits, arg2_bits) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(arg0_bits, arg1_bits, arg2_bits) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj4(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 4 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(closure_bits, arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
        func(arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj5(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 5 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            closure_bits,
            arg0_bits,
            arg1_bits,
            arg2_bits,
            arg3_bits,
            arg4_bits,
        ) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj6(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 6 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            closure_bits,
            arg0_bits,
            arg1_bits,
            arg2_bits,
            arg3_bits,
            arg4_bits,
            arg5_bits,
        ) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
        ) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj7(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 7 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            closure_bits,
            arg0_bits,
            arg1_bits,
            arg2_bits,
            arg3_bits,
            arg4_bits,
            arg5_bits,
            arg6_bits,
        ) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
        ) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj8(
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
) -> u64 {
    profile_hit(&CALL_DISPATCH_COUNT);
    let func_obj = obj_from_bits(func_bits);
    let Some(func_ptr) = func_obj.as_ptr() else {
        raise!("TypeError", "call expects function object");
    };
    if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
        raise!("TypeError", "call expects function object");
    }
    let arity = function_arity(func_ptr);
    if arity != 8 {
        raise!("TypeError", "call arity mismatch");
    }
    let fn_ptr = function_fn_ptr(func_ptr);
    let closure_bits = function_closure_bits(func_ptr);
    let name_bits = function_name_bits(func_ptr);
    if !recursion_guard_enter() {
        raise!("RecursionError", "maximum recursion depth exceeded");
    }
    frame_stack_push(name_bits);
    let res = if closure_bits != 0 {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            closure_bits,
            arg0_bits,
            arg1_bits,
            arg2_bits,
            arg3_bits,
            arg4_bits,
            arg5_bits,
            arg6_bits,
            arg7_bits,
        ) as u64
    } else {
        let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
            std::mem::transmute(fn_ptr as usize);
        func(
            arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits, arg7_bits,
        ) as u64
    };
    frame_stack_pop();
    recursion_guard_exit();
    res
}

unsafe fn call_function_obj_vec(func_bits: u64, args: &[u64]) -> u64 {
    match args.len() {
        0 => call_function_obj0(func_bits),
        1 => call_function_obj1(func_bits, args[0]),
        2 => call_function_obj2(func_bits, args[0], args[1]),
        3 => call_function_obj3(func_bits, args[0], args[1], args[2]),
        4 => call_function_obj4(func_bits, args[0], args[1], args[2], args[3]),
        5 => call_function_obj5(func_bits, args[0], args[1], args[2], args[3], args[4]),
        6 => call_function_obj6(
            func_bits, args[0], args[1], args[2], args[3], args[4], args[5],
        ),
        7 => call_function_obj7(
            func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
        ),
        8 => call_function_obj8(
            func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
        ),
        _ => raise!("TypeError", "call arity mismatch"),
    }
}

unsafe fn lookup_call_attr(obj_ptr: *mut u8) -> Option<u64> {
    let call_name_bits = intern_static_name(&INTERN_CALL_NAME, b"__call__");
    attr_lookup_ptr(obj_ptr, call_name_bits)
}

unsafe fn class_layout_size(class_ptr: *mut u8) -> usize {
    let size_name_bits = intern_static_name(&INTERN_MOLT_LAYOUT_SIZE, b"__molt_layout_size__");
    if let Some(size_bits) = class_attr_lookup_raw_mro(class_ptr, size_name_bits) {
        if let Some(size) = obj_from_bits(size_bits).as_int() {
            if size > 0 {
                return size as usize;
            }
        }
    }
    8
}

unsafe fn alloc_instance_for_class(class_ptr: *mut u8) -> u64 {
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let size = class_layout_size(class_ptr);
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let obj_ptr = alloc_object_zeroed_with_pool(total_size, TYPE_ID_OBJECT);
    if obj_ptr.is_null() {
        return MoltObject::none().bits();
    }
    object_set_class_bits(obj_ptr, class_bits);
    inc_ref_bits(class_bits);
    MoltObject::from_ptr(obj_ptr).bits()
}

unsafe fn call_class_init_with_args(class_ptr: *mut u8, args: &[u64]) -> u64 {
    let inst_bits = alloc_instance_for_class(class_ptr);
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return inst_bits;
    };
    let init_name_bits = intern_static_name(&INTERN_INIT_NAME, b"__init__");
    let Some(init_bits) = class_attr_lookup(class_ptr, class_ptr, Some(inst_ptr), init_name_bits)
    else {
        return inst_bits;
    };
    let pos_capacity = MoltObject::from_int(args.len() as i64).bits();
    let builder_bits = molt_callargs_new(pos_capacity, MoltObject::from_int(0).bits());
    if builder_bits == 0 {
        return inst_bits;
    }
    for &arg in args {
        let _ = molt_callargs_push_pos(builder_bits, arg);
    }
    let _ = molt_call_bind(init_bits, builder_bits);
    inst_bits
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
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                raise!("TypeError", "object is not callable");
            };
            call_callable0(call_attr_bits)
        }
        _ => raise!("TypeError", "object is not callable"),
    }
}

unsafe fn call_callable1(call_bits: u64, arg0_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        raise!("TypeError", "object is not callable");
    };
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => call_function_obj1(call_bits, arg0_bits),
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            call_function_obj2(func_bits, self_bits, arg0_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[arg0_bits]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                raise!("TypeError", "object is not callable");
            };
            call_callable1(call_attr_bits, arg0_bits)
        }
        _ => raise!("TypeError", "object is not callable"),
    }
}

unsafe fn callable_arity(call_bits: u64) -> Option<usize> {
    let call_obj = obj_from_bits(call_bits);
    let call_ptr = call_obj.as_ptr()?;
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => Some(function_arity(call_ptr) as usize),
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let func_obj = obj_from_bits(func_bits);
            let func_ptr = func_obj.as_ptr()?;
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            Some(function_arity(func_ptr) as usize)
        }
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let call_attr_bits = lookup_call_attr(call_ptr)?;
            callable_arity(call_attr_bits)
        }
        _ => None,
    }
}

unsafe fn call_callable2(call_bits: u64, arg0_bits: u64, arg1_bits: u64) -> u64 {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        raise!("TypeError", "object is not callable");
    };
    match object_type_id(call_ptr) {
        TYPE_ID_FUNCTION => call_function_obj2(call_bits, arg0_bits, arg1_bits),
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            let self_bits = bound_method_self_bits(call_ptr);
            call_function_obj3(func_bits, self_bits, arg0_bits, arg1_bits)
        }
        TYPE_ID_TYPE => call_class_init_with_args(call_ptr, &[arg0_bits, arg1_bits]),
        TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
            let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                raise!("TypeError", "object is not callable");
            };
            call_callable2(call_attr_bits, arg0_bits, arg1_bits)
        }
        _ => raise!("TypeError", "object is not callable"),
    }
}

unsafe fn function_attr_bits(func_ptr: *mut u8, attr_bits: u64) -> Option<u64> {
    let dict_bits = function_dict_bits(func_ptr);
    if dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    dict_get_in_place(dict_ptr, attr_bits)
}

#[no_mangle]
pub extern "C" fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            raise!("TypeError", "object is not callable");
        };
        let mut func_bits = call_bits;
        let mut self_bits = None;
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {}
            TYPE_ID_BOUND_METHOD => {
                func_bits = bound_method_func_bits(call_ptr);
                self_bits = Some(bound_method_self_bits(call_ptr));
            }
            TYPE_ID_TYPE => {
                let inst_bits = alloc_instance_for_class(call_ptr);
                let init_name_bits = intern_static_name(&INTERN_INIT_NAME, b"__init__");
                let Some(init_bits) = class_attr_lookup_raw_mro(call_ptr, init_name_bits) else {
                    return inst_bits;
                };
                let builder_ptr = ptr_from_bits(builder_bits);
                if builder_ptr.is_null() {
                    return inst_bits;
                }
                let args_ptr = callargs_ptr(builder_ptr);
                if !args_ptr.is_null() {
                    (*args_ptr).pos.insert(0, inst_bits);
                }
                let _ = molt_call_bind(init_bits, builder_bits);
                return inst_bits;
            }
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let Some(call_attr_bits) = lookup_call_attr(call_ptr) else {
                    raise!("TypeError", "object is not callable");
                };
                return molt_call_bind(call_attr_bits, builder_bits);
            }
            _ => raise!("TypeError", "object is not callable"),
        }
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            raise!("TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            raise!("TypeError", "call expects function object");
        }
        let builder_ptr = builder_bits as *mut u8;
        if builder_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_ptr = callargs_ptr(builder_ptr);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        *(builder_ptr as *mut *mut CallArgs) = std::ptr::null_mut();
        let mut args = Box::from_raw(args_ptr);
        if let Some(self_bits) = self_bits {
            args.pos.insert(0, self_bits);
        }

        let arg_names_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_MOLT_ARG_NAMES, b"__molt_arg_names__"),
        );
        let arg_names = if let Some(bits) = arg_names_bits {
            let arg_names_ptr = obj_from_bits(bits).as_ptr();
            let Some(arg_names_ptr) = arg_names_ptr else {
                raise!("TypeError", "call expects function object");
            };
            if object_type_id(arg_names_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "call expects function object");
            }
            seq_vec_ref(arg_names_ptr).clone()
        } else {
            if let Some(bound_args) = bind_builtin_call(func_bits, func_ptr, &args) {
                return call_function_obj_vec(func_bits, bound_args.as_slice());
            }
            raise!("TypeError", "call expects function object");
        };

        let posonly_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_MOLT_POSONLY, b"__molt_posonly__"),
        )
        .unwrap_or_else(|| MoltObject::from_int(0).bits());
        let posonly = obj_from_bits(posonly_bits).as_int().unwrap_or(0).max(0) as usize;

        let kwonly_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_MOLT_KWONLY_NAMES, b"__molt_kwonly_names__"),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let mut kwonly_names: Vec<u64> = Vec::new();
        if !obj_from_bits(kwonly_bits).is_none() {
            let Some(kw_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                raise!("TypeError", "call expects function object");
            };
            if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "call expects function object");
            }
            kwonly_names = seq_vec_ref(kw_ptr).clone();
        }

        let vararg_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_MOLT_VARARG, b"__molt_vararg__"),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let varkw_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_MOLT_VARKW, b"__molt_varkw__"),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let has_vararg = !obj_from_bits(vararg_bits).is_none();
        let has_varkw = !obj_from_bits(varkw_bits).is_none();

        let defaults_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_DEFAULTS_NAME, b"__defaults__"),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let mut defaults: Vec<u64> = Vec::new();
        if !obj_from_bits(defaults_bits).is_none() {
            let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                raise!("TypeError", "call expects function object");
            };
            if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                raise!("TypeError", "call expects function object");
            }
            defaults = seq_vec_ref(def_ptr).clone();
        }

        let kwdefaults_bits = function_attr_bits(
            func_ptr,
            intern_static_name(&INTERN_KWDEFAULTS_NAME, b"__kwdefaults__"),
        )
        .unwrap_or_else(|| MoltObject::none().bits());
        let mut kwdefaults_ptr = None;
        if !obj_from_bits(kwdefaults_bits).is_none() {
            let Some(ptr) = obj_from_bits(kwdefaults_bits).as_ptr() else {
                raise!("TypeError", "call expects function object");
            };
            if object_type_id(ptr) != TYPE_ID_DICT {
                raise!("TypeError", "call expects function object");
            }
            kwdefaults_ptr = Some(ptr);
        }

        let total_pos = arg_names.len();
        let kwonly_start = total_pos + if has_vararg { 1 } else { 0 };
        let total_params = kwonly_start + kwonly_names.len() + if has_varkw { 1 } else { 0 };
        let mut slots: Vec<Option<u64>> = vec![None; total_params];
        let mut extra_pos: Vec<u64> = Vec::new();
        for (idx, val) in args.pos.iter().copied().enumerate() {
            if idx < total_pos {
                slots[idx] = Some(val);
            } else if has_vararg {
                extra_pos.push(val);
            } else {
                raise!("TypeError", "too many positional arguments");
            }
        }

        let mut extra_kwargs: Vec<u64> = Vec::new();
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let mut matched = false;
            for (idx, param_bits) in arg_names.iter().copied().enumerate() {
                if obj_eq(name_obj, obj_from_bits(param_bits)) {
                    if idx < posonly {
                        let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                        let msg =
                            format!("got positional-only argument '{name}' passed as keyword");
                        raise!("TypeError", &msg);
                    }
                    if slots[idx].is_some() {
                        let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                        let msg = format!("got multiple values for argument '{name}'");
                        raise!("TypeError", &msg);
                    }
                    slots[idx] = Some(val_bits);
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
            for (kw_idx, kw_name_bits) in kwonly_names.iter().copied().enumerate() {
                if obj_eq(name_obj, obj_from_bits(kw_name_bits)) {
                    let slot_idx = kwonly_start + kw_idx;
                    if slots[slot_idx].is_some() {
                        let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                        let msg = format!("got multiple values for argument '{name}'");
                        raise!("TypeError", &msg);
                    }
                    slots[slot_idx] = Some(val_bits);
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
            if has_varkw {
                extra_kwargs.push(name_bits);
                extra_kwargs.push(val_bits);
            } else {
                let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                let msg = format!("got an unexpected keyword '{name}'");
                raise!("TypeError", &msg);
            }
        }

        let defaults_len = defaults.len();
        let default_start = total_pos.saturating_sub(defaults_len);
        for idx in 0..total_pos {
            if slots[idx].is_some() {
                continue;
            }
            if idx >= default_start {
                slots[idx] = Some(defaults[idx - default_start]);
                continue;
            }
            let name = string_obj_to_owned(obj_from_bits(arg_names[idx]))
                .unwrap_or_else(|| "?".to_string());
            let msg = format!("missing required argument '{name}'");
            raise!("TypeError", &msg);
        }

        for (kw_idx, name_bits) in kwonly_names.iter().copied().enumerate() {
            let slot_idx = kwonly_start + kw_idx;
            if slots[slot_idx].is_some() {
                continue;
            }
            let mut default = None;
            if let Some(dict_ptr) = kwdefaults_ptr {
                default = dict_get_in_place(dict_ptr, name_bits);
            }
            if let Some(val) = default {
                slots[slot_idx] = Some(val);
                continue;
            }
            let name =
                string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "?".to_string());
            let msg = format!("missing required keyword-only argument '{name}'");
            raise!("TypeError", &msg);
        }

        if has_vararg {
            let tuple_ptr = alloc_tuple(extra_pos.as_slice());
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            slots[total_pos] = Some(MoltObject::from_ptr(tuple_ptr).bits());
        }

        if has_varkw {
            let dict_ptr = alloc_dict_with_pairs(extra_kwargs.as_slice());
            if dict_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let varkw_idx = kwonly_start + kwonly_names.len();
            slots[varkw_idx] = Some(MoltObject::from_ptr(dict_ptr).bits());
        }

        let mut final_args: Vec<u64> = Vec::with_capacity(slots.len());
        for slot in slots {
            let Some(val) = slot else {
                raise!("TypeError", "call binding failed");
            };
            final_args.push(val);
        }
        call_function_obj_vec(func_bits, final_args.as_slice())
    }
}

unsafe fn bind_builtin_call(
    func_bits: u64,
    func_ptr: *mut u8,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    let fn_ptr = function_fn_ptr(func_ptr);
    if fn_ptr == dict_get_method as usize as u64 {
        return bind_builtin_keywords(
            args,
            &["key", "default"],
            Some(MoltObject::none().bits()),
            None,
        );
    }
    if fn_ptr == dict_setdefault_method as usize as u64 {
        return bind_builtin_keywords(
            args,
            &["key", "default"],
            Some(MoltObject::none().bits()),
            None,
        );
    }
    if fn_ptr == dict_update_method as usize as u64 {
        return bind_builtin_keywords(args, &["other"], Some(missing_bits()), None);
    }
    if fn_ptr == dict_pop_method as usize as u64 {
        return bind_builtin_pop(args);
    }
    if fn_ptr == molt_list_sort as usize as u64 {
        return bind_builtin_list_sort(args);
    }

    if !args.kw_names.is_empty() {
        raise!("TypeError", "keywords are not supported for this builtin");
    }

    let mut out = args.pos.clone();
    let arity = function_arity(func_ptr) as usize;
    if out.len() > arity {
        raise!("TypeError", "too many positional arguments");
    }
    let missing = arity - out.len();
    if missing == 0 {
        return Some(out);
    }
    let default_kind = molt_function_default_kind(func_bits);
    if missing == 1 {
        if default_kind == FUNC_DEFAULT_NONE {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_DICT_POP {
            out.push(MoltObject::from_int(1).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_DICT_UPDATE {
            out.push(missing_bits());
            return Some(out);
        }
    }
    if missing == 2 && default_kind == FUNC_DEFAULT_DICT_POP {
        out.push(MoltObject::none().bits());
        out.push(MoltObject::from_int(0).bits());
        return Some(out);
    }
    raise!("TypeError", "missing required arguments");
}

unsafe fn bind_builtin_keywords(
    args: &CallArgs,
    names: &[&str],
    default_bits: Option<u64>,
    extra_bits: Option<u64>,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        raise!("TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut values: Vec<Option<u64>> = vec![None; names.len()];
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        let idx = pos_idx - 1;
        if idx >= names.len() {
            raise!("TypeError", "too many positional arguments");
        }
        values[idx] = Some(args.pos[pos_idx]);
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in names.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    raise!("TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            raise!("TypeError", &msg);
        }
    }
    for (idx, val) in values.iter_mut().enumerate() {
        if val.is_none() {
            if let Some(bits) = default_bits {
                *val = Some(bits);
                continue;
            }
            let name = names[idx];
            let msg = format!("missing required argument '{name}'");
            raise!("TypeError", &msg);
        }
    }
    for val in values.into_iter().flatten() {
        out.push(val);
    }
    if let Some(extra) = extra_bits {
        out.push(extra);
    }
    Some(out)
}

unsafe fn bind_builtin_list_sort(args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        raise!("TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        raise!("TypeError", "too many positional arguments");
    }
    let mut key_bits = MoltObject::none().bits();
    let mut reverse_bits = MoltObject::from_bool(false).bits();
    let mut key_set = false;
    let mut reverse_set = false;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "key" => {
                if key_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    raise!("TypeError", &msg);
                }
                key_bits = val_bits;
                key_set = true;
            }
            "reverse" => {
                if reverse_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    raise!("TypeError", &msg);
                }
                reverse_bits = val_bits;
                reverse_set = true;
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                raise!("TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], key_bits, reverse_bits])
}

unsafe fn bind_builtin_pop(args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        raise!("TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut key: Option<u64> = None;
    let mut default: Option<u64> = None;
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        if key.is_none() {
            key = Some(args.pos[pos_idx]);
        } else if default.is_none() {
            default = Some(args.pos[pos_idx]);
        } else {
            raise!("TypeError", "too many positional arguments");
        }
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str == "key" {
            if key.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                raise!("TypeError", &msg);
            }
            key = Some(val_bits);
        } else if name_str == "default" {
            if default.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                raise!("TypeError", &msg);
            }
            default = Some(val_bits);
        } else {
            let msg = format!("got an unexpected keyword '{name_str}'");
            raise!("TypeError", &msg);
        }
    }
    let Some(key_bits) = key else {
        raise!("TypeError", "missing required argument 'key'");
    };
    let (default_bits, has_default) = if let Some(bits) = default {
        (bits, MoltObject::from_int(1).bits())
    } else {
        (MoltObject::none().bits(), MoltObject::from_int(0).bits())
    };
    out.push(key_bits);
    out.push(default_bits);
    out.push(has_default);
    Some(out)
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
    MoltObject::from_ptr(ptr).bits()
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
        let next_bits = bases.first().copied()?;
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

unsafe fn class_field_offset(class_ptr: *mut u8, attr_bits: u64) -> Option<usize> {
    let dict_bits = class_dict_bits(class_ptr);
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    let fields_bits = intern_static_name(&INTERN_FIELD_OFFSETS_NAME, b"__molt_field_offsets__");
    let offsets_bits = dict_get_in_place(dict_ptr, fields_bits)?;
    let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
    if object_type_id(offsets_ptr) != TYPE_ID_DICT {
        return None;
    }
    let offset_bits = dict_get_in_place(offsets_ptr, attr_bits)?;
    obj_from_bits(offset_bits)
        .as_int()
        .and_then(|val| if val >= 0 { Some(val as usize) } else { None })
}

fn attr_name_bits_from_bytes(slice: &[u8]) -> Option<u64> {
    if let Some(bits) = ATTR_NAME_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| entry.bytes == slice)
            .map(|entry| entry.bits)
    }) {
        inc_ref_bits(bits);
        return Some(bits);
    }
    let ptr = alloc_string(slice);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    ATTR_NAME_TLS.with(|cell| {
        let mut entry = cell.borrow_mut();
        if let Some(prev) = entry.take() {
            dec_ref_bits(prev.bits);
        }
        inc_ref_bits(bits);
        *entry = Some(AttrNameCacheEntry {
            bytes: slice.to_vec(),
            bits,
        });
    });
    Some(bits)
}

fn descriptor_cache_lookup(
    class_bits: u64,
    attr_bits: u64,
    version: u64,
) -> Option<DescriptorCacheEntry> {
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| {
                entry.class_bits == class_bits
                    && entry.attr_bits == attr_bits
                    && entry.version == version
            })
            .cloned()
    })
}

fn descriptor_cache_store(entry: DescriptorCacheEntry) {
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        *cell.borrow_mut() = Some(entry);
    });
}

unsafe fn descriptor_method_bits(val_bits: u64, name_bits: u64) -> Option<u64> {
    let class_bits = if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => MoltObject::from_ptr(ptr).bits(),
                TYPE_ID_OBJECT => object_class_bits(ptr),
                _ => type_of_bits(val_bits),
            }
        }
    } else {
        type_of_bits(val_bits)
    };
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
    let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
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
    let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
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
    profile_hit(&ATTR_LOOKUP_COUNT);
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
            "__traceback__" => {
                let bits = exception_trace_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            _ => {}
        }
    }
    if type_id == TYPE_ID_MEMORYVIEW {
        let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
        match name.as_str() {
            "format" => {
                let bits = memoryview_format_bits(obj_ptr);
                inc_ref_bits(bits);
                return Some(bits);
            }
            "itemsize" => {
                return Some(MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits());
            }
            "ndim" => {
                return Some(MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits());
            }
            "shape" => {
                let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(shape));
            }
            "strides" => {
                let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                return Some(tuple_from_isize_slice(strides));
            }
            "readonly" => {
                return Some(MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits());
            }
            "nbytes" => {
                return Some(MoltObject::from_int(memoryview_nbytes(obj_ptr) as i64).bits());
            }
            _ => {}
        }
    }
    if type_id == TYPE_ID_DICT {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = dict_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
    }
    if type_id == TYPE_ID_LIST {
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if let Some(func_bits) = list_method_bits(name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
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
        let target_bits = super_obj_bits(obj_ptr);
        let target_ptr = maybe_ptr_from_bits(target_bits);
        let obj_type_bits = if let Some(raw_ptr) = target_ptr {
            if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                target_bits
            } else {
                type_of_bits(target_bits)
            }
        } else {
            type_of_bits(target_bits)
        };
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
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
        if let Some(raw_ptr) = target_ptr {
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
                        let getattribute_bits =
                            intern_static_name(&INTERN_GETATTRIBUTE_NAME, b"__getattribute__");
                        if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattribute_bits)) {
                            if let Some(call_bits) = class_attr_lookup(
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattribute_bits,
                            ) {
                                let res_bits = call_callable1(call_bits, attr_bits);
                                return Some(res_bits);
                            }
                        }
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
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattr_bits = intern_static_name(&INTERN_GETATTR_NAME, b"__getattr__");
                        if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                            if let Some(call_bits) =
                                class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
                            {
                                let res_bits = call_callable1(call_bits, attr_bits);
                                return Some(res_bits);
                            }
                        }
                    }
                }
            }
        }
        return None;
    }
    if type_id == TYPE_ID_OBJECT {
        let class_bits = object_class_bits(obj_ptr);
        let mut cached_attr_bits: Option<u64> = None;
        let mut class_version = 0u64;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattribute_bits =
                        intern_static_name(&INTERN_GETATTRIBUTE_NAME, b"__getattribute__");
                    if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattribute_bits)) {
                        if let Some(call_bits) = class_attr_lookup(
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            getattribute_bits,
                        ) {
                            let res_bits = call_callable1(call_bits, attr_bits);
                            return Some(res_bits);
                        }
                    }
                    class_version = class_layout_version_bits(class_ptr);
                    if let Some(entry) =
                        descriptor_cache_lookup(class_bits, attr_bits, class_version)
                    {
                        if let Some(bits) = entry.data_desc_bits {
                            return descriptor_bind(bits, class_ptr, Some(obj_ptr));
                        }
                        cached_attr_bits = entry.class_attr_bits;
                    }
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if descriptor_is_data(val_bits) {
                                descriptor_cache_store(DescriptorCacheEntry {
                                    class_bits,
                                    attr_bits,
                                    version: class_version,
                                    data_desc_bits: Some(val_bits),
                                    class_attr_bits: None,
                                });
                                return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                            }
                            cached_attr_bits = Some(val_bits);
                            descriptor_cache_store(DescriptorCacheEntry {
                                class_bits,
                                attr_bits,
                                version: class_version,
                                data_desc_bits: None,
                                class_attr_bits: Some(val_bits),
                            });
                        } else {
                            descriptor_cache_store(DescriptorCacheEntry {
                                class_bits,
                                attr_bits,
                                version: class_version,
                                data_desc_bits: None,
                                class_attr_bits: None,
                            });
                        }
                    }
                    if let Some(offset) = class_field_offset(class_ptr, attr_bits) {
                        let bits = object_field_get_ptr_raw(obj_ptr, offset);
                        return Some(bits);
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
                    if cached_attr_bits.is_none() {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            cached_attr_bits = Some(val_bits);
                            descriptor_cache_store(DescriptorCacheEntry {
                                class_bits,
                                attr_bits,
                                version: class_version,
                                data_desc_bits: None,
                                class_attr_bits: Some(val_bits),
                            });
                        }
                    }
                    if let Some(val_bits) = cached_attr_bits {
                        return descriptor_bind(val_bits, class_ptr, Some(obj_ptr));
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let getattr_bits = intern_static_name(&INTERN_GETATTR_NAME, b"__getattr__");
                    if !obj_eq(obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                        if let Some(call_bits) =
                            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
                        {
                            let res_bits = call_callable1(call_bits, attr_bits);
                            return Some(res_bits);
                        }
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
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
        return MoltObject::none().bits() as i64;
    };
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
pub unsafe extern "C" fn molt_get_attr_ptr(
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    molt_get_attr_generic(obj_ptr_bits, attr_name_bits, attr_name_len_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
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
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(dict_ptr, attr_bits, val_bits);
                class_bump_layout_version(obj_ptr);
                dec_ref_bits(attr_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        dec_ref_bits(attr_bits);
        return attr_error("type", attr_name);
    }
    if type_id == TYPE_ID_EXCEPTION {
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
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
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
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
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let setattr_bits = intern_static_name(&INTERN_SETATTR_NAME, b"__setattr__");
                        if let Some(call_bits) =
                            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), setattr_bits)
                        {
                            let _ = call_callable2(call_bits, attr_bits, val_bits);
                            dec_ref_bits(attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
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
        let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
            return MoltObject::none().bits() as i64;
        };
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    let setattr_bits = intern_static_name(&INTERN_SETATTR_NAME, b"__setattr__");
                    if let Some(call_bits) =
                        class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), setattr_bits)
                    {
                        let _ = call_callable2(call_bits, attr_bits, val_bits);
                        dec_ref_bits(attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
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
                    if let Some(offset) = class_field_offset(class_ptr, attr_bits) {
                        dec_ref_bits(attr_bits);
                        return object_field_set_ptr_raw(obj_ptr, offset, val_bits) as i64;
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

unsafe fn del_attr_ptr(obj_ptr: *mut u8, attr_bits: u64, attr_name: &str) -> i64 {
    let type_id = object_type_id(obj_ptr);
    if type_id == TYPE_ID_MODULE {
        let dict_bits = module_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT && dict_del_in_place(dict_ptr, attr_bits) {
                return MoltObject::none().bits() as i64;
            }
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        raise!("AttributeError", &msg);
    }
    if type_id == TYPE_ID_TYPE {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(class_bits) {
            raise!("TypeError", "cannot delete attributes on builtin type");
        }
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT && dict_del_in_place(dict_ptr, attr_bits) {
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
        }
        let class_name =
            string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
        raise!("AttributeError", &msg);
    }
    if type_id == TYPE_ID_EXCEPTION {
        if attr_name == "__cause__" || attr_name == "__context__" {
            unsafe {
                let slot = if attr_name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if !obj_from_bits(old_bits).is_none() {
                    dec_ref_bits(old_bits);
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(none_bits);
                    *slot = none_bits;
                }
                if attr_name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(false).bits();
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
        if attr_name == "__suppress_context__" {
            unsafe {
                let suppress_bits = MoltObject::from_bool(false).bits();
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
        let dict_bits = function_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error("function", attr_name);
    }
    if type_id == TYPE_ID_DATACLASS {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() && (*desc_ptr).frozen {
            raise!("TypeError", "cannot delete frozen dataclass field");
        }
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let del_bits = property_del_bits(desc_ptr);
                                    if obj_from_bits(del_bits).is_none() {
                                        return property_no_deleter(attr_name, class_ptr);
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj1(del_bits, inst_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let del_bits = intern_static_name(&INTERN_DELETE_NAME, b"__delete__");
                            if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                                let method_obj = obj_from_bits(method_bits);
                                let Some(method_ptr) = method_obj.as_ptr() else {
                                    raise!("TypeError", "__delete__ must be a function");
                                };
                                if object_type_id(method_ptr) != TYPE_ID_FUNCTION {
                                    raise!("TypeError", "__delete__ must be a function");
                                }
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj2(method_bits, self_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits = intern_static_name(&INTERN_SET_NAME, b"__set__");
                            if descriptor_method_bits(desc_bits, set_bits).is_some() {
                                return descriptor_no_deleter(attr_name, class_ptr);
                            }
                        }
                    }
                }
            }
            if (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error(type_label, attr_name);
            }
        }
        let dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
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
    if type_id == TYPE_ID_OBJECT {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error("object", attr_name);
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error("object", attr_name);
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(attr_name, class_ptr);
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(&INTERN_DELETE_NAME, b"__delete__");
                        if let Some(method_bits) = descriptor_method_bits(desc_bits, del_bits) {
                            let method_obj = obj_from_bits(method_bits);
                            let Some(method_ptr) = method_obj.as_ptr() else {
                                raise!("TypeError", "__delete__ must be a function");
                            };
                            if object_type_id(method_ptr) != TYPE_ID_FUNCTION {
                                raise!("TypeError", "__delete__ must be a function");
                            }
                            let self_bits = desc_bits;
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let _ = call_function_obj2(method_bits, self_bits, inst_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(&INTERN_SET_NAME, b"__set__");
                        if descriptor_method_bits(desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(attr_name, class_ptr);
                        }
                    }
                }
            }
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        return attr_error("object", attr_name);
    }
    attr_error(type_name(MoltObject::from_ptr(obj_ptr)), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    molt_set_attr_generic(obj_ptr_bits, attr_name_bits, attr_name_len_bits, val_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    let obj_ptr = ptr_from_bits(obj_ptr_bits);
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if obj_ptr.is_null() {
        raise!("AttributeError", "object has no attribute");
    }
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(attr_bits) = attr_name_bits_from_bytes(slice) else {
        return MoltObject::none().bits() as i64;
    };
    let res = del_attr_ptr(obj_ptr, attr_bits, attr_name);
    dec_ref_bits(attr_bits);
    res
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    molt_del_attr_generic(obj_ptr_bits, attr_name_bits, attr_name_len_bits)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_get_attr_generic(bits_from_ptr(ptr), attr_name_bits, attr_name_len_bits);
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_special(
    obj_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
        return attr_error(type_name(obj), attr_name);
    };
    let name_ptr = alloc_string(slice);
    if name_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_bits = object_class_bits(obj_ptr);
    let class_ptr = obj_from_bits(class_bits).as_ptr();
    let res = if let Some(class_ptr) = class_ptr {
        if object_type_id(class_ptr) == TYPE_ID_TYPE {
            class_attr_lookup(class_ptr, class_ptr, Some(obj_ptr), name_bits)
        } else {
            None
        }
    } else {
        None
    };
    dec_ref_bits(name_bits);
    if let Some(bits) = res {
        return bits as i64;
    }
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_set_attr_generic(
            bits_from_ptr(ptr),
            attr_name_bits,
            attr_name_len_bits,
            val_bits,
        );
    }
    let obj = obj_from_bits(obj_bits);
    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
    let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
    attr_error(type_name(obj), attr_name)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_bits: u64,
    attr_name_len_bits: u64,
) -> i64 {
    let attr_name_ptr = ptr_from_const_bits(attr_name_bits);
    let attr_name_len = usize_from_bits(attr_name_len_bits);
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        return molt_del_attr_generic(bits_from_ptr(ptr), attr_name_bits, attr_name_len_bits);
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
            return molt_set_attr_generic(
                bits_from_ptr(obj_ptr),
                bits_from_const_ptr(bytes),
                len as u64,
                val_bits,
            ) as u64;
        }
    }
    let obj = obj_from_bits(obj_bits);
    let name =
        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
    attr_error(type_name(obj), &name) as u64
}

#[no_mangle]
pub extern "C" fn molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
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
            return del_attr_ptr(obj_ptr, name_bits, &attr_name) as u64;
        }
        let obj = obj_from_bits(obj_bits);
        attr_error(type_name(obj), &attr_name) as u64
    }
}
mod arena;
use arena::TempArena;
