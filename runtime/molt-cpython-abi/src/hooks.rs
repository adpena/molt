//! Runtime hooks vtable — pluggable object allocators from `molt-lang-runtime`.
//!
//! `molt-lang-cpython-abi` cannot depend on `molt-lang-runtime` (that would
//! create a circular dependency). Instead, the runtime registers concrete
//! implementations at startup via [`set_runtime_hooks`].
//!
//! Every hook function uses `extern "C"` with primitive types so the
//! registration call works across crate boundaries without monomorphisation.
//!
//! ## Handle encoding
//!
//! All `u64` parameters and return values are raw `MoltObject` bit patterns
//! (QNAN-boxed). `0` is reserved for "null / not found / error".

use std::sync::OnceLock;

/// Vtable of runtime-provided object-allocation and inspection hooks.
/// All function pointers are `extern "C"` for ABI stability across crate boundaries.
#[derive(Clone, Copy)]
#[allow(dead_code)]
#[repr(C)]
pub struct RuntimeHooks {
    // ── Allocation ────────────────────────────────────────────────────────────
    /// Allocate a UTF-8 string object. Returns handle bits, 0 on failure.
    pub alloc_str: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate a bytes object. Returns handle bits, 0 on failure.
    pub alloc_bytes: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate an empty list. Returns handle bits.
    pub alloc_list: unsafe extern "C" fn() -> u64,
    /// Append `item_bits` to the list at `list_bits`.
    pub list_append: unsafe extern "C" fn(list_bits: u64, item_bits: u64),
    /// Return the number of items in a list.
    pub list_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the list, or 0 if out of range.
    pub list_item: unsafe extern "C" fn(bits: u64, i: usize) -> u64,
    /// Allocate a tuple of exactly `n` slots. Slots are uninitialized (None).
    pub alloc_tuple: unsafe extern "C" fn(n: usize) -> u64,
    /// Set slot `i` of tuple `bits` to `val_bits`.
    pub tuple_set: unsafe extern "C" fn(bits: u64, i: usize, val_bits: u64),
    /// Return the number of items in a tuple.
    pub tuple_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the tuple, or 0 if out of range.
    pub tuple_item: unsafe extern "C" fn(bits: u64, i: usize) -> u64,
    /// Allocate an empty dict. Returns handle bits.
    pub alloc_dict: unsafe extern "C" fn() -> u64,
    /// Insert or overwrite a key→value pair in the dict.
    pub dict_set: unsafe extern "C" fn(dict_bits: u64, key_bits: u64, val_bits: u64),
    /// Lookup `key_bits` in the dict. Returns 0 if not found.
    pub dict_get: unsafe extern "C" fn(dict_bits: u64, key_bits: u64) -> u64,
    /// Return the number of entries in a dict.
    pub dict_len: unsafe extern "C" fn(bits: u64) -> usize,
    // ── Data access ───────────────────────────────────────────────────────────
    /// Return a pointer to the UTF-8 bytes of a string handle, writing the
    /// length into `*out_len`. Pointer is valid until next GC cycle.
    /// Returns null on error.
    pub str_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    /// Return a pointer to the raw bytes of a bytes handle.
    pub bytes_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    // ── Type classification ───────────────────────────────────────────────────
    /// Classify a heap-pointer handle into a `MoltTypeTag` discriminant (u8).
    /// Used by `classify_handle` to fill in the SIMD type-tag table for heap types.
    pub classify_heap: unsafe extern "C" fn(bits: u64) -> u8,
    // ── Reference counting ────────────────────────────────────────────────────
    /// Increment the Molt reference count for a heap object.
    pub inc_ref: unsafe extern "C" fn(bits: u64),
    /// Decrement the Molt reference count; deallocate if it reaches zero.
    pub dec_ref: unsafe extern "C" fn(bits: u64),
    // ── Module / C-extension support ─────────────────────────────────────────
    /// Allocate a new Molt module object whose `__name__` is the UTF-8 string
    /// in `name_data[..name_len]`.  Returns module handle bits, 0 on failure.
    pub alloc_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Set `module_bits.__dict__[name_data[..name_len]] = value_bits`.
    /// `module_bits` must be a Molt module handle.  Returns 0 on success, -1 on failure.
    pub module_set_attr: unsafe extern "C" fn(
        module_bits: u64,
        name_data: *const u8,
        name_len: usize,
        value_bits: u64,
    ) -> std::os::raw::c_int,
    /// Register a `PyCFunction`-style C function pointer (`meth_addr`) as a
    /// callable Molt function.  `flags` follows CPython's `METH_*` bitmask.
    /// `name_data[..name_len]` is the function's `__name__`.  Returns the bits
    /// of the resulting Molt callable, 0 on failure (e.g. unsupported flags).
    pub register_c_function: unsafe extern "C" fn(
        meth_addr: u64,
        flags: std::os::raw::c_int,
        name_data: *const u8,
        name_len: usize,
    ) -> u64,
}

/// Global hook table, set once by `molt-lang-runtime` at init time.
static RUNTIME_HOOKS: OnceLock<RuntimeHooks> = OnceLock::new();

/// Register the runtime hook vtable. Must be called exactly once, before any
/// C extension function is invoked. Panics if called more than once.
///
/// # Safety
/// All function pointers in `hooks` must remain valid for the lifetime of the process.
pub unsafe fn set_runtime_hooks(hooks: RuntimeHooks) {
    RUNTIME_HOOKS
        .set(hooks)
        .unwrap_or_else(|_| panic!("molt_cpython_abi: runtime hooks already registered"));
}

/// C-callable registration entry point for `molt-lang-runtime`.
///
/// # Safety
/// Same as `set_runtime_hooks`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_register_hooks(hooks: *const RuntimeHooks) {
    if hooks.is_null() {
        return;
    }
    unsafe {
        set_runtime_hooks(*hooks);
    }
}

/// Access the runtime hooks. Returns `None` if hooks have not been registered
/// (pre-init or test contexts). Callers must degrade gracefully (return None/0).
#[inline]
pub fn hooks() -> Option<&'static RuntimeHooks> {
    RUNTIME_HOOKS.get()
}

// ─── No-op stubs for pre-init or test use ────────────────────────────────────

unsafe extern "C" fn stub_alloc_str(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_bytes(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_list() -> u64 {
    0
}
unsafe extern "C" fn stub_list_append(_list_bits: u64, _item_bits: u64) {}
unsafe extern "C" fn stub_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_list_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_tuple(_n: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_tuple_set(_bits: u64, _i: usize, _val: u64) {}
unsafe extern "C" fn stub_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_tuple_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_dict() -> u64 {
    0
}
unsafe extern "C" fn stub_dict_set(_d: u64, _k: u64, _v: u64) {}
unsafe extern "C" fn stub_dict_get(_d: u64, _k: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_dict_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_str_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    c"".as_ptr().cast()
}
unsafe extern "C" fn stub_bytes_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    std::ptr::null()
}
unsafe extern "C" fn stub_classify_heap(_bits: u64) -> u8 {
    crate::abi_types::MoltTypeTag::Other as u8
}
unsafe extern "C" fn stub_inc_ref(_bits: u64) {}
unsafe extern "C" fn stub_dec_ref(_bits: u64) {}
unsafe extern "C" fn stub_alloc_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_module_set_attr(
    _m: u64,
    _data: *const u8,
    _len: usize,
    _v: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _data: *const u8,
    _len: usize,
) -> u64 {
    0
}

/// A no-op hooks table used when the runtime hasn't registered yet.
pub const STUB_HOOKS: RuntimeHooks = RuntimeHooks {
    alloc_str: stub_alloc_str,
    alloc_bytes: stub_alloc_bytes,
    alloc_list: stub_alloc_list,
    list_append: stub_list_append,
    list_len: stub_list_len,
    list_item: stub_list_item,
    alloc_tuple: stub_alloc_tuple,
    tuple_set: stub_tuple_set,
    tuple_len: stub_tuple_len,
    tuple_item: stub_tuple_item,
    alloc_dict: stub_alloc_dict,
    dict_set: stub_dict_set,
    dict_get: stub_dict_get,
    dict_len: stub_dict_len,
    str_data: stub_str_data,
    bytes_data: stub_bytes_data,
    classify_heap: stub_classify_heap,
    inc_ref: stub_inc_ref,
    dec_ref: stub_dec_ref,
    alloc_module: stub_alloc_module,
    module_set_attr: stub_module_set_attr,
    register_c_function: stub_register_c_function,
};

/// Return the registered hooks or fall back to the no-op stubs.
/// Use this in API functions where a partial result is better than a panic.
#[inline]
pub fn hooks_or_stubs() -> &'static RuntimeHooks {
    RUNTIME_HOOKS.get().unwrap_or(&STUB_HOOKS)
}
