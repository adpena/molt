//! One linker/validator for executable LIR test bodies.
use crate::wasm::WasmBackend;
use crate::wasm::body::WasmBody;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_data::WasmDataSegments;
use crate::wasm_import_tracking::TrackedImportIds;
use std::collections::BTreeMap;
use wasm_encoder::{
    EntityType, ExportKind, ExportSection, Function, ImportSection, MemorySection, MemoryType,
    Module,
};

pub(super) fn executable_module(body: &WasmBody) -> Vec<u8> {
    let mut backend = WasmBackend::new();
    backend.data_segments = WasmDataSegments::new(64);
    let literal_out = backend.add_data_segment_mutable(false, &[0; 8]);
    // A body with RC sites calls the module's generated transition bodies,
    // which reach the runtime through the inc/dec imports.
    let refcount = !body.test_view().refcount_calls.is_empty();
    let transition_imports = refcount
        .then_some([WasmRuntimeImport::IncRefObj, WasmRuntimeImport::DecRefObj])
        .into_iter()
        .flatten();
    let mut imports = ImportSection::new();
    let mut import_indices = BTreeMap::new();
    for import in body.runtime_imports().chain(transition_imports) {
        let index = import_indices.len() as u32;
        if let std::collections::btree_map::Entry::Vacant(entry) = import_indices.entry(import) {
            entry.insert(index);
            let (params, results) = import.machine_signature();
            let import_type = backend.types.function(params, results);
            imports.import(
                "molt_runtime",
                import.name(),
                EntityType::Function(import_type),
            );
        }
    }
    backend.import_ids = TrackedImportIds::new(import_indices.clone());
    backend.func_count = import_indices.len() as u32;
    if refcount {
        crate::wasm::refcount::emit_refcount_bodies(&mut backend, false);
    }
    let run_index = backend.func_count;
    let function_type = backend
        .types
        .function(&body.param_types, &body.result_types);
    backend.funcs.function(function_type);
    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, run_index);
    exports.export("memory", ExportKind::Memory, 0);
    let mut function = Function::new(body.locals.iter().map(|&ty| (1, ty)));
    body.emit_into(
        "lir_test_execution",
        &mut backend,
        run_index,
        false,
        literal_out,
        |import| import_indices[&import],
        &mut function,
    );
    backend.codes.function(&function);
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
    module.section(&backend.types);
    module.section(&imports);
    module.section(&backend.funcs);
    module.section(&memories);
    module.section(&exports);
    module.section(&backend.codes);
    module.section(backend.data_segments.section());
    let bytes = module.finish();
    wasmparser::Validator::new()
        .validate_all(&bytes)
        .expect("valid executable LIR CFG and typed stack");
    bytes
}
