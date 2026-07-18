use serde::{Serialize, Serializer, ser::SerializeTuple};

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct WasmLinkFacts {
    pub schema_version: u32,
    pub function_import_count: u32,
    pub defined_function_count: u32,
    pub code_body_count: u32,
    pub operator_count: u64,
    pub function_references: Vec<WasmFunctionReferences>,
    pub function_types: Vec<Option<WasmFunctionType>>,
    pub function_type_indices: Vec<u32>,
    pub root_function_indices: Vec<u32>,
    pub element_function_indices: Vec<u32>,
    pub declared_function_indices: Vec<u32>,
    pub active_element_segments: Vec<WasmActiveElementSegment>,
    pub active_function_elements: Vec<WasmActiveFunctionElement>,
    pub callable_table_entries: Vec<WasmCallableTableEntryFact>,
    pub callable_table_attestation_present: bool,
    pub callable_table_layout: Option<CallableTableLayout>,
    pub table_mutations: Vec<WasmTableMutation>,
    pub reachable_table_mutations: Vec<WasmTableMutation>,
    pub forbidden_callable_alias_exports: Vec<String>,
    pub dynamic_table_dispatch: bool,
    pub dynamic_dispatch_functions: Vec<u32>,
    pub reachable_dynamic_dispatch: bool,
    pub function_reference_dispatch_functions: Vec<u32>,
    pub reachable_function_reference_dispatch: bool,
    pub indirect_call_tables: Vec<u32>,
    pub reachable_indirect_call_tables: Vec<u32>,
    pub indirect_calls: Vec<WasmIndirectCall>,
    pub table_reads: Vec<WasmTableRead>,
    pub reachable_table_reads: Vec<WasmTableRead>,
    pub exported_table_indices: Vec<u32>,
    pub tables: Vec<WasmTableFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WasmFunctionType {
    pub type_index: u32,
    pub params: Vec<Vec<u8>>,
    pub results: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmCallableTableEntryFact {
    pub slot: u32,
    pub function_index: u32,
    pub type_index: u32,
    pub role: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WasmFunctionReferences {
    pub function_index: u32,
    pub direct_calls: Vec<u32>,
    pub ref_funcs: Vec<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WasmActiveFunctionElement {
    pub table_index: u32,
    pub slot: u32,
    pub function_index: u32,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct WasmActiveElementSegment {
    pub table_index: u32,
    pub base: u32,
    pub item_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WasmTableMutation {
    pub function_index: u32,
    pub operation: &'static str,
    pub table_index: u32,
    pub source_table_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WasmIndirectCall {
    pub function_index: u32,
    pub table_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WasmTableRead {
    pub function_index: u32,
    pub table_index: u32,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct WasmTableFact {
    pub table_index: u32,
    pub imported: bool,
    pub minimum: u64,
    pub maximum: Option<u64>,
    pub table64: bool,
    pub shared: bool,
    pub untyped_funcref: bool,
    pub encoded_element_type: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CallableTableLayout {
    pub fixed_prefix_base: u32,
    pub fixed_prefix_len: u32,
    pub finalized_app_base: u32,
    pub app_entry_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableTableArtifactRole {
    Monolithic,
    App,
    Runtime,
}

impl Serialize for WasmFunctionReferences {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(3)?;
        row.serialize_element(&self.function_index)?;
        row.serialize_element(&self.direct_calls)?;
        row.serialize_element(&self.ref_funcs)?;
        row.end()
    }
}

impl Serialize for WasmActiveFunctionElement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(3)?;
        row.serialize_element(&self.table_index)?;
        row.serialize_element(&self.slot)?;
        row.serialize_element(&self.function_index)?;
        row.end()
    }
}

impl Serialize for WasmCallableTableEntryFact {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(4)?;
        row.serialize_element(&self.slot)?;
        row.serialize_element(&self.function_index)?;
        row.serialize_element(&self.type_index)?;
        row.serialize_element(&self.role)?;
        row.end()
    }
}

impl Serialize for WasmTableMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(4)?;
        row.serialize_element(&self.function_index)?;
        row.serialize_element(&self.operation)?;
        row.serialize_element(&self.table_index)?;
        row.serialize_element(&self.source_table_index)?;
        row.end()
    }
}

impl Serialize for WasmIndirectCall {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(2)?;
        row.serialize_element(&self.function_index)?;
        row.serialize_element(&self.table_index)?;
        row.end()
    }
}

impl Serialize for WasmTableRead {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut row = serializer.serialize_tuple(2)?;
        row.serialize_element(&self.function_index)?;
        row.serialize_element(&self.table_index)?;
        row.end()
    }
}
