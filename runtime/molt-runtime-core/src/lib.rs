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

/// Execute a body while "holding the GIL".
///
/// Stub implementation: binds a [`PyToken`] and runs the body immediately.
/// The real version in `molt-runtime` actually acquires the GIL.
#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:expr) => {{
        let $py = $crate::PyToken::new();
        $body
    }};
}

// ---------------------------------------------------------------------------
// Prelude — single glob-import for extracted crates
// ---------------------------------------------------------------------------

/// Prelude for extracted stdlib crates.
pub mod prelude {
    pub use crate::type_ids::*;
    pub use crate::{
        bits_from_ptr, obj_from_bits, ptr_from_bits,
        MoltObject, PyToken,
    };
    pub use crate::with_gil_entry;
}
