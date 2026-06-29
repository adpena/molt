use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{ConstantCache, box_int, stable_ic_site_id};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{Function, Instruction};

pub(super) fn collect_live_object_locals_for_call(
    locals: &WasmFrameLocals,
    last_use_local: &BTreeMap<String, usize>,
    rel_idx: usize,
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
        if last_use_local
            .get(local.name())
            .is_none_or(|last| *last <= rel_idx)
        {
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

pub(super) fn store_call_result(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    out: u32,
    returns_alias_param: bool,
) {
    if returns_alias_param {
        func.instruction(&Instruction::LocalTee(out));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    } else {
        func.instruction(&Instruction::LocalSet(out));
    }
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
    use super::collect_live_object_locals_for_call;
    use crate::wasm::frame_locals::WasmLiteralPayload;
    use crate::wasm::{WasmFrameLocalKind, WasmFrameLocals, WasmFrameSyntheticLocal};
    use std::collections::BTreeMap;

    #[test]
    fn call_retention_uses_typed_local_kind_not_name_shape() {
        let mut locals = WasmFrameLocals::new();
        locals.insert("__molt_tmp0".to_string(), 0);
        locals.insert("payload_ptr".to_string(), 1);
        locals.insert("__multi_ret_0".to_string(), 2);
        locals.insert(WasmFrameLocals::NONE_NAME.to_string(), 3);

        let last_use_local = BTreeMap::from([
            ("__molt_tmp0".to_string(), 10),
            ("payload_ptr".to_string(), 10),
            ("__multi_ret_0".to_string(), 10),
            (WasmFrameLocals::NONE_NAME.to_string(), 10),
        ]);

        assert_eq!(
            collect_live_object_locals_for_call(&locals, &last_use_local, 0, None),
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
            WasmLiteralPayload::String,
            true,
            &mut local_types,
            &mut local_count,
        );
        locals.ensure_multi_return_callee_value(0, &mut local_types, &mut local_count);
        locals.ensure_multi_return_call_value("pair", 0, &mut local_types, &mut local_count);

        let last_use_local = BTreeMap::from([
            ("value".to_string(), 10),
            ("__molt_tmp0".to_string(), 10),
            ("payload_ptr".to_string(), 10),
            ("payload_len".to_string(), 10),
            ("__multi_ret_0".to_string(), 10),
            ("__multi_call_pair_0".to_string(), 10),
        ]);

        assert_eq!(
            collect_live_object_locals_for_call(&locals, &last_use_local, 0, None),
            vec![0]
        );
    }
}
