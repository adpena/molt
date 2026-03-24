//! Runtime support for deoptimization (speculative optimization bailout).

use std::collections::HashMap;
use std::sync::OnceLock;

/// Global function registry: maps function names to callable function pointers.
/// Populated at runtime when compiled functions are loaded into the module.
static FUNCTION_REGISTRY: OnceLock<std::sync::Mutex<HashMap<String, extern "C" fn(u64) -> u64>>> =
    OnceLock::new();

fn registry() -> &'static std::sync::Mutex<HashMap<String, extern "C" fn(u64) -> u64>> {
    FUNCTION_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Register a compiled function for deopt fallback lookup.
///
/// Called by the loader when a compiled module is installed. The `name_ptr` /
/// `name_len` pair must point to a valid UTF-8 byte slice that remains live
/// for the duration of the program (typically a static string embedded in the
/// compiled code object).
#[unsafe(no_mangle)]
pub extern "C" fn molt_register_function(
    name_ptr: *const u8,
    name_len: usize,
    func_ptr: extern "C" fn(u64) -> u64,
) {
    // Safety: name_ptr points to a static UTF-8 byte slice supplied by the
    // compiled code object. The caller guarantees lifetime and alignment.
    let name = unsafe {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
    };
    registry().lock().unwrap().insert(name.to_string(), func_ptr);
}

/// Deoptimization state — captures live values at the deopt point
/// for transfer to the generic (unoptimized) function version.
#[repr(C)]
pub struct DeoptState {
    /// Function name of the generic fallback to invoke.
    pub fallback_name_ptr: *const u8,
    pub fallback_name_len: usize,
    /// Number of live values captured.
    pub num_values: usize,
    /// Array of NaN-boxed live values.
    pub values: [u64; 32], // max 32 live values at a deopt point
}

impl DeoptState {
    pub fn new() -> Self {
        Self {
            fallback_name_ptr: std::ptr::null(),
            fallback_name_len: 0,
            num_values: 0,
            values: [0u64; 32],
        }
    }
}

/// Runtime deopt transfer entry point.
/// Called by compiled code when a type guard or speculation fails.
/// Captures live values and transfers control to the generic version.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deopt_transfer(state: *const DeoptState) -> u64 {
    // Safety: state is allocated by the compiled code on the stack.
    if state.is_null() {
        return 0; // null state = no fallback, return None sentinel
    }
    let state = unsafe { &*state };

    DEOPT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Resolve the fallback function name from the state pointer.
    let name = if !state.fallback_name_ptr.is_null() && state.fallback_name_len > 0 {
        // Safety: fallback_name_ptr and fallback_name_len are set by compiled
        // code from a static string slice embedded in the code object. The
        // pointer remains valid for the lifetime of the program.
        unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                state.fallback_name_ptr,
                state.fallback_name_len,
            ))
        }
    } else {
        #[cfg(debug_assertions)]
        eprintln!("[DEOPT] no fallback name — returning None sentinel");
        return 0;
    };

    #[cfg(debug_assertions)]
    eprintln!(
        "[DEOPT] Transferring to {} with {} live values",
        name, state.num_values
    );

    // Look up the fallback function in the global registry and call it.
    if let Some(fallback) = registry().lock().unwrap().get(name).copied() {
        // Pass the first live value as the argument; fall back to 0 if none.
        let arg = if state.num_values > 0 {
            state.values[0]
        } else {
            0
        };
        return fallback(arg);
    }

    #[cfg(debug_assertions)]
    eprintln!("[DEOPT] fallback '{}' not found in registry", name);

    0 // Return None sentinel — fallback not registered
}

/// Counter for deopt events (for profiling/diagnostics).
static DEOPT_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Get total number of deopt events since program start.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deopt_count() -> u64 {
    DEOPT_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deopt_transfer_null_state_returns_zero() {
        let result = molt_deopt_transfer(std::ptr::null());
        assert_eq!(result, 0);
    }

    #[test]
    fn register_and_deopt_transfer_calls_fallback() {
        // Register a simple fallback that doubles its argument.
        extern "C" fn double_it(x: u64) -> u64 {
            x.wrapping_mul(2)
        }

        let name = b"test_double_fallback";
        molt_register_function(name.as_ptr(), name.len(), double_it);

        // Build a DeoptState pointing at the registered name.
        let mut ds = DeoptState::new();
        ds.fallback_name_ptr = name.as_ptr();
        ds.fallback_name_len = name.len();
        ds.num_values = 1;
        ds.values[0] = 21;

        let result = molt_deopt_transfer(&ds as *const DeoptState);
        assert_eq!(result, 42, "fallback should have returned 21*2=42");
    }

    #[test]
    fn deopt_transfer_unknown_name_returns_zero() {
        let name = b"__nonexistent_function__";
        let mut ds = DeoptState::new();
        ds.fallback_name_ptr = name.as_ptr();
        ds.fallback_name_len = name.len();
        ds.num_values = 0;

        let result = molt_deopt_transfer(&ds as *const DeoptState);
        assert_eq!(result, 0);
    }

    #[test]
    fn deopt_transfer_no_values_passes_zero() {
        extern "C" fn returns_arg(x: u64) -> u64 {
            x
        }

        let name = b"test_no_values_func";
        molt_register_function(name.as_ptr(), name.len(), returns_arg);

        let mut ds = DeoptState::new();
        ds.fallback_name_ptr = name.as_ptr();
        ds.fallback_name_len = name.len();
        ds.num_values = 0; // no live values → arg should be 0

        let result = molt_deopt_transfer(&ds as *const DeoptState);
        assert_eq!(result, 0);
    }
}
