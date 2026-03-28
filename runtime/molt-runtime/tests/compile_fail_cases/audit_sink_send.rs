//! Verify that non-Send types cannot be installed as audit sinks.
//! AuditSink requires Send + Sync, so types containing Rc (which is !Send)
//! must be rejected at compile time.

use std::rc::Rc;

struct BadSink(Rc<()>); // Rc is !Send

impl molt_runtime::audit::AuditSink for BadSink {
    fn emit(&self, _event: &molt_runtime::audit::AuditEvent) {}
}

fn main() {
    // ERROR: BadSink does not implement Send (required by AuditSink bound)
    molt_runtime::audit::set_audit_sink(Box::new(BadSink(Rc::new(()))));
}
