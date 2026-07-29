use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_i32_const;
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmFunctionSymbol {
    Defined {
        defined_func_index: u32,
    },
    RuntimeImport(WasmRuntimeImport),
    /// Stable ordinal among canonical `env` user-function imports. Runtime
    /// import stripping may change the absolute function index, so relocations
    /// must resolve these by import class/ordinal rather than a stale index.
    UserImport {
        user_import_ordinal: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmCallableTableRole {
    DirectCallable,
    Trampoline,
    AppCallableResolver,
    ResolverEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmCallableTableAddress {
    /// The shared runtime ABI prefix is deliberately stable across separately
    /// instantiated runtime and app modules. These addresses are ABI constants,
    /// not linker-owned function-table allocations.
    FixedSharedRuntimeAbi {
        finalized_app_base: u32,
    },
    Relocatable(WasmFunctionSymbol),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WasmCallableTableTarget {
    pub(crate) current_table_index: u32,
    pub(crate) address: WasmCallableTableAddress,
    pub(crate) role: WasmCallableTableRole,
}

impl WasmCallableTableTarget {
    pub(crate) fn with_role(&self, role: WasmCallableTableRole) -> Self {
        Self {
            current_table_index: self.current_table_index,
            address: self.address,
            role,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableRelocSite {
    pub(crate) defined_func_index: u32,
    pub(crate) offset_in_func: u32,
    pub(crate) target: WasmFunctionSymbol,
    pub(crate) role: WasmCallableTableRole,
}

#[derive(Default)]
pub(crate) struct WasmTableRelocations {
    relocs: Vec<TableRelocSite>,
}

impl WasmTableRelocations {
    pub(crate) fn relocs(&self) -> &[TableRelocSite] {
        &self.relocs
    }

    pub(crate) fn emit_i32(
        &mut self,
        reloc_enabled: bool,
        func_import_count: u32,
        owner_func_index: u32,
        func: &mut Function,
        target: &WasmCallableTableTarget,
    ) {
        if reloc_enabled {
            match &target.address {
                WasmCallableTableAddress::Relocatable(symbol) => {
                    let defined_func_index = owner_func_index
                        .checked_sub(func_import_count)
                        .expect(
                            "callable-table relocation can only be recorded for defined WASM function bodies",
                        );
                    let body_len = u32::try_from(func.byte_len())
                        .expect("callable-table relocation body length exceeds u32");
                    let offset_in_func = body_len
                        .checked_add(1)
                        .expect("callable-table relocation instruction offset overflow");
                    self.relocs.push(TableRelocSite {
                        defined_func_index,
                        offset_in_func,
                        target: *symbol,
                        role: target.role,
                    });
                }
                WasmCallableTableAddress::FixedSharedRuntimeAbi { finalized_app_base } => assert!(
                    target.current_table_index < *finalized_app_base,
                    "fixed shared-runtime callable address {} must be below finalized app base {}",
                    target.current_table_index,
                    finalized_app_base
                ),
            }
        }
        emit_i32_const(func, reloc_enabled, target.current_table_index as i32);
    }

    pub(crate) fn emit_i64(
        &mut self,
        reloc_enabled: bool,
        func_import_count: u32,
        owner_func_index: u32,
        func: &mut Function,
        target: &WasmCallableTableTarget,
    ) {
        self.emit_i32(
            reloc_enabled,
            func_import_count,
            owner_func_index,
            func,
            target,
        );
        func.instruction(&Instruction::I64ExtendI32U);
    }
}
