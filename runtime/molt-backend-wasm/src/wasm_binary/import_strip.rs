use std::collections::BTreeSet;

use wasm_encoder::{
    ConstExpr, Encode, EntityType, ExportKind, ExportSection, ImportSection, MemoryType, TagKind,
    TagType,
};
use wasmparser::{ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

use super::code_remap::remap_code_section;
use super::emit::encode_u32_leb128_padded;
use super::leb::{encode_u32_leb128, read_u32_leb128};
use super::types::{encoder_ref_type, encoder_val_type};

const RUNTIME_IMPORT_MODULE: &str = "molt_runtime";

struct StripPlan {
    func_import_count: u32,
    removed_count: u32,
    import_remap: Vec<Option<u32>>,
    imports: Vec<ImportEntry>,
    exports: Vec<ExportEntry>,
    elements: Vec<ElementEntry>,
}

struct ImportEntry {
    module: String,
    name: String,
    entity_ty: EntityType,
    remove: bool,
}

struct ExportEntry {
    name: String,
    kind: ExportKind,
    index: u32,
}

struct ElementEntry {
    mode: ElementModeSpec,
    indices: Vec<u32>,
}

enum ElementModeSpec {
    Active { table: Option<u32>, offset: i32 },
    Passive,
    Declared,
}

impl StripPlan {
    fn build(bytes: &[u8], unused_names: &BTreeSet<String>) -> Result<Self, String> {
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

    fn remap_func_index(&self, old: u32) -> Result<u32, String> {
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

/// Strip unused function imports from a serialized WASM module.
///
/// `unused_names` contains import field names in the `molt_runtime` module that
/// should be removed. The rewrite fails loudly if any removed import is still
/// referenced; import tracking bugs must not silently produce invalid binaries.
pub(crate) fn strip_unused_imports(bytes: Vec<u8>, unused_names: &BTreeSet<String>) -> Vec<u8> {
    strip_unused_imports_checked(bytes, unused_names)
        .unwrap_or_else(|err| panic!("failed to strip unused WASM imports: {err}"))
}

fn strip_unused_imports_checked(
    bytes: Vec<u8>,
    unused_names: &BTreeSet<String>,
) -> Result<Vec<u8>, String> {
    let plan = StripPlan::build(&bytes, unused_names)?;
    if plan.removed_count == 0 {
        return Ok(bytes);
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(bytes.get(..8).ok_or("WASM binary missing header")?);

    let mut pos = 8usize;
    while pos < bytes.len() {
        let section_id = *bytes
            .get(pos)
            .ok_or_else(|| format!("missing section id at byte offset {pos}"))?;
        pos += 1;
        let (section_size, content_start) = read_u32_leb128(&bytes, pos)
            .ok_or_else(|| format!("invalid section size at byte offset {pos}"))?;
        let content_end = content_start
            .checked_add(section_size as usize)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format!("section {section_id} size overflows module"))?;
        let section_bytes = &bytes[content_start..content_end];

        match section_id {
            2 => {
                let mut section = ImportSection::new();
                for import in &plan.imports {
                    if !import.remove {
                        section.import(&import.module, &import.name, import.entity_ty.clone());
                    }
                }
                out.push(2);
                section.encode(&mut out);
            }
            7 => {
                let mut section = ExportSection::new();
                for export in &plan.exports {
                    let index = if export.kind == ExportKind::Func {
                        plan.remap_func_index(export.index)?
                    } else {
                        export.index
                    };
                    section.export(&export.name, export.kind, index);
                }
                out.push(7);
                section.encode(&mut out);
            }
            8 => {
                let (old_idx, _) = read_u32_leb128(section_bytes, 0)
                    .ok_or("start section missing function index")?;
                let new_idx = plan.remap_func_index(old_idx)?;
                let mut body = Vec::new();
                encode_u32_leb128(new_idx, &mut body);
                out.push(8);
                encode_u32_leb128(body.len() as u32, &mut out);
                out.extend_from_slice(&body);
            }
            9 => {
                let section = encode_element_section(&plan)?;
                out.push(9);
                encode_u32_leb128(section.len() as u32, &mut out);
                out.extend_from_slice(&section);
            }
            10 => {
                let new_code =
                    remap_code_section(section_bytes, &|old| plan.remap_func_index(old))?;
                out.push(10);
                encode_u32_leb128(new_code.len() as u32, &mut out);
                out.extend_from_slice(&new_code);
            }
            _ => {
                out.push(section_id);
                out.extend_from_slice(&bytes[pos..content_end]);
            }
        }

        pos = content_end;
    }

    if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1") {
        eprintln!(
            "[molt-wasm-import-strip] eliminated {} unused imports \
             ({} -> {}), binary {} -> {} bytes (saved {} bytes)",
            plan.removed_count,
            plan.func_import_count,
            plan.func_import_count - plan.removed_count,
            bytes.len(),
            out.len(),
            bytes.len().saturating_sub(out.len()),
        );
    }

    Ok(out)
}

fn encode_element_section(plan: &StripPlan) -> Result<Vec<u8>, String> {
    let mut payload = Vec::new();
    encode_u32_leb128(plan.elements.len() as u32, &mut payload);
    for element in &plan.elements {
        let indices = element
            .indices
            .iter()
            .map(|index| plan.remap_func_index(*index))
            .collect::<Result<Vec<_>, _>>()?;
        match element.mode {
            ElementModeSpec::Active {
                table: None,
                offset,
            } => {
                encode_u32_leb128(0, &mut payload);
                ConstExpr::i32_const(offset).encode(&mut payload);
            }
            ElementModeSpec::Active {
                table: Some(table),
                offset,
            } => {
                encode_u32_leb128(2, &mut payload);
                encode_u32_leb128(table, &mut payload);
                ConstExpr::i32_const(offset).encode(&mut payload);
                payload.push(0);
            }
            ElementModeSpec::Passive => {
                encode_u32_leb128(1, &mut payload);
                payload.push(0);
            }
            ElementModeSpec::Declared => {
                encode_u32_leb128(3, &mut payload);
                payload.push(0);
            }
        }
        encode_u32_leb128(indices.len() as u32, &mut payload);
        for index in indices {
            encode_u32_leb128_padded(index, &mut payload);
        }
    }
    Ok(payload)
}

fn entity_type_from_parser(ty: TypeRef) -> Result<EntityType, String> {
    Ok(match ty {
        TypeRef::Func(idx) | TypeRef::FuncExact(idx) => EntityType::Function(idx),
        TypeRef::Table(t) => EntityType::Table(wasm_encoder::TableType {
            element_type: encoder_ref_type(t.element_type),
            table64: t.table64,
            minimum: t.initial,
            maximum: t.maximum,
            shared: t.shared,
        }),
        TypeRef::Memory(m) => EntityType::Memory(MemoryType {
            minimum: m.initial,
            maximum: m.maximum,
            memory64: m.memory64,
            shared: m.shared,
            page_size_log2: m.page_size_log2,
        }),
        TypeRef::Global(g) => EntityType::Global(wasm_encoder::GlobalType {
            val_type: encoder_val_type(g.content_type),
            mutable: g.mutable,
            shared: g.shared,
        }),
        TypeRef::Tag(t) => EntityType::Tag(TagType {
            kind: TagKind::Exception,
            func_type_idx: t.func_type_idx,
        }),
    })
}

fn export_kind_from_parser(kind: ExternalKind) -> ExportKind {
    match kind {
        ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
        ExternalKind::Table => ExportKind::Table,
        ExternalKind::Memory => ExportKind::Memory,
        ExternalKind::Global => ExportKind::Global,
        ExternalKind::Tag => ExportKind::Tag,
    }
}

fn element_entry_from_parser(element: wasmparser::Element<'_>) -> Result<ElementEntry, String> {
    let ElementItems::Functions(funcs) = element.items else {
        return Err("unsupported expression element segment in WASM import strip".to_string());
    };
    let mut indices = Vec::new();
    for func in funcs {
        indices.push(func.map_err(|err| format!("failed to parse element function: {err}"))?);
    }
    let mode = match element.kind {
        wasmparser::ElementKind::Active {
            table_index,
            offset_expr,
        } => ElementModeSpec::Active {
            table: table_index.filter(|&table| table != 0),
            offset: const_i32_offset(offset_expr)?,
        },
        wasmparser::ElementKind::Passive => ElementModeSpec::Passive,
        wasmparser::ElementKind::Declared => ElementModeSpec::Declared,
    };
    Ok(ElementEntry { mode, indices })
}

fn const_i32_offset(expr: wasmparser::ConstExpr<'_>) -> Result<i32, String> {
    let mut ops = expr.get_operators_reader();
    let offset = match ops
        .read()
        .map_err(|err| format!("failed to read element offset expression: {err}"))?
    {
        Operator::I32Const { value } => value,
        other => {
            return Err(format!(
                "unsupported element offset expression in WASM import strip: {other:?}"
            ));
        }
    };
    match ops
        .read()
        .map_err(|err| format!("failed to read element offset terminator: {err}"))?
    {
        Operator::End => Ok(offset),
        other => Err(format!(
            "element offset expression has trailing operator in WASM import strip: {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use wasm_encoder::{
        CodeSection, ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, EntityType,
        ExportKind, ExportSection, Function, FunctionSection, ImportSection, Instruction, Module,
        RefType, StartSection, TableSection, TableType, TypeSection,
    };
    use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

    use super::super::leb::read_u32_leb128;
    use super::strip_unused_imports;

    fn fixture_module() -> Vec<u8> {
        let mut module = Module::new();

        let mut types = TypeSection::new();
        types.ty().function([], []);
        module.section(&types);

        let mut imports = ImportSection::new();
        imports.import("env", "dead", EntityType::Function(0));
        imports.import("molt_runtime", "dead", EntityType::Function(0));
        imports.import("molt_runtime", "live", EntityType::Function(0));
        module.section(&imports);

        let mut funcs = FunctionSection::new();
        funcs.function(0);
        module.section(&funcs);

        let mut tables = TableSection::new();
        tables.table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 2,
            maximum: None,
            shared: false,
        });
        module.section(&tables);

        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, 3);
        module.section(&exports);

        module.section(&StartSection { function_index: 3 });

        let offset = ConstExpr::i32_const(0);
        let mut elements = ElementSection::new();
        elements.segment(ElementSegment {
            mode: ElementMode::Active {
                table: None,
                offset: &offset,
            },
            elements: Elements::Functions(std::borrow::Cow::Owned(vec![2, 3])),
        });
        module.section(&elements);

        let mut codes = CodeSection::new();
        let mut body = Function::new([]);
        body.instruction(&Instruction::Call(2));
        body.instruction(&Instruction::End);
        codes.function(&body);
        module.section(&codes);

        module.finish()
    }

    fn function_import_names(bytes: &[u8]) -> Vec<(String, String)> {
        let mut imports = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            if let Ok(Payload::ImportSection(reader)) = payload {
                for import in reader.into_imports().flatten() {
                    if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                        imports.push((import.module.to_string(), import.name.to_string()));
                    }
                }
            }
        }
        imports
    }

    fn function_exports(bytes: &[u8]) -> BTreeMap<String, u32> {
        let mut exports = BTreeMap::new();
        for payload in Parser::new(0).parse_all(bytes) {
            if let Ok(Payload::ExportSection(reader)) = payload {
                for export in reader.into_iter().flatten() {
                    if matches!(export.kind, ExternalKind::Func | ExternalKind::FuncExact) {
                        exports.insert(export.name.to_string(), export.index);
                    }
                }
            }
        }
        exports
    }

    fn start_function(bytes: &[u8]) -> Option<u32> {
        for payload in Parser::new(0).parse_all(bytes) {
            if let Ok(Payload::StartSection { func, .. }) = payload {
                return Some(func);
            }
        }
        None
    }

    fn element_function_indices(bytes: &[u8]) -> Vec<u32> {
        let mut indices = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            if let Ok(Payload::ElementSection(reader)) = payload {
                for element in reader.into_iter().flatten() {
                    if let wasmparser::ElementItems::Functions(funcs) = element.items {
                        indices.extend(funcs.into_iter().flatten());
                    }
                }
            }
        }
        indices
    }

    fn element_section_payload(bytes: &[u8]) -> Vec<u8> {
        let mut pos = 8usize;
        while pos < bytes.len() {
            let section_id = bytes[pos];
            pos += 1;
            let (section_len, content_start) =
                read_u32_leb128(bytes, pos).expect("section size must parse");
            let content_end = content_start + section_len as usize;
            if section_id == 9 {
                return bytes[content_start..content_end].to_vec();
            }
            pos = content_end;
        }
        panic!("element section missing");
    }

    fn direct_call_indices(bytes: &[u8]) -> Vec<u32> {
        let mut calls = Vec::new();
        for payload in Parser::new(0).parse_all(bytes) {
            if let Ok(Payload::CodeSectionEntry(body)) = payload
                && let Ok(mut ops) = body.get_operators_reader()
            {
                while let Ok(op) = ops.read() {
                    if let wasmparser::Operator::Call { function_index } = op {
                        calls.push(function_index);
                    }
                }
            }
        }
        calls
    }

    #[test]
    fn strip_unused_imports_remaps_all_function_index_surfaces() {
        let mut unused = BTreeSet::new();
        unused.insert("dead".to_string());

        let stripped = strip_unused_imports(fixture_module(), &unused);
        wasmparser::Validator::new()
            .validate_all(&stripped)
            .expect("stripped module must validate");

        assert_eq!(
            function_import_names(&stripped),
            vec![
                ("env".to_string(), "dead".to_string()),
                ("molt_runtime".to_string(), "live".to_string()),
            ]
        );
        assert_eq!(function_exports(&stripped)["run"], 2);
        assert_eq!(start_function(&stripped), Some(2));
        assert_eq!(element_function_indices(&stripped), vec![1, 2]);
        assert_eq!(direct_call_indices(&stripped), vec![1]);
    }

    #[test]
    fn strip_unused_imports_keeps_element_indices_relocation_padded() {
        let mut unused = BTreeSet::new();
        unused.insert("dead".to_string());

        let stripped = strip_unused_imports(fixture_module(), &unused);
        let payload = element_section_payload(&stripped);

        assert_eq!(
            payload,
            vec![
                0x01, // segment count
                0x00, // active table 0 segment
                0x41, 0x00, 0x0B, // i32.const 0; end
                0x02, // two function indices
                0x81, 0x80, 0x80, 0x80, 0x00, // remapped function index 1
                0x82, 0x80, 0x80, 0x80, 0x00, // remapped function index 2
            ]
        );
        assert_eq!(element_function_indices(&stripped), vec![1, 2]);
    }

    #[test]
    #[should_panic(expected = "references removed function import index")]
    fn strip_unused_imports_fails_if_removed_import_is_still_called() {
        let mut unused = BTreeSet::new();
        unused.insert("live".to_string());
        let _ = strip_unused_imports(fixture_module(), &unused);
    }
}
