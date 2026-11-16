use super::*;
use wasmtime::{Global, GlobalType, Mutability};

/// The placement facts of a wasm32 side module (the tool-conventions
/// `dylink.0` memory info): the data size and alignment the host reserves at
/// `env.__memory_base`, and the table slots it reserves at `env.__table_base`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SideModuleLayout {
    pub(super) memory_size: u32,
    pub(super) memory_align_log2: u32,
    pub(super) table_size: u32,
    pub(super) table_align_log2: u32,
}

/// The layout of `bytes` when it is a side module, else None.
pub(super) fn side_module_layout(bytes: &[u8]) -> Result<Option<SideModuleLayout>> {
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let wasmparser::Payload::CustomSection(reader) = payload? else {
            continue;
        };
        let wasmparser::KnownCustom::Dylink0(dylink) = reader.as_known() else {
            continue;
        };
        let mut layout = None;
        for subsection in dylink {
            match subsection? {
                wasmparser::Dylink0Subsection::MemInfo(info) => {
                    layout = Some(SideModuleLayout {
                        memory_size: info.memory_size,
                        memory_align_log2: info.memory_alignment,
                        table_size: info.table_size,
                        table_align_log2: info.table_alignment,
                    });
                }
                wasmparser::Dylink0Subsection::Needed(needed) if !needed.is_empty() => {
                    bail!("a split application must not need other shared libraries: {needed:?}");
                }
                _ => {}
            }
        }
        return layout
            .map(Some)
            .context("side module dylink.0 section has no memory info");
    }
    Ok(None)
}

fn align_up(value: u64, align_log2: u32) -> Result<u64> {
    let align = 1u64
        .checked_shl(align_log2)
        .context("side module alignment exceeds 64 bits")?;
    Ok(value.div_ceil(align) * align)
}

/// Place a side module after the started runtime: grow the shared memory for
/// its data and the shared table for its functions, define its bases and
/// shadow stack, and resolve its GOT entries against the runtime's exports.
/// Pages the host grows are never handed out by the runtime allocator, which
/// only uses pages its own `memory.grow` returns.
pub(super) fn place_side_module(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
    application: &Module,
    runtime: &Instance,
    table: Table,
    layout: SideModuleLayout,
) -> Result<()> {
    let memory = store
        .data()
        .memory
        .context("a side module needs the shared runtime memory")?;
    let memory_base = if layout.memory_size == 0 {
        0
    } else {
        let align = 1u64 << layout.memory_align_log2.min(16);
        let pages = (u64::from(layout.memory_size) + align).div_ceil(65536);
        let previous = memory
            .grow(&mut *store, pages)
            .context("grow memory for side module data")?;
        align_up(previous * 65536, layout.memory_align_log2)?
    };
    let table_base = align_up(table.size(&*store), layout.table_align_log2)?;
    let needed = table_base + u64::from(layout.table_size);
    if needed > table.size(&*store) {
        let grow = needed - table.size(&*store);
        table
            .grow(&mut *store, grow, Ref::Func(None))
            .context("grow table for side module functions")?;
    }
    let i32_const = |store: &mut Store<HostState>, value: u64| -> Result<Global> {
        let value = i32::try_from(value).context("side module base exceeds wasm32")?;
        Global::new(
            store,
            GlobalType::new(ValType::I32, Mutability::Const),
            Val::I32(value),
        )
    };
    let memory_base = i32_const(store, memory_base)?;
    let table_base_global = i32_const(store, table_base)?;
    linker.define(&mut *store, "env", "__memory_base", memory_base)?;
    linker.define(&mut *store, "env", "__table_base", table_base_global)?;
    let stack_pointer = runtime
        .get_global(&mut *store, "__stack_pointer")
        .context("runtime module does not export its __stack_pointer")?;
    linker.define(&mut *store, "env", "__stack_pointer", stack_pointer)?;

    let mut slots: Option<HashMap<usize, u64>> = None;
    for import in application.imports() {
        let (module, name) = (import.module(), import.name());
        let address = match module {
            "GOT.mem" => {
                let global = runtime.get_global(&mut *store, name).with_context(|| {
                    format!("runtime module does not export data symbol {name}")
                })?;
                match global.get(&mut *store) {
                    Val::I32(address) => address,
                    other => bail!("runtime data symbol {name} is not an i32 address: {other:?}"),
                }
            }
            "GOT.func" => {
                let func = runtime
                    .get_func(&mut *store, name)
                    .with_context(|| format!("runtime module does not export function {name}"))?;
                let key = func.to_raw(&mut *store) as usize;
                let slots = slots.get_or_insert_with(|| {
                    let mut found = HashMap::new();
                    for slot in 0..table.size(&*store) {
                        if let Some(Ref::Func(Some(entry))) = table.get(&mut *store, slot) {
                            found
                                .entry(entry.to_raw(&mut *store) as usize)
                                .or_insert(slot);
                        }
                    }
                    found
                });
                let slot = match slots.get(&key) {
                    Some(&slot) => slot,
                    None => {
                        let slot = table
                            .grow(&mut *store, 1, Ref::Func(Some(func)))
                            .context("grow table for a side module function address")?;
                        slots.insert(key, slot);
                        slot
                    }
                };
                i32::try_from(slot).context("table slot exceeds wasm32")?
            }
            _ => continue,
        };
        let got = Global::new(
            &mut *store,
            GlobalType::new(ValType::I32, Mutability::Var),
            Val::I32(address),
        )?;
        linker.define(&mut *store, module, name, got)?;
    }
    Ok(())
}

/// Run a placed side module's data relocations and constructors once, before
/// any of its code runs.
pub(super) fn start_side_module(
    store: &mut Store<HostState>,
    application: &Instance,
) -> Result<()> {
    for name in ["__wasm_apply_data_relocs", "__wasm_call_ctors"] {
        if let Some(start) = application.get_func(&mut *store, name) {
            start
                .typed::<(), ()>(&*store)
                .with_context(|| format!("malformed side module {name}"))?
                .call(&mut *store, ())
                .with_context(|| format!("run side module {name}"))?;
        }
    }
    Ok(())
}
