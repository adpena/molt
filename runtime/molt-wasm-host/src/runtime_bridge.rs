use super::*;

pub(super) fn configure_wasm_table_base(
    store: &mut Store<HostState>,
    instance: &Instance,
    wasm_table_base: Option<u64>,
) -> Result<()> {
    let Some(base) = wasm_table_base else {
        return Ok(());
    };
    let Some(func) = instance.get_func(&mut *store, "molt_set_wasm_table_base") else {
        return Ok(());
    };
    debug_log(|| format!("setting wasm table base to {base}"));
    let mut results = alloc_results(&func.ty(&*store), "molt_set_wasm_table_base")?;
    func.call(&mut *store, &[Val::I64(base as i64)], &mut results)
        .map_err(|err| anyhow::anyhow!("call molt_set_wasm_table_base: {err}"))?;
    Ok(())
}

pub(super) fn merge_limits(
    left: Option<Limits>,
    right: Option<Limits>,
    label: &str,
) -> Result<Option<Limits>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(lim), None) | (None, Some(lim)) => Ok(Some(lim)),
        (Some(a), Some(b)) => {
            let min = a.min.max(b.min);
            let max = match (a.max, b.max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if let Some(max) = max
                && min > max
            {
                bail!("incompatible {label} limits: min {min} > max {max}");
            }
            Ok(Some(Limits { min, max }))
        }
    }
}

pub(super) fn memory_limits(module: &Module) -> Option<MemoryType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "memory" {
            return None;
        }
        match import.ty() {
            ExternType::Memory(mem) => Some(mem),
            _ => None,
        }
    })
}

pub(super) fn table_limits(module: &Module) -> Option<TableType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "__indirect_function_table" {
            return None;
        }
        match import.ty() {
            ExternType::Table(table) => Some(table),
            _ => None,
        }
    })
}

pub(super) fn collect_call_indirect_imports(module: &Module) -> Vec<(String, FuncType)> {
    module
        .imports()
        .filter_map(|import| {
            let name = import.name();
            if import.module() != "env" || !name.starts_with("molt_call_indirect") {
                return None;
            }
            let ty = match import.ty() {
                ExternType::Func(func) => func,
                _ => return None,
            };
            Some((name.to_string(), ty))
        })
        .collect()
}

pub(super) fn has_runtime_imports(module: &Module) -> bool {
    module
        .imports()
        .any(|import| import.module() == "molt_runtime")
}

pub(super) fn make_call_indirect_func(
    store: &mut Store<HostState>,
    name: String,
    ty: FuncType,
    registry: Arc<Mutex<HashMap<String, Option<Func>>>>,
) -> Func {
    Func::new(store, ty, move |mut caller, params, results| {
        let func = registry
            .lock()
            .ok()
            .and_then(|map| map.get(&name).cloned())
            .flatten();
        let Some(func) = func else {
            return Err(wasmtime::Error::msg(format!(
                "{name} used before output instantiation"
            )));
        };
        func.call(&mut caller, params, results)
    })
}

pub(super) fn box_int(value: u64) -> u64 {
    QNAN | TAG_INT | (value & INT_MASK)
}

pub(super) fn is_bool_bits(bits: u64) -> bool {
    (bits & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
}

pub(super) fn unbox_bool(bits: u64) -> bool {
    (bits & 1) == 1
}

pub(super) fn ensure_memory(caller: &mut Caller<HostState>) -> Result<Memory> {
    if let Some(mem) = caller.data().memory {
        return Ok(mem);
    }
    if let Some(mem) = caller
        .get_export("molt_memory")
        .and_then(Extern::into_memory)
    {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    if let Some(mem) = caller.get_export("memory").and_then(Extern::into_memory) {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    bail!("wasm memory not available");
}

pub(super) fn db_host_unavailable(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    out_ptr: usize,
) -> i32 {
    if out_ptr == 0 {
        return 2;
    }
    let bytes = 0u64.to_le_bytes();
    if memory.write(caller, out_ptr, &bytes).is_err() {
        return 2;
    }
    7
}

pub(super) fn read_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>> {
    if ptr == 0 || len <= 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len as usize];
    memory.read(caller, ptr as usize, &mut buf)?;
    Ok(buf)
}

pub(super) fn write_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    bytes: &[u8],
) -> Result<()> {
    if ptr == 0 {
        bail!("null pointer");
    }
    memory.write(caller, ptr as usize, bytes)?;
    Ok(())
}

pub(super) fn write_u32(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    val: u32,
) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

pub(super) fn write_u64(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    val: u64,
) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

pub(super) fn map_io_error(err: &std::io::Error) -> i32 {
    if let Some(code) = err.raw_os_error() {
        return code;
    }
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return libc::EWOULDBLOCK;
    }
    libc::EIO
}

pub(super) fn define_resource_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let on_allocate = Func::wrap(&mut *store, |size: i32| -> i32 {
        use molt_runtime::resource;
        match resource::with_tracker(|t| t.on_allocate(size as usize)) {
            Ok(()) => 0, // allocation permitted
            Err(_) => 1, // allocation denied
        }
    });
    let on_free = Func::wrap(&mut *store, |size: i32| {
        use molt_runtime::resource;
        resource::with_tracker(|t| t.on_free(size as usize));
    });
    linker.define(
        &mut *store,
        "env",
        "molt_resource_on_allocate_host",
        on_allocate,
    )?;
    linker.define(&mut *store, "env", "molt_resource_on_free_host", on_free)?;
    Ok(())
}

pub(super) fn set_memory_from_exports(store: &mut Store<HostState>, instance: &wasmtime::Instance) {
    if store.data().memory.is_some() {
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "molt_memory") {
        store.data_mut().memory = Some(mem);
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "memory") {
        store.data_mut().memory = Some(mem);
    }
}

pub(super) fn register_call_indirect_exports(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    registry: &Arc<Mutex<HashMap<String, Option<Func>>>>,
    names: &[String],
) -> Result<()> {
    let mut map = registry
        .lock()
        .map_err(|_| anyhow::anyhow!("call_indirect registry poisoned"))?;
    for name in names {
        let func = instance
            .get_func(&mut *store, name)
            .with_context(|| format!("missing export {name}"))?;
        map.insert(name.clone(), Some(func));
    }
    Ok(())
}

pub(super) fn alloc_results(ty: &FuncType, export_name: &str) -> Result<Vec<Val>> {
    let mut results = Vec::new();
    for val_ty in ty.results() {
        let Some(val) = Val::default_for_ty(&val_ty) else {
            bail!("unsupported {export_name} return type: {val_ty:?}");
        };
        results.push(val);
    }
    Ok(results)
}
