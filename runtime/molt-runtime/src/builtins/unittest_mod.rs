//! Intrinsics for `unittest` stdlib module.
//!
//! Coverage: TestResult, TestCase assertions, TestLoader, TextTestRunner,
//! formatting and comparison helpers.

use crate::{
    MoltObject, alloc_string,
    is_truthy, obj_from_bits, string_obj_to_owned,
    to_i64, int_bits_from_i64,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

fn mk_str(py: &crate::PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// TestResult handle
// ---------------------------------------------------------------------------

struct TestResultState {
    tests_run: i64,
    failures: Vec<(String, String)>,
    errors: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
    expected_failures: Vec<(String, String)>,
    unexpected_successes: Vec<String>,
    should_stop: bool,
}

impl TestResultState {
    fn new() -> Self {
        TestResultState {
            tests_run: 0,
            failures: Vec::new(),
            errors: Vec::new(),
            skipped: Vec::new(),
            expected_failures: Vec::new(),
            unexpected_successes: Vec::new(),
            should_stop: false,
        }
    }

    fn was_successful(&self) -> bool {
        self.failures.is_empty() && self.errors.is_empty() && self.unexpected_successes.is_empty()
    }
}

thread_local! {
    static RESULTS: RefCell<HashMap<i64, TestResultState>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Public extern "C" intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let id = next_handle_id();
        RESULTS.with(|m| m.borrow_mut().insert(id, TestResultState::new()));
        int_bits_from_i64(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_tests_run(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => int_bits_from_i64(_py, s.tests_run),
                None => int_bits_from_i64(_py, 0),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_start_test(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.tests_run += 1;
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_add_failure(handle_bits: u64, test_bits: u64, err_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let test = string_obj_to_owned(obj_from_bits(test_bits)).unwrap_or_default();
        let err = string_obj_to_owned(obj_from_bits(err_bits)).unwrap_or_default();
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.failures.push((test, err));
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_add_error(handle_bits: u64, test_bits: u64, err_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let test = string_obj_to_owned(obj_from_bits(test_bits)).unwrap_or_default();
        let err = string_obj_to_owned(obj_from_bits(err_bits)).unwrap_or_default();
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.errors.push((test, err));
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_add_skip(handle_bits: u64, test_bits: u64, reason_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let test = string_obj_to_owned(obj_from_bits(test_bits)).unwrap_or_default();
        let reason = string_obj_to_owned(obj_from_bits(reason_bits)).unwrap_or_default();
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.skipped.push((test, reason));
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_add_expected_failure(handle_bits: u64, test_bits: u64, err_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let test = string_obj_to_owned(obj_from_bits(test_bits)).unwrap_or_default();
        let err = string_obj_to_owned(obj_from_bits(err_bits)).unwrap_or_default();
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.expected_failures.push((test, err));
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_add_unexpected_success(handle_bits: u64, test_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let test = string_obj_to_owned(obj_from_bits(test_bits)).unwrap_or_default();
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.unexpected_successes.push(test);
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_was_successful(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => MoltObject::from_bool(s.was_successful()).bits(),
                None => MoltObject::from_bool(false).bits(),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_stop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&handle) {
                s.should_stop = true;
            }
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_should_stop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => MoltObject::from_bool(s.should_stop).bits(),
                None => MoltObject::from_bool(false).bits(),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_failures_count(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => int_bits_from_i64(_py, s.failures.len() as i64),
                None => int_bits_from_i64(_py, 0),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_errors_count(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => int_bits_from_i64(_py, s.errors.len() as i64),
                None => int_bits_from_i64(_py, 0),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_skipped_count(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => int_bits_from_i64(_py, s.skipped.len() as i64),
                None => int_bits_from_i64(_py, 0),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_summary(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        RESULTS.with(|m| {
            match m.borrow().get(&handle) {
                Some(s) => {
                    let summary = format!(
                        "Ran {} test{}, {} failures, {} errors, {} skipped",
                        s.tests_run,
                        if s.tests_run == 1 { "" } else { "s" },
                        s.failures.len(),
                        s.errors.len(),
                        s.skipped.len(),
                    );
                    mk_str(_py, &summary)
                }
                None => mk_str(_py, "No test results"),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_result_drop(handle_bits: u64) {
    let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
    RESULTS.with(|m| m.borrow_mut().remove(&handle));
}

// ---------------------------------------------------------------------------
// Assertion formatting helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_format_failure(first_bits: u64, second_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let first = string_obj_to_owned(obj_from_bits(first_bits)).unwrap_or_else(|| "None".to_string());
        let second = string_obj_to_owned(obj_from_bits(second_bits)).unwrap_or_else(|| "None".to_string());
        let msg_obj = obj_from_bits(msg_bits);
        let result = if msg_obj.is_none() {
            format!("{} != {}", first, second)
        } else {
            let msg = string_obj_to_owned(msg_obj).unwrap_or_default();
            format!("{} : {} != {}", msg, first, second)
        };
        mk_str(_py, &result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_safe_repr(obj_bits: u64, short_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let text = string_obj_to_owned(obj_from_bits(obj_bits)).unwrap_or_else(|| "None".to_string());
        let short = is_truthy(_py, obj_from_bits(short_bits));
        let result = if short && text.len() > 80 {
            format!("{}...", &text[..77])
        } else {
            text
        };
        mk_str(_py, &result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unittest_count_diff_all_purpose(
    first_bits: u64,
    second_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let first = string_obj_to_owned(obj_from_bits(first_bits)).unwrap_or_default();
        let second = string_obj_to_owned(obj_from_bits(second_bits)).unwrap_or_default();
        if first == second {
            return mk_str(_py, "");
        }
        let result = format!("First: {}\nSecond: {}", first, second);
        mk_str(_py, &result)
    })
}
