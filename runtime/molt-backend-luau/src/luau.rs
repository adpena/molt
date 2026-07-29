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
use crate::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR};
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
mod dict_runtime;
mod frame_runtime;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdentityProvenance {
    Singleton(ScalarKind),
    ValueScalar(ScalarKind),
    Reference,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdentityLowering {
    Constant(bool),
    Direct,
    Reject,
}

fn identity_provenance(plan: &ScalarRepresentationPlan, name: &str) -> IdentityProvenance {
    match plan.name_scalar_kind(name) {
        Some(kind @ (ScalarKind::Bool | ScalarKind::NoneValue)) => {
            IdentityProvenance::Singleton(kind)
        }
        Some(kind @ (ScalarKind::Int | ScalarKind::Float | ScalarKind::Str)) => {
            IdentityProvenance::ValueScalar(kind)
        }
        None if plan.name_container_kind(name).is_some() || plan.name_is_known_non_scalar(name) => {
            IdentityProvenance::Reference
        }
        None => IdentityProvenance::Unknown,
    }
}

/// Canonical Luau identity admission and lowering decision. Luau compares
/// primitive numbers/strings by value and erases int/float provenance, so only
/// explicit aliasing, singleton values, reference carriers, or statically
/// disjoint provenance classes can implement Python identity exactly.
fn identity_lowering(plan: &ScalarRepresentationPlan, lhs: &str, rhs: &str) -> IdentityLowering {
    identity_lowering_for_provenance(
        lhs == rhs,
        identity_provenance(plan, lhs),
        identity_provenance(plan, rhs),
    )
}

/// Provenance-only identity decision shared by validation and emission. Keeping
/// the SSA-alias override explicit makes this table directly comparable with
/// the formal identity-admission model.
fn identity_lowering_for_provenance(
    same_ssa: bool,
    lhs: IdentityProvenance,
    rhs: IdentityProvenance,
) -> IdentityLowering {
    if same_ssa {
        return IdentityLowering::Constant(true);
    }
    use IdentityProvenance::{Reference, Singleton, Unknown, ValueScalar};
    match (lhs, rhs) {
        (ValueScalar(lhs_kind), ValueScalar(rhs_kind)) if lhs_kind == rhs_kind => {
            IdentityLowering::Reject
        }
        (ValueScalar(_), Unknown) | (Unknown, ValueScalar(_)) | (Unknown, Unknown) => {
            IdentityLowering::Reject
        }
        (Singleton(lhs_kind), Singleton(rhs_kind)) if lhs_kind == rhs_kind => {
            IdentityLowering::Direct
        }
        (Reference, Reference)
        | (Reference, Unknown)
        | (Unknown, Reference)
        | (Singleton(_), Unknown)
        | (Unknown, Singleton(_)) => IdentityLowering::Direct,
        _ => IdentityLowering::Constant(false),
    }
}

/// Transpiles Molt `SimpleIR` into Luau source text.
pub struct LuauBackend {
    output: String,
    /// Current indentation level (number of tabs).
    indent: usize,
    uses_forward_decls: bool,
    /// Raw IR function symbols in the active module. This distinguishes
    /// user-defined names in the reserved `molt_` namespace from compiler
    /// runtime references carried by call op metadata.
    function_symbols: BTreeSet<String>,
    /// Functions without their own TRACE_ENTER_SLOT that nevertheless consume
    /// execution-frame ops. Their caller threads the active context through a
    /// backend-private trailing parameter (module chunks are the canonical case).
    inherited_frame_context_functions: BTreeSet<String>,
    /// Variables that have been pre-declared at function scope and should use
    /// assignment (`var = val`) instead of `local var = val` in emit_op.
    hoisted_vars: BTreeSet<String>,
    /// Variables whose runtime carrier is a Python sequence table. This is a
    /// storage-representation fact only; it never changes function return arity.
    tuple_vars: BTreeSet<String>,
    /// Backend-neutral scalar representation facts for the function currently
    /// being emitted.
    scalar_plan: ScalarRepresentationPlan,
    /// Stack of pcall counter values for nested try/except blocks.
    try_depth_counter: Vec<u32>,
    /// Monotonically increasing counter for generating unique pcall variable names.
    pcall_counter: u32,
    /// Monotonically increasing counter for backend-owned temporary locals.
    temp_counter: u32,
    /// True when we are inside a pcall body (between pcall_wrap_begin and
    /// pcall_wrap_end). exception_last should return nil in this zone.
    inside_pcall_body: bool,
    /// Whether the active function owns a TRACE_ENTER_SLOT activation local.
    /// Compiler-generated module chunks inherit the caller's execution context.
    has_local_frame_context: bool,
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
    /// Dispatch failures accumulated during private source assembly. The only
    /// production compile entrypoint returns `Result` and rejects this set;
    /// unsupported operations never emit a substitute value.
    unsupported_ops: Vec<String>,
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
            function_symbols: BTreeSet::new(),
            inherited_frame_context_functions: BTreeSet::new(),
            hoisted_vars: BTreeSet::new(),
            tuple_vars: BTreeSet::new(),
            scalar_plan: ScalarRepresentationPlan::default(),
            try_depth_counter: Vec::new(),
            pcall_counter: 0,
            temp_counter: 0,
            inside_pcall_body: false,
            has_local_frame_context: false,
            nonneg_consts: BTreeSet::new(),
            scope_local_count: 0,
            func_body_indent: 1,
            in_spill_do_block: false,
            needs_local_spill: false,
            unsupported_ops: Vec::new(),
        }
    }
}

impl LuauBackend {
    fn frame_context_expr(&self) -> &'static str {
        assert!(
            self.has_local_frame_context,
            "validated Luau IR requested execution-frame state without a Local or Inherited context"
        );
        "__molt_frame_context"
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

const USER_SYMBOL_ESCAPE_PREFIX: &str = "_m_user_";
const LABEL_SYMBOL_ESCAPE_PREFIX: &str = "_m_label_";

#[derive(Clone, Copy)]
enum LuauSymbolDomain {
    UserValue,
    StringLabel,
}

fn encode_symbol(name: &str, domain: LuauSymbolDomain) -> String {
    let prefix = match domain {
        LuauSymbolDomain::UserValue => USER_SYMBOL_ESCAPE_PREFIX,
        LuauSymbolDomain::StringLabel => LABEL_SYMBOL_ESCAPE_PREFIX,
    };
    let mut encoded = String::with_capacity(prefix.len() + name.len() * 2);
    encoded.push_str(prefix);
    for byte in name.as_bytes() {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

/// Canonical, injective Molt IR symbol mapping for Luau. Ordinary valid names
/// stay readable and allocation-minimal. Invalid spellings, Luau keywords, and
/// compiler-reserved namespaces are encoded from their complete UTF-8 bytes,
/// so punctuation and namespace collisions can never alias another IR symbol.
fn sanitize_ident(name: &str) -> String {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic());
    let valid_tail = chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if valid_start
        && valid_tail
        && !is_luau_keyword(name)
        && !name.starts_with("molt_")
        && !name.starts_with("__")
        && !name.starts_with("_m_")
    {
        return name.to_string();
    }
    encode_symbol(name, LuauSymbolDomain::UserValue)
}

fn sanitize_string_label(label: &str) -> String {
    encode_symbol(label, LuauSymbolDomain::StringLabel)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LuauFunctionSymbol<'a> {
    CompilerEntrypoint,
    User(&'a str),
}

fn classify_function_symbol(name: &str) -> LuauFunctionSymbol<'_> {
    if name == "molt_main" {
        LuauFunctionSymbol::CompilerEntrypoint
    } else {
        LuauFunctionSymbol::User(name)
    }
}

fn emit_function_ident(name: &str) -> String {
    match classify_function_symbol(name) {
        LuauFunctionSymbol::CompilerEntrypoint => "molt_main".to_string(),
        LuauFunctionSymbol::User(user_name) => sanitize_ident(user_name),
    }
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
