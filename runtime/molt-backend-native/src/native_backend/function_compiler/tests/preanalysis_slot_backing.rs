use super::*;

#[test]
fn slot_backed_join_names_skip_load_only_phi_join_carriers() {
    let ops = vec![
        OpIR {
            kind: "phi".to_string(),
            out: Some("joined".to_string()),
            args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(18),
            ..OpIR::default()
        },
        OpIR {
            kind: "load_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            out: Some("joined".to_string()),
            ..OpIR::default()
        },
    ];

    let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

    assert!(
        !names.contains("_bb4_arg0"),
        "load-only structured phi join carriers must stay on the SSA path",
    );
}

#[test]
fn slot_backed_join_names_keep_explicit_store_backed_join_carriers() {
    let ops = vec![
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            args: Some(vec!["src".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(18),
            ..OpIR::default()
        },
        OpIR {
            kind: "load_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            out: Some("joined".to_string()),
            ..OpIR::default()
        },
    ];

    let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

    assert!(
        names.contains("_bb4_arg0"),
        "explicit store-backed join carriers must remain slot-backed",
    );
}

#[test]
fn exception_slot_backing_ignores_compiler_value_temps() {
    let ops = vec![
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("slot".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_v7".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("v116".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_v8".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("handler_slot".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_pop".to_string(),
            ..OpIR::default()
        },
    ];
    let exception_labels = BTreeSet::from([7]);

    let names = collect_slot_backed_join_names(&ops, &exception_labels, false);

    assert!(names.contains("_bb4_arg0"));
    assert!(names.contains("slot"));
    assert!(names.contains("handler_slot"));
    for temp in ["_v7", "v116", "_v8"] {
        assert!(
            !names.contains(temp),
            "compiler value temp {temp} must not become exception slot-backed"
        );
    }
}
