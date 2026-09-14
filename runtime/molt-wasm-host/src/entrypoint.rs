use super::*;
use wasmtime_wasi::I32Exit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuestTermination {
    Returned,
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
    export_name: &'static str,
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
            // static_type[0] in the generated Molt WASM ABI.
            validate_exported_function_type(application, "molt_main", &[], &[ValType::I64])?;
            validate_exported_function_type(
                application,
                "molt_isolate_bootstrap",
                &[],
                &[ValType::I64],
            )?;
            validate_exported_function_type(
                application,
                "molt_isolate_import",
                &[ValType::I64],
                &[ValType::I64],
            )?;
            validate_exported_function_type(runtime, "molt_exception_pending", &[], &[ValType::I64])
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ApplicationStatusPhase {
    BeforeMoltMain,
    AfterMoltMain,
}

impl ApplicationStatusPhase {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeMoltMain => "before molt_main",
            Self::AfterMoltMain => "after molt_main",
        }
    }

    fn pending_diagnostic(self) -> &'static str {
        match self {
            Self::BeforeMoltMain => {
                "MOLT_APP_BOOTSTRAP_FAILED: pending runtime exception before molt_main"
            }
            Self::AfterMoltMain => {
                "MOLT_APP_BOOTSTRAP_FAILED: molt_main returned with a pending runtime exception"
            }
        }
    }
}

pub(super) fn admit_application_runtime_status(
    store: &mut Store<HostState>,
    runtime: &Instance,
    phase: ApplicationStatusPhase,
) -> Result<()> {
    let pending = runtime
        .get_typed_func::<(), i64>(&mut *store, "molt_exception_pending")
        .context("missing or malformed molt_exception_pending startup status export")?;
    if read_runtime_exception_pending(store, &pending, phase.label())? {
        bail!("{}", phase.pending_diagnostic());
    }
    Ok(())
}

fn call_molt_application_entrypoint(
    store: &mut Store<HostState>,
    application: &Instance,
    runtime: &Instance,
) -> Result<GuestTermination> {
    // The generated WASM ABI's static type 0 is () -> i64, and the canonical
    // entry wrapper is emitted with that type. The boxed result is deliberately
    // ignored here, preserving the production host's existing entry semantics.
    let main = application
        .get_typed_func::<(), i64>(&mut *store, "molt_main")
        .context("missing or malformed molt_main application entrypoint export")?;
    admit_application_runtime_status(store, runtime, ApplicationStatusPhase::BeforeMoltMain)?;
    log::debug!("calling molt_main");
    let _entry_result = main
        .call(&mut *store, ())
        .context("call molt_main application entrypoint")?;
    log::debug!("molt_main returned");
    admit_application_runtime_status(store, runtime, ApplicationStatusPhase::AfterMoltMain)?;
    Ok(GuestTermination::Returned)
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
        } => call_molt_application_entrypoint(store, application, runtime),
        GuestEntrypoint::WasiCommand { command } => call_wasi_command_entrypoint(store, command),
    }
}
