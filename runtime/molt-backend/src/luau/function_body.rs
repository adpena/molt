use super::*;

impl LuauBackend {
    pub(super) fn emit_function_body(&mut self, func: &FunctionIR) {
        // Pre-process: lower early returns (store+jump→ret) then strip dead code.
        let ops = lower_early_returns(&func.ops);
        let ops = strip_dead_after_return(&ops);
        let ops = lower_iter_to_for(&ops);
        let ops = hoist_exception_edge_block_arg_stores(&ops);
        let (ops, pcall_escaped_vars) = lower_try_to_pcall(&ops);
        let scalar_func = FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops: ops.clone(),
            param_types: func.param_types.clone(),
            source_file: func.source_file.clone(),
            is_extern: func.is_extern,
        };
        self.scalar_plan = ScalarRepresentationPlan::for_function_ir(&scalar_func);

        // Build typed parameter list.  When `param_types` carries per-param
        // type hints from the frontend we emit Luau type annotations so the
        // native JIT can skip runtime type guards.
        let typed_params: Vec<String> = func
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ident = sanitize_ident(p);
                let luau_ty = func
                    .param_types
                    .as_ref()
                    .and_then(|pts| pts.get(i))
                    .map(|t| python_type_to_luau(t))
                    .unwrap_or("any");
                format!("{ident}: {luau_ty}")
            })
            .collect();
        let params = typed_params.join(", ");

        let name = sanitize_ident(&func.name);
        if self.uses_forward_decls {
            // Forward-declared assignment form — @native is not supported on
            // bare `name = function(` in Luau, so we skip the attribute here.
            let _ = writeln!(self.output, "{name} = function({params})");
        } else {
            // Emit @native attribute to enable Luau's native codegen.  This is
            // zero-cost when the JIT is off and enables specialisation when it
            // is on.  Type-annotated parameters further allow the JIT to skip
            // runtime type guards.
            self.output.push_str("@native\n");
            let _ = writeln!(self.output, "local function {name}({params})");
        }
        self.push_indent();

        // Mark position for post-processing hoisted var declarations.
        let func_start = self.output.len();

        // Reset per-function state.
        self.hoisted_vars.clear();
        self.tuple_vars.clear();
        self.try_depth_counter.clear();
        self.pcall_counter = 0;
        self.inside_pcall_body = false;
        self.nonneg_consts.clear();
        self.scope_local_count = 0;
        self.func_body_indent = self.indent as u32;
        self.in_spill_do_block = false;
        // Pre-count ops that will produce `local` declarations.
        // If > 190, enable local-spilling `do...end` blocks.
        let local_producing_ops = ops
            .iter()
            .filter(|op| op.out.is_some() && op.out.as_deref() != Some("none"))
            .count();
        self.needs_local_spill = local_producing_ops > 190;

        // Pre-declare loop index variables so they persist across iterations.
        let mut loop_idx_vars = Vec::new();
        for op in &ops {
            if op.kind == "loop_index_start"
                && let Some(ref out_name) = op.out
            {
                loop_idx_vars.push(sanitize_ident(out_name));
            }
        }
        if !loop_idx_vars.is_empty() {
            for var in &loop_idx_vars {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Pre-declare closure slot variables used by closure_store/closure_load.
        // These are generator/coroutine state variables that must persist across
        // loop iterations and function calls.
        {
            let mut closure_slots: Vec<String> = Vec::new();
            for op in &ops {
                if (op.kind == "closure_store" || op.kind == "closure_load")
                    && let Some(ref args) = op.args
                    && let Some(slot) = args.first()
                {
                    let var_name = format!("__closure_{}", sanitize_ident(slot));
                    if !closure_slots.contains(&var_name) {
                        closure_slots.push(var_name);
                    }
                }
            }
            for var in &closure_slots {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Pre-scan: collect variables defined by const/const_int with non-negative values.
        for op in &ops {
            match op.kind.as_str() {
                "const" | "const_int" => {
                    if let (Some(out_name), Some(v)) = (&op.out, op.value)
                        && v >= 0
                    {
                        self.nonneg_consts.insert(out_name.clone());
                    }
                }
                _ => {}
            }
        }

        // Phi hoisting: find `end_if` followed by `phi` ops and collect
        // the phi output variables.  Also find variables first declared
        // inside if/else blocks but referenced outside (scope escape).
        let mut phi_assignments: BTreeMap<usize, Vec<(String, Vec<String>)>> = BTreeMap::new();
        {
            // Pass 1: find phi ops that follow end_if and record their
            // output vars plus branch values.
            let mut i = 0;
            while i < ops.len() {
                if ops[i].kind == "end_if" {
                    // Scan forward for consecutive phi ops.
                    let end_if_idx = i;
                    let mut j = i + 1;
                    while j < ops.len() && ops[j].kind == "phi" {
                        if let Some(ref out_name) = ops[j].out {
                            let phi_var = sanitize_ident(out_name);
                            let args: Vec<String> = ops[j]
                                .args
                                .as_deref()
                                .unwrap_or(&[])
                                .iter()
                                .map(|a| sanitize_ident(a))
                                .collect();
                            phi_assignments
                                .entry(end_if_idx)
                                .or_default()
                                .push((phi_var.clone(), args));
                            self.hoisted_vars.insert(phi_var);
                        }
                        j += 1;
                    }
                }
                i += 1;
            }

            // Pass 2: find variables first declared inside if/else/loop
            // blocks but used outside, OR declared in one block and used
            // in a different block at the same depth (e.g., two sequential
            // while loops). Track (depth, block_id) pairs.
            let mut depth: i32 = 0;
            let mut block_id: u32 = 0;
            let mut decl_scope: BTreeMap<String, (i32, u32)> = BTreeMap::new();
            let param_set: BTreeSet<String> =
                func.params.iter().map(|p| sanitize_ident(p)).collect();

            for op in &ops {
                match op.kind.as_str() {
                    "if" | "loop_start" | "for_range" | "for_iter" | "pcall_wrap_begin" => {
                        depth += 1;
                        block_id += 1;
                    }
                    "else" => {
                        // else starts a new block at the same depth
                        block_id += 1;
                    }
                    "end_if" | "loop_end" | "end_for" | "pcall_wrap_end" => {
                        depth -= 1;
                        block_id += 1;
                    }
                    _ => {}
                }
                // Record first declaration site of each variable.
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                    && !op.kind.starts_with("nop")
                {
                    let var = sanitize_ident(out_name);
                    decl_scope.entry(var).or_insert((depth, block_id));
                }
                // Check if any referenced variable was declared at a deeper
                // depth OR in a different block at the same depth.
                let refs: Vec<&str> = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| s.as_str())
                    .chain(op.var.as_deref())
                    .collect();
                for r in refs {
                    let var = sanitize_ident(r);
                    if param_set.contains(&var) {
                        continue;
                    }
                    if let Some(&(dd, db)) = decl_scope.get(&var) {
                        // Hoist if: declared deeper, OR declared at same
                        // depth but in a different block (different loop/if).
                        if dd > depth || (dd > 0 && dd == depth && db != block_id) {
                            self.hoisted_vars.insert(var);
                        }
                    }
                }
            }
        }

        // Add pcall-escaped variables to hoisted set so they use assignment
        // form instead of `local` inside the pcall closure.
        for escaped_var in &pcall_escaped_vars {
            self.hoisted_vars.insert(sanitize_ident(escaped_var));
        }
        // TIR store_var/load_var represent named storage slots that must remain
        // visible across structured control-flow edges in the emitted function.
        for op in &ops {
            if op.kind == "store_var"
                && let Some(name) = op.var.as_deref().or(op.out.as_deref())
            {
                self.hoisted_vars.insert(sanitize_ident(name));
            }
        }

        // Emit pre-declarations for hoisted variables.  Cap at 150 to stay
        // within Luau's ~200 local register limit.  Variables beyond the cap
        // are removed from hoisted_vars so they get `local` declarations
        // inline (which Luau handles as new inner-scope bindings).
        if !self.hoisted_vars.is_empty() {
            let mut sorted: Vec<String> = self.hoisted_vars.iter().cloned().collect();
            sorted.sort();
            let cap = 150;
            if sorted.len() > cap {
                let overflow: Vec<String> = sorted.drain(cap..).collect();
                for var in &overflow {
                    self.hoisted_vars.remove(var);
                }
            }
            for var in &sorted {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Build a map: for each if block, record the phi assignments to
        // inject into true/false branches.  We need to find the matching
        // if/else/end_if structure for each phi group.
        // Strategy: walk ops, track if/else/end_if nesting, and for each
        // end_if that has phi_assignments, record the injection points.
        //
        // For a pattern: if(idx_a) ... else(idx_b) ... end_if(idx_c) phi
        // We inject:
        //   - at end of true branch (just before else): phi_var = args[0]
        //   - at end of false branch (just before end_if): phi_var = args[1]
        //
        // We track: for each end_if index with phis, find the matching
        // if and else indices.
        let mut phi_inject_before_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        let mut phi_inject_before_end_if: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        // For if-without-else + phi, we need to synthesize an else branch.
        // Track: end_if_idx → Vec<(phi_var, false_val)>
        let mut phi_synthesize_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        if !phi_assignments.is_empty() {
            // Walk ops to find if/else/end_if triples.
            let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new(); // (if_idx, else_idx)
            for (idx, op) in ops.iter().enumerate() {
                match op.kind.as_str() {
                    "if" => {
                        if_stack.push((idx, None));
                    }
                    "else" => {
                        if let Some(last) = if_stack.last_mut() {
                            last.1 = Some(idx);
                        }
                    }
                    "end_if" => {
                        if let Some((if_idx, else_idx)) = if_stack.pop()
                            && let Some(phis) = phi_assignments.get(&idx)
                        {
                            for (phi_var, args) in phis {
                                if let Some(else_i) = else_idx {
                                    // True branch value: inject before else.
                                    let true_val =
                                        args.first().cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_else
                                        .entry(else_i)
                                        .or_default()
                                        .push((phi_var.clone(), true_val));
                                    // False branch value: inject before end_if.
                                    let false_val =
                                        args.get(1).cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_end_if
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), false_val));
                                } else {
                                    // No else branch — this is the `and` short-circuit
                                    // pattern.  The true branch sets the phi from
                                    // args[0].  When false, the phi should get the
                                    // if-condition variable (the LHS of `and`).
                                    let true_val =
                                        args.first().cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_end_if
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), true_val));
                                    // Extract the condition variable from the `if` op.
                                    let cond_var = ops[if_idx]
                                        .args
                                        .as_deref()
                                        .and_then(|a| a.first())
                                        .map(|s| sanitize_ident(s))
                                        .unwrap_or_else(|| "nil".to_string());
                                    phi_synthesize_else
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), cond_var));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Emit ops with phi injection and loop_start handling.
        let mut i = 0;
        while i < ops.len() {
            // Inject phi true-branch assignments before else.
            if let Some(injects) = phi_inject_before_else.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}"));
                }
            }
            // Inject phi false-branch assignments before end_if.
            if let Some(injects) = phi_inject_before_end_if.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}"));
                }
            }
            // Synthesize else branch for if-without-else + phi (and pattern).
            // This assigns the condition variable when the if body was skipped.
            if ops[i].kind == "end_if"
                && let Some(synth) = phi_synthesize_else.get(&i)
            {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
                for (var, cond_val) in synth {
                    self.emit_line(&format!("{var} = {cond_val}"));
                }
            }

            if ops[i].kind == "loop_start"
                && i + 1 < ops.len()
                && ops[i + 1].kind == "loop_index_start"
            {
                let idx_op = &ops[i + 1];
                if let Some(ref out_name) = idx_op.out {
                    let out = sanitize_ident(out_name);
                    let args = idx_op.args.as_deref().unwrap_or(&[]);
                    if let Some(start_val) = args.first() {
                        let start = sanitize_ident(start_val);
                        self.emit_line(&format!("{out} = {start}"));
                    } else {
                        self.emit_line(&format!("{out} = 0"));
                    }
                }
                self.emit_op(&ops[i]);
                i += 2;
            } else {
                self.emit_op(&ops[i]);
                i += 1;
            }
        }

        self.pop_indent();
        self.emit_line("end");

        // Post-process: (1) for hoisted variables, replace `local var = ...`
        // with `var = ...` (the pre-declaration already emitted `local var`),
        // and (2) deduplicate any remaining `local` declarations — if a
        // variable was already declared with `local` earlier in the function,
        // subsequent `local var = ...` lines become plain `var = ...`.
        {
            let func_output = &self.output[func_start..];
            let mut patched = String::with_capacity(func_output.len());
            let mut seen_locals: BTreeSet<String> = BTreeSet::new();
            // Seed with function parameters — they are implicitly declared.
            for p in &func.params {
                seen_locals.insert(sanitize_ident(p));
            }
            for line in func_output.lines() {
                let trimmed = line.trim_start();
                let mut replaced = false;
                if let Some(after_local) = trimmed.strip_prefix("local ") {
                    // Extract the variable name: "local vXXX = ..." or "local vXXX"
                    // Skip "local function ..." lines — those are function defs.
                    if !after_local.starts_with("function ") {
                        let var_end = after_local
                            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                            .unwrap_or(after_local.len());
                        let var_name = &after_local[..var_end];
                        if !var_name.is_empty() {
                            let rest = after_local[var_end..].trim_start();
                            if rest.starts_with('=') {
                                // This is `local var = ...` — check for hoisted or duplicate.
                                if self.hoisted_vars.contains(var_name)
                                    || !seen_locals.insert(var_name.to_string())
                                {
                                    // Already declared — strip `local `.
                                    let indent = &line[..line.len() - trimmed.len()];
                                    patched.push_str(indent);
                                    patched.push_str(after_local);
                                    patched.push('\n');
                                    replaced = true;
                                } else {
                                    // First declaration — keep `local`.
                                    // (already inserted into seen_locals above)
                                }
                            } else if rest.is_empty() || rest.starts_with("--") {
                                // Bare `local var` pre-declaration.
                                seen_locals.insert(var_name.to_string());
                            }
                        }
                    }
                }
                if !replaced {
                    patched.push_str(line);
                    patched.push('\n');
                }
            }
            self.output.truncate(func_start);
            self.output.push_str(&patched);
        }

        self.hoisted_vars.clear();
        self.tuple_vars.clear();
    }
}
