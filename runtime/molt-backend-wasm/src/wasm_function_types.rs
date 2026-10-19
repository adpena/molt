//! Module-local structural function types, assigned in first-use order.

use std::collections::HashMap;

use wasm_encoder::{Encode, Section, TypeSection, ValType};

type ResultIndices = HashMap<Box<[ValType]>, u32>;

#[derive(Default)]
pub(crate) struct WasmFunctionTypes {
    section: TypeSection,
    // Parameter partitions are disjoint. Boxed signatures use their arity,
    // avoiding a temporary vector (or an arity walk) on repeated requests.
    boxed: HashMap<usize, ResultIndices>,
    machine: HashMap<Box<[ValType]>, ResultIndices>,
}

impl WasmFunctionTypes {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn function(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        if params.iter().all(|ty| *ty == ValType::I64) {
            return self.boxed_signature(params.len(), results);
        }
        if let Some(index) = self.machine.get(params).and_then(|rows| rows.get(results)) {
            return *index;
        }
        let index = self.section.len();
        self.section
            .ty()
            .function(params.iter().copied(), results.iter().copied());
        self.machine
            .entry(params.into())
            .or_default()
            .insert(results.into(), index);
        index
    }

    pub(crate) fn boxed(&mut self, arity: usize, returns_value: bool) -> u32 {
        let results = if returns_value {
            &[ValType::I64][..]
        } else {
            &[]
        };
        self.boxed_signature(arity, results)
    }

    fn boxed_signature(&mut self, arity: usize, results: &[ValType]) -> u32 {
        if let Some(index) = self.boxed.get(&arity).and_then(|rows| rows.get(results)) {
            return *index;
        }
        let index = self.section.len();
        self.section.ty().function(
            std::iter::repeat_n(ValType::I64, arity),
            results.iter().copied(),
        );
        self.boxed
            .entry(arity)
            .or_default()
            .insert(results.into(), index);
        index
    }
}

impl Encode for WasmFunctionTypes {
    fn encode(&self, sink: &mut Vec<u8>) {
        self.section.encode(sink);
    }
}

impl Section for WasmFunctionTypes {
    fn id(&self) -> u8 {
        self.section.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmparser::{CompositeInnerType, Parser, Payload, ValType as ParsedType};

    fn signatures(types: &WasmFunctionTypes) -> Vec<(Vec<ParsedType>, Vec<ParsedType>)> {
        let mut module = wasm_encoder::Module::new();
        module.section(types);
        let bytes = module.finish();
        wasmparser::Validator::new().validate_all(&bytes).unwrap();
        let mut signatures = Vec::new();
        for payload in Parser::new(0).parse_all(&bytes) {
            if let Payload::TypeSection(reader) = payload.unwrap() {
                for group in reader {
                    for ty in group.unwrap().into_types() {
                        let CompositeInnerType::Func(function) = ty.composite_type.inner else {
                            panic!("expected a flat function type");
                        };
                        signatures.push((function.params().to_vec(), function.results().to_vec()));
                    }
                }
            }
        }
        signatures
    }

    #[test]
    fn equivalent_producers_share_the_same_encoded_signature() {
        let mut types = WasmFunctionTypes::new();
        let boxed = types.boxed(2, true);
        let mixed = types.function(&[ValType::I32, ValType::I64], &[ValType::I64]);
        let void = types.boxed(2, false);
        for _ in 0..100 {
            assert_eq!(
                types.function(&[ValType::I64, ValType::I64], &[ValType::I64]),
                boxed
            );
            assert_eq!(
                types.function(&[ValType::I32, ValType::I64], &[ValType::I64]),
                mixed
            );
            assert_eq!(types.function(&[ValType::I64, ValType::I64], &[]), void);
        }
        assert_eq!(
            signatures(&types),
            vec![
                (
                    vec![ParsedType::I64, ParsedType::I64],
                    vec![ParsedType::I64]
                ),
                (
                    vec![ParsedType::I32, ParsedType::I64],
                    vec![ParsedType::I64]
                ),
                (vec![ParsedType::I64, ParsedType::I64], vec![]),
            ]
        );
    }

    #[test]
    fn sparse_boxed_arities_encode_only_the_requested_types() {
        let mut types = WasmFunctionTypes::new();
        let large = types.boxed(512, true);
        let empty = types.boxed(0, false);
        assert_ne!(large, empty);
        assert_eq!(large, types.boxed(512, true));
        let signatures = signatures(&types);
        assert_eq!(signatures.len(), 2);
        assert_eq!(signatures[large as usize].0.len(), 512);
        assert!(signatures[empty as usize].0.is_empty());
    }
}
