//! One linker/validator for executable LIR test bodies.
use crate::wasm::WasmBackend;
use crate::wasm::body::WasmBody;
use crate::wasm_data::WasmDataSegments;
use crate::wasm_function_types::WasmFunctionTypes;
use std::collections::BTreeMap;
use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    MemorySection, MemoryType, Module,
};

pub(super) fn executable_module(body: &WasmBody) -> Vec<u8> {
    let mut types = WasmFunctionTypes::new();
    let function_type = types.function(&body.param_types, &body.result_types);
    let mut imports = ImportSection::new();
    let mut import_indices = BTreeMap::new();
    for import in body.runtime_imports() {
        let index = import_indices.len() as u32;
        if let std::collections::btree_map::Entry::Vacant(entry) = import_indices.entry(import) {
            entry.insert(index);
            let (params, results) = import.machine_signature();
            let import_type = types.function(params, results);
            imports.import(
                "molt_runtime",
                import.name(),
                EntityType::Function(import_type),
            );
        }
    }
    let mut functions = FunctionSection::new();
    functions.function(function_type);
    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, import_indices.len() as u32);
    exports.export("memory", ExportKind::Memory, 0);
    let mut function = Function::new(body.locals.iter().map(|&ty| (1, ty)));
    let mut backend = WasmBackend::new();
    backend.data_segments = WasmDataSegments::new(64);
    let literal_out = backend.add_data_segment_mutable(false, &[0; 8]);
    body.emit_into(
        "lir_test_execution",
        &mut backend,
        0,
        false,
        literal_out,
        |import| import_indices[&import],
        &mut function,
    );
    let mut code = CodeSection::new();
    code.function(&function);
    // Keep the same data authority that emitted literal pointers. A zero-filled
    // memory without these segments cannot exercise constructor payloads.
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: u64::from(backend.data_segments.offset())
            .div_ceil(65_536)
            .max(1),
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&memories);
    module.section(&exports);
    module.section(&code);
    module.section(backend.data_segments.section());
    let bytes = module.finish();
    wasmparser::Validator::new()
        .validate_all(&bytes)
        .expect("valid executable LIR CFG and typed stack");
    bytes
}
