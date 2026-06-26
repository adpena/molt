use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ReturnAliasSummary {
    Param(usize),
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
fn alias_source_name<'a>(
    op: &'a OpIR,
    summaries: &BTreeMap<String, ReturnAliasSummary>,
) -> Option<&'a str> {
    match op.kind.as_str() {
        "copy" | "box" | "unbox" | "cast" | "widen" | "identity_alias" | "binding_alias" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        "copy_var" | "load_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str)
            .or(op.var.as_deref()),
        "store_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        "call" => {
            let callee = op.s_value.as_ref()?;
            let ReturnAliasSummary::Param(param_idx) = *summaries.get(callee)?;
            op.args
                .as_ref()
                .and_then(|args| args.get(param_idx))
                .map(String::as_str)
        }
        _ => None,
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
fn compute_function_return_alias_summary(
    func: &FunctionIR,
    known: &BTreeMap<String, ReturnAliasSummary>,
) -> Option<ReturnAliasSummary> {
    let trace_alias = std::env::var("MOLT_DEBUG_RETURN_ALIAS").as_deref() == Ok("1");
    let mut alias_roots: BTreeMap<String, String> = BTreeMap::new();
    for param in &func.params {
        if param != "none" {
            alias_roots.insert(param.clone(), param.clone());
        }
    }

    for op in &func.ops {
        let logical_out = op.out.as_ref().or_else(|| {
            if op.kind == "store_var" {
                op.var.as_ref()
            } else {
                None
            }
        });
        let Some(out) = logical_out else {
            continue;
        };
        if out == "none" {
            continue;
        }
        if let Some(src) = alias_source_name(op, known) {
            let root = alias_roots
                .get(src)
                .cloned()
                .unwrap_or_else(|| src.to_string());
            alias_roots.insert(out.clone(), root);
            if trace_alias {
                eprintln!(
                    "[molt alias] func={} op={} out={} src={} root={}",
                    func.name, op.kind, out, src, alias_roots[out]
                );
            }
        }
    }

    let const_none_names: BTreeSet<&str> = func
        .ops
        .iter()
        .filter(|op| op.kind == "const_none")
        .filter_map(|op| op.out.as_deref())
        .collect();

    let mut summary: Option<ReturnAliasSummary> = None;
    let mut saw_ret = false;
    for (ret_idx, op) in func.ops.iter().enumerate() {
        match op.kind.as_str() {
            "ret" => {
                let ret_name = op.var.as_ref()?;
                if const_none_names.contains(ret_name.as_str()) {
                    let mut scan_idx = ret_idx;
                    let mut synthetic_raise_tail = false;
                    while scan_idx > 0 {
                        scan_idx -= 1;
                        let prev = &func.ops[scan_idx];
                        match prev.kind.as_str() {
                            "const_none" if prev.out.as_deref() == Some(ret_name.as_str()) => {}
                            "line" | "check_exception" => {}
                            "raise" => {
                                synthetic_raise_tail = true;
                                break;
                            }
                            _ => break,
                        }
                    }
                    if synthetic_raise_tail {
                        continue;
                    }
                }
                saw_ret = true;
                let root = alias_roots
                    .get(ret_name)
                    .cloned()
                    .unwrap_or_else(|| ret_name.clone());
                let param_idx = func.params.iter().position(|param| param == &root)?;
                if trace_alias {
                    eprintln!(
                        "[molt alias] func={} ret_name={} root={} param_idx={}",
                        func.name, ret_name, root, param_idx
                    );
                }
                let current = ReturnAliasSummary::Param(param_idx);
                match summary {
                    None => summary = Some(current),
                    Some(existing) if existing == current => {}
                    Some(_) => return None,
                }
            }
            "ret_void" => {}
            _ => {}
        }
    }

    saw_ret.then_some(summary).flatten()
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
pub fn compute_return_alias_summaries(
    functions: &[FunctionIR],
) -> BTreeMap<String, ReturnAliasSummary> {
    let mut summaries: BTreeMap<String, ReturnAliasSummary> = BTreeMap::new();
    loop {
        let mut changed = false;
        for func in functions {
            let next = compute_function_return_alias_summary(func, &summaries);
            match next {
                Some(summary) => {
                    if summaries.get(&func.name).copied() != Some(summary) {
                        summaries.insert(func.name.clone(), summary);
                        changed = true;
                    }
                }
                None => {
                    if summaries.remove(&func.name).is_some() {
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return summaries;
        }
    }
}
