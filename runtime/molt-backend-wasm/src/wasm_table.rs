use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_i32_const;
use wasm_encoder::Instruction;

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
    /// Stable ordinal among `molt_native` callable imports in import-section
    /// order. Address-taken native initializers resolve by this identity so a
    /// relocatable link binds the table slot to the provider's definition.
    NativeCallableImport {
        native_import_ordinal: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmCallableTableRole {
    DirectCallable,
    Trampoline,
    AppCallableResolver,
    ResolverEntry,
}

/// A callable-table slot as compiled code names it: its index now, the
/// function a relocatable link rebinds it to, and the role of the reference.
/// Every slot belongs to the app; runtime callables are keys (HF-699 K1, K3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WasmCallableTableSlot {
    pub(crate) current_table_index: u32,
    pub(crate) symbol: WasmFunctionSymbol,
    pub(crate) role: WasmCallableTableRole,
}

/// The identity compiled code stores for a callable (HF-699 K5): an app
/// table slot, or a runtime key that the runtime resolves through its
/// generated table. Keys and slots are distinct types; a key is never a
/// table index, so no relocation or `i32` encoding applies to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmCallableTableTarget {
    TableSlot(WasmCallableTableSlot),
    RuntimeKey(u64),
}

impl WasmCallableTableSlot {
    pub(crate) fn with_role(&self, role: WasmCallableTableRole) -> Self {
        Self { role, ..*self }
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
        target: &WasmCallableTableSlot,
    ) {
        if reloc_enabled {
            let defined_func_index = owner_func_index.checked_sub(func_import_count).expect(
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
                target: target.symbol,
                role: target.role,
            });
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
        match target {
            WasmCallableTableTarget::TableSlot(slot) => {
                self.emit_i32(
                    reloc_enabled,
                    func_import_count,
                    owner_func_index,
                    func,
                    slot,
                );
                func.instruction(&Instruction::I64ExtendI32U);
            }
            // A key is a 64-bit constant above every table index (K5).
            WasmCallableTableTarget::RuntimeKey(key) => {
                func.instruction(&Instruction::I64Const(*key as i64));
            }
        }
    }
}
