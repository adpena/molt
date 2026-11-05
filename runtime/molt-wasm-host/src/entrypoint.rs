use super::*;
use wasmtime_wasi::I32Exit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuestTermination {
    Returned,
    ApplicationExit(i32),
    WasiExit(i32),
}

pub(super) enum WasiCommandResult<T> {
    Value(T),
    Exit(GuestTermination),
}

pub(super) fn classify_wasi_command_result<T>(
    result: wasmtime::Result<T>,
    context: &'static str,
) -> Result<WasiCommandResult<T>> {
    match result {
        Ok(value) => Ok(WasiCommandResult::Value(value)),
        Err(error) => {
            if let Some(exit) = error.downcast_ref::<I32Exit>() {
                return Ok(WasiCommandResult::Exit(GuestTermination::WasiExit(exit.0)));
            }
            Err(error).context(context)
        }
    }
}

pub(super) enum GuestEntrypoint<'a> {
    MoltApplication {
        application: &'a Instance,
        runtime: &'a Instance,
        lifetime: &'a mut MoltRuntimeLifetime,
    },
    WasiCommand {
        command: &'a Instance,
    },
}

pub(super) enum GuestModuleEntrypoint<'a> {
    MoltApplication {
        application: &'a Module,
        runtime: &'a Module,
    },
    WasiCommand {
        command: &'a Module,
    },
}

fn validate_exported_function_type(
    module: &Module,
    export_name: &str,
    expected_params: &[ValType],
    expected_results: &[ValType],
) -> Result<()> {
    let export = module
        .exports()
        .find(|export| export.name() == export_name)
        .with_context(|| format!("missing {export_name} guest entrypoint export"))?;
    let ExternType::Func(function_type) = export.ty() else {
        bail!("malformed {export_name} guest entrypoint export: expected function");
    };
    if !exact_value_types(function_type.params(), expected_params)
        || !exact_value_types(function_type.results(), expected_results)
    {
        bail!(
            "malformed {export_name} guest entrypoint export: expected params={expected_params:?} results={expected_results:?}"
        );
    }
    Ok(())
}

fn exact_value_types(actual: impl ExactSizeIterator<Item = ValType>, expected: &[ValType]) -> bool {
    actual.len() == expected.len()
        && actual
            .zip(expected)
            .all(|(actual, expected)| ValType::eq(&actual, expected))
}

pub(super) fn validate_guest_module_entrypoint(
    entrypoint: GuestModuleEntrypoint<'_>,
) -> Result<()> {
    match entrypoint {
        GuestModuleEntrypoint::MoltApplication {
            application,
            runtime,
        } => {
            validate_exported_function_type(
                application,
                &compiled_execution_symbol("molt_main"),
                &[ValType::I64],
                &[ValType::I64],
            )?;
            validate_exported_function_type(
                application,
                &compiled_execution_symbol("molt_isolate_bootstrap"),
                &[ValType::I64],
                &[ValType::I64],
            )?;
            validate_exported_function_type(
                application,
                &compiled_execution_symbol("molt_isolate_import"),
                &[ValType::I64, ValType::I64],
                &[ValType::I64],
            )?;
            validate_exported_function_type(
                runtime,
                runtime_symbol("molt_exception_pending_detached")?,
                &[],
                &[ValType::I64],
            )?;
            for export in [
                "molt_main_exit_status",
                "molt_runtime_execution_enter",
                "molt_runtime_shutdown",
                "molt_attached_context",
                "molt_attached_context_abi_fingerprint",
            ] {
                validate_exported_function_type(
                    runtime,
                    runtime_symbol(export)?,
                    &[],
                    &[ValType::I64],
                )?;
            }
            validate_exported_function_type(
                runtime,
                runtime_symbol("molt_runtime_execution_leave")?,
                &[ValType::I64],
                &[],
            )?;
            validate_exported_function_type(
                runtime,
                runtime_symbol("molt_compiled_execution_enter")?,
                &[ValType::I64],
                &[ValType::I64],
            )?;
            validate_exported_function_type(
                runtime,
                runtime_symbol("molt_compiled_execution_leave")?,
                &[ValType::I64, ValType::I64],
                &[],
            )
        }
        GuestModuleEntrypoint::WasiCommand { command } => {
            validate_exported_function_type(command, "_start", &[], &[])
        }
    }
}

fn read_runtime_exception_pending(
    store: &mut Store<HostState>,
    pending: &wasmtime::TypedFunc<(), i64>,
    phase: &'static str,
) -> Result<bool> {
    let status = pending
        .call(&mut *store, ())
        .with_context(|| format!("call molt_exception_pending {phase}"))?;
    match status {
        0 => Ok(false),
        1 => Ok(true),
        other => bail!(
            "malformed molt_exception_pending status {phase}: expected i64 raw 0 or 1, got {other}"
        ),
    }
}

fn admit_application_runtime_status(
    store: &mut Store<HostState>,
    runtime: &Instance,
) -> Result<()> {
    let pending = runtime
        .get_typed_func::<(), i64>(
            &mut *store,
            runtime_symbol("molt_exception_pending_detached")?,
        )
        .context("missing or malformed molt_exception_pending startup status export")?;
    if read_runtime_exception_pending(store, &pending, "before molt_main")? {
        bail!("MOLT_APP_BOOTSTRAP_FAILED: pending runtime exception before molt_main");
    }
    Ok(())
}

pub(super) fn call_molt_application_entrypoint(
    store: &mut Store<HostState>,
    application: &Instance,
    runtime: &Instance,
    context: i64,
) -> Result<GuestTermination> {
    // The host owns the execution scope; the application receives its actual
    // context before the logical Python arguments and returns one boxed word.
    let main = application
        .get_typed_func::<i64, i64>(&mut *store, &compiled_execution_symbol("molt_main"))
        .context("missing or malformed molt_main application entrypoint export")?;
    admit_application_runtime_status(store, runtime)?;
    let report_exit = runtime
        .get_typed_func::<(), i64>(&mut *store, runtime_symbol("molt_main_exit_status")?)
        .context("missing or malformed molt_main_exit_status application status export")?;
    log::debug!("calling molt_main");
    let _entry_result = main
        .call(&mut *store, context)
        .context("call molt_main application entrypoint")?;
    log::debug!("molt_main returned");
    // The runtime owns uncaught-exception reporting, SystemExit and exception
    // consumption. Report once while the execution lease remains entered.
    let raw_status = report_exit
        .call(&mut *store, ())
        .context("call molt_main_exit_status application reporter")?;
    let status = u32::try_from(raw_status).with_context(|| {
        format!("malformed molt_main_exit_status status: expected i64 raw u32, got {raw_status}")
    })? as i32;
    Ok(if status == 0 {
        GuestTermination::Returned
    } else {
        GuestTermination::ApplicationExit(status)
    })
}

fn call_wasi_command_entrypoint(
    store: &mut Store<HostState>,
    command: &Instance,
) -> Result<GuestTermination> {
    let start = command
        .get_typed_func::<(), ()>(&mut *store, "_start")
        .context("missing or malformed _start WASI command entrypoint export")?;
    log::debug!("calling WASI command _start");
    match classify_wasi_command_result(
        start.call(&mut *store, ()),
        "call _start WASI command entrypoint",
    )? {
        WasiCommandResult::Value(()) => {
            log::debug!("WASI command _start returned");
            Ok(GuestTermination::Returned)
        }
        WasiCommandResult::Exit(termination) => Ok(termination),
    }
}

pub(super) fn call_guest_entrypoint(
    store: &mut Store<HostState>,
    entrypoint: GuestEntrypoint<'_>,
) -> Result<GuestTermination> {
    match entrypoint {
        GuestEntrypoint::MoltApplication {
            application,
            runtime,
            lifetime,
        } => lifetime.execute(store, |store, context| {
            call_molt_application_entrypoint(store, application, runtime, context)
        }),
        GuestEntrypoint::WasiCommand { command } => call_wasi_command_entrypoint(store, command),
    }
}

/// Finite host ownership begins at runtime instantiation, before fallible setup.
/// Execution only borrows this owner; finalization consumes it exactly once.
#[must_use = "finish the runtime lifetime before releasing its host resources"]
pub(super) struct MoltRuntimeLifetime {
    enter: wasmtime::TypedFunc<(), i64>,
    leave: wasmtime::TypedFunc<i64, ()>,
    shutdown: wasmtime::TypedFunc<(), i64>,
    pending: wasmtime::TypedFunc<(), i64>,
    context: wasmtime::TypedFunc<(), i64>,
    compiled_enter: wasmtime::TypedFunc<i64, i64>,
    compiled_leave: wasmtime::TypedFunc<(i64, i64), ()>,
    schema: ContextSchemaAdmission,
    entered: bool,
}

enum ContextSchemaAdmission {
    Offered(wasmtime::TypedFunc<(), i64>),
    Admitted,
}

impl MoltRuntimeLifetime {
    pub(super) fn admit(store: &mut Store<HostState>, runtime: &Instance) -> Result<Self> {
        // Module admission proved these types before instantiation. Resolve
        // them without calling guest code, then publish this cleanup owner.
        Ok(Self {
            enter: runtime
                .get_typed_func::<(), i64>(
                    &mut *store,
                    runtime_symbol("molt_runtime_execution_enter")?,
                )
                .context("missing or malformed runtime execution enter export")?,
            leave: runtime
                .get_typed_func::<i64, ()>(
                    &mut *store,
                    runtime_symbol("molt_runtime_execution_leave")?,
                )
                .context("missing or malformed runtime execution leave export")?,
            shutdown: runtime
                .get_typed_func::<(), i64>(&mut *store, runtime_symbol("molt_runtime_shutdown")?)
                .context("missing or malformed runtime shutdown export")?,
            pending: runtime
                .get_typed_func::<(), i64>(
                    &mut *store,
                    runtime_symbol("molt_exception_pending_detached")?,
                )
                .context("missing or malformed runtime pending exception export")?,
            context: runtime
                .get_typed_func::<(), i64>(&mut *store, runtime_symbol("molt_attached_context")?)?,
            compiled_enter: runtime.get_typed_func::<i64, i64>(
                &mut *store,
                runtime_symbol("molt_compiled_execution_enter")?,
            )?,
            compiled_leave: runtime.get_typed_func::<(i64, i64), ()>(
                &mut *store,
                runtime_symbol("molt_compiled_execution_leave")?,
            )?,
            schema: ContextSchemaAdmission::Offered(runtime.get_typed_func::<(), i64>(
                &mut *store,
                runtime_symbol("molt_attached_context_abi_fingerprint")?,
            )?),
            entered: false,
        })
    }

    pub(super) fn validate_context_schema(&mut self, store: &mut Store<HostState>) -> Result<()> {
        if let ContextSchemaAdmission::Offered(fingerprint) = &self.schema {
            let offered = fingerprint.call(&mut *store, ())? as u64;
            if offered != ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT {
                bail!(
                    "MOLT_ATTACHED_CONTEXT_ABI_MISMATCH: runtime fingerprint differs from application schema"
                );
            }
            self.schema = ContextSchemaAdmission::Admitted;
        }
        Ok(())
    }

    pub(super) fn execute<T>(
        &mut self,
        store: &mut Store<HostState>,
        operation: impl FnOnce(&mut Store<HostState>, i64) -> Result<T>,
    ) -> Result<T> {
        self.validate_context_schema(store)?;
        let token = self
            .enter
            .call(&mut *store, ())
            .context("enter runtime execution")?;
        if token == 0 {
            bail!("runtime returned an empty execution-boundary token");
        }
        self.entered = true;
        let result = (|| {
            let context = self
                .context
                .call(&mut *store, ())
                .context("get attached execution context")?;
            let address =
                u32::try_from(context).context("attached context is outside wasm32 memory")?;
            let memory = store
                .data()
                .memory
                .context("attached execution context requires runtime memory")?;
            if address == 0
                || address as usize
                    % std::mem::align_of::<molt_codegen_abi::AttachedExecutionContextStorage>()
                    != 0
                || (address as usize)
                    .checked_add(std::mem::size_of::<
                        molt_codegen_abi::AttachedExecutionContextStorage,
                    >())
                    .is_none_or(|end| end > memory.data_size(&*store))
            {
                bail!("attached execution context header is outside runtime memory");
            }
            let scope = self
                .compiled_enter
                .call(&mut *store, context)
                .context("enter compiled execution scope")?;
            if scope == 0 {
                bail!("runtime returned an empty compiled execution scope");
            }
            let result = operation(store, context);
            let retired = self
                .compiled_leave
                .call(&mut *store, (context, scope))
                .context("leave compiled execution scope");
            preserve_application_and_cleanup_result(result, retired)
        })();
        let released = self
            .leave
            .call(&mut *store, token)
            .context("leave runtime execution");
        preserve_application_and_cleanup_result(result, released)
    }

    pub(super) fn finish<T>(self, store: &mut Store<HostState>, result: Result<T>) -> Result<T> {
        // This canonical Boundary observer neither initializes nor re-enters
        // the runtime, including after failed execution admission.
        let pending = read_runtime_exception_pending(
            store,
            &self.pending,
            "before runtime shutdown",
        )
        .and_then(|pending| {
            if pending {
                bail!(
                    "MOLT_APP_BOOTSTRAP_FAILED: pending runtime exception before runtime shutdown"
                );
            }
            Ok(())
        });
        let result = preserve_application_and_cleanup_result(result, pending);
        let finalized = self
            .shutdown
            .call(&mut *store, ())
            .and_then(|status| {
                if status == 1 || (status == 0 && !self.entered) {
                    Ok(())
                } else {
                    bail!("runtime shutdown did not complete: status={status}")
                }
            })
            .context("finalize Molt application runtime");
        preserve_application_and_cleanup_result(result, finalized)
    }
}

pub(super) fn preserve_application_and_cleanup_result<T>(
    result: Result<T>,
    cleanup: Result<()>,
) -> Result<T> {
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => {
            // Retain the primary typed cause (including Wasmtime traps) while
            // exposing the complete cleanup failure rather than masking either.
            Err(primary.context(format!("cleanup also failed: {cleanup:#}")))
        }
    }
}
