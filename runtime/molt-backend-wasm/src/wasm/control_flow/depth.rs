use crate::OpIR;

#[derive(Clone, Copy)]
pub(in crate::wasm) enum ControlKind {
    Block,
    Loop,
    If,
    Try,
}

pub(in crate::wasm) fn loop_break_depth(control_stack: &[ControlKind]) -> Option<u32> {
    let mut found_loop = false;
    for (depth, entry) in control_stack.iter().rev().enumerate() {
        match entry {
            ControlKind::Block if found_loop => return Some(depth as u32),
            ControlKind::Loop => {
                found_loop = true;
            }
            _ => {}
        }
    }
    None
}

pub(in crate::wasm) fn loop_continue_depth(control_stack: &[ControlKind]) -> Option<u32> {
    for (depth, entry) in control_stack.iter().rev().enumerate() {
        if matches!(entry, ControlKind::Loop) {
            return Some(depth as u32);
        }
    }
    None
}

pub(in crate::wasm) fn has_non_linear_control_flow(ops: &[OpIR]) -> bool {
    ops.iter().any(|op| op.kind.is_wasm_split_barrier())
}
