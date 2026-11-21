use super::backend_selection::{NativeCodegenBackend, select_native_codegen_backend};
use super::*;
use molt_ir::backend_environment::BackendEnv;
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;

fn validate_extern_function_signatures(functions: &[FunctionIR]) {
    let mut signatures = BTreeMap::new();
    for function in functions.iter().filter(|function| function.is_extern) {
        let signature = function
            .extern_signature()
            .unwrap_or_else(|error| panic!("invalid native extern declaration: {error}"));
        if let Some(previous) = signatures.insert(function.name.clone(), signature)
            && previous != signature
        {
            panic!(
                "conflicting native extern declarations for `{}`: {:?} vs {:?}",
                function.name, previous, signature
            );
        }
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Declare every document row in order, before building recursive or forward calls.
    pub(super) fn declare_final_functions(
        &mut self,
        functions: &NativeFinalFunctions,
        table: &molt_ir::FunctionTable,
    ) -> (
        Vec<cranelift_module::FuncId>,
        BTreeMap<String, NativeFunctionLinkageAbi>,
    ) {
        let mut rows = BTreeMap::new();
        let ids = table
            .ids()
            .map(|id| {
                let definition = table.defines(id);
                let name = table.symbol(id);
                let abi = functions.linkage(id, self.module_context.as_ref());
                rows.insert(name.to_owned(), abi.clone());
                let signature = super::super::codegen_builder::linkage_signature(&self.module, abi);
                let linkage = if definition {
                    Linkage::Export
                } else {
                    Linkage::Import
                };
                declare_compiled_execution_function(&mut self.module, name, linkage, &signature)
                    .unwrap_or_else(|error| match *error {
                        cranelift_module::ModuleError::IncompatibleSignature(
                            _,
                            prior,
                            incoming,
                        ) => panic!(
                            "incompatible machine declaration for `{name}`: {prior} vs {incoming}"
                        ),
                        error => panic!("Cranelift declaration for {name}: {error}"),
                    })
            })
            .collect();
        (ids, rows)
    }

    /// Give the shared walker a physical entry owned by the object or JIT module.
    /// Every logical argument follows the hidden execution context. A failed
    /// definition is dropped before it can enter the machine-code queue.
    pub(super) fn build_walker_definition(
        &mut self,
        id: cranelift_module::FuncId,
        definition: &NativeFinalFunction,
        table: &molt_ir::FunctionTable,
        linkage: &BTreeMap<String, NativeFunctionLinkageAbi>,
        runtime: &molt_ir::runtime_callable_symbols::RuntimeFnSet,
    ) -> Result<(), Vec<String>> {
        let func = Self::lower_cranelift_definition(
            &mut self.module,
            &mut self.import_ids,
            id,
            definition,
            table,
            linkage,
            runtime,
        )?;
        Self::emit_walker_metadata(&mut self.module, &definition.tir);
        self.queue_compiled_definition(id, &definition.tir.name, func);
        Ok(())
    }

    pub(super) fn lower_cranelift_definition(
        module: &mut dyn Module,
        import_ids: &mut RuntimeImportIds,
        id: cranelift_module::FuncId,
        definition: &NativeFinalFunction,
        table: &molt_ir::FunctionTable,
        linkage: &BTreeMap<String, NativeFunctionLinkageAbi>,
        runtime: &molt_ir::runtime_callable_symbols::RuntimeFnSet,
    ) -> Result<Function, Vec<String>> {
        use super::super::codegen_builder::NativeCodegen;
        let body = &definition.tir;
        let trace_start = env_setting(BackendEnv::MOLT_TRACE_COMPILE_FUNC)
            .as_deref()
            .is_some_and(parse_truthy_env)
            .then(std::time::Instant::now);
        if trace_start.is_some() {
            let ops = body.operation_count();
            let params = body.param_types.len();
            eprintln!(
                "[molt-native-compile] start {} ops={ops} params={params}",
                body.name
            );
            let _ = crate::debug_artifacts::append_debug_artifact(
                "native/compile_trace.txt",
                format!("start name={} ops={ops} params={params}\n", body.name),
            );
        }
        if let Some(filter) = env_setting(BackendEnv::MOLT_DUMP_FINAL_FUNC_IR)
            && body.name.contains(&filter)
        {
            let stem = super::deferred_codegen::debug_file_stem(&body.name);
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("native/final_ir/{stem}.txt"),
                crate::tir::printer::print_function(body),
            );
        }
        let mut function = Function::new();
        function.signature = module
            .declarations()
            .get_function_decl(id)
            .signature
            .clone();
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let (execution, logical) = builder
            .block_params(entry)
            .split_first()
            .expect("attached definition");
        let execution = *execution;
        let parameters = logical.to_vec();
        let mut imports = NativeFunctionImports::default();
        crate::codegen_ssa::walk::lower_function(
            &mut NativeCodegen {
                words: NativeWords {
                    builder: &mut builder,
                },
                module,
                import_ids,
                imports: &mut imports,
            },
            crate::codegen_ssa::walk::FunctionInput {
                func: body,
                plan: &definition.carriers,
                entry: &parameters,
                linkage,
                functions: table,
                runtime,
            },
        )?;
        inspect_native_cfg(&body.name, builder.func);
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        complete_attached_calls(module, &mut function, execution);
        if let Some(start) = trace_start {
            let elapsed = start.elapsed();
            eprintln!(
                "[molt-native-compile] done {} after {elapsed:.2?}",
                body.name
            );
            let _ = crate::debug_artifacts::append_debug_artifact(
                "native/compile_trace.txt",
                format!("done name={} elapsed={elapsed:.2?}\n", body.name),
            );
        }
        Ok(function)
    }

    /// A task constructor reads its poll's frame size before calling it.
    pub(super) fn emit_walker_metadata(module: &mut dyn Module, body: &crate::tir::TirFunction) {
        if let Some(crate::tir::ops::AttrValue::Int(size)) = body
            .attrs
            .get(crate::tir::lower_from_simple::STATEFUL_FRAME_SIZE_ATTR)
        {
            define_stateful_frame_size(module, &body.name, *size);
        }
    }

    pub fn compile(self, ir: SimpleIR) -> CompileOutput {
        self.compile_checked(ir)
            .unwrap_or_else(|error| panic!("native SimpleIR admission failed: {error}"))
    }

    pub fn compile_checked(
        self,
        ir: SimpleIR,
    ) -> Result<CompileOutput, molt_ir::ir_schema::FunctionOpShapeDiagnostic> {
        // Backend selection: MOLT_BACKEND=llvm is an explicit contract. A
        // missing LLVM feature must fail closed instead of substituting a
        // different backend and producing misleading validation evidence.
        #[cfg(feature = "llvm")]
        if self.llvm_target.is_wasm() {
            // LLVM is the one WebAssembly producer: the triple selects it.
            return self.compile_selected_checked(ir, NativeCodegenBackend::Llvm);
        }
        let backend_setting = env_setting(BackendEnv::MOLT_BACKEND);
        let backend = select_native_codegen_backend(backend_setting.as_deref());
        self.compile_selected_checked(ir, backend)
    }

    /// Compile directly through LLVM without mutating process-global backend
    /// selection. This is the canonical API for explicit multi-backend callers
    /// and parallel tests.
    #[cfg(feature = "llvm")]
    pub fn compile_llvm(self, ir: SimpleIR) -> CompileOutput {
        self.compile_llvm_checked(ir)
            .unwrap_or_else(|error| panic!("LLVM SimpleIR admission failed: {error}"))
    }

    #[cfg(feature = "llvm")]
    pub fn compile_llvm_checked(
        self,
        ir: SimpleIR,
    ) -> Result<CompileOutput, molt_ir::ir_schema::FunctionOpShapeDiagnostic> {
        self.compile_selected_checked(ir, NativeCodegenBackend::Llvm)
    }

    fn compile_selected_checked(
        self,
        ir: SimpleIR,
        backend: NativeCodegenBackend,
    ) -> Result<CompileOutput, molt_ir::ir_schema::FunctionOpShapeDiagnostic> {
        // The backend's first read of a program: resolve the callees of a
        // hand-built document; a decoded one passes through (HF-847, D2).
        let mut ir = ir;
        ir.resolve_function_references()
            .unwrap_or_else(|error| panic!("native SimpleIR decode failed: {error}"));
        self.admit_program(&ir)?;
        Ok(self.compile_selected(ir, backend))
    }

    /// Admit a program's op shapes and program-slot custody before any pass
    /// runs. Every code owner admits the same way.
    pub(in crate::native_backend::simple_backend) fn admit_program(
        &self,
        ir: &SimpleIR,
    ) -> Result<(), molt_ir::ir_schema::FunctionOpShapeDiagnostic> {
        molt_ir::ir_schema::validate_simple_ir_op_shapes(ir)?;
        if let Some(bindings) = self.code_slots.as_ref() {
            bindings.validate_functions(&ir.functions)?;
        }
        if self.emit_app_callable_resolver
            || self.code_slots.is_some()
            || self.module_registry.is_some()
        {
            molt_ir::ir::validate_module_slots(self.module_registry.as_ref(), &ir.functions)?;
        }
        Ok(())
    }

    fn compile_selected(mut self, ir: SimpleIR, backend: NativeCodegenBackend) -> CompileOutput {
        let use_llvm = backend.uses_llvm();
        let timing = backend_timing_enabled();
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        let program = self.prepare_native_codegen(&mut ir, use_llvm, timing, &compile_start);
        //  LLVM backend dispatch
        // When MOLT_BACKEND=llvm and the llvm feature is compiled in, route
        // through the LLVM backend instead of Cranelift.  Each function is
        // lifted to TIR, lowered to LLVM IR, then the whole module is
        // optimized and emitted as a native object file.
        #[cfg(feature = "llvm")]
        if use_llvm {
            use crate::llvm_backend::{LlvmBackend, MoltOptLevel};

            let emit_resolver_here = program.emit_resolver_here;
            let app_callable_manifest = &program.app_callable_manifest;

            let llvm_target = self.llvm_target;
            let context = inkwell::context::Context::create();
            let mut llvm = LlvmBackend::new_for_target(&context, "molt_module", llvm_target);

            // Runtime functions are declared on first use by typed identity.

            let func_count = ir.functions.iter().filter(|f| !f.is_extern).count();
            let total_ops: usize = ir
                .functions
                .iter()
                .filter(|f| !f.is_extern)
                .map(|f| f.ops.len())
                .sum();
            eprintln!(
                "MOLT_BACKEND(llvm): compiling {func_count} functions ({total_ops} total ops)"
            );
            let codegen_start = std::time::Instant::now();

            // Unified cost model (Tier-0 S2) for the LLVM target, derived from
            // the host CPU's vector feature string so the vectorizer can size
            // lanes to the actual machine (behavior-neutral today: the width is
            // a dead annotation, but structurally correct for real SIMD codegen).
            let host_features = inkwell::targets::TargetMachine::get_host_cpu_features();
            let llvm_tti = crate::tir::target_info::TargetInfo::from_llvm_feature_string(
                if llvm_target.is_wasm() {
                    ""
                } else {
                    host_features.to_str().unwrap_or("")
                },
            );
            // Source ABI metadata survives transport retirement unchanged.
            let mut program_signatures = molt_ir::linkage_abi::document_signatures(&ir.functions);
            let mut parameter_custody: Vec<_> = ir
                .functions
                .iter()
                .map(|function| function.parameter_custody.clone())
                .collect();
            let mut llvm_cached_tir = crate::tir::pipeline_cache::run_cached_tir_pipeline(
                &ir.functions,
                crate::tir::pipeline_cache::TirPipelineRunOptions {
                    target_info: llvm_tti.clone(),
                    cache_flavor: crate::tir::pipeline_cache::TirPipelineCacheFlavor::Llvm,
                    cache: self.tir_cache.clone(),
                    process_externs: false,
                    tir_dump: env_setting(BackendEnv::TIR_DUMP).as_deref() == Some("1"),
                    tir_stats: env_setting(BackendEnv::TIR_OPT_STATS).as_deref() == Some("1"),
                    progress_prefix: Some("MOLT_BACKEND(llvm)"),
                    resource_plan: crate::tir::pipeline_cache::tir_optimization_resource_plan(),
                },
                preprocess_backend_tir_input,
            )
            .cached_tir;

            // Run the shared module transformations on the LLVM lane's owned
            // TIR, then lower those bodies directly to LLVM. Source SimpleIR
            // bodies retire during assembly; neither pipeline materializes a
            // SimpleIR projection.
            // The Cranelift module phase is skipped for `use_llvm`, so inlining
            // runs once under the LLVM target's cost model.
            //
            // Extern functions are runtime declarations with empty bodies (the
            // shared-stdlib partition's `stdlib_shared.o` symbols, already
            // externalized before this branch): they are not inlinable and stay
            // OUT of the module, so calls to them remain opaque call-graph edges
            // (exactly correct  an extern body is not owned by this object). They
            // are re-declared below for call resolution. Because externalization
            // has already physically removed their bodies, the module the inliner
            // sees contains only locally-owned bodies, so the `non_inlinable` set
            // is empty here (the native lane needs it only because its module
            // phase runs *before* externalization).
            let non_inlinable = molt_ir::deterministic_hash::HashSet::default();
            // A guarded call names a row of this document (a body, or the
            // extern row of a function another object defines): the row
            // supplies its signature, by id (HF-847 slice 1).
            let owned = crate::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
                ir.functions,
                &mut llvm_cached_tir,
                crate::tir::pipeline_cache::TirOwnedModulePipelineOptions {
                    target_info: &llvm_tti,
                    module_name: "llvm_module",
                    non_inlinable: &non_inlinable,
                    missing_tir_context: "LLVM TIR cache runner",
                    mode: if self.skip_ir_passes {
                        crate::tir::pipeline_cache::TirOwnedModulePipelineMode::TerminalDropsOnly
                    } else {
                        crate::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase
                    },
                    linkage: Some(&program_signatures),
                },
            );
            // A specialized clone is another entry of its origin (design 78,
            // slice 3): the origin's signature and entry custody.
            for clone in &owned.clones {
                assert_eq!(
                    clone.id.index(),
                    program_signatures.len(),
                    "specialized clone row"
                );
                program_signatures.push(program_signatures[clone.origin.index()]);
                parameter_custody.push(parameter_custody[clone.origin.index()].clone());
            }
            let tir_funcs = owned.functions;
            assert_eq!(
                tir_funcs.len(),
                program_signatures.len(),
                "final linkage rows"
            );
            super::program_pipeline::dump_rewritten_tir_if_requested(
                tir_funcs.iter().map(|(_, body)| body),
            );
            // The module phase returns document order. Each body contributes its
            // local ABI and carrier plan together, including specialized clones.
            for (((external, body), signature), custody) in tir_funcs
                .iter()
                .zip(program_signatures)
                .zip(parameter_custody)
            {
                if *external {
                    continue;
                }
                let facts = crate::representation_plan::ProducerCarrierPlan::build(body);
                llvm.function_linkage_abis.insert(
                    body.name.clone(),
                    facts.linkage_abi(body, signature, custody),
                );
                llvm.function_repr_facts.insert(body.name.clone(), facts);
            }
            // Independently compiled callers and providers share the frozen row.
            if let Some(module_context) = self.module_context.as_ref() {
                llvm.function_linkage_abis
                    .extend(module_context.function_linkage_abis.clone());
            }

            // `tir_funcs` is in document order: a position is a FunctionId.
            llvm.function_table =
                molt_ir::FunctionTable::from_rows(tir_funcs.iter().map(|(is_extern, function)| {
                    let kind = if *is_extern {
                        molt_ir::function_table::FunctionRowKind::Extern
                    } else {
                        molt_ir::function_table::FunctionRowKind::Body
                    };
                    (function.name.as_str(), kind)
                }));
            for (is_extern, tir_func) in &tir_funcs {
                if *is_extern {
                    crate::llvm_backend::lowering::declare_extern_tir_function(tir_func, &llvm);
                } else {
                    crate::llvm_backend::lowering::declare_tir_function(tir_func, &llvm);
                }
            }

            for (is_extern, tir_func) in tir_funcs {
                if is_extern {
                    continue;
                }
                if env_setting(BackendEnv::MOLT_TIR_DUMP).as_deref() == Some("1") {
                    eprintln!(
                        "[LLVM] TIR for '{}':\n{}",
                        tir_func.name,
                        crate::tir::printer::print_function(&tir_func)
                    );
                }
                crate::llvm_backend::lowering::try_lower_tir_to_llvm(&tir_func, &llvm)
                    .unwrap_or_else(|err| panic!("{err}"));
            }

            //  Per-app callable resolver
            // The LLVM-compiled application object must carry
            // `molt_app_resolve_callable` (referenced by the CLI's main stub and
            // registered with the runtime before `molt_runtime_init`) exactly
            // like the Cranelift object. Emitted into the LLVM module here,
            // after every function is lowered, so the callable manifest already
            // names declarations whose addresses the resolver table takes. Gated
            // on `emit_resolver_here` so batch/stdlib-cache LLVM objects never
            // emit a duplicate `_molt_app_resolve_callable` symbol.
            if emit_resolver_here {
                llvm.emit_app_resolver_function(app_callable_manifest);
            }
            //  Module registry blob (import bedrock)
            // Same single-object custody as the resolver: the main application
            // object carries the exported registry blob whose init-pointer
            // column is the MODULE_INIT_TABLE.
            if emit_resolver_here && let Some(registry) = self.module_registry.as_ref() {
                llvm.emit_module_registry_blob(registry);
            }
            if emit_resolver_here {
                llvm.emit_program_slot_bindings(
                    self.code_slots.as_ref(),
                    self.module_registry.as_ref(),
                );
            }
            // The application metadata owner carries the WASM entry, as it
            // carries the resolver and registry; stdlib-cache and batch
            // objects hold function bodies only.
            if llvm_target.is_wasm() && emit_resolver_here {
                if let Some(registry) = self.module_registry.as_ref() {
                    llvm.emit_wasm_isolate_import(registry);
                }
                llvm.emit_wasm_entry_wrapper(
                    self.module_registry.is_some(),
                    &app_callable_manifest.runtime_callables,
                );
                llvm.emit_wasm_attached_context_abi_section();
            }

            // Dump LLVM IR under the repo-local debug artifact root when
            // MOLT_LLVM_DUMP_IR=1. Each module (one per batch worker) gets
            // its own file, so parallel batches never overwrite each other.
            let dump_ir = env_setting(BackendEnv::MOLT_LLVM_DUMP_IR).as_deref() == Some("1");
            let write_ir_dump = |name: &str| {
                if let Ok(path) = crate::debug_artifacts::prepare_unique_debug_artifact_path(name) {
                    let _ = std::fs::write(path, llvm.dump_ir());
                }
            };
            if dump_ir {
                write_ir_dump("llvm/before_opt.ll");
            }

            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed before optimization:\n{}",
                    msg.to_string()
                )
            });

            llvm.optimize(MoltOptLevel::Aggressive)
                .unwrap_or_else(|err| panic!("{err}"));
            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed after optimization:\n{}",
                    msg.to_string()
                )
            });

            if dump_ir {
                write_ir_dump("llvm/after_opt.ll");
            }

            if timing {
                let codegen_elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: LLVM codegen + optimization took {codegen_elapsed:.2?}"
                );
            }

            let tmp_obj = crate::debug_artifacts::prepare_unique_debug_artifact_path(
                "llvm/molt_llvm_output.o",
            )
            .expect("failed to prepare LLVM object path");
            llvm.emit_object(&tmp_obj, MoltOptLevel::Aggressive)
                .expect("LLVM object emission failed");
            let bytes = std::fs::read(&tmp_obj).unwrap_or_else(|err| {
                panic!(
                    "failed to read LLVM object file at {}: {}",
                    tmp_obj.display(),
                    err
                )
            });
            let _ = std::fs::remove_file(&tmp_obj);

            if timing {
                let total_elapsed = compile_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: total LLVM backend compile: {total_elapsed:.2?}                      ({func_count} functions, {total_ops} ops, {} bytes)",
                    bytes.len()
                );
            }

            return CompileOutput { bytes };
        }
        let (func_count, total_ops) = self
            .emit_cranelift_program(program, timing)
            .unwrap_or_else(|errors| panic!("Cranelift lowering failed:\n{}", errors.join("\n")));

        let emit_start = std::time::Instant::now();
        let SimpleBackend { module, .. } = self;
        let product = finalize_native_object(module.into_object().finish());
        let bytes = product.emit().unwrap();
        if timing {
            let emit_elapsed = emit_start.elapsed();
            let total_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: object emit took {emit_elapsed:.2?}");
            eprintln!(
                "MOLT_BACKEND_TIMING: total backend compile: {total_elapsed:.2?} \
                 ({func_count} functions, {total_ops} ops, {} bytes)",
                bytes.len()
            );
        }
        CompileOutput { bytes }
    }

    /// Preserve the full program's type flow at preparation time, then omit
    /// terminal bodies whose canonical entries are already in the image.
    pub fn prepare_jit_application(
        &mut self,
        mut ir: SimpleIR,
        shared_stdlib: &BTreeSet<String>,
    ) -> Result<PreparedJitApplication, String> {
        self.skip_shared_stdlib_partition = true;
        self.admit_program(&ir).map_err(|error| error.to_string())?;
        let start = std::time::Instant::now();
        let program = self.prepare_native_codegen(&mut ir, false, backend_timing_enabled(), &start);
        let mut final_functions = program.final_functions.expect("prepared native bodies");
        final_functions.bind_shared_bodies(shared_stdlib);
        let registry = self
            .module_registry
            .take()
            .ok_or("missing JIT module registry")?;
        let linkage = self
            .module_context
            .take()
            .ok_or("missing JIT linkage context")?;
        let mut roots = program.app_callable_manifest.compiled_bodies.clone();
        roots.extend(registry.init_symbols.iter().cloned());
        final_functions.retain_reachable(&roots);
        final_functions.validate_extern_calls()?;
        final_functions.into_jit_artifact(registry, linkage, program.app_callable_manifest)
    }

    /// Run the shared program preparation for one native compilation and
    /// capture the facts codegen consumes after source bodies retire.
    pub(in crate::native_backend::simple_backend) fn prepare_native_codegen(
        &mut self,
        ir: &mut SimpleIR,
        use_llvm: bool,
        timing: bool,
        compile_start: &std::time::Instant,
    ) -> NativeCodegenProgram {
        // Validate declarations already present on input before any pass can
        // rewrite their callers. The post-preparation validation below also
        // covers declarations created by shared-stdlib externalization.
        crate::ir::validate_extern_call_abis(ir).unwrap_or_else(|error| panic!("native {error}"));
        let prepared = self.prepare_program_for_codegen(ir, use_llvm, timing, compile_start);
        let emit_resolver_here = prepared.emit_resolver_here;
        assert!(
            (self.code_slots.is_none() && self.module_registry.is_none()) || emit_resolver_here,
            "program-slot definitions require the application metadata owner"
        );
        let final_functions = prepared.final_functions;
        let final_ir = final_functions
            .as_ref()
            .map_or(&*ir, NativeFinalFunctions::ir);
        // Extern bodies are discarded by both native backends. Capture and
        // validate their complete target-neutral ABI first so no backend can
        // reconstruct a declaration from a caller after the authority is gone.
        validate_extern_function_signatures(&final_ir.functions);
        match &final_functions {
            Some(functions) => functions.validate_extern_calls(),
            None => crate::ir::validate_extern_call_abis(final_ir),
        }
        .unwrap_or_else(|error| panic!("native {error}"));
        if let Some(module_context) = self.module_context.as_ref() {
            module_context
                .validate_function_linkage_abis(&final_ir.functions)
                .unwrap_or_else(|error| panic!("invalid native module context: {error}"));
        }
        NativeCodegenProgram {
            emit_resolver_here,
            app_callable_manifest: prepared.app_callable_manifest,
            final_functions,
        }
    }

    /// Lower every final function into the code owner, then emit the program
    /// metadata (app resolver, module registry, program slots) and the
    /// generated-object ABI anchor. Returns (definitions, terminal operations including terminators).
    pub(in crate::native_backend::simple_backend) fn emit_cranelift_program(
        &mut self,
        program: NativeCodegenProgram,
        timing: bool,
    ) -> Result<(usize, usize), Vec<String>> {
        let NativeCodegenProgram {
            emit_resolver_here,
            app_callable_manifest,
            final_functions,
        } = program;
        let final_functions = final_functions.expect("Cranelift requires final function custody");
        let table = &molt_ir::FunctionTable::of(&final_functions.ir().functions);
        let (declarations, linkage) = self.declare_final_functions(&final_functions, table);
        let runtime =
            crate::runtime_callable_symbols::RuntimeCallableSymbols::capture().runtime_fn_set();
        let func_count = final_functions.bodies().count();
        let total_ops: usize = final_functions
            .bodies()
            .map(crate::tir::TirFunction::operation_count)
            .sum();
        eprintln!("MOLT_BACKEND: compiling {func_count} functions ({total_ops} terminal ops)");
        let codegen_start = std::time::Instant::now();
        let mut compiled = 0u32;
        let mut slowest_func: Option<(String, std::time::Duration)> = None;
        // Progress reporting: pick interval based on function count so the
        // user sees roughly 20 updates during a long build, but at least
        // every 50 functions.
        let progress_interval = (func_count / 20).clamp(1, 50);
        let mut last_progress = std::time::Instant::now();
        let mut deferred_codegen_ops = 0usize;

        for (id, function) in final_functions.into_local_functions() {
            let func_name = table.symbol(id).to_owned();
            let func_op_count = function.tir.operation_count();
            debug_assert_eq!(
                self.module
                    .declarations()
                    .get_function_decl(declarations[id.index()])
                    .name
                    .as_deref(),
                Some(compiled_execution_symbol(&func_name).as_str())
            );
            let func_start = std::time::Instant::now();
            self.build_walker_definition(
                declarations[id.index()],
                &function,
                table,
                &linkage,
                &runtime,
            )?;
            let func_elapsed = func_start.elapsed();
            if timing && func_elapsed.as_millis() > 500 {
                eprintln!("MOLT_BACKEND_TIMING: function `{func_name}` took {func_elapsed:.2?}");
            }
            if slowest_func.as_ref().is_none_or(|(_, d)| func_elapsed > *d) {
                slowest_func = Some((func_name, func_elapsed));
            }
            deferred_codegen_ops = deferred_codegen_ops.saturating_add(func_op_count);
            if should_flush_deferred_codegen(self.deferred_defines.len(), deferred_codegen_ops) {
                let deferred_count = self.deferred_defines.len();
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: bounded Cranelift flush ({deferred_count} functions, {deferred_codegen_ops} terminal ops) took {flush_elapsed:.2?}"
                    );
                }
                deferred_codegen_ops = 0;
            }
            compiled += 1;
            // Print progress at regular intervals, or every 500ms for
            // slow builds where individual functions take a long time.
            if (compiled as usize).is_multiple_of(progress_interval)
                || last_progress.elapsed().as_millis() >= 500
            {
                let pct = (compiled as f64 / func_count as f64 * 100.0) as u32;
                let elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: [{pct:3}%] compiled {compiled}/{func_count} functions ({elapsed:.1?} elapsed)"
                );
                last_progress = std::time::Instant::now();
            }
        }
        if timing {
            let codegen_elapsed = codegen_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: Cranelift codegen took {codegen_elapsed:.2?}");
            if let Some((name, dur)) = &slowest_func {
                eprintln!("MOLT_BACKEND_TIMING: slowest function: `{name}` ({dur:.2?})");
            }
        }
        //  Parallel Cranelift compilation
        // All functions were IR-built sequentially above (declarations
        // and Cranelift IR construction are not thread-safe), but actual
        // machine-code compilation (register allocation, instruction
        // selection, encoding) is deferred.  Flush them now in parallel.
        {
            let deferred_count = self.deferred_defines.len();
            if deferred_count > 0 {
                let deferred_ops = deferred_codegen_ops;
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: final Cranelift flush ({deferred_count} functions, {deferred_ops} terminal ops) took {flush_elapsed:.2?}"
                    );
                }
            }
        }
        //  Per-app callable resolver
        // Emit `molt_app_resolve_callable` AFTER the main flush so every runtime
        // callable FuncId created by a direct call already exists in the module
        // (reused via `get_name`); only manifest callables are address-taken
        // here. The main stub registers this resolver before `molt_runtime_init`,
        // so the runtime resolves dynamic callables through it instead of
        // monolithic staticlib resolvers, keeping wide resolver exports
        // native-unreachable for dead-stripping.
        //
        // Emit it ONCE per final binary, into the designated main application
        // object (`emit_app_callable_resolver`). Stdlib-cache batch objects and
        // all-but-one program batch set this `false`, so there is no duplicate
        // `_molt_app_resolve_callable` symbol at link. The threaded manifest
        // covers every name-resolved callable across all objects, including
        // stdlib wrappers compiled into the separate stdlib cache object.
        self.emit_cranelift_metadata(emit_resolver_here, &app_callable_manifest);
        //  Post-compilation: fail closed on declared-but-undefined exports.
        // These are always backend contract violations: either a call site
        // declared an impossible overload, a function was skipped, or codegen
        // failed to define a body.
        let mut undefined_exports = Vec::new();
        let declared: Vec<(String, cranelift_codegen::ir::Signature)> = self
            .module
            .declarations()
            .get_functions()
            .filter_map(|(_fid, decl)| {
                let name = decl.name.clone()?;
                if decl.linkage == cranelift_module::Linkage::Export
                    && !self.defined_func_names.contains(&name)
                {
                    Some((name, decl.signature.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (name, sig) in declared {
            // In batched compilation, functions that exist in another batch
            // are valid imports for the linker to resolve at merge time.
            if name
                .strip_suffix(ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX)
                .is_some_and(|logical| self.external_function_names.contains(logical))
            {
                self.module
                    .declare_function(&name, cranelift_module::Linkage::Import, &sig)
                    .unwrap_or_else(|err| {
                        panic!("failed to mark cross-batch function `{name}` as import: {err}")
                    });
                continue;
            }
            undefined_exports.push(name);
        }
        if !undefined_exports.is_empty() {
            undefined_exports.sort();
            panic!(
                "native backend left {} exported function declaration(s) undefined: {}",
                undefined_exports.len(),
                undefined_exports.join(", ")
            );
        }

        Ok((func_count, total_ops))
    }

    pub(super) fn emit_cranelift_metadata(
        &mut self,
        emit_resolver_here: bool,
        manifest: &crate::passes::AppCallableManifest,
    ) {
        if emit_resolver_here {
            self.emit_app_resolver_function(manifest);
            let registry = self.module_registry.take();
            let bindings = self.code_slots.take();
            if let Some(registry) = registry.as_ref() {
                self.emit_module_registry_blob(registry);
                if !self.external_program_storage {
                    self.emit_module_ready_binding(registry);
                }
            }
            if !self.external_program_storage {
                self.emit_program_slot_bindings(bindings.as_ref(), registry.as_ref());
            }
        }
        emit_generated_object_abi_anchor(&mut self.module);
    }
}

/// The facts one native compilation carries from program preparation into
/// codegen, for every code owner.
pub(in crate::native_backend::simple_backend) struct NativeCodegenProgram {
    pub(super) emit_resolver_here: bool,
    pub(super) app_callable_manifest: crate::passes::AppCallableManifest,
    pub(super) final_functions: Option<NativeFinalFunctions>,
}

/// `MOLT_BACKEND_TIMING` admission, shared by every code owner.
pub(in crate::native_backend::simple_backend) fn backend_timing_enabled() -> bool {
    env_setting(BackendEnv::MOLT_BACKEND_TIMING)
        .as_deref()
        .map(parse_truthy_env)
        .unwrap_or(false)
}
