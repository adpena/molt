//! Core API surface for molt-runtime subcrates.
//!
//! Extracted crates (molt-runtime-crypto, molt-runtime-net, etc.)
//! depend on this crate instead of the full molt-runtime.

// Re-export the object model.
pub use molt_obj_model::MoltObject;
pub use molt_obj_model::{register_ptr, resolve_ptr};

// ---------------------------------------------------------------------------
// Convenience helpers (mirror the signatures in molt-runtime/src/object/mod.rs)
// ---------------------------------------------------------------------------

/// Construct a `MoltObject` from its raw 64-bit NaN-boxed representation.
#[inline]
pub fn obj_from_bits(bits: u64) -> MoltObject {
    MoltObject::from_bits(bits)
}

/// Extract a raw pointer from a NaN-boxed `u64`.
///
/// Tries `MoltObject::as_ptr()` first; falls back to the pointer registry.
#[inline]
pub fn ptr_from_bits(bits: u64) -> *mut u8 {
    let obj = obj_from_bits(bits);
    if obj.is_ptr() {
        return obj.as_ptr().unwrap_or(std::ptr::null_mut());
    }
    resolve_ptr(bits).unwrap_or(std::ptr::null_mut())
}

/// Register a raw pointer and return its `u64` address for NaN-boxing.
#[inline]
pub fn bits_from_ptr(ptr: *mut u8) -> u64 {
    register_ptr(ptr)
}

// ---------------------------------------------------------------------------
// Type ID constants (canonical copies — values must match molt-runtime)
// ---------------------------------------------------------------------------

pub mod type_ids {
    pub const TYPE_ID_OBJECT: u32 = 100;
    pub const TYPE_ID_STRING: u32 = 200;
    pub const TYPE_ID_LIST: u32 = 201;
    pub const TYPE_ID_BYTES: u32 = 202;
    pub const TYPE_ID_LIST_BUILDER: u32 = 203;
    pub const TYPE_ID_DICT: u32 = 204;
    pub const TYPE_ID_DICT_BUILDER: u32 = 205;
    pub const TYPE_ID_TUPLE: u32 = 206;
    pub const TYPE_ID_DICT_KEYS_VIEW: u32 = 207;
    pub const TYPE_ID_DICT_VALUES_VIEW: u32 = 208;
    pub const TYPE_ID_DICT_ITEMS_VIEW: u32 = 209;
    pub const TYPE_ID_ITER: u32 = 210;
    pub const TYPE_ID_BYTEARRAY: u32 = 211;
    pub const TYPE_ID_RANGE: u32 = 212;
    pub const TYPE_ID_SLICE: u32 = 213;
    pub const TYPE_ID_EXCEPTION: u32 = 214;
    pub const TYPE_ID_DATACLASS: u32 = 215;
    pub const TYPE_ID_BUFFER2D: u32 = 216;
    pub const TYPE_ID_CONTEXT_MANAGER: u32 = 217;
    pub const TYPE_ID_FILE_HANDLE: u32 = 218;
    pub const TYPE_ID_MEMORYVIEW: u32 = 219;
    pub const TYPE_ID_INTARRAY: u32 = 220;
    pub const TYPE_ID_FUNCTION: u32 = 221;
    pub const TYPE_ID_BOUND_METHOD: u32 = 222;
    pub const TYPE_ID_MODULE: u32 = 223;
    pub const TYPE_ID_TYPE: u32 = 224;
    pub const TYPE_ID_GENERATOR: u32 = 225;
    pub const TYPE_ID_CLASSMETHOD: u32 = 226;
    pub const TYPE_ID_STATICMETHOD: u32 = 227;
    pub const TYPE_ID_PROPERTY: u32 = 228;
    pub const TYPE_ID_SUPER: u32 = 229;
    pub const TYPE_ID_SET: u32 = 230;
    pub const TYPE_ID_SET_BUILDER: u32 = 231;
    pub const TYPE_ID_FROZENSET: u32 = 232;
    pub const TYPE_ID_BIGINT: u32 = 233;
    pub const TYPE_ID_COMPLEX: u32 = 234;
    pub const TYPE_ID_ENUMERATE: u32 = 235;
    pub const TYPE_ID_CALLARGS: u32 = 236;
    pub const TYPE_ID_NOT_IMPLEMENTED: u32 = 237;
    pub const TYPE_ID_CALL_ITER: u32 = 238;
    pub const TYPE_ID_REVERSED: u32 = 239;
    pub const TYPE_ID_ZIP: u32 = 240;
    pub const TYPE_ID_MAP: u32 = 241;
    pub const TYPE_ID_FILTER: u32 = 242;
    pub const TYPE_ID_CODE: u32 = 243;
    pub const TYPE_ID_ELLIPSIS: u32 = 244;
    pub const TYPE_ID_GENERIC_ALIAS: u32 = 245;
    pub const TYPE_ID_ASYNC_GENERATOR: u32 = 246;
    pub const TYPE_ID_UNION: u32 = 247;
}

// ---------------------------------------------------------------------------
// GIL token stub
// ---------------------------------------------------------------------------

/// Zero-sized GIL token. Proves the caller holds the GIL.
///
/// This is a stub — the real GIL implementation lives in `molt-runtime`.
/// Extracted crates use this to satisfy API signatures without pulling in
/// the full runtime.
#[derive(Clone, Copy)]
pub struct PyToken(());

impl PyToken {
    /// Create a new token. In the real runtime this would be gated by the
    /// GIL; here it is unconditional so that extracted crates can compile.
    #[inline(always)]
    pub fn new() -> Self {
        Self(())
    }
}

/// RAII guard that releases the GIL on creation and re-acquires on drop.
///
/// In the real runtime, this delegates to `molt-runtime`'s
/// `concurrency::GilReleaseGuard` via `#[no_mangle]` FFI functions.
/// The token preserves GIL depth so nested releases work correctly.
pub struct GilReleaseGuard {
    token: u64,
}

impl GilReleaseGuard {
    #[inline]
    pub fn new() -> Self {
        let token = unsafe { ffi::molt_gil_release_guard() };
        Self { token }
    }
}

impl Drop for GilReleaseGuard {
    #[inline]
    fn drop(&mut self) {
        unsafe { ffi::molt_gil_reacquire_guard(self.token) };
    }
}

/// Execute a body while "holding the GIL".
///
/// Stub implementation: binds a [`PyToken`] and runs the body immediately.
/// The real version in `molt-runtime` actually acquires the GIL.
///
/// Wraps the body in `catch_unwind` to prevent panics from unwinding through
/// `extern "C"` boundaries (which is undefined behavior in Rust). On panic,
/// a `RuntimeError` exception is raised and a safe zero-sentinel is returned.
#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:expr) => {{
        let __py_token = $crate::PyToken::new();
        let $py = &__py_token;

        match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| $body)) {
            Ok(val) => val,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic in FFI boundary".to_string()
                };
                $crate::rt_raise_str("RuntimeError", &msg);
                // SAFETY: All FFI return types used with this macro (u64, i64,
                // i32, *mut u8, ()) are safely zero-initializable. The caller
                // will check for the pending exception before using this value.
                unsafe { ::std::mem::zeroed() }
            }
        }
    }};
}

// ---------------------------------------------------------------------------
// GIL acquisition — function-pointer dispatch so extracted crates can acquire
// the GIL without depending on molt-runtime.
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicPtr, Ordering};

/// CoreGilToken is an alias for PyToken — both represent proof that the GIL is held.
/// This unifies the token type so bridge functions (which take &PyToken) work seamlessly
/// with with_core_gil! (which produces a CoreGilToken).
pub type CoreGilToken = PyToken;

/// GIL vtable — function pointers for acquire/release.
/// Populated by molt-runtime at init time.
#[repr(C)]
pub struct GilVtable {
    /// Acquire the GIL. Returns an opaque guard value.
    /// The guard must be passed to `release` when done.
    pub acquire: unsafe extern "C" fn() -> u64,
    /// Release the GIL. Takes the guard value from acquire.
    pub release: unsafe extern "C" fn(u64),
    /// Check if the GIL is currently held by this thread.
    pub is_held: unsafe extern "C" fn() -> bool,
}

// SAFETY: The vtable is populated once at init time and then only read.
// All function pointers are plain `extern "C"` fn pointers (Send + Sync).
unsafe impl Send for GilVtable {}
unsafe impl Sync for GilVtable {}

static GIL_VTABLE: AtomicPtr<GilVtable> = AtomicPtr::new(std::ptr::null_mut());

/// Initialize the GIL vtable. Called once by molt-runtime at startup.
pub fn set_gil_vtable(vtable: &'static GilVtable) {
    GIL_VTABLE.store(
        vtable as *const GilVtable as *mut GilVtable,
        Ordering::Release,
    );
}

/// Acquire the GIL via the vtable. Panics if vtable not initialized.
#[inline]
pub fn core_gil_acquire() -> u64 {
    let ptr = GIL_VTABLE.load(Ordering::Acquire);
    assert!(
        !ptr.is_null(),
        "GIL vtable not initialized — call molt_runtime_init() first"
    );
    unsafe { ((*ptr).acquire)() }
}

/// Release the GIL via the vtable.
#[inline]
pub fn core_gil_release(guard: u64) {
    let ptr = GIL_VTABLE.load(Ordering::Acquire);
    if !ptr.is_null() {
        unsafe { ((*ptr).release)(guard) }
    }
}

/// RAII guard for GIL acquisition, usable from any crate.
pub struct CoreGilGuard {
    guard_token: u64,
}

impl CoreGilGuard {
    #[inline]
    pub fn new() -> Self {
        Self {
            guard_token: core_gil_acquire(),
        }
    }

    #[inline]
    pub fn token(&self) -> CoreGilToken {
        PyToken::new()
    }
}

impl Drop for CoreGilGuard {
    #[inline]
    fn drop(&mut self) {
        core_gil_release(self.guard_token);
    }
}

/// Cross-crate GIL entry macro — equivalent to with_gil_entry! but works from any crate.
///
/// Wraps the body in `catch_unwind` to prevent panics from unwinding through
/// `extern "C"` boundaries (which is undefined behavior in Rust).
#[macro_export]
macro_rules! with_core_gil {
    ($py:ident, $body:block) => {{
        let _gil_guard = $crate::CoreGilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;

        match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| $body)) {
            Ok(val) => val,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic in FFI boundary".to_string()
                };
                $crate::rt_raise_str("RuntimeError", &msg);
                // SAFETY: All FFI return types used with this macro (u64, i64,
                // i32, *mut u8, ()) are safely zero-initializable. The caller
                // will check for the pending exception before using this value.
                unsafe { ::std::mem::zeroed() }
            }
        }
    }};
}

// ---------------------------------------------------------------------------
// FFI — extern "C" declarations resolved by the linker from molt-runtime.
//
// These are the REAL runtime functions.  Each one has a matching
// `#[no_mangle] pub extern "C"` definition in molt-runtime (object/ops.rs,
// object/builders.rs, builtins/exceptions.rs, c_api.rs, etc.).
//
// Because every final binary links both molt-runtime and the extracted
// crates into a single artifact, the linker resolves these symbols
// automatically — no function-pointer tables, no traits, no init step.
// ---------------------------------------------------------------------------

pub mod ffi {
    //! Raw `extern "C"` imports from `molt-runtime`.
    //!
    //! Extracted crates can call these directly.  Safe wrappers are provided
    //! in the parent module for the most common operations.

    unsafe extern "C" {
        // -- Object allocation (c_api.rs) ------------------------------------

        /// Allocate a new string object from raw UTF-8 bytes.
        /// Returns the NaN-boxed `u64` handle (0/None on OOM).
        pub fn molt_string_from(data: *const u8, len: u64) -> u64;

        /// Allocate a new bytes object from raw data.
        /// Returns the NaN-boxed `u64` handle.
        pub fn molt_bytes_from(data: *const u8, len: u64) -> u64;

        /// Read the raw data pointer from a string handle.
        /// Writes the byte length into `*out_len`.
        pub fn molt_string_as_ptr(string_bits: u64, out_len: *mut u64) -> *const u8;

        /// Read the raw data pointer from a bytes handle.
        /// Writes the byte length into `*out_len`.
        pub fn molt_bytes_as_ptr(bytes_bits: u64, out_len: *mut u64) -> *const u8;

        /// Allocate a generic object with `size` user-data bytes.
        pub fn molt_alloc(size_bits: u64) -> u64;

        /// Allocate a tuple from a C array of NaN-boxed elements.
        pub fn molt_tuple_from_array(items: *const u64, len: u64) -> u64;

        /// Allocate a list from a C array of NaN-boxed elements.
        pub fn molt_list_from_array(items: *const u64, len: u64) -> u64;

        /// Allocate a dict from parallel key/value C arrays.
        pub fn molt_dict_from_pairs(keys: *const u64, values: *const u64, len: u64) -> u64;

        /// Allocate a new empty dict with the given capacity hint.
        pub fn molt_dict_new(capacity_bits: u64) -> u64;

        // -- Scalar constructors (c_api.rs) ----------------------------------

        /// Return the singleton `None` handle.
        pub fn molt_none() -> u64;

        /// Box an `i64` into a NaN-boxed int handle.
        pub fn molt_int_from_i64(value: i64) -> u64;

        /// Box an `f64` into a NaN-boxed float handle.
        pub fn molt_float_from_f64(value: f64) -> u64;

        // -- Reference counting (object/ops.rs) ------------------------------

        /// Increment the reference count for a NaN-boxed object.
        pub fn molt_inc_ref_obj(bits: u64);

        /// Decrement the reference count for a NaN-boxed object.
        pub fn molt_dec_ref_obj(bits: u64);

        /// Batched inc-ref: adds `count` to the refcount in one atomic op.
        /// Returns `bits` unchanged (for chaining).
        pub fn molt_inc_ref_n(bits: u64, count: u32) -> u64;

        /// Batched dec-ref: decrements `count` times (each may trigger dealloc).
        pub fn molt_dec_ref_n(bits: u64, count: u32);

        // -- Low-level pointer refcount (object/mod.rs) ----------------------

        /// Increment refcount given a raw object pointer.
        pub fn molt_inc_ref(ptr: *mut u8);

        /// Decrement refcount given a raw object pointer.
        pub fn molt_dec_ref(ptr: *mut u8);

        // -- Exception machinery (builtins/exceptions.rs) --------------------

        /// Create a new exception object.
        /// `kind_bits`: NaN-boxed string for the exception class name.
        /// `args_bits`: NaN-boxed tuple of arguments.
        pub fn molt_exception_new(kind_bits: u64, args_bits: u64) -> u64;

        /// Raise an already-constructed exception object.
        /// Records it into the current exception slot. Returns None bits.
        pub fn molt_raise(exc_bits: u64) -> u64;

        /// Returns 1 if an exception is pending, 0 otherwise.
        pub fn molt_exception_pending() -> u64;

        /// Fast-path pending check (skips GIL entry when possible).
        pub fn molt_exception_pending_fast() -> u64;

        /// Returns the currently active exception handle (or None).
        pub fn molt_exception_active() -> u64;

        /// Clears the pending exception, returning None.
        pub fn molt_exception_clear() -> u64;

        /// Returns the last recorded exception handle (for `sys.last_value`).
        pub fn molt_exception_last() -> u64;

        /// Push an exception-handler frame onto the exception stack.
        pub fn molt_exception_stack_enter() -> u64;

        /// Pop an exception-handler frame from the exception stack.
        pub fn molt_exception_stack_exit(prev_bits: u64) -> u64;

        /// Get the exception kind string from an exception handle.
        pub fn molt_exception_kind(exc_bits: u64) -> u64;

        /// Get the message string from an exception handle.
        pub fn molt_exception_message(exc_bits: u64) -> u64;

        // -- Truthiness / type introspection (object/ops.rs) -----------------

        /// Returns 1 if the value is truthy, 0 otherwise, -1 on error.
        pub fn molt_is_truthy(val: u64) -> i64;

        /// Fast truthy check for known-int values.  Zero is falsy.
        /// Falls back to `molt_is_truthy` for unexpected types.
        pub fn molt_is_truthy_int(bits: u64) -> i64;

        /// Fast truthy check for known-bool values.  `False` is falsy.
        /// Falls back to `molt_is_truthy` for unexpected types.
        pub fn molt_is_truthy_bool(bits: u64) -> i64;

        /// Returns a NaN-boxed `type` object for the given value.
        pub fn molt_type_of(val_bits: u64) -> u64;

        // -- Conversions (object/ops.rs) -------------------------------------

        /// Convert any value to its `str()` representation (NaN-boxed string).
        pub fn molt_str_from_obj(val_bits: u64) -> u64;

        /// Convert any value to its `repr()` representation (NaN-boxed string).
        pub fn molt_repr_from_obj(val_bits: u64) -> u64;

        /// Convert a value to int (like `int(x)`).
        pub fn molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64;

        /// Convert a value to float (like `float(x)`).
        pub fn molt_float_from_obj(val_bits: u64) -> u64;

        // -- Collection operations -------------------------------------------

        /// Append a value to a list. Returns None on success.
        pub fn molt_list_append(list_bits: u64, val_bits: u64) -> u64;

        /// Pop an element from a list at the given index.
        pub fn molt_list_pop(list_bits: u64, index_bits: u64) -> u64;

        /// Extend a list with elements from another iterable.
        pub fn molt_list_extend(list_bits: u64, other_bits: u64) -> u64;

        /// Dict subscript get with default.
        pub fn molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64;

        /// Dict subscript set.
        pub fn molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64;

        /// Check if a string contains a substring. Returns bool bits.
        pub fn molt_str_contains(container_bits: u64, item_bits: u64) -> u64;

        /// Check if a list contains a value. Returns bool bits.
        pub fn molt_list_contains(container_bits: u64, item_bits: u64) -> u64;

        /// Check if a dict contains a key. Returns bool bits.
        pub fn molt_dict_contains(container_bits: u64, item_bits: u64) -> u64;

        /// String equality check. Returns bool bits.
        pub fn molt_string_eq(a: u64, b: u64) -> u64;

        // -- Sequence unpacking (object/ops.rs) ------------------------------

        /// Unpack a sequence into `expected_count` elements written to `output_ptr`.
        /// Returns 0 on success, None bits on error.
        pub fn molt_unpack_sequence(
            seq_bits: u64,
            expected_count: u64,
            output_ptr: *mut u64,
        ) -> u64;

        // -- GIL release/re-acquire (concurrency/gil.rs) ----------------------

        /// Release the GIL and return an opaque token encoding the saved depth.
        /// Resolved by `molt-runtime`'s concurrency::gil module.
        pub fn molt_gil_release_guard() -> u64;

        /// Re-acquire the GIL using the token returned by `molt_gil_release_guard`.
        pub fn molt_gil_reacquire_guard(token: u64);
    }
}

// ---------------------------------------------------------------------------
// Safe wrapper helpers — ergonomic Rust API over the raw FFI.
//
// These let extracted crates write idiomatic Rust without unsafe blocks
// for the most common runtime operations.
// ---------------------------------------------------------------------------

/// Allocate a new Molt string from a Rust `&str`. Returns the NaN-boxed handle.
/// Returns `MoltObject::none().bits()` on allocation failure.
#[inline]
pub fn rt_string_from(s: &str) -> u64 {
    unsafe { ffi::molt_string_from(s.as_ptr(), s.len() as u64) }
}

/// Allocate a new Molt string from raw bytes. Returns the NaN-boxed handle.
#[inline]
pub fn rt_string_from_bytes(bytes: &[u8]) -> u64 {
    unsafe { ffi::molt_string_from(bytes.as_ptr(), bytes.len() as u64) }
}

/// Allocate a new Molt bytes object. Returns the NaN-boxed handle.
#[inline]
pub fn rt_bytes_from(bytes: &[u8]) -> u64 {
    unsafe { ffi::molt_bytes_from(bytes.as_ptr(), bytes.len() as u64) }
}

/// Return the singleton `None` handle.
#[inline]
pub fn rt_none() -> u64 {
    unsafe { ffi::molt_none() }
}

/// Box a Rust `i64` into a NaN-boxed int handle.
#[inline]
pub fn rt_int(value: i64) -> u64 {
    unsafe { ffi::molt_int_from_i64(value) }
}

/// Box a Rust `f64` into a NaN-boxed float handle.
#[inline]
pub fn rt_float(value: f64) -> u64 {
    unsafe { ffi::molt_float_from_f64(value) }
}

/// Allocate a tuple from a slice of NaN-boxed elements.
#[inline]
pub fn rt_tuple(elems: &[u64]) -> u64 {
    unsafe { ffi::molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) }
}

/// Allocate a list from a slice of NaN-boxed elements.
#[inline]
pub fn rt_list(elems: &[u64]) -> u64 {
    unsafe { ffi::molt_list_from_array(elems.as_ptr(), elems.len() as u64) }
}

/// Allocate a new empty dict with optional capacity hint.
#[inline]
pub fn rt_dict(capacity: usize) -> u64 {
    unsafe { ffi::molt_dict_new(MoltObject::from_int(capacity as i64).bits()) }
}

/// Increment the reference count for a NaN-boxed object handle.
#[inline]
pub fn rt_inc_ref(bits: u64) {
    unsafe { ffi::molt_inc_ref_obj(bits) }
}

/// Decrement the reference count for a NaN-boxed object handle.
#[inline]
pub fn rt_dec_ref(bits: u64) {
    unsafe { ffi::molt_dec_ref_obj(bits) }
}

/// Check whether an exception is currently pending.
#[inline]
pub fn rt_exception_pending() -> bool {
    unsafe { ffi::molt_exception_pending() != 0 }
}

/// Fast-path exception check (avoids full GIL entry when possible).
#[inline]
pub fn rt_exception_pending_fast() -> bool {
    unsafe { ffi::molt_exception_pending_fast() != 0 }
}

/// Raise an already-constructed exception. Returns None bits.
#[inline]
pub fn rt_raise(exc_bits: u64) -> u64 {
    unsafe { ffi::molt_raise(exc_bits) }
}

/// Create and raise an exception from kind name + message strings.
/// Returns None bits (the caller should return early on exception).
pub fn rt_raise_str(kind: &str, message: &str) -> u64 {
    let kind_bits = rt_string_from(kind);
    let msg_bits = rt_string_from(message);
    let args_bits = rt_tuple(&[msg_bits]);
    let exc_bits = unsafe { ffi::molt_exception_new(kind_bits, args_bits) };
    rt_dec_ref(kind_bits);
    rt_dec_ref(args_bits);
    unsafe { ffi::molt_raise(exc_bits) }
}

/// Clear the pending exception. Returns None bits.
#[inline]
pub fn rt_exception_clear() -> u64 {
    unsafe { ffi::molt_exception_clear() }
}

/// Check truthiness of a NaN-boxed value.
#[inline]
pub fn rt_is_truthy(bits: u64) -> bool {
    unsafe { ffi::molt_is_truthy(bits) == 1 }
}

/// Get the `str()` of a NaN-boxed value. Returns a NaN-boxed string handle.
#[inline]
pub fn rt_str(bits: u64) -> u64 {
    unsafe { ffi::molt_str_from_obj(bits) }
}

/// Get the `repr()` of a NaN-boxed value. Returns a NaN-boxed string handle.
#[inline]
pub fn rt_repr(bits: u64) -> u64 {
    unsafe { ffi::molt_repr_from_obj(bits) }
}

/// Read the UTF-8 bytes from a Molt string handle.
///
/// Returns `None` if the handle is not a valid string or if the pointer is null.
pub fn rt_string_as_bytes(string_bits: u64) -> Option<&'static [u8]> {
    let mut len: u64 = 0;
    let ptr = unsafe { ffi::molt_string_as_ptr(string_bits, &mut len) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr, len as usize) })
}

/// Read the raw bytes from a Molt bytes handle.
///
/// Returns `None` if the handle is not a valid bytes object or if the pointer is null.
pub fn rt_bytes_as_slice(bytes_bits: u64) -> Option<&'static [u8]> {
    let mut len: u64 = 0;
    let ptr = unsafe { ffi::molt_bytes_as_ptr(bytes_bits, &mut len) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr, len as usize) })
}

// ---------------------------------------------------------------------------
// Prelude — single glob-import for extracted crates
// ---------------------------------------------------------------------------

/// Prelude for extracted stdlib crates.
///
/// Provides type IDs, object model types, the GIL token, convenience
/// helpers, and safe wrappers over the runtime FFI.
pub mod prelude {
    pub use crate::type_ids::*;
    pub use crate::with_core_gil;
    pub use crate::with_gil_entry;
    pub use crate::{
        bits_from_ptr, obj_from_bits, ptr_from_bits, CoreGilGuard, CoreGilToken, GilReleaseGuard,
        MoltObject, PyToken,
    };

    // Safe runtime wrappers
    pub use crate::{
        rt_bytes_as_slice, rt_bytes_from, rt_dec_ref, rt_dict, rt_exception_clear,
        rt_exception_pending, rt_exception_pending_fast, rt_float, rt_inc_ref, rt_int,
        rt_is_truthy, rt_list, rt_none, rt_raise, rt_raise_str, rt_repr, rt_str,
        rt_string_as_bytes, rt_string_from, rt_string_from_bytes, rt_tuple,
    };
}

// ---------------------------------------------------------------------------
// RuntimeVtable — single struct replacing 58 individual extern "C" FFI
// bridge functions between molt-runtime and molt-runtime-serial.
// ---------------------------------------------------------------------------

/// Function-pointer table for the runtime → serial bridge.
///
/// Instead of 58 individual `extern "C"` imports, extracted crates receive a
/// single `&'static RuntimeVtable` whose fields are filled in at init time by
/// `molt-runtime`.  Every pointer uses an FFI-safe `extern "C"` signature so
/// the struct can be shared across dynamic-library boundaries.
#[repr(C)]
pub struct RuntimeVtable {
    // --- Exception handling ---
    pub raise_exception: unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u64,
    pub exception_pending: unsafe extern "C" fn() -> i32,

    // --- Allocation ---
    pub alloc_tuple: unsafe extern "C" fn(*const u64, usize) -> *mut u8,
    pub alloc_list: unsafe extern "C" fn(*const u64, usize) -> *mut u8,
    pub alloc_string: unsafe extern "C" fn(*const u8, usize) -> *mut u8,
    pub alloc_bytes: unsafe extern "C" fn(*const u8, usize) -> *mut u8,
    pub alloc_dict_with_pairs: unsafe extern "C" fn(*const u64, usize) -> *mut u8,

    // --- Object inspection ---
    pub object_type_id: unsafe extern "C" fn(*mut u8) -> u32,
    pub string_obj_to_owned: unsafe extern "C" fn(u64, *mut *const u8, *mut usize) -> i32,
    pub type_name: unsafe extern "C" fn(u64, *mut *const u8, *mut usize) -> i32,
    pub is_truthy: unsafe extern "C" fn(u64) -> i32,
    pub bytes_like_slice: unsafe extern "C" fn(*mut u8, *mut *const u8, *mut usize) -> i32,
    pub string_bytes: unsafe extern "C" fn(*mut u8, *mut *const u8, *mut usize) -> i32,
    pub string_len: unsafe extern "C" fn(*mut u8) -> usize,
    pub bytes_like_slice_raw: unsafe extern "C" fn(*mut u8, *mut *const u8, *mut usize) -> i32,
    pub format_obj: unsafe extern "C" fn(u64, *mut *const u8, *mut usize) -> i32,
    pub format_obj_str: unsafe extern "C" fn(u64, *mut *const u8, *mut usize) -> i32,
    pub class_name_for_error: unsafe extern "C" fn(u64, *mut *const u8, *mut usize) -> i32,
    pub type_of_bits: unsafe extern "C" fn(u64) -> u64,
    pub maybe_ptr_from_bits: unsafe extern "C" fn(u64) -> *mut u8,
    pub molt_is_callable: unsafe extern "C" fn(u64) -> i32,

    // --- Memoryview ---
    pub memoryview_is_c_contiguous_view: unsafe extern "C" fn(*mut u8) -> i32,
    pub memoryview_readonly: unsafe extern "C" fn(*mut u8) -> i32,
    pub memoryview_nbytes: unsafe extern "C" fn(*mut u8) -> usize,
    pub memoryview_offset: unsafe extern "C" fn(*mut u8) -> isize,
    pub memoryview_owner_bits: unsafe extern "C" fn(*mut u8) -> u64,

    // --- Reference counting ---
    pub release_ptr: unsafe extern "C" fn(*mut u8),
    pub dec_ref_bits: unsafe extern "C" fn(u64),
    pub inc_ref_bits: unsafe extern "C" fn(u64),

    // --- Numerics ---
    pub to_i64: unsafe extern "C" fn(u64, *mut i64) -> i32,
    pub to_f64: unsafe extern "C" fn(u64, *mut f64) -> i32,
    pub to_bigint: unsafe extern "C" fn(u64, *mut i32, *mut *const u8, *mut usize) -> i32,
    pub int_bits_from_i64: unsafe extern "C" fn(i64) -> u64,
    pub int_bits_from_i128: unsafe extern "C" fn(u64, u64) -> u64,
    pub int_bits_from_bigint: unsafe extern "C" fn(i32, *const u8, usize) -> u64,
    pub bigint_ptr_from_bits: unsafe extern "C" fn(u64) -> *mut u8,
    pub bigint_ref: unsafe extern "C" fn(*mut u8, *mut i32, *mut *const u8, *mut usize) -> i32,
    pub bigint_from_f64_trunc:
        unsafe extern "C" fn(f64, *mut i32, *mut *const u8, *mut usize) -> i32,
    pub bigint_bits: unsafe extern "C" fn(i32, *const u8, usize) -> u64,
    pub bigint_to_inline: unsafe extern "C" fn(i32, *const u8, usize) -> u64,
    pub index_i64_from_obj: unsafe extern "C" fn(u64, *const u8, usize) -> i64,
    pub index_bigint_from_obj:
        unsafe extern "C" fn(u64, *const u8, usize, *mut i32, *mut *const u8, *mut usize) -> i32,

    // --- Callable / protocol ---
    pub call_callable0: unsafe extern "C" fn(u64) -> u64,
    pub call_callable2: unsafe extern "C" fn(u64, u64, u64) -> u64,
    pub attr_lookup_ptr_allow_missing: unsafe extern "C" fn(*mut u8, u64) -> u64,
    pub intern_static_name: unsafe extern "C" fn(*const u8, usize) -> u64,

    // --- Container helpers ---
    pub bytearray_vec: unsafe extern "C" fn(*mut u8) -> *mut Vec<u8>,
    pub dict_get_in_place: unsafe extern "C" fn(*mut u8, u64, *mut u64) -> i32,
    pub dict_set_in_place: unsafe extern "C" fn(*mut u8, u64, u64) -> i32,
    pub list_len: unsafe extern "C" fn(*mut u8) -> usize,
    pub seq_vec_ptr: unsafe extern "C" fn(*mut u8) -> *mut Vec<u64>,
    pub dict_order_clone: unsafe extern "C" fn(*mut u8, *mut *const u64, *mut usize) -> i32,

    // --- Iteration ---
    pub molt_iter: unsafe extern "C" fn(u64) -> u64,
    pub molt_iter_next: unsafe extern "C" fn(u64, *mut u64) -> i32,
    pub raise_not_iterable: unsafe extern "C" fn(u64) -> u64,
    pub molt_sorted_builtin: unsafe extern "C" fn(u64) -> u64,
    pub molt_mul: unsafe extern "C" fn(u64, u64) -> u64,

    // --- OS ---
    pub fill_os_random: unsafe extern "C" fn(*mut u8, usize) -> i32,

    // --- Extended helpers (email / zipfile / decimal) ---
    pub alloc_list_with_capacity: unsafe extern "C" fn(*const u64, usize, usize) -> *mut u8,
    pub attr_name_bits_from_bytes: unsafe extern "C" fn(*const u8, usize, *mut u64) -> i32,
    pub call_class_init_with_args: unsafe extern "C" fn(*mut u8, *const u64, usize) -> u64,
    pub missing_bits: unsafe extern "C" fn() -> u64,
    pub molt_getattr_builtin: unsafe extern "C" fn(u64, u64, u64) -> u64,
    pub molt_module_import: unsafe extern "C" fn(u64) -> u64,
}

// SAFETY: The vtable is populated once at init time and then only read.
// All function pointers are plain `extern "C"` fn pointers (Send + Sync).
unsafe impl Send for RuntimeVtable {}
unsafe impl Sync for RuntimeVtable {}
