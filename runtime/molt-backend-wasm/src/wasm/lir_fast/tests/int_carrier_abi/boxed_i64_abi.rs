use super::super::*;

/// On the production boxed-i64 ABI path, a function whose integer params are
/// proven `RawI64Safe` keeps the fast path (entry args lower to `I64`); a
/// `MaybeBigInt` param forces the entry arg to `DynBox`, so the boxed-i64 ABI
/// (which requires all-`I64` entry args) bails to `None` — falling back to
/// the IntFastLane-guarded slow path. This is the structural gate that keeps
/// the unsound bare op un-emittable for unproven ints.
#[test]
fn boxed_i64_abi_bails_when_param_is_maybe_bigint() {
    let proven = make_add_two_consts_func(20, 22);
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&proven).is_some(),
        "range-proven raw-i64 values keep the boxed-i64 ABI fast path"
    );

    let unproven = make_add_two_params_func();
    assert!(
        lower_tir_to_wasm_boxed_i64_abi(&unproven).is_none(),
        "a MaybeBigInt param must bail the boxed-i64 ABI (entry arg is DynBox)"
    );
}
