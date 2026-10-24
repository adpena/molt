use molt_backend::WasmBackend;
use molt_backend::{ModuleRegistryIR, SimpleIR};
use std::io;
use std::path::Path;

use super::super::cli_args::WasmTargetOptions;
use molt_artifact_publish::write_bytes_atomically;

pub(crate) fn validate_wasm_module_catalog(
    ir: &SimpleIR,
    registry: &ModuleRegistryIR,
) -> io::Result<()> {
    registry.validate().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("WASM module catalog rejected: {err}"),
        )
    })?;
    let functions: std::collections::BTreeSet<&str> = ir
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    let missing: Vec<&str> = registry
        .init_symbols
        .iter()
        .map(String::as_str)
        .filter(|symbol| !functions.contains(symbol))
        .collect();
    if !missing.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "WASM module catalog references missing initializer symbols: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}

pub(super) fn emit_wasm_target(
    ir: SimpleIR,
    module_registry: Option<ModuleRegistryIR>,
    code_slots: molt_ir::code_slots::CodeSlotBindings,
    output_file: &str,
    options: WasmTargetOptions,
    capture_origin: bool,
) -> io::Result<Option<crate::backend_process::origin::EmittedArtifact>> {
    if let Some(registry) = module_registry.as_ref() {
        validate_wasm_module_catalog(&ir, registry)?;
    }
    let backend = WasmBackend::with_options(options.compile_options())
        .with_module_registry(module_registry)
        .with_code_slots(code_slots);
    let wasm_bytes = backend.compile(ir);
    write_bytes_atomically(Path::new(output_file), &wasm_bytes).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to publish backend output {output_file:?}: {error}"),
        )
    })?;
    println!("Successfully compiled to {output_file}");
    Ok(
        capture_origin.then(|| crate::backend_process::origin::EmittedArtifact {
            output: crate::backend_process::input::ContentIdentity::of_bytes(&wasm_bytes),
            codegen: crate::backend_process::origin::CodegenOrigin::in_process(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_backend::FunctionIR;

    fn registry(symbol: &str) -> ModuleRegistryIR {
        ModuleRegistryIR {
            schema: 1,
            registry_digest: "test".to_string(),
            blob: vec![0],
            relocs: Vec::new(),
            init_symbols: vec![symbol.to_string()],
            rows: vec![molt_ir::ModuleRegistryRowIR {
                name: "demo".into(),
                init_symbol: Some(symbol.to_string()),
            }],
        }
    }

    fn ir(body_present: bool) -> SimpleIR {
        let symbol = "molt_init_demo";
        let mut functions = Vec::new();
        if body_present {
            functions.push(FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: symbol.to_string(),
                ..FunctionIR::default()
            });
        }
        SimpleIR {
            functions,
            profile: None,
        }
    }

    #[test]
    fn catalog_rejects_missing_initializer_before_codegen() {
        let err = validate_wasm_module_catalog(&ir(false), &registry("molt_init_demo"))
            .expect_err("missing initializer must fail closed");
        assert!(err.to_string().contains("missing initializer symbols"));
    }
}
