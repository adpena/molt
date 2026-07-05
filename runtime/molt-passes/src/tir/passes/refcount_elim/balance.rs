use crate::tir::op_kinds_generated::{
    RefcountBalanceRole, opcode_is_refcount_heap_exposure_table, opcode_refcount_balance_role_table,
};
use crate::tir::ops::OpCode;

/// Returns `true` if the opcode causes its operands to have heap exposure.
pub(super) fn is_heap_exposing(opcode: OpCode) -> bool {
    opcode_is_refcount_heap_exposure_table(opcode)
}

pub(super) fn refcount_balance_role(opcode: OpCode) -> RefcountBalanceRole {
    opcode_refcount_balance_role_table(opcode)
}

pub(super) fn is_refcount_balance_op(opcode: OpCode) -> bool {
    refcount_balance_role(opcode).is_refcount_balance()
}

pub(super) fn complementary_refcount_opcode(opcode: OpCode) -> Option<OpCode> {
    refcount_balance_role(opcode).complementary_opcode()
}
