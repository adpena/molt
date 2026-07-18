mod callable_table_generated;
mod encoding;
mod layout;
mod model;
mod publication;
mod scan;

use callable_table_generated::{
    CALLABLE_TABLE_LAYOUT_SECTION_NAME, CALLABLE_TABLE_LAYOUT_VERSION, CALLABLE_TABLE_SECTION_NAME,
    CALLABLE_TABLE_SECTION_VERSION, CALLABLE_TABLE_VALUE_TYPE_FORMAT,
};

pub use model::{
    CallableTableArtifactRole, CallableTableLayout, WasmActiveElementSegment,
    WasmActiveFunctionElement, WasmCallableTableEntryFact, WasmFunctionReferences,
    WasmFunctionType, WasmIndirectCall, WasmLinkFacts, WasmTableFact, WasmTableMutation,
    WasmTableRead,
};
pub use publication::{
    publish_callable_table_attestation, scan_and_write_callable_table_attestation,
};
pub use scan::scan_wasm_link_facts;

pub const WASM_LINK_FACTS_SCHEMA_VERSION: u32 = 4;

#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod scan_tests;
