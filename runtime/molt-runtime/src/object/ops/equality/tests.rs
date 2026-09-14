use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

static TRACE: AtomicU64 = AtomicU64::new(0);
static MUTATION_TARGET: AtomicU64 = AtomicU64::new(0);
static MUTATION_NAME: AtomicU64 = AtomicU64::new(0);
static MUTATION_REPLACEMENT: AtomicU64 = AtomicU64::new(0);
static REPLACEMENT_CLASS: AtomicU64 = AtomicU64::new(0);
static DESCRIPTOR_ATTRIBUTE_ERROR: AtomicU64 = AtomicU64::new(0);

fn trace(marker: u64) {
    TRACE.store(TRACE.load(Ordering::SeqCst) * 10 + marker, Ordering::SeqCst);
}

fn mutate_configured_class(_py: &PyToken<'_>) -> bool {
    let target = MUTATION_TARGET.load(Ordering::SeqCst);
    let name = MUTATION_NAME.load(Ordering::SeqCst);
    let replacement = MUTATION_REPLACEMENT.load(Ordering::SeqCst);
    if replacement == 0 {
        crate::molt_del_attr_name(target, name);
    } else {
        crate::molt_set_attr_name(target, name, replacement);
    }
    !exception_pending(_py)
}

extern "C" fn lhs_mutates_rhs(_self_bits: u64, _rhs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        trace(1);
        if !mutate_configured_class(_py) {
            return MoltObject::none().bits();
        }
        crate::builtins::methods::not_implemented_bits(_py)
    })
}

extern "C" fn preferred_rhs_mutates_lhs(_self_bits: u64, _lhs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        trace(2);
        if !mutate_configured_class(_py) {
            return MoltObject::none().bits();
        }
        crate::builtins::methods::not_implemented_bits(_py)
    })
}

extern "C" fn lhs_changes_rhs_class(_self_bits: u64, rhs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        trace(1);
        let class_name = attr_name_bits_from_bytes(_py, b"__class__").unwrap();
        crate::molt_set_attr_name(
            rhs_bits,
            class_name,
            REPLACEMENT_CLASS.load(Ordering::SeqCst),
        );
        dec_ref_bits(_py, class_name);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        crate::builtins::methods::not_implemented_bits(_py)
    })
}

extern "C" fn stale_dunder(_self_bits: u64, _other_bits: u64) -> u64 {
    trace(9);
    MoltObject::from_int(99).bits()
}

extern "C" fn fresh_dunder(_self_bits: u64, _other_bits: u64) -> u64 {
    trace(3);
    MoltObject::from_int(42).bits()
}

extern "C" fn decline_dunder(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        trace(1);
        crate::builtins::methods::not_implemented_bits(_py)
    })
}

extern "C" fn descriptor_get_raises(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        trace(4);
        let kind = if DESCRIPTOR_ATTRIBUTE_ERROR.load(Ordering::SeqCst) != 0 {
            "AttributeError"
        } else {
            "RuntimeError"
        };
        raise_exception::<u64>(_py, kind, "binary descriptor bind failure")
    })
}

fn runtime_function(_py: &PyToken<'_>, name: &'static str, target: *const (), arity: u64) -> u64 {
    let ptr = alloc_runtime_function_obj(_py, runtime_fn_addr(name, target), arity);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn test_class(_py: &PyToken<'_>, name: &[u8], base: u64) -> u64 {
    let name_bits = attr_name_bits_from_bytes(_py, name).unwrap();
    let class_bits = crate::molt_class_new(name_bits);
    crate::molt_class_set_base(class_bits, base);
    let class_ptr = obj_from_bits(class_bits).as_ptr().expect("test class");
    unsafe { crate::object::class_finish_definition(_py, class_ptr) }.expect("finish test class");
    dec_ref_bits(_py, name_bits);
    assert!(!exception_pending(_py));
    class_bits
}

fn test_instance(_py: &PyToken<'_>, class_bits: u64) -> u64 {
    let class_ptr = obj_from_bits(class_bits).as_ptr().expect("test class");
    let instance = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
    assert!(!obj_from_bits(instance).is_none());
    assert!(!exception_pending(_py));
    instance
}

fn set_class_attr(_py: &PyToken<'_>, class_bits: u64, name_bits: u64, value_bits: u64) {
    crate::molt_set_attr_name(class_bits, name_bits, value_bits);
    assert!(!exception_pending(_py));
}

struct BinaryFixture {
    op_name: u64,
    rop_name: u64,
    lhs_class: u64,
    rhs_class: u64,
    lhs: u64,
    rhs: u64,
}

impl BinaryFixture {
    fn new(_py: &PyToken<'_>, name: &[u8], rhs_is_subclass: bool) -> Self {
        let object = builtin_classes(_py).object;
        let lhs_class = test_class(_py, &[name, b"Lhs"].concat(), object);
        let rhs_base = if rhs_is_subclass { lhs_class } else { object };
        let rhs_class = test_class(_py, &[name, b"Rhs"].concat(), rhs_base);
        Self {
            op_name: attr_name_bits_from_bytes(_py, b"__add__").unwrap(),
            rop_name: attr_name_bits_from_bytes(_py, b"__radd__").unwrap(),
            lhs_class,
            rhs_class,
            lhs: test_instance(_py, lhs_class),
            rhs: test_instance(_py, rhs_class),
        }
    }

    fn call(&self, _py: &PyToken<'_>) -> Option<u64> {
        unsafe { call_binary_dunder(_py, self.lhs, self.rhs, self.op_name, self.rop_name) }
    }

    fn release(self, _py: &PyToken<'_>) {
        for bits in [
            self.lhs,
            self.rhs,
            self.rhs_class,
            self.lhs_class,
            self.rop_name,
            self.op_name,
        ] {
            dec_ref_bits(_py, bits);
        }
    }
}

fn reset_mutation() {
    MUTATION_TARGET.store(0, Ordering::SeqCst);
    MUTATION_NAME.store(0, Ordering::SeqCst);
    MUTATION_REPLACEMENT.store(0, Ordering::SeqCst);
    REPLACEMENT_CLASS.store(0, Ordering::SeqCst);
}

#[test]
fn same_type_does_not_try_reflected_method_after_not_implemented() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let fixture = BinaryFixture::new(_py, b"SameTypeReflected", false);
        let forward = runtime_function(_py, "same_type_forward", decline_dunder as *const (), 2);
        let reflected = runtime_function(_py, "same_type_reflected", stale_dunder as *const (), 2);
        set_class_attr(_py, fixture.lhs_class, fixture.op_name, forward);
        set_class_attr(_py, fixture.lhs_class, fixture.rop_name, reflected);
        TRACE.store(0, Ordering::SeqCst);
        let result = unsafe {
            call_binary_dunder(
                _py,
                fixture.lhs,
                fixture.lhs,
                fixture.op_name,
                fixture.rop_name,
            )
        };
        assert!(result.is_none());
        assert_eq!(TRACE.load(Ordering::SeqCst), 1);
        assert!(!exception_pending(_py));
        fixture.release(_py);
        dec_ref_bits(_py, reflected);
        dec_ref_bits(_py, forward);
    });
}

#[test]
fn inherited_reflected_method_does_not_preempt_base_forward_method() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let fixture = BinaryFixture::new(_py, b"InheritedReflected", true);
        let forward = runtime_function(_py, "inherited_forward", fresh_dunder as *const (), 2);
        let reflected = runtime_function(_py, "inherited_reflected", stale_dunder as *const (), 2);
        set_class_attr(_py, fixture.lhs_class, fixture.op_name, forward);
        set_class_attr(_py, fixture.lhs_class, fixture.rop_name, reflected);
        TRACE.store(0, Ordering::SeqCst);
        let result = fixture.call(_py).expect("forward method result");
        assert_eq!(result, MoltObject::from_int(42).bits());
        assert_eq!(TRACE.load(Ordering::SeqCst), 3);
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, result);
        fixture.release(_py);
        dec_ref_bits(_py, reflected);
        dec_ref_bits(_py, forward);
    });
}

#[test]
fn lhs_callback_refreshes_rhs_reflected_dunder_after_rebind_or_delete() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for replace in [true, false] {
            let fixture = BinaryFixture::new(
                _py,
                if replace { b"LhsRebind" } else { b"LhsDelete" },
                false,
            );
            let lhs = runtime_function(
                _py,
                "lhs_mutates_rhs_reflected_dunder",
                lhs_mutates_rhs as *const (),
                2,
            );
            let stale = runtime_function(
                _py,
                "stale_rhs_reflected_dunder",
                stale_dunder as *const (),
                2,
            );
            let fresh = runtime_function(
                _py,
                "fresh_rhs_reflected_dunder",
                fresh_dunder as *const (),
                2,
            );
            set_class_attr(_py, fixture.lhs_class, fixture.op_name, lhs);
            set_class_attr(_py, fixture.rhs_class, fixture.rop_name, stale);

            TRACE.store(0, Ordering::SeqCst);
            MUTATION_TARGET.store(fixture.rhs_class, Ordering::SeqCst);
            MUTATION_NAME.store(fixture.rop_name, Ordering::SeqCst);
            MUTATION_REPLACEMENT.store(if replace { fresh } else { 0 }, Ordering::SeqCst);
            let result = fixture.call(_py);

            if replace {
                assert_eq!(result, Some(MoltObject::from_int(42).bits()));
                assert_eq!(TRACE.load(Ordering::SeqCst), 13);
            } else {
                assert_eq!(result, None);
                assert_eq!(TRACE.load(Ordering::SeqCst), 1);
            }
            assert!(!exception_pending(_py));
            reset_mutation();
            fixture.release(_py);
            for bits in [fresh, stale, lhs] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn preferred_rhs_callback_refreshes_lhs_dunder_after_rebind_or_delete() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for replace in [true, false] {
            let fixture =
                BinaryFixture::new(_py, if replace { b"RhsRebind" } else { b"RhsDelete" }, true);
            let rhs = runtime_function(
                _py,
                "preferred_rhs_mutates_lhs_dunder",
                preferred_rhs_mutates_lhs as *const (),
                2,
            );
            let stale = runtime_function(_py, "stale_lhs_dunder", stale_dunder as *const (), 2);
            let fresh = runtime_function(_py, "fresh_lhs_dunder", fresh_dunder as *const (), 2);
            set_class_attr(_py, fixture.lhs_class, fixture.op_name, stale);
            set_class_attr(_py, fixture.rhs_class, fixture.rop_name, rhs);

            TRACE.store(0, Ordering::SeqCst);
            MUTATION_TARGET.store(fixture.lhs_class, Ordering::SeqCst);
            MUTATION_NAME.store(fixture.op_name, Ordering::SeqCst);
            MUTATION_REPLACEMENT.store(if replace { fresh } else { 0 }, Ordering::SeqCst);
            let result = fixture.call(_py);

            if replace {
                assert_eq!(result, Some(MoltObject::from_int(42).bits()));
                assert_eq!(TRACE.load(Ordering::SeqCst), 23);
            } else {
                assert_eq!(result, None);
                assert_eq!(TRACE.load(Ordering::SeqCst), 2);
            }
            assert!(!exception_pending(_py));
            reset_mutation();
            fixture.release(_py);
            for bits in [fresh, stale, rhs] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn lhs_callback_refreshes_rhs_receiver_type_before_reflected_attempt() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let fixture = BinaryFixture::new(_py, b"ReceiverType", false);
        let updated_class = test_class(_py, b"ReceiverTypeUpdatedRhs", builtin_classes(_py).object);
        let lhs = runtime_function(
            _py,
            "lhs_changes_rhs_class",
            lhs_changes_rhs_class as *const (),
            2,
        );
        let stale = runtime_function(_py, "old_rhs_type_reflected", stale_dunder as *const (), 2);
        let fresh = runtime_function(
            _py,
            "updated_rhs_type_reflected",
            fresh_dunder as *const (),
            2,
        );
        set_class_attr(_py, fixture.lhs_class, fixture.op_name, lhs);
        set_class_attr(_py, fixture.rhs_class, fixture.rop_name, stale);
        let rop_name = fixture.rop_name;
        set_class_attr(_py, updated_class, rop_name, fresh);

        TRACE.store(0, Ordering::SeqCst);
        REPLACEMENT_CLASS.store(updated_class, Ordering::SeqCst);
        assert_eq!(fixture.call(_py), Some(MoltObject::from_int(42).bits()));
        assert_eq!(TRACE.load(Ordering::SeqCst), 13);
        assert_eq!(type_of_bits(_py, fixture.rhs), updated_class);
        assert!(!exception_pending(_py));

        reset_mutation();
        fixture.release(_py);
        dec_ref_bits(_py, updated_class);
        for bits in [fresh, stale, lhs] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn descriptor_bind_error_follows_binary_target_version_policy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = crate::runtime_state(_py);
        let version_snapshot = state.sys_version_info.lock().unwrap().clone();

        for minor in [12, 13, 14] {
            *state.sys_version_info.lock().unwrap() =
                Some(crate::state::runtime_state::PythonVersionInfo {
                    major: 3,
                    minor,
                    micro: 0,
                    releaselevel: "final".to_string(),
                    serial: 0,
                });

            for kind in ["AttributeError", "RuntimeError"] {
                let fixture_name = format!("Py3{minor}{kind}");
                let fixture = BinaryFixture::new(_py, fixture_name.as_bytes(), false);
                let getter = runtime_function(
                    _py,
                    "binary_descriptor_get_raises",
                    descriptor_get_raises as *const (),
                    1,
                );
                let property_ptr = crate::alloc_property_obj(
                    _py,
                    getter,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                );
                assert!(!property_ptr.is_null());
                let property = MoltObject::from_ptr(property_ptr).bits();
                let fallback = runtime_function(
                    _py,
                    "descriptor_error_fallback",
                    fresh_dunder as *const (),
                    2,
                );
                set_class_attr(_py, fixture.lhs_class, fixture.op_name, property);
                set_class_attr(_py, fixture.rhs_class, fixture.rop_name, fallback);

                DESCRIPTOR_ATTRIBUTE_ERROR
                    .store(u64::from(kind == "AttributeError"), Ordering::SeqCst);
                TRACE.store(0, Ordering::SeqCst);
                let result = fixture.call(_py);
                if minor >= 14 && kind == "AttributeError" {
                    assert_eq!(result, Some(MoltObject::from_int(42).bits()));
                    assert_eq!(TRACE.load(Ordering::SeqCst), 43);
                    assert!(!exception_pending(_py));
                    dec_ref_bits(_py, result.expect("reflected fallback result"));
                } else {
                    assert_eq!(result, Some(MoltObject::none().bits()));
                    assert_eq!(TRACE.load(Ordering::SeqCst), 4);
                    assert!(exception_pending(_py));
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        _py, error, kind
                    ));
                    crate::molt_exception_clear();
                    dec_ref_bits(_py, error);
                }

                fixture.release(_py);
                for bits in [fallback, property, getter] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            }
        }
        *state.sys_version_info.lock().unwrap() = version_snapshot;
        DESCRIPTOR_ATTRIBUTE_ERROR.store(0, Ordering::SeqCst);
    });
}
