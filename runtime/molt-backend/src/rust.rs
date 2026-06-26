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
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

mod prelude;

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

    /// Compile and reject any preview-blocker stubs in the output.
    pub fn compile_checked(&mut self, ir: &SimpleIR) -> Result<String, String> {
        let source = self.compile(ir);
        let stubs = rust_stub_markers(&source);
        if stubs.is_empty() {
            Ok(source)
        } else {
            Err(format!(
                "output contains unimplemented op stubs: {} — use --target luau or native",
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

    // ── Op emission ───────────────────────────────────────────────────────────

    fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
        self.current_scalar_plan
            .as_ref()
            .is_some_and(|plan| plan.op_prefers_integer_runtime_lane(op))
    }

    fn emit_op(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let _is_hoisted = |name: &str| self.hoisted_vars.contains(name);
        if let Some(name) = op.out.as_deref() {
            let out_name = rust_ident(name);
            self.clear_alias(&out_name);
        }

        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        match op.kind.as_str() {
            // ── Constants ──────────────────────────────────────────────────────
            "const" | "int_const" => {
                let o = out();
                let rhs = if let Some(v) = op.value {
                    format!("MoltValue::Int({v})")
                } else if let Some(f) = op.f_value {
                    format!("MoltValue::Float({f:.17})")
                } else if let Some(ref s) = op.s_value {
                    format!("MoltValue::Str({}.to_string())", rust_string_literal(s))
                } else {
                    "MoltValue::None".to_string()
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_float" => {
                let o = out();
                let f = op.f_value.unwrap_or(0.0);
                let rhs = format!("MoltValue::Float({f:.17})");
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_str" | "string_const" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("");
                let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_bool" | "bool_const" => {
                let o = out();
                let b = op.value.unwrap_or(0) != 0;
                let rhs = format!("MoltValue::Bool({b})");
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_none" | "none_const" => {
                let o = out();
                self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
            }
            "const_bytes" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("");
                let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_bigint" => {
                let o = out();
                let s = op.s_value.as_deref().unwrap_or("0");
                let rhs = format!(
                    "MoltValue::Int({}.parse::<i64>().unwrap_or(0))",
                    rust_string_literal(s)
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "const_not_implemented" | "const_ellipsis" => {
                let o = out();
                self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                // no comment needed — #![allow(unused)] covers it
            }
            "box" | "box_from_raw_int" => {
                let o = out();
                let rhs = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|src| rust_clone(src))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── Variable access ────────────────────────────────────────────────
            "load_local" => {
                let o = out();
                let v = var_ref(op);
                self.emit_line(&declare(
                    &o,
                    &format!("{v}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, v);
            }
            "load_var" | "copy_var" => {
                let o = out();
                let v = var_ref(op);
                self.emit_line(&declare(
                    &o,
                    &format!("{v}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, v);
            }
            "store_var" => {
                if let Some(name) = op.var.as_deref().or(op.out.as_deref()) {
                    let dst = rust_ident(name);
                    self.clear_alias(&dst);
                    let rhs = op
                        .args
                        .as_deref()
                        .and_then(|args| args.first())
                        .map(|src| rust_clone(src))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&format!("{dst} = {rhs};"));
                }
            }
            "load" | "guarded_load" => {
                let o = out();
                if let Some(obj) = op.args.as_ref().and_then(|a| a.first()) {
                    let obj = rust_value(obj);
                    let slot_key = rust_slot_key(op.value.unwrap_or(0));
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_item(&{obj}, &{slot_key})"),
                        &self.hoisted_vars.clone(),
                    ));
                    let alias_key = format!("__alias_key_{o}");
                    self.emit_line(&declare(
                        &alias_key,
                        &format!("{slot_key}.clone()"),
                        &self.hoisted_vars.clone(),
                    ));
                    self.note_indexed_alias(o, obj, alias_key);
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "closure_load" => {
                let o = out();
                let slot = op
                    .args
                    .as_ref()
                    .and_then(|a| a.first())
                    .map(|s| format!("__closure_{}", rust_ident(s)))
                    .unwrap_or_else(|| var_ref(op));
                self.emit_line(&declare(
                    &o,
                    &format!("{slot}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_alias(o, slot);
            }
            "store_local" => {
                let v = var_ref(op);
                if let Some(src) = op.args.as_ref().and_then(|a| a.first()) {
                    let s = rust_ident(src);
                    self.emit_line(&format!("{v} = {s}.clone();"));
                    self.note_alias(v, s);
                } else {
                    self.clear_alias(&v);
                }
            }
            "store" | "store_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let value = rust_clone(&args[1]);
                    let slot_key = rust_slot_key(op.value.unwrap_or(0));
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!("molt_set_item(&mut {obj}, {slot_key}, {value});"));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }
            "closure_store" => {
                if let Some(args) = &op.args
                    && args.len() >= 2
                {
                    let slot = format!("__closure_{}", rust_ident(&args[0]));
                    let src = rust_ident(&args[1]);
                    self.emit_line(&format!("{slot} = {src}.clone();"));
                }
            }
            "phi" => {
                // Phi nodes are handled by the hoisting logic above; skip here.
            }

            // ── Arithmetic ─────────────────────────────────────────────────────
            "add" | "inplace_add" | "binop_add" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_add(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_add({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "sub" | "inplace_sub" | "binop_sub" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_sub(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_sub({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "mul" | "inplace_mul" | "binop_mul" => {
                let o = out();
                let (a, b) = args2(op);
                if self.op_prefers_integer_runtime_lane(op) {
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Int(molt_int(&{a}).wrapping_mul(molt_int(&{b})))"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_mul({a}.clone(), {b}.clone())"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "div" | "true_div" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_div({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "floor_div" | "floordiv" | "binop_floor_div" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_floor_div({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "mod" | "modulo" | "binop_mod" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_mod({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "pow" | "binop_pow" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_pow({a}.clone(), {b}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "neg" | "unary_neg" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_neg({a}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "unary_not" | "not" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(!molt_bool(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // Bitwise
            "band" | "bit_and" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) & molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bor" | "bit_or" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) | molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bxor" | "bit_xor" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) ^ molt_int(&{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "lshift" | "shl" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) << (molt_int(&{b}) as u32))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "rshift" | "shr" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}) >> (molt_int(&{b}) as u32))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Comparisons ────────────────────────────────────────────────────
            "eq" | "cmp_eq" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ne" | "cmp_ne" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(!molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "lt" | "cmp_lt" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_lt(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "le" | "cmp_le" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_le(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "gt" | "cmp_gt" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_gt(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ge" | "cmp_ge" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_ge(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "is" | "is_not" => {
                // Python `is` — identity check (use == for value equality in Rust)
                let o = out();
                let (a, b) = args2(op);
                let negate = op.kind == "is_not";
                let cmp = if negate { "!" } else { "" };
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool({cmp}molt_eq(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "in" | "not_in" => {
                let o = out();
                let (a, b) = args2(op);
                let negate = op.kind == "not_in";
                let prefix = if negate { "!" } else { "" };
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool({prefix}molt_in(&{a}, &{b}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "contains" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = rust_ident(&args[0]);
                    let value = rust_ident(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("MoltValue::Bool(molt_in(&{value}, &{container}))"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }

            // ── Boolean logic ──────────────────────────────────────────────────
            "and" | "_m_and" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("if !molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "or" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("if molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Control flow ───────────────────────────────────────────────────
            "if" | "branch_false" => {
                let cond = arg0(op);
                self.emit_line(&format!("if molt_bool(&{cond}) {{"));
                self.indent += 1;
            }
            "if_not" | "branch_true" => {
                let cond = arg0(op);
                self.emit_line(&format!("if !molt_bool(&{cond}) {{"));
                self.indent += 1;
            }
            "else" => {
                self.indent -= 1;
                self.emit_line("} else {");
                self.indent += 1;
            }
            "end_if" => {
                self.indent -= 1;
                self.emit_line("}");
            }
            "loop_start" | "while_start" => {
                self.emit_line("loop {");
                self.indent += 1;
            }
            "loop_end" | "while_end" => {
                self.indent -= 1;
                self.emit_line("}");
            }
            "loop_break_if_false" => {
                let cond = arg0(op);
                self.emit_line(&format!("if !molt_bool(&{cond}) {{ break; }}"));
            }
            "loop_break_if_true" => {
                let cond = arg0(op);
                self.emit_line(&format!("if molt_bool(&{cond}) {{ break; }}"));
            }
            "loop_break_if_exception" => {
                // Value-less exception-flag break: exit an iterator-consumer loop
                // when a runtime exception is pending (the producer returned the
                // None sentinel on a mid-iteration raise).  Reads the same
                // sacrosanct flag the runtime CHECK_EXCEPTION uses; the still
                // pending exception then rides up the lazy-return path.
                self.emit_line("if molt_exception_pending() != 0 { break; }");
            }
            "loop_break" => {
                self.emit_line("break;");
            }
            "loop_continue" | "loop_carry_update" | "loop_carry_init" => {
                self.emit_line("continue;");
            }
            "loop_index_next" => {
                // Update loop index — 1-arg: assign; 2-arg: add-step.
                // After updating the phi var, also write back to the locals frame slot
                // (if any) so that post-loop index reads see the correct value.
                if let Some(ref out_name) = op.out {
                    let o = rust_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let new_val_expr = if args.len() >= 2 {
                        let current = rust_ident(&args[0]);
                        let step = rust_ident(&args[1]);
                        format!("molt_add({current}.clone(), {step}.clone())")
                    } else if let Some(new_val) = args.first() {
                        format!("{}.clone()", rust_ident(new_val))
                    } else {
                        String::new()
                    };
                    if !new_val_expr.is_empty() {
                        self.emit_line(&format!("{o} = {new_val_expr};"));
                        // Write the updated phi value back to the locals frame so
                        // post-loop `index` ops read the final (not stale) value.
                        if let Some((frame, slot)) = self.phi_to_frame.get(&o).cloned() {
                            self.emit_line(&format!(
                                "molt_set_item(&mut {frame}, {slot}.clone(), {o}.clone());"
                            ));
                        }
                    }
                }
            }
            "loop_index_start" => {
                // Initialization is handled in the loop preamble above; skip here.
            }
            "iter" => {
                let o = out();
                let src = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_iter(&{src})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "iter_next" => {
                let o = out();
                let iter_var = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_iter_next(&mut {iter_var})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "for_range" => {
                // for_range: args = [out_var, start, stop, step]
                let args = op.args.as_deref().unwrap_or(&[]);
                let iter_var = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_".to_string());
                let start = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let stop = args
                    .get(2)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let step = args
                    .get(3)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
                // Emit as a while loop to keep MoltValue
                self.emit_line(&format!("{{ let mut __range_i = molt_int(&{start}); let __range_stop = molt_int(&{stop}); let __range_step = molt_int(&{step});"));
                self.emit_line("while (__range_step > 0 && __range_i < __range_stop) || (__range_step < 0 && __range_i > __range_stop) {");
                self.indent += 1;
                self.emit_line(&format!(
                    "let mut {iter_var}: MoltValue = MoltValue::Int(__range_i);"
                ));
            }
            "for_iter" => {
                // for_iter (comprehension-inlined): out = loop_var, args[0] = iterable.
                // The comprehension inliner in lib.rs always emits this convention.
                let iter_var = out();
                let iterable = arg0(op);
                self.emit_line(&format!("for {iter_var} in molt_iter_list(&{iterable}) {{"));
                self.indent += 1;
            }
            "range_new" => {
                // range_new(start, stop, step) — used by comprehension-inlined source_ops.
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let start = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let stop = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let step = args
                    .get(2)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "end_for" => {
                // Range loops open an extra block + while; make sure the index
                // advances before closing the while body.
                let closes_range = op.args.as_ref().is_some_and(|args| !args.is_empty());
                if closes_range {
                    self.emit_line("__range_i += __range_step;");
                }
                if self.indent > 0 {
                    self.indent -= 1;
                }
                self.emit_line("}");
                if closes_range {
                    if self.indent > 0 {
                        self.indent -= 1;
                    }
                    self.emit_line("}");
                }
            }
            "break" => {
                self.emit_line("break;");
            }
            "continue" => {
                self.emit_line("continue;");
            }

            // ── Return ─────────────────────────────────────────────────────────
            "return" | "ret" => {
                if self.current_is_main {
                    self.emit_param_writeback();
                    self.emit_line("return;");
                } else if let Some(val) = op.args.as_ref().and_then(|a| a.first()) {
                    let v = rust_ident(val);
                    self.emit_param_writeback();
                    self.emit_line(&format!("return {v}.clone();"));
                } else if let Some(ref v) = op.var {
                    let v = rust_ident(v);
                    self.emit_param_writeback();
                    self.emit_line(&format!("return {v}.clone();"));
                } else {
                    self.emit_param_writeback();
                    self.emit_line("return MoltValue::None;");
                }
            }
            "return_none" | "ret_none" | "ret_void" => {
                self.emit_param_writeback();
                if self.current_is_main {
                    self.emit_line("return;");
                } else {
                    self.emit_line("return MoltValue::None;");
                }
            }

            // ── Function calls ─────────────────────────────────────────────────
            "call" | "call_func" | "call_internal" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(ref fn_name) = op.s_value {
                    // Direct static call with mutable arg-vector writeback.
                    let fn_ident = rust_ident(fn_name);
                    let call_args: Vec<String> = args.iter().map(|a| rust_clone(a)).collect();
                    self.emit_line(&format!(
                        "let mut __call_args: Vec<MoltValue> = vec![{}];",
                        call_args.join(", ")
                    ));
                    self.emit_line(&format!(
                        "let mut __call_ret: MoltValue = {fn_ident}(&mut __call_args);"
                    ));
                    for (idx, arg) in args.iter().enumerate() {
                        let var = rust_ident(arg);
                        if is_assignable_var(&var) {
                            self.emit_line(&format!(
                                "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                            ));
                            self.emit_alias_writeback(&var);
                        }
                    }
                    if o == "_" || o == "none" {
                        self.emit_line("__call_ret;");
                    } else {
                        self.emit_line(&declare(
                            &o,
                            "__call_ret.clone()",
                            &self.hoisted_vars.clone(),
                        ));
                    }
                } else if args.is_empty() {
                    if o == "_" || o == "none" {
                        self.emit_line("MoltValue::None;");
                    } else {
                        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                    }
                } else {
                    // Dynamic call: args[0] is the MoltValue::Func to invoke.
                    let func_var = rust_ident(&args[0]);
                    let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
                    self.emit_line(&format!(
                        "let mut __call_args: Vec<MoltValue> = vec![{}];",
                        call_args.join(", ")
                    ));
                    self.emit_line(&format!(
                        "let mut __call_ret: MoltValue = molt_call(&{func_var}, &mut __call_args);"
                    ));
                    for (idx, arg) in args[1..].iter().enumerate() {
                        let var = rust_ident(arg);
                        if is_assignable_var(&var) {
                            self.emit_line(&format!(
                                "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                            ));
                            self.emit_alias_writeback(&var);
                        }
                    }
                    if o == "_" || o == "none" {
                        self.emit_line("__call_ret;");
                    } else {
                        self.emit_line(&declare(
                            &o,
                            "__call_ret.clone()",
                            &self.hoisted_vars.clone(),
                        ));
                    }
                }
            }
            "call_method" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                // args: [obj, arg0, arg1, ...]; s_value carries the method name.
                let obj = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_".to_string());
                let method = op.s_value.as_deref().unwrap_or("");
                let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
                if method == "append" {
                    let arg = call_args
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&format!("molt_list_append(&mut {obj}, {arg});"));
                    self.emit_alias_writeback(&obj);
                    if o != "_" && o != "none" {
                        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                    }
                } else {
                    let rhs = match method {
                        "keys" => format!("molt_dict_keys(&{obj})"),
                        "values" => format!("molt_dict_values(&{obj})"),
                        "items" => format!("molt_dict_items(&{obj})"),
                        "get" => {
                            let key = call_args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            let default = call_args
                                .get(1)
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            format!(
                                "{{ let __k = {key}; if let Some((_, v)) = if let MoltValue::Dict(d) = &{obj} {{ d.iter().find(|(k,_)| molt_eq(k, &__k)) }} else {{ None }} {{ v.clone() }} else {{ {default} }} }}"
                            )
                        }
                        _ => format!(
                            "/* MOLT_STUB: method {obj}.{method}({}) */ MoltValue::None",
                            call_args.join(", ")
                        ),
                    };
                    if o == "_" || o == "none" {
                        self.emit_line(&format!("{rhs};"));
                    } else {
                        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
                    }
                }
            }
            "call_bind" | "call_indirect" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let rhs = if args.len() >= 2 {
                    let func = rust_ident(&args[0]);
                    let builder = rust_ident(&args[1]);
                    let extra_args = args[2..]
                        .iter()
                        .map(|a| rust_clone(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let extra_stmt = if extra_args.is_empty() {
                        String::new()
                    } else {
                        format!("__call_args.extend(vec![{extra_args}]);")
                    };
                    format!(
                        "{{ let mut __call_args = Vec::new(); \
                           if let MoltValue::List(__pos) = &{builder} {{ \
                               __call_args.extend(__pos.iter().cloned()); \
                           }} else if !matches!({builder}, MoltValue::None) {{ \
                               __call_args.push({builder}.clone()); \
                           }} \
                           {extra_stmt} \
                           let __ret = molt_call(&{func}, &mut __call_args); \
                           __ret }}"
                    )
                } else if let Some(func) = args.first() {
                    format!(
                        "{{ let mut __call_args = Vec::new(); molt_call(&{}, &mut __call_args) }}",
                        rust_ident(func)
                    )
                } else {
                    "MoltValue::None".to_string()
                };
                if o == "_" || o == "none" {
                    self.emit_line(&format!("{rhs};"));
                } else {
                    self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
                }
            }
            "callargs_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "callargs_push_pos" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let val = rust_ident(&args[1]);
                    self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
                    self.emit_alias_writeback(&list);
                }
            }
            "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let other = rust_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __item in molt_iter_list(&{other}) {{ molt_list_append(&mut {list}, __item); }}"
                    ));
                    self.emit_alias_writeback(&list);
                }
            }
            "callargs_push_kw" | "callargs_expand_kwstar" => {
                // Keyword arguments are currently ignored in the Rust subset.
            }
            "func_new" | "func_new_closure" => {
                let o = out();
                let rhs = if let Some(ref fn_name) = op.s_value {
                    let fn_ident = rust_ident(fn_name);
                    format!(
                        "MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {fn_ident}(args)))"
                    )
                } else {
                    "MoltValue::None".to_string()
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "code_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 9 {
                    let filename = rust_ident(&args[0]);
                    let name = rust_ident(&args[1]);
                    let firstlineno = rust_ident(&args[2]);
                    let linetable = rust_ident(&args[3]);
                    let varnames = rust_ident(&args[4]);
                    let names = rust_ident(&args[5]);
                    let argcount = rust_ident(&args[6]);
                    let posonlyargcount = rust_ident(&args[7]);
                    let kwonlyargcount = rust_ident(&args[8]);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "molt_code_new(&{filename}, &{name}, &{firstlineno}, &{linetable}, &{varnames}, &{names}, &{argcount}, &{posonlyargcount}, &{kwonlyargcount})"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "code_slots_init" => {
                let count = op.value.unwrap_or(0);
                self.emit_line(&format!("molt_code_slots_init({count});"));
            }
            "code_slot_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(code) = args.first() {
                    let code = rust_ident(code);
                    let code_id = op.value.unwrap_or(0);
                    self.emit_line(&format!("molt_code_slot_set({code_id}, &{code});"));
                }
            }
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    let helper = if matches!(
                        op.kind.as_str(),
                        "exception_last_pending" | "exception_finally_pending_observer"
                    ) {
                        "molt_exception_last_pending()"
                    } else {
                        "molt_exception_last()"
                    };
                    self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
                }
            }
            "exception_stack_depth" | "exception_stack_enter" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    let helper = if op.kind == "exception_stack_enter" {
                        "molt_exception_stack_enter()"
                    } else {
                        "molt_exception_stack_depth()"
                    };
                    self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
                }
            }
            "exception_clear" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        "molt_exception_clear()",
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line("molt_exception_clear();");
                }
            }
            "exception_stack_exit" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let prev = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&format!("molt_exception_stack_exit(&{prev});"));
            }
            "exception_stack_set_depth" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let depth = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&format!("molt_exception_stack_set_depth(&{depth});"));
            }
            "exception_stack_clear" => {
                self.emit_line("molt_exception_stack_clear();");
            }
            "exception_set_last" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let exc = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&format!("molt_exception_set_last(&{exc});"));
            }
            "exception_active" => {
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        "molt_exception_active()",
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "trace_enter_slot" => {
                let code_id = op.value.unwrap_or(0);
                self.emit_line(&format!("molt_trace_enter_slot({code_id});"));
            }
            "trace_exit" => {
                self.emit_line("molt_trace_exit();");
            }
            "frame_locals_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let locals = args
                    .first()
                    .map(|arg| rust_ident(arg))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&format!("molt_frame_locals_set(&{locals});"));
            }
            "builtin_func" => {
                let o = out();
                let builtin = op.s_value.as_deref().unwrap_or("");
                self.emit_line(&declare(
                    &o,
                    &format!("molt_builtin_func({})", rust_string_literal(builtin)),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Builtins ───────────────────────────────────────────────────────
            "print" | "builtin_print" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let arg_list = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("molt_print(&[{arg_list}]);"));
            }
            "len" | "builtin_len" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_len(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int" | "cast_int" | "builtin_int" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Int(molt_int(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "int_from_str_of_obj" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let a = args
                    .first()
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let base = args
                    .get(1)
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let has_base = args
                    .get(2)
                    .map(|s| rust_value(s))
                    .unwrap_or_else(|| "MoltValue::Bool(false)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "{{ let __s = molt_str(&{a}); if molt_bool(&{has_base}) {{ let __base = molt_int(&{base}); MoltValue::Int(if (2..=36).contains(&__base) {{ i64::from_str_radix(__s.trim(), __base as u32).unwrap_or(0) }} else {{ 0 }}) }} else {{ MoltValue::Int(molt_int(&MoltValue::Str(__s))) }} }}"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "float" | "cast_float" | "builtin_float" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Float(molt_float(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "float_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Float(molt_float(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "str" | "cast_str" | "builtin_str" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Str(molt_str(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "bool" | "cast_bool" | "builtin_bool" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_bool(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "chr" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_chr(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ord" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_ord(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "ord_at" => {
                let o = out();
                let (obj, key) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_ord_at(&{obj}, &{key})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "abs" | "builtin_abs" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_abs({a}.clone())"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Collections ────────────────────────────────────────────────────
            "build_list" | "alloc" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "build_dict" | "dict_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                // args: [k0, v0, k1, v1, ...]
                let mut pairs = Vec::new();
                let mut i = 0;
                while i + 1 < args.len() {
                    let k = rust_ident(&args[i]);
                    let v = rust_ident(&args[i + 1]);
                    pairs.push(format!("({k}.clone(), {v}.clone())"));
                    i += 2;
                }
                let rhs = format!("MoltValue::Dict(vec![{}])", pairs.join(", "));
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "list_append" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = rust_ident(&args[0]);
                    let val = rust_ident(&args[1]);
                    self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
                    self.emit_alias_writeback(&list);
                }
            }
            "get_item" | "subscript" | "index" => {
                let o = out();
                let (obj, key) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_get_item(&{obj}, &{key})"),
                    &self.hoisted_vars.clone(),
                ));
                let alias_key = format!("__alias_key_{o}");
                self.emit_line(&declare(
                    &alias_key,
                    &format!("{key}.clone()"),
                    &self.hoisted_vars.clone(),
                ));
                self.note_indexed_alias(o, obj, alias_key);
            }
            "dict_get" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let obj = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let key = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                if let Some(default) = args.get(2) {
                    let default = rust_ident(default);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __v = molt_get_item(&{obj}, &{key}); if matches!(__v, MoltValue::None) {{ {default}.clone() }} else {{ __v }} }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_item(&{obj}, &{key})"),
                        &self.hoisted_vars.clone(),
                    ));
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = rust_ident(&args[0]);
                    let key = rust_ident(&args[1]);
                    let val = rust_ident(&args[2]);
                    // Record phi→frame mapping so loop_index_next can write back.
                    self.phi_to_frame
                        .insert(val.clone(), (obj.clone(), key.clone()));
                    self.emit_line(&format!(
                        "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
                    ));
                    self.emit_alias_writeback(&obj);
                }
            }
            "dict_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = rust_ident(&args[0]);
                    let key = rust_ident(&args[1]);
                    let val = rust_ident(&args[2]);
                    self.emit_line(&format!(
                        "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
                    ));
                    self.emit_alias_writeback(&obj);
                }
            }
            "get_attr" | "load_attr" => {
                let o = out();
                let obj = arg0(op);
                let attr = op
                    .s_value
                    .as_deref()
                    .or_else(|| op.args.as_ref().and_then(|a| a.get(1)).map(|s| s.as_str()))
                    .unwrap_or("__unknown__");
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_get_attr(&{obj}, {attr_lit})",
                        attr_lit = rust_string_literal(attr)
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }
            "get_attr_name" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name(&{obj}, &{attr})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "get_attr_name_default" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    let default = args
                        .get(2)
                        .map(|name| rust_value(name))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name_default(&{obj}, &{attr}, &{default})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "set_attr" | "store_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let value_index = if args.len() >= 3 { 2 } else { 1 };
                    let value = rust_clone(&args[value_index]);
                    let attr = op
                        .s_value
                        .as_deref()
                        .or_else(|| args.get(1).map(|s| s.as_str()))
                        .unwrap_or("__unknown__");
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {obj}, MoltValue::Str({attr_lit}.to_string()), {value});",
                            attr_lit = rust_string_literal(attr)
                        ));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }

            // ── Enumerate / zip / sorted / reversed ────────────────────────────
            "enumerate" => {
                let o = out();
                let a = arg0(op);
                let start = op
                    .args
                    .as_ref()
                    .and_then(|a| a.get(1))
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!("molt_enumerate(&{a}, molt_int(&{start}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "zip" => {
                let o = out();
                let (a, b) = args2(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_zip(&{a}, &{b})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "sorted" | "builtin_sorted" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_sorted(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "reversed" | "builtin_reversed" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_reversed(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "sum" | "builtin_sum" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_sum(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "any" | "builtin_any" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_any(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "all" | "builtin_all" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Bool(molt_all(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Range ──────────────────────────────────────────────────────────
            "range" | "builtin_range" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let (start, stop, step) = match args.len() {
                    1 => (
                        "MoltValue::Int(0)".to_string(),
                        rust_ident(&args[0]),
                        "MoltValue::Int(1)".to_string(),
                    ),
                    2 => (
                        rust_ident(&args[0]),
                        rust_ident(&args[1]),
                        "MoltValue::Int(1)".to_string(),
                    ),
                    _ => (
                        rust_ident(&args[0]),
                        rust_ident(&args[1]),
                        rust_ident(&args[2]),
                    ),
                };
                self.emit_line(&declare(
                    &o,
                    &format!(
                        "molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"
                    ),
                    &self.hoisted_vars.clone(),
                ));
            }

            "class_new" | "module_new" | "object_new" | "builtin_type" => {
                let o = out();
                self.emit_line(&declare(
                    &o,
                    "MoltValue::Dict(vec![])",
                    &self.hoisted_vars.clone(),
                ));
            }
            "bound_method_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let method = rust_value(&args[0]);
                    let obj = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __bound_method = {method}.clone(); let __bound_self = {obj}.clone(); MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {{ let mut __bound = vec![__bound_self.clone()]; __bound.extend(args.iter().cloned()); molt_call(&__bound_method, &mut __bound) }})) }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "alloc_class_static" | "alloc_class_trusted" | "alloc_class" => {
                let o = out();
                self.emit_line(&declare(
                    &o,
                    "MoltValue::Dict(vec![])",
                    &self.hoisted_vars.clone(),
                ));
            }
            "object_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = rust_ident(&args[0]);
                    let class = rust_clone(&args[1]);
                    if is_assignable_var(&obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {obj}, MoltValue::Str(\"__class__\".to_string()), {class});"
                        ));
                        self.emit_alias_writeback(&obj);
                    }
                }
            }
            "class_set_base" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class_obj = rust_ident(&args[0]);
                    let base = rust_clone(&args[1]);
                    if is_assignable_var(&class_obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {class_obj}, MoltValue::Str(\"__base__\".to_string()), {base});"
                        ));
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_set_layout_version" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class_obj = rust_ident(&args[0]);
                    let version = rust_clone(&args[1]);
                    if is_assignable_var(&class_obj) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {class_obj}, MoltValue::Str(\"__molt_layout_version__\".to_string()), {version});"
                        ));
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_merge_layout" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let class_obj = rust_ident(&args[0]);
                    let offsets = rust_clone(&args[1]);
                    let size = rust_clone(&args[2]);
                    if is_assignable_var(&class_obj) {
                        let expr =
                            format!("molt_class_merge_layout(&mut {class_obj}, {offsets}, {size})");
                        if is_assignable_var(op.out.as_deref().unwrap_or("")) {
                            let o = out();
                            self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                        } else {
                            self.emit_line(&format!("{expr};"));
                        }
                        self.emit_alias_writeback(&class_obj);
                    }
                }
            }
            "class_apply_set_name"
            | "class_layout_version"
            | "class_layout_field_count"
            | "class_layout_slot_count" => {}
            "module_cache_get" | "module_load_cached" => {
                let o = out();
                let name = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|name| rust_value(name))
                    .or_else(|| {
                        op.s_value.as_deref().map(|name| {
                            format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                        })
                    })
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_module_cache_get(&{name})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&format!("molt_module_cache_get(&{name});"));
                }
            }
            "module_cache_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let name = rust_value(&args[0]);
                    let module = rust_clone(&args[1]);
                    let expr = format!("molt_module_cache_set(&{name}, {module})");
                    let o = out();
                    if o != "_" && o != "none" && !o.is_empty() {
                        self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                    } else {
                        self.emit_line(&format!("{expr};"));
                    }
                }
            }
            "module_cache_del" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(name_arg) = args.first() {
                    let name = rust_value(name_arg);
                    let expr = format!("molt_module_cache_del(&{name})");
                    let o = out();
                    if o != "_" && o != "none" && !o.is_empty() {
                        self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
                    } else {
                        self.emit_line(&format!("{expr};"));
                    }
                }
            }
            "module_import" => {
                let o = out();
                let module = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .map(|name| rust_value(name))
                    .or_else(|| {
                        op.s_value.as_deref().map(|name| {
                            format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                        })
                    })
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                self.emit_line(&declare(
                    &o,
                    &format!("molt_import_module(&{module})"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "module_get_attr" | "module_import_from" | "module_get_name" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    let module = args
                        .first()
                        .map(|name| rust_value(name))
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    self.emit_line(&declare(
                        &o,
                        &format!(
                            "molt_get_attr_name(&{module}, &MoltValue::Str({}.to_string()))",
                            rust_string_literal(attr_str)
                        ),
                        &self.hoisted_vars.clone(),
                    ));
                } else if args.len() >= 2 {
                    let module = rust_value(&args[0]);
                    let attr = rust_value(&args[1]);
                    self.emit_line(&declare(
                        &o,
                        &format!("molt_get_attr_name(&{module}, &{attr})"),
                        &self.hoisted_vars.clone(),
                    ));
                } else {
                    self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
                }
            }
            "module_set_attr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let module = rust_ident(&args[0]);
                    let attr = rust_clone(&args[1]);
                    let value = rust_clone(&args[2]);
                    if is_assignable_var(&module) {
                        self.emit_line(&format!(
                            "molt_set_attr_name(&mut {module}, {attr}, {value});"
                        ));
                        self.emit_alias_writeback(&module);
                    }
                }
            }

            // ── No-ops / markers ───────────────────────────────────────────────
            "nop"
            | "comment"
            | "debug_label"
            | "line"
            | "type_assert"
            | "br_if"
            | "branch"
            | "alloc_task"
            | "block_on"
            | "asyncgen_locals_register"
            | "cancel_current"
            | "cancel_token_cancel"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_get_current"
            | "cancel_token_is_cancelled"
            | "cancel_token_new"
            | "cancel_token_set_current"
            | "cancelled"
            | "check_exception"
            | "ascii_from_obj"
            | "bridge_unavailable" => {
                // Stub: these ops may produce an output variable in the IR.
                // Declare it so downstream phi references compile.
                let o = out();
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&format!(
                        "let mut {o}: MoltValue = /* MOLT_STUB: {} */ MoltValue::None;",
                        op.kind
                    ));
                }
            }
            "inc_ref" | "borrow" | "binding_alias" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                if o != "_"
                    && o != "none"
                    && !o.is_empty()
                    && let Some(src) = args.first()
                {
                    let src = rust_clone(src);
                    self.emit_line(&declare(&o, &src, &self.hoisted_vars.clone()));
                }
            }
            "dec_ref" | "release" => {}

            // ── Class / instance stubs ─────────────────────────────────────────
            "alloc_instance" | "init_instance" | "instance_set_field" | "instance_get_field"
            | "instance_has_field" => {
                let o = out();
                if o != "_" && o != "none" {
                    self.emit_line(&declare(
                        &o,
                        "MoltValue::Dict(vec![])",
                        &self.hoisted_vars.clone(),
                    ));
                }
            }

            // ── Exception stubs ────────────────────────────────────────────────
            "raise" | "reraise" => {
                // In stub/native-Rust mode, Python exceptions cannot propagate
                // through the Rust call stack.  Instead of silently returning
                // None (which hides real errors), we panic with context so the
                // failure is immediately visible during testing.
                let msg = if op.args.as_ref().is_none_or(|a| a.is_empty()) {
                    "\"Python raise with no argument\"".to_string()
                } else {
                    format!(
                        "\"Python raise: {{:?}}\", {}",
                        &op.args.as_ref().unwrap()[0]
                    )
                };
                self.emit_line(&format!("panic!({msg});"));
            }
            "try_start" | "try_end" | "except_start" | "except_end" | "finally_start"
            | "finally_end" => {
                // No Rust equivalent in v1 — exception control flow ops are
                // structural markers only.  The actual error handling is done
                // via Result propagation in the generated Rust code.
            }

            // ── String operations ──────────────────────────────────────────────
            "format_string" | "string_format" => {
                let o = out();
                // Simple f-string: just convert all args to string and concat
                let args = op.args.as_deref().unwrap_or(&[]);
                let parts = args
                    .iter()
                    .map(|a| format!("molt_str(&{})", rust_ident(a)))
                    .collect::<Vec<_>>()
                    .join(" + &");
                let rhs = if parts.is_empty() {
                    "MoltValue::Str(String::new())".to_string()
                } else {
                    format!("MoltValue::Str({parts})")
                };
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── String ops ────────────────────────────────────────────────────
            "str_from_obj" => {
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::Str(molt_str(&{a}))"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "repr_from_obj" => {
                // molt_repr already returns MoltValue
                let o = out();
                let a = arg0(op);
                self.emit_line(&declare(
                    &o,
                    &format!("molt_repr(&{a})"),
                    &self.hoisted_vars.clone(),
                ));
            }

            // ── Sequence / tuple ops ──────────────────────────────────────────
            "tuple_new" | "list_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let items = args
                    .iter()
                    .map(|a| rust_clone(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&declare(
                    &o,
                    &format!("MoltValue::List(vec![{items}])"),
                    &self.hoisted_vars.clone(),
                ));
            }
            "list_fill_new" => {
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let count = args
                    .first()
                    .map(|a| rust_ident(a))
                    .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
                let fill = args
                    .get(1)
                    .map(|a| rust_ident(a))
                    .unwrap_or_else(|| "MoltValue::None".to_string());
                let rhs = format!(
                    "{{ let __n = match &{count} {{ MoltValue::Int(v) => (*v).max(0) as usize, MoltValue::Bool(v) => if *v {{ 1 }} else {{ 0 }}, _ => 0 }}; MoltValue::List(vec![{fill}.clone(); __n]) }}"
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
            "unpack_sequence" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(seq_name) = args.first() {
                    let seq = rust_ident(seq_name);
                    let outputs = &args[1..];
                    let expected_count = op.value.unwrap_or(outputs.len() as i64).max(0) as usize;
                    self.emit_line(&format!(
                        "let __unpack_seq = molt_unpack_sequence(&{seq}, {expected_count});"
                    ));
                    for (index, out_name) in outputs.iter().take(expected_count).enumerate() {
                        let out = rust_ident(out_name);
                        self.emit_line(&declare(
                            &out,
                            &format!("__unpack_seq[{index}].clone()"),
                            &self.hoisted_vars.clone(),
                        ));
                    }
                }
            }
            "string_join" => {
                // string_join(sep, iterable) → sep.join(str(x) for x in iterable)
                let o = out();
                let args = op.args.as_deref().unwrap_or(&[]);
                let sep = args
                    .first()
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "MoltValue::Str(\"\".to_string())".to_string());
                let seq = args
                    .get(1)
                    .map(|s| rust_ident(s))
                    .unwrap_or_else(|| "_seq".to_string());
                let rhs = format!(
                    "{{ let __sep = molt_str(&{sep}); if let MoltValue::List(ref __items) = {seq} {{ MoltValue::Str(__items.iter().map(|x| molt_str(x)).collect::<Vec<_>>().join(&__sep)) }} else {{ MoltValue::Str(molt_str(&{seq})) }} }}"
                );
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }

            // ── Catch-all stub ─────────────────────────────────────────────────
            other => {
                let o = out();
                let kind = other;
                if o != "_" && o != "none" && !o.is_empty() {
                    self.emit_line(&format!(
                        "let mut {o}: MoltValue = /* MOLT_STUB: {kind} */ MoltValue::None;"
                    ));
                } else {
                    self.emit_line(&format!("/* MOLT_STUB: {kind} */"));
                }
            }
        }
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

// ── IR lowering passes (shared logic, simpler than Luau variants) ─────────────

/// Mark unreachable ops after return as nop so they don't emit dead code.
fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None;
    for op in ops {
        let kind = op.kind.as_str();
        let is_open = matches!(
            kind,
            "if" | "if_not" | "loop_start" | "while_start" | "for_range" | "for_iter"
        );
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "while_end" | "end_for");

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            if let Some(d) = dead_at_depth
                && d > depth
            {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            dead_at_depth = None;
        }

        let is_terminator = matches!(
            kind,
            "ret"
                | "return"
                | "return_value"
                | "return_none"
                | "ret_none"
                | "ret_void"
                | "jump"
                | "raise"
                | "reraise"
        );
        result.push(op.clone());
        if is_terminator {
            dead_at_depth = Some(depth);
        }
    }
    result
}

/// Lower early returns (store+jump→ret pattern) — no-op for Rust since we emit `return`.
fn lower_early_returns(ops: &[OpIR]) -> Vec<OpIR> {
    ops.to_vec()
}

/// Convert `call iter() + for_iter` patterns to plain for_iter if already present.
fn lower_iter_to_for(ops: &[OpIR]) -> Vec<OpIR> {
    ops.to_vec()
}

// ── Phi hoisting helpers ──────────────────────────────────────────────────────

fn collect_phi_assignments(
    ops: &[OpIR],
    hoisted_vars: &mut BTreeSet<String>,
) -> BTreeMap<usize, Vec<(String, Vec<String>)>> {
    let mut phi_assignments: BTreeMap<usize, Vec<(String, Vec<String>)>> = BTreeMap::new();
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind == "end_if" {
            let end_if_idx = i;
            let mut j = i + 1;
            while j < ops.len() && ops[j].kind == "phi" {
                if let Some(ref out_name) = ops[j].out {
                    let phi_var = rust_ident(out_name);
                    let args: Vec<String> = ops[j]
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| rust_ident(a))
                        .collect();
                    phi_assignments
                        .entry(end_if_idx)
                        .or_default()
                        .push((phi_var.clone(), args));
                    hoisted_vars.insert(phi_var);
                }
                j += 1;
            }
        }
        i += 1;
    }
    phi_assignments
}

fn build_phi_injection_maps(
    ops: &[OpIR],
    phi_assignments: &BTreeMap<usize, Vec<(String, Vec<String>)>>,
) -> (
    BTreeMap<usize, Vec<(String, String)>>,
    BTreeMap<usize, Vec<(String, String)>>,
) {
    let mut before_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut before_end_if: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "if" | "if_not" => if_stack.push((idx, None)),
            "else" => {
                if let Some(last) = if_stack.last_mut() {
                    last.1 = Some(idx);
                }
            }
            "end_if" => {
                if let Some((_if_idx, else_idx)) = if_stack.pop()
                    && let Some(phis) = phi_assignments.get(&idx)
                {
                    for (phi_var, args) in phis {
                        if let Some(else_i) = else_idx {
                            let true_val = args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_else
                                .entry(else_i)
                                .or_default()
                                .push((phi_var.clone(), true_val));
                            let false_val = args
                                .get(1)
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_end_if
                                .entry(idx)
                                .or_default()
                                .push((phi_var.clone(), false_val));
                        } else {
                            let true_val = args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_end_if
                                .entry(idx)
                                .or_default()
                                .push((phi_var.clone(), true_val));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (before_else, before_end_if)
}

fn collect_scope_escapes(ops: &[OpIR], func: &FunctionIR, hoisted_vars: &mut BTreeSet<String>) {
    let mut depth: i32 = 0;
    let mut decl_depth: BTreeMap<String, i32> = BTreeMap::new();
    let param_set: BTreeSet<String> = func.params.iter().map(|p| rust_ident(p)).collect();

    for op in ops {
        match op.kind.as_str() {
            "if" | "if_not" | "loop_start" | "while_start" | "for_range" | "for_iter" => depth += 1,
            "end_if" | "loop_end" | "while_end" | "end_for" => depth -= 1,
            _ => {}
        }
        if let Some(ref out_name) = op.out
            && out_name != "none"
            && !op.kind.starts_with("nop")
        {
            let var = rust_ident(out_name);
            decl_depth.entry(var).or_insert(depth);
        }
        let mut refs: Vec<String> = op
            .args
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|s| rust_ident(s))
            .collect();
        if let Some(v) = op.var.as_deref() {
            refs.push(rust_ident(v));
        }
        for r in refs {
            if param_set.contains(&r) {
                continue;
            }
            if let Some(&dd) = decl_depth.get(&r)
                && dd > depth
            {
                hoisted_vars.insert(r);
            }
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

fn rust_value(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        rust_ident(name)
    }
}

fn rust_clone(name: &str) -> String {
    if name.is_empty() || name == "none" || name == "_" {
        "MoltValue::None".to_string()
    } else {
        format!("{}.clone()", rust_ident(name))
    }
}

fn rust_slot_key(offset: i64) -> String {
    format!("MoltValue::Str(\"__slot_{offset}\".to_string())")
}

fn is_assignable_var(name: &str) -> bool {
    !(name.is_empty() || name == "_" || name == "none")
}

fn out_var(op: &OpIR) -> String {
    rust_ident(op.out.as_deref().unwrap_or("_"))
}

fn var_ref(op: &OpIR) -> String {
    rust_ident(op.var.as_deref().unwrap_or("_"))
}

fn arg0(op: &OpIR) -> String {
    op.args
        .as_deref()
        .and_then(|a| a.first())
        .map(|s| rust_value(s))
        .unwrap_or_else(|| "MoltValue::None".to_string())
}

fn args2(op: &OpIR) -> (String, String) {
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

fn rust_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
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
mod tests {
    use super::*;
    use crate::{FunctionIR, SimpleIR};

    #[test]
    fn compile_keeps_annotation_functions_when_referenced() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "__main____annotate__".to_string(),
                    params: vec!["args".to_string()],
                    ops: vec![OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("fn __main____annotate__("));
    }

    #[test]
    fn compile_int_from_str_of_obj_preserves_base_operand() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![
                    "value".to_string(),
                    "base".to_string(),
                    "has_base".to_string(),
                ],
                ops: vec![
                    OpIR {
                        kind: "int_from_str_of_obj".to_string(),
                        args: Some(vec![
                            "value".to_string(),
                            "base".to_string(),
                            "has_base".to_string(),
                        ]),
                        out: Some("out".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("out".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("molt_bool(&has_base)"));
        assert!(source.contains("let __base = molt_int(&base);"));
        assert!(source.contains("i64::from_str_radix(__s.trim(), __base as u32)"));
    }

    #[test]
    fn compile_numeric_equality_does_not_fall_back_for_non_numeric_values() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "cmp_eq".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("fn molt_is_numeric(x: &MoltValue) -> bool"));
        assert!(source.contains("_ if molt_is_numeric(a) && molt_is_numeric(b) =>"));
        assert!(source.contains("_ => false,"));
    }

    #[test]
    fn compile_rust_arithmetic_fast_path_ignores_transport_hints() {
        let mut backend = RustBackend::new();
        let mut add = OpIR {
            kind: "add".to_string(),
            args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
            out: Some("sum".to_string()),
            ..OpIR::default()
        };
        add.fast_int = Some(true);
        add.type_hint = Some("int".to_string());
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "helper".to_string(),
                params: vec!["lhs".to_string(), "rhs".to_string()],
                ops: vec![
                    add,
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("sum".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("let mut sum: MoltValue = molt_add(lhs.clone(), rhs.clone());"));
        assert!(!source.contains(
            "let mut sum: MoltValue = MoltValue::Int(molt_int(&lhs).wrapping_add(molt_int(&rhs)))"
        ));
    }

    #[test]
    fn compile_rust_arithmetic_fast_path_uses_typed_operands_without_transport_hints() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "helper".to_string(),
                params: vec!["lhs".to_string(), "rhs".to_string()],
                ops: vec![
                    OpIR {
                        kind: "add".to_string(),
                        args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                        out: Some("sum".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("sum".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: Some(vec!["int".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains(
            "let mut sum: MoltValue = MoltValue::Int(molt_int(&lhs).wrapping_add(molt_int(&rhs)))"
        ));
        assert!(!source.contains("let mut sum: MoltValue = molt_add(lhs.clone(), rhs.clone());"));
    }

    #[test]
    fn compile_list_append_writes_back_indexed_aliases() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "helper".to_string(),
                    params: vec!["v0".to_string(), "v1".to_string(), "v3".to_string()],
                    ops: vec![
                        OpIR {
                            kind: "index".to_string(),
                            args: Some(vec!["v0".to_string(), "v1".to_string()]),
                            out: Some("v2".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "list_append".to_string(),
                            args: Some(vec!["v2".to_string(), "v3".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "return_none".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("let mut __alias_key_v2: MoltValue = v1.clone();"));
        assert!(source.contains("molt_list_append(&mut v2, v3.clone());"));
        assert!(source.contains("molt_set_item(&mut v0, __alias_key_v2.clone(), v2.clone());"));
    }

    #[test]
    fn compile_call_method_uses_s_value_method_name() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec!["items".to_string(), "value".to_string()],
                ops: vec![
                    OpIR {
                        kind: "call_method".to_string(),
                        s_value: Some("append".to_string()),
                        args: Some(vec!["items".to_string(), "value".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend
            .compile_checked(&ir)
            .expect("call_method should lower from s_value without stub markers");
        assert!(source.contains("molt_list_append(&mut items, value.clone());"));
        assert!(!source.contains("MOLT_STUB: method"));
    }

    #[test]
    fn compile_ord_at_emits_fused_helper() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "ord_at_unicode".to_string(),
                params: vec!["s".to_string(), "i".to_string()],
                ops: vec![
                    OpIR {
                        kind: "ord_at".to_string(),
                        args: Some(vec!["s".to_string(), "i".to_string()]),
                        out: Some("code".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("code".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: Some(vec!["str".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend
            .compile_checked(&ir)
            .expect("ord_at should lower without stub markers");
        assert!(source.contains("fn molt_ord_at(obj: &MoltValue, key: &MoltValue)"));
        assert!(source.contains("fn molt_get_item(obj: &MoltValue, key: &MoltValue)"));
        assert!(source.contains("fn molt_ord(x: &MoltValue)"));
        assert!(source.contains("let mut code: MoltValue = molt_ord_at(&s, &i);"));
        assert!(!source.contains("MOLT_STUB"));
    }

    #[test]
    fn compile_code_slots_contains_and_ref_markers_without_stubs() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![
                    "filename".to_string(),
                    "name".to_string(),
                    "firstlineno".to_string(),
                    "linetable".to_string(),
                    "varnames".to_string(),
                    "names".to_string(),
                    "argcount".to_string(),
                    "posonlyargcount".to_string(),
                    "kwonlyargcount".to_string(),
                    "container".to_string(),
                    "needle".to_string(),
                ],
                ops: vec![
                    OpIR {
                        kind: "code_slots_init".to_string(),
                        value: Some(4),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "code_new".to_string(),
                        args: Some(vec![
                            "filename".to_string(),
                            "name".to_string(),
                            "firstlineno".to_string(),
                            "linetable".to_string(),
                            "varnames".to_string(),
                            "names".to_string(),
                            "argcount".to_string(),
                            "posonlyargcount".to_string(),
                            "kwonlyargcount".to_string(),
                        ]),
                        out: Some("code".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "inc_ref".to_string(),
                        args: Some(vec!["code".to_string()]),
                        out: Some("owned_code".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(2),
                        args: Some(vec!["owned_code".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "frame_locals_set".to_string(),
                        args: Some(vec!["container".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_enter".to_string(),
                        out: Some("exc_base".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_depth".to_string(),
                        out: Some("exc_depth".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_set_depth".to_string(),
                        args: Some(vec!["exc_depth".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_exit".to_string(),
                        args: Some(vec!["exc_base".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("last_exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last_pending".to_string(),
                        out: Some("pending_exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_clear".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dec_ref".to_string(),
                        args: Some(vec!["owned_code".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "contains".to_string(),
                        args: Some(vec!["container".to_string(), "needle".to_string()]),
                        out: Some("present".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("present".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend
            .compile_checked(&ir)
            .expect("Rust source should lower code metadata, contains, and ref markers");
        assert!(source.contains("fn molt_code_new("));
        assert!(source.contains("fn molt_code_slots_init("));
        assert!(source.contains("fn molt_code_slot_set("));
        assert!(source.contains("molt_code_slots_init(4);"));
        assert!(source.contains(
            "let mut code: MoltValue = molt_code_new(&filename, &name, &firstlineno, &linetable, &varnames, &names, &argcount, &posonlyargcount, &kwonlyargcount);"
        ));
        assert!(source.contains("let mut owned_code: MoltValue = code.clone();"));
        assert!(source.contains("molt_code_slot_set(2, &owned_code);"));
        assert!(source.contains("fn molt_exception_stack_enter() -> MoltValue"));
        assert!(source.contains("fn molt_trace_enter_slot(code_id: i64) -> MoltValue"));
        assert!(source.contains("let mut exc_base: MoltValue = molt_exception_stack_enter();"));
        assert!(source.contains("let mut exc_depth: MoltValue = molt_exception_stack_depth();"));
        assert!(source.contains("molt_exception_stack_set_depth(&exc_depth);"));
        assert!(source.contains("molt_exception_stack_exit(&exc_base);"));
        assert!(source.contains("let mut last_exc: MoltValue = molt_exception_last();"));
        assert!(source.contains("let mut pending_exc: MoltValue = molt_exception_last_pending();"));
        assert!(source.contains(
            "let mut present: MoltValue = MoltValue::Bool(molt_in(&needle, &container));"
        ));
        assert!(!source.contains("MOLT_STUB"));
    }

    #[test]
    fn compile_checked_reports_stub_markers() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unsupported".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "matmul".to_string(),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let err = backend
            .compile_checked(&ir)
            .expect_err("unsupported ops should be rejected with marker details");
        assert!(err.contains("MOLT_STUB: matmul"));
    }

    #[test]
    fn compile_boolean_short_circuit_omits_unused_if_parentheses() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "and".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "or".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v3".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("if !molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
        assert!(source.contains("if molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
        assert!(!source.contains("(if !molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
        assert!(!source.contains("(if molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
    }

    #[test]
    fn compile_unpack_sequence_lowers_outputs_instead_of_stub() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec!["seq".to_string()],
                ops: vec![
                    OpIR {
                        kind: "unpack_sequence".to_string(),
                        args: Some(vec![
                            "seq".to_string(),
                            "left".to_string(),
                            "right".to_string(),
                        ]),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "tuple_new".to_string(),
                        args: Some(vec!["left".to_string(), "right".to_string()]),
                        out: Some("pair".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("pair".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("fn molt_unpack_sequence("));
        assert!(source.contains("fn molt_unpack_too_many_message("));
        assert!(source.contains("fn molt_runtime_target_at_least("));
        assert!(source.contains("cannot unpack non-iterable {} object"));
        assert!(!source.contains("cannot unpack non-sequence"));
        assert!(source.contains("let __unpack_seq"));
        assert!(source.contains("let mut left: MoltValue = __unpack_seq[0].clone();"));
        assert!(source.contains("let mut right: MoltValue = __unpack_seq[1].clone();"));
        assert!(!source.contains("MOLT_STUB: unpack_sequence"));
    }

    #[test]
    fn compile_module_cache_ops_lower_to_runtime_cache() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("alpha".to_string()),
                        out: Some("name".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        args: Some(vec!["name".to_string()]),
                        out: Some("miss".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_new".to_string(),
                        args: Some(vec!["name".to_string()]),
                        out: Some("module".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_set".to_string(),
                        args: Some(vec!["name".to_string(), "module".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        args: Some(vec!["name".to_string()]),
                        out: Some("hit".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_del".to_string(),
                        args: Some(vec!["name".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend
            .compile_checked(&ir)
            .expect("module cache ops should lower without stub markers");
        assert!(source.contains("fn molt_module_cache_get("));
        assert!(source.contains("fn molt_module_cache_set("));
        assert!(source.contains("fn molt_module_cache_del("));
        assert!(source.contains("let mut miss: MoltValue = molt_module_cache_get(&name);"));
        assert!(source.contains("molt_module_cache_set(&name, module.clone());"));
        assert!(source.contains("let mut hit: MoltValue = molt_module_cache_get(&name);"));
        assert!(source.contains("molt_module_cache_del(&name);"));
        assert!(!source.contains("let mut miss: MoltValue = MoltValue::Bool(true);"));
        assert!(!source.contains("let mut hit: MoltValue = MoltValue::Bool(true);"));
        assert!(!source.contains("MOLT_STUB: module_cache"));
    }

    #[test]
    fn compile_const_bigint_parses_from_string_literal() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bigint".to_string(),
                        s_value: Some("2305843009213693951".to_string()),
                        out: Some("big".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(
            source.contains("MoltValue::Int(\"2305843009213693951\".parse::<i64>().unwrap_or(0))")
        );
        assert!(
            !source.contains("MoltValue::Int(2305843009213693951.parse::<i64>().unwrap_or(0))")
        );
    }

    #[test]
    fn compile_store_var_and_load_var_use_named_local_storage() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "helper".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("src".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store_var".to_string(),
                        var: Some("rows".to_string()),
                        args: Some(vec!["src".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "load_var".to_string(),
                        var: Some("rows".to_string()),
                        out: Some("tmp".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("tmp".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(source.contains("let mut rows: MoltValue = MoltValue::None;"));
        assert!(source.contains("rows = src.clone();"));
        assert!(source.contains("let mut tmp: MoltValue = rows.clone();"));
        assert!(!source.contains("MOLT_STUB: store_var"));
        assert!(!source.contains("MOLT_STUB: load_var"));
    }

    #[test]
    fn jump_after_loop_does_not_capture_scoped_set_item_temps() {
        let mut backend = RustBackend::new();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "helper".to_string(),
                params: vec!["frame".to_string()],
                ops: vec![
                    OpIR {
                        kind: "loop_start".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("answer".to_string()),
                        out: Some("key".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(42),
                        out: Some("val".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_item".to_string(),
                        args: Some(vec![
                            "frame".to_string(),
                            "key".to_string(),
                            "val".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_break".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_end".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let source = backend.compile(&ir);
        assert!(!source.contains("return molt_get_item(&frame, &key);"));
        assert!(source.contains("return MoltValue::None; /* jump: no prior store */"));
    }

    #[test]
    fn strip_dead_after_return_skips_jump_after_nested_return_until_else() {
        let ops = vec![
            OpIR {
                kind: "if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "return_none".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("v0".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            },
        ];

        let lowered = strip_dead_after_return(&ops);
        let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
        assert_eq!(kinds, vec!["if", "return_none", "else", "const", "end_if"]);
    }

    #[test]
    fn strip_dead_after_return_skips_top_level_jump_after_return() {
        let ops = vec![
            OpIR {
                kind: "return_none".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("v0".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
        ];

        let lowered = strip_dead_after_return(&ops);
        let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
        assert_eq!(kinds, vec!["return_none"]);
    }
}
