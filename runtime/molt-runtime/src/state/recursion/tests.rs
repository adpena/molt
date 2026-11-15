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

/// Runs the generator compiled calls use over this thread's live record:
/// each load and store touches the real word at the generated offset, so a
/// stale offset or an unpublished limit pointer fails here as it would in
/// compiled code.
struct LiveRecord;

impl molt_codegen_abi::word::WordBuilder for LiveRecord {
    type Word = u64;
    type Predicate = bool;
    fn constant(&mut self, value: u64) -> u64 {
        value
    }
    fn and(&mut self, value: u64, mask: u64) -> u64 {
        value & mask
    }
    fn or(&mut self, value: u64, bits: u64) -> u64 {
        value | bits
    }
    fn shift_left(&mut self, value: u64, amount: u32) -> u64 {
        value << amount
    }
    fn shift_right_arithmetic(&mut self, value: u64, amount: u32) -> u64 {
        ((value as i64) >> amount) as u64
    }
    fn add_immediate(&mut self, value: u64, delta: i64) -> u64 {
        value.wrapping_add_signed(delta)
    }
    fn compare(
        &mut self,
        comparison: molt_codegen_abi::word::WordComparison,
        left: u64,
        right: u64,
    ) -> bool {
        use molt_codegen_abi::word::WordComparison::*;
        match comparison {
            Equal => left == right,
            NotEqual => left != right,
            UnsignedLessThan => left < right,
        }
    }
    fn both(&mut self, left: bool, right: bool) -> bool {
        left && right
    }
}

impl molt_codegen_abi::thread_record::RecordWordBuilder for LiveRecord {
    fn load_word(&mut self, address: u64, offset: i32) -> u64 {
        let at = address.wrapping_add_signed(offset.into()) as usize;
        assert_ne!(
            at, offset as usize,
            "compiled code would load through a null word"
        );
        // SAFETY: the address is a published record word or the limit it names.
        unsafe { std::ptr::read(at as *const u64) }
    }
    fn store_word(&mut self, address: u64, offset: i32, value: u64) {
        let at = address.wrapping_add_signed(offset.into()) as usize;
        // SAFETY: only the depth word of this thread's record is stored.
        unsafe { std::ptr::write(at as *mut u64, value) }
    }
    fn load_shared_word(&mut self, address: u64, offset: i32) -> u64 {
        let at = address.wrapping_add_signed(offset.into()) as usize;
        assert_ne!(
            at, offset as usize,
            "compiled code would load through a null word"
        );
        // SAFETY: the runtime's atomic limit word, read as compiled code does.
        unsafe { (*(at as *const std::sync::atomic::AtomicU64)).load(Ordering::Relaxed) }
    }
}

/// A compiled call's charge: the inline check, then the count it admits.
fn compiled_enter(py: &crate::PyToken<'_>) -> bool {
    let record = std::ptr::from_ref(py.execution_tls()) as u64;
    if !molt_codegen_abi::thread_record::generate_recursion_admits(&mut LiveRecord, record) {
        return false;
    }
    molt_codegen_abi::thread_record::generate_recursion_count(&mut LiveRecord, record);
    true
}

fn compiled_exit(py: &crate::PyToken<'_>) {
    let record = std::ptr::from_ref(py.execution_tls()) as u64;
    molt_codegen_abi::thread_record::generate_recursion_release(&mut LiveRecord, record);
}

#[test]
fn generated_and_runtime_guards_share_depth_and_rejected_limit_is_atomic() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry!(_py, {
        let _limit = RestoreLimit::set(_py, 2);
        assert_eq!(recursion_depth(_py), 0);
        let outer = RecursionGuard::enter(_py).unwrap();
        assert!(compiled_enter(_py));
        assert_eq!(recursion_depth(_py), 2);
        assert!(!compiled_enter(_py), "the limit refuses the third frame");
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
        // The raised limit reaches the compiled check through the published
        // pointer, with no rebinding.
        assert!(!compiled_enter(_py));
        drop(nested);
        assert!(compiled_enter(_py));
        compiled_exit(_py);
        compiled_exit(_py);
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
                DEFAULT_RECURSION_LIMIT as u64
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
