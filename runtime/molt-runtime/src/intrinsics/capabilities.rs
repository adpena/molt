use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::{
    MoltObject, capability_fix_hint, has_capability, is_trusted, raise_exception,
    string_obj_to_owned,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_list() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = has_capability(py, "_init");
        let mut names: Vec<&str> = crate::runtime_state(py)
            .capabilities
            .get()
            .expect("frozen capabilities")
            .iter()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        let mut values = Vec::with_capacity(names.len());
        for name in names {
            let pointer = crate::alloc_string(py, name.as_bytes());
            if pointer.is_null() {
                return MoltObject::none().bits();
            }
            values.push(unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(pointer).bits(),
                )
            });
        }
        let list = crate::alloc_list(
            py,
            molt_runtime_core::OwnedRuntimeValue::as_bits_slice(&values),
        );
        if list.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_trusted() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(is_trusted(_py)).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_has(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(crate::obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "capability name must be str"),
        };
        let allowed = has_capability(_py, &name);
        {
            let decision = if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {name} capability"),
                }
            };
            audit_emit(AuditEvent::new(
                "capability.has",
                "capability.has",
                AuditArgs::Custom(name),
                decision,
                module_path!().to_string(),
            ));
        }
        MoltObject::from_bool(allowed).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_require(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_obj_to_owned(crate::obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "capability name must be str"),
        };
        let allowed = has_capability(_py, &name);
        // Emit audit event for the explicit require() intrinsic.
        // Because `name` is a runtime string we build the event manually.
        {
            let decision = if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {name} capability"),
                }
            };
            audit_emit(AuditEvent::new(
                "capability.require",
                "capability.require",
                AuditArgs::Custom(name.clone()),
                decision,
                module_path!().to_string(),
            ));
        }
        if !allowed {
            let hint = capability_fix_hint(&name);
            return raise_exception::<_>(
                _py,
                "PermissionError",
                &format!("missing '{name}' capability. {hint}"),
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_require_operation(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // The intrinsic caller owns its argument until this call returns. Borrow
        // the operation identifier rather than allocating on successful checks.
        let outcome = unsafe {
            crate::object::ops_format::with_string_bytes(crate::obj_from_bits(name_bits), |bytes| {
                let Ok(name) = std::str::from_utf8(bytes) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "host operation name must contain only valid Unicode scalar values",
                    ));
                };
                crate::require_named_operation::<u64>(_py, name, AuditArgs::None)
            })
        };
        match outcome {
            Some(Ok(())) => MoltObject::none().bits(),
            Some(Err(error)) => error,
            None => raise_exception::<u64>(_py, "TypeError", "host operation name must be str"),
        }
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{PyToken, dec_ref_bits, exception_pending, obj_from_bits};

    fn name_bits(py: &PyToken<'_>, name: &str) -> u64 {
        let pointer = crate::alloc_string(py, name.as_bytes());
        assert!(!pointer.is_null());
        MoltObject::from_ptr(pointer).bits()
    }

    fn take_error(py: &PyToken<'_>) -> (String, String) {
        assert!(exception_pending(py));
        let error = crate::molt_exception_last();
        crate::molt_exception_clear();
        let kind = crate::type_name(py, obj_from_bits(error)).to_string();
        let message = crate::builtins::exceptions::molt_exception_message(error);
        let text = string_obj_to_owned(obj_from_bits(message)).unwrap();
        dec_ref_bits(py, message);
        dec_ref_bits(py, error);
        (kind, text)
    }

    #[test]
    fn named_pool_preflight_reports_complete_missing_set_for_every_grant_subset() {
        let required = ["os.cpu_count", "thread.spawn", "thread.shared"];
        for mask in 0..8 {
            let grants = required
                .iter()
                .enumerate()
                .filter_map(|(index, &name)| (mask & (1 << index) != 0).then_some(name))
                .collect::<Vec<_>>()
                .join(",");
            let missing = required
                .iter()
                .enumerate()
                .filter_map(|(index, &name)| (mask & (1 << index) == 0).then_some(name))
                .collect::<Vec<_>>()
                .join(", ");
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &grants,
                |py| {
                    for operation in [
                        "concurrent.threadpool.default",
                        "concurrent.processpool.default",
                    ] {
                        let name = name_bits(py, operation);
                        let result = molt_capabilities_require_operation(name);
                        dec_ref_bits(py, name);
                        assert_eq!(result, MoltObject::none().bits());
                        if missing.is_empty() {
                            assert!(!exception_pending(py));
                        } else {
                            assert_eq!(
                                take_error(py),
                                (
                                    "PermissionError".into(),
                                    format!("missing [{missing}] capabilities for '{operation}'"),
                                )
                            );
                        }
                    }
                },
            );
        }
    }

    #[test]
    fn multiprocessing_spawn_reports_complete_selected_missing_set() {
        for (grants, missing) in [
            ("", Some("process.exec, env.read")),
            ("process.exec", Some("env.read")),
            ("env.read", Some("process.exec")),
            ("process.exec,env.read", None),
        ] {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
                let name = name_bits(py, "multiprocessing.spawn");
                let result = molt_capabilities_require_operation(name);
                dec_ref_bits(py, name);
                assert_eq!(result, MoltObject::none().bits());
                if let Some(missing) = missing {
                    assert_eq!(
                        take_error(py),
                        (
                            "PermissionError".to_string(),
                            format!("missing [{missing}] capabilities for 'multiprocessing.spawn'")
                        )
                    );
                } else {
                    assert!(!exception_pending(py));
                }
            });
        }
    }

    #[test]
    fn explicit_pool_creation_does_not_request_cpu_information() {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "thread.spawn,thread.shared",
            |py| {
                assert!(!crate::has_capability(py, "os.cpu_count"));
                let handle = crate::builtins::concurrent::molt_concurrent_threadpool_new(
                    MoltObject::from_int(1).bits(),
                );
                assert!(!exception_pending(py));
                crate::builtins::concurrent::molt_concurrent_threadpool_drop(handle);
                assert!(!exception_pending(py));
            },
        );
    }

    #[test]
    fn named_operation_validation_is_fail_closed() {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("", |py| {
            let name = name_bits(py, "unknown.operation");
            molt_capabilities_require_operation(name);
            dec_ref_bits(py, name);
            assert_eq!(
                take_error(py),
                (
                    "ValueError".into(),
                    "unknown host operation 'unknown.operation'".into()
                )
            );
            molt_capabilities_require_operation(MoltObject::from_int(1).bits());
            assert_eq!(
                take_error(py),
                ("TypeError".into(), "host operation name must be str".into())
            );
        });
    }
}
