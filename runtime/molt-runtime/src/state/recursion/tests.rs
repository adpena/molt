use super::*;
use crate::state::runtime_state::{
    RuntimeState, clear_thread_runtime_state, runtime_state_for_gil, set_thread_runtime_state,
};

struct RestoreLimit(usize);

impl RestoreLimit {
    fn set(limit: usize) -> Self {
        let saved = Self(recursion_limit_get());
        recursion_limit_set(limit);
        saved
    }
}

impl Drop for RestoreLimit {
    fn drop(&mut self) {
        recursion_limit_set(self.0);
    }
}

#[test]
fn generated_and_runtime_guards_share_depth_and_rejected_limit_is_atomic() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _limit = RestoreLimit::set(2);
    crate::with_gil_entry!(_py, {
        assert_eq!(recursion_depth(), 0);
        let outer = RecursionGuard::enter(_py).unwrap();
        assert_eq!(crate::object::ops_sys::molt_recursion_enter_fast(), 1);
        assert_eq!(recursion_depth(), 2);
        assert_eq!(crate::object::ops_sys::molt_recursion_enter_fast(), 0);
        assert_eq!(
            recursion_depth(),
            2,
            "rejected entry must not retain a charge"
        );

        crate::object::ops_sys::molt_setrecursionlimit(crate::MoltObject::from_int(2).bits());
        assert!(crate::exception_pending(_py));
        crate::clear_exception(_py);
        assert_eq!(recursion_limit_get(), 2);
        crate::object::ops_sys::molt_setrecursionlimit(crate::MoltObject::from_int(3).bits());
        assert!(!crate::exception_pending(_py));
        assert_eq!(recursion_limit_get(), 3);

        let nested = RecursionGuard::enter(_py).unwrap();
        assert_eq!(recursion_depth(), 3);
        drop(nested);
        crate::object::ops_sys::molt_recursion_exit_fast();
        drop(outer);
        assert_thread_recursion_idle();
    });
}

#[test]
fn threads_have_independent_depth_and_share_live_runtime_limit() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _limit = RestoreLimit::set(2);
    assert!(recursion_guard_enter());
    assert!(recursion_guard_enter());
    assert!(!recursion_guard_enter());
    let observed = std::thread::spawn(|| {
        assert_eq!(recursion_depth(), 0);
        assert_eq!(recursion_limit_get(), 2);
        assert!(recursion_guard_enter());
        recursion_limit_set(1);
        assert!(!recursion_guard_enter());
        recursion_guard_exit();
        assert_thread_recursion_idle();
        recursion_limit_get()
    })
    .join()
    .unwrap();
    assert_eq!(observed, 1);
    assert_eq!(recursion_limit_get(), 1);
    assert_eq!(
        recursion_depth(),
        2,
        "worker exit cannot discharge the caller"
    );
    assert!(!recursion_guard_enter());
    recursion_guard_exit();
    recursion_guard_exit();
    assert_thread_recursion_idle();
}

#[test]
fn isolated_runtime_limit_does_not_modify_shared_runtime_or_new_runtime() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _limit = RestoreLimit::set(31);
    let shared = runtime_state_for_gil().unwrap() as *const RuntimeState as usize;
    std::thread::spawn(move || {
        let mut isolated = Box::new(RuntimeState::new());
        set_thread_runtime_state(&mut *isolated);
        assert_eq!(recursion_limit_get(), DEFAULT_RECURSION_LIMIT);
        recursion_limit_set(17);
        assert!(recursion_guard_enter());
        recursion_guard_exit();
        assert_thread_recursion_idle();
        clear_thread_runtime_state();
        assert_eq!(
            runtime_state_for_gil().unwrap() as *const RuntimeState as usize,
            shared
        );
        assert_eq!(recursion_limit_get(), 31);
        drop(isolated);
        let fresh = RuntimeState::new();
        assert_eq!(
            fresh.recursion_limit.load(Ordering::Relaxed),
            DEFAULT_RECURSION_LIMIT
        );
    })
    .join()
    .unwrap();
    assert_eq!(recursion_limit_get(), 31);
}

#[test]
fn guard_unwind_releases_charge_and_unbalanced_exit_is_diagnosed() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry!(_py, {
        let result = crate::test_support::with_expected_panic(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = RecursionGuard::enter(_py).unwrap();
                panic!("exercise guard unwinding");
            }))
        });
        assert!(result.is_err());
        assert_thread_recursion_idle();
        let underflow = crate::test_support::with_expected_panic(|| {
            std::panic::catch_unwind(recursion_guard_exit)
        });
        assert!(underflow.is_err());
        assert_thread_recursion_idle();
    });
}
