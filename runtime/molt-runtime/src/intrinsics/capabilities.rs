use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::{
    MoltObject, capability_fix_hint, has_capability, is_trusted, raise_exception,
    string_obj_to_owned,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_trusted() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(is_trusted(_py)).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_capabilities_has(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
