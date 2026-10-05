//! Source-raise admission precedes every instance-only metadata operation.
//! Transport and deferred publication retain the admitted instance identity.

use super::*;

/// Borrow a class or instance and return one owned exception instance. Ordinary
/// raises and runtime class ingress share this authority; throw's separate
/// class/value/traceback restoration protocol remains in `throw_protocol`.
pub(super) fn normalize_raise_operand(py: &PyToken<'_>, bits: u64, cause: bool) -> Option<u64> {
    let operand = ExceptionValue::pin(py, bits);
    if exception_is_instance(py, bits) {
        return Some(operand.into_bits());
    }
    if !exception_is_class(py, bits) {
        return raise_exception::<_>(
            py,
            "TypeError",
            if cause {
                "exception causes must derive from BaseException"
            } else {
                "exceptions must derive from BaseException"
            },
        );
    }
    let instance = ExceptionValue::adopt(py, unsafe { crate::call_callable0(py, bits) });
    if exception_pending(py) {
        return None;
    }
    if !exception_is_instance(py, instance.bits()) {
        let class = format_obj(py, obj_from_bits(bits));
        let actual = format_obj(py, obj_from_bits(type_of_bits(py, instance.bits())));
        if exception_pending(py) {
            return None;
        }
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "calling {class} should have returned an instance of BaseException, not {actual}"
            ),
        );
    }
    Some(instance.into_bits())
}

/// Both source expressions have already been evaluated, exception first and
/// cause second. MISSING means no `from` clause; None is an explicit cause.
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_prepare_raise(exc_bits: u64, cause_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _cause = ExceptionValue::pin(py, cause_bits);
        let Some(exception) = normalize_raise_operand(py, exc_bits, false) else {
            return MoltObject::none().bits();
        };
        let exception = ExceptionValue::adopt(py, exception);
        if cause_bits != crate::missing_bits(py) {
            let cause = if obj_from_bits(cause_bits).is_none() {
                ExceptionValue::pin(py, cause_bits)
            } else {
                let Some(cause) = normalize_raise_operand(py, cause_bits, true) else {
                    return MoltObject::none().bits();
                };
                ExceptionValue::adopt(py, cause)
            };
            if let Err(message) = exception_replace_field_bits(
                py,
                exception.bits(),
                ExceptionFieldSlot::Cause,
                cause.bits(),
            ) {
                if !exception_pending(py) {
                    return raise_exception::<u64>(py, "TypeError", message);
                }
                return MoltObject::none().bits();
            }
        }
        exception.into_bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_raise_admission_normalizes_classes_and_preserves_instance_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = exception_type_bits_from_name(py, "ValueError");
            let cause_class = exception_type_bits_from_name(py, "KeyError");
            let instance =
                ExceptionValue::adopt(py, molt_exception_prepare_raise(class, cause_class));
            assert!(!exception_pending(py));
            assert!(exception_matches_type(py, instance.bits(), class));
            let cause = exception_field(py, instance.bits(), ExceptionFieldSlot::Cause).unwrap();
            assert!(exception_matches_type(py, cause.bits(), cause_class));
            let again = ExceptionValue::adopt(
                py,
                molt_exception_prepare_raise(instance.bits(), crate::missing_bits(py)),
            );
            assert_eq!(again.bits(), instance.bits());
            assert!(exception_field(py, class, ExceptionFieldSlot::Cause).is_none());
            assert!(!exception_pending(py));
            let suppressed = ExceptionValue::adopt(
                py,
                molt_exception_prepare_raise(instance.bits(), MoltObject::none().bits()),
            );
            assert_eq!(suppressed.bits(), instance.bits());
            assert!(
                obj_from_bits(
                    exception_field(py, instance.bits(), ExceptionFieldSlot::Cause)
                        .unwrap()
                        .bits()
                )
                .is_none()
            );
        });
    }

    #[test]
    fn source_raise_admission_rejects_invalid_causes_before_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = exception_type_bits_from_name(py, "ValueError");
            let result = molt_exception_prepare_raise(class, MoltObject::from_int(1).bits());
            assert!(obj_from_bits(result).is_none());
            let error = exception_last_bits_noinc(py).unwrap();
            assert!(exception_matches_builtin_name(py, error, "TypeError"));
            assert_eq!(
                format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                "exception causes must derive from BaseException"
            );
            clear_exception(py);
        });
    }
}
