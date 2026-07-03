use super::*;

pub(super) fn define_isolate_host_imports(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
    engine: &Engine,
) -> Result<()> {
    let bootstrap_ty = FuncType::new(engine, [], [ValType::I64]);
    let bootstrap = Func::new(
        &mut *store,
        bootstrap_ty,
        |mut caller: Caller<'_, HostState>, params, results| {
            debug_log(|| "env::molt_isolate_bootstrap -> app export".to_string());
            let func = caller
                .data()
                .isolate_bootstrap_export
                .as_ref()
                .cloned()
                .ok_or_else(|| {
                    wasmtime::Error::msg("molt_isolate_bootstrap export not registered")
                })?;
            let result = func.call(&mut caller, params, results);
            debug_log(|| format!("env::molt_isolate_bootstrap <- {result:?}"));
            result
        },
    );
    linker.define(&mut *store, "env", "molt_isolate_bootstrap", bootstrap)?;

    let import_ty = FuncType::new(engine, [ValType::I64], [ValType::I64]);
    let import = Func::new(
        &mut *store,
        import_ty,
        |mut caller: Caller<'_, HostState>, params, results| {
            debug_log(|| format!("env::molt_isolate_import -> app export params={params:?}"));
            let func = caller
                .data()
                .isolate_import_export
                .as_ref()
                .cloned()
                .ok_or_else(|| wasmtime::Error::msg("molt_isolate_import export not registered"))?;
            let result = func.call(&mut caller, params, results);
            debug_log(|| format!("env::molt_isolate_import <- {result:?} results={results:?}"));
            result
        },
    );
    linker.define(&mut *store, "env", "molt_isolate_import", import)?;
    Ok(())
}

pub(super) fn register_isolate_exports(
    store: &mut Store<HostState>,
    instance: &Instance,
) -> Result<()> {
    let bootstrap = instance
        .get_func(&mut *store, "molt_isolate_bootstrap")
        .context("missing molt_isolate_bootstrap export")?;
    let import = instance
        .get_func(&mut *store, "molt_isolate_import")
        .context("missing molt_isolate_import export")?;
    let state = store.data_mut();
    state.isolate_bootstrap_export = Some(bootstrap);
    state.isolate_import_export = Some(import);
    Ok(())
}

fn call_zero_arg_export(
    store: &mut Store<HostState>,
    instance: &Instance,
    export_name: &'static str,
) -> Result<()> {
    let func = instance
        .get_func(&mut *store, export_name)
        .with_context(|| format!("missing {export_name} export"))?;
    debug_log(|| format!("calling {export_name}"));
    let mut results = alloc_results(&func.ty(&*store), export_name)?;
    func.call(&mut *store, &[], &mut results)
        .map_err(|err| anyhow::anyhow!("call {export_name}: {err}"))?;
    debug_log(|| format!("{export_name} returned"));
    Ok(())
}

pub(super) fn call_app_startup_entries(
    store: &mut Store<HostState>,
    instance: &Instance,
) -> Result<()> {
    // Normal execution has exactly one startup authority: the exported
    // molt_main wrapper. It owns runtime init, manifest install, table init, and
    // app entry execution. Host-export setup is routed through molt_host_init
    // in the JS/browser hosts; pre-calling raw isolate bootstrap here creates a
    // second initialization lane before the wrapper has run.
    call_zero_arg_export(store, instance, "molt_main")
}
