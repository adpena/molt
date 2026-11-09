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
pub(crate) fn molt_rt_object_type_id_attached_body(_py: &PyToken<'_>, ptr: *mut u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe { object_type_id(ptr) }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

#[unsafe(export_name = molt_codegen_abi::attached_symbol!("__molt_tk_has_capability"))]
pub extern "C" fn __molt_tk_has_capability(
    __molt_context: u64,
    name_ptr: *const u8,
    name_len: usize,
) -> i32 {
    {
        let _py = unsafe { crate::PyToken::from_attached_context(__molt_context) };
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
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/attached_exports_generated/tk_bridge.rs"
));
