use super::*;
use crate::state::runtime_state::{
    RuntimeState, clear_thread_runtime_state, set_thread_runtime_state,
};

struct RestoreLimit<'a, 'py> {
    py: &'a crate::PyToken<'py>,
    value: usize,
}
impl<'a, 'py> RestoreLimit<'a, 'py> {
    fn set(py: &'a crate::PyToken<'py>, limit: usize) -> Self {
        let value = recursion_limit_get(py);
        recursion_limit_set(py, limit);
        Self { py, value }
    }
}
impl Drop for RestoreLimit<'_, '_> {
    fn drop(&mut self) {
        recursion_limit_set(self.py, self.value);
    }
}

#[test]
fn generated_and_runtime_guards_share_depth_and_rejected_limit_is_atomic() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry!(_py, {
        let _limit = RestoreLimit::set(_py, 2);
        assert_eq!(recursion_depth(_py), 0);
        let outer = RecursionGuard::enter(_py).unwrap();
        assert_eq!(
            crate::object::ops_sys::molt_recursion_enter_fast_attached_body(_py),
            1
        );
        assert_eq!(recursion_depth(_py), 2);
        assert_eq!(
            crate::object::ops_sys::molt_recursion_enter_fast_attached_body(_py),
            0
        );
        assert_eq!(
            recursion_depth(_py),
            2,
            "rejected entry must not retain a charge"
        );
        crate::object::ops_sys::molt_setrecursionlimit_attached_body(
            _py,
            crate::MoltObject::from_int(2).bits(),
        );
        assert!(crate::exception_pending(_py));
        crate::clear_exception(_py);
        assert_eq!(recursion_limit_get(_py), 2);
        crate::object::ops_sys::molt_setrecursionlimit_attached_body(
            _py,
            crate::MoltObject::from_int(3).bits(),
        );
        assert!(!crate::exception_pending(_py));
        assert_eq!(recursion_limit_get(_py), 3);
        let nested = RecursionGuard::enter(_py).unwrap();
        assert_eq!(recursion_depth(_py), 3);
        drop(nested);
        crate::object::ops_sys::molt_recursion_exit_fast_attached_body(_py);
        drop(outer);
        assert_thread_recursion_idle();
    });
}

#[test]
fn threads_have_independent_depth_and_share_live_runtime_limit() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| {
        let _limit = RestoreLimit::set(py, 2);
        assert!(recursion_guard_enter(py));
        assert!(recursion_guard_enter(py));
        assert!(!recursion_guard_enter(py));
        let worker = std::thread::spawn(|| {
            crate::with_gil(|py| {
                assert_eq!(recursion_depth(py), 0);
                assert_eq!(recursion_limit_get(py), 2);
                assert!(recursion_guard_enter(py));
                recursion_limit_set(py, 1);
                assert!(!recursion_guard_enter(py));
                recursion_guard_exit(py);
                assert_thread_recursion_idle();
                recursion_limit_get(py)
            })
        });
        let observed = {
            let _suspended = crate::GilReleaseGuard::suspend();
            worker.join().unwrap()
        };
        assert_eq!(observed, 1);
        assert_eq!(recursion_limit_get(py), 1);
        assert_eq!(
            recursion_depth(py),
            2,
            "worker exit cannot discharge the caller"
        );
        assert!(!recursion_guard_enter(py));
        recursion_guard_exit(py);
        recursion_guard_exit(py);
        assert_thread_recursion_idle();
    });
}

#[test]
fn isolated_runtime_limit_does_not_modify_shared_runtime_or_new_runtime() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| {
        let _limit = RestoreLimit::set(py, 31);
        let shared = crate::runtime_state(py) as *const RuntimeState as usize;
        let worker = std::thread::spawn(move || {
            let mut isolated = Box::new(RuntimeState::new());
            set_thread_runtime_state(&mut *isolated);
            {
                let gil = crate::GilGuard::new();
                let py = gil.token();
                assert_eq!(recursion_limit_get(py), DEFAULT_RECURSION_LIMIT);
                recursion_limit_set(py, 17);
                assert!(recursion_guard_enter(py));
                recursion_guard_exit(py);
                assert_thread_recursion_idle();
            }
            clear_thread_runtime_state();
            {
                let gil = crate::GilGuard::new();
                let py = gil.token();
                assert_eq!(
                    crate::runtime_state(py) as *const RuntimeState as usize,
                    shared
                );
                assert_eq!(recursion_limit_get(py), 31);
            }
            drop(isolated);
            let fresh = RuntimeState::new();
            assert_eq!(
                fresh.recursion_limit.load(Ordering::Relaxed),
                DEFAULT_RECURSION_LIMIT
            );
        });
        {
            let _suspended = crate::GilReleaseGuard::suspend();
            worker.join().unwrap();
        }
        assert_eq!(recursion_limit_get(py), 31);
    });
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
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recursion_guard_exit(_py)))
        });
        assert!(underflow.is_err());
        assert_thread_recursion_idle();
    });
}
