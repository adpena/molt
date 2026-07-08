use std::collections::BTreeSet;

use wasm_encoder::{Encode, ExportKind, ExportSection, ImportSection};

use super::code_remap::remap_code_section;
use super::leb::{encode_u32_leb128, read_u32_leb128};

mod plan;
mod sections;

#[cfg(test)]
mod tests;

use plan::StripPlan;
use sections::encode_element_section;

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
