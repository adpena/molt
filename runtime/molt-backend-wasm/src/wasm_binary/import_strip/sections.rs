use wasm_encoder::{ConstExpr, Encode, EntityType, ExportKind, MemoryType, TagKind, TagType};
use wasmparser::{ElementItems, ExternalKind, Operator, TypeRef};

use super::super::emit::encode_u32_leb128_padded;
use super::super::leb::encode_u32_leb128;
use super::super::types::{encoder_ref_type, encoder_val_type};
use super::plan::{ElementEntry, ElementModeSpec, StripPlan};

pub(super) fn encode_element_section(plan: &StripPlan) -> Result<Vec<u8>, String> {
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

pub(super) fn entity_type_from_parser(ty: TypeRef) -> Result<EntityType, String> {
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

pub(super) fn export_kind_from_parser(kind: ExternalKind) -> ExportKind {
    match kind {
        ExternalKind::Func | ExternalKind::FuncExact => ExportKind::Func,
        ExternalKind::Table => ExportKind::Table,
        ExternalKind::Memory => ExportKind::Memory,
        ExternalKind::Global => ExportKind::Global,
        ExternalKind::Tag => ExportKind::Tag,
    }
}

pub(super) fn element_entry_from_parser(
    element: wasmparser::Element<'_>,
) -> Result<ElementEntry, String> {
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
