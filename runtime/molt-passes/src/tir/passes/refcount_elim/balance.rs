use crate::tir::op_kinds_generated::{RefcountBalanceRole, opcode_refcount_balance_role_table};
use crate::tir::ops::OpCode;

pub(super) fn refcount_balance_role(opcode: OpCode) -> RefcountBalanceRole {
    opcode_refcount_balance_role_table(opcode)
}
