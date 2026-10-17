// CPython compatibility outcomes projected by tools/compatibility_error_protocol.py.
use crate::{
    ExceptionSentinel, MoltObject, PyToken, obj_from_bits, raise_exception, runtime_state,
};

pub(crate) enum CompatibilityError<'a> {
    PathGlobNonRelative {
        non_relative: bool,
    },
    CounterFromkeys,
    WindowsSignalHandler,
    AbstractSignalHandler,
    RawIoBaseReadinto,
    MemoryviewSubView {
        rank: usize,
        indices: usize,
        tuple: bool,
        assignment: bool,
    },
    MemoryviewSlice {
        rank: usize,
        slices: usize,
        tuple: bool,
        assignment: bool,
    },
    MemoryviewLookup {
        rank: usize,
    },
    MemoryviewFormatSyntax {
        format: &'a str,
    },
    MemoryviewFormatScalar {
        format: &'a str,
    },
}

impl CompatibilityError<'_> {
    fn message(self, py: &PyToken<'_>) -> Result<String, &'static str> {
        let target = crate::object::ops_sys::runtime_target_python_info(runtime_state(py));
        let supported_minor = match &self {
            Self::RawIoBaseReadinto => matches!(target.minor, 12..=15),
            _ => matches!(target.minor, 12..=14),
        };
        if target.major != 3 || !supported_minor {
            return Err("unsupported compatibility-error target");
        }
        let message = match self {
            Self::PathGlobNonRelative { non_relative } if non_relative => {
                "Non-relative patterns are unsupported".to_owned()
            }
            Self::CounterFromkeys => {
                "Counter.fromkeys() is undefined.  Use Counter(iterable) instead.".to_owned()
            }
            Self::WindowsSignalHandler if cfg!(target_os = "windows") => String::new(),
            Self::AbstractSignalHandler => String::new(),
            Self::RawIoBaseReadinto => String::new(),
            Self::MemoryviewSubView {
                rank,
                indices,
                tuple,
                assignment,
            } if rank > 0 && indices < rank => {
                if tuple || assignment {
                    "sub-views are not implemented".to_owned()
                } else {
                    "multi-dimensional sub-views are not implemented".to_owned()
                }
            }
            Self::MemoryviewSlice {
                rank,
                slices,
                tuple,
                assignment,
            } if rank > 0 && slices > 0 && (tuple || (assignment && rank > 1)) => {
                if assignment {
                    "memoryview slice assignments are currently restricted to ndim = 1".to_owned()
                } else {
                    "multi-dimensional slicing is not implemented".to_owned()
                }
            }
            Self::MemoryviewLookup { rank } if rank > 1 && target.minor == 14 => {
                "multi-dimensional lookup is not implemented".to_owned()
            }
            Self::MemoryviewFormatSyntax { format }
                if format.strip_prefix('@').unwrap_or(format).len() != 1 =>
            {
                format!("memoryview: unsupported format {format}")
            }
            Self::MemoryviewFormatScalar { format }
                if format.len() == 1 && !b"bBhHiIlLqQnNefd?cP".contains(&format.as_bytes()[0]) =>
            {
                format!("memoryview: format {format} not supported")
            }
            _ => {
                return Err("inapplicable compatibility-error outcome");
            }
        };
        Ok(message)
    }

    pub(crate) fn raise<T: ExceptionSentinel>(self, py: &PyToken<'_>) -> T {
        match self.message(py) {
            Ok(message) => raise_exception(py, "NotImplementedError", &message),
            Err(message) => raise_exception(py, "SystemError", message),
        }
    }

    fn exception(self, py: &PyToken<'_>) -> u64 {
        let message = match self.message(py) {
            Ok(message) => message,
            Err(message) => return raise_exception(py, "SystemError", message),
        };
        let ptr = crate::builtins::exceptions::alloc_exception(py, "NotImplementedError", &message);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compatibility_error(outcome_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let outcome = match obj_from_bits(outcome_bits).as_int() {
            Some(1) => CompatibilityError::CounterFromkeys,
            Some(2) => CompatibilityError::WindowsSignalHandler,
            Some(3) => CompatibilityError::AbstractSignalHandler,
            _ => {
                return raise_exception(
                    py,
                    "SystemError",
                    "inapplicable compatibility-error outcome",
                );
            }
        };
        outcome.exception(py)
    })
}
#[cfg(test)]
mod tests {
    use super::CompatibilityError;

    #[test]
    fn raw_readinto_target_support_does_not_widen_older_outcomes() {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for minor in 12..=15 {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::RawIoBaseReadinto.message(py).as_deref(),
                        Ok("")
                    );
                });
            }
            for minor in [11, 16] {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::RawIoBaseReadinto.message(py),
                        Err("unsupported compatibility-error target"),
                    );
                });
            }
            transaction.with_target_python_minor(py, 15, || {
                assert_eq!(
                    CompatibilityError::MemoryviewLookup { rank: 2 }.message(py),
                    Err("unsupported compatibility-error target"),
                );
                assert_eq!(
                    CompatibilityError::CounterFromkeys.message(py),
                    Err("unsupported compatibility-error target"),
                );
            });
        });
    }

    #[test]
    fn glob_outcome_rejects_relative_facts_and_unsupported_targets() {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for minor in 12..=14 {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(
                        CompatibilityError::PathGlobNonRelative { non_relative: true }
                            .message(py)
                            .as_deref(),
                        Ok("Non-relative patterns are unsupported"),
                    );
                    assert_eq!(
                        CompatibilityError::PathGlobNonRelative {
                            non_relative: false
                        }
                        .message(py),
                        Err("inapplicable compatibility-error outcome"),
                    );
                });
            }
            transaction.with_target_python_minor(py, 15, || {
                assert_eq!(
                    CompatibilityError::PathGlobNonRelative { non_relative: true }.message(py),
                    Err("unsupported compatibility-error target"),
                );
            });
        });
    }
}
