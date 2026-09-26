//! Minimal runtime string authority for ABI integration tests.
//!
//! Physical exception instances and normal C-API string results require the
//! same `alloc_str`/`str_data` contract as production. Tests wire this helper
//! instead of depending on the deleted text-only exception side channel.

#![allow(dead_code)]

use molt_cpython_abi::hooks::{OwnedHandleResult, RuntimeHooks};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::ptr;
use std::sync::{LazyLock, Mutex};

struct FakeString {
    bytes: Box<[u8]>,
}

static STRINGS: LazyLock<Mutex<HashMap<u64, Box<FakeString>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub unsafe extern "C" fn alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() || len == 0 {
        Box::<[u8]>::default()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
            .to_vec()
            .into_boxed_slice()
    };
    let value = Box::new(FakeString { bytes });
    let bits = MoltObject::from_ptr((&raw const *value).cast_mut().cast::<u8>()).bits();
    STRINGS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(bits, value);
    bits
}

pub unsafe extern "C" fn str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let strings = STRINGS.lock().unwrap_or_else(|error| error.into_inner());
    let Some(value) = strings.get(&bits) else {
        return ptr::null();
    };
    if !out_len.is_null() {
        unsafe { *out_len = value.bytes.len() };
    }
    value.bytes.as_ptr()
}

// This deliberately small fake supplies scalar fixtures, not a second Python
// formatting oracle. Runtime-backed tests own formatting conformance.
unsafe fn stringify(bits: u64, repr: bool) -> OwnedHandleResult {
    let value = MoltObject::from_bits(bits);
    let text = if value.is_none() {
        "None".to_owned()
    } else if let Some(value) = value.as_bool() {
        if value { "True" } else { "False" }.to_owned()
    } else if let Some(value) = value.as_int() {
        value.to_string()
    } else if let Some(value) = value.as_float() {
        value.to_string()
    } else {
        let strings = STRINGS.lock().unwrap_or_else(|error| error.into_inner());
        let Some(value) = strings.get(&bits) else {
            return OwnedHandleResult::error();
        };
        if !repr {
            return OwnedHandleResult::ok(bits);
        }
        format!("'{}'", String::from_utf8_lossy(&value.bytes))
    };
    OwnedHandleResult::ok(unsafe { alloc_str(text.as_ptr(), text.len()) })
}

pub unsafe extern "C" fn object_str(bits: u64) -> OwnedHandleResult {
    unsafe { stringify(bits, false) }
}

pub unsafe extern "C" fn object_repr(bits: u64) -> OwnedHandleResult {
    unsafe { stringify(bits, true) }
}

pub fn wire(hooks: &mut RuntimeHooks) {
    hooks.alloc_str = alloc_str;
    hooks.str_data = str_data;
    hooks.object_str = object_str;
    hooks.object_repr = object_repr;
}

pub fn contains(bits: u64) -> bool {
    STRINGS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&bits)
}
