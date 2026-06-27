//! Luau transpiler backend for Molt.
//!
//! Transpiles `SimpleIR` → Luau source code suitable for Roblox Studio.
//! Unlike the native/WASM backends that emit binary, this produces a `.luau`
//! text file that can be executed directly in Roblox's Luau VM.
//!
//! This backend is intentionally a preview target. Production build paths must
//! reject lowered output that still contains comment-only control-flow markers
//! or stub markers for unsupported semantics.

use crate::repr::{ContainerKind, ScalarKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[path = "luau_backend/ir_rewrites.rs"]
mod ir_rewrites;
use ir_rewrites::{
    hoist_exception_edge_block_arg_stores, lower_early_returns, lower_iter_to_for,
    lower_try_to_pcall, strip_dead_after_return,
};

#[path = "luau_backend/source_postprocess.rs"]
mod source_postprocess;
use source_postprocess::optimize_luau_source;

#[path = "luau_backend/source_checks.rs"]
mod source_checks;
pub use source_checks::{review_luau_perf, validate_luau_source};

mod compile_pipeline;
mod function_body;
mod helpers;
mod op_attributes;
mod op_calls;
mod op_container_access;
mod op_control;
mod op_emitter;
mod op_exceptions;
mod op_iteration;
mod op_lists;
mod op_maps;
mod op_objects;
mod op_pcall;
mod op_returns;
mod op_runtime_surface;
mod op_scalar_builtins;
mod op_scalar_exprs;
mod op_scalar_kernels;
mod op_scalars;
mod op_sets;
mod op_strings;
mod op_tuples;
mod op_values;

/// Transpiles Molt `SimpleIR` into Luau source text.
pub struct LuauBackend {
    output: String,
    /// Current indentation level (number of tabs).
    indent: usize,
    uses_forward_decls: bool,
    /// Variables that have been pre-declared at function scope and should use
    /// assignment (`var = val`) instead of `local var = val` in emit_op.
    hoisted_vars: BTreeSet<String>,
    /// Variables produced by `tuple_new` / `tuple_from_list` ops.  When one of
    /// these is returned we emit `return table.unpack(v)` so the caller
    /// receives multiple values instead of a single table.
    tuple_vars: BTreeSet<String>,
    /// Backend-neutral scalar representation facts for the function currently
    /// being emitted.
    scalar_plan: ScalarRepresentationPlan,
    /// Stack of pcall counter values for nested try/except blocks.
    try_depth_counter: Vec<u32>,
    /// Monotonically increasing counter for generating unique pcall variable names.
    pcall_counter: u32,
    /// True when we are inside a pcall body (between pcall_wrap_begin and
    /// pcall_wrap_end). exception_last should return nil in this zone.
    inside_pcall_body: bool,
    /// Variables known to hold non-negative integer constants.  Populated from
    /// `const` / `const_int` ops with `value >= 0`.  Used to skip the negative
    /// index ternary in get_item / set_item / del_item / string index paths.
    nonneg_consts: BTreeSet<String>,
    /// Counter of local declarations at function scope level 1 (inside the
    /// function body but not inside nested blocks).  Used to insert `do...end`
    /// scope blocks when nearing Luau's 200 local register limit.
    scope_local_count: u32,
    /// The indent level at which the function body sits (normally 1).
    /// Used to determine when we're at the top scope for local counting.
    func_body_indent: u32,
    /// True when we've opened a `do` block for local spilling.
    in_spill_do_block: bool,
    /// True when the current function needs local spilling (>190 ops with output).
    needs_local_spill: bool,
}

impl Default for LuauBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LuauBackend {
    pub fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            indent: 0,
            uses_forward_decls: false,
            hoisted_vars: BTreeSet::new(),
            tuple_vars: BTreeSet::new(),
            scalar_plan: ScalarRepresentationPlan::default(),
            try_depth_counter: Vec::new(),
            pcall_counter: 0,
            inside_pcall_body: false,
            nonneg_consts: BTreeSet::new(),
            scope_local_count: 0,
            func_body_indent: 1,
            in_spill_do_block: false,
            needs_local_spill: false,
        }
    }
}

/// Map a Python/Molt type hint string to a Luau type annotation.
///
/// Returns a `&'static str` for the common primitive cases and falls back
/// to `"any"` for anything the Luau type system cannot express directly.
fn python_type_to_luau(hint: &str) -> &'static str {
    match hint {
        "int" | "Int" => "number",
        "float" | "Float" => "number",
        "str" | "Str" | "string" => "string",
        "bool" | "Bool" | "boolean" => "boolean",
        "None" | "NoneType" => "nil",
        "list" | "List" => "{any}",
        "dict" | "Dict" => "{[any]: any}",
        s if s.starts_with("list[") || s.starts_with("List[") => "{any}",
        s if s.starts_with("dict[") || s.starts_with("Dict[") => "{[any]: any}",
        _ => "any",
    }
}

/// Sanitize a Molt IR identifier for Luau.
/// Replaces `.` and `-` with `_`, and prefixes Luau keywords with `_m_`.
fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        return "_empty".to_string();
    }
    if is_luau_keyword(&cleaned) {
        format!("_m_{cleaned}")
    } else if cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_luau_keyword(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
            | "continue"
            | "type"
            | "export"
    )
}

fn escape_luau_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}
#[cfg(test)]
mod tests;
