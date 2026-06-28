use super::*;

pub(super) fn emit_local_slot_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "store_var" => {
            let args_names = op.args.as_ref().expect("store_var args missing");
            let src_name = args_names
                .first()
                .expect("store_var requires one source arg");
            let dst_name = op
                .var
                .as_ref()
                .or(op.out.as_ref())
                .expect("store_var requires destination");
            copy_local(func, locals, src_name, dst_name);
            true
        }
        "delete_var" => {
            let args_names = op.args.as_ref().expect("delete_var args missing");
            let missing_name = args_names
                .first()
                .expect("delete_var requires missing-sentinel operand");
            let old_name = args_names
                .get(1)
                .expect("delete_var requires old-slot operand");
            let dst_name = op
                .var
                .as_ref()
                .or(op.out.as_ref())
                .expect("delete_var requires destination");
            let _old_slot = locals[old_name];
            copy_local(func, locals, missing_name, dst_name);
            true
        }
        "load_var" | "copy_var" | "copy" | "identity_alias" | "binding_alias" => {
            let src_name = op
                .var
                .as_ref()
                .or_else(|| op.args.as_ref().and_then(|args| args.first()))
                .expect("load_var/copy_var requires source");
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let src = locals[src_name];
                func.instruction(&Instruction::LocalGet(src));
                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                copy_local(func, locals, src_name, out_name);
            }
            true
        }
        _ => false,
    }
}

fn copy_local(func: &mut Function, locals: &WasmFrameLocals, src_name: &str, dst_name: &str) {
    let src = locals[src_name];
    let dst = locals[dst_name];
    func.instruction(&Instruction::LocalGet(src));
    func.instruction(&Instruction::LocalSet(dst));
}
