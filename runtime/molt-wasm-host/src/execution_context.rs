use super::*;
pub(super) use molt_ir::runtime_callable_abi_generated::{
    ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT, ATTACHED_EXECUTION_CONTEXT_ABI_SECTION,
    ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX, RuntimeExecutionContext, compiled_execution_symbol,
    runtime_execution_abi, runtime_execution_abi_for_symbol,
};

pub(super) fn runtime_symbol(logical: &str) -> Result<&'static str> {
    runtime_execution_abi(logical)
        .map(|abi| abi.physical_symbol)
        .with_context(|| format!("unclassified runtime execution ABI: {logical}"))
}

pub(super) fn validate_application_context_source(
    application: &ModuleSource,
    runtime: Option<&ModuleSource>,
) -> Result<()> {
    validate_context_section(application.bytes()).with_context(|| {
        format!(
            "validate application context schema {}",
            application.path().display()
        )
    })?;
    if let Some(runtime) = runtime {
        require_imported_context_memory(application.bytes(), "application").with_context(|| {
            format!(
                "validate application context memory {}",
                application.path().display()
            )
        })?;
        require_imported_context_memory(runtime.bytes(), "runtime").with_context(|| {
            format!(
                "validate runtime context memory {}",
                runtime.path().display()
            )
        })?;
    }
    Ok(())
}

fn validate_context_section(bytes: &[u8]) -> Result<()> {
    let mut seen = false;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::CustomSection(section) = payload?
            && section.name() == ATTACHED_EXECUTION_CONTEXT_ABI_SECTION
        {
            if seen || section.data() != ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT.to_le_bytes() {
                bail!(
                    "MOLT_ATTACHED_CONTEXT_ABI_MISMATCH: duplicate, malformed or stale application context schema"
                );
            }
            seen = true;
        }
    }
    if !seen {
        bail!("MOLT_ATTACHED_CONTEXT_ABI_MISMATCH: missing application context schema");
    }
    Ok(())
}

fn require_imported_context_memory(bytes: &[u8], role: &str) -> Result<()> {
    let mut memories = 0;
    let mut canonical = false;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload? {
            wasmparser::Payload::ImportSection(imports) => {
                for import in imports.into_imports() {
                    let import = import?;
                    if let wasmparser::TypeRef::Memory(memory) = import.ty {
                        memories += 1;
                        canonical =
                            import.module == "env" && import.name == "memory" && !memory.memory64;
                    }
                }
            }
            wasmparser::Payload::MemorySection(memories_defined) => {
                memories += memories_defined.count();
            }
            _ => {}
        }
    }
    if memories != 1 || !canonical {
        bail!(
            "MOLT_ATTACHED_CONTEXT_MEMORY_REQUIRED: split {role} requires exactly one imported env.memory"
        );
    }
    Ok(())
}

pub(super) fn caller_context(caller: &mut Caller<'_, HostState>) -> Result<i64> {
    let getter = caller
        .get_export(runtime_symbol("molt_attached_context")?)
        .and_then(Extern::into_func)
        .context("missing canonical molt_attached_context export")?
        .typed::<(), i64>(&*caller)?;
    let context = getter.call(&mut *caller, ())?;
    if context == 0 {
        bail!("runtime returned an empty attached execution context");
    }
    Ok(context)
}

/// A runtime callback selected by the generated ABI. Host services borrow the
/// calling guest's live context; they never mint a second runtime admission.
#[derive(Clone, Copy)]
pub(super) struct RuntimeFunction {
    function: Func,
    context: RuntimeExecutionContext,
}

impl RuntimeFunction {
    pub(super) fn resolve(
        caller: &mut Caller<'_, HostState>,
        logical: &str,
    ) -> Result<Option<Self>> {
        let abi = runtime_execution_abi(logical)
            .with_context(|| format!("unclassified runtime execution ABI: {logical}"))?;
        match caller.get_export(abi.physical_symbol) {
            None => Ok(None),
            Some(Extern::Func(function)) => Ok(Some(Self {
                function,
                context: abi.context,
            })),
            Some(_) => bail!(
                "malformed canonical runtime export {}: expected function",
                abi.physical_symbol
            ),
        }
    }

    pub(super) fn call(
        &self,
        caller: &mut Caller<'_, HostState>,
        args: &[Val],
        results: &mut [Val],
    ) -> Result<()> {
        if self.context == RuntimeExecutionContext::Boundary {
            return self.function.call(caller, args, results);
        }
        let context = caller_context(caller)?;
        let mut physical = Vec::with_capacity(args.len() + 1);
        physical.push(Val::I64(context));
        physical.extend_from_slice(args);
        self.function.call(caller, &physical, results)
    }
}

pub(super) fn validate_instance_context_memory(
    store: &mut Store<HostState>,
    instance: &Instance,
) -> Result<()> {
    let shared = store
        .data()
        .memory
        .context("MOLT_ATTACHED_CONTEXT_MEMORY_REQUIRED: missing shared imported memory")?;
    let memories: Vec<Memory> = instance
        .exports(&mut *store)
        .filter_map(|export| export.into_memory())
        .collect();
    for memory in memories {
        if memory.data_ptr(&*store) != shared.data_ptr(&*store)
            || memory.data_size(&*store) != shared.data_size(&*store)
        {
            bail!(
                "MOLT_ATTACHED_CONTEXT_MEMORY_REQUIRED: instance exported private context memory"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_runtime_imports_reject_obsolete_and_unregistered_identities() {
        let attached = compiled_execution_symbol("molt_exception_last");
        assert_eq!(
            runtime_execution_abi_for_symbol(&attached).unwrap().context,
            RuntimeExecutionContext::Attached
        );
        assert_eq!(
            runtime_execution_abi_for_symbol("molt_runtime_shutdown")
                .unwrap()
                .context,
            RuntimeExecutionContext::Boundary
        );
        for invalid in [
            "exception_last".to_string(),
            "molt_exception_last".to_string(),
            "molt_exception_last__molt_context_obsolete".to_string(),
            compiled_execution_symbol("molt_unknown_runtime_import"),
            compiled_execution_symbol("molt_runtime_shutdown"),
        ] {
            assert!(
                runtime_execution_abi_for_symbol(&invalid).is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn application_schema_rejects_absent_stale_and_duplicate_witnesses() {
        let valid = crate::execution_fixture::application_bytes("(module)");
        validate_context_section(&valid).unwrap();
        assert!(validate_context_section(&wat::parse_str("(module)").unwrap()).is_err());
        let mut stale = valid.clone();
        *stale.last_mut().unwrap() ^= 1;
        assert!(validate_context_section(&stale).is_err());
        let mut duplicate = valid.clone();
        duplicate.extend_from_slice(&valid[8..]);
        assert!(validate_context_section(&duplicate).is_err());
    }

    #[test]
    fn context_memory_requires_one_canonical_import_and_no_private_memory() {
        let shared = wat::parse_str("(module (import \"env\" \"memory\" (memory 1)))").unwrap();
        require_imported_context_memory(&shared, "application").unwrap();
        for invalid in [
            "(module)",
            "(module (memory 1))",
            "(module (import \"env\" \"private\" (memory 1)))",
            "(module (import \"other\" \"memory\" (memory 1)))",
            "(module (import \"env\" \"memory\" (memory 1)) (memory 1))",
            "(module (import \"env\" \"memory\" (memory 1)) (import \"env\" \"memory\" (memory 1)))",
        ] {
            assert!(
                require_imported_context_memory(&wat::parse_str(invalid).unwrap(), "application")
                    .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn exported_context_memory_must_be_the_actual_shared_import() {
        let engine = build_engine().unwrap();
        let mut store = Store::new(&engine, crate::main_tests::test_host_state());
        let shared = Memory::new(&mut store, MemoryType::new(1, None)).unwrap();
        store.data_mut().memory = Some(shared);
        let private = Module::new(&engine, "(module (memory (export \"memory\") 1))").unwrap();
        let private = Instance::new(&mut store, &private, &[]).unwrap();
        assert!(validate_instance_context_memory(&mut store, &private).is_err());
        let imported = Module::new(
            &engine,
            "(module (import \"env\" \"memory\" (memory 1)) (export \"memory\" (memory 0)))",
        )
        .unwrap();
        let imported = Instance::new(&mut store, &imported, &[shared.into()]).unwrap();
        validate_instance_context_memory(&mut store, &imported).unwrap();
    }

    #[test]
    fn split_application_reads_the_runtime_context_header_in_shared_memory() {
        let engine = build_engine().unwrap();
        let runtime_bytes = wat::parse_str(crate::execution_fixture::render(r#"(module
            (import "env" "memory" (memory 1))
            (export "memory" (memory 0))
            (func (export "molt_runtime_execution_enter") (result i64) i64.const 41)
            (func (export "molt_runtime_execution_leave") (param i64)
                local.get 0 i64.const 41 i64.ne if unreachable end)
            (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
            (func (export "molt_main_exit_status") (result i64) i64.const 0)
            (func (export "molt_exception_pending_detached") (result i64) i64.const 0)
            (func (export "molt_exception_pending@CONTEXT_SUFFIX@") (param $context i64) (result i64)
                local.get $context i64.const 32768 i64.ne if unreachable end
                local.get $context i32.wrap_i64 i64.load i64.const 49152 i64.ne if unreachable end
                i64.const 0)
            (func (export "molt_attached_context_abi_fingerprint") (result i64) i64.const @CONTEXT_FINGERPRINT@)
            (func (export "molt_attached_context") (result i64)
                i32.const 32768 i64.const 49152 i64.store i64.const 32768)
            (func (export "molt_compiled_execution_enter") (param i64) (result i64)
                local.get 0 i64.const 32768 i64.ne if unreachable end i64.const 17)
            (func (export "molt_compiled_execution_leave") (param i64 i64)
                local.get 0 i64.const 32768 i64.ne if unreachable end
                local.get 1 i64.const 17 i64.ne if unreachable end))"#)).unwrap();
        let application_bytes = crate::execution_fixture::application_bytes(
            r#"(module
            (import "env" "memory" (memory 1))
            (import "molt_runtime" "molt_exception_pending@CONTEXT_SUFFIX@" (func $pending (param i64) (result i64)))
            (export "memory" (memory 0))
            (func (export "molt_isolate_bootstrap@CONTEXT_SUFFIX@") (param i64) (result i64) i64.const 0)
            (func (export "molt_isolate_import@CONTEXT_SUFFIX@") (param i64 i64) (result i64) local.get 1)
            (func (export "molt_main@CONTEXT_SUFFIX@") (param $context i64) (result i64)
                local.get $context i64.const 32768 i64.ne if unreachable end
                local.get $context i32.wrap_i64 i64.load i64.const 49152 i64.ne if unreachable end
                local.get $context call $pending i64.eqz i32.eqz if unreachable end
                i64.const 0))"#,
        );
        validate_context_section(&application_bytes).unwrap();
        require_imported_context_memory(&application_bytes, "application").unwrap();
        require_imported_context_memory(&runtime_bytes, "runtime").unwrap();
        let application = Module::new(&engine, &application_bytes).unwrap();
        let runtime = Module::new(&engine, &runtime_bytes).unwrap();
        assert_eq!(
            execute_loaded_guest(
                &engine,
                &application,
                Some(&runtime),
                LoadedGuestOptions {
                    kind: LoadedGuestKind::MoltApplication { linked: false },
                    launch_environment: &[],
                    vfs_envs: &[],
                    guest_args: &[],
                }
            )
            .unwrap(),
            GuestTermination::Returned
        );
    }
}
