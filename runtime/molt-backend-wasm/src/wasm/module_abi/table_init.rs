use std::borrow::Cow;
use std::collections::BTreeSet;

use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, ExportKind, Function,
    Instruction,
};

use super::callable_table::WasmCallableTablePlan;
use crate::wasm::WasmBackend;
use crate::wasm_binary::{emit_call, emit_i32_const, emit_ref_func, encode_u32_leb128_padded};
use crate::wasm_data::DataSegmentRef;

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
