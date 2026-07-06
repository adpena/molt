use super::rust_ident;
use crate::OpIR;
use std::collections::BTreeSet;

pub(super) fn rust_value(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        rust_ident(name)
    }
}

pub(super) fn rust_clone(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        format!("{}.clone()", rust_ident(name))
    }
}

pub(super) fn rust_slot_key(offset: i64) -> String {
    format!("MoltValue::Str(\"__slot_{offset}\".to_string())")
}

pub(super) fn is_assignable_var(name: &str) -> bool {
    !(name.is_empty() || name == "_" || name == "none")
}

pub(super) fn out_var(op: &OpIR) -> String {
    rust_ident(op.out.as_deref().unwrap_or("_"))
}

pub(super) fn declare_molt_value(out_name: &str, rhs: &str, hoisted: &BTreeSet<String>) -> String {
    if hoisted.contains(out_name) {
        format!("{out_name} = {rhs};")
    } else {
        format!("let mut {out_name}: MoltValue = {rhs};")
    }
}

pub(super) fn var_ref(op: &OpIR) -> String {
    rust_ident(op.var.as_deref().unwrap_or("_"))
}

pub(super) fn arg0(op: &OpIR) -> String {
    op.args
        .as_deref()
        .and_then(|a| a.first())
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string())
}

pub(super) fn args2(op: &OpIR) -> (String, String) {
    let args = op.args.as_deref().unwrap_or(&[]);
    let a = args
        .first()
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string());
    let b = args
        .get(1)
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string());
    (a, b)
}

pub(super) fn rust_stub_marker(op: &OpIR, reason: impl Into<String>) -> String {
    let mut detail = format!("MOLT_STUB: {}: {}", op.kind, reason.into());
    if let Some(out) = op.out.as_deref().filter(|out| !out.is_empty()) {
        detail.push_str(&format!(" -> `{out}`"));
    }
    if let Some(args) = op.args.as_ref().filter(|args| !args.is_empty()) {
        detail.push_str(&format!(" args=({})", args.join(", ")));
    }
    detail
}

pub(super) fn rust_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}
