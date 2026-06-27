use super::*;

pub(super) fn collect_live_object_locals_for_call(
    locals: &BTreeMap<String, u32>,
    last_use_local: &BTreeMap<String, usize>,
    rel_idx: usize,
    out_name: Option<&String>,
) -> Vec<u32> {
    let mut live = BTreeSet::new();
    for (name, &local_idx) in locals {
        if name == "none" {
            continue;
        }
        if out_name.is_some_and(|out| out == name) {
            continue;
        }
        if name.starts_with("__molt_tmp") || name.ends_with("_ptr") || name.ends_with("_len") {
            continue;
        }
        if last_use_local.get(name).is_none_or(|last| *last <= rel_idx) {
            continue;
        }
        live.insert(local_idx);
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
