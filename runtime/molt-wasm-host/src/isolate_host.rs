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
            log::debug!("env::molt_isolate_bootstrap -> app export");
            let func = caller
                .data()
                .isolate_bootstrap_export
                .as_ref()
                .cloned()
                .ok_or_else(|| {
                    wasmtime::Error::msg("molt_isolate_bootstrap export not registered")
                })?;
            let result = func.call(&mut caller, params, results);
            log::debug!("env::molt_isolate_bootstrap <- {result:?}");
            result
        },
    );
    linker.define(&mut *store, "env", "molt_isolate_bootstrap", bootstrap)?;

    let import_ty = FuncType::new(engine, [ValType::I64], [ValType::I64]);
    let import = Func::new(
        &mut *store,
        import_ty,
        |mut caller: Caller<'_, HostState>, params, results| {
            log::debug!("env::molt_isolate_import -> app export params={params:?}");
            let func = caller
                .data()
                .isolate_import_export
                .as_ref()
                .cloned()
                .ok_or_else(|| wasmtime::Error::msg("molt_isolate_import export not registered"))?;
            let result = func.call(&mut caller, params, results);
            log::debug!("env::molt_isolate_import <- {result:?} results={results:?}");
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
        .get_typed_func::<(), i64>(&mut *store, "molt_isolate_bootstrap")
        .context("missing or malformed molt_isolate_bootstrap export")?;
    let import = instance
        .get_typed_func::<i64, i64>(&mut *store, "molt_isolate_import")
        .context("missing or malformed molt_isolate_import export")?;
    // Publish only after both signatures match the runtime import ABI.
    let state = store.data_mut();
    state.isolate_bootstrap_export = Some(bootstrap.func().to_owned());
    state.isolate_import_export = Some(import.func().to_owned());
    Ok(())
}
