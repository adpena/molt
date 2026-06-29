use super::WasmFunctionFrame;
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm::constant_ops::emit_seeded_runtime_const_op;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

impl WasmFunctionFrame {
    pub(in crate::wasm) fn emit_debug_local_map(&self, func_ir: &FunctionIR) {
        if std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok().as_deref()
            != Some(func_ir.name.as_str())
        {
            return;
        }
        eprintln!("WASM_DEBUG_FUNC {}", func_ir.name);
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
            eprintln!(
                "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?}",
                idx, op.kind, op.var, op.out, op.args, mapped
            );
        }
    }

    pub(in crate::wasm) fn emit_dispatch_seed_initializers(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        if !self.control_mode.needs_dispatch() {
            return;
        }
        for (_, op) in &self.seeded_runtime_const_ops {
            emit_seeded_runtime_const_op(
                backend,
                func,
                op,
                &self.locals,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
            );
        }
        for (local_idx, bits) in self.const_seed_locals.iter().copied() {
            func.instruction(&Instruction::I64Const(bits));
            func.instruction(&Instruction::LocalSet(local_idx));
        }
    }

    pub(in crate::wasm) fn emit_entry_initializers(
        &self,
        func: &mut Function,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
    ) {
        self.const_cache.emit_init(func);
        if let Some(idx) = self.arena_local {
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ArenaNew],
            );
            func.instruction(&Instruction::LocalSet(idx));
        }
    }

    pub(in crate::wasm) fn emit_implicit_return(
        &self,
        func: &mut Function,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
    ) {
        if let Some(arena_idx) = self.arena_local {
            func.instruction(&Instruction::LocalGet(arena_idx));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ArenaFree],
            );
        }
        self.const_cache.emit_none(func);
        func.instruction(&Instruction::End);
    }
}
