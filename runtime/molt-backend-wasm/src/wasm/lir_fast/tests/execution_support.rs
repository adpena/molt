//! One linker/validator for executable LIR test bodies.
use crate::wasm::WasmBackend;
use crate::wasm::body::WasmBody;
use crate::wasm_abi::emit_static_type_section;
use crate::wasm_data::DataSegmentRef;
use std::collections::BTreeMap;
use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Module, TypeSection,
};

pub(super) fn executable_module(body: &WasmBody) -> Vec<u8> {
    let mut types = TypeSection::new();
    emit_static_type_section(&mut types);
    let function_type = types.len();
    types
        .ty()
        .function(body.param_types.clone(), body.result_types.clone());
    let mut imports = ImportSection::new();
    let mut import_indices = BTreeMap::new();
    for import in body.runtime_imports() {
        let index = import_indices.len() as u32;
        if let std::collections::btree_map::Entry::Vacant(entry) = import_indices.entry(import) {
            entry.insert(index);
            imports.import(
                "molt_runtime",
                import.name(),
                EntityType::Function(import.type_idx()),
            );
        }
    }
    let mut functions = FunctionSection::new();
    functions.function(function_type);
    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, import_indices.len() as u32);
    let mut function = Function::new(body.locals.iter().map(|&ty| (1, ty)));
    body.emit_into(
        "lir_test_execution",
        &mut WasmBackend::new(),
        0,
        false,
        DataSegmentRef {
            offset: 0,
            index: 0,
        },
        |import| import_indices[&import],
        &mut function,
    );
    let mut code = CodeSection::new();
    code.function(&function);
    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
    let bytes = module.finish();
    wasmparser::Validator::new()
        .validate_all(&bytes)
        .expect("valid executable LIR CFG and typed stack");
    bytes
}
