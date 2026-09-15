use crate::FunctionIR;
use crate::tir::op_kinds_generated::{
    kind_to_opcode_table, opcode_may_throw_table, simpleir_kind_is_block_ender,
    simpleir_kind_is_block_leader, simpleir_kind_is_exception_check,
};
use crate::tir::{IntRange, OpCode};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_safe_exception_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_EXC_ELIDE").is_ok() {
        return;
    }
    // Only an executed check establishes a clean fallthrough. A non-throwing
    // predecessor alone says nothing about earlier unchecked operations or
    // another incoming CFG edge. Polling checks also service Python callbacks
    // and must execute even when the incoming exception state is known clean.
    let mut may_be_pending = true;
    func_ir.ops.retain(|op| {
        if simpleir_kind_is_exception_check(&op.kind) {
            let has_exception_edge = op.value.is_some();
            let unused_result = op.out.as_deref().is_none_or(|name| name == "none");
            let keep = may_be_pending || op.is_async_work_poll() || !unused_result;
            if has_exception_edge {
                may_be_pending = false;
            } else {
                // An observation without a handler edge cannot prove clean
                // fallthrough, and a fused poll can introduce a new failure.
                may_be_pending |= op.is_async_work_poll();
            }
            return keep || !has_exception_edge;
        }
        if simpleir_kind_is_block_leader(&op.kind) || simpleir_kind_is_block_ender(&op.kind) {
            may_be_pending = true;
        } else if let Some(opcode) = kind_to_opcode_table(&op.kind) {
            // SimpleIR may carry a full-i64 integer in the boxed lane. The TIR
            // ConstInt opcode is raw, but its final boxed transport can allocate.
            let boxed_integer = opcode == OpCode::ConstInt
                && !op
                    .value
                    .is_some_and(|value| IntRange::point(value).fits_inline_int47());
            may_be_pending |=
                opcode_may_throw_table(opcode) || boxed_integer || op.is_async_work_poll();
        } else {
            // Unknown/preserved operations do not establish an exception proof.
            may_be_pending = true;
        }
        true
    });
}
