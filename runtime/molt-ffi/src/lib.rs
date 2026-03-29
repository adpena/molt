//! Stable C API for Molt runtime intrinsics.
//!
//! This crate exposes Molt's stdlib functions via `extern "C"` wrappers,
//! enabling external runtimes (e.g., Monty) to call Molt's 327 stdlib
//! modules without reimplementing them.
//!
//! # Architecture
//!
//! ```text
//! Monty VM → yield OsCall("json.loads", args)
//!     → Host → molt_ffi_json_loads(args)  // This crate
//!     → Molt runtime → jiter-backed JSON parsing
//!     → Resume with result
//! ```
//!
//! # Conventions
//!
//! - All functions use `extern "C"` ABI
//! - All values are NaN-boxed `u64` (Molt's object representation)
//! - Error returns use the runtime's exception mechanism
//! - Functions are prefixed with `molt_ffi_` to avoid symbol conflicts
//! - The runtime must be initialized via `molt_ffi_init()` before any calls
//!
//! # Status
//!
//! This is a scaffold defining the API surface. Implementation will wrap
//! existing `molt_*` functions from `molt-runtime` with stable C signatures
//! and documentation.

/// Initialize the Molt runtime for FFI use.
///
/// Must be called once before any other `molt_ffi_*` function.
/// Sets up the RuntimeState, GIL, and thread-local storage.
///
/// # Safety
///
/// Must be called from the main thread before spawning any threads
/// that call `molt_ffi_*` functions.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_init() {
    // Will call molt_runtime_init() + molt_runtime_init_resources() etc.
}

/// Shut down the Molt runtime.
///
/// Must be called after all FFI work is complete. Flushes audit sinks,
/// frees the RuntimeState, and releases resources.
///
/// # Safety
///
/// No `molt_ffi_*` calls may be made after this returns.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_shutdown() {
    // Will call molt_runtime_shutdown()
}

/// Get the FFI API version.
///
/// Returns a version number that callers can check for compatibility.
/// The version increments on any breaking API change.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_version() -> u32 {
    1
}

// ── JSON module ────────────────────────────────────────────────────

/// Parse a JSON string into a Molt object.
///
/// # Arguments
/// - `json_str_bits`: NaN-boxed pointer to a Molt string object containing JSON
///
/// # Returns
/// - NaN-boxed Molt object (dict, list, str, int, float, bool, None)
/// - On error: sets pending exception and returns None bits
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_json_loads(_json_str_bits: u64) -> u64 {
    // Will delegate to molt_json_loads in the runtime
    0 // placeholder
}

/// Serialize a Molt object to a JSON string.
///
/// # Arguments
/// - `obj_bits`: NaN-boxed Molt object to serialize
///
/// # Returns
/// - NaN-boxed pointer to a Molt string containing JSON
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_json_dumps(_obj_bits: u64) -> u64 {
    0 // placeholder
}

// ── Math module ────────────────────────────────────────────────────

/// Compute the square root of a float.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_sqrt(_x_bits: u64) -> u64 {
    0 // placeholder
}

/// Compute the absolute value.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_fabs(_x_bits: u64) -> u64 {
    0 // placeholder
}

// ── String utilities ───────────────────────────────────────────────

/// Get the length of a string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_len(_obj_bits: u64) -> u64 {
    0 // placeholder
}

/// Convert an object to its string representation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_str(_obj_bits: u64) -> u64 {
    0 // placeholder
}

/// Convert an object to its repr() string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_repr(_obj_bits: u64) -> u64 {
    0 // placeholder
}

// ── Capabilities ───────────────────────────────────────────────────

/// Check if a capability is granted.
///
/// # Arguments
/// - `cap_name_bits`: NaN-boxed pointer to a capability name string
///
/// # Returns
/// - 1 if the capability is granted, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_has_capability(_cap_name_bits: u64) -> u64 {
    0 // placeholder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_version_returns_one() {
        assert_eq!(molt_ffi_version(), 1);
    }

    #[test]
    fn ffi_placeholders_return_zero() {
        // Placeholders return 0 (None bits) until wired to runtime
        assert_eq!(molt_ffi_json_loads(0), 0);
        assert_eq!(molt_ffi_json_dumps(0), 0);
        assert_eq!(molt_ffi_math_sqrt(0), 0);
        assert_eq!(molt_ffi_len(0), 0);
    }
}
