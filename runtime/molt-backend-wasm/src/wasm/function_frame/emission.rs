use super::WasmFunctionFrame;
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::constant_ops::{emit_const_anchor_materialization, emit_release_const_anchors};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use std::fmt::Write as _;
use wasm_encoder::{Function, Instruction};

impl WasmFunctionFrame {
    pub(in crate::wasm) fn emit_debug_local_map(&self, func_ir: &FunctionIR, func_index: u32) {
        let Some(filter) = std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok() else {
            return;
        };
        if filter != "1" && !func_ir.name.contains(&filter) {
            return;
        }
        let mut dump = format!("WASM_DEBUG_FUNC {} index={func_index}\n", func_ir.name);
        for (idx, op) in func_ir.ops.iter().enumerate() {
            let mut mentioned: Vec<String> = Vec::new();
            if let Some(args) = &op.args {
                mentioned.extend(args.iter().cloned());
            }
            if let Some(var) = &op.var {
                mentioned.push(var.clone());
            }
            if let Some(out) = &op.out {
                mentioned.push(out.clone());
            }
            mentioned.sort();
            mentioned.dedup();
            let mapped: Vec<String> = mentioned
                .into_iter()
                .filter_map(|name| self.locals.get(&name).map(|slot| format!("{name}->{slot}")))
                .collect();
            let _ = writeln!(
                dump,
                "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?}",
                idx, op.kind, op.var, op.out, op.args, mapped
            );
        }
        eprint!("{dump}");
        let sanitized: String = func_ir
            .name
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
                _ => '_',
            })
            .collect();
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("wasm/locals/{sanitized}.log"),
            &dump,
        );
    }

    pub(in crate::wasm) fn emit_const_anchor_initializers(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        for anchor in &self.const_anchors {
            self.const_cache.emit_none(func);
            func.instruction(&Instruction::LocalSet(anchor.local));
        }

        for anchor in &self.const_anchors {
            emit_const_anchor_materialization(
                backend,
                func,
                &anchor.op,
                &self.locals,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
                anchor.local,
            );
            let policy = WasmConstOpPolicy::for_op(&anchor.op)
                .unwrap_or_else(|| panic!("missing WASM const policy for {}", anchor.op.kind));
            if policy.materialization_can_fail() {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[WasmRuntimeImport::ExceptionPending],
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                self.const_cache.emit_none(func);
                self.emit_const_anchor_releases(func, import_ids, reloc_enabled);
                func.instruction(&Instruction::Return);
                func.instruction(&Instruction::End);
            }
            if let Some((source, aliases)) = anchor.scratch_aliases.split_first() {
                for alias in aliases {
                    func.instruction(&Instruction::LocalGet(source.ptr_local()));
                    func.instruction(&Instruction::LocalSet(alias.ptr_local()));
                    func.instruction(&Instruction::LocalGet(source.len_local()));
                    func.instruction(&Instruction::LocalSet(alias.len_local()));
                }
            }
        }

        if self.control_mode.needs_dispatch() {
            for (local_idx, bits) in self.const_seed_locals.iter().copied() {
                func.instruction(&Instruction::I64Const(bits));
                func.instruction(&Instruction::LocalSet(local_idx));
            }
        }
    }

    pub(in crate::wasm) fn emit_entry_initializers(&self, func: &mut Function) {
        self.const_cache.emit_init(func);
    }

    pub(in crate::wasm) fn emit_const_anchor_releases(
        &self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        emit_release_const_anchors(func, self.const_anchor_locals(), import_ids, reloc_enabled);
    }

    pub(in crate::wasm) fn emit_implicit_return(
        &self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        self.const_cache.emit_none(func);
        self.emit_const_anchor_releases(func, import_ids, reloc_enabled);
        func.instruction(&Instruction::End);
    }
}
