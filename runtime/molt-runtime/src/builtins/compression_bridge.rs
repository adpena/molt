//! Bridge wrappers for molt-runtime-compression.
//!
//! These `#[no_mangle] extern "C"` functions expose `pub(crate)` helpers
//! with C-ABI-safe signatures so that `molt-runtime-compression` can call
//! them via matching `extern "C"` declarations at link time.
//!
//! The caller (compression crate) already holds the GIL via its own
//! `with_gil_entry!` macro. Each wrapper re-acquires a token via the
//! runtime's `with_gil_entry!` (which is reentrant / a no-op if already held).

use crate::*;

type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
type RuntimeExtensionStateClear = unsafe extern "C" fn(u64, *mut u8);
type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

pub(crate) fn molt_bridge_runtime_state_get_or_init_attached_body(
    _py: &PyToken<'_>,
    key_ptr: *const u8,
    key_len: usize,
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let key = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
        crate::state::runtime_extension_state_get_or_init(
            crate::state::runtime_state::runtime_state(_py),
            key,
            init,
            clear,
            drop,
        )
    }
}

// -- Numbers / ints -----------------------------------------------------------

pub(crate) fn molt_bridge_int_bits_from_i64_attached_body(_py: &PyToken<'_>, val: i64) -> u64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        crate::builtins::numbers::int_bits_from_i64(_py, val)
    }
}

pub(crate) fn molt_bridge_to_i64_attached_body(
    _py: &PyToken<'_>,
    obj_bits: u64,
    ok: *mut bool,
) -> i64 {
    let obj = obj_from_bits(obj_bits);
    match crate::builtins::numbers::to_i64(obj) {
        Some(v) => {
            unsafe {
                *ok = true;
            }
            v
        }
        None => {
            unsafe {
                *ok = false;
            }
            0
        }
    }
}

pub(crate) fn molt_bridge_index_i64_from_obj_attached_body(
    _py: &PyToken<'_>,
    obj_bits: u64,
    err_ptr: *const u8,
    err_len: usize,
) -> i64 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let err =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_ptr, err_len)) };
        crate::builtins::numbers::index_i64_from_obj(_py, obj_bits, err)
    }
}

// -- Exceptions ---------------------------------------------------------------

pub(crate) fn molt_bridge_exception_pending_attached_body(_py: &PyToken<'_>) -> bool {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        exception_pending(_py)
    }
}

// -- Bytes / memory -----------------------------------------------------------

pub(crate) fn molt_bridge_alloc_bytes_attached_body(
    _py: &PyToken<'_>,
    data_ptr: *const u8,
    data_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    }
}

pub(crate) fn molt_bridge_bytes_data_attached_body(_py: &PyToken<'_>, ptr: *mut u8) -> *const u8 {
    unsafe { bytes_data(ptr) }
}

pub(crate) fn molt_bridge_bytes_len_attached_body(_py: &PyToken<'_>, ptr: *mut u8) -> usize {
    unsafe { bytes_len(ptr) }
}

// -- Strings ------------------------------------------------------------------

pub(crate) fn molt_bridge_string_obj_to_owned_attached_body(
    _py: &PyToken<'_>,
    obj_bits: u64,
    out_len: *mut usize,
) -> *mut u8 {
    let obj = obj_from_bits(obj_bits);
    match string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            unsafe { crate::resource::bridge_buffer::export_u8_box_ptr(bytes, out_len) }
        }
        None => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_free_string(ptr: *mut u8, len: usize) {
    unsafe { crate::resource::bridge_buffer::__molt_bridge_free_u8(ptr, len) };
}

pub(crate) fn molt_bridge_type_name_attached_body(
    _py: &PyToken<'_>,
    obj_bits: u64,
    out_len: *mut usize,
) -> *const u8 {
    use std::cell::RefCell;
    thread_local! {
        static BUF: RefCell<String> = const { RefCell::new(String::new()) };
    }
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let obj = obj_from_bits(obj_bits);
        let name = type_name(_py, obj);
        BUF.with(|buf| {
            let mut buf = buf.borrow_mut();
            buf.clear();
            buf.push_str(&name);
            unsafe {
                *out_len = buf.len();
            }
            buf.as_ptr()
        })
    }
}

pub(crate) fn molt_bridge_alloc_string_attached_body(
    _py: &PyToken<'_>,
    data_ptr: *const u8,
    data_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    }
}

// -- Object allocation --------------------------------------------------------

pub(crate) fn molt_bridge_alloc_list_attached_body(
    _py: &PyToken<'_>,
    items_ptr: *const u64,
    items_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let items = unsafe { std::slice::from_raw_parts(items_ptr, items_len) };
        alloc_list(_py, items)
    }
}

pub(crate) fn molt_bridge_alloc_tuple_attached_body(
    _py: &PyToken<'_>,
    items_ptr: *const u64,
    items_len: usize,
) -> *mut u8 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(_py.attached_context()) };
        let items = unsafe { std::slice::from_raw_parts(items_ptr, items_len) };
        alloc_tuple(_py, items)
    }
}

// -- Object inspection --------------------------------------------------------

pub(crate) fn molt_bridge_object_type_id_attached_body(_py: &PyToken<'_>, ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

// -- Pointer registry ---------------------------------------------------------

pub(crate) fn molt_bridge_release_ptr_attached_body(_py: &PyToken<'_>, ptr: *mut u8) {
    let _ = release_ptr(ptr);
}

// Typed payload owners share the root native-handle seal and Arc lifetime.
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_zlib_compressor_owner_new => "__molt_compression_zlib_compressor_owner_new",
    __molt_compression_zlib_compressor_owner_clone => "__molt_compression_zlib_compressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::zlib::CompressorHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_zlib_decompressor_owner_new => "__molt_compression_zlib_decompressor_owner_new",
    __molt_compression_zlib_decompressor_owner_clone => "__molt_compression_zlib_decompressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::zlib::DecompressorHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_bz2_compressor_owner_new => "__molt_compression_bz2_compressor_owner_new",
    __molt_compression_bz2_compressor_owner_clone => "__molt_compression_bz2_compressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::bz2::Bz2CompressorHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_bz2_decompressor_owner_new => "__molt_compression_bz2_decompressor_owner_new",
    __molt_compression_bz2_decompressor_owner_clone => "__molt_compression_bz2_decompressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::bz2::Bz2DecompressorHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_bz2_file_owner_new => "__molt_compression_bz2_file_owner_new",
    __molt_compression_bz2_file_owner_clone => "__molt_compression_bz2_file_owner_clone",
    std::sync::Mutex<molt_runtime_compression::bz2::Bz2FileHandle>
);
#[cfg(not(target_arch = "wasm32"))]
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_lzma_compressor_owner_new => "__molt_compression_lzma_compressor_owner_new",
    __molt_compression_lzma_compressor_owner_clone => "__molt_compression_lzma_compressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::lzma::LzmaCompressorHandle>
);
#[cfg(not(target_arch = "wasm32"))]
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_lzma_decompressor_owner_new => "__molt_compression_lzma_decompressor_owner_new",
    __molt_compression_lzma_decompressor_owner_clone => "__molt_compression_lzma_decompressor_owner_clone",
    std::sync::Mutex<molt_runtime_compression::lzma::LzmaDecompressorHandle>
);
#[cfg(not(target_arch = "wasm32"))]
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_lzma_file_owner_new => "__molt_compression_lzma_file_owner_new",
    __molt_compression_lzma_file_owner_clone => "__molt_compression_lzma_file_owner_clone",
    std::sync::Mutex<molt_runtime_compression::lzma::LzmaFileHandle>
);
crate::object::native_handle::native_handle_bridge!(
    __molt_compression_gzip_file_owner_new => "__molt_compression_gzip_file_owner_new",
    __molt_compression_gzip_file_owner_clone => "__molt_compression_gzip_file_owner_clone",
    std::sync::Mutex<molt_runtime_compression::gzip::GzipFileHandle>
);

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/attached_exports_generated/builtins/compression_bridge.rs"
));
