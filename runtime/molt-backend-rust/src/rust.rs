//! Rust source-code transpiler backend for Molt.
//!
//! Transpiles `SimpleIR` → idiomatic-ish Rust source code.
//! Each Python module becomes a `.rs` file with:
//!   - A `MoltValue` enum (Python's dynamic type system in Rust)
//!   - Conditional runtime helpers (only the ones referenced)
//!   - One `fn` per Python function
//!   - `fn molt_main()` for module-level code
//!   - `fn main() { molt_main(); }`
//!
//! # Design
//! Variables are universally `MoltValue` and cloned on every use. This is
//! correct-first — type specialization and borrow elision are future passes.
//! Phi nodes are hoisted to function-top `let mut` declarations, same
//! strategy as the Luau backend.

use crate::representation_plan::ScalarRepresentationPlan;
use crate::{FunctionIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

mod emit_helpers;
mod lowering;
mod op_emitter;
mod prelude;
mod runtime_surface;

use lowering::{
    build_phi_injection_maps, collect_phi_assignments, collect_scope_escapes, lower_early_returns,
    lower_iter_to_for, strip_dead_after_return,
};

#[derive(Clone)]
enum AliasBinding {
    Value(String),
    Indexed { obj: String, key: String },
}

#[derive(Clone)]
struct JumpReturnCandidate {
    expr: String,
    min_scope_depth: i32,
}

/// Transpiles Molt `SimpleIR` into Rust source text.
pub struct RustBackend {
    output: String,
    indent: usize,
    hoisted_vars: BTreeSet<String>,
    /// When true, emit `use molt_rs::*;` instead of the inline MoltValue prelude.
    /// The caller is responsible for adding `molt-rs` to `Cargo.toml`.
    use_crate: bool,
    /// Tracks phi var → (frame_var, slot_var) from store_index ops inside loops.
    /// Used to emit a writeback when loop_index_next updates the phi var,
    /// so the locals frame stays coherent after the loop exits.
    phi_to_frame: BTreeMap<String, (String, String)>,
    /// Best-effort alias graph from temporaries to their source bindings.
    /// Used to propagate side-effecting mutations on cloned temps back to roots.
    aliases: BTreeMap<String, AliasBinding>,
    /// Current function params (as Rust identifiers) for call-by-object writeback.
    current_params: Vec<String>,
    current_is_main: bool,
    current_scalar_plan: Option<ScalarRepresentationPlan>,
    /// Authoritative fail-closed record of op kinds the dispatch could not
    /// lower. Populated by `emit_unsupported_op` at the moment the catch-all
    /// fires, so the fail-closed check in `compile_checked` does NOT depend on
    /// text-scanning the emitted output for a marker comment (a caller emitting
    /// a value without the exact marker string, or a stray placeholder, would
    /// otherwise slip a nil/`MoltValue::None` past the gate — the silent
    /// wrong-codegen class this field closes).
    unsupported_ops: Vec<String>,
}

impl Default for RustBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RustBackend {
    pub fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            indent: 0,
            hoisted_vars: BTreeSet::new(),
            use_crate: false,
            phi_to_frame: BTreeMap::new(),
            aliases: BTreeMap::new(),
            current_params: Vec::new(),
            current_is_main: false,
            current_scalar_plan: None,
            unsupported_ops: Vec::new(),
        }
    }

    /// Build a backend that emits `use molt_rs::*;` instead of the inline prelude.
    pub fn new_with_crate() -> Self {
        Self {
            use_crate: true,
            ..Self::new()
        }
    }

    /// Compile the given IR to a Rust source string.
    pub fn compile(&mut self, ir: &SimpleIR) -> String {
        // Reset the fail-closed accumulator for this compilation so a reused
        // backend instance does not carry unsupported-op records across runs.
        self.unsupported_ops.clear();
        // Phase 1: emit all function bodies into a temporary buffer so we
        // can scan which runtime helpers are actually referenced.
        let mut func_body = String::with_capacity(16384);
        std::mem::swap(&mut self.output, &mut func_body);

        for func in &ir.functions {
            self.emit_function(func);
            self.output.push('\n');
        }

        // Entry point
        self.emit_line("fn main() {");
        self.push_indent();
        self.emit_line("molt_main();");
        self.pop_indent();
        self.emit_line("}");

        let bodies = std::mem::take(&mut self.output);
        self.output = func_body;

        // Phase 2: emit file header + conditional prelude (or crate import).
        self.emit_header();
        if self.use_crate {
            self.output.push_str("use molt_rs::*;\n\n");
        } else {
            self.emit_prelude_conditional(&bodies);
        }

        // Phase 3: combine prelude + function bodies.
        self.output.push_str(&bodies);

        std::mem::take(&mut self.output)
    }

    /// Compile and reject any op the dispatch could not lower.
    ///
    /// Fail-closed authority: the decision is driven by
    /// `self.unsupported_ops`, recorded by `emit_unsupported_op` at the moment
    /// the dispatch catch-all fires — NOT by text-scanning the emitted source
    /// for a marker comment. An unsupported op therefore cannot slip a
    /// fabricated `MoltValue::None` past this gate by lacking the exact marker
    /// string. The output text scan (`rust_stub_markers`) is retained as a
    /// belt-and-suspenders check for legacy inline stub markers that predate
    /// the accumulator, but the accumulator is the primary authority.
    pub fn compile_checked(&mut self, ir: &SimpleIR) -> Result<String, String> {
        let source = self.compile(ir);
        if !self.unsupported_ops.is_empty() {
            return Err(format!(
                "rust backend refuses to emit fail-open codegen for unsupported op(s): {} \
                 -- use --target luau or native, or add lowering to the rust op dispatch",
                self.unsupported_ops.join(", ")
            ));
        }
        let stubs = rust_stub_markers(&source);
        if stubs.is_empty() {
            Ok(source)
        } else {
            Err(format!(
                "output contains unimplemented op stubs: {} -- use --target luau or native",
                stubs.join(", ")
            ))
        }
    }

    fn clear_alias(&mut self, var: &str) {
        let mut stack = vec![var.to_string()];
        while let Some(target) = stack.pop() {
            self.aliases.remove(&target);
            let children: Vec<String> = self
                .aliases
                .iter()
                .filter_map(|(k, binding)| match binding {
                    AliasBinding::Value(parent) if parent == &target => Some(k.clone()),
                    AliasBinding::Indexed { obj, .. } if obj == &target => Some(k.clone()),
                    _ => None,
                })
                .collect();
            for child in children {
                self.aliases.remove(&child);
                stack.push(child);
            }
        }
    }

    fn note_alias(&mut self, dst: String, src: String) {
        self.clear_alias(&dst);
        // Record the DIRECT parent (not the root) so emit_alias_writeback
        // propagates mutations through each intermediate phi var correctly.
        // e.g. v265→v130→v146 ensures both v130 and v146 get updated.
        if dst != src {
            self.aliases.insert(dst, AliasBinding::Value(src));
        }
    }

    fn note_indexed_alias(&mut self, dst: String, obj: String, key: String) {
        self.clear_alias(&dst);
        self.aliases.insert(dst, AliasBinding::Indexed { obj, key });
    }

    fn emit_alias_writeback(&mut self, var: &str) {
        let mut cur = var.to_string();
        let mut seen = BTreeSet::new();
        while let Some(binding) = self.aliases.get(&cur).cloned() {
            let next = match binding {
                AliasBinding::Value(parent) => {
                    self.emit_line(&format!("{parent} = {cur}.clone();"));
                    parent
                }
                AliasBinding::Indexed { obj, key } => {
                    self.emit_line(&format!(
                        "molt_set_item(&mut {obj}, {key}.clone(), {cur}.clone());"
                    ));
                    obj
                }
            };
            if !seen.insert(next.clone()) {
                break;
            }
            cur = next;
        }
    }

    fn emit_param_writeback(&mut self) {
        if self.current_is_main || self.current_params.is_empty() {
            return;
        }
        let params = self.current_params.clone();
        for (i, param) in params.iter().enumerate() {
            self.emit_line(&format!(
                "if args___.len() <= {i} {{ args___.resize({len}, MoltValue::None); }}",
                len = i + 1
            ));
            self.emit_line(&format!("args___[{i}] = {param}.clone();"));
        }
    }

    // Function emission

    fn emit_function(&mut self, func: &FunctionIR) {
        let is_main = func.name == "molt_main"
            || func.name == "__main__"
            || func.name == "molt___main__"
            || (func.params.is_empty() && func.name.starts_with("molt_main"));
        self.current_is_main = is_main;
        self.current_params = if is_main {
            Vec::new()
        } else {
            func.params.iter().map(|p| rust_ident(p)).collect()
        };
        self.aliases.clear();

        let name = rust_ident(&func.name);

        // Pre-lower ops
        let ops = lower_early_returns(&func.ops);
        let ops = strip_dead_after_return(&ops);
        let ops = lower_iter_to_for(&ops);
        let plan_func = FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops: ops.clone(),
            param_types: func.param_types.clone(),
            source_file: func.source_file.clone(),
            is_extern: func.is_extern,
        };
        self.current_scalar_plan = Some(ScalarRepresentationPlan::for_function_ir(&plan_func));

        // Collect loop index vars (need pre-declaration so they persist across iterations)
        let loop_idx_vars: Vec<String> = ops
            .iter()
            .filter(|op| op.kind == "loop_index_start")
            .filter_map(|op| op.out.as_deref())
            .map(rust_ident)
            .collect();

        let named_storage_vars: Vec<String> = {
            let mut seen = Vec::new();
            for op in &ops {
                if op.kind == "store_var"
                    && let Some(name) = op.var.as_deref().or(op.out.as_deref())
                {
                    let storage = rust_ident(name);
                    if !self.current_params.contains(&storage) && !seen.contains(&storage) {
                        seen.push(storage);
                    }
                }
            }
            seen
        };

        // Collect closure slot vars
        let closure_slots: Vec<String> = {
            let mut seen = Vec::new();
            for op in &ops {
                if (op.kind == "closure_store" || op.kind == "closure_load")
                    && let Some(slot) = op.args.as_ref().and_then(|a| a.first())
                {
                    let v = format!("__closure_{}", rust_ident(slot));
                    if !seen.contains(&v) {
                        seen.push(v);
                    }
                }
            }
            seen
        };

        // Phi hoisting — same algorithm as Luau backend
        self.hoisted_vars.clear();
        self.phi_to_frame.clear();
        let phi_assignments = collect_phi_assignments(&ops, &mut self.hoisted_vars);
        let (phi_inject_before_else, phi_inject_before_end_if) =
            build_phi_injection_maps(&ops, &phi_assignments);

        // Scope-escape hoisting
        collect_scope_escapes(&ops, func, &mut self.hoisted_vars);

        let mut stable_return_vars: BTreeSet<String> =
            self.current_params.iter().cloned().collect();
        stable_return_vars.extend(loop_idx_vars.iter().cloned());
        stable_return_vars.extend(closure_slots.iter().cloned());
        stable_return_vars.extend(named_storage_vars.iter().cloned());
        stable_return_vars.extend(self.hoisted_vars.iter().cloned());

        if is_main {
            self.emit_line("fn molt_main() {");
        } else {
            let _ = writeln!(
                self.output,
                "fn {name}(args___: &mut Vec<MoltValue>) -> MoltValue {{"
            );
            self.indent += 1;
            // Destructure params from args
            for (i, p) in func.params.iter().enumerate() {
                let pname = rust_ident(p);
                self.emit_line(&format!("let mut {pname}: MoltValue = args___.get({i}).cloned().unwrap_or(MoltValue::None);"));
            }
        }
        self.indent += 1;

        // Emit pre-declarations for hoisted vars
        for v in &loop_idx_vars {
            self.emit_line(&format!("let mut {v}: MoltValue = MoltValue::None;"));
        }
        for v in &closure_slots {
            self.emit_line(&format!("let mut {v}: MoltValue = MoltValue::None;"));
        }
        for v in &named_storage_vars {
            self.emit_line(&format!("let mut {v}: MoltValue = MoltValue::None;"));
        }
        let mut sorted_hoisted: Vec<String> = self.hoisted_vars.iter().cloned().collect();
        sorted_hoisted.sort();
        for v in &sorted_hoisted {
            if !loop_idx_vars.contains(v) && !named_storage_vars.contains(v) {
                self.emit_line(&format!("let mut {v}: MoltValue = MoltValue::None;"));
            }
        }

        // Save function body start for hoisted-var post-processing
        let func_body_start = self.output.len();

        // Emit ops
        // Track the most recent store result for use by `jump`.
        // The `jump N` IR op is a forward goto used for early function returns:
        //   store result → var/frame[slot]; jump N; ... ; label N: load var/frame[slot]; ret
        // We emit `return <stored_expr>;` at the jump site so the early return value is
        // correctly returned to the caller.
        //
        // Two patterns (tree_shake_luau decides which):
        //   - store_local(var, val): after optimization, `var` holds the return value
        //   - store_index(frame, slot, val): unoptimized, must molt_get_item to recover
        let mut last_jump_return: Option<JumpReturnCandidate> = None; // the Rust expr to return at `jump`
        let mut scope_depth: i32 = 0;
        let mut i = 0;
        while i < ops.len() {
            if let Some(injects) = phi_inject_before_else.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}.clone();"));
                }
            }
            if let Some(injects) = phi_inject_before_end_if.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}.clone();"));
                }
            }

            // Track last store for jump early-return inference.
            match ops[i].kind.as_str() {
                "store_local" | "store" | "store_init" => {
                    // store_local(var, val) → var holds the return value directly
                    if let Some(ref v) = ops[i].var {
                        let dst = rust_ident(v);
                        let min_scope_depth = if stable_return_vars.contains(&dst) {
                            0
                        } else {
                            scope_depth
                        };
                        last_jump_return = Some(JumpReturnCandidate {
                            expr: format!("{dst}.clone()"),
                            min_scope_depth,
                        });
                    }
                }
                "store_index" | "set_item" | "store_subscript" => {
                    // store_index(frame, slot, val) returns the stored source value.
                    // Tracking frame/slot references directly leaks block-scoped
                    // temps when the eventual jump is emitted after the scope closes.
                    if let Some(args) = ops[i].args.as_deref()
                        && args.len() >= 3
                    {
                        let src = rust_ident(&args[2]);
                        let min_scope_depth = if stable_return_vars.contains(&src) {
                            0
                        } else {
                            scope_depth
                        };
                        last_jump_return = Some(JumpReturnCandidate {
                            expr: format!("{src}.clone()"),
                            min_scope_depth,
                        });
                    }
                }
                _ => {}
            }

            // Intercept `jump N`: emit early return via last stored value.
            // This covers: store → jump → (skipped code) → label → load → ret
            if ops[i].kind == "jump" {
                if self.current_is_main {
                    self.emit_param_writeback();
                    self.emit_line("return;");
                } else if let Some(candidate) = last_jump_return.clone() {
                    self.emit_param_writeback();
                    self.emit_line(&format!("return {};", candidate.expr));
                } else {
                    self.emit_param_writeback();
                    self.emit_line("return MoltValue::None; /* jump: no prior store */");
                }
                i += 1;
                continue;
            }

            // `label N` is the jump target — it's a no-op in structured Rust code.
            if ops[i].kind == "label" {
                i += 1;
                continue;
            }

            let processed_kind = if ops[i].kind == "loop_start"
                && i + 1 < ops.len()
                && ops[i + 1].kind == "loop_index_start"
            {
                let idx_op = &ops[i + 1];
                if let Some(ref out_name) = idx_op.out {
                    let out = rust_ident(out_name);
                    let args = idx_op.args.as_deref().unwrap_or(&[]);
                    let start = args
                        .first()
                        .map(|s| rust_ident(s))
                        .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                    self.emit_line(&format!("{out} = {start}.clone();"));
                }
                self.emit_op(&ops[i]);
                i += 2;
                "loop_start"
            } else {
                let kind = ops[i].kind.as_str();
                self.emit_op(&ops[i]);
                i += 1;
                kind
            };

            match processed_kind {
                "if" | "if_not" | "loop_start" | "while_start" | "for_range" | "for_iter" => {
                    scope_depth += 1;
                }
                "else" => {
                    if last_jump_return
                        .as_ref()
                        .is_some_and(|candidate| candidate.min_scope_depth >= scope_depth)
                    {
                        last_jump_return = None;
                    }
                }
                "end_if" | "loop_end" | "while_end" | "end_for" => {
                    scope_depth = (scope_depth - 1).max(0);
                    if last_jump_return
                        .as_ref()
                        .is_some_and(|candidate| candidate.min_scope_depth > scope_depth)
                    {
                        last_jump_return = None;
                    }
                }
                _ => {}
            }
        }

        let needs_implicit_none = ops
            .iter()
            .rev()
            .find(|op| {
                !matches!(
                    op.kind.as_str(),
                    "nop" | "comment" | "debug_label" | "line" | "check_exception" | "label"
                )
            })
            .is_none_or(|op| {
                !matches!(
                    op.kind.as_str(),
                    "ret"
                        | "return"
                        | "return_value"
                        | "return_none"
                        | "ret_none"
                        | "ret_void"
                        | "jump"
                        | "raise"
                        | "reraise"
                )
            });

        self.indent -= 1;
        if is_main {
            // main doesn't have an explicit return
        } else if needs_implicit_none {
            self.emit_param_writeback();
            self.emit_line("MoltValue::None");
        }
        self.emit_line("}");

        // Post-process: replace `let mut hoisted_var: MoltValue = ...` → `hoisted_var = ...`
        if !self.hoisted_vars.is_empty() {
            let func_output = &self.output[func_body_start..];
            let mut patched = String::with_capacity(func_output.len());
            for line in func_output.lines() {
                let trimmed = line.trim_start();
                let mut replaced = false;
                // Match pattern: "let mut VAR: MoltValue = ..." where VAR is hoisted
                if let Some(after) = trimmed.strip_prefix("let mut ") {
                    // skip "let mut "
                    let var_end = after
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after.len());
                    let var_name = &after[..var_end];
                    if self.hoisted_vars.contains(var_name) {
                        let rest = after[var_end..].trim_start();
                        // Skip pre-declaration lines (": MoltValue;" with no "=")
                        if rest.starts_with(": MoltValue =") || rest.starts_with("=") {
                            let indent_str = &line[..line.len() - trimmed.len()];
                            // Strip "let mut " and ": MoltValue" type annotation if present
                            let assign_part =
                                if let Some(stripped) = rest.strip_prefix(": MoltValue =") {
                                    format!("{var_name} ={stripped}")
                                } else {
                                    format!("{var_name} {rest}")
                                };
                            patched.push_str(indent_str);
                            patched.push_str(&assign_part);
                            patched.push('\n');
                            replaced = true;
                        }
                    }
                }
                if !replaced {
                    patched.push_str(line);
                    patched.push('\n');
                }
            }
            self.output.truncate(func_body_start);
            self.output.push_str(&patched);
        }

        self.hoisted_vars.clear();
        self.phi_to_frame.clear();
        self.aliases.clear();
        self.current_params.clear();
        self.current_is_main = false;
        self.current_scalar_plan = None;
    }

    // ── Emit helpers ──────────────────────────────────────────────────────────

    fn emit_line(&mut self, line: &str) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
        self.output.push_str(line);
        self.output.push('\n');
    }

    fn push_indent(&mut self) {
        self.indent += 1;
    }
    fn pop_indent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }
}

// ── Identifier / string helpers ───────────────────────────────────────────────

/// Sanitize a Molt IR identifier to a valid Rust identifier.
pub(crate) fn rust_ident(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        return "_".to_string();
    }
    // Replace characters that are valid in Python but not Rust
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Ensure it doesn't start with a digit
    let s = if s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("v_{s}")
    } else {
        s
    };
    // Avoid Rust keywords
    match s.as_str() {
        "type" | "match" | "move" | "ref" | "use" | "mod" | "pub" | "fn" | "let" | "mut"
        | "impl" | "trait" | "struct" | "enum" | "where" | "super" | "self" | "crate"
        | "extern" | "as" | "in" | "for" | "loop" | "while" | "if" | "else" | "return"
        | "break" | "continue" | "box" | "unsafe" | "static" | "const" | "dyn" | "async"
        | "await" => {
            format!("{s}_")
        }
        _ => s,
    }
}

fn rust_stub_markers(source: &str) -> Vec<String> {
    let mut markers = BTreeSet::new();
    for line in source.lines() {
        let mut tail = line;
        while let Some(start) = tail.find("/* MOLT_STUB:") {
            let marker_start = start + "/* ".len();
            let after_marker = &tail[marker_start..];
            let marker_end = after_marker
                .find(" */")
                .or_else(|| after_marker.find("*/"))
                .unwrap_or(after_marker.len());
            markers.insert(after_marker[..marker_end].trim().to_string());
            tail = &after_marker[marker_end..];
        }
    }
    markers.into_iter().take(8).collect()
}

#[cfg(test)]
mod tests;
