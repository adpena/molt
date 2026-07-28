#![cfg(feature = "llvm")]

use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

#[path = "support/generated_object_abi.rs"]
mod generated_object_abi;

#[test]
fn llvm_object_retains_exact_generated_object_abi_through_dead_strip() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let output = backend.compile_llvm(ir);
    generated_object_abi::assert_exact_import_and_dead_strip_link(&output.bytes, "LLVM");
}
