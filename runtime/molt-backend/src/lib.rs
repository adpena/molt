#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers

use std::fmt::Write as _;

// The TIR lower layer now lives in the molt-tir crate (decomposition doc 21, move
// T1). Re-export its modules at this crate root so molt-backend's existing
// `crate::tir::*` / `crate::passes::*` / `crate::ir::*` /
// `crate::repr::*` / `crate::representation_plan::*` / leaf-util paths resolve unchanged.
pub use molt_tir::{
    debug_artifacts, intrinsic_symbols, ir, ir_schema, json_boundary, passes, process_diagnostics,
    repr, representation_plan, tir,
};

pub use molt_tir::intrinsic_symbols::{
    runtime_intrinsic_symbols_from_env, runtime_intrinsic_symbols_required,
};
mod ir_rewrites;
pub use crate::ir_rewrites::{
    elide_useless_try_blocks, elide_useless_try_blocks_for_function, rewrite_annotate_stubs,
    rewrite_copy_aliases, rewrite_phi_to_store_load,
};
#[cfg(feature = "llvm")]
pub mod llvm_backend;
pub mod luau_ir;
pub mod luau_lower;
#[cfg(feature = "native-backend")]
mod native_backend;
#[cfg(feature = "native-backend")]
pub use crate::native_backend::{CompileOutput, NativeBackendModuleContext, SimpleBackend};
#[cfg(feature = "native-backend")]
pub(crate) use crate::native_backend::{
    DeferredDefine, NanBoxConsts, VarValue, block_has_terminator, extend_unique_tracked,
    switch_to_block_tracking, unbox_int,
};
#[cfg(any(feature = "native-backend", feature = "llvm"))]
mod native_backend_consts;
#[cfg(any(feature = "native-backend", feature = "llvm"))]
use native_backend_consts::*;
mod stdlib_module_symbols;
pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
pub use crate::passes::{
    apply_profile_order, build_const_int_map, canonicalize_direct_raise_edges,
    compute_intrinsic_manifest, compute_intrinsic_manifest_checked, elide_dead_struct_allocs,
    elide_safe_exception_checks, eliminate_dead_functions, eliminate_dead_imports,
    eliminate_dead_ops, eliminate_redundant_guard_tags, eliminate_unbound_local_checks,
    escape_analysis, fold_constants, fold_constants_cross_block, fuse_method_dispatch,
    hoist_loop_invariants, inject_runtime_exit, rc_coalescing, rewrite_stateful_loops,
    split_megafunctions,
};
pub use crate::stdlib_module_symbols::{
    STDLIB_MODULE_SYMBOLS_ENV, parse_stdlib_module_symbols, stdlib_module_symbols_from_env,
};
pub use molt_tir::MOLT_CLOSURE_PARAM_NAME;
/// The representation lattice element (the orthogonal carrier axis to
/// `TirType`). Re-exported publicly because it appears in the signature of the
/// `pub` `tir::lower_to_lir::lower_function_to_lir` (Phase 1 of the typed-IR
/// convergence), which the WASM/LIR codegen path drives with the proven
/// `repr_by_value`.
pub use molt_tir::repr::Repr;

#[cfg(feature = "luau-backend")]
pub mod luau;
#[cfg(feature = "rust-backend")]
pub mod rust;
#[cfg(feature = "wasm-backend")]
pub mod wasm;
#[cfg(feature = "wasm-backend")]
mod wasm_abi;
#[cfg(feature = "wasm-backend")]
mod wasm_binary;
#[cfg(feature = "wasm-backend")]
mod wasm_dispatch;
#[cfg(feature = "wasm-backend")]
mod wasm_imports;
#[cfg(feature = "wasm-backend")]
mod wasm_plan;
#[cfg(feature = "wasm-backend")]
mod wasm_values;

#[cfg(feature = "egraphs")]
pub mod egraph_simplify;

#[cfg(any(feature = "native-backend", feature = "llvm"))]
fn pending_bits() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

#[cfg(any(feature = "native-backend", feature = "llvm"))]
pub(crate) fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for b in func_name
        .as_bytes()
        .iter()
        .chain(lane.as_bytes().iter())
        .copied()
    {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= op_idx as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    // Keep the id within inline-int payload range and avoid zero.
    let id = (hash & ((1u64 << 46) - 1)).max(1);
    id as i64
}

struct DumpIrConfig {
    mode: String,
    filter: Option<String>,
}

pub(crate) fn should_dump_ir() -> Option<DumpIrConfig> {
    let raw = std::env::var("MOLT_DUMP_IR").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let (mode, filter) = if let Some((left, right)) = trimmed.split_once(':') {
        let left_trim = left.trim();
        let right_trim = right.trim();
        let mode = if left_trim.eq_ignore_ascii_case("full") {
            "full"
        } else {
            "control"
        };
        let filter = if right_trim.is_empty() {
            None
        } else {
            Some(right_trim.to_string())
        };
        (mode.to_string(), filter)
    } else if lower == "full" || lower == "control" || lower == "1" || lower == "all" {
        let mode = if lower == "full" { "full" } else { "control" };
        (mode.to_string(), None)
    } else {
        ("control".to_string(), Some(trimmed.to_string()))
    };
    Some(DumpIrConfig { mode, filter })
}

pub(crate) fn dump_ir_matches(config: &DumpIrConfig, func_name: &str) -> bool {
    let Some(filter) = config.filter.as_ref() else {
        return true;
    };
    if filter == "1" || filter.eq_ignore_ascii_case("all") {
        return true;
    }
    func_name == filter || func_name.contains(filter)
}

pub(crate) fn dump_ir_ops(func_ir: &FunctionIR, mode: &str) {
    let mut out = String::new();
    let full = mode.eq_ignore_ascii_case("full");
    let mut last_written = 0usize;
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !full {
            let kind = op.kind.as_str();
            let is_control = matches!(
                kind,
                "if" | "else"
                    | "end_if"
                    | "phi"
                    | "label"
                    | "state_label"
                    | "jump"
                    | "br_if"
                    | "loop_start"
                    | "loop_end"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_break_if_exception"
                    | "loop_break"
                    | "loop_continue"
                    | "ret"
            );
            if !is_control {
                continue;
            }
        }
        let mut detail = Vec::new();
        if let Some(out_name) = &op.out {
            detail.push(format!("out={out_name}"));
        }
        if let Some(var) = &op.var {
            detail.push(format!("var={var}"));
        }
        if let Some(args) = &op.args {
            detail.push(format!("args=[{}]", args.join(", ")));
        }
        if let Some(val) = op.value {
            detail.push(format!("value={val}"));
        }
        if let Some(val) = op.f_value {
            detail.push(format!("f_value={val}"));
        }
        if let Some(val) = &op.s_value {
            detail.push(format!("s_value={val}"));
        }
        if let Some(bytes) = &op.bytes {
            detail.push(format!("bytes_len={}", bytes.len()));
        }
        if let Some(fast_int) = op.fast_int {
            detail.push(format!("fast_int={fast_int}"));
        }
        let _ = writeln!(out, "{idx:04}: {:<20} {}", op.kind, detail.join(" "));
        last_written = idx;
    }
    if last_written == 0 && func_ir.ops.is_empty() {
        return;
    }
    eprintln!("IR ops for {} (mode={}):\n{}", func_ir.name, mode, out);
    if std::env::var("MOLT_DUMP_IR_FILE").as_deref() == Ok("1") {
        let _ = std::fs::create_dir_all("logs");
        let sanitized = func_ir
            .name
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
                _ => '_',
            })
            .collect::<String>();
        let path = std::path::Path::new("logs").join(format!("ir_dump_{sanitized}.log"));
        let _ = std::fs::write(path, &out);
    }
}

#[derive(
    Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug, serde::Deserialize, serde::Serialize,
)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) enum TrampolineKind {
    Plain,
    Generator,
    Coroutine,
    AsyncGen,
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct TrampolineSpec {
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
    /// Whether the target function returns a value. Trampolines use this
    /// to set the correct import signature — functions with ret_void only
    /// don't have a return in their signature.
    #[cfg_attr(
        not(any(feature = "native-backend", feature = "llvm")),
        allow(dead_code)
    )]
    pub(crate) target_has_ret: bool,
}

const EXTERN_SIGNATURE_RETURN_VALUE: &str = "__molt_extern_signature_return";

fn function_body_requires_value_return(func: &FunctionIR) -> bool {
    func.ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "ret"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
        )
    })
}

pub fn externalize_function_with_signature(func: &mut FunctionIR) {
    let returns_value = function_body_requires_value_return(func);
    func.is_extern = true;
    func.ops = if returns_value {
        vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some(EXTERN_SIGNATURE_RETURN_VALUE.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec![EXTERN_SIGNATURE_RETURN_VALUE.to_string()]),
                ..OpIR::default()
            },
        ]
    } else {
        vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }]
    };
}

pub(crate) fn function_requires_value_return(func: &FunctionIR) -> bool {
    if func.is_extern {
        assert!(
            !func.ops.is_empty(),
            "extern function `{}` is missing return-signature metadata",
            func.name
        );
    }
    function_body_requires_value_return(func)
}

pub(crate) fn env_setting(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}
