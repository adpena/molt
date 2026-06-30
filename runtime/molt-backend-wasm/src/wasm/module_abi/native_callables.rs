use std::collections::{BTreeMap, btree_map::Entry};

use wasm_encoder::{EntityType, ValType};

use crate::native_callable_abi::{
    NATIVE_CALLABLE_ABI_CHOICES, NativeCallableAbi, parse_native_callable_abi,
};
use crate::wasm::WasmBackend;
use crate::wasm_abi::{STATIC_FUNC_TYPES, TypeSectionExt};
use crate::{OpIR, SimpleIR};

const NATIVE_CALLABLE_IMPORT_MODULE: &str = "molt_native";

pub(super) struct WasmNativeCallableImportEmission {
    pub(super) imports: WasmNativeCallableImports,
    pub(super) next_type_idx: u32,
}

#[derive(Clone, Debug, Default)]
pub(in crate::wasm) struct WasmNativeCallableImports {
    by_export: BTreeMap<String, WasmNativeCallableImport>,
}

#[derive(Clone, Debug)]
pub(in crate::wasm) struct WasmNativeCallableImport {
    pub(in crate::wasm) export_name: String,
    pub(in crate::wasm) binding: String,
    pub(in crate::wasm) abi: String,
    pub(in crate::wasm) abi_contract: NativeCallableAbi,
    pub(in crate::wasm) symbol: String,
    pub(in crate::wasm) arity: usize,
    pub(in crate::wasm) function_index: u32,
}

impl WasmNativeCallableImports {
    fn insert(&mut self, import: WasmNativeCallableImport) {
        if self
            .by_export
            .insert(import.export_name.clone(), import)
            .is_some()
        {
            panic!("duplicate wasm native callable import insertion");
        }
    }

    pub(in crate::wasm) fn required(&self, export_name: &str) -> &WasmNativeCallableImport {
        self.by_export.get(export_name).unwrap_or_else(|| {
            panic!(
                "native callable export `{export_name}` reached wasm codegen without native import custody"
            )
        })
    }
}

impl WasmNativeCallableImport {
    pub(in crate::wasm) fn assert_matches_op(&self, op: &OpIR) {
        let binding = op.native_callable_binding.as_deref().unwrap_or("<missing>");
        let abi = op.native_callable_abi.as_deref().unwrap_or("<missing>");
        let symbol = op
            .native_callable_symbol
            .as_deref()
            .unwrap_or("<module-attr>");
        if binding != self.binding || abi != self.abi || symbol != self.symbol {
            panic!(
                "native callable export `{}` wasm import custody drifted: op binding={binding} abi={abi} symbol={symbol}; import binding={} abi={} symbol={}",
                self.export_name, self.binding, self.abi, self.symbol
            );
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeCallableRequest {
    export_name: String,
    binding: String,
    abi: String,
    abi_contract: NativeCallableAbi,
    symbol: String,
    arity: usize,
}

#[derive(Clone, Debug)]
struct NativeSymbolImport {
    abi: String,
    abi_contract: NativeCallableAbi,
    arity: usize,
    type_idx: u32,
    function_index: u32,
}

fn wasm_native_callable_abi(export_name: &str, abi: &str, arity: usize) -> NativeCallableAbi {
    let parsed = parse_native_callable_abi(abi).unwrap_or_else(|| {
        panic!(
            "native callable export `{export_name}` declares unknown ABI `{abi}`; known ABIs: {NATIVE_CALLABLE_ABI_CHOICES}"
        )
    });
    if let Some(expected_arity) = parsed.fixed_arity()
        && arity != expected_arity
    {
        panic!(
            "native callable export `{export_name}` declares `{}` with arity {arity}; expected exactly {expected_arity} ABI payload argument(s)",
            parsed.token()
        );
    }
    parsed
}

impl NativeCallableRequest {
    fn from_op(op: &OpIR) -> Option<Self> {
        let export_name = op.native_callable_export.as_deref()?;
        if op.kind != "invoke_ffi" {
            panic!(
                "native callable export `{export_name}` is attached to unsupported op kind `{}`",
                op.kind
            );
        }
        let binding = op.native_callable_binding.as_deref().unwrap_or("<missing>");
        let abi = op.native_callable_abi.as_deref().unwrap_or("<missing>");
        if binding == "module_attr" {
            let parsed = parse_native_callable_abi(abi).unwrap_or_else(|| {
                panic!(
                    "native callable export `{export_name}` declares unknown ABI `{abi}`; known ABIs: {NATIVE_CALLABLE_ABI_CHOICES}"
                )
            });
            if parsed == NativeCallableAbi::ForwardF32V1 {
                panic!(
                    "native callable export `{export_name}` uses module_attr with forward_f32 memory ABI"
                );
            }
            return None;
        }
        if binding != "direct_symbol" {
            panic!(
                "native callable export `{export_name}` uses binding `{binding}`; wasm native ABI dispatch requires direct_symbol"
            );
        }
        let symbol = op
            .native_callable_symbol
            .as_deref()
            .unwrap_or_else(|| {
                panic!(
                    "native callable export `{export_name}` uses direct_symbol without native_callable_symbol"
                )
            })
            .to_string();
        if symbol.is_empty() {
            panic!("native callable export `{export_name}` has an empty direct symbol");
        }
        let args = op.args.as_ref().unwrap_or_else(|| {
            panic!("native callable export `{export_name}` invoke_ffi is missing args")
        });
        let arity = args.len();
        let abi_contract = wasm_native_callable_abi(export_name, abi, arity);
        Some(Self {
            export_name: export_name.to_string(),
            binding: binding.to_string(),
            abi: abi.to_string(),
            abi_contract,
            symbol,
            arity,
        })
    }
}

impl WasmBackend {
    pub(super) fn emit_native_callable_import_surface(
        &mut self,
        ir: &SimpleIR,
        mut next_type_idx: u32,
    ) -> WasmNativeCallableImportEmission {
        let requests = native_callable_requests(ir);
        let mut imports = WasmNativeCallableImports::default();
        let mut dynamic_type_indices = BTreeMap::new();
        let mut symbol_imports: BTreeMap<String, NativeSymbolImport> = BTreeMap::new();

        for request in requests.values() {
            let type_idx = native_callable_type_idx(
                &mut self.types,
                &mut next_type_idx,
                &mut dynamic_type_indices,
                request.abi_contract,
                request.arity,
            );
            let symbol_import = match symbol_imports.entry(request.symbol.clone()) {
                Entry::Vacant(entry) => {
                    let function_index = self.func_count;
                    self.imports.import(
                        NATIVE_CALLABLE_IMPORT_MODULE,
                        &request.symbol,
                        EntityType::Function(type_idx),
                    );
                    self.func_count += 1;
                    entry
                        .insert(NativeSymbolImport {
                            abi: request.abi.clone(),
                            abi_contract: request.abi_contract,
                            arity: request.arity,
                            type_idx,
                            function_index,
                        })
                        .clone()
                }
                Entry::Occupied(entry) => {
                    let import = entry.get();
                    if import.abi != request.abi
                        || import.abi_contract != request.abi_contract
                        || import.arity != request.arity
                        || import.type_idx != type_idx
                    {
                        panic!(
                            "native callable symbol `{}` is reused with incompatible wasm ABI: existing abi={} arity={} type_idx={}, requested export `{}` abi={} arity={} type_idx={}",
                            request.symbol,
                            import.abi,
                            import.arity,
                            import.type_idx,
                            request.export_name,
                            request.abi,
                            request.arity,
                            type_idx
                        );
                    }
                    import.clone()
                }
            };
            imports.insert(WasmNativeCallableImport {
                export_name: request.export_name.clone(),
                binding: request.binding.clone(),
                abi: request.abi.clone(),
                abi_contract: request.abi_contract,
                symbol: request.symbol.clone(),
                arity: request.arity,
                function_index: symbol_import.function_index,
            });
        }

        WasmNativeCallableImportEmission {
            imports,
            next_type_idx,
        }
    }
}

fn native_callable_requests(ir: &SimpleIR) -> BTreeMap<String, NativeCallableRequest> {
    let mut requests = BTreeMap::new();
    for func in &ir.functions {
        for op in &func.ops {
            let Some(request) = NativeCallableRequest::from_op(op) else {
                continue;
            };
            match requests.entry(request.export_name.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(request);
                }
                Entry::Occupied(entry) => {
                    if entry.get() != &request {
                        panic!(
                            "native callable export `{}` has conflicting wasm ABI metadata",
                            request.export_name
                        );
                    }
                }
            }
        }
    }
    requests
}

fn native_callable_type_idx(
    types: &mut wasm_encoder::TypeSection,
    next_type_idx: &mut u32,
    dynamic_type_indices: &mut BTreeMap<usize, u32>,
    abi: NativeCallableAbi,
    arity: usize,
) -> u32 {
    match abi {
        NativeCallableAbi::ObjectCallV1 => {
            i64_params_to_i64_result_type_idx(types, next_type_idx, dynamic_type_indices, arity)
        }
        NativeCallableAbi::ObjectCallargsV1 => static_native_callable_type_idx(abi),
        NativeCallableAbi::ForwardF32V1 => static_native_callable_type_idx(abi),
    }
}

fn static_native_callable_type_idx(abi: NativeCallableAbi) -> u32 {
    let signature = abi.wasm_signature();
    STATIC_FUNC_TYPES
        .iter()
        .position(|spec| {
            spec.params.len() == signature.params.len()
                && spec.results.len() == signature.results.len()
                && spec
                    .params
                    .iter()
                    .zip(signature.params.iter())
                    .all(|(actual, expected)| val_type_matches(*expected, *actual))
                && spec
                    .results
                    .iter()
                    .zip(signature.results.iter())
                    .all(|(actual, expected)| val_type_matches(*expected, *actual))
        })
        .unwrap_or_else(|| {
            panic!(
                "native callable ABI {} has no static WASM type {:?} -> {:?}",
                abi.token(),
                signature.params,
                signature.results
            )
        }) as u32
}

fn val_type_matches(expected: &str, actual: ValType) -> bool {
    matches!(
        (expected, actual),
        ("i32", ValType::I32)
            | ("i64", ValType::I64)
            | ("f32", ValType::F32)
            | ("f64", ValType::F64)
    )
}

fn i64_params_to_i64_result_type_idx(
    types: &mut wasm_encoder::TypeSection,
    next_type_idx: &mut u32,
    dynamic_type_indices: &mut BTreeMap<usize, u32>,
    arity: usize,
) -> u32 {
    if let Some(type_idx) = static_boxed_object_call_type_idx(arity) {
        return type_idx;
    }
    if let Some(type_idx) = dynamic_type_indices.get(&arity) {
        return *type_idx;
    }
    let type_idx = *next_type_idx;
    types.function(
        std::iter::repeat_n(ValType::I64, arity),
        std::iter::once(ValType::I64),
    );
    *next_type_idx += 1;
    dynamic_type_indices.insert(arity, type_idx);
    type_idx
}

fn static_boxed_object_call_type_idx(arity: usize) -> Option<u32> {
    STATIC_FUNC_TYPES
        .iter()
        .position(|spec| {
            spec.params.len() == arity
                && spec.params.iter().all(|ty| *ty == ValType::I64)
                && spec.results.len() == 1
                && spec.results[0] == ValType::I64
        })
        .map(|idx| idx as u32)
}
