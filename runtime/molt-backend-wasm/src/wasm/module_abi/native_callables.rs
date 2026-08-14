use std::collections::{BTreeMap, btree_map::Entry};

use wasm_encoder::{EntityType, ValType};

use crate::native_callable_abi::{
    NATIVE_CALLABLE_ABI_CHOICES, NativeCallableAbi, NativeCallableWasmType,
    parse_native_callable_abi,
};
use crate::wasm::WasmBackend;
use crate::wasm_abi::{NATIVE_CALLABLE_IMPORT_MODULE, TypeSectionExt, static_func_type_idx};
use crate::{OpIR, SimpleIR};

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
            if parsed.requires_direct_symbol_binding() {
                panic!(
                    "native callable export `{export_name}` uses module_attr with direct-symbol-only ABI `{}`",
                    parsed.token()
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
        if func.is_extern {
            continue;
        }
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
    dynamic_type_indices: &mut BTreeMap<(NativeCallableAbi, usize), u32>,
    abi: NativeCallableAbi,
    arity: usize,
) -> u32 {
    let signature = abi
        .wasm_machine_signature(arity)
        .expect("validated native callable arity must have a WASM machine signature");
    let params = signature
        .params
        .iter()
        .copied()
        .map(wasm_val_type)
        .collect::<Vec<_>>();
    let results = signature
        .results
        .iter()
        .copied()
        .map(wasm_val_type)
        .collect::<Vec<_>>();
    if let Some(type_idx) = static_func_type_idx(&params, &results) {
        return type_idx;
    }
    let key = (abi, arity);
    if let Some(type_idx) = dynamic_type_indices.get(&key) {
        return *type_idx;
    }
    let type_idx = *next_type_idx;
    types.function(params, results);
    *next_type_idx += 1;
    dynamic_type_indices.insert(key, type_idx);
    type_idx
}

fn wasm_val_type(value: NativeCallableWasmType) -> ValType {
    match value {
        NativeCallableWasmType::I32 => ValType::I32,
        NativeCallableWasmType::I64 => ValType::I64,
        NativeCallableWasmType::F32 => ValType::F32,
        NativeCallableWasmType::F64 => ValType::F64,
    }
}
