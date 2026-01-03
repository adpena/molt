//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use crossbeam_deque::{Injector, Stealer, Worker};
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Cursor;
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
}

struct Buffer2D {
    rows: usize,
    cols: usize,
    data: Vec<i64>,
}

const TYPE_ID_STRING: u32 = 200;
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
const MAX_SMALL_LIST: usize = 16;
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

thread_local! {
    static PARSE_ARENA: RefCell<TempArena> = RefCell::new(TempArena::new(8 * 1024));
}

static LAST_EXCEPTION: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

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
            if type_id == TYPE_ID_LIST {
                return list_len(ptr) > 0;
            }
            if type_id == TYPE_ID_TUPLE {
                return tuple_len(ptr) > 0;
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
            if type_id == TYPE_ID_SLICE {
                return true;
            }
            if type_id == TYPE_ID_DATACLASS {
                return true;
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return true;
            }
        }
    }
    false
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
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        unsafe { molt_inc_ref(ptr) };
    }
}

fn dec_ref_bits(bits: u64) {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        unsafe { molt_dec_ref(ptr) };
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

unsafe fn string_len(ptr: *mut u8) -> usize {
    *(ptr as *const usize)
}

unsafe fn string_bytes(ptr: *mut u8) -> *const u8 {
    ptr.add(std::mem::size_of::<usize>())
}

unsafe fn bytes_len(ptr: *mut u8) -> usize {
    string_len(ptr)
}

unsafe fn bytes_data(ptr: *mut u8) -> *const u8 {
    string_bytes(ptr)
}

unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    let type_id = object_type_id(ptr);
    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        let len = bytes_len(ptr);
        let data = bytes_data(ptr);
        return Some(std::slice::from_raw_parts(data, len));
    }
    None
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

unsafe fn context_enter_fn(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn context_exit_fn(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

unsafe fn context_payload_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
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

unsafe fn buffer2d_ptr(ptr: *mut u8) -> *mut Buffer2D {
    *(ptr as *const *mut Buffer2D)
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

fn alloc_bytes(bytes: &[u8]) -> *mut u8 {
    alloc_bytes_like(bytes, TYPE_ID_BYTES)
}

fn alloc_bytearray(bytes: &[u8]) -> *mut u8 {
    alloc_bytes_like(bytes, TYPE_ID_BYTEARRAY)
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
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
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
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
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
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
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
    }
    ptr
}

fn alloc_exception_obj(kind_bits: u64, msg_bits: u64) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
    let ptr = alloc_object(total, TYPE_ID_EXCEPTION);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        *(ptr as *mut u64) = kind_bits;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = msg_bits;
        inc_ref_bits(kind_bits);
        inc_ref_bits(msg_bits);
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

extern "C" fn context_null_enter(payload_bits: u64) -> u64 {
    inc_ref_bits(payload_bits);
    payload_bits
}

extern "C" fn context_null_exit(_payload_bits: u64, _exc_bits: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

fn record_exception(ptr: *mut u8) {
    let cell = LAST_EXCEPTION.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if let Some(old_ptr) = guard.take() {
        let old_bits = MoltObject::from_ptr(old_ptr as *mut u8).bits();
        dec_ref_bits(old_bits);
    }
    *guard = Some(ptr as usize);
    let new_bits = MoltObject::from_ptr(ptr).bits();
    inc_ref_bits(new_bits);
}

fn raise_exception(kind: &str, message: &str) -> ! {
    let ptr = alloc_exception(kind, message);
    if !ptr.is_null() {
        record_exception(ptr);
    }
    eprintln!("{kind}: {message}");
    std::process::exit(1);
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
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        let header = ptr as *mut MoltHeader;
        (*header).type_id = 100;
        (*header).ref_count = 1;
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        ptr.add(std::mem::size_of::<MoltHeader>())
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
        raise_exception("ValueError", "range() arg 3 must not be zero");
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
        None => raise_exception("TypeError", "dataclass name must be a str"),
    };
    let field_names_obj = obj_from_bits(field_names_bits);
    let field_names = match decode_string_list(field_names_obj) {
        Some(val) => val,
        None => raise_exception(
            "TypeError",
            "dataclass field names must be a list/tuple of str",
        ),
    };
    let values_obj = obj_from_bits(values_bits);
    let values = match decode_value_list(values_obj) {
        Some(val) => val,
        None => raise_exception("TypeError", "dataclass values must be a list/tuple"),
    };
    if field_names.len() != values.len() {
        raise_exception("TypeError", "dataclass constructor argument mismatch");
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
    let frozen = (flags & 0x1) != 0;
    let eq = (flags & 0x2) != 0;
    let repr = (flags & 0x4) != 0;
    let desc = Box::new(DataclassDesc {
        name,
        field_names,
        frozen,
        eq,
        repr,
    });
    let desc_ptr = Box::into_raw(desc);

    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
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
        None => raise_exception("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let fields = dataclass_fields_ref(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                raise_exception("TypeError", "dataclass field index out of range");
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
        None => raise_exception("TypeError", "dataclass field index must be int"),
    };
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DATACLASS {
                return MoltObject::none().bits();
            }
            let desc_ptr = dataclass_desc_ptr(ptr);
            if !desc_ptr.is_null() && (*desc_ptr).frozen {
                raise_exception("TypeError", "cannot assign to frozen dataclass field");
            }
            let fields = dataclass_fields_mut(ptr);
            if idx < 0 || idx as usize >= fields.len() {
                raise_exception("TypeError", "dataclass field index out of range");
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
pub extern "C" fn molt_exception_new(kind_bits: u64, msg_bits: u64) -> u64 {
    let kind_obj = obj_from_bits(kind_bits);
    let msg_obj = obj_from_bits(msg_bits);
    if let Some(ptr) = kind_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                raise_exception("TypeError", "exception kind must be a str");
            }
        }
    } else {
        raise_exception("TypeError", "exception kind must be a str");
    }
    if let Some(ptr) = msg_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                raise_exception("TypeError", "exception message must be a str");
            }
        }
    } else {
        raise_exception("TypeError", "exception message must be a str");
    }
    let ptr = alloc_exception_obj(kind_bits, msg_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_kind(exc_bits: u64) -> u64 {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise_exception("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise_exception("TypeError", "expected exception object");
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
        raise_exception("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise_exception("TypeError", "expected exception object");
        }
        let bits = exception_msg_bits(ptr);
        inc_ref_bits(bits);
        bits
    }
}

#[no_mangle]
pub extern "C" fn molt_exception_set_last(exc_bits: u64) {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(ptr) = exc_obj.as_ptr() else {
        raise_exception("TypeError", "expected exception object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            raise_exception("TypeError", "expected exception object");
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
pub extern "C" fn molt_context_new(enter_fn: u64, exit_fn: u64, payload_bits: u64) -> u64 {
    if enter_fn == 0 || exit_fn == 0 {
        raise_exception("TypeError", "context manager hooks must be non-null");
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
        raise_exception("TypeError", "context manager must be an object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_CONTEXT_MANAGER {
            raise_exception("TypeError", "context manager protocol not supported");
        }
        let enter_fn_addr = context_enter_fn(ptr);
        if enter_fn_addr == 0 {
            raise_exception("TypeError", "context manager missing __enter__");
        }
        let enter_fn =
            std::mem::transmute::<usize, extern "C" fn(u64) -> u64>(enter_fn_addr as usize);
        enter_fn(context_payload_bits(ptr))
    }
}

#[no_mangle]
pub extern "C" fn molt_context_exit(ctx_bits: u64, exc_bits: u64) -> u64 {
    let ctx_obj = obj_from_bits(ctx_bits);
    let Some(ptr) = ctx_obj.as_ptr() else {
        raise_exception("TypeError", "context manager must be an object");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_CONTEXT_MANAGER {
            raise_exception("TypeError", "context manager protocol not supported");
        }
        let exit_fn_addr = context_exit_fn(ptr);
        if exit_fn_addr == 0 {
            raise_exception("TypeError", "context manager missing __exit__");
        }
        let exit_fn =
            std::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(exit_fn_addr as usize);
        exit_fn(context_payload_bits(ptr), exc_bits)
    }
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
pub extern "C" fn molt_bridge_unavailable(msg_bits: u64) -> u64 {
    let msg = format_obj_str(obj_from_bits(msg_bits));
    eprintln!("Molt bridge unavailable: {msg}");
    std::process::exit(1);
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_new(rows_bits: u64, cols_bits: u64, init_bits: u64) -> u64 {
    let rows = match to_i64(obj_from_bits(rows_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise_exception("TypeError", "rows must be a non-negative int"),
    };
    let cols = match to_i64(obj_from_bits(cols_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise_exception("TypeError", "cols must be a non-negative int"),
    };
    let init = match obj_from_bits(init_bits).as_int() {
        Some(val) => val,
        None => raise_exception("TypeError", "init must be an int"),
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
        _ => raise_exception("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise_exception("TypeError", "col must be a non-negative int"),
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
                raise_exception("IndexError", "buffer2d index out of range");
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
        _ => raise_exception("TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => raise_exception("TypeError", "col must be a non-negative int"),
    };
    let val = match obj_from_bits(val_bits).as_int() {
        Some(v) => v,
        None => raise_exception("TypeError", "value must be an int"),
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
                raise_exception("IndexError", "buffer2d index out of range");
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
        _ => raise_exception("TypeError", "matmul expects buffer2d operands"),
    };
    unsafe {
        if object_type_id(a_ptr) != TYPE_ID_BUFFER2D || object_type_id(b_ptr) != TYPE_ID_BUFFER2D {
            raise_exception("TypeError", "matmul expects buffer2d operands");
        }
        let a_buf = buffer2d_ptr(a_ptr);
        let b_buf = buffer2d_ptr(b_ptr);
        if a_buf.is_null() || b_buf.is_null() {
            return MoltObject::none().bits();
        }
        let a_buf = &*a_buf;
        let b_buf = &*b_buf;
        if a_buf.cols != b_buf.rows {
            raise_exception("ValueError", "matmul dimension mismatch");
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
pub extern "C" fn molt_chan_new() -> *mut u8 {
    let (s, r) = unbounded();
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
    static mut CALLED: bool = false;
    if !CALLED {
        CALLED = true;
        pending_bits_i64()
    } else {
        CALLED = false;
        0
    }
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
                let mut combined = Vec::with_capacity(l_len + r_len);
                combined.extend_from_slice(l_bytes);
                combined.extend_from_slice(r_bytes);
                let ptr = alloc_string(&combined);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTES {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                let mut combined = Vec::with_capacity(l_len + r_len);
                combined.extend_from_slice(l_bytes);
                combined.extend_from_slice(r_bytes);
                let ptr = alloc_bytes(&combined);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                let mut combined = Vec::with_capacity(l_len + r_len);
                combined.extend_from_slice(l_bytes);
                combined.extend_from_slice(r_bytes);
                let ptr = alloc_bytearray(&combined);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
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
        None => raise_exception("TypeError", "guard type tag must be int"),
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
        _ => false,
    };
    if !matches {
        raise_exception("TypeError", "type guard mismatch");
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

enum SliceError {
    Type,
    Value,
}

fn slice_error(err: SliceError) -> u64 {
    match err {
        SliceError::Type => {
            raise_exception("TypeError", "slice indices must be integers or None");
        }
        SliceError::Value => {
            raise_exception("ValueError", "slice step cannot be zero");
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
            if type_id == TYPE_ID_LIST {
                return MoltObject::from_int(list_len(ptr) as i64).bits();
            }
            if type_id == TYPE_ID_TUPLE {
                return MoltObject::from_int(tuple_len(ptr) as i64).bits();
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
            if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                let idx = bytes_find_impl(hay_bytes, needle_bytes);
                return MoltObject::from_int(idx).bits();
            }
            let hay_str = std::str::from_utf8_unchecked(hay_bytes);
            let needle_str = std::str::from_utf8_unchecked(needle_bytes);
            let idx = match hay_str.find(needle_str) {
                Some(byte_idx) => hay_str[..byte_idx].chars().count() as i64,
                None => -1,
            };
            return MoltObject::from_int(idx).bits();
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
                raise_exception("TypeError", "startswith expects str arguments");
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
                raise_exception("TypeError", "endswith expects str arguments");
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
                raise_exception("TypeError", "count expects str arguments");
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            if needle_bytes.is_empty() {
                let hay_str = std::str::from_utf8_unchecked(hay_bytes);
                let count = hay_str.chars().count() as i64 + 1;
                return MoltObject::from_int(count).bits();
            }
            let count = if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                bytes_count_impl(hay_bytes, needle_bytes)
            } else {
                let hay_str = std::str::from_utf8_unchecked(hay_bytes);
                let needle_str = std::str::from_utf8_unchecked(needle_bytes);
                hay_str.matches(needle_str).count() as i64
            };
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
            raise_exception("TypeError", "join expects a str separator");
        }
        let elems = match items.as_ptr() {
            Some(ptr) => {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    seq_vec_ref(ptr)
                } else {
                    raise_exception("TypeError", "join expects a list or tuple of str");
                }
            }
            None => raise_exception("TypeError", "join expects a list or tuple of str"),
        };
        let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
        let mut total_len = 0usize;
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let elem_ptr = match elem_obj.as_ptr() {
                Some(ptr) => ptr,
                None => raise_exception("TypeError", "join expects a list or tuple of str"),
            };
            if object_type_id(elem_ptr) != TYPE_ID_STRING {
                raise_exception("TypeError", "join expects a list or tuple of str");
            }
            total_len += string_len(elem_ptr);
        }
        if !elems.is_empty() {
            total_len = total_len.saturating_add(sep_bytes.len() * (elems.len() - 1));
        }
        let mut out = Vec::with_capacity(total_len);
        for (idx, &elem_bits) in elems.iter().enumerate() {
            if idx > 0 {
                out.extend_from_slice(sep_bytes);
            }
            let elem_ptr = obj_from_bits(elem_bits).as_ptr().unwrap();
            let elem_bytes =
                std::slice::from_raw_parts(string_bytes(elem_ptr), string_len(elem_ptr));
            out.extend_from_slice(elem_bytes);
        }
        let out_ptr = alloc_string(&out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    let spec_obj = obj_from_bits(spec_bits);
    let spec_ptr = match spec_obj.as_ptr() {
        Some(ptr) => ptr,
        None => raise_exception("TypeError", "format spec must be a str"),
    };
    unsafe {
        if object_type_id(spec_ptr) != TYPE_ID_STRING {
            raise_exception("TypeError", "format spec must be a str");
        }
        let spec_bytes = std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
        let spec_text = match std::str::from_utf8(spec_bytes) {
            Ok(val) => val,
            Err(_) => raise_exception("ValueError", "format spec must be valid UTF-8"),
        };
        let spec = match parse_format_spec(spec_text) {
            Ok(val) => val,
            Err(msg) => raise_exception("ValueError", msg),
        };
        let obj = obj_from_bits(val_bits);
        let rendered = match format_with_spec(obj, &spec) {
            Ok(val) => val,
            Err((kind, msg)) => raise_exception(kind, msg),
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
    memmem::find(hay_bytes, needle_bytes)
        .map(|v| v as i64)
        .unwrap_or(-1)
}

fn bytes_count_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> i64 {
    if needle_bytes.is_empty() {
        return hay_bytes.len() as i64 + 1;
    }
    if needle_bytes.len() == 1 {
        return hay_bytes.iter().filter(|b| **b == needle_bytes[0]).count() as i64;
    }
    let finder = memmem::Finder::new(needle_bytes);
    let mut count = 0i64;
    let mut start = 0usize;
    while start <= hay_bytes.len() {
        if let Some(idx) = finder.find(&hay_bytes[start..]) {
            count += 1;
            start += idx + needle_bytes.len();
        } else {
            break;
        }
    }
    count
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

fn split_bytes_impl(hay: &[u8], needle: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let finder = memmem::Finder::new(needle);
    for idx in finder.find_iter(hay) {
        parts.push(hay[start..idx].to_vec());
        start = idx + needle.len();
    }
    parts.push(hay[start..].to_vec());
    Some(parts)
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

fn split_string_impl(hay_bytes: &[u8], needle_bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    if needle_bytes.is_empty() {
        return None;
    }
    if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
        return split_bytes_impl(hay_bytes, needle_bytes);
    }
    let hay_str = unsafe { std::str::from_utf8_unchecked(hay_bytes) };
    let needle_str = unsafe { std::str::from_utf8_unchecked(needle_bytes) };
    let mut parts = Vec::new();
    for part in hay_str.split(needle_str) {
        parts.push(part.as_bytes().to_vec());
    }
    Some(parts)
}

fn replace_string_impl(
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
        let mut out = String::with_capacity(
            hay_str.len() + replacement_str.len() * (hay_str.chars().count() + 1),
        );
        out.push_str(replacement_str);
        for ch in hay_str.chars() {
            out.push(ch);
            out.push_str(replacement_str);
        }
        return Some(out.into_bytes());
    }
    if hay_bytes.is_ascii() && needle_bytes.is_ascii() && replacement_bytes.is_ascii() {
        return replace_bytes_impl(hay_bytes, needle_bytes, replacement_bytes);
    }
    let hay_str = unsafe { std::str::from_utf8_unchecked(hay_bytes) };
    let needle_str = unsafe { std::str::from_utf8_unchecked(needle_bytes) };
    let replacement_str = unsafe { std::str::from_utf8_unchecked(replacement_bytes) };
    Some(hay_str.replace(needle_str, replacement_str).into_bytes())
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
                raise_exception("ValueError", "empty separator");
            }
            let parts = match split_string_impl(hay_bytes, needle_bytes) {
                Some(parts) => parts,
                None => return MoltObject::none().bits(),
            };
            let mut elems: Vec<u64> = Vec::with_capacity(parts.len());
            for part in parts {
                let ptr = alloc_string(&part);
                if ptr.is_null() {
                    for bits in elems {
                        dec_ref_bits(bits);
                    }
                    return MoltObject::none().bits();
                }
                elems.push(MoltObject::from_ptr(ptr).bits());
            }
            let list_ptr = alloc_list_with_capacity(&elems, elems.len().max(MAX_SMALL_LIST));
            if list_ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(bits);
                }
                return MoltObject::none().bits();
            }
            for bits in elems {
                dec_ref_bits(bits);
            }
            return MoltObject::from_ptr(list_ptr).bits();
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
            let out = match replace_string_impl(hay_bytes, needle_bytes, repl_bytes) {
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
                raise_exception("ValueError", "empty separator");
            }
            let parts = match split_bytes_impl(hay_bytes, needle_bytes) {
                Some(parts) => parts,
                None => return MoltObject::none().bits(),
            };
            let mut elems: Vec<u64> = Vec::with_capacity(parts.len());
            for part in parts {
                let ptr = alloc_bytes(&part);
                if ptr.is_null() {
                    for bits in elems {
                        dec_ref_bits(bits);
                    }
                    return MoltObject::none().bits();
                }
                elems.push(MoltObject::from_ptr(ptr).bits());
            }
            let list_ptr = alloc_list_with_capacity(&elems, elems.len().max(MAX_SMALL_LIST));
            if list_ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(bits);
                }
                return MoltObject::none().bits();
            }
            for bits in elems {
                dec_ref_bits(bits);
            }
            return MoltObject::from_ptr(list_ptr).bits();
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
                raise_exception("ValueError", "empty separator");
            }
            let parts = match split_bytes_impl(hay_bytes, needle_bytes) {
                Some(parts) => parts,
                None => return MoltObject::none().bits(),
            };
            let mut elems: Vec<u64> = Vec::with_capacity(parts.len());
            for part in parts {
                let ptr = alloc_bytearray(&part);
                if ptr.is_null() {
                    for bits in elems {
                        dec_ref_bits(bits);
                    }
                    return MoltObject::none().bits();
                }
                elems.push(MoltObject::from_ptr(ptr).bits());
            }
            let list_ptr = alloc_list_with_capacity(&elems, elems.len().max(MAX_SMALL_LIST));
            if list_ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(bits);
                }
                return MoltObject::none().bits();
            }
            for bits in elems {
                dec_ref_bits(bits);
            }
            return MoltObject::from_ptr(list_ptr).bits();
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
        }
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    let obj = obj_from_bits(obj_bits);
    let key = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
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
            if type_id == TYPE_ID_DICT {
                dict_set_in_place(ptr, key_bits, val_bits);
                return obj_bits;
            }
        }
    }
    MoltObject::none().bits()
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
    let obj = obj_from_bits(iter_bits);
    if let Some(ptr) = obj.as_ptr() {
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
pub extern "C" fn molt_iter_next(iter_bits: u64) -> u64 {
    let obj = obj_from_bits(iter_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
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
                println!("{}", format_exception(ptr));
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
            if object_type_id(ptr) == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return String::from_utf8_lossy(bytes).into_owned();
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
        match header.type_id {
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
                dec_ref_bits(kind_bits);
                dec_ref_bits(msg_bits);
            }
            TYPE_ID_CONTEXT_MANAGER => {
                let payload_bits = context_payload_bits(ptr);
                dec_ref_bits(payload_bits);
            }
            TYPE_ID_DATACLASS => {
                let vec_ptr = dataclass_fields_ptr(ptr);
                if !vec_ptr.is_null() {
                    let vec = Box::from_raw(vec_ptr);
                    for bits in vec.iter() {
                        dec_ref_bits(*bits);
                    }
                }
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() {
                    drop(Box::from_raw(desc_ptr));
                }
            }
            TYPE_ID_BUFFER2D => {
                let buf_ptr = buffer2d_ptr(ptr);
                if !buf_ptr.is_null() {
                    drop(Box::from_raw(buf_ptr));
                }
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

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_generic(
    _obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len: usize,
) -> i64 {
    let _s = {
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        std::str::from_utf8(slice).unwrap()
    };
    0
}
mod arena;
use arena::TempArena;
