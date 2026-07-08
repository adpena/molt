use std::collections::BTreeSet;

use wasm_encoder::{EntityType, ExportKind};
use wasmparser::{Parser, Payload, TypeRef};

use super::sections::{
    element_entry_from_parser, entity_type_from_parser, export_kind_from_parser,
};

const RUNTIME_IMPORT_MODULE: &str = "molt_runtime";

pub(super) struct StripPlan {
    pub(super) func_import_count: u32,
    pub(super) removed_count: u32,
    pub(super) import_remap: Vec<Option<u32>>,
    pub(super) imports: Vec<ImportEntry>,
    pub(super) exports: Vec<ExportEntry>,
    pub(super) elements: Vec<ElementEntry>,
}

pub(super) struct ImportEntry {
    pub(super) module: String,
    pub(super) name: String,
    pub(super) entity_ty: EntityType,
    pub(super) remove: bool,
}

pub(super) struct ExportEntry {
    pub(super) name: String,
    pub(super) kind: ExportKind,
    pub(super) index: u32,
}

pub(super) struct ElementEntry {
    pub(super) mode: ElementModeSpec,
    pub(super) indices: Vec<u32>,
}

pub(super) enum ElementModeSpec {
    Active { table: Option<u32>, offset: i32 },
    Passive,
    Declared,
}

impl StripPlan {
    pub(super) fn build(bytes: &[u8], unused_names: &BTreeSet<String>) -> Result<Self, String> {
        let mut plan = Self {
            func_import_count: 0,
            removed_count: 0,
            import_remap: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            elements: Vec::new(),
        };

        for payload in Parser::new(0).parse_all(bytes) {
            match payload.map_err(|err| format!("failed to parse WASM payload: {err}"))? {
                Payload::ImportSection(reader) => {
                    let mut next_func_index = 0u32;
                    for import in reader.into_imports() {
                        let import =
                            import.map_err(|err| format!("failed to parse import: {err}"))?;
                        let is_func = matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_));
                        let remove = is_func
                            && import.module == RUNTIME_IMPORT_MODULE
                            && unused_names.contains(import.name);
                        if is_func {
                            plan.func_import_count += 1;
                            if remove {
                                plan.import_remap.push(None);
                                plan.removed_count += 1;
                            } else {
                                plan.import_remap.push(Some(next_func_index));
                                next_func_index += 1;
                            }
                        }
                        plan.imports.push(ImportEntry {
                            module: import.module.to_string(),
                            name: import.name.to_string(),
                            entity_ty: entity_type_from_parser(import.ty)?,
                            remove,
                        });
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export =
                            export.map_err(|err| format!("failed to parse export: {err}"))?;
                        plan.exports.push(ExportEntry {
                            name: export.name.to_string(),
                            kind: export_kind_from_parser(export.kind),
                            index: export.index,
                        });
                    }
                }
                Payload::ElementSection(reader) => {
                    for element in reader {
                        let element =
                            element.map_err(|err| format!("failed to parse element: {err}"))?;
                        plan.elements.push(element_entry_from_parser(element)?);
                    }
                }
                _ => {}
            }
        }

        Ok(plan)
    }

    pub(super) fn remap_func_index(&self, old: u32) -> Result<u32, String> {
        if old < self.func_import_count {
            return self
                .import_remap
                .get(old as usize)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    format!("WASM body references removed function import index {old}")
                });
        }
        old.checked_sub(self.removed_count)
            .ok_or_else(|| format!("function index {old} underflowed import strip remap"))
    }
}
