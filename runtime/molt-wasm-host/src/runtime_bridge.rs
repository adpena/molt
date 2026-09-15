use super::*;

/// One immutable export family, published only after complete ABI admission.
pub(super) type IndirectRegistry = Arc<std::sync::OnceLock<HashMap<String, Func>>>;

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
            let export_name = format!("molt_{name}");
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
    log::debug!("setting wasm table base to {base}");
    let mut results = alloc_results(&func.ty(&*store), "molt_set_wasm_table_base")?;
    func.call(&mut *store, &[Val::I64(base as i64)], &mut results)
        .context("call molt_set_wasm_table_base")?;
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

pub(super) fn plan_call_indirect_imports(
    output: &Module,
    runtime: Option<&Module>,
    is_wasi_command: bool,
) -> Result<Vec<(String, FuncType)>> {
    let mut imports = std::collections::BTreeMap::<String, FuncType>::new();
    for module in std::iter::once(output).chain(runtime) {
        for import in module.imports() {
            let name = import.name();
            if import.module() != "env" || !name.starts_with("molt_call_indirect") {
                continue;
            }
            let ExternType::Func(ty) = import.ty() else {
                bail!("malformed {name} import: expected a function");
            };
            validate_call_indirect_type(name, &ty)?;
            if let Some(previous) = imports.get(name) {
                require_indirect_signature(name, &ty, previous)?;
            } else {
                imports.insert(name.to_string(), ty);
            }
        }
    }
    if is_wasi_command {
        if !imports.is_empty() {
            validate_command_indirect_table(output)?;
        }
    } else {
        for (name, expected) in &imports {
            let export = output
                .exports()
                .find(|export| export.name() == name)
                .with_context(|| format!("missing indirect-call application export {name}"))?;
            let ExternType::Func(actual) = export.ty() else {
                bail!("malformed indirect-call application export {name}: expected a function");
            };
            require_indirect_signature(name, &actual, expected)?;
        }
    }
    Ok(imports.into_iter().collect())
}

fn require_indirect_signature(name: &str, actual: &FuncType, expected: &FuncType) -> Result<()> {
    // Wasmtime fast-paths matching interned types and preserves compatible
    // function subtypes. A dynamic
    // Func::call checks result values only after the callee has already run.
    // The ABI must reject mismatched results before any guest side effect.
    if !actual.matches(expected) {
        bail!(
            "{name} signature mismatch: expected params={:?} results={:?}, got params={:?} results={:?}",
            expected.params().collect::<Vec<_>>(),
            expected.results().collect::<Vec<_>>(),
            actual.params().collect::<Vec<_>>(),
            actual.results().collect::<Vec<_>>()
        );
    }
    Ok(())
}

pub(super) fn has_runtime_imports(module: &Module) -> bool {
    module
        .imports()
        .any(|import| import.module() == "molt_runtime")
}

fn validate_call_indirect_type(name: &str, ty: &FuncType) -> Result<()> {
    let suffix = name.strip_prefix("molt_call_indirect").unwrap_or("");
    let arity = suffix
        .parse::<usize>()
        .with_context(|| format!("malformed indirect-call import name: {name}"))?;
    if suffix != arity.to_string()
        || arity.checked_add(1) != Some(ty.params().len())
        || ty.results().len() != 1
        || !ty
            .params()
            .chain(ty.results())
            .all(|value| ValType::eq(&value, &ValType::I64))
    {
        bail!("malformed {name} import: expected (table index i64, {arity} i64 arguments) -> i64");
    }
    Ok(())
}

fn validate_command_indirect_table(module: &Module) -> Result<()> {
    let export = module
        .exports()
        .find(|export| export.name() == "__indirect_function_table")
        .context("WASI command using Molt indirect calls must export __indirect_function_table; link the command with --export-table")?;
    let ExternType::Table(table) = export.ty() else {
        bail!("WASI command __indirect_function_table export must be a table");
    };
    if table.is_64() || !wasmtime::RefType::eq(table.element(), &wasmtime::RefType::FUNCREF) {
        bail!("WASI command indirect dispatch requires a wasm32 funcref table");
    }
    Ok(())
}

/// Bind the same host import ABI to its explicit application or command owner.
/// Applications forward to generated wrappers; standalone Rust/WASI commands
/// dispatch raw wasm32 function pointers through their own exported table.
pub(super) fn define_call_indirect_imports(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
    imports: &[(String, FuncType)],
    is_wasi_command: bool,
) -> Result<()> {
    let registry = store.data().call_indirect.clone();
    for (name, ty) in imports {
        let target = if is_wasi_command {
            IndirectDispatch::CommandTable {
                callee_type: FuncType::new(ty.engine(), ty.params().skip(1), ty.results()),
            }
        } else {
            IndirectDispatch::ApplicationExports(registry.clone())
        };
        let func = make_call_indirect_func(&mut *store, name.clone(), ty.clone(), target);
        linker.define(&mut *store, "env", name, func)?;
    }
    Ok(())
}

enum IndirectDispatch {
    ApplicationExports(IndirectRegistry),
    CommandTable { callee_type: FuncType },
}

fn make_call_indirect_func(
    store: &mut Store<HostState>,
    name: String,
    ty: FuncType,
    target: IndirectDispatch,
) -> Func {
    Func::new(
        store,
        ty,
        move |mut caller, params, results| match &target {
            IndirectDispatch::CommandTable { callee_type } => {
                let index = params[0]
                    .i64()
                    .ok_or_else(|| wasmtime::Error::msg("indirect-call index is not i64"))?;
                let index = u32::try_from(index).map_err(|_| {
                    wasmtime::Error::msg(format!("{name} table index does not fit wasm32: {index}"))
                })?;
                let table = caller
                    .get_export("__indirect_function_table")
                    .and_then(Extern::into_table)
                    .ok_or_else(|| {
                        wasmtime::Error::msg("WASI command indirect-call table is unavailable")
                    })?;
                let entry = table.get(&mut caller, u64::from(index)).ok_or_else(|| {
                    wasmtime::Error::msg(format!("{name} table index out of bounds: {index}"))
                })?;
                let Ref::Func(Some(func)) = entry else {
                    wasmtime::bail!("{name} table index {index} is null or not a function");
                };
                // The guest may mutate its table between calls. Revalidate the
                // selected function, not a cached signature for the table slot.
                require_indirect_signature(&name, &func.ty(&caller), callee_type)
                    .with_context(|| format!("dispatch {name} at command table index {index}"))?;
                func.call(&mut caller, &params[1..], results)
                    .with_context(|| format!("dispatch {name} at command table index {index}"))
            }
            IndirectDispatch::ApplicationExports(registry) => {
                let func = registry
                    .get()
                    .and_then(|exports| exports.get(&name))
                    .copied();
                let Some(func) = func else {
                    return Err(wasmtime::Error::msg(format!(
                        "{name} used before output instantiation"
                    )));
                };
                func.call(&mut caller, params, results)
            }
        },
    )
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
        use molt_runtime_resource as resource;
        match resource::with_tracker(|t| t.on_allocate(size as usize)) {
            Ok(()) => 0, // allocation permitted
            Err(_) => 1, // allocation denied
        }
    });
    let on_free = Func::wrap(&mut *store, |size: i32| {
        use molt_runtime_resource as resource;
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
    registry: &IndirectRegistry,
    imports: &[(String, FuncType)],
) -> Result<()> {
    let mut admitted = Vec::with_capacity(imports.len());
    for (name, expected) in imports {
        let func = instance
            .get_func(&mut *store, name)
            .with_context(|| format!("missing export {name}"))?;
        require_indirect_signature(name, &func.ty(&*store), expected)?;
        admitted.push((name.clone(), func));
    }
    // Publish as one transaction only after every immutable export is admitted.
    // Application dispatch then needs no per-call lock, type validation or
    // nullable entry layered on top of the already-optional map lookup.
    registry
        .set(admitted.into_iter().collect())
        .map_err(|_| wasmtime::Error::msg("indirect-call application exports already registered"))
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

#[cfg(test)]
mod indirect_dispatch_tests {
    use super::*;

    fn execute_command(wat: &str) -> Result<GuestTermination> {
        let engine = build_engine().expect("production host engine");
        let module = Module::new(&engine, wat)?;
        execute_loaded_guest(
            &engine,
            &module,
            None,
            LoadedGuestOptions {
                kind: LoadedGuestKind::WasiCommand,
                vfs_envs: &[],
                guest_args: &[],
                wasm_table_base: None,
            },
        )
    }

    #[test]
    fn commands_dispatch_actual_table_functions_across_arity_and_start_phase() {
        for arity in [0, 1, 2, 13] {
            let parameters = " i64".repeat(arity);
            let target_parameters = if arity == 0 {
                String::new()
            } else {
                format!("(param{parameters})")
            };
            let sum = (0..arity)
                .map(|index| format!("local.get {index} i64.add "))
                .collect::<String>();
            let arguments = (1..=arity)
                .map(|argument| format!("i64.const {argument} "))
                .collect::<String>();
            let expected = 10 + arity * (arity + 1) / 2;
            for core_start in [false, true] {
                let start = if core_start { "(start $run)" } else { "" };
                let wat = format!(
                    r#"(module
                        (import "env" "molt_call_indirect{arity}"
                          (func $call (param i64{parameters}) (result i64)))
                        (table (export "__indirect_function_table") 2 funcref)
                        (func $target {target_parameters} (result i64) i64.const 10 {sum})
                        (elem (i32.const 1) $target)
                        (func $run (export "_start")
                          i64.const 1 {arguments} call $call
                          i64.const {expected} i64.ne if unreachable end)
                        {start})"#
                );
                assert_eq!(
                    execute_command(&wat).unwrap(),
                    GuestTermination::Returned,
                    "arity={arity} core_start={core_start}"
                );
            }
        }
    }

    #[test]
    fn command_indirect_admission_rejects_missing_or_wrong_table_before_start() {
        for (table, diagnostic) in [
            ("(table 2 funcref)", "must export __indirect_function_table"),
            (
                "(memory (export \"__indirect_function_table\") 1)",
                "export must be a table",
            ),
            (
                "(table (export \"__indirect_function_table\") 2 externref)",
                "requires a wasm32 funcref table",
            ),
        ] {
            let wat = format!(
                r#"(module
                    (import "env" "molt_call_indirect0" (func (param i64) (result i64)))
                    {table}
                    (func $start unreachable) (start $start)
                    (func (export "_start")))"#
            );
            let error = execute_command(&wat).unwrap_err();
            assert!(format!("{error:#}").contains(diagnostic), "{error:#}");
        }
    }

    #[test]
    fn command_indirect_dispatch_rejects_bad_indices_nulls_and_signature_mismatch() {
        for (index, target, diagnostic) in [
            ("-1", "(result i64) i64.const 0", "does not fit wasm32"),
            (
                "4294967296",
                "(result i64) i64.const 0",
                "does not fit wasm32",
            ),
            ("2", "(result i64) i64.const 0", "out of bounds"),
            ("0", "(result i64) i64.const 0", "is null or not a function"),
            ("1", "(result i32) unreachable", "signature mismatch"),
            (
                "1",
                "(param i64) (result i64) unreachable",
                "signature mismatch",
            ),
            ("1", "unreachable", "signature mismatch"),
            ("1", "(result i64 i64) unreachable", "signature mismatch"),
        ] {
            let wat = format!(
                r#"(module
                    (import "env" "molt_call_indirect0" (func $call (param i64) (result i64)))
                    (table (export "__indirect_function_table") 2 funcref)
                    (func $target {target}) (elem (i32.const 1) $target)
                    (func (export "_start") i64.const {index} call $call drop))"#
            );
            let error = execute_command(&wat).unwrap_err();
            assert!(format!("{error:#}").contains(diagnostic), "{error:#}");
            assert!(
                error.downcast_ref::<wasmtime::Trap>().is_none(),
                "ABI rejection must precede the mismatched target's unreachable: {error:#}"
            );
        }
    }

    #[test]
    fn command_indirect_dispatch_revalidates_mutated_tables_and_preserves_valid_traps() {
        for (bad_signature, expected_mismatch) in [("(result i32)", true), ("(result i64)", false)]
        {
            let wat = format!(
                r#"(module
                    (import "env" "molt_call_indirect0" (func $call (param i64) (result i64)))
                    (table (export "__indirect_function_table") 2 funcref)
                    (func $good (result i64) i64.const 42)
                    (func $bad {bad_signature} unreachable)
                    (elem (i32.const 1) $good)
                    (elem declare func $bad)
                    (func (export "_start")
                        i64.const 1 call $call i64.const 42 i64.ne if unreachable end
                        i32.const 1 ref.func $bad table.set
                        i64.const 1 call $call drop))"#
            );
            let error = execute_command(&wat).unwrap_err();
            assert!(format!("{error:#}").contains("dispatch molt_call_indirect0"));
            assert_eq!(
                format!("{error:#}").contains("signature mismatch"),
                expected_mismatch
            );
            assert_eq!(
                error.downcast_ref::<wasmtime::Trap>().is_none(),
                expected_mismatch
            );
        }
    }

    #[test]
    fn command_indirect_dispatch_accepts_compatible_function_subtypes() {
        let wat = r#"(module
            (type $base (sub (func (result i64))))
            (type $derived (sub $base (func (result i64))))
            (import "env" "molt_call_indirect0" (func $call (param i64) (result i64)))
            (table (export "__indirect_function_table") 2 funcref)
            (func $target (type $derived) i64.const 42)
            (elem (i32.const 1) $target)
            (func (export "_start")
                i64.const 1 call $call i64.const 42 i64.ne if unreachable end))"#;
        assert_eq!(execute_command(wat).unwrap(), GuestTermination::Returned);
    }

    #[test]
    fn application_indirect_abis_are_admitted_before_either_core_start() {
        let engine = build_engine().expect("production host engine");
        for linked in [true, false] {
            for wrapper in [
                "",
                "(memory (export \"molt_call_indirect0\") 1)",
                "(func (export \"molt_call_indirect0\") (param i64) (result i32) unreachable)",
                "(func (export \"molt_call_indirect0\") (result i64) unreachable)",
            ] {
                let application_import = if linked {
                    r#"(import "env" "molt_call_indirect0" (func (param i64) (result i64)))"#
                } else {
                    r#"(import "molt_runtime" "run" (func))"#
                };
                let application = Module::new(
                    &engine,
                    format!(
                        r#"(module
                        {application_import}
                        {wrapper}
                        (func $initialize unreachable) (start $initialize)
                        (func (export "molt_main") (result i64) i64.const 0)
                        (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
                        (func (export "molt_isolate_import") (param i64) (result i64) i64.const 0)
                        (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
                        (func (export "molt_runtime_execution_leave") (param i64))
                        (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
                        (func (export "molt_exception_pending") (result i64) i64.const 0))"#
                    ),
                )
                .unwrap();
                let runtime = (!linked).then(|| {
                    Module::new(
                        &engine,
                        r#"(module
                    (import "env" "molt_call_indirect0" (func (param i64) (result i64)))
                    (func $initialize unreachable) (start $initialize)
                    (func (export "molt_run"))
                    (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
                    (func (export "molt_runtime_execution_leave") (param i64))
                    (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
                    (func (export "molt_exception_pending") (result i64) i64.const 0))"#,
                    )
                    .unwrap()
                });
                let error = execute_loaded_guest(
                    &engine,
                    &application,
                    runtime.as_ref(),
                    LoadedGuestOptions {
                        kind: LoadedGuestKind::MoltApplication { linked },
                        vfs_envs: &[],
                        guest_args: &[],
                        wasm_table_base: None,
                    },
                )
                .unwrap_err();
                assert!(
                    format!("{error:#}").contains("molt_call_indirect0"),
                    "{error:#}"
                );
                assert!(
                    error.downcast_ref::<wasmtime::Trap>().is_none(),
                    "admission ran a core start: {error:#}"
                );
            }
        }
    }

    #[test]
    fn split_indirect_plan_unifies_both_consumers_and_registration_is_atomic() {
        let engine = build_engine().expect("production host engine");
        let runtime = Module::new(
            &engine,
            r#"(module
            (import "env" "molt_call_indirect1" (func (param i64 i64) (result i64))))"#,
        )
        .unwrap();
        let output = Module::new(
            &engine,
            r#"(module
            (import "env" "molt_call_indirect0" (func (param i64) (result i64)))
            (import "env" "molt_call_indirect1" (func (param i64 i64) (result i64)))
            (func (export "molt_call_indirect0") (param i64) (result i64) i64.const 0)
            (func (export "molt_call_indirect1") (param i64 i64) (result i64) i64.const 0))"#,
        )
        .unwrap();
        let plan = plan_call_indirect_imports(&output, Some(&runtime), false).unwrap();
        assert_eq!(
            plan.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["molt_call_indirect0", "molt_call_indirect1"]
        );
        let mut store = Store::new(&engine, crate::main_tests::test_host_state());
        let malformed = Module::new(
            &engine,
            r#"(module
            (func (export "molt_call_indirect0") (param i64) (result i64) i64.const 0)
            (func (export "molt_call_indirect1") (param i64 i64) (result i32) i32.const 0))"#,
        )
        .unwrap();
        let instance = Instance::new(&mut store, &malformed, &[]).unwrap();
        let registry = store.data().call_indirect.clone();
        let error =
            register_call_indirect_exports(&mut store, &instance, &registry, &plan).unwrap_err();
        assert!(format!("{error:#}").contains("molt_call_indirect1 signature mismatch"));
        assert!(
            registry.get().is_none(),
            "a rejected export family must publish no partial registry"
        );
        let valid = Module::new(
            &engine,
            r#"(module
            (func (export "molt_call_indirect0") (param i64) (result i64) i64.const 0)
            (func (export "molt_call_indirect1") (param i64 i64) (result i64) i64.const 0))"#,
        )
        .unwrap();
        let valid_instance = Instance::new(&mut store, &valid, &[]).unwrap();
        register_call_indirect_exports(&mut store, &valid_instance, &registry, &plan).unwrap();
        assert_eq!(registry.get().unwrap().len(), 2);
        let error = register_call_indirect_exports(&mut store, &valid_instance, &registry, &plan)
            .unwrap_err();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn non_function_indirect_import_is_rejected_before_core_start() {
        let error = execute_command(
            r#"(module
            (import "env" "molt_call_indirect0" (global i64))
            (func $initialize unreachable) (start $initialize)
            (func (export "_start")))"#,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("malformed molt_call_indirect0 import"));
        assert!(error.downcast_ref::<wasmtime::Trap>().is_none());
    }

    #[test]
    fn indirect_imports_validate_canonical_names_and_function_signatures() {
        for (name, signature) in [
            ("molt_call_indirect00", "(param i64) (result i64)"),
            ("molt_call_indirect0", "(param i32) (result i64)"),
            ("molt_call_indirect0", "(param i64) (result i32)"),
            ("molt_call_indirect1", "(param i64) (result i64)"),
            ("molt_call_indirectbogus", "(param i64) (result i64)"),
        ] {
            let wat = format!(
                r#"(module
                    (import "env" "{name}" (func {signature}))
                    (table (export "__indirect_function_table") 2 funcref)
                    (func $start unreachable) (start $start)
                    (func (export "_start")))"#
            );
            let error = execute_command(&wat).unwrap_err();
            assert!(format!("{error:#}").contains("malformed"), "{error:#}");
        }
    }

    #[test]
    fn applications_keep_generated_wrapper_dispatch_without_raw_table_requirement() {
        let engine = build_engine().expect("production host engine");
        let module = Module::new(
            &engine,
            r#"(module
                (import "env" "molt_call_indirect0" (func $call (param i64) (result i64)))
                (func (export "molt_call_indirect0") (param i64) (result i64) i64.const 42)
                (func (export "molt_main") (result i64)
                    i64.const 37 call $call i64.const 42 i64.ne if unreachable end
                    i64.const 0)
                (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
                (func (export "molt_isolate_import") (param i64) (result i64) i64.const 0)
                (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
                (func (export "molt_runtime_execution_leave") (param i64))
                (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
                (func (export "molt_exception_pending") (result i64) i64.const 0))"#,
        )
        .unwrap();
        assert_eq!(
            execute_loaded_guest(
                &engine,
                &module,
                None,
                LoadedGuestOptions {
                    kind: LoadedGuestKind::MoltApplication { linked: true },
                    vfs_envs: &[],
                    guest_args: &[],
                    wasm_table_base: None,
                },
            )
            .unwrap(),
            GuestTermination::Returned
        );
    }

    #[test]
    fn split_applications_dispatch_both_modules_through_one_export_family() {
        let engine = build_engine().expect("production host engine");
        let runtime = Module::new(
            &engine,
            r#"(module
            (import "env" "molt_call_indirect1" (func $call (param i64 i64) (result i64)))
            (func (export "molt_run") (result i64)
                i64.const 37 i64.const 5 call $call)
            (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
            (func (export "molt_runtime_execution_leave") (param i64))
            (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
            (func (export "molt_exception_pending") (result i64) i64.const 0))"#,
        )
        .unwrap();
        let application = Module::new(
            &engine,
            r#"(module
            (import "env" "molt_call_indirect0" (func $call (param i64) (result i64)))
            (import "molt_runtime" "run" (func $run (result i64)))
            (func (export "molt_call_indirect0") (param i64) (result i64) i64.const 42)
            (func (export "molt_call_indirect1") (param i64 i64) (result i64)
                local.get 0 local.get 1 i64.add)
            (func (export "molt_main") (result i64)
                i64.const 37 call $call i64.const 42 i64.ne if unreachable end
                call $run i64.const 42 i64.ne if unreachable end
                i64.const 0)
            (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
            (func (export "molt_isolate_import") (param i64) (result i64) i64.const 0))"#,
        )
        .unwrap();
        assert_eq!(
            execute_loaded_guest(
                &engine,
                &application,
                Some(&runtime),
                LoadedGuestOptions {
                    kind: LoadedGuestKind::MoltApplication { linked: false },
                    vfs_envs: &[],
                    guest_args: &[],
                    wasm_table_base: None,
                }
            )
            .unwrap(),
            GuestTermination::Returned
        );
    }
}
