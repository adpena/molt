mod depth;

pub(in crate::wasm) use self::depth::{
    ControlKind, has_non_linear_control_flow, loop_break_depth, loop_continue_depth,
};

pub(in crate::wasm) fn dispatch_control_panic(
    function_name: &str,
    op_idx: usize,
    message: impl std::fmt::Display,
) -> ! {
    panic!("invalid WASM dispatch control in function `{function_name}` op {op_idx}: {message}")
}

#[cfg(test)]
mod tests;
