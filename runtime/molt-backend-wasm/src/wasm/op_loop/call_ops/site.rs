use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{ConstantCache, box_int, stable_ic_site_id};
use molt_tir::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalAccessKind {
    Read,
    Definition,
}

/// Path-local value-epoch authority for call-boundary retention.
///
/// Physical WASM slots are reused across non-overlapping SSA values. A raw
/// "last use is later" query therefore mistakes a future value for the stale
/// bits currently occupying its slot and can temporarily root already-released
/// objects across calls (including `gc.collect`). The next canonical def/use
/// event answers the actual question: retain the current epoch only when its
/// next event is a read; a definition first means the old slot contents are
/// dead. Stateful/jumpful emitters build this over their exact path slice.
pub(in crate::wasm::op_loop) struct CallRetentionLiveness {
    accesses: BTreeMap<String, Vec<(usize, LocalAccessKind)>>,
}

impl CallRetentionLiveness {
    pub(in crate::wasm::op_loop) fn for_region(ops: &[OpIR]) -> Self {
        let mut accesses: BTreeMap<String, Vec<(usize, LocalAccessKind)>> = BTreeMap::new();
        for (op_idx, op) in ops.iter().enumerate() {
            visit_simple_ir_reads(op, |read| {
                if read.name != "none" {
                    accesses
                        .entry(read.name.to_string())
                        .or_default()
                        .push((op_idx, LocalAccessKind::Read));
                }
            });
            visit_simple_ir_defined_names(op, |name| {
                if name != "none" {
                    accesses
                        .entry(name.to_string())
                        .or_default()
                        .push((op_idx, LocalAccessKind::Definition));
                }
            });
        }
        Self { accesses }
    }

    fn current_epoch_is_read_later(&self, name: &str, op_idx: usize) -> bool {
        let Some(accesses) = self.accesses.get(name) else {
            return false;
        };
        let next = accesses.partition_point(|(access_idx, _)| *access_idx <= op_idx);
        accesses
            .get(next)
            .is_some_and(|(_, kind)| *kind == LocalAccessKind::Read)
    }
}

pub(super) fn collect_live_object_locals_for_call(
    locals: &WasmFrameLocals,
    liveness: &CallRetentionLiveness,
    op_idx: usize,
    out_name: Option<&String>,
) -> Vec<u32> {
    let mut live = BTreeSet::new();
    for local in locals.named_locals() {
        if out_name.is_some_and(|out| out == local.name()) {
            continue;
        }
        if local.kind().is_call_retention_exempt() {
            continue;
        }
        if !liveness.current_epoch_is_read_later(local.name(), op_idx) {
            continue;
        }
        live.insert(local.slot());
    }
    live.into_iter().collect()
}

pub(super) fn retain_live_object_locals(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    live_object_locals: &[u32],
) {
    for local_idx in live_object_locals {
        func.instruction(&Instruction::LocalGet(*local_idx));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    }
}

pub(super) fn release_live_object_locals(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    live_object_locals: &[u32],
) {
    for local_idx in live_object_locals.iter().rev() {
        func.instruction(&Instruction::LocalGet(*local_idx));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
        );
    }
}

pub(super) fn push_call_args(func: &mut Function, locals: &WasmFrameLocals, args_names: &[String]) {
    for arg_name in args_names {
        let arg = locals[arg_name];
        func.instruction(&Instruction::LocalGet(arg));
    }
}

pub(super) fn store_call_result(func: &mut Function, out: u32) {
    func.instruction(&Instruction::LocalSet(out));
}

pub(super) fn spill_call_args(
    func: &mut Function,
    locals: &WasmFrameLocals,
    spill_base: u32,
    args_names: &[String],
) {
    for (i, arg_name) in args_names.iter().enumerate() {
        let arg = locals[arg_name];
        func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
        func.instruction(&Instruction::LocalGet(arg));
        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
    }
}

pub(super) fn build_positional_callargs(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    locals: &WasmFrameLocals,
    callargs_tmp: u32,
    args_names: &[String],
) {
    func.instruction(&Instruction::I64Const(args_names.len() as i64));
    func.instruction(&Instruction::I64Const(0));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallargsNew],
    );
    func.instruction(&Instruction::LocalSet(callargs_tmp));
    for arg_name in args_names {
        let arg = locals[arg_name];
        func.instruction(&Instruction::LocalGet(callargs_tmp));
        func.instruction(&Instruction::LocalGet(arg));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallargsPushPos],
        );
        func.instruction(&Instruction::Drop);
    }
}

pub(super) fn emit_call_site_id(func: &mut Function, func_name: &str, op_idx: usize, label: &str) {
    let site_bits = box_int(stable_ic_site_id(func_name, op_idx, label));
    func.instruction(&Instruction::I64Const(site_bits));
}

pub(super) fn emit_pending_exception_return(func: &mut Function, const_cache: &ConstantCache) {
    const_cache.emit_none(func);
    func.instruction(&Instruction::Return);
}

#[cfg(test)]
mod tests {
    use super::{CallRetentionLiveness, collect_live_object_locals_for_call};
    use crate::OpIR;
    use crate::wasm::{WasmFrameLocalKind, WasmFrameLocals, WasmFrameSyntheticLocal};
    use crate::wasm_abi_generated::WasmConstLiteralPayload;

    fn op(kind: &str, out: Option<&str>, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: out.map(str::to_string),
            args: Some(args.iter().map(|value| (*value).to_string()).collect()),
            ..OpIR::default()
        }
    }

    #[test]
    fn call_retention_uses_typed_local_kind_not_name_shape() {
        let mut locals = WasmFrameLocals::new();
        locals.insert("__molt_tmp0".to_string(), 0);
        locals.insert("payload_ptr".to_string(), 1);
        locals.insert("__multi_ret_0".to_string(), 2);
        locals.insert(WasmFrameLocals::NONE_NAME.to_string(), 3);

        let liveness = CallRetentionLiveness::for_region(&[
            op("call", Some("result"), &[]),
            op(
                "tuple_new",
                Some("later"),
                &["__molt_tmp0", "payload_ptr", "__multi_ret_0", "none"],
            ),
        ]);
        assert_eq!(
            collect_live_object_locals_for_call(&locals, &liveness, 0, None,),
            vec![0, 1, 2]
        );
        assert_eq!(
            locals.local_kind(WasmFrameLocals::NONE_NAME),
            Some(WasmFrameLocalKind::NoneSingleton)
        );
    }

    #[test]
    fn call_retention_exempts_frame_owned_locals_by_kind() {
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        locals.insert("value".to_string(), local_count);
        local_count += 1;
        locals.ensure_synthetic(
            WasmFrameSyntheticLocal::MoltTmp0,
            &mut local_types,
            &mut local_count,
        );
        locals.ensure_literal_scratch(
            "payload",
            WasmConstLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );

        let liveness = CallRetentionLiveness::for_region(&[
            op("call", Some("result"), &[]),
            op(
                "tuple_new",
                Some("later"),
                &["value", "__molt_tmp0", "payload_ptr", "payload_len"],
            ),
        ]);
        assert_eq!(
            collect_live_object_locals_for_call(&locals, &liveness, 0, None,),
            vec![0]
        );
    }

    #[test]
    fn call_retention_exempts_every_dead_sink_alias_by_physical_kind() {
        let mut locals = WasmFrameLocals::new();
        locals.insert_dead_sink_alias("dead_result".to_string(), 0);
        let liveness = CallRetentionLiveness::for_region(&[
            op("call", Some("result"), &[]),
            op("print", None, &["dead_result"]),
        ]);
        assert!(collect_live_object_locals_for_call(&locals, &liveness, 0, None,).is_empty());
        assert_eq!(
            locals.local_kind("dead_result"),
            Some(WasmFrameLocalKind::FixedSynthetic(
                WasmFrameSyntheticLocal::DeadSink
            ))
        );
    }

    #[test]
    fn call_retention_does_not_pin_stale_bits_for_future_or_redefined_values() {
        let mut locals = WasmFrameLocals::new();
        locals.insert("released".to_string(), 0);
        locals.insert("future".to_string(), 0);
        locals.insert("redefined".to_string(), 1);

        let mut redefine = op("store_var", None, &["incoming"]);
        redefine.var = Some("redefined".to_string());
        let liveness = CallRetentionLiveness::for_region(&[
            op("call", Some("result"), &[]),
            op("const", Some("future"), &[]),
            redefine,
            op("tuple_new", Some("later"), &["future", "redefined"]),
        ]);

        assert!(collect_live_object_locals_for_call(&locals, &liveness, 0, None).is_empty());
    }
}
