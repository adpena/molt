use super::*;

#[test]
fn cleanup_roots_collapse_join_alias_duplicates() {
    let func = FunctionIR {
        name: "join_alias_cleanup".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const_str".to_string(),
                out: Some("src".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                args: Some(vec!["src".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy_var".to_string(),
                var: Some("joined".to_string()),
                out: Some("arg_alias".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                s_value: Some("callee".to_string()),
                args: Some(vec!["arg_alias".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("out".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    let arg_cleanup_roots =
        cleanup_roots_for_names(&analysis.alias_roots, ["arg_alias".to_string()]);

    assert_eq!(arg_cleanup_roots, BTreeSet::from(["src".to_string()]));
    assert!(arg_cleanup_roots.contains(alias_root_name(&analysis.alias_roots, "_bb4_arg0")));
    assert!(arg_cleanup_roots.contains(alias_root_name(&analysis.alias_roots, "joined")));
}

#[test]
fn cleanup_root_marking_dedups_aliases() {
    let alias_roots = BTreeMap::from([
        ("alias".to_string(), "root".to_string()),
        ("join".to_string(), "root".to_string()),
    ]);
    let mut already_decrefed = BTreeSet::new();

    assert!(mark_cleanup_root_once(
        &alias_roots,
        &mut already_decrefed,
        "alias",
    ));
    assert!(!mark_cleanup_root_once(
        &alias_roots,
        &mut already_decrefed,
        "join",
    ));
    assert!(!mark_cleanup_root_once(
        &alias_roots,
        &mut already_decrefed,
        "root",
    ));
    assert_eq!(already_decrefed, BTreeSet::from(["root".to_string()]));
}

#[test]
fn protected_cleanup_rearms_preserved_alias_root() {
    let alias_roots = BTreeMap::from([("phi_in".to_string(), "src".to_string())]);
    let protected = BTreeSet::from(["phi_in"]);
    let cleanup = vec!["phi_in".to_string(), "dead".to_string()];
    let mut carry = Vec::new();
    let mut already_decrefed = BTreeSet::from(["src".to_string(), "dead".to_string()]);

    let actual = protect_cleanup_names(
        &mut carry,
        cleanup,
        &protected,
        &alias_roots,
        &mut already_decrefed,
    );

    assert_eq!(carry, vec!["phi_in".to_string()]);
    assert_eq!(actual, vec!["dead".to_string()]);
    assert!(!already_decrefed.contains("src"));
    assert!(already_decrefed.contains("dead"));
}
