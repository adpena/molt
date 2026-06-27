use super::*;

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
        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::function_frame::WasmLiteralPayload;

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

pub(super) fn release_live_object_locals(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    live_object_locals: &[u32],
) {
    for local_idx in live_object_locals.iter().rev() {
        func.instruction(&Instruction::LocalGet(*local_idx));
        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
    }
}
