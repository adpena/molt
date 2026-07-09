use super::super::super::super::class_def_layout::ClassDefLayout;
use super::super::super::super::context::CompileFuncContext;
use super::super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_class_new(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let name = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(name));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassNew],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_class_def(
    func: &mut Function,
    op: &OpIR,
    ctx: &CompileFuncContext<'_>,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let meta = op.s_value.as_deref().expect("class_def needs s_value");
    let layout = ClassDefLayout::parse(meta);

    let spill_base = ctx.class_def_spill_offset;
    let attrs_base = layout.attrs_base_offset(spill_base);
    let attrs_start = layout.attrs_start_arg_index();

    // `class_def` spills boxed handles through shared linear memory before the
    // runtime helper snapshots them. Pin every handle across that helper call.
    for arg_name in args {
        let arg = locals[arg_name];
        func.instruction(&Instruction::LocalGet(arg));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    }

    for (i, base_name) in args[1..1 + layout.nbases()].iter().enumerate() {
        let base = locals[base_name];
        func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
        func.instruction(&Instruction::LocalGet(base));
        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
    }

    for i in 0..layout.nattrs() {
        let key = locals[&args[attrs_start + i * 2]];
        let val = locals[&args[attrs_start + i * 2 + 1]];
        func.instruction(&Instruction::I32Const(
            (attrs_base + (i as u32) * 16) as i32,
        ));
        func.instruction(&Instruction::LocalGet(key));
        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::I32Const(
            (attrs_base + (i as u32) * 16 + 8) as i32,
        ));
        func.instruction(&Instruction::LocalGet(val));
        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
    }

    let name = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(name));
    func.instruction(&Instruction::I32Const(spill_base as i32));
    func.instruction(&Instruction::I64ExtendI32U);
    func.instruction(&Instruction::I64Const(layout.nbases() as i64));
    func.instruction(&Instruction::I32Const(attrs_base as i32));
    func.instruction(&Instruction::I64ExtendI32U);
    func.instruction(&Instruction::I64Const(layout.nattrs() as i64));
    func.instruction(&Instruction::I64Const(layout.layout_size()));
    func.instruction(&Instruction::I64Const(layout.layout_version()));
    func.instruction(&Instruction::I64Const(layout.flags()));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::GuardedClassDef],
    );
    store_result_or_drop(func, op, locals);
    for arg_name in args.iter().rev() {
        let arg = locals[arg_name];
        func.instruction(&Instruction::LocalGet(arg));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
        );
    }
}

pub(super) fn emit_class_set_base(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    let base_bits = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(class_bits));
    func.instruction(&Instruction::LocalGet(base_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassSetBase],
    );
    store_result_or_drop(func, op, locals);
}

pub(super) fn emit_class_apply_set_name(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    let class_bits = locals[&args[0]];
    func.instruction(&Instruction::LocalGet(class_bits));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ClassApplySetName],
    );
    store_result_or_drop(func, op, locals);
}
