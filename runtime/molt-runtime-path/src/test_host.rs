//! Test-only ABI host for the extracted path crate.
//!
//! Production binaries resolve the bridge symbols below from `molt-runtime`.
//! Crate-local unit tests link `molt-runtime-path` without that host, so this
//! module provides the same symbol surface for the test binary.  Runtime object
//! behavior stays explicit: scalar immediates are handled here, while calls that
//! need the full Molt object heap abort with a named symbol instead of drifting
//! into an incomplete compatibility runtime.

use molt_runtime_core::MoltObject;

const INLINE_INT_MIN: i64 = -(1i64 << 46);
const INLINE_INT_MAX_EXCLUSIVE: i64 = 1i64 << 46;

fn none_bits() -> u64 {
    MoltObject::none().bits()
}

fn bool_bits(value: bool) -> u64 {
    MoltObject::from_bool(value).bits()
}

fn int_bits(value: i64) -> u64 {
    if !(INLINE_INT_MIN..INLINE_INT_MAX_EXCLUSIVE).contains(&value) {
        unexpected("molt_int_from_i64");
    }
    MoltObject::from_int(value).bits()
}

fn immediate_truthy(bits: u64) -> Option<bool> {
    let obj = MoltObject::from_bits(bits);
    if obj.is_none() {
        return Some(false);
    }
    if let Some(value) = obj.as_bool() {
        return Some(value);
    }
    if let Some(value) = obj.as_int() {
        return Some(value != 0);
    }
    if let Some(value) = obj.as_float() {
        return Some(value != 0.0);
    }
    None
}

fn immediate_type_name(bits: u64) -> Option<&'static str> {
    let obj = MoltObject::from_bits(bits);
    if obj.is_none() {
        return Some("NoneType");
    }
    if obj.is_bool() {
        return Some("bool");
    }
    if obj.is_int() {
        return Some("int");
    }
    if obj.is_float() {
        return Some("float");
    }
    None
}

fn export_bytes(bytes: &[u8], out_ptr: *mut *const u8, out_len: *mut usize) -> i32 {
    if out_ptr.is_null() || out_len.is_null() {
        return 0;
    }
    let boxed: Box<[u8]> = bytes.to_vec().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *const u8;
    unsafe {
        *out_ptr = ptr;
        *out_len = len;
    }
    1
}

fn unexpected(name: &'static str) -> ! {
    eprintln!("molt-runtime-path unit test crossed into full runtime ABI symbol `{name}`");
    std::process::abort();
}

macro_rules! unexpected_u64 {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> u64 {
            let _ = ($($arg),*);
            unexpected(stringify!($name))
        }
    };
}

macro_rules! unexpected_i64 {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> i64 {
            let _ = ($($arg),*);
            unexpected(stringify!($name))
        }
    };
}

macro_rules! unexpected_ptr_const_u8 {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> *const u8 {
            let _ = ($($arg),*);
            unexpected(stringify!($name))
        }
    };
}

macro_rules! unexpected_ptr_mut_u8 {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> *mut u8 {
            let _ = ($($arg),*);
            unexpected(stringify!($name))
        }
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_bridge_free_u8(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_bridge_free_u64(ptr: *mut u64, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

unexpected_ptr_mut_u8!(__molt_path_alloc_tuple(elems_ptr: *const u64, elems_len: usize));
unexpected_ptr_mut_u8!(__molt_path_alloc_list(elems_ptr: *const u64, elems_len: usize));
unexpected_ptr_mut_u8!(__molt_path_alloc_string(data_ptr: *const u8, data_len: usize));
unexpected_ptr_mut_u8!(__molt_path_alloc_bytes(data_ptr: *const u8, data_len: usize));
unexpected_ptr_mut_u8!(__molt_path_alloc_dict_with_pairs(
    pairs_ptr: *const u64,
    pairs_len: usize,
));

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_exception(
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    let _ = (type_ptr, type_len, msg_ptr, msg_len);
    unexpected("__molt_path_raise_exception")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_exception_pending() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error(
    err_kind: u32,
    err_msg_ptr: *const u8,
    err_msg_len: usize,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    let _ = (err_kind, err_msg_ptr, err_msg_len, ctx_ptr, ctx_len);
    unexpected("__molt_path_raise_os_error")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error_errno(
    errno: i64,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    let _ = (errno, ctx_ptr, ctx_len);
    unexpected("__molt_path_raise_os_error_errno")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_object_type_id(ptr: *mut u8) -> u32 {
    let _ = ptr;
    unexpected("__molt_path_object_type_id")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let _ = (bits, out_ptr, out_len);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_is_truthy(bits: u64) -> i32 {
    match immediate_truthy(bits) {
        Some(value) => i32::from(value),
        None => unexpected("__molt_path_is_truthy"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_bytes_like_slice(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let _ = (ptr, out_ptr, out_len);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_dec_ref_bits(bits: u64) {
    let _ = bits;
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_inc_ref_bits(bits: u64) {
    let _ = bits;
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_i64(bits: u64, out: *mut i64) -> i32 {
    if out.is_null() {
        return 0;
    }
    match MoltObject::from_bits(bits).as_int() {
        Some(value) => {
            unsafe {
                *out = value;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_f64(bits: u64, out: *mut f64) -> i32 {
    if out.is_null() {
        return 0;
    }
    let obj = MoltObject::from_bits(bits);
    let Some(value) = obj
        .as_float()
        .or_else(|| obj.as_int().map(|value| value as f64))
    else {
        return 0;
    };
    unsafe {
        *out = value;
    }
    1
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    let _ = ptr;
    unexpected("__molt_path_seq_vec_ptr")
}

unexpected_u64!(__molt_path_molt_iter(bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32 {
    let _ = (iter_bits, out);
    unexpected("__molt_path_molt_iter_next")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_has_capability(name_ptr: *const u8, name_len: usize) -> i32 {
    let _ = (name_ptr, name_len);
    1
}

unexpected_u64!(__molt_path_molt_object_hash(bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_path_from_bits(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let _ = (bits, out_ptr, out_len);
    unexpected("__molt_path_path_from_bits")
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    match immediate_type_name(bits) {
        Some(name) => export_bytes(name.as_bytes(), out_ptr, out_len),
        None => unexpected("__molt_path_type_name"),
    }
}

unexpected_u64!(molt_string_from(data: *const u8, len: u64));
unexpected_u64!(molt_bytes_from(data: *const u8, len: u64));
unexpected_ptr_const_u8!(molt_string_as_ptr(string_bits: u64, out_len: *mut u64));
unexpected_ptr_const_u8!(molt_bytes_as_ptr(bytes_bits: u64, out_len: *mut u64));
unexpected_u64!(molt_alloc(size_bits: u64));
unexpected_u64!(molt_tuple_from_array(items: *const u64, len: u64));
unexpected_u64!(molt_list_from_array(items: *const u64, len: u64));
unexpected_u64!(molt_dict_from_pairs(keys: *const u64, values: *const u64, len: u64));
unexpected_u64!(molt_dict_new(capacity_bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn molt_none() -> u64 {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    int_bits(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_f64(value: f64) -> u64 {
    MoltObject::from_float(value).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    let _ = bits;
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_obj(bits: u64) {
    let _ = bits;
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_n(bits: u64, count: u32) -> u64 {
    let _ = count;
    bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_n(bits: u64, count: u32) {
    let _ = (bits, count);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref(ptr: *mut u8) {
    let _ = ptr;
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref(ptr: *mut u8) {
    let _ = ptr;
}

unexpected_u64!(molt_exception_new(kind_bits: u64, args_bits: u64));
unexpected_u64!(molt_exception_new_builtin(tag: u64, args_bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_match_builtin(exc_bits: u64, tag: u64) -> u64 {
    let _ = (exc_bits, tag);
    bool_bits(false)
}

unexpected_u64!(molt_raise(exc_bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending() -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending_fast() -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_active() -> u64 {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_clear() -> u64 {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_last() -> u64 {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_enter() -> u64 {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_exit(prev_bits: u64) -> u64 {
    let _ = prev_bits;
    none_bits()
}

unexpected_u64!(molt_exception_kind(exc_bits: u64));
unexpected_u64!(molt_exception_message(exc_bits: u64));

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy(val: u64) -> i64 {
    match immediate_truthy(val) {
        Some(value) => i64::from(value),
        None => unexpected("molt_is_truthy"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_int(bits: u64) -> i64 {
    i64::from(MoltObject::from_bits(bits).as_int().unwrap_or(0) != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_int_nogil(bits: u64) -> i64 {
    molt_is_truthy_int(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_bool(bits: u64) -> i64 {
    i64::from(MoltObject::from_bits(bits).as_bool().unwrap_or(false))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_bool_nogil(bits: u64) -> i64 {
    molt_is_truthy_bool(bits)
}

unexpected_u64!(molt_list_int_getitem_nogil(list_bits: u64, index_bits: u64));
unexpected_u64!(molt_list_int_setitem_nogil(
    list_bits: u64,
    index_bits: u64,
    value_bits: u64,
));
unexpected_i64!(molt_list_int_getitem_raw(list_bits: u64, raw_index: i64));
unexpected_i64!(molt_list_int_getitem_raw_checked(
    list_bits: u64,
    raw_index: i64
));
unexpected_u64!(molt_list_int_setitem_raw(
    list_bits: u64,
    raw_index: i64,
    raw_value: i64,
));
unexpected_u64!(molt_type_of(val_bits: u64));
unexpected_u64!(molt_str_from_obj(val_bits: u64));
unexpected_u64!(molt_repr_from_obj(val_bits: u64));
unexpected_u64!(molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64));
unexpected_u64!(molt_float_from_obj(val_bits: u64));
unexpected_u64!(molt_fast_list_append(method_bits: u64, elem_bits: u64));
unexpected_u64!(molt_fast_str_join(method_bits: u64, iterable_bits: u64));
unexpected_u64!(molt_fast_dict_get(method_bits: u64, key_bits: u64, default_bits: u64));
unexpected_u64!(molt_list_append(list_bits: u64, val_bits: u64));
unexpected_u64!(molt_list_pop(list_bits: u64, index_bits: u64));
unexpected_u64!(molt_list_extend(list_bits: u64, other_bits: u64));
unexpected_u64!(molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64));
unexpected_u64!(molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64));
unexpected_u64!(molt_str_contains(container_bits: u64, item_bits: u64));
unexpected_u64!(molt_list_contains(container_bits: u64, item_bits: u64));
unexpected_u64!(molt_dict_contains(container_bits: u64, item_bits: u64));
unexpected_u64!(molt_string_eq(a: u64, b: u64));
unexpected_u64!(molt_unpack_sequence(
    seq_bits: u64,
    expected_count: u64,
    output_ptr: *mut u64,
));

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_release_guard() -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_reacquire_guard(token: u64) {
    let _ = token;
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_target_at_least(major: i64, minor: i64) -> i64 {
    i64::from(major < 3 || (major == 3 && minor <= 12))
}
