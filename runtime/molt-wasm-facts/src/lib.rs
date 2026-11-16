mod execution_manifest_generated;
mod model;
mod scan;

pub use execution_manifest_generated::{
    WASM_EXECUTION_MANIFEST_REQUIRED_ENVIRONMENT, WASM_EXECUTION_MANIFEST_VERSION,
};

pub use model::{
    WasmActiveElementSegment, WasmActiveFunctionElement, WasmCanonicalExportTypeFact,
    WasmCanonicalExternType, WasmCanonicalImportTypeFact, WasmFunctionReferences, WasmFunctionType,
    WasmGotDataGlobalFact, WasmIndirectCall, WasmLinkFacts, WasmLinkingSymbolFact, WasmTableFact,
    WasmTableMutation, WasmTableRead,
};
pub use scan::scan_wasm_link_facts;

pub const WASM_LINK_FACTS_SCHEMA_VERSION: u32 = 9;

#[cfg(test)]
mod scan_tests;
