use std::borrow::Cow;
use std::collections::BTreeSet;

use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, EntityType,
    ExportKind, Function, Instruction, MemoryType,
};

use super::callable_table::WasmCallableTablePlan;
use crate::wasm::WasmBackend;
use crate::wasm_binary::{emit_call, emit_i32_const, emit_ref_func, encode_u32_leb128_padded};
use crate::wasm_data::DataSegmentRef;
use crate::wasm_plan::DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES;

pub(super) struct WasmModuleHostSurface {
    pub(super) manifest_segment: DataSegmentRef,
    pub(super) manifest_len: usize,
    pub(super) call_func_spill_offset: u32,
    pub(super) class_def_spill_offset: u32,
    pub(super) const_str_scratch_segment: DataSegmentRef,
}

pub(super) struct WasmCallableTableElements {
    pub(super) element_section: Option<ElementSection>,
    pub(super) element_payload: Option<Vec<u8>>,
}

impl WasmCallableTablePlan {
    fn runtime_initialized_entries(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.table_indices
            .iter()
            .copied()
            .enumerate()
            .filter(|(slot, _func_index)| self.runtime_initializes_slot(*slot))
    }

    fn runtime_initializes_slot(&self, slot: usize) -> bool {
        slot >= self.split_runtime_owned_slot_start || slot < self.split_runtime_shared_abi_slot_end
    }
}

impl WasmBackend {
    pub(super) fn emit_linear_memory_surface(&mut self) {
        let page_size: u64 = 64 * 1024;
        let required_pages = (self.data_segments.offset() as u64).div_ceil(page_size);
        let floor_pages = std::env::var("MOLT_WASM_MIN_PAGES")
            .ok()
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(64);
        let minimum_pages = required_pages.max(floor_pages);
        let memory_ty = MemoryType {
            minimum: minimum_pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        };
        self.imports
            .import("env", "memory", EntityType::Memory(memory_ty));
        self.exports.export("molt_memory", ExportKind::Memory, 0);
    }

    pub(super) fn prepare_module_host_surface(
        &mut self,
        reloc_enabled: bool,
        mut manifest_intrinsic_names: BTreeSet<String>,
        max_call_arity: usize,
        max_class_def_words: usize,
    ) -> WasmModuleHostSurface {
        manifest_intrinsic_names.extend(
            DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES
                .iter()
                .map(|name| (*name).to_string()),
        );
        let manifest_bytes: Vec<u8> = {
            let mut buf = Vec::new();
            for (i, name) in manifest_intrinsic_names.iter().enumerate() {
                if i > 0 {
                    buf.push(0);
                }
                buf.extend_from_slice(name.as_bytes());
            }
            buf
        };
        let manifest_segment = self.add_data_segment(reloc_enabled, &manifest_bytes);
        let manifest_len = manifest_bytes.len();

        // The runtime copies call_func args before dispatching, so nested
        // WASM->runtime->WASM calls cannot observe stale data in this buffer.
        let spill_slots = max_call_arity.max(1);
        let spill_bytes = vec![0u8; spill_slots * 8];
        let spill_segment = self.add_data_segment_mutable(reloc_enabled, &spill_bytes);

        // The class_def helper snapshots the bases/attrs payload before nested calls.
        let class_def_words = max_class_def_words.max(2);
        let class_def_bytes = vec![0u8; class_def_words * 8];
        let class_def_segment = self.add_data_segment_mutable(reloc_enabled, &class_def_bytes);

        // A fixed scratch slot replaces per-const_str heap allocations and leaks.
        let const_str_scratch_bytes = vec![0u8; 8];
        let const_str_scratch_segment =
            self.add_data_segment_mutable(reloc_enabled, &const_str_scratch_bytes);

        WasmModuleHostSurface {
            manifest_segment,
            manifest_len,
            call_func_spill_offset: spill_segment.offset,
            class_def_spill_offset: class_def_segment.offset,
            const_str_scratch_segment,
        }
    }

    pub(super) fn emit_table_elements(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
        manifest_segment: DataSegmentRef,
        manifest_len: usize,
    ) -> WasmCallableTableElements {
        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index = self.compile_table_init(reloc_enabled, plan);
            self.exports
                .export("molt_table_init", ExportKind::Func, table_init_index);
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for table init wrapper"));
            let wrapper_index = self.compile_molt_main_wrapper(
                reloc_enabled,
                main_index,
                table_init_index,
                manifest_segment,
                manifest_len as u32,
            );
            self.exports
                .export("molt_main", ExportKind::Func, wrapper_index);

            let mut ref_exported = BTreeSet::new();
            for (slot, func_index) in plan.runtime_initialized_entries() {
                let table_index = plan.table_base + slot as u32;
                if ref_exported.insert(table_index) {
                    let name = format!("__molt_table_ref_{table_index}");
                    self.exports.export(&name, ExportKind::Func, func_index);
                }
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (plan.table_indices.len() as u32).encode(&mut payload);
            for func_index in &plan.table_indices {
                encode_u32_leb128_padded(*func_index, &mut payload);
            }
            element_payload = Some(payload);
        } else {
            let mut section = ElementSection::new();
            let offset = ConstExpr::i32_const(plan.table_base as i32);
            section.segment(ElementSegment {
                mode: ElementMode::Active {
                    table: None,
                    offset: &offset,
                },
                elements: Elements::Functions(Cow::Borrowed(&plan.table_indices)),
            });
            element_section = Some(section);
        }
        WasmCallableTableElements {
            element_section,
            element_payload,
        }
    }

    fn compile_table_init(&mut self, reloc_enabled: bool, plan: &WasmCallableTablePlan) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(8);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        for (slot, target_index) in plan.runtime_initialized_entries() {
            let table_index = plan.table_base + slot as u32;
            emit_i32_const(&mut func, reloc_enabled, table_index as i32);
            emit_ref_func(&mut func, reloc_enabled, target_index);
            func.instruction(&Instruction::TableSet(0));
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn compile_molt_main_wrapper(
        &mut self,
        reloc_enabled: bool,
        main_index: u32,
        table_init_index: u32,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(0);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        self.emit_host_init_sequence(
            reloc_enabled,
            func_index,
            &mut func,
            table_init_index,
            manifest_segment,
            manifest_len,
        );
        emit_call(&mut func, reloc_enabled, main_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn emit_host_init_sequence(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        table_init_index: u32,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) {
        emit_call(func, reloc_enabled, self.import_ids["runtime_init"]);
        func.instruction(&Instruction::Drop);
        if manifest_len > 0 {
            self.emit_data_ptr(reloc_enabled, func_index, func, manifest_segment);
            func.instruction(&Instruction::I64Const(i64::from(manifest_len)));
            emit_call(
                func,
                reloc_enabled,
                self.import_ids["set_intrinsic_manifest"],
            );
            func.instruction(&Instruction::Drop);
        }
        emit_call(func, reloc_enabled, table_init_index);
    }
}
