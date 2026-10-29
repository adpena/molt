use super::*;

#[test]
fn shared_drop_fact_marker_set_is_explicit_for_wasm() {
    assert!(is_shared_drop_fact_marker(molt_ir::OpKind::DropInserted));
    assert!(is_shared_drop_fact_marker(
        molt_ir::OpKind::ExceptionRegionDropsInserted
    ));
    assert!(!is_shared_drop_fact_marker(molt_ir::OpKind::IncRef));
    assert!(!is_shared_drop_fact_marker(molt_ir::OpKind::DecRef));
    assert!(!is_shared_drop_fact_marker(molt_ir::OpKind::Release));
}
