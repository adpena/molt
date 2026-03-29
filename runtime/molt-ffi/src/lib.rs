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
//! `molt_ffi_init`, `molt_ffi_shutdown`, and `molt_ffi_version` are wired
//! to real runtime intrinsics. Remaining functions are placeholders that
//! will wrap existing `molt_*` functions from `molt-runtime`.

use std::sync::atomic::{AtomicBool, Ordering};

/// The FFI API version. Increment on any breaking API change.
const FFI_API_VERSION: u32 = 1;

/// Tracks whether `molt_ffi_init` has been called successfully.
static FFI_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Linker stubs ───────────────────────────────────────────────────
//
// `molt-runtime` declares several `extern "C"` symbols that are normally
// provided by the compiler-generated WASM module (isolate entrypoints and
// indirect-call trampolines). When building `molt-ffi` as a cdylib/staticlib,
// the linker needs concrete definitions. These stubs return safe no-op values.

/// Stub: isolate bootstrap is not used in FFI mode.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

/// Stub: isolate import is not used in FFI mode.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

// Indirect-call trampolines — the runtime declares these as extern but they
// are only invoked when calling back into compiler-generated function tables.
// In FFI mode there is no function table, so these should never be reached.
// We provide stubs that return -1 (error sentinel) to make linking succeed
// and to surface a clear failure if they are ever called unexpectedly.
macro_rules! indirect_call_stub {
    ($name:ident $(, $arg:ident: $ty:ty)*) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(_func_idx: u64 $(, $arg: $ty)*) -> i64 {
            // Should never be called in FFI mode.
            -1
        }
    };
}

indirect_call_stub!(molt_call_indirect0);
indirect_call_stub!(molt_call_indirect1, _a0: u64);
indirect_call_stub!(molt_call_indirect2, _a0: u64, _a1: u64);
indirect_call_stub!(molt_call_indirect3, _a0: u64, _a1: u64, _a2: u64);
indirect_call_stub!(molt_call_indirect4, _a0: u64, _a1: u64, _a2: u64, _a3: u64);
indirect_call_stub!(molt_call_indirect5, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64);
indirect_call_stub!(molt_call_indirect6, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64);
indirect_call_stub!(molt_call_indirect7, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64);
indirect_call_stub!(molt_call_indirect8, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64);
indirect_call_stub!(molt_call_indirect9, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64);
indirect_call_stub!(molt_call_indirect10, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64);
indirect_call_stub!(molt_call_indirect11, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64, _a10: u64);
indirect_call_stub!(molt_call_indirect12, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64, _a10: u64, _a11: u64);
indirect_call_stub!(molt_call_indirect13, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64, _a10: u64, _a11: u64, _a12: u64);

// ── Public FFI API ─────────────────────────────────────────────────

/// Initialize the Molt runtime for FFI use.
///
/// Must be called once before any other `molt_ffi_*` function.
/// Sets up the RuntimeState, GIL, thread-local storage, resource limits,
/// audit sink, and IO mode from environment variables.
///
/// Returns `1` on success, `0` if shutdown has already occurred, and `1`
/// (idempotent) if already initialized.
///
/// # Safety
///
/// Must be called from the main thread before spawning any threads
/// that call `molt_ffi_*` functions.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_init() -> u64 {
    // Calls through the Rust module path (not extern "C" linkage) so that
    // the cdylib linker resolves the symbol from the rlib dependency.
    let result = molt_runtime::lifecycle::init();
    if result != 0 {
        FFI_INITIALIZED.store(true, Ordering::Release);
    }
    result
}

/// Shut down the Molt runtime.
///
/// Flushes audit sinks, tears down the RuntimeState, and releases all
/// resources. Returns `1` on success, `0` if not initialized.
///
/// # Safety
///
/// No `molt_ffi_*` calls may be made after this returns.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_shutdown() -> u64 {
    FFI_INITIALIZED.store(false, Ordering::Release);
    molt_runtime::lifecycle::shutdown()
}

/// Get the FFI API version.
///
/// Returns a version number that callers can check for compatibility.
/// The version increments on any breaking API change.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_version() -> u32 {
    FFI_API_VERSION
}

/// Check whether the FFI runtime has been initialized.
///
/// Returns `1` if `molt_ffi_init` has been called and `molt_ffi_shutdown`
/// has not yet been called, `0` otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_is_initialized() -> u32 {
    FFI_INITIALIZED.load(Ordering::Acquire) as u32
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
    // TODO: delegate to molt_json_loads in the runtime
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
    // TODO: delegate to molt_json_dumps in the runtime
    0 // placeholder
}

// ── Math module ────────────────────────────────────────────────────

/// Compute the square root of a float.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_sqrt(_x_bits: u64) -> u64 {
    // TODO: delegate to molt_math_sqrt in the runtime
    0 // placeholder
}

/// Compute the absolute value.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_fabs(_x_bits: u64) -> u64 {
    // TODO: delegate to molt_math_fabs in the runtime
    0 // placeholder
}

// ── String utilities ───────────────────────────────────────────────

/// Get the length of a string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_len(_obj_bits: u64) -> u64 {
    // TODO: delegate to molt_builtin_len in the runtime
    0 // placeholder
}

/// Convert an object to its string representation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_str(_obj_bits: u64) -> u64 {
    // TODO: delegate to molt_builtin_str in the runtime
    0 // placeholder
}

/// Convert an object to its repr() string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_repr(_obj_bits: u64) -> u64 {
    // TODO: delegate to molt_builtin_repr in the runtime
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
    // TODO: delegate to molt_capability_check in the runtime
    0 // placeholder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_version_returns_one() {
        assert_eq!(molt_ffi_version(), FFI_API_VERSION);
    }

    #[test]
    fn ffi_placeholders_return_zero() {
        // Placeholders return 0 (None bits) until wired to runtime
        assert_eq!(molt_ffi_json_loads(0), 0);
        assert_eq!(molt_ffi_json_dumps(0), 0);
        assert_eq!(molt_ffi_math_sqrt(0), 0);
        assert_eq!(molt_ffi_len(0), 0);
    }

    #[test]
    fn ffi_not_initialized_by_default() {
        // Without calling molt_ffi_init, is_initialized should return 0
        assert_eq!(molt_ffi_is_initialized(), 0);
    }
}
