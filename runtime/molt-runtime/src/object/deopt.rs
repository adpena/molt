//! Runtime support for deoptimization (speculative optimization bailout).

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

    // In a full implementation:
    // 1. Look up the fallback function by name
    // 2. Materialize live values into the fallback's parameter slots
    // 3. Call the fallback function
    // 4. Return its result
    //
    // For now: log the deopt event and return 0 (None).
    // The fallback function invocation requires the module's function table
    // which will be wired in when the compilation pipeline fully integrates.

    DEOPT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    #[cfg(debug_assertions)]
    {
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
            "<unknown>"
        };
        eprintln!(
            "[DEOPT] Transferring to {} with {} live values",
            name, state.num_values
        );
    }

    0 // Return None sentinel — generic fallback not yet wired
}

/// Counter for deopt events (for profiling/diagnostics).
static DEOPT_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Get total number of deopt events since program start.
#[unsafe(no_mangle)]
pub extern "C" fn molt_deopt_count() -> u64 {
    DEOPT_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}
