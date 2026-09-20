use super::*;

#[test]
fn cleanup_candidates_can_skip_named_value() {
    let mut names = vec!["callee".into(), "other".into()];
    let uses = BTreeMap::from([("callee".into(), 5), ("other".into(), 5)]);
    let cleanup = drain_cleanup_candidates(
        NativeRcAuthority::NativeValueTracking,
        &mut names,
        &uses,
        5,
        Some("callee"),
    );
    assert_eq!(cleanup, vec!["other"]);
    assert_eq!(names, vec!["callee"]);
}

#[test]
fn authority_disabled_tracked_drain_clears_without_cleanup() {
    let mut names = vec!["dead".into()];
    let uses = BTreeMap::from([("dead".into(), 1)]);
    assert!(
        drain_cleanup_candidates(
            NativeRcAuthority::TirDropInsertion,
            &mut names,
            &uses,
            1,
            None
        )
        .is_empty()
    );
    assert!(names.is_empty());
}

#[test]
fn sibling_candidate_inventories_do_not_share_release_state() {
    let names = vec!["dead".into()];
    let uses = BTreeMap::from([("dead".into(), 1)]);
    for mut sibling in [names.clone(), names] {
        assert_eq!(
            drain_cleanup_candidates(
                NativeRcAuthority::NativeValueTracking,
                &mut sibling,
                &uses,
                1,
                None
            ),
            vec!["dead"]
        );
    }
}

#[test]
fn authority_from_drop_inserted_selects_single_rc_owner() {
    assert_eq!(
        NativeRcAuthority::from_drop_inserted(true),
        NativeRcAuthority::TirDropInsertion
    );
    assert_eq!(
        NativeRcAuthority::from_drop_inserted(false),
        NativeRcAuthority::NativeValueTracking
    );
    assert!(!NativeRcAuthority::TirDropInsertion.native_value_tracking_enabled());
    assert!(NativeRcAuthority::NativeValueTracking.native_value_tracking_enabled());
}
