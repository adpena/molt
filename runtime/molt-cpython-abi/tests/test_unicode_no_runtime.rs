//! Fresh-process Unicode admission before runtime hook registration.
//! Invalid code points require no state; C errors use the real, detached
//! initialization-phase thread-state owner, without a runtime or attachment.
use molt_cpython_abi::{
    abi_types,
    api::{errors, object, strings},
};

#[test]
fn unicode_predicates_refuse_missing_runtime_but_preserve_invalid_code_domain() {
    // Canonical C-ABI type storage must exist before publishing a C error.
    // This initializes static ABI shells, not a runtime or interpreter.
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    assert!(molt_cpython_abi::hooks::hooks().is_none());
    assert!(!errors::raised_error_pending());
    let assert_detached = || {
        assert!(molt_cpython_abi::hooks::hooks().is_none());
        assert_eq!(unsafe { object::Py_IsInitialized() }, 0);
        assert_eq!(unsafe { object::PyGILState_Check() }, 0);
        assert!(unsafe { object::_PyThreadState_UncheckedGet() }.is_null());
    };
    assert_detached();
    assert!(!object::current_thread_has_retained_runtime_state());

    let predicates: &[extern "C" fn(u32) -> i32] = &[
        strings::_PyUnicode_IsAlpha,
        strings::_PyUnicode_IsLowercase,
        strings::_PyUnicode_IsUppercase,
        strings::_PyUnicode_IsTitlecase,
        strings::_PyUnicode_IsDecimalDigit,
        strings::_PyUnicode_IsDigit,
        strings::_PyUnicode_IsNumeric,
        strings::_PyUnicode_IsPrintable,
        strings::_PyUnicode_IsWhitespace,
        strings::_PyUnicode_IsLinebreak,
    ];
    // The invalid-domain branch must not bootstrap an error record merely to
    // answer false, even when no runtime target or lifecycle scope exists.
    for predicate in predicates {
        for invalid in [0x110000, u32::MAX] {
            assert_eq!(predicate(invalid), 0);
            assert!(!errors::raised_error_pending());
            assert!(!object::current_thread_has_retained_runtime_state());
        }
    }

    // Process-static ABI storage does not admit C error publication. Reuse the
    // same detached initialization transaction as production hook readiness and
    // the existing pre-hook exception control, not a fake hook/runtime provider.
    let initialization_owner = 0_u8;
    let owner = std::ptr::from_ref(&initialization_owner).expose_provenance();
    assert_eq!(object::runtime_retained_thread_state_count_for(owner), 0);
    object::prepare_runtime_thread_state_lifetime();
    object::arm_runtime_thread_state_lifetime();
    let initializing = object::RuntimeInitializationThreadStateGuard::enter(owner);
    assert_eq!(object::runtime_retained_thread_state_count_for(owner), 1);
    assert!(object::current_thread_has_retained_runtime_state());
    assert_detached();
    for predicate in predicates {
        assert_eq!(predicate(0x110000), 0);
        assert!(!errors::raised_error_pending());
        assert_eq!(predicate(0x41), 0);
        let error = errors::take_current_error().expect("missing runtime must set error");
        assert_eq!(
            error.exc_type,
            (&raw mut abi_types::PyExc_SystemError).cast()
        );
        assert!(
            error.value.is_null(),
            "no fabricated pre-runtime exception instance"
        );
        assert!(error.traceback.is_null());
        drop(error);
        assert!(!errors::raised_error_pending());
        assert_detached();
    }
    drop(initializing);
    assert_eq!(object::runtime_retained_thread_state_count_for(owner), 0);
    assert!(!object::current_thread_has_retained_runtime_state());
    assert!(!errors::raised_error_pending());
    assert_detached();
}
