use super::*;

impl LuauBackend {
    /// Emit source into a private buffer. Public callers must enter through
    /// `compile_checked`, which owns the Result contract and never publishes a
    /// partial program.
    pub(super) fn emit_source(&mut self, ir: &SimpleIR) -> String {
        // Reset the fail-closed accumulator for this compilation so a reused
        // backend instance does not carry unsupported-op records across runs.
        self.unsupported_ops.clear();
        self.function_symbols = ir.functions.iter().map(|func| func.name.clone()).collect();
        self.inherited_frame_context_functions = ir
            .functions
            .iter()
            .filter(|func| matches!(func.execution_context, ExecutionContextPolicy::Inherited))
            .map(|func| func.name.clone())
            .collect();
        let has_execution_frames = ir
            .functions
            .iter()
            .flat_map(|func| &func.ops)
            .any(OpIR::uses_execution_frame);
        // Phase 1: Emit all function bodies to a temporary buffer so we can
        // scan which runtime helpers are actually referenced.
        let emit_funcs: Vec<&FunctionIR> = ir.functions.iter().collect();

        let mut func_output = String::with_capacity(8192);
        std::mem::swap(&mut self.output, &mut func_output);

        let extra_forward_decls = self.collect_invocation_forward_decls(&emit_funcs);

        // Defer forward declarations until the selected runtime closure is
        // known. Helpers and guest declarations share Luau's 200-local chunk.
        self.uses_forward_decls = emit_funcs.len() > 1 || !extra_forward_decls.is_empty();

        for func in &emit_funcs {
            eprintln!("Luau: emitting function {}", func.name);
            self.emit_function_body(func);
            self.output.push('\n');
        }

        self.emit_line("-- Entry point");
        self.emit_line("if molt_main then");
        self.push_indent();
        if has_execution_frames {
            self.emit_line("local __molt_ok, __molt_error = xpcall(molt_main, function(error_value) local context, owner = molt_frame_context(); local exception, _restored = molt_frame_finalize(context, owner, 0, error_value, true); return exception end)");
            self.emit_line("if not __molt_ok then error(__molt_error, 0) end");
        } else {
            self.emit_line("molt_main()");
        }
        self.pop_indent();
        self.emit_line("end");

        let mut func_body = std::mem::take(&mut self.output);
        self.output = func_output;

        // Phase 2: Run text-level optimizations on the function body BEFORE
        // scanning for prelude helpers. Inlining passes may eliminate helper
        // calls, leaving dead definitions if we scan before optimization.
        optimize_luau_source(&mut func_body);

        // Phase 3: Emit prelude with only the helpers that survive optimization.
        let runtime_locals =
            self.emit_prelude_conditional(&func_body, runtime_prelude::needs_builtin_namespace(ir));
        let total_decls = emit_funcs.len() + extra_forward_decls.len();
        if runtime_locals + total_decls > runtime_prelude::CHUNK_LOCAL_LIMIT {
            self.unsupported_ops.push(format!(
                "runtime exports and guest function declarations need {} chunk locals, exceeding the {} available before the entry-point guard",
                runtime_locals + total_decls,
                runtime_prelude::CHUNK_LOCAL_LIMIT,
            ));
        } else if self.uses_forward_decls {
            self.emit_line("-- Forward declarations");
            for func in &emit_funcs {
                let name = emit_function_ident(&func.name);
                self.emit_line(&format!("local {name}"));
            }
            for name in &extra_forward_decls {
                self.emit_line(&format!("local {name}"));
            }
            self.output.push('\n');
        }

        // Phase 4: Combine prelude + optimized function bodies.
        self.output.push_str(&func_body);

        std::mem::take(&mut self.output)
    }

    #[cfg(test)]
    pub(super) fn compile(&mut self, ir: &SimpleIR) -> String {
        self.emit_source(ir)
    }

    /// Compile via the IR pipeline with validation and performance review.
    ///
    /// This path is intentionally fail-closed: preview/IR-pipeline builds must
    /// not emit unchecked Luau when validation discovers unsupported semantics.
    pub fn compile_via_ir(&mut self, ir: &SimpleIR) -> Result<String, String> {
        self.compile_checked(ir)
    }

    /// Compile the given IR and reject preview-blocker markers that would
    /// otherwise silently emit syntactically valid but semantically incomplete
    /// Luau.
    pub fn compile_checked(&mut self, ir: &SimpleIR) -> Result<String, String> {
        validate_luau_function_symbol_contract(ir)?;
        molt_tir::target_admission::validate_target_contract_with_representation_plan(
            ir,
            &crate::tir::target_info::TargetInfo::luau_release_fast(),
            validate_luau_identity_contract,
        )?;
        runtime_prelude::validate_ir_adapters(ir)?;
        let source = self.emit_source(ir);
        // Dispatch failures emit no source and this Result boundary never
        // publishes a partial program. Source validation separately owns
        // malformed control-flow diagnostics and block structure.
        if !self.unsupported_ops.is_empty() {
            return Err(format!(
                "luau backend refuses to emit fail-open codegen for unsupported op(s): {} \
                 -- use --target native, or add lowering to the luau op dispatch",
                self.unsupported_ops.join(", ")
            ));
        }
        validate_luau_source(&source)?;

        // Performance review — report remaining opportunities to stderr.
        let perf_issues = review_luau_perf(&source);
        if !perf_issues.is_empty() {
            eprintln!(
                "[molt-luau] Performance review ({} issue{}):",
                perf_issues.len(),
                if perf_issues.len() == 1 { "" } else { "s" }
            );
            for (ln, cat, msg) in perf_issues.iter().take(20) {
                eprintln!("  L{ln} [{cat}] {msg}");
            }
            if perf_issues.len() > 20 {
                eprintln!("  ... {} more", perf_issues.len() - 20);
            }
        } else {
            eprintln!("[molt-luau] Performance review: clean — no issues found");
        }

        Ok(source)
    }

    pub(super) fn emit_prelude_conditional(
        &mut self,
        func_body: &str,
        publish_builtins: bool,
    ) -> usize {
        match runtime_prelude::library()
            .and_then(|library| library.emit(func_body, publish_builtins))
        {
            Ok(prelude) => {
                self.output.push_str(&prelude.source);
                prelude.local_count
            }
            Err(reason) => {
                self.unsupported_ops
                    .push(format!("runtime helper dependency: {reason}"));
                0
            }
        }
    }
}

pub(super) fn validate_luau_function_symbol_contract(ir: &SimpleIR) -> Result<(), String> {
    let mut entrypoints = 0usize;
    for function in &ir.functions {
        if classify_function_symbol(&function.name) != LuauFunctionSymbol::CompilerEntrypoint {
            continue;
        }
        entrypoints += 1;
        if !function.params.is_empty() {
            return Err(
                "luau target rejected before source generation: `molt_main` is the compiler ABI entrypoint and must have no parameters; user functions named `molt_main` must arrive through the frontend's qualified symbol authority"
                    .to_string(),
            );
        }
    }
    if entrypoints > 1 {
        return Err(
            "luau target rejected before source generation: duplicate compiler ABI entrypoint `molt_main`"
                .to_string(),
        );
    }
    Ok(())
}

pub(super) fn validate_luau_identity_contract(
    function: &FunctionIR,
    plan: &ScalarRepresentationPlan,
) -> Result<(), String> {
    // Luau's CallArgs runtime currently has one exact **mapping carrier: the
    // ordered Molt dict.  Do not let the presence of an emitter arm imply the
    // generic Python keys()/__getitem__ mapping protocol.  A direct canonical
    // producer is deliberately required until that shared protocol exists.
    let mut canonical_ordered_mappings = BTreeSet::new();
    for (index, op) in function.ops.iter().enumerate() {
        if molt_ir::tir::op_kinds_generated::simpleir_kind_requires_luau_ordered_mapping(&op.kind) {
            let args = op.args.as_deref().unwrap_or(&[]);
            if let Some(mapping) = args.get(1)
                && !canonical_ordered_mappings.contains(mapping)
            {
                return Err(format!(
                    "luau target rejected before source generation: {}:op#{index} `callargs_expand_kwstar`: Luau requires a directly produced canonical ordered Molt dict because the generic Python keys/getitem mapping protocol is unavailable",
                    function.name,
                ));
            }
        }
        if let Some(out) = op.out.as_ref() {
            canonical_ordered_mappings.remove(out);
            if matches!(op.kind.as_str(), "dict_new" | "build_dict") {
                canonical_ordered_mappings.insert(out.clone());
            }
        }
        if !matches!(op.kind.as_str(), "is" | "is_not") {
            continue;
        }
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() < 2 {
            continue;
        }
        if identity_lowering(plan, &args[0], &args[1]) == IdentityLowering::Reject {
            return Err(format!(
                "luau target rejected before source generation: {}:op#{index} `{}`: identity needs alias/reference/singleton provenance or statically disjoint scalar kinds because Luau compares same-kind numbers and strings by value",
                function.name, op.kind,
            ));
        }
    }
    Ok(())
}
