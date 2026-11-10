use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::wasm_binary::Function;
use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, ExportKind,
    Instruction,
};

mod app_callable_resolver;
mod call_site;
mod layout;
mod runtime_callables;
mod trampoline_emit;

use crate::SimpleIR;
use crate::TrampolineSpec;
use crate::wasm::WasmBackend;
use crate::wasm_binary::{emit_call, encode_u32_leb128_padded};
use crate::wasm_data::DataSegmentRef;
use crate::wasm_table::{
    WasmCallableTableAddress, WasmCallableTableRole, WasmCallableTableSlot,
    WasmCallableTableTarget, WasmFunctionSymbol,
};
pub(in crate::wasm) use call_site::WasmCallableCallSiteAbi;

/// How the table plan names a callable (HF-699 K5): a slot of this plan's
/// callable table, or a runtime key that the runtime resolves through its
/// generated key table. Runtime polls are keys on every target (K1); compact
/// builtins and app-resolver entries keep table slots until their own stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallableIdentity {
    TableSlot(u32),
    RuntimeKey(u64),
}

impl CallableIdentity {
    /// The plan slot of a callable that only a table slot can name, such as
    /// a trampoline target or a compiled body.
    fn table_slot(self, name: &str) -> u32 {
        match self {
            Self::TableSlot(slot) => slot,
            Self::RuntimeKey(key) => {
                panic!("`{name}` is runtime key 0x{key:x}, not a callable-table slot")
            }
        }
    }
}

pub(in crate::wasm) struct WasmCallableTablePlan {
    table_base: u32,
    fixed_shared_runtime_abi_base: Option<u32>,
    table_entries: Vec<WasmCallableTableEntry>,
    split_runtime_shared_abi_slot_end: usize,
    func_to_table_idx: BTreeMap<String, CallableIdentity>,
    func_to_index: BTreeMap<String, u32>,
    func_to_trampoline_idx: BTreeMap<String, u32>,
    /// Table slots of address-taken native initializers, keyed by symbol.
    native_initializer_to_table_idx: BTreeMap<String, u32>,
    app_callable_resolver: Option<WasmAppCallableResolverPlan>,
    positional_call_shapes: BTreeMap<String, (usize, bool)>,
    function_abi_returns_value: BTreeMap<String, bool>,
    trampoline_entries: Vec<WasmCallableTrampolineEntry>,
}

pub(super) struct WasmAppCallableResolverPlan {
    resolver_func_index: u32,
    resolver_target: WasmCallableTableSlot,
    entries: Vec<WasmAppCallableResolverEntry>,
}

pub(super) struct WasmAppCallableResolverEntry {
    name: String,
    target: WasmCallableTableSlot,
}

pub(super) struct WasmCallableTrampolineEntry {
    name: String,
    expected_func_index: u32,
    target_func_index: u32,
    target: WasmCallableTableSlot,
    spec: TrampolineSpec,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WasmCallableTableEntry {
    func_index: u32,
    symbol: WasmFunctionSymbol,
}

pub(super) struct WasmCallableTableElements {
    pub(super) element_section: Option<ElementSection>,
    pub(super) element_payload: Option<Vec<u8>>,
    pub(super) layout_payload: Vec<u8>,
}

impl WasmCallableTablePlan {
    pub(in crate::wasm) fn function_index(&self, name: &str) -> Option<u32> {
        self.func_to_index.get(name).copied()
    }

    pub(super) fn call_site_abi<'a>(
        &'a self,
        escaped_callable_targets: &'a BTreeSet<String>,
        call_func_spill_offset: u32,
        entry_custody: &'a BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration>,
    ) -> WasmCallableCallSiteAbi<'a> {
        WasmCallableCallSiteAbi::from_table_plan(
            self,
            escaped_callable_targets,
            call_func_spill_offset,
            entry_custody,
        )
    }

    fn app_callable_resolver_target(&self) -> Option<WasmCallableTableSlot> {
        self.app_callable_resolver
            .as_ref()
            .map(|resolver| resolver.resolver_target)
    }

    /// The stored identity of a planned callable: a runtime key as itself
    /// (K5), a plan slot as its current table address.
    fn target_for_slot(
        &self,
        identity: CallableIdentity,
        role: WasmCallableTableRole,
    ) -> WasmCallableTableTarget {
        match identity {
            CallableIdentity::TableSlot(slot) => {
                WasmCallableTableTarget::TableSlot(self.table_slot_target(slot, role))
            }
            CallableIdentity::RuntimeKey(key) => WasmCallableTableTarget::RuntimeKey(key),
        }
    }

    fn table_slot_target(&self, slot: u32, role: WasmCallableTableRole) -> WasmCallableTableSlot {
        let entry = self
            .table_entries
            .get(slot as usize)
            .unwrap_or_else(|| panic!("callable table slot outside plan: {slot}"));
        let fixed_prefix_len = self.split_runtime_shared_abi_slot_end as u32;
        let (current_table_index, address) = if slot < fixed_prefix_len {
            let base = self.fixed_shared_runtime_abi_base.unwrap_or_else(|| {
                panic!("fixed callable-table slot {slot} has no shared runtime base")
            });
            (
                base.checked_add(slot)
                    .expect("fixed callable-table address overflow"),
                WasmCallableTableAddress::FixedSharedRuntimeAbi {
                    finalized_app_base: self.table_base,
                },
            )
        } else {
            (
                self.table_base
                    .checked_add(slot - fixed_prefix_len)
                    .expect("relocatable callable-table address overflow"),
                WasmCallableTableAddress::Relocatable(entry.symbol),
            )
        };
        WasmCallableTableSlot {
            current_table_index,
            address,
            role,
        }
    }

    pub(super) fn validate_ir_call_target_closure(&self, ir: &SimpleIR) {
        if let Some(issue) = self.ir_call_target_closure_issue(ir) {
            panic!("{issue}");
        }
    }

    fn ir_call_target_closure_issue(&self, ir: &SimpleIR) -> Option<String> {
        let mut issues: Vec<String> = Vec::new();
        let mut task_target_signatures = crate::wasm_abi::POLL_TABLE_IMPORTS
            .iter()
            .filter_map(|spec| {
                let (params, results) = spec.import.machine_signature();
                (params == [wasm_encoder::ValType::I64] && results == [wasm_encoder::ValType::I64])
                    .then_some((spec.import.runtime_export_name(), (1, true)))
            })
            .collect::<BTreeMap<_, _>>();
        for function in &ir.functions {
            match function.function_signature() {
                Ok(signature) => {
                    task_target_signatures.insert(
                        function.name.as_str(),
                        (signature.arity, signature.returns_value),
                    );
                }
                Err(error) => issues.push(error),
            }
        }
        for func_ir in &ir.functions {
            if func_ir.is_extern {
                continue;
            }
            for (op_idx, op) in func_ir.ops.iter().enumerate() {
                let kind = op.kind;
                let requires = match kind {
                    molt_ir::OpKind::Call | molt_ir::OpKind::CallInternal => {
                        TargetRequirement::FunctionIndex
                    }
                    molt_ir::OpKind::CallGuarded => TargetRequirement::FunctionAndTable,
                    molt_ir::OpKind::CallAsync | molt_ir::OpKind::AllocTask => {
                        TargetRequirement::PollTable
                    }
                    molt_ir::OpKind::FuncNew | molt_ir::OpKind::FuncNewClosure => {
                        TargetRequirement::FunctionObject
                    }
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
                    task_target_signatures.get(target).copied(),
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
        kind: molt_ir::OpKind,
        target: &str,
        requires: TargetRequirement,
        target_signature: Option<(usize, bool)>,
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
                if target_signature != Some((1, true)) {
                    issues.push(format!(
                        "{owner} op {op_idx} {kind} targets {target}, expected task ABI (i64) -> i64, got {target_signature:?}"
                    ));
                }
                // A task stores its poll identity as a value, so a runtime
                // poll's key serves as well as a compiled poll's slot (K1).
                if !self.func_to_table_idx.contains_key(target) {
                    issues.push(format!(
                        "{owner} op {op_idx} {kind} task target not indexed: {target}"
                    ));
                }
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
        kind: molt_ir::OpKind,
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
        kind: molt_ir::OpKind,
        target: &str,
    ) {
        match self.func_to_table_idx.get(target) {
            Some(CallableIdentity::TableSlot(_)) => {}
            Some(CallableIdentity::RuntimeKey(key)) => issues.push(format!(
                "{owner} op {op_idx} {kind} targets runtime key 0x{key:x} of {target}, which only a task constructor may store"
            )),
            None => issues.push(format!(
                "{owner} op {op_idx} {kind} table target not indexed: {target}"
            )),
        }
    }

    fn require_trampoline_slot(
        &self,
        issues: &mut Vec<String>,
        owner: &str,
        op_idx: usize,
        kind: molt_ir::OpKind,
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
        let fixed_prefix_len = plan.split_runtime_shared_abi_slot_end;
        let app_entries = &plan.table_entries[fixed_prefix_len..];
        if reloc_enabled {
            for (slot, entry) in plan.table_entries.iter().enumerate() {
                self.exports.export(
                    &format!(
                        "{}.entry.{slot}",
                        crate::wasm_abi_generated::callable_table::CALLABLE_TABLE_LAYOUT_SECTION_NAME
                    ),
                    ExportKind::Func,
                    entry.func_index,
                );
            }
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for entry wrapper"));
            let wrapper_index = self.compile_entry_wrapper(
                reloc_enabled,
                main_index,
                plan.app_callable_resolver_target(),
                manifest_segment,
                manifest_len as u32,
            );
            self.exports.export(
                &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol("molt_main"),
                ExportKind::Func,
                wrapper_index,
            );
            if let Some(host_init_index) = self.molt_host_init_index {
                let host_init_wrapper_index = self.compile_entry_wrapper(
                    reloc_enabled,
                    host_init_index,
                    plan.app_callable_resolver_target(),
                    manifest_segment,
                    manifest_len as u32,
                );
                self.exports.export(
                    &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
                        "molt_host_init",
                    ),
                    ExportKind::Func,
                    host_init_wrapper_index,
                );
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (app_entries.len() as u32).encode(&mut payload);
            for entry in app_entries {
                encode_u32_leb128_padded(entry.func_index, &mut payload);
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
                elements: Elements::Functions(Cow::Owned(
                    app_entries.iter().map(|entry| entry.func_index).collect(),
                )),
            });
            element_section = Some(section);
            if self.module_registry.is_some() {
                let main_index = self
                    .molt_main_index
                    .unwrap_or_else(|| panic!("molt_main missing for module registry wrapper"));
                let wrapper_index = self.compile_entry_wrapper(
                    reloc_enabled,
                    main_index,
                    plan.app_callable_resolver_target(),
                    manifest_segment,
                    manifest_len as u32,
                );
                self.exports.export(
                    &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
                        "molt_main",
                    ),
                    ExportKind::Func,
                    wrapper_index,
                );
                if let Some(host_init_index) = self.molt_host_init_index {
                    let host_init_wrapper_index = self.compile_entry_wrapper(
                        reloc_enabled,
                        host_init_index,
                        plan.app_callable_resolver_target(),
                        manifest_segment,
                        manifest_len as u32,
                    );
                    self.exports.export(
                        &molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(
                            "molt_host_init",
                        ),
                        ExportKind::Func,
                        host_init_wrapper_index,
                    );
                }
            }
        }
        let mut layout_payload = Vec::new();
        crate::wasm_abi_generated::callable_table::CALLABLE_TABLE_LAYOUT_VERSION
            .encode(&mut layout_payload);
        plan.fixed_shared_runtime_abi_base
            .unwrap_or(0)
            .encode(&mut layout_payload);
        (fixed_prefix_len as u32).encode(&mut layout_payload);
        plan.table_base.encode(&mut layout_payload);
        (app_entries.len() as u32).encode(&mut layout_payload);
        WasmCallableTableElements {
            element_section,
            element_payload,
            layout_payload,
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
                entry.target,
                entry.spec,
            );
        }
    }

    fn compile_entry_wrapper(
        &mut self,
        reloc_enabled: bool,
        entry_index: u32,
        app_callable_resolver_target: Option<WasmCallableTableSlot>,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) -> u32 {
        let func_index = self.func_count;
        let type_idx = self.types.boxed(1, true);
        self.funcs.function(type_idx);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        func.attach_context(1, self.execution_call_abis.clone());
        self.emit_host_init_sequence(
            reloc_enabled,
            func_index,
            &mut func,
            app_callable_resolver_target,
            manifest_segment,
            manifest_len,
        );
        emit_call(&mut func, reloc_enabled, entry_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn emit_host_init_sequence(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        app_callable_resolver_target: Option<WasmCallableTableSlot>,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) {
        if let Some(target) = app_callable_resolver_target {
            self.table_relocations.emit_i64(
                reloc_enabled,
                self.func_import_count,
                func_index,
                func,
                &WasmCallableTableTarget::TableSlot(
                    target.with_role(WasmCallableTableRole::AppCallableResolver),
                ),
            );
            emit_call(
                func,
                reloc_enabled,
                self.import_ids
                    [crate::wasm_abi_generated::WasmRuntimeImport::SetAppCallableResolver],
            );
            func.instruction(&Instruction::Drop);
        }
        if let Some(registry_segment) = self.module_registry_segment {
            self.emit_data_ptr_i32(reloc_enabled, func_index, func, registry_segment);
            emit_call(
                func,
                reloc_enabled,
                self.import_ids
                    [crate::wasm_abi_generated::WasmRuntimeImport::ModuleRegistryInstall],
            );
            func.instruction(&Instruction::Drop);
        }
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
        let function_abi_returns_value = func_to_index
            .keys()
            .map(|name| (name.clone(), true))
            .collect();
        WasmCallableTablePlan {
            table_base: 0,
            fixed_shared_runtime_abi_base: None,
            table_entries: Vec::new(),
            split_runtime_shared_abi_slot_end: 0,
            func_to_table_idx: func_to_table_idx
                .into_iter()
                .map(|(name, slot)| (name, CallableIdentity::TableSlot(slot)))
                .collect(),
            func_to_index,
            func_to_trampoline_idx,
            native_initializer_to_table_idx: BTreeMap::new(),
            app_callable_resolver: None,
            positional_call_shapes: BTreeMap::new(),
            function_abi_returns_value,
            trampoline_entries: Vec::new(),
        }
    }

    fn ir_with_op(kind: molt_ir::OpKind, target: &str) -> SimpleIR {
        SimpleIR {
            functions: vec![FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".to_string(),
                params: Vec::new(),
                ops: vec![OpIR {
                    s_value: Some(target.to_string()),
                    ..OpIR::new(kind)
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            }],
            profile: None,
        }
    }

    #[test]
    fn callable_table_validation_rejects_missing_direct_call_target() {
        let issue = plan(BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
            .ir_call_target_closure_issue(&ir_with_op(molt_ir::OpKind::Call, "sys___init_metadata"))
            .expect("missing direct call target should fail closed");

        assert!(issue.contains("wasm callable table target validation failed"));
        assert!(issue.contains("molt_main op 0 call function target not indexed"));
        assert!(issue.contains("sys___init_metadata"));
    }

    #[test]
    fn task_target_validation_uses_physical_signature_not_name_or_callable_arity() {
        for kind in [molt_ir::OpKind::AllocTask, molt_ir::OpKind::CallAsync] {
            for (target, arity, returns_value, accepted) in [
                ("opaque_body", 1, true, true),
                ("ordinary_poll", 0, true, false),
                ("ordinary_poll", 2, true, false),
                ("void_poll", 1, false, false),
            ] {
                let mut ir = ir_with_op(kind, target);
                let mut body = ir.functions[0].clone();
                body.name = target.to_string();
                body.params = (0..arity).map(|index| format!("arg{index}")).collect();
                body.return_abi = if returns_value {
                    molt_ir::FunctionReturnAbi::Value
                } else {
                    molt_ir::FunctionReturnAbi::Void
                };
                body.ops = vec![OpIR::new(molt_ir::OpKind::RetVoid)];
                ir.functions.push(body);
                let mut table = plan(
                    BTreeMap::from([(target.to_string(), 7)]),
                    BTreeMap::from([(target.to_string(), 42)]),
                    BTreeMap::new(),
                );
                // Constructor arity deliberately differs from its task body ABI.
                table
                    .positional_call_shapes
                    .insert(target.to_string(), (0, false));
                let issue = table.ir_call_target_closure_issue(&ir);
                assert_eq!(issue.is_none(), accepted, "{kind} {target}: {issue:?}");
                if !accepted {
                    assert!(issue.unwrap().contains("expected task ABI (i64) -> i64"));
                }
            }
        }
    }

    fn plan_with_runtime_poll_key(target: &str) -> WasmCallableTablePlan {
        let mut table = plan(BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        let key = crate::wasm_abi::RUNTIME_POLL_CALLABLE_KEY_BASE + 1;
        table
            .func_to_table_idx
            .insert(target.to_string(), CallableIdentity::RuntimeKey(key));
        table
    }

    #[test]
    fn task_target_validation_accepts_generated_runtime_poll_signature() {
        // A runtime poll's identity is its key (HF-699 K1), and a task
        // constructor may store a key.
        let target = "molt_async_sleep_poll";
        let table = plan_with_runtime_poll_key(target);
        for kind in [molt_ir::OpKind::CallAsync, molt_ir::OpKind::AllocTask] {
            assert!(
                table
                    .ir_call_target_closure_issue(&ir_with_op(kind, target))
                    .is_none(),
                "{kind}"
            );
        }
    }

    #[test]
    fn callable_table_validation_rejects_runtime_keys_outside_task_constructors() {
        // Only a task constructor stores an identity as a value the runtime
        // resolves. A guarded call or a function object needs a table slot.
        let target = "molt_async_sleep_poll";
        let mut table = plan_with_runtime_poll_key(target);
        table.func_to_index.insert(target.to_string(), 42);
        for kind in [molt_ir::OpKind::CallGuarded, molt_ir::OpKind::FuncNew] {
            let issue = table
                .ir_call_target_closure_issue(&ir_with_op(kind, target))
                .unwrap_or_else(|| panic!("{kind} must refuse a runtime key"));
            assert!(
                issue.contains("which only a task constructor may store"),
                "{kind}: {issue}"
            );
        }
    }

    #[test]
    fn callable_table_validation_rejects_function_object_without_trampoline() {
        let issue = plan(
            BTreeMap::from([("callee".to_string(), 7)]),
            BTreeMap::from([("callee".to_string(), 42)]),
            BTreeMap::new(),
        )
        .ir_call_target_closure_issue(&ir_with_op(molt_ir::OpKind::FuncNew, "callee"))
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
        .ir_call_target_closure_issue(&ir_with_op(molt_ir::OpKind::CallGuarded, "callee"));

        assert!(issue.is_none());
    }
}
