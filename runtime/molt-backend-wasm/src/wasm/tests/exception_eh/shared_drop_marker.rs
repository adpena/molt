use super::*;

#[test]
fn shared_drop_fact_marker_set_is_explicit_for_wasm() {
    assert!(is_shared_drop_fact_marker("drop_inserted"));
    assert!(is_shared_drop_fact_marker(
        "exception_region_drops_inserted"
    ));
    assert!(!is_shared_drop_fact_marker("inc_ref"));
    assert!(!is_shared_drop_fact_marker("dec_ref"));
    assert!(!is_shared_drop_fact_marker("release"));
}
