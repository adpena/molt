//! Tests for the RuntimeHooks vtable and stub hooks.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::hooks::{DecodedHandleResult, hooks_or_stubs};
use std::ptr;

unsafe extern "C" fn fixture_alloc_str(_data: *const u8, _len: usize) -> u64 {
    0
}

fn fixture_hooks() -> molt_cpython_abi::hooks::RuntimeHooks {
    let mut hooks = support::stub_runtime_hooks();
    // Preserve fail-closed string semantics while exercising optional Some
    // identity through the same installer as every other test in this binary.
    hooks.alloc_str = Some(fixture_alloc_str);
    hooks
}

fn init() -> support::AbiTestThreadStateTransaction {
    support::enter_abi_test(fixture_hooks())
}

// ---------------------------------------------------------------------------
// hooks_or_stubs returns stubs when no runtime registered
// ---------------------------------------------------------------------------

#[test]
fn test_hooks_or_stubs_returns_stubs() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    // The transaction adds lifecycle custody; the explicit string fixture
    // and all other object hooks retain fail-closed stub behavior.
    assert!(matches!(
        unsafe { h.numeric_identity_new(molt_lang_obj_model::MoltObject::from_int(1000).bits()) }
            .decode(),
        DecodedHandleResult::Error
    ));
    let mut payload = 99.0;
    assert_eq!(unsafe { (h.float_payload)(0, &raw mut payload) }, -1);
    assert_eq!(
        payload, 99.0,
        "refused extraction must not publish a payload"
    );

    let str_bits = unsafe { h.alloc_str(b"hello".as_ptr(), 5) };
    assert_eq!(str_bits, 0);

    let bytes_bits = unsafe { (h.alloc_bytes)(b"data".as_ptr(), 4) };
    assert_eq!(bytes_bits, 0);

    let int_bits = unsafe { (h.int_from_i64)(i64::MAX) };
    assert_eq!(int_bits, 0);

    let uint_bits = unsafe { (h.int_from_u64)(u64::MAX) };
    assert_eq!(uint_bits, 0);

    let list_bits = unsafe { (h.alloc_list)() };
    assert_eq!(list_bits, 0);

    let tuple_bits = unsafe { h.alloc_tuple(3) };
    assert_eq!(tuple_bits, 0);

    assert!(matches!(
        unsafe { (h.slice_new)(0, 0, 0) }.decode(),
        DecodedHandleResult::Error
    ));
    assert!(matches!(
        unsafe { (h.slice_item)(0, 0) }.decode(),
        DecodedHandleResult::Error
    ));

    assert!(matches!(
        unsafe { h.method_new(0, 0) }.decode(),
        DecodedHandleResult::Error
    ));
    for part in [
        molt_cpython_abi::hooks::MethodPart::Function,
        molt_cpython_abi::hooks::MethodPart::Receiver,
    ] {
        assert!(matches!(
            unsafe { (h.method_part)(0, part) }.decode(),
            DecodedHandleResult::Error
        ));
    }
    let dict_bits = unsafe { (h.alloc_dict)() };

    assert_eq!(dict_bits, 0);

    // A missing runtime must not turn either an empty vector or a malformed
    // span into an accepted call or attempt to read unavailable operands.
    for (positional, keywords) in [(0, 0), (1, 0), (0, 1), (usize::MAX, 1)] {
        assert!(matches!(
            unsafe { (h.object_vectorcall)(1, ptr::null(), positional, ptr::null(), keywords) }
                .decode(),
            DecodedHandleResult::Error
        ));
    }
}

#[test]
fn test_stub_list_operations() {
    let _abi_test = init();
    let h = hooks_or_stubs();

    // list_len / list_item on nonexistent list
    let len = unsafe { (h.list_len)(0) };
    assert_eq!(len, 0);

    let item = unsafe { (h.list_item)(0, 0) };
    assert!(matches!(item.decode(), DecodedHandleResult::Error));

    assert_eq!(unsafe { (h.list_append)(0, 0, std::ptr::null_mut()) }, -1);
}

#[test]
fn test_stub_tuple_operations() {
    let _abi_test = init();
    let h = hooks_or_stubs();

    let len = unsafe { h.tuple_len(0) };
    assert_eq!(len, 0);

    let item = unsafe { h.tuple_item(0, 0) };
    assert!(matches!(item.decode(), DecodedHandleResult::Error));

    assert!(matches!(
        unsafe { h.tuple_set(0, 0, 0, std::ptr::null_mut()) }.decode(),
        DecodedHandleResult::Error
    ));
}

#[test]
fn test_stub_dict_operations() {
    let _abi_test = init();
    let h = hooks_or_stubs();

    let len = unsafe { (h.dict_len)(0) };
    assert_eq!(len, 0);

    let val = unsafe { (h.dict_get)(0, 0, molt_cpython_abi::hooks::DictHashSource::Compute, 0) };
    assert!(matches!(val.decode(), DecodedHandleResult::Error));

    assert_eq!(
        unsafe { (h.dict_mutate)(0, 0, 0, 0, None, std::ptr::null_mut()) },
        -1
    );
}

#[test]
fn test_stub_str_data() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    let mut len: usize = 999;
    let ptr = unsafe { (h.str_data)(0, &mut len) };
    assert!(!ptr.is_null());
    assert_eq!(len, 0);
}

#[test]
fn test_stub_bytes_data() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    let mut len: usize = 999;
    let ptr = unsafe { (h.bytes_data)(0, &mut len) };
    assert!(ptr.is_null());
    assert_eq!(len, 0);
}

#[test]
fn test_stub_str_data_null_out_len() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    // Should not crash when out_len is null
    let ptr = unsafe { (h.str_data)(0, ptr::null_mut()) };
    assert!(!ptr.is_null());
}

#[test]
fn test_stub_bytes_data_null_out_len() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    let ptr = unsafe { (h.bytes_data)(0, ptr::null_mut()) };
    assert!(ptr.is_null());
}

#[test]
fn test_stub_buffer_hooks_fail_closed_and_clear_view() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    let mut view = molt_cpython_abi::hooks::MoltBufferView {
        data: std::ptr::dangling_mut::<u8>(),
        len: 8,
        readonly: 0,
        ..molt_cpython_abi::hooks::MoltBufferView::default()
    };
    assert_eq!(unsafe { (h.buffer_acquire)(0, &mut view) }, -1);
    assert!(view.data.is_null());
    assert_eq!(unsafe { (h.buffer_release)(&mut view) }, 0);
    assert!(view.data.is_null());
}

#[test]
fn test_stub_classify_heap() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    let tag = unsafe { h.classify_heap(0) };
    assert_eq!(tag, molt_cpython_abi::abi_types::MoltTypeTag::Other as u8);
}

#[test]
fn test_stub_inc_dec_ref_no_crash() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    // Should be noops
    unsafe { (h.inc_ref)(0) };
    unsafe { (h.dec_ref)(0) };
    unsafe { (h.inc_ref)(12345) };
    unsafe { (h.dec_ref)(12345) };
}

// ---------------------------------------------------------------------------
// F2/F3/F6 teeth: the numeric and dict hooks return explicit errors under stubs
// so the ABI never fabricates a wrong answer when the runtime authority is
// absent. (The real bignum-correct / exception-setting behavior lives in the
// runtime authority and is proved there; here we prove the stub fails closed.)
// ---------------------------------------------------------------------------

#[test]
fn test_stub_number_hooks_fail_closed() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    // Every discriminant must return the typed error status under the stub table.
    for op in 0..12u32 {
        assert!(matches!(
            unsafe { (h.number_binary_op)(op, 0, 1, 2) }.decode(),
            DecodedHandleResult::Error
        ));
    }
    for op in 0..=molt_cpython_abi::hooks::NumberUnaryOp::Long as u32 {
        assert!(matches!(
            unsafe { (h.number_unary_op)(op, 1) }.decode(),
            DecodedHandleResult::Error
        ));
    }
    assert!(matches!(
        unsafe { (h.number_power)(0, 2, 3, 0) }.decode(),
        DecodedHandleResult::Error
    ));
    assert!(matches!(
        unsafe { (h.number_power)(0, 2, 3, 5) }.decode(),
        DecodedHandleResult::Error
    ));
}

#[test]
fn test_stub_dict_op_hook_fails_closed() {
    let _abi_test = init();
    let h = hooks_or_stubs();
    for op in 0..3u32 {
        assert_eq!(unsafe { (h.dict_op)(op, 0) }, 0);
    }
}

// ---------------------------------------------------------------------------
// F1 teeth: PyArg_ParseTuple must NOT fake success. A format that requires a
// positional argument, against an empty argument tuple, must return 0 (failure)
// with an exception set — never 1 (success). Previously an unknown/unsatisfied
// format unit could still return 1, poisoning the caller with uninitialized
// output slots.
// ---------------------------------------------------------------------------

#[test]
fn test_pyarg_parse_missing_required_arg_fails_closed() {
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // args tuple is 0 (empty / None under stubs); format "i" wants one int.
    let mut out_slot: std::os::raw::c_int = 4242;
    let mut outs: [*mut std::ffi::c_void; 1] = [(&mut out_slot as *mut std::os::raw::c_int).cast()];
    let rc = unsafe {
        molt_cpython_abi::api::errors::molt_pyarg_parse_tuple_inner(
            ptr::null_mut(),
            c"i".as_ptr(),
            outs.as_mut_ptr(),
            1,
        )
    };
    assert_eq!(
        rc, 0,
        "PyArg_ParseTuple must fail (0), not fake success (1)"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a failed PyArg_ParseTuple must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn numeric_mode_and_semantic_target_are_strict_hook_contracts() {
    use molt_cpython_abi::hooks::{NumberOperationMode, RUNTIME_HOOKS_ABI_VERSION, STUB_HOOKS};
    assert_eq!(
        NumberOperationMode::from_abi(0),
        Some(NumberOperationMode::Normal)
    );
    assert_eq!(
        NumberOperationMode::from_abi(1),
        Some(NumberOperationMode::InPlace)
    );
    for invalid in [2, u32::MAX] {
        assert_eq!(NumberOperationMode::from_abi(invalid), None);
    }
    assert_eq!(unsafe { (STUB_HOOKS.target_python_minor)() }, -1);
    let mut prior = support::stub_runtime_hooks();
    prior.abi_version = RUNTIME_HOOKS_ABI_VERSION - 1;
    assert!(!unsafe { molt_cpython_abi::try_set_runtime_hooks(prior) });
}

#[test]
fn test_abi_transaction_cleanup_reports_leaks_and_preserves_primary_failure() {
    use molt_cpython_abi::api::object;
    use std::cell::Cell;
    use std::panic::{catch_unwind, panic_any};

    #[derive(Debug)]
    struct PrimaryFailure;

    for primary_failure in [false, true] {
        let marker = 0_u8;
        let address = std::ptr::from_ref(&marker) as usize;
        let registered = Cell::new(false);
        let outcome = catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _transaction = init();
            assert!(object::runtime_execution_thread_is_attached());
            assert_eq!(unsafe { hooks_or_stubs().native_gc_allocate(address) }, 0);
            registered.set(true);
            if primary_failure {
                panic_any(PrimaryFailure);
            }
            // Leave this test-owned marker in the real fixture ledger so Drop
            // encounters an ownership failure at the lexical test boundary.
        }));
        if registered.get() {
            // Retire only the deliberate fault, after cleanup has detected it.
            unsafe { (hooks_or_stubs().native_gc_deallocate)(address) };
        }
        let failure = outcome.expect_err("the cleanup fault must be observable");
        if primary_failure {
            assert!(failure.downcast_ref::<PrimaryFailure>().is_some());
        } else {
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .expect("cleanup failure must retain its diagnostic");
            assert!(message.contains("leaked native GC identities"));
        }
        assert!(!object::runtime_execution_thread_is_attached());
        assert!(!object::current_thread_has_retained_runtime_state());
        {
            let _transaction = init();
            assert!(object::runtime_execution_thread_is_attached());
        }
        assert!(!object::runtime_execution_thread_is_attached());
        assert!(!object::current_thread_has_retained_runtime_state());
    }
}

unsafe extern "C" fn conflicting_alloc_list() -> u64 {
    17
}

unsafe extern "C" fn conflicting_alloc_str(_data: *const u8, _len: usize) -> u64 {
    23
}

unsafe extern "C" fn unnormalized_runtime_is_initialized() -> std::os::raw::c_int {
    0
}

fn assert_no_execution_state() {
    assert!(!molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
    assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
}

#[test]
fn test_abi_transaction_rejects_conflicting_normalized_hooks_before_attachment() {
    use molt_cpython_abi::api::object;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Install and release the baseline before attempting another transaction:
    // a nested attempt would block on the process-owned fixture lock.
    drop(init());
    assert_no_execution_state();
    for field in [
        "abi_magic",
        "abi_version",
        "struct_size",
        "alloc_list",
        "alloc_str_none",
        "alloc_str_some",
    ] {
        let mut requested = fixture_hooks();
        let diagnostic_field = match field {
            "abi_magic" => {
                requested.abi_magic ^= 1;
                "abi_magic"
            }
            "abi_version" => {
                requested.abi_version += 1;
                "abi_version"
            }
            "struct_size" => {
                requested.struct_size += 1;
                "struct_size"
            }
            "alloc_list" => {
                requested.alloc_list = conflicting_alloc_list;
                "alloc_list"
            }
            "alloc_str_none" => {
                requested.alloc_str = None;
                "alloc_str"
            }
            "alloc_str_some" => {
                requested.alloc_str = Some(conflicting_alloc_str);
                "alloc_str"
            }
            _ => unreachable!(),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _transaction = support::enter_abi_test(requested);
        }));
        let failure = outcome.expect_err("an incompatible table must be rejected");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("hook rejection must retain its field diagnostic");
        assert!(
            message.contains(&format!("incompatible RuntimeHooks.{diagnostic_field}")),
            "{message}"
        );
        assert_no_execution_state();

        // The rejected request never replaced either original allocator, and
        // unwinding its lock does not prevent a subsequent valid attachment.
        let installed = molt_cpython_abi::hooks::hooks().unwrap();
        assert_eq!(unsafe { (installed.alloc_list)() }, 0);
        assert_eq!(unsafe { installed.alloc_str(b"x".as_ptr(), 1) }, 0);
        {
            let _transaction = init();
            assert!(object::runtime_execution_thread_is_attached());
            assert!(unsafe { molt_cpython_abi::api::sequences::PyList_New(0) }.is_null());
            assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        }
        assert_no_execution_state();
    }
}

#[test]
fn test_abi_transaction_accepts_identical_normalized_hooks() {
    for override_custody in [false, false, true] {
        let mut requested = fixture_hooks();
        if override_custody {
            requested.runtime_is_initialized = unnormalized_runtime_is_initialized;
        }
        {
            let _transaction = support::enter_abi_test(requested);
            assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
            assert_eq!(unsafe { (hooks_or_stubs().runtime_is_initialized)() }, 1);
            assert_eq!(unsafe { hooks_or_stubs().alloc_str(b"x".as_ptr(), 1) }, 0);
        }
        assert_no_execution_state();
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_abi_transaction_rejects_malformed_first_install_before_attachment() {
    const CHILD: &str = "MOLT_ABI_TEST_MALFORMED_FIRST_HOOKS";
    if std::env::var_os(CHILD).is_none() {
        // A fresh self-image is necessary: Rust test ordering cannot establish
        // that no sibling in this process has already installed its table.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "test_abi_transaction_rejects_malformed_first_install_before_attachment",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("spawn isolated hook registration control");
        assert!(
            output.status.success(),
            "child stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("malformed-first-install-refused")
        );
        return;
    }
    assert!(molt_cpython_abi::hooks::hooks().is_none());
    let mut malformed = fixture_hooks();
    malformed.abi_magic ^= 1;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _transaction = support::enter_abi_test(malformed);
    }));
    let failure = outcome.expect_err("first incompatible registration must be refused");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains("registration failed before attachment"));
    assert!(molt_cpython_abi::hooks::hooks().is_none());
    assert_no_execution_state();
    drop(init());
    assert_no_execution_state();
    println!("malformed-first-install-refused");
}
