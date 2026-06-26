use super::*;

#[test]
fn drain_cleanup_entry_tracked_can_skip_named_value() {
    let mut names = vec!["callee".to_string(), "other".to_string()];
    let mut entry_vars = BTreeMap::new();
    let callee = Value::from_u32(11);
    let other = Value::from_u32(22);
    entry_vars.insert("callee".to_string(), callee);
    entry_vars.insert("other".to_string(), other);
    let last_use = BTreeMap::from([
        ("callee".to_string(), 5usize),
        ("other".to_string(), 5usize),
    ]);
    let alias_roots = BTreeMap::new();
    let mut already_decrefed = BTreeSet::new();

    let cleanup = drain_cleanup_entry_tracked(
        &mut names,
        &mut entry_vars,
        &last_use,
        &alias_roots,
        &mut already_decrefed,
        5,
        Some("callee"),
    );

    assert_eq!(cleanup, vec![other]);
    assert_eq!(names, vec!["callee".to_string()]);
    assert!(entry_vars.contains_key("callee"));
    assert!(!entry_vars.contains_key("other"));
}

#[test]
fn authority_disabled_tracked_drain_clears_without_cleanup() {
    let mut names = vec!["dead".to_string()];
    let last_use = BTreeMap::from([("dead".to_string(), 1usize)]);
    let alias_roots = BTreeMap::new();
    let mut already_decrefed = BTreeSet::new();

    let cleanup = drain_cleanup_tracked_dedup_with_authority(
        false,
        &mut names,
        &last_use,
        &alias_roots,
        1,
        None,
        Some(&mut already_decrefed),
    );

    assert!(cleanup.is_empty());
    assert!(names.is_empty());
    assert!(already_decrefed.is_empty());
}

#[test]
fn authority_disabled_entry_drain_clears_without_cleanup() {
    let mut names = vec!["dead".to_string()];
    let mut entry_vars = BTreeMap::from([("dead".to_string(), Value::from_u32(17))]);
    let last_use = BTreeMap::from([("dead".to_string(), 1usize)]);
    let alias_roots = BTreeMap::new();
    let mut already_decrefed = BTreeSet::new();

    let cleanup = drain_cleanup_entry_tracked_with_authority(
        false,
        &mut names,
        &mut entry_vars,
        &last_use,
        &alias_roots,
        &mut already_decrefed,
        1,
        None,
    );

    assert!(cleanup.is_empty());
    assert!(names.is_empty());
    assert!(entry_vars.is_empty());
    assert!(already_decrefed.is_empty());
}
