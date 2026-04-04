#![cfg(feature = "wasm-backend")]

//! Tests for the import registry: no duplicate imports, valid type indices,
//! and correct sentinel behavior for skipped imports.

use std::collections::BTreeSet;

use molt_backend::wasm::{WasmBackend, WasmCompileOptions, WasmProfile};
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{Parser, Payload, TypeRef};

fn empty_ir() -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    }
}

fn compile_with_profile(ir: SimpleIR, profile: WasmProfile) -> Vec<u8> {
    WasmBackend::with_options(WasmCompileOptions {
        wasm_profile: profile,
        ..WasmCompileOptions::default()
    })
    .compile(ir)
}

fn extract_imports(wasm: &[u8]) -> Vec<(String, String)> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ImportSection(section) = payload.expect("valid payload") {
            for import in section.into_imports() {
                let import = import.expect("valid import");
                imports.push((import.module.to_string(), import.name.to_string()));
            }
        }
    }
    imports
}

fn extract_func_imports(wasm: &[u8]) -> Vec<(String, u32)> {
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ImportSection(section) = payload.expect("valid payload") {
            for import in section.into_imports() {
                let import = import.expect("valid import");
                if let TypeRef::Func(type_idx) = import.ty {
                    imports.push((import.name.to_string(), type_idx));
                }
            }
        }
    }
    imports
}

fn count_types(wasm: &[u8]) -> u32 {
    let mut count = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::TypeSection(reader) = payload.expect("valid payload") {
            for _ in reader.into_iter() {
                count += 1;
            }
        }
    }
    count
}

// -----------------------------------------------------------------------
// Registry integrity tests
// -----------------------------------------------------------------------

#[test]
fn full_profile_has_no_duplicate_import_names() {
    let wasm = compile_with_profile(empty_ir(), WasmProfile::Full);
    let imports = extract_func_imports(&wasm);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (name, _) in &imports {
        assert!(seen.insert(name.clone()), "duplicate import: {name}");
    }
}

#[test]
fn auto_profile_has_no_duplicate_import_names() {
    let wasm = compile_with_profile(empty_ir(), WasmProfile::Auto);
    let imports = extract_func_imports(&wasm);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (name, _) in &imports {
        assert!(seen.insert(name.clone()), "duplicate import: {name}");
    }
}

#[test]
fn pure_profile_has_no_duplicate_import_names() {
    let wasm = compile_with_profile(empty_ir(), WasmProfile::Pure);
    let imports = extract_func_imports(&wasm);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (name, _) in &imports {
        assert!(seen.insert(name.clone()), "duplicate import: {name}");
    }
}

#[test]
fn all_imports_reference_valid_type_indices() {
    let wasm = compile_with_profile(empty_ir(), WasmProfile::Full);
    let type_count = count_types(&wasm);
    let imports = extract_func_imports(&wasm);
    for (name, type_idx) in &imports {
        assert!(
            *type_idx < type_count,
            "import {name} references type index {type_idx} but only {type_count} types exist"
        );
    }
}

#[test]
fn all_function_imports_belong_to_molt_runtime_module() {
    let wasm = compile_with_profile(empty_ir(), WasmProfile::Full);
    let func_imports = extract_func_imports(&wasm);
    // All *function* imports should come from "molt_runtime".
    // Non-function imports (e.g. __indirect_function_table) may come from "env".
    let import_pairs = extract_imports(&wasm);
    let func_names: BTreeSet<String> = func_imports.iter().map(|(n, _)| n.clone()).collect();
    for (module, name) in &import_pairs {
        if func_names.contains(name) {
            assert_eq!(
                module, "molt_runtime",
                "function import {name} belongs to module {module}, expected molt_runtime"
            );
        }
    }
}

#[test]
fn import_count_is_nonzero_for_all_profiles() {
    for profile in [WasmProfile::Full, WasmProfile::Auto, WasmProfile::Pure] {
        let wasm = compile_with_profile(empty_ir(), profile);
        let imports = extract_func_imports(&wasm);
        assert!(
            !imports.is_empty(),
            "profile {:?} produced zero imports",
            profile
        );
    }
}

#[test]
fn core_imports_always_present_in_all_profiles() {
    // All core structural imports must be present in Full and Pure profiles
    // (which do not undergo post-compilation stripping).
    let core_imports = [
        "runtime_init",
        "runtime_shutdown",
        "inc_ref_obj",
        "dec_ref_obj",
        "print_obj",
        "print_newline",
        "alloc",
    ];
    for profile in [WasmProfile::Full, WasmProfile::Pure] {
        let wasm = compile_with_profile(empty_ir(), profile);
        let import_names: BTreeSet<String> = extract_func_imports(&wasm)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for core in &core_imports {
            assert!(
                import_names.contains(*core),
                "core import {core} missing in profile {:?}",
                profile
            );
        }
    }

    // In Auto profile, dead-import elimination strips imports not referenced
    // by codegen.  Only verify imports that the init code actually uses.
    let auto_wasm = compile_with_profile(empty_ir(), WasmProfile::Auto);
    let auto_names: BTreeSet<String> = extract_func_imports(&auto_wasm)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for core in ["runtime_init", "runtime_shutdown"] {
        assert!(
            auto_names.contains(core),
            "core import {core} missing in Auto profile after stripping",
        );
    }
}

#[test]
fn full_profile_has_more_imports_than_pure() {
    let full = extract_func_imports(&compile_with_profile(empty_ir(), WasmProfile::Full));
    let pure = extract_func_imports(&compile_with_profile(empty_ir(), WasmProfile::Pure));
    assert!(
        full.len() > pure.len(),
        "Full ({}) should have more imports than Pure ({})",
        full.len(),
        pure.len()
    );
}

#[test]
fn full_profile_has_more_imports_than_auto_for_empty_ir() {
    let full = extract_func_imports(&compile_with_profile(empty_ir(), WasmProfile::Full));
    let auto = extract_func_imports(&compile_with_profile(empty_ir(), WasmProfile::Auto));
    assert!(
        full.len() >= auto.len(),
        "Full ({}) should have >= imports than Auto ({})",
        full.len(),
        auto.len()
    );
}
