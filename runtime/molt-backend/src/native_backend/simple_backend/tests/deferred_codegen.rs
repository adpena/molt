use super::*;

#[test]
fn deferred_codegen_flush_predicate_bounds_function_and_op_retention() {
    assert!(!should_flush_deferred_codegen(
        0,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET
    ));
    assert!(!should_flush_deferred_codegen(
        DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT - 1,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET - 1
    ));
    assert!(should_flush_deferred_codegen(
        DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT,
        1
    ));
    assert!(should_flush_deferred_codegen(
        1,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET
    ));
}
