use std::collections::BTreeSet;
use std::time::Instant;

use wasm_encoder::{RawSection, TagKind, TagSection, TagType};

use super::callable_table::WasmCallableTableElements;
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm_abi::TAG_EXCEPTION_FUNC_TYPE;
use crate::wasm_binary::{add_reloc_sections, strip_unused_imports, validate_wasm_sections};
use crate::wasm_options::WasmProfile;
use crate::wasm_plan::{emit_wasm_stage_audit, simple_ir_stage_shape};

pub(super) struct WasmModuleFinalizationInput<'a> {
    pub(super) functions: &'a [FunctionIR],
    pub(super) callable_table_elements: WasmCallableTableElements,
    pub(super) reloc_enabled: bool,
}

impl WasmBackend {
    pub(super) fn finalize_wasm_module(
        mut self,
        input: WasmModuleFinalizationInput<'_>,
    ) -> Vec<u8> {
        let WasmModuleFinalizationInput {
            functions,
            callable_table_elements,
            reloc_enabled,
        } = input;

        self.emit_linear_memory_surface();
        self.emit_import_audit();
        self.append_ordered_sections(&callable_table_elements);

        let unused_imports = if self.options.wasm_profile != WasmProfile::Full {
            self.import_ids.unused_names().into_iter().collect()
        } else {
            BTreeSet::new()
        };
        let module_finish_start = Instant::now();
        let mut bytes = self.module.finish();
        emit_wasm_stage_audit(
            "after-module-finish",
            simple_ir_stage_shape(functions),
            Some(bytes.len()),
            None,
            None,
            Some(module_finish_start.elapsed().as_millis()),
        );

        bytes = Self::strip_unused_imports_if_enabled(bytes, functions, unused_imports);
        if reloc_enabled {
            bytes = add_reloc_sections(
                bytes,
                self.data_segments.segments(),
                self.data_segments.relocs(),
            );
        }
        bytes
    }

    fn emit_import_audit(&self) {
        if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() != Ok("1") {
            return;
        }

        let unused = self.import_ids.unused_names();
        let total = self.import_ids.len();
        let used = total - unused.len();
        let pct = if total > 0 {
            (unused.len() as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "[molt-wasm-import-audit] {used}/{total} imports used, {} unused ({pct:.1}% bloat)",
            unused.len()
        );
        if !unused.is_empty() {
            eprintln!("[molt-wasm-import-audit] unused imports:");
            for name in &unused {
                eprintln!("  - {name}");
            }
        }

        let eh_imports = [
            "exception_push",
            "exception_pop",
            "exception_pending",
            "exception_clear",
            "exception_new",
            "exception_new_builtin",
            "exception_new_builtin_empty",
            "exception_new_builtin_one",
            "exception_new_from_class",
            "exception_match_builtin",
            "exception_kind",
            "exception_class",
            "exception_message",
            "exception_active",
            "exception_last",
            "exception_last_pending",
            "exception_stack_clear",
            "exception_set_cause",
            "exception_set_value",
            "exception_context_set",
            "exception_set_last",
            "raise",
        ];
        let eh_used: Vec<&str> = eh_imports
            .iter()
            .copied()
            .filter(|name| self.import_ids.is_used_name(name))
            .collect();
        let eh_eliminable: Vec<&str> = ["exception_push", "exception_pop", "exception_pending"]
            .iter()
            .copied()
            .filter(|name| self.import_ids.is_used_name(name))
            .collect();
        eprintln!(
            "[molt-wasm-import-audit] exception host calls: {}/{} used ({} eliminable by native EH: {})",
            eh_used.len(),
            eh_imports.len(),
            eh_eliminable.len(),
            eh_eliminable.join(", "),
        );
        if self.options.native_eh_enabled && !self.options.reloc_enabled {
            eprintln!("[molt-wasm-import-audit] native EH ENABLED: tag section emitted");
        } else if self.options.native_eh_enabled && self.options.reloc_enabled {
            eprintln!(
                "[molt-wasm-import-audit] native EH requested but suppressed (reloc mode; wasm-ld doesn't support EH relocations)"
            );
        } else {
            eprintln!("[molt-wasm-import-audit] native EH disabled (MOLT_WASM_NATIVE_EH=0)");
        }

        eprintln!(
            "[molt-wasm-import-audit] tail calls emitted: {} (return_call instructions)",
            self.tail_calls_emitted
        );

        let total_data_bytes = self.data_segments.total_data_bytes();
        let dedup_hits = self.data_segments.dedup_entry_count();
        eprintln!(
            "[molt-wasm-import-audit] data segments: {} segments, {} total bytes, {} dedup cache entries",
            self.data_segments.segment_count(),
            total_data_bytes,
            dedup_hits,
        );
    }

    fn append_ordered_sections(&mut self, callable_table_elements: &WasmCallableTableElements) {
        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);

        if self.options.native_eh_enabled && !self.options.reloc_enabled {
            let mut tags = TagSection::new();
            tags.tag(TagType {
                kind: TagKind::Exception,
                func_type_idx: TAG_EXCEPTION_FUNC_TYPE,
            });
            self.module.section(&tags);
        }

        self.module.section(&self.exports);
        if let Some(element_section) = callable_table_elements.element_section.as_ref() {
            self.module.section(element_section);
        }
        if let Some(payload) = callable_table_elements.element_payload.as_ref() {
            let raw_section = RawSection {
                id: 9,
                data: payload,
            };
            self.module.section(&raw_section);
        }
        self.module.section(&self.codes);
        self.module.section(self.data_segments.section());
    }

    fn strip_unused_imports_if_enabled(
        mut bytes: Vec<u8>,
        functions: &[FunctionIR],
        unused: BTreeSet<String>,
    ) -> Vec<u8> {
        if unused.is_empty() {
            return bytes;
        }

        let before_len = bytes.len();
        emit_wasm_stage_audit(
            "before-strip-unused-imports",
            simple_ir_stage_shape(functions),
            Some(before_len),
            Some(unused.len()),
            None,
            None,
        );
        let strip_start = Instant::now();
        let stripped = strip_unused_imports(bytes.clone(), &unused);
        emit_wasm_stage_audit(
            "after-strip-unused-imports",
            simple_ir_stage_shape(functions),
            Some(stripped.len()),
            Some(unused.len()),
            None,
            Some(strip_start.elapsed().as_millis()),
        );
        if validate_wasm_sections(&stripped) {
            eprintln!(
                "[molt-wasm-strip] eliminated {} unused imports, \
                 {} -> {} bytes (saved {})",
                unused.len(),
                before_len,
                stripped.len(),
                before_len.saturating_sub(stripped.len()),
            );
            bytes = stripped;
        } else {
            eprintln!(
                "[molt-wasm-strip] stripping {} unused imports produced \
                 invalid WASM; keeping original ({} bytes)",
                unused.len(),
                before_len,
            );
        }
        bytes
    }
}
