mod encoding;
mod layout;
mod model;
mod publication;
mod scan;

const CALLABLE_TABLE_SECTION_NAME: &str = "molt.callable_table";
const CALLABLE_TABLE_LAYOUT_SECTION_NAME: &str = "molt.callable_table.layout";
const CALLABLE_TABLE_SECTION_VERSION: u32 = 1;
const CALLABLE_TABLE_LAYOUT_VERSION: u32 = 1;
const CALLABLE_TABLE_VALUE_TYPE_FORMAT: u32 = 1;

pub use model::{
    CallableTableArtifactRole, CallableTableLayout, WasmActiveElementSegment,
    WasmActiveFunctionElement, WasmCallableTableEntryFact, WasmFunctionReferences,
    WasmFunctionType, WasmIndirectCall, WasmLinkFacts, WasmTableFact, WasmTableMutation,
};
pub use publication::{
    publish_callable_table_attestation, scan_and_write_callable_table_attestation,
};
pub use scan::scan_wasm_link_facts;

#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod scan_tests;
