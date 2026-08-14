use super::super::super::op_loop::WasmFunctionEmitContext;
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use super::super::state_remap::{build_sparse_state_remap_entries, emit_sparse_state_remap_lookup};
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_values::POINTER_MASK;
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_stateful_resume_prelude(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    let self_ptr_local = locals
        .self_ptr_local
        .expect("self ptr local missing for stateful wasm");
    let self_param = *op_emitter
        .locals()
        .get(WasmFrameLocals::SELF_PARAM_NAME)
        .expect("self_param missing for stateful wasm");
    let self_local = *op_emitter
        .locals()
        .get("self")
        .expect("self local missing for stateful wasm");
    let resume = plan
        .state_resume
        .as_ref()
        .expect("state resume maps missing for stateful wasm");
    let state_remap_table_entries = resume.remap_table.as_ref().map(|(entries, _)| *entries);
    let sparse_state_remap_entries = state_remap_table_entries
        .is_none()
        .then(|| build_sparse_state_remap_entries(&resume.state_map));

    func.instruction(&Instruction::LocalGet(self_param));
    func.instruction(&Instruction::LocalSet(self_ptr_local));

    func.instruction(&Instruction::LocalGet(self_param));
    func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
    func.instruction(&Instruction::I64And);
    op_emitter.const_cache().emit_qnan_tag_ptr(func);
    func.instruction(&Instruction::I64Or);
    func.instruction(&Instruction::LocalSet(self_local));

    func.instruction(&Instruction::LocalGet(self_ptr_local));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjGetState],
    );
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(-1));
    func.instruction(&Instruction::I64Xor);
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::Else);
    if let Some(remap_entries) = state_remap_table_entries {
        let remap_base_local = locals
            .state_remap_base_local
            .expect("state remap base local missing for stateful wasm");
        let remap_value_local = locals
            .state_remap_value_local
            .expect("state remap value local missing for stateful wasm");
        func.instruction(&Instruction::LocalGet(locals.state_local));
        func.instruction(&Instruction::I64Const(remap_entries));
        func.instruction(&Instruction::I64LtU);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(remap_base_local));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(locals.state_local));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::I32Const(8));
        func.instruction(&Instruction::I32Mul);
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalSet(remap_value_local));
        func.instruction(&Instruction::LocalGet(remap_value_local));
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::I64GeS);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(remap_value_local));
        func.instruction(&Instruction::LocalSet(locals.state_local));
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::End);
    } else {
        emit_sparse_state_remap_lookup(
            func,
            locals.state_local,
            sparse_state_remap_entries
                .as_deref()
                .expect("sparse state remap entries missing for stateful wasm"),
        );
    }
    func.instruction(&Instruction::End);

    // Every host poll invocation must execute the real SimpleIR entry prefix
    // through the ordinary stateful dispatcher: that prefix may contain
    // exception checks and other control edges, so replaying it as a plain
    // instruction slice is unsound. Preserve the remapped target separately,
    // start dispatch at operation zero, and let state_switch transfer to the
    // saved initial/resume target after the prefix has seeded its block-arg
    // carriers.
    let resume_state_local = locals
        .resume_state_local
        .expect("resume state local missing for stateful wasm");
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::LocalSet(resume_state_local));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::LocalSet(locals.state_local));
}
