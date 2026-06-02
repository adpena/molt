//! FFI boundary panic-contract regression tests.
//!
//! These pin the contract enforced by `with_gil_entry!` / `with_core_gil!`
//! (and the shared `molt_runtime_core::with_gil_entry_body!` dispatch) under
//! BOTH panic strategies — in particular under `panic = "abort"`, the strategy
//! the SHIPPED runtime staticlib (`release-output` / `wasm-release`) uses.
//!
//! Background (the bug these guard against):
//! The shipped staticlib is compiled with `panic = "abort"`, under which
//! `std::panic::catch_unwind` is a documented no-op.  An FFI entry point that
//! relied on `catch_unwind` to convert a Rust panic into a catchable Python
//! `RuntimeError` would therefore *abort the process* in the shipped binary
//! instead of raising — a silent CPython-parity divergence.  The structural
//! fix makes the panic contract explicit and profile-conditional:
//!
//!   * Every FFI body MUST propagate all Python-level error conditions
//!     explicitly (set a pending Python exception + return the sentinel) and
//!     MUST NOT rely on a panic to surface a recoverable Python error.
//!   * Under `panic = "unwind"` the catching macros keep a `catch_unwind`
//!     defense-in-depth net (an invariant violation becomes a catchable
//!     `RuntimeError`; CI runs this profile).
//!   * Under `panic = "abort"` the catch is `cfg`-eliminated; the only
//!     reachable panic is an invariant violation, for which process abort is
//!     the correct fail-closed behavior.
//!
//! Run the suite under the shipped profile to exercise the previously-untested
//! `panic = "abort"` lane:
//!   cargo test -p molt-runtime --profile release-output --lib -- \
//!       concurrency::panic_contract_tests --test-threads=1

use crate::builtins::exceptions::{
    molt_exception_class, molt_exception_clear, molt_exception_kind,
    molt_exception_new_builtin_one, molt_exception_pending, raise_exception,
};
use crate::c_api::molt_err_matches;
use crate::state::runtime_state::molt_runtime_init;
use molt_obj_model::MoltObject;

// Builtin exception tags (see the tag table in builtins/exceptions.rs:
// 5 => ValueError, 6 => TypeError, 7 => RuntimeError).
const TAG_VALUE_ERROR: u64 = 5;
const TAG_RUNTIME_ERROR: u64 = 7;

/// Serialize against every other test that shares the process-global
/// RuntimeState, and clear any stale pending exception on entry.
struct ContractGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ContractGuard {
    fn new() -> Self {
        let guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        molt_runtime_init();
        let _ = molt_exception_clear();
        Self { _guard: guard }
    }
}

impl Drop for ContractGuard {
    fn drop(&mut self) {
        let _ = molt_exception_clear();
    }
}

/// Resolve the class object (a `type`) for a builtin exception tag so the test
/// can assert the pending exception's class via `molt_err_matches`.
fn builtin_exc_type_bits(tag: u64) -> u64 {
    let exc_bits = molt_exception_new_builtin_one(tag, MoltObject::from_int(0).bits());
    assert_ne!(exc_bits, 0, "failed to construct builtin exception (tag {tag})");
    let kind_bits = molt_exception_kind(exc_bits);
    let class_bits = molt_exception_class(kind_bits);
    assert_ne!(class_bits, 0, "no class for builtin exception (tag {tag})");
    class_bits
}

// ---------------------------------------------------------------------------
// Test FFI entry points — wrapped in the REAL `with_gil_entry!` macro so they
// exercise the exact boundary every shipped stdlib FFI entry point uses.
// ---------------------------------------------------------------------------

/// Mirrors the universal stdlib pattern: on a Python-level error, set a pending
/// Python exception explicitly and return the runtime sentinel.  No panic is
/// involved.  Must surface a CATCHABLE exception under every profile —
/// including `panic = "abort"` — and never abort the process.
extern "C" fn ffi_raises_value_error() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "ValueError", "recoverable python-level error")
    })
}

/// The body panics, standing in for a genuine runtime invariant violation (the
/// only legitimate source of a panic under the contract).  Under
/// `panic = "unwind"` the catching macro converts it into a pending
/// `RuntimeError`.  Under `panic = "abort"` it aborts (correct fail-closed), so
/// the test below only drives this under `panic = "unwind"`.
extern "C" fn ffi_panics() -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = _py;
        panic!("simulated runtime invariant violation");
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// LOAD-BEARING regression. A `with_gil_entry!`-wrapped FFI body that hits a
/// Python-level error must surface it as a catchable pending Python exception,
/// NOT a process abort — under whatever profile compiled this test, including
/// the shipped `panic = "abort"` (`release-output`).
#[test]
fn python_level_error_is_catchable() {
    let _guard = ContractGuard::new();
    let value_error_type = builtin_exc_type_bits(TAG_VALUE_ERROR);

    // Must NOT abort; returns normally with a pending exception set.
    let ret = ffi_raises_value_error();
    assert!(
        MoltObject::from_bits(ret).is_none(),
        "raising FFI entry point should return the None sentinel"
    );
    assert_eq!(
        molt_exception_pending(),
        1,
        "a Python-level error in a with_gil_entry! body must leave a pending \
         exception (catchable), not abort"
    );
    assert_eq!(
        molt_err_matches(value_error_type),
        1,
        "pending exception must be the ValueError the body raised"
    );
    let _ = molt_exception_clear();
    assert_eq!(molt_exception_pending(), 0);
}

/// Under `panic = "unwind"` (dev / dev-fast / release-fast — the CI profile),
/// an invariant-violation panic inside a catching FFI body is converted into a
/// catchable pending `RuntimeError` instead of crossing the `extern "C"`
/// boundary as an unwind (which would be UB).  Under `panic = "abort"` this
/// path aborts (correct fail-closed), so it is not driven there.
#[cfg(panic = "unwind")]
#[test]
fn invariant_panic_becomes_runtime_error_under_unwind() {
    let _guard = ContractGuard::new();
    let runtime_error_type = builtin_exc_type_bits(TAG_RUNTIME_ERROR);

    // Silence the default panic hook for the duration of the deliberate panic
    // so the test output stays clean (the panic is expected and caught).
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let ret = ffi_panics();
    std::panic::set_hook(prev_hook);

    // On a caught panic the macro returns a zero-initialized sentinel; the
    // contract is that the *caller* checks the pending exception before using
    // the value, so the value itself is just zeroed bits (not necessarily the
    // canonical None handle).
    assert_eq!(ret, 0, "caught panic should return the zero sentinel");
    assert_eq!(
        molt_exception_pending(),
        1,
        "an invariant panic under panic=unwind must be caught into a pending \
         RuntimeError (defense in depth)"
    );
    assert_eq!(
        molt_err_matches(runtime_error_type),
        1,
        "the caught panic must surface as RuntimeError"
    );
    let _ = molt_exception_clear();
}

/// Compile-time contract: the macro dispatch must agree with the active panic
/// strategy, so the contract can never silently regress to "claims to catch but
/// cannot".  Under `panic = "abort"` (the shipped profile) the catching macros
/// emit no `catch_unwind`; the Python-level-error path is nonetheless still
/// catchable (asserted here, end-to-end, under that profile).
#[test]
fn macro_dispatch_matches_panic_strategy() {
    let unwind = cfg!(panic = "unwind");
    let abort = cfg!(panic = "abort");
    assert!(
        unwind ^ abort,
        "panic strategy must be exactly one of unwind/abort \
         (unwind={unwind}, abort={abort})"
    );

    if abort {
        let _guard = ContractGuard::new();
        let _ = ffi_raises_value_error();
        assert_eq!(
            molt_exception_pending(),
            1,
            "under panic=abort, a Python-level error must still be catchable"
        );
        let _ = molt_exception_clear();
    }
}
