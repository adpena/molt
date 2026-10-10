//! FFI bridge shims for `molt-runtime-tk`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The tk crate declares matching
//! `unsafe extern "C"` imports and they are resolved at link time.

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_emit};
use crate::*;

// ---------------------------------------------------------------------------
// Object layout access
// ---------------------------------------------------------------------------

/// Return the type-id tag for the object at `ptr`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_rt_object_type_id(ptr: *mut u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe { object_type_id(ptr) }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_tk_has_capability(name_ptr: *const u8, name_len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len))
        };
        let allowed = crate::has_capability(_py, name);
        let decision = if allowed {
            AuditDecision::Allowed
        } else {
            AuditDecision::Denied {
                reason: format!("missing {name} capability"),
            }
        };
        audit_emit(AuditEvent::new(
            "tk.has_capability",
            "tk.has_capability",
            AuditArgs::Custom(name.to_string()),
            decision,
            module_path!().to_string(),
        ));
        if allowed { 1 } else { 0 }
    })
}
