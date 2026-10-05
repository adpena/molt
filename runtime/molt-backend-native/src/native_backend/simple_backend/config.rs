use super::*;

#[cfg(feature = "native-backend")]
impl Default for SimpleBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(in crate::native_backend::simple_backend) fn cloned_shared_flags(
        flags: &settings::Flags,
        opt_level_override: Option<&str>,
    ) -> Result<settings::Flags, String> {
        let mut builder = settings::builder();
        for value in flags.iter() {
            let configured = if value.name == "opt_level" {
                opt_level_override
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.value_string())
            } else {
                value.value_string()
            };
            builder
                .set(value.name, &configured)
                .map_err(|err| format!("shared flag {}={configured:?}: {err}", value.name))?;
        }
        Ok(settings::Flags::new(builder))
    }

    pub(in crate::native_backend::simple_backend) fn rebuild_owned_isa(
        target_isa: &dyn isa::TargetIsa,
        opt_level_override: Option<&str>,
    ) -> Result<isa::OwnedTargetIsa, String> {
        let isa_builder = isa::Builder::from_target_isa(target_isa);
        let shared_flags = Self::cloned_shared_flags(target_isa.flags(), opt_level_override)?;
        isa_builder
            .finish(shared_flags)
            .map_err(|err| format!("TargetIsa finish: {err}"))
    }

    pub fn new() -> Self {
        Self::new_with_target(None)
    }

    pub fn new_with_target(target: Option<&str>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        // Cranelift optimization level: "none", "speed", or "speed_and_size".
        // Default to "speed" for production quality codegen.  Override with
        // MOLT_BACKEND_OPT_LEVEL=none for fast dev-loop compilation (~3-5x
        // faster compile times at the cost of ~30-50% slower generated code).
        let opt_level =
            env_setting("MOLT_BACKEND_OPT_LEVEL").unwrap_or_else(|| "speed".to_string());
        flag_builder
            .set("opt_level", &opt_level)
            .unwrap_or_else(|err| panic!("invalid MOLT_BACKEND_OPT_LEVEL={opt_level:?}: {err:?}"));
        let regalloc_algorithm =
            env_setting("MOLT_BACKEND_REGALLOC_ALGORITHM").unwrap_or_else(|| {
                // When opt_level=none, default to the fast single-pass
                // allocator regardless of build profile  the user has
                // explicitly asked for compile-time speed.
                if opt_level == "none" {
                    "single_pass".to_string()
                } else {
                    "backtracking".to_string()
                }
            });
        flag_builder
            .set("regalloc_algorithm", &regalloc_algorithm)
            .unwrap_or_else(|err| {
                panic!("invalid MOLT_BACKEND_REGALLOC_ALGORITHM={regalloc_algorithm:?}: {err:?}")
            });
        // Cranelift 0.128 adds explicit minimum function alignment tuning.
        // Default to 16-byte release alignment for better i-cache/branch
        // behavior on hot call-heavy kernels; keep debug/dev unchanged.
        let min_alignment_log2 = env_setting("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2")
            .unwrap_or_else(|| {
                if cfg!(debug_assertions) {
                    "0".to_string()
                } else {
                    "4".to_string()
                }
            });
        flag_builder
            .set("log2_min_function_alignment", &min_alignment_log2)
            .unwrap_or_else(|err| {
                panic!(
                    "invalid MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2={min_alignment_log2:?}: {err:?}"
                )
            });
        if let Some(libcall_call_conv) = env_setting("MOLT_BACKEND_LIBCALL_CALL_CONV") {
            flag_builder
                .set("libcall_call_conv", &libcall_call_conv)
                .unwrap_or_else(|err| {
                    panic!("invalid MOLT_BACKEND_LIBCALL_CALL_CONV={libcall_call_conv:?}: {err:?}")
                });
        }
        // Cranelift verifier catches IR invariant violations (type mismatches,
        // dominator tree bugs). Enable in debug builds; disable in release for
        // speed. Override with MOLT_BACKEND_ENABLE_VERIFIER=0|1.
        let default_enable_verifier = cfg!(debug_assertions);
        let enable_verifier = env_setting("MOLT_BACKEND_ENABLE_VERIFIER")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(default_enable_verifier);
        flag_builder
            .set(
                "enable_verifier",
                if enable_verifier { "true" } else { "false" },
            )
            .unwrap();
        // Cranelift alias analysis: enables redundant-load elimination across
        // memory operations within a basic block. Safe for our codegen because
        // we never emit raw pointer aliasing between different object fields.
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        // Emit CFG metadata in machine code output  enables downstream tools
        // and profilers to reconstruct control-flow graphs from compiled objects.
        flag_builder.set("machine_code_cfg_info", "true").unwrap();
        // Use colocated libcalls: our generated code and runtime libcalls live
        // in the same link unit  colocated calls skip GOT/PLT indirection and
        // use direct PC-relative calls instead.
        flag_builder.set("use_colocated_libcalls", "true").unwrap();
        // Detect whether we are targeting aarch64  either because we are
        // compiling natively on aarch64, or because an explicit cross-compile
        // target triple was supplied that contains "aarch64".
        let targeting_aarch64 = match target {
            Some(t) => t.contains("aarch64"),
            None => cfg!(target_arch = "aarch64"),
        };
        // Frame pointers: always preserve on aarch64 to ensure correct stack
        // frame layout for large functions (>16KB frames).  Cranelift 0.128 can
        // generate incorrect SP-relative accesses on aarch64 when frame pointers
        // are omitted and the frame exceeds the immediate offset range, leading
        // to SIGTRAP (exit 133) in generated code.  On x86_64 the cost is one
        // register (rbp); on aarch64 x29 is conventionally reserved anyway.
        // Debug builds always preserve for profiler/debugger support.
        flag_builder
            .set(
                "preserve_frame_pointers",
                if cfg!(debug_assertions) || targeting_aarch64 {
                    "true"
                } else {
                    "false"
                },
            )
            .unwrap();
        // Spectre mitigations: Molt compiles trusted user code (not sandboxed
        // plugins), so Spectre v1 heap/table mitigations add unnecessary overhead.
        flag_builder
            .set("enable_heap_access_spectre_mitigation", "false")
            .unwrap();
        flag_builder
            .set("enable_table_access_spectre_mitigation", "false")
            .unwrap();
        // Stack probing: guard pages detect stack overflow in large/recursive
        // frames instead of silently segfaulting. Cranelift 0.131 does not
        // implement stack probing on AArch64, so enabling it there is a
        // compile-time backend panic. AArch64 keeps frame pointers above; stack
        // probing is enabled only where the selected Cranelift target supports it.
        flag_builder
            .set(
                "enable_probestack",
                if targeting_aarch64 { "false" } else { "true" },
            )
            .unwrap();
        // On x86_64, inline probes are safe and faster for deep recursion.
        // When probing is disabled the strategy setting is inert.
        flag_builder
            .set(
                "probestack_strategy",
                if targeting_aarch64 {
                    "outline"
                } else {
                    "inline"
                },
            )
            .unwrap();
        // The standalone target baseline is portable. Host specialization is
        // explicit and participates in the codegen identity of every cache.
        let portable = crate::native_codegen_portable();
        let isa_builder = configured_native_isa_builder(target, portable);

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder = ObjectBuilder::new(
            isa,
            "molt_output",
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        // Emit each function into its own object section so the linker can
        // discard unreferenced runtime functions via -dead_strip / --gc-sections.
        builder.per_function_section(true);
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self {
            module,
            ctx,
            trampoline_ids: BTreeMap::new(),
            import_ids: BTreeMap::new(),
            skip_ir_passes: false,
            skip_shared_stdlib_partition: false,
            emit_app_callable_resolver: true,
            app_callable_manifest: None,
            external_function_names: std::collections::BTreeSet::new(),
            module_registry: None,
            module_context: None,
            partition_sources: BTreeMap::new(),
            data_pool: BTreeMap::new(),
            next_data_id: 0,
            declared_func_arities: BTreeMap::new(),
            defined_func_names: std::collections::BTreeSet::new(),
            deferred_defines: Vec::new(),
            function_entry_custody: BTreeMap::new(),
        }
    }

    /// Effective target inputs from the same ISA constructor used by codegen.
    /// TIR and object generation consume the same selected ISA capabilities.
    pub fn object_codegen_identity(target: Option<&str>) -> serde_json::Value {
        let backend = Self::new_with_target(target);
        let isa = backend.module.isa();
        let flags: BTreeMap<String, String> = isa
            .flags()
            .iter()
            .map(|value| (value.name.to_string(), value.value_string()))
            .collect();
        let isa_flags: BTreeMap<String, String> = isa
            .isa_flags()
            .into_iter()
            .map(|value| (value.name.to_string(), value.value_string()))
            .collect();
        let simd = backend.target_simd_caps();
        let identity = serde_json::json!({
            "target": isa.triple().to_string(),
            "shared_flags": flags,
            "isa_flags": isa_flags,
            "tir_simd": [simd.avx, simd.avx2, simd.avx512f],
            "backend": env_setting("MOLT_BACKEND"),
        });
        #[cfg(feature = "llvm")]
        let identity = {
            let mut identity = identity;
            // The LLVM target-machine owner projects its exact CPU policy;
            // baseline mode must not inherit ambient host-only features.
            use inkwell::targets::TargetMachine;
            let (cpu, features) = crate::llvm_backend::native_cpu_features();
            identity["llvm_host"] = serde_json::json!({
                "triple": TargetMachine::get_default_triple().as_str().to_str().unwrap(),
                "cpu": cpu,
                "features": features,
            });
            identity
        };
        identity
    }

    pub(in crate::native_backend::simple_backend) fn target_simd_caps(
        &self,
    ) -> crate::tir::target_info::SimdCaps {
        let enabled = |name: &str| {
            self.module
                .isa()
                .isa_flags()
                .into_iter()
                .any(|value| value.name == name && value.value_string() == "true")
        };
        crate::tir::target_info::SimdCaps {
            avx: enabled("has_avx"),
            avx2: enabled("has_avx2"),
            avx512f: enabled("has_avx512f"),
        }
    }

    pub fn prepare_module_context(functions: &mut Vec<FunctionIR>) -> NativeBackendModuleContext {
        NativeBackendModuleContext::from_functions(functions)
    }

    pub fn set_module_context(&mut self, context: NativeBackendModuleContext) {
        self.partition_sources
            .extend(context.partition_sources.clone());
        self.module_context = Some(context);
    }
}

/// Target identity is independent of whether the caller spelled the host
/// triple explicitly. Cranelift owns feature detection for every host target.
#[cfg(feature = "native-backend")]
fn configured_native_isa_builder(target: Option<&str>, portable: bool) -> isa::Builder {
    let requested = match target {
        Some(name) => name
            .parse::<target_lexicon::Triple>()
            .unwrap_or_else(|error| {
                panic!("target {name} is not supported: {error}");
            }),
        None => target_lexicon::Triple::host(),
    };
    let host = target_lexicon::Triple::host();
    let targets_host = requested.architecture == host.architecture
        && requested.operating_system == host.operating_system
        && requested.environment == host.environment
        && requested.binary_format == host.binary_format;
    let mut builder = isa::lookup(requested).unwrap_or_else(|error| {
        panic!("target is not supported: {error}");
    });
    if targets_host && !portable {
        cranelift_native::infer_native_flags(&mut builder).unwrap_or_else(|error| {
            panic!("host machine is not supported: {error}");
        });
    }
    builder
}

#[cfg(all(test, feature = "native-backend"))]
mod host_target_identity_tests {
    use super::*;

    #[test]
    fn portable_host_uses_only_the_target_baseline_flags() {
        let host = target_lexicon::Triple::host();
        let finish = |builder: isa::Builder| {
            builder
                .finish(settings::Flags::new(settings::builder()))
                .unwrap()
        };
        let baseline = finish(isa::lookup(host.clone()).unwrap());
        let portable = finish(configured_native_isa_builder(Some(&host.to_string()), true));
        let flags = |target: &dyn isa::TargetIsa| {
            target
                .isa_flags()
                .into_iter()
                .map(|value| (value.name.to_string(), value.value_string()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(flags(baseline.as_ref()), flags(portable.as_ref()));
    }

    #[test]
    fn explicit_host_and_implicit_host_have_identical_cpu_features() {
        let host = target_lexicon::Triple::host().to_string();
        for portable in [false, true] {
            let finish = |target| {
                configured_native_isa_builder(target, portable)
                    .finish(settings::Flags::new(settings::builder()))
                    .unwrap()
            };
            let implicit = finish(None);
            let explicit = finish(Some(host.as_str()));
            let flags = |isa: &dyn isa::TargetIsa| {
                isa.isa_flags()
                    .into_iter()
                    .map(|value| (value.name.to_string(), value.value_string()))
                    .collect::<BTreeMap<_, _>>()
            };
            assert_eq!(implicit.triple(), explicit.triple());
            assert_eq!(flags(implicit.as_ref()), flags(explicit.as_ref()));
        }
    }
}
