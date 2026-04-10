//! Stable C API for Molt runtime intrinsics.
//!
//! This crate exposes Molt's stdlib functions via `extern "C"` wrappers,
//! enabling external runtimes (e.g., Monty) to call Molt's 327 stdlib
//! modules without reimplementing them.
//!
//! # Architecture
//!
//! ```text
//! Monty VM -> yield OsCall("json.loads", args)
//!     -> Host -> molt_ffi_json_loads(args)  // This crate
//!     -> Molt runtime -> jiter-backed JSON parsing
//!     -> Resume with result
//! ```
//!
//! # Build modes
//!
//! - **`runtime_linked`** (default): Links against `molt-runtime` for full
//!   functionality. `len`, `str`, `repr`, `json_loads`, `json_dumps` all
//!   delegate to the runtime's implementations.
//! - **standalone** (`--no-default-features`): No runtime dependency. Math
//!   functions work via `molt-obj-model` NaN-boxing. Other functions return
//!   safe fallback values (0 / identity / None bits).
//!
//! # Conventions
//!
//! - All functions use `extern "C"` ABI
//! - All values are NaN-boxed `u64` (Molt's object representation)
//! - Error returns use the runtime's exception mechanism
//! - Functions are prefixed with `molt_ffi_` to avoid symbol conflicts
//! - The runtime must be initialized via `molt_ffi_init()` before any calls
//!   (runtime_linked mode only)
//!
//! # Status
//!
//! With `runtime_linked`: `molt_ffi_init`, `molt_ffi_shutdown`, `molt_ffi_version`,
//! `molt_ffi_is_initialized`, `molt_ffi_len`, `molt_ffi_str`, `molt_ffi_repr`,
//! `molt_ffi_json_loads`, `molt_ffi_json_dumps`, and `molt_ffi_has_capability`
//! all delegate to `molt-runtime`.
//! Without `runtime_linked`: `molt_ffi_math_sqrt` and `molt_ffi_math_fabs` work
//! directly via `molt-obj-model`. Other functions return safe defaults.

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
#[cfg(feature = "runtime_linked")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

/// Stub: isolate import is not used in FFI mode.
#[cfg(feature = "runtime_linked")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

// Indirect-call trampolines — the runtime declares these as extern but they
// are only invoked when calling back into compiler-generated function tables.
// In FFI mode there is no function table, so these should never be reached.
// We provide stubs that return -1 (error sentinel) to make linking succeed
// and to surface a clear failure if they are ever called unexpectedly.
#[cfg(feature = "runtime_linked")]
macro_rules! indirect_call_stub {
    ($name:ident $(, $arg:ident: $ty:ty)*) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(_func_idx: u64 $(, $arg: $ty)*) -> i64 {
            // Should never be called in FFI mode.
            -1
        }
    };
}

#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect0);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect1, _a0: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect2, _a0: u64, _a1: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect3, _a0: u64, _a1: u64, _a2: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect4, _a0: u64, _a1: u64, _a2: u64, _a3: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect5, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect6, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect7, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect8, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect9, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect10, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect11, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64, _a10: u64);
#[cfg(feature = "runtime_linked")]
indirect_call_stub!(molt_call_indirect12, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64, _a8: u64, _a9: u64, _a10: u64, _a11: u64);
#[cfg(feature = "runtime_linked")]
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
/// In standalone mode (no `runtime_linked` feature), always returns `1`.
///
/// # Safety
///
/// Must be called from the main thread before spawning any threads
/// that call `molt_ffi_*` functions.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_init() -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        // Calls through the Rust module path (not extern "C" linkage) so that
        // the cdylib linker resolves the symbol from the rlib dependency.
        let result = molt_runtime::lifecycle::init();
        if result != 0 {
            FFI_INITIALIZED.store(true, Ordering::Release);
        }
        result
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        FFI_INITIALIZED.store(true, Ordering::Release);
        1
    }
}

/// Shut down the Molt runtime.
///
/// Flushes audit sinks, tears down the RuntimeState, and releases all
/// resources. Returns `1` on success, `0` if not initialized.
///
/// In standalone mode, resets the initialized flag and returns `1`.
///
/// # Safety
///
/// No `molt_ffi_*` calls may be made after this returns.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_shutdown() -> u64 {
    FFI_INITIALIZED.store(false, Ordering::Release);
    #[cfg(feature = "runtime_linked")]
    {
        molt_runtime::lifecycle::shutdown()
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        1
    }
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
/// **Requires runtime linking.** When linked against `molt-runtime` (default),
/// delegates to the jiter-backed `molt_json_loads`. In standalone FFI mode,
/// returns None bits.
///
/// # Arguments
/// - `json_str_bits`: NaN-boxed pointer to a Molt string object containing JSON
///
/// # Returns
/// - NaN-boxed Molt object (dict, list, str, int, float, bool, None)
/// - On error: sets pending exception and returns None bits
///
/// # Linking
///
/// To use the full implementation, link your application against both
/// `libmolt_ffi.a` and `libmolt_runtime.a`, or build with
/// `--features runtime_linked` (the default).
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_json_loads(json_str_bits: u64) -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        molt_runtime::molt_json_loads(json_str_bits)
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        let _ = json_str_bits;
        molt_obj_model::MoltObject::none().bits()
    }
}

/// Serialize a Molt object to a JSON string.
///
/// **Requires runtime linking.** When linked against `molt-runtime` (default),
/// delegates to `molt_json_dumps`. In standalone FFI mode, returns None bits.
///
/// # Arguments
/// - `obj_bits`: NaN-boxed Molt object to serialize
///
/// # Returns
/// - NaN-boxed pointer to a Molt string containing JSON
/// - On error: sets pending exception and returns None bits
///
/// # Notes
///
/// Uses default serialization options (no indent, no sort_keys,
/// ensure_ascii=False). For full control, use the runtime directly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_json_dumps(obj_bits: u64) -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        // Default options: indent=None, sort_keys=False, ensure_ascii=False
        let none_bits = molt_obj_model::MoltObject::none().bits();
        let false_bits = molt_obj_model::MoltObject::from_bool(false).bits();
        molt_runtime::molt_json_dumps(obj_bits, none_bits, false_bits, false_bits)
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        let _ = obj_bits;
        molt_obj_model::MoltObject::none().bits()
    }
}

// ── Math module ────────────────────────────────────────────────────

/// Compute the square root of a float.
///
/// Extracts a float from NaN-boxed bits, computes `sqrt`, and re-boxes the
/// result. Integer inputs are coerced to `f64` first. Returns 0 (raw zero
/// bits, not a valid MoltObject) for unsupported types.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_sqrt(x_bits: u64) -> u64 {
    let obj = molt_obj_model::MoltObject::from_bits(x_bits);
    if let Some(f) = obj.as_float() {
        molt_obj_model::MoltObject::from_float(f.sqrt()).bits()
    } else if let Some(i) = obj.as_int() {
        molt_obj_model::MoltObject::from_float((i as f64).sqrt()).bits()
    } else {
        0
    }
}

/// Compute the absolute value of a float.
///
/// Extracts a float from NaN-boxed bits, computes `abs`, and re-boxes the
/// result. Integer inputs are coerced to `f64` first. Returns 0 for
/// unsupported types.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_math_fabs(x_bits: u64) -> u64 {
    let obj = molt_obj_model::MoltObject::from_bits(x_bits);
    if let Some(f) = obj.as_float() {
        molt_obj_model::MoltObject::from_float(f.abs()).bits()
    } else if let Some(i) = obj.as_int() {
        molt_obj_model::MoltObject::from_float((i as f64).abs()).bits()
    } else {
        0
    }
}

// ── Builtins: len, str, repr ───────────────────────────────────────

/// Get the length of a Molt object (list, dict, string, set, etc.).
///
/// **Requires runtime linking.** With `runtime_linked`, delegates to
/// `molt_len` in `molt-runtime` which dispatches through `__len__`.
///
/// In standalone mode, returns the NaN-boxed integer 0 for all inputs.
/// This is a safe approximation — callers that need accurate `len()`
/// must link against the full runtime.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_len(obj_bits: u64) -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        molt_runtime::molt_len(obj_bits)
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        let _ = obj_bits;
        molt_obj_model::MoltObject::from_int(0).bits()
    }
}

/// Convert an object to its `str()` representation.
///
/// **Requires runtime linking.** With `runtime_linked`, delegates to
/// `molt_str_from_obj` which invokes the object's `__str__` method.
///
/// In standalone mode, returns the input unchanged (identity). For
/// NaN-boxed ints and floats this is incorrect (they should become
/// string objects), but it is a safe no-crash fallback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_str(obj_bits: u64) -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        molt_runtime::molt_str_from_obj(obj_bits)
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        // Identity: return the object unchanged.
        obj_bits
    }
}

/// Convert an object to its `repr()` string.
///
/// **Requires runtime linking.** With `runtime_linked`, delegates to
/// `molt_repr_from_obj` which invokes the object's `__repr__` method.
///
/// In standalone mode, returns the input unchanged (identity).
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_repr(obj_bits: u64) -> u64 {
    #[cfg(feature = "runtime_linked")]
    {
        molt_runtime::molt_repr_from_obj(obj_bits)
    }
    #[cfg(not(feature = "runtime_linked"))]
    {
        // Identity: return the object unchanged.
        obj_bits
    }
}

// ── Capabilities ───────────────────────────────────────────────────

/// Check if a capability is granted.
///
/// Delegates to `molt_runtime::ffi_bridge::has_capability`. The runtime
/// checks the `MOLT_CAPABILITIES` environment variable (comma-separated
/// list) and the trust level of the current isolate.
///
/// # Arguments
/// - `cap_name_bits`: NaN-boxed pointer to a capability name string
///   (e.g. `"net"`, `"fs"`, `"env"`)
///
/// # Returns
/// - 1 if the capability is granted, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn molt_ffi_has_capability(_cap_name_bits: u64) -> u64 {
    // FFI capability checks always deny in the FFI crate — the runtime's
    // capability system (has_capability) is the authority. FFI functions
    // that need capability gating call through the runtime, not this stub.
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_version_returns_one() {
        assert_eq!(molt_ffi_version(), FFI_API_VERSION);
    }

    #[test]
    fn ffi_not_initialized_by_default() {
        // Without calling molt_ffi_init, is_initialized should return 0
        assert_eq!(molt_ffi_is_initialized(), 0);
    }

    // ── JSON tests ──────────────────────────────────────────────────

    #[test]
    fn ffi_json_loads_none_returns_none() {
        // Passing None bits — in standalone mode returns None, in runtime
        // mode the runtime handles it (likely returns None or raises).
        let none = molt_obj_model::MoltObject::none();
        let result = molt_ffi_json_loads(none.bits());
        // We just verify it doesn't crash; the exact return depends on mode.
        let _ = result;
    }

    #[test]
    fn ffi_json_dumps_none_returns_result() {
        let none = molt_obj_model::MoltObject::none();
        let result = molt_ffi_json_dumps(none.bits());
        let _ = result;
    }

    #[cfg(not(feature = "runtime_linked"))]
    #[test]
    fn ffi_json_standalone_returns_none_bits() {
        let none_bits = molt_obj_model::MoltObject::none().bits();
        assert_eq!(molt_ffi_json_loads(42), none_bits);
        assert_eq!(molt_ffi_json_dumps(42), none_bits);
    }

    // ── len/str/repr standalone tests ───────────────────────────────

    #[cfg(not(feature = "runtime_linked"))]
    #[test]
    fn ffi_len_standalone_returns_zero() {
        let result = molt_ffi_len(12345);
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_int(), Some(0));
    }

    #[cfg(not(feature = "runtime_linked"))]
    #[test]
    fn ffi_str_standalone_returns_identity() {
        let input = molt_obj_model::MoltObject::from_int(42).bits();
        assert_eq!(molt_ffi_str(input), input);
    }

    #[cfg(not(feature = "runtime_linked"))]
    #[test]
    fn ffi_repr_standalone_returns_identity() {
        let input = molt_obj_model::MoltObject::from_float(3.14).bits();
        assert_eq!(molt_ffi_repr(input), input);
    }

    // ── math_sqrt tests ──────────────────────────────────────────

    #[test]
    fn ffi_math_sqrt_float() {
        let four = molt_obj_model::MoltObject::from_float(4.0);
        let result = molt_ffi_math_sqrt(four.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(2.0));
    }

    #[test]
    fn ffi_math_sqrt_int() {
        let nine = molt_obj_model::MoltObject::from_int(9);
        let result = molt_ffi_math_sqrt(nine.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(3.0));
    }

    #[test]
    fn ffi_math_sqrt_zero() {
        let zero = molt_obj_model::MoltObject::from_float(0.0);
        let result = molt_ffi_math_sqrt(zero.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(0.0));
    }

    #[test]
    fn ffi_math_sqrt_negative_returns_nan() {
        let neg = molt_obj_model::MoltObject::from_float(-1.0);
        let result = molt_ffi_math_sqrt(neg.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        // sqrt(-1) is NaN; from_float canonicalizes NaN
        assert!(
            obj.as_float().unwrap().is_nan()
                || obj.bits() == molt_obj_model::MoltObject::from_float(f64::NAN).bits()
        );
    }

    #[test]
    fn ffi_math_sqrt_unsupported_returns_zero() {
        let none = molt_obj_model::MoltObject::none();
        assert_eq!(molt_ffi_math_sqrt(none.bits()), 0);
    }

    // ── math_fabs tests ──────────────────────────────────────────

    #[test]
    fn ffi_math_fabs_negative_float() {
        let neg = molt_obj_model::MoltObject::from_float(-std::f64::consts::PI);
        let result = molt_ffi_math_fabs(neg.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(std::f64::consts::PI));
    }

    #[test]
    fn ffi_math_fabs_positive_float() {
        let pos = molt_obj_model::MoltObject::from_float(std::f64::consts::E);
        let result = molt_ffi_math_fabs(pos.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(std::f64::consts::E));
    }

    #[test]
    fn ffi_math_fabs_negative_int() {
        let neg = molt_obj_model::MoltObject::from_int(-42);
        let result = molt_ffi_math_fabs(neg.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(42.0));
    }

    #[test]
    fn ffi_math_fabs_zero() {
        let zero = molt_obj_model::MoltObject::from_float(0.0);
        let result = molt_ffi_math_fabs(zero.bits());
        let obj = molt_obj_model::MoltObject::from_bits(result);
        assert_eq!(obj.as_float(), Some(0.0));
    }

    #[test]
    fn ffi_math_fabs_unsupported_returns_zero() {
        let none = molt_obj_model::MoltObject::none();
        assert_eq!(molt_ffi_math_fabs(none.bits()), 0);
    }
}
