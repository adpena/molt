//! Minimal runtime string authority for ABI integration tests.
//!
//! Physical exception instances and normal C-API string results require the
//! same `alloc_str`/`str_data` contract as production. Tests wire this helper
//! instead of depending on the deleted text-only exception side channel.

#![allow(dead_code)]

use molt_cpython_abi::hooks::RuntimeHooks;
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

pub unsafe extern "C" fn float_repr(value: f64, out: *mut u8, cap: usize) -> usize {
    let rendered = value.to_string();
    let bytes = rendered.as_bytes();
    if bytes.len() <= cap && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

pub fn wire(hooks: &mut RuntimeHooks) {
    hooks.alloc_str = alloc_str;
    hooks.str_data = str_data;
    hooks.float_repr = float_repr;
}

pub fn contains(bits: u64) -> bool {
    STRINGS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&bits)
}
