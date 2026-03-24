//! WASM Component Model support.
//! Provides WIT (WebAssembly Interface Types) generation and
//! canonical ABI for string/list passing.

use super::function::TirModule;
use super::types::TirType;

/// Generate a WIT (WebAssembly Interface Types) definition for a TIR module.
pub fn generate_wit(module: &TirModule) -> String {
    let mut wit = String::new();
    wit.push_str(&format!("package molt:{}\n\n", module.name));
    wit.push_str("world molt-module {\n");

    for func in &module.functions {
        let params: Vec<String> = func
            .param_types
            .iter()
            .enumerate()
            .map(|(i, ty)| format!("p{}: {}", i, tir_type_to_wit(ty)))
            .collect();
        let ret = tir_type_to_wit(&func.return_type);
        wit.push_str(&format!(
            "  export {}: func({}) -> {}\n",
            func.name,
            params.join(", "),
            ret
        ));
    }

    wit.push_str("}\n");
    wit
}

fn tir_type_to_wit(ty: &TirType) -> &'static str {
    match ty {
        TirType::I64 => "s64",
        TirType::F64 => "f64",
        TirType::Bool => "bool",
        TirType::Str => "string",
        TirType::None => "s64", // sentinel
        _ => "s64",             // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::types::TirType;

    fn make_module(name: &str, funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: name.to_string(),
            functions: funcs,
            class_hierarchy: None,
        }
    }

    #[test]
    fn wit_header_contains_package_name() {
        let module = make_module("mylib", vec![]);
        let wit = generate_wit(&module);
        assert!(wit.starts_with("package molt:mylib\n"), "unexpected: {wit}");
        assert!(wit.contains("world molt-module {"));
    }

    #[test]
    fn wit_exports_function_with_correct_types() {
        let func = TirFunction::new(
            "add".to_string(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let module = make_module("math", vec![func]);
        let wit = generate_wit(&module);
        assert!(
            wit.contains("export add: func(p0: s64, p1: s64) -> s64"),
            "unexpected: {wit}"
        );
    }

    #[test]
    fn wit_str_type_maps_to_string() {
        let func = TirFunction::new("greet".to_string(), vec![TirType::Str], TirType::Str);
        let module = make_module("greeter", vec![func]);
        let wit = generate_wit(&module);
        assert!(
            wit.contains("p0: string") && wit.contains("-> string"),
            "unexpected: {wit}"
        );
    }

    #[test]
    fn wit_bool_type_maps_correctly() {
        let func = TirFunction::new("is_ok".to_string(), vec![], TirType::Bool);
        let module = make_module("checks", vec![func]);
        let wit = generate_wit(&module);
        assert!(wit.contains("-> bool"), "unexpected: {wit}");
    }

    #[test]
    fn tir_type_to_wit_none_is_sentinel() {
        assert_eq!(tir_type_to_wit(&TirType::None), "s64");
    }
}
