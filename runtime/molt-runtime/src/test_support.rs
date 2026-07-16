//! Shared process-wide test authorities.
//!
//! Expected panics must not invoke the platform backtrace resolver. On Windows,
//! many concurrent, deliberately caught panics can otherwise deadlock inside
//! `dbghelp` while test and worker threads are entering loader/TLS teardown.
//! Unexpected panics still delegate to the original hook unchanged.

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::Once;

thread_local! {
    static EXPECTED_PANIC_DEPTH: Cell<u32> = const { Cell::new(0) };
}

static INSTALL_EXPECTED_PANIC_HOOK: Once = Once::new();

fn install_expected_panic_hook() {
    INSTALL_EXPECTED_PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let expected = EXPECTED_PANIC_DEPTH
                .try_with(|depth| depth.get() != 0)
                .unwrap_or(false);
            if !expected {
                previous(info);
            }
        }));
    });
}

struct ExpectedPanicGuard;

impl ExpectedPanicGuard {
    fn enter() -> Self {
        install_expected_panic_hook();
        EXPECTED_PANIC_DEPTH.with(|depth| {
            depth.set(
                depth
                    .get()
                    .checked_add(1)
                    .expect("expected-panic nesting overflow"),
            );
        });
        Self
    }
}

impl Drop for ExpectedPanicGuard {
    fn drop(&mut self) {
        EXPECTED_PANIC_DEPTH.with(|depth| {
            let current = depth.get();
            assert_ne!(current, 0, "unmatched expected-panic guard");
            depth.set(current - 1);
        });
    }
}

pub(crate) fn with_expected_panic<F, R>(operation: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ExpectedPanicGuard::enter();
    operation()
}

pub(crate) fn catch_expected_unwind<F, R>(operation: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R,
{
    with_expected_panic(|| std::panic::catch_unwind(AssertUnwindSafe(operation)))
}

#[test]
fn expected_panic_hook_is_thread_local_and_nestable() {
    assert!(catch_expected_unwind(|| panic!("outer expected panic")).is_err());
    assert!(
        catch_expected_unwind(|| {
            assert!(catch_expected_unwind(|| panic!("inner expected panic")).is_err());
            panic!("second outer expected panic");
        })
        .is_err()
    );
}
