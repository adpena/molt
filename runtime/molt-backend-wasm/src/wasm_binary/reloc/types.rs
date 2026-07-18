use crate::wasm_table::{WasmCallableTableRole, WasmFunctionSymbol};

#[derive(Clone, Copy)]
pub(super) enum PendingReloc {
    Function {
        offset: u32,
        func_index: u32,
    },
    Type {
        offset: u32,
        type_index: u32,
    },
    DataAddr {
        offset: u32,
        segment_index: u32,
    },
    TableIndex {
        offset: u32,
        target: WasmFunctionSymbol,
        role: WasmCallableTableRole,
    },
}

#[derive(Clone, Copy)]
pub(super) struct RelocEntry {
    pub(super) ty: u8,
    pub(super) offset: u32,
    pub(super) index: u32,
    pub(super) addend: i32,
}

#[derive(Clone, Debug)]
pub(super) struct FunctionImport {
    pub(super) module: String,
    pub(super) name: String,
}
