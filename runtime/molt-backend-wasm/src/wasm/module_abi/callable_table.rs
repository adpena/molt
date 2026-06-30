use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, ExportKind, Function,
    Instruction,
};

mod call_site;
mod layout;
mod runtime_callables;
mod trampoline_emit;

use crate::SimpleIR;
use crate::TrampolineSpec;
use crate::passes::ReturnAliasSummary;
use crate::wasm::WasmBackend;
use crate::wasm_binary::{emit_call, emit_i32_const, emit_ref_func, encode_u32_leb128_padded};
use crate::wasm_data::DataSegmentRef;
pub(in crate::wasm) use call_site::WasmCallableCallSiteAbi;

pub(super) struct WasmCallableTablePlan {
    table_base: u32,
    table_indices: Vec<u32>,
    split_runtime_owned_slot_start: usize,
    split_runtime_shared_abi_slot_end: usize,
    func_to_table_idx: BTreeMap<String, u32>,
    func_to_index: BTreeMap<String, u32>,
    func_to_trampoline_idx: BTreeMap<String, u32>,
    closure_functions: BTreeSet<String>,
    trampoline_entries: Vec<WasmCallableTrampolineEntry>,
}

pub(super) struct WasmCallableTrampolineEntry {
    name: String,
    expected_func_index: u32,
    target_func_index: u32,
    table_index: u32,
    spec: TrampolineSpec,
    multi_return_count: Option<usize>,
}

pub(super) struct WasmCallableTableElements {
    pub(super) element_section: Option<ElementSection>,
    pub(super) element_payload: Option<Vec<u8>>,
}

impl WasmCallableTablePlan {
    pub(super) fn call_site_abi<'a>(
        &'a self,
        escaped_callable_targets: &'a BTreeSet<String>,
        call_func_spill_offset: u32,
        return_alias_summaries: &'a BTreeMap<String, ReturnAliasSummary>,
    ) -> WasmCallableCallSiteAbi<'a> {
        WasmCallableCallSiteAbi::from_table_plan(
            self,
            escaped_callable_targets,
            call_func_spill_offset,
            return_alias_summaries,
        )
    }

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

    pub(super) fn validate_ir_call_target_closure(&self, ir: &SimpleIR) {
        if let Some(issue) = self.ir_call_target_closure_issue(ir) {
            panic!("{issue}");
        }
    }

    fn ir_call_target_closure_issue(&self, ir: &SimpleIR) -> Option<String> {
        let mut issues: Vec<String> = Vec::new();
        for func_ir in &ir.functions {
            for (op_idx, op) in func_ir.ops.iter().enumerate() {
                let kind = op.kind.as_str();
                let requires = match kind {
                    "call" | "call_internal" => TargetRequirement::FunctionIndex,
                    "call_guarded" => TargetRequirement::FunctionAndTable,
                    "call_async" | "alloc_task" => TargetRequirement::PollTable,
                    "func_new" | "func_new_closure" => TargetRequirement::FunctionObject,
                    _ => continue,
                };
                let Some(target) = op.s_value.as_deref() else {
                    issues.push(format!(
                        "{} op {} {} has no target symbol",
                        func_ir.name, op_idx, kind
                    ));
                    continue;
                };
                self.collect_target_issues(
                    &mut issues,
                    func_ir.name.as_str(),
                    op_idx,
                    kind,
                    target,
                    requires,
                );
            }
        }
        if issues.is_empty() {
            return None;
        }
        let limit = 12usize;
        let preview = issues
            .iter()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if issues.len() > limit {
            format!("; ... (+{} more)", issues.len() - limit)
        } else {
            String::new()
        };
        Some(format!(
            "wasm callable table target validation failed: {preview}{suffix}"
        ))
    }

    fn collect_target_issues(
        &self,
        issues: &mut Vec<String>,
        owner: &str,
        op_idx: usize,
        kind: &str,
        target: &str,
        requires: TargetRequirement,
    ) {
        match requires {
            TargetRequirement::FunctionIndex => {
                self.require_function_index(issues, owner, op_idx, kind, target);
            }
            TargetRequirement::FunctionAndTable => {
                self.require_function_index(issues, owner, op_idx, kind, target);
                self.require_table_slot(issues, owner, op_idx, kind, target);
            }
            TargetRequirement::PollTable => {
                if !target.ends_with("_poll") {
                    issues.push(format!(
                        "{owner} op {op_idx} {kind} targets {target}, expected *_poll"
                    ));
                }
                self.require_table_slot(issues, owner, op_idx, kind, target);
            }
            TargetRequirement::FunctionObject => {
                self.require_table_slot(issues, owner, op_idx, kind, target);
                self.require_trampoline_slot(issues, owner, op_idx, kind, target);
            }
        }
    }

    fn require_function_index(
        &self,
        issues: &mut Vec<String>,
        owner: &str,
        op_idx: usize,
        kind: &str,
        target: &str,
    ) {
        if !self.func_to_index.contains_key(target) {
            issues.push(format!(
                "{owner} op {op_idx} {kind} function target not indexed: {target}"
            ));
        }
    }

    fn require_table_slot(
        &self,
        issues: &mut Vec<String>,
        owner: &str,
        op_idx: usize,
        kind: &str,
        target: &str,
    ) {
        if !self.func_to_table_idx.contains_key(target) {
            issues.push(format!(
                "{owner} op {op_idx} {kind} table target not indexed: {target}"
            ));
        }
    }

    fn require_trampoline_slot(
        &self,
        issues: &mut Vec<String>,
        owner: &str,
        op_idx: usize,
        kind: &str,
        target: &str,
    ) {
        if !self.func_to_trampoline_idx.contains_key(target) {
            issues.push(format!(
                "{owner} op {op_idx} {kind} trampoline target not indexed: {target}"
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum TargetRequirement {
    FunctionIndex,
    FunctionAndTable,
    PollTable,
    FunctionObject,
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

    pub(super) fn emit_table_abi_trampolines(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
    ) {
        for entry in &plan.trampoline_entries {
            if self.func_count != entry.expected_func_index {
                panic!(
                    "wasm trampoline index mismatch for {}: expected {}, got {}",
                    entry.name, entry.expected_func_index, self.func_count
                );
            }
            self.compile_trampoline(
                reloc_enabled,
                entry.target_func_index,
                entry.table_index,
                entry.spec,
                entry.multi_return_count,
            );
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
        emit_call(
            func,
            reloc_enabled,
            self.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RuntimeInit],
        );
        func.instruction(&Instruction::Drop);
        if manifest_len > 0 {
            self.emit_data_ptr(reloc_enabled, func_index, func, manifest_segment);
            func.instruction(&Instruction::I64Const(i64::from(manifest_len)));
            emit_call(
                func,
                reloc_enabled,
                self.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::SetIntrinsicManifest],
            );
            func.instruction(&Instruction::Drop);
        }
        emit_call(func, reloc_enabled, table_init_index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionIR, OpIR, SimpleIR};

    fn plan(
        func_to_table_idx: BTreeMap<String, u32>,
        func_to_index: BTreeMap<String, u32>,
        func_to_trampoline_idx: BTreeMap<String, u32>,
    ) -> WasmCallableTablePlan {
        WasmCallableTablePlan {
            table_base: 0,
            table_indices: Vec::new(),
            split_runtime_owned_slot_start: 0,
            split_runtime_shared_abi_slot_end: 0,
            func_to_table_idx,
            func_to_index,
            func_to_trampoline_idx,
            closure_functions: BTreeSet::new(),
            trampoline_entries: Vec::new(),
        }
    }

    fn ir_with_op(kind: &str, target: &str) -> SimpleIR {
        SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                ops: vec![OpIR {
                    kind: kind.to_string(),
                    s_value: Some(target.to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        }
    }

    #[test]
    fn callable_table_validation_rejects_missing_direct_call_target() {
        let issue = plan(BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
            .ir_call_target_closure_issue(&ir_with_op("call", "sys___init_metadata"))
            .expect("missing direct call target should fail closed");

        assert!(issue.contains("wasm callable table target validation failed"));
        assert!(issue.contains("molt_main op 0 call function target not indexed"));
        assert!(issue.contains("sys___init_metadata"));
    }

    #[test]
    fn callable_table_validation_rejects_function_object_without_trampoline() {
        let issue = plan(
            BTreeMap::from([("callee".to_string(), 7)]),
            BTreeMap::from([("callee".to_string(), 42)]),
            BTreeMap::new(),
        )
        .ir_call_target_closure_issue(&ir_with_op("func_new", "callee"))
        .expect("function objects require trampoline table custody");

        assert!(issue.contains("func_new trampoline target not indexed: callee"));
    }

    #[test]
    fn callable_table_validation_accepts_complete_guarded_call_target() {
        let issue = plan(
            BTreeMap::from([("callee".to_string(), 7)]),
            BTreeMap::from([("callee".to_string(), 42)]),
            BTreeMap::new(),
        )
        .ir_call_target_closure_issue(&ir_with_op("call_guarded", "callee"));

        assert!(issue.is_none());
    }
}
