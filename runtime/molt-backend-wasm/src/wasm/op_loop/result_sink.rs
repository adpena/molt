use super::*;

pub(super) fn store_result_or_drop(func: &mut Function, op: &OpIR, locals: &BTreeMap<String, u32>) {
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

pub(super) fn store_non_none_result_or_drop(
    func: &mut Function,
    op: &OpIR,
    locals: &BTreeMap<String, u32>,
) {
    if let Some(out) = op.out.as_ref()
        && out != "none"
    {
        func.instruction(&Instruction::LocalSet(locals[out]));
    } else {
        func.instruction(&Instruction::Drop);
    }
}
