use super::*;

struct RuntimeImportBinding {
    import_name: String,
    export_name: String,
    export: wasmtime::ModuleExport,
}

/// The generated Molt runtime namespace is a function bridge, not arbitrary
/// Wasm linking: RuntimeImportRegistrar emits EntityType::Function exclusively.
/// Resolve each name and admit every import signature before either core start.
pub(super) struct RuntimeImportPlan {
    bindings: Vec<RuntimeImportBinding>,
}

impl RuntimeImportPlan {
    pub(super) fn new(application: &Module, runtime: Option<&Module>) -> Result<Self> {
        let mut bindings = std::collections::BTreeMap::new();
        for import in application.imports() {
            if import.module() != "molt_runtime" {
                continue;
            }
            let name = import.name();
            let ExternType::Func(expected) = import.ty() else {
                bail!("malformed molt_runtime::{name} import: expected a function");
            };
            let runtime = runtime.context("molt_runtime imports require a runtime module")?;
            let export_name = runtime_execution_abi_for_symbol(name)
                .with_context(|| format!("unclassified runtime execution ABI symbol: {name}"))?
                .physical_symbol
                .to_owned();
            let export_type = runtime
                .get_export(&export_name)
                .with_context(|| format!("missing runtime export {export_name}"))?;
            let ExternType::Func(actual) = export_type else {
                bail!("malformed runtime export {export_name}: expected a function");
            };
            if !actual.matches(&expected) {
                bail!(
                    "runtime export {export_name} signature mismatch for molt_runtime::{name}: expected {expected:?}, got {actual:?}"
                );
            }
            // Repeated imports may have different compatible supertypes. Admit
            // every occurrence, but publish its single definition only once.
            if !bindings.contains_key(name) {
                let export = runtime
                    .get_export_index(&export_name)
                    .context("admitted runtime export has no module index")?;
                bindings.insert(
                    name.to_string(),
                    RuntimeImportBinding {
                        import_name: name.to_string(),
                        export_name,
                        export,
                    },
                );
            }
        }
        Ok(Self {
            bindings: bindings.into_values().collect(),
        })
    }

    pub(super) fn bind(
        &self,
        linker: &mut Linker<HostState>,
        store: &mut Store<HostState>,
        runtime: &Instance,
    ) -> Result<()> {
        for binding in &self.bindings {
            let export = runtime
                .get_module_export(&mut *store, &binding.export)
                .with_context(|| {
                    format!(
                        "runtime instance lost planned export {}",
                        binding.export_name
                    )
                })?;
            linker.define(&mut *store, "molt_runtime", &binding.import_name, export)?;
        }
        Ok(())
    }
}

fn host_limits_satisfy(
    actual_min: u64,
    actual_max: Option<u64>,
    expected_min: u64,
    expected_max: Option<u64>,
) -> bool {
    actual_min >= expected_min
        && expected_max.is_none_or(|maximum| actual_max.is_some_and(|actual| actual <= maximum))
}

/// Admit only the concrete host definitions registered by execute_loaded_guest:
/// functions and its shared env memory/table. The runtime bridge is separately
/// planned above; no placeholder imports or additional guest instance is needed.
/// Wasmtime still owns final instantiation and the runtime's full import check.
pub(super) fn admit_application_host_imports(
    linker: &Linker<HostState>,
    store: &mut Store<HostState>,
    application: &Module,
) -> Result<()> {
    for import in application.imports() {
        if import.module() == "molt_runtime" {
            continue;
        }
        // A side module's placement imports (its bases, the shared shadow
        // stack and its GOT) exist only once the runtime has started;
        // `place_side_module` defines them and instantiation checks their types.
        if matches!(import.module(), "GOT.mem" | "GOT.func")
            || (import.module() == "env"
                && matches!(
                    import.name(),
                    "__stack_pointer" | "__memory_base" | "__table_base"
                ))
        {
            continue;
        }
        let name = format!("{}::{}", import.module(), import.name());
        let definition = linker
            .get(&mut *store, import.module(), import.name())
            .with_context(|| format!("missing application host import {name}"))?;
        let expected = import.ty();
        let actual = definition.ty(&*store);
        let compatible = match (&actual, &expected) {
            (ExternType::Func(actual), ExternType::Func(expected)) => actual.matches(expected),
            (ExternType::Memory(actual), ExternType::Memory(expected)) => {
                actual.is_64() == expected.is_64()
                    && actual.is_shared() == expected.is_shared()
                    && actual.page_size_log2() == expected.page_size_log2()
                    && host_limits_satisfy(
                        actual.minimum(),
                        actual.maximum(),
                        expected.minimum(),
                        expected.maximum(),
                    )
            }
            (ExternType::Table(actual), ExternType::Table(expected)) => {
                actual.is_64() == expected.is_64()
                    && wasmtime::RefType::eq(actual.element(), expected.element())
                    && host_limits_satisfy(
                        actual.minimum(),
                        actual.maximum(),
                        expected.minimum(),
                        expected.maximum(),
                    )
            }
            _ => false,
        };
        if !compatible {
            bail!(
                "application host import {name} type mismatch: expected {expected:?}, got {actual:?}"
            );
        }
    }
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

/// A raw WASI command has no deployment manifest, so it may not import the
/// Molt runtime or any Molt host service.
pub(super) fn admit_command_imports(output: &Module, kind: LoadedGuestKind<'_>) -> Result<()> {
    match kind {
        LoadedGuestKind::WasiCommandManifest(command)
            if !std::ptr::eq(output, command.module()) =>
        {
            bail!("manifest-bound WASI command requires its captured module");
        }
        LoadedGuestKind::WasiCommand => {
            for import in output.imports() {
                let (namespace, name) = (import.module(), import.name());
                if matches!(namespace, "molt_runtime" | "molt_native")
                    || (namespace == "env" && name.starts_with("molt_"))
                {
                    bail!(
                        "Molt host imports require a current deployment manifest; use manifest.json instead of --wasi-command: {namespace}.{name}"
                    );
                }
            }
        }
        LoadedGuestKind::WasiCommandManifest(_) | LoadedGuestKind::MoltApplication { .. } => {}
    }
    Ok(())
}

pub(super) fn has_runtime_imports(module: &Module) -> bool {
    module
        .imports()
        .any(|import| import.module() == "molt_runtime")
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

pub(super) fn map_io_error(err: &std::io::Error) -> GuestErrno {
    guest_errno::from_io_error(err)
}

pub(super) fn last_socket_error() -> GuestErrno {
    #[cfg(unix)]
    {
        map_io_error(&std::io::Error::last_os_error())
    }
    #[cfg(windows)]
    {
        guest_errno::from_native_errno(unsafe { winsock::WSAGetLastError() })
    }
}

pub(super) fn define_resource_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let on_allocate = Func::wrap(&mut *store, |size: i32| -> i32 {
        use molt_runtime_resource as resource;
        match resource::charge_allocation(size as usize) {
            Ok(()) => 0, // allocation permitted
            Err(_) => 1, // allocation denied
        }
    });
    let on_free = Func::wrap(&mut *store, |size: i32| {
        use molt_runtime_resource as resource;
        resource::release_allocation(size as usize);
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
