use super::*;

fn op_with(kind: &str, out: Option<&str>, s_value: Option<&str>, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        out: out.map(str::to_string),
        s_value: s_value.map(str::to_string),
        args: if args.is_empty() {
            None
        } else {
            Some(args.iter().map(|a| a.to_string()).collect())
        },
        ..Default::default()
    }
}

fn assert_source_site(op: &OpIR, line: i64, col: i64, end_col: i64) {
    assert_eq!(op.source_line, Some(line), "source_line for {}", op.kind);
    assert_eq!(op.col_offset, Some(col), "col_offset for {}", op.kind);
    assert_eq!(
        op.end_col_offset,
        Some(end_col),
        "end_col_offset for {}",
        op.kind
    );
}

#[test]
fn split_field_deforestation_preserves_source_site() {
    let mut field = op_with(
        "string_split_field",
        Some("field"),
        None,
        &["hay", "sep", "idx"],
    );
    field.source_line = Some(31);
    field.col_offset = Some(2);
    field.end_col_offset = Some(12);
    let mut len = op_with("len", Some("n"), None, &["field"]);
    len.source_line = Some(32);
    len.col_offset = Some(4);
    len.end_col_offset = Some(9);
    let mut func = FunctionIR {
        name: "f".to_string(),
        params: vec![],
        ops: vec![field, op_with("check_exception", None, None, &[]), len],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    deforest_split_field_reads(&mut func);

    let start = func
        .ops
        .iter()
        .find(|op| op.kind == "string_split_field_start")
        .unwrap_or_else(|| {
            panic!(
                "field source op should be split into property reads: {:?}",
                func.ops
            )
        });
    assert_source_site(start, 31, 2, 12);
    let len_from_bounds = func
        .ops
        .iter()
        .find(|op| op.kind == "string_split_field_len_from_bounds")
        .expect("len consumer should be rewritten through bounds");
    assert_source_site(len_from_bounds, 32, 4, 9);
}

#[test]
fn fuse_method_dispatch_rewrites_getattr_call_idiom() {
    // get_attr_generic_ptr(recv, "compute") -> callargs -> call_bind
    // must collapse to a single call_method_ic(recv, x) op.
    let mut func = FunctionIR {
        name: "f".to_string(),
        params: vec!["recv".to_string(), "x".to_string()],
        ops: vec![
            op_with(
                "get_attr_generic_ptr",
                Some("t"),
                Some("compute"),
                &["recv"],
            ),
            op_with("check_exception", None, None, &[]),
            op_with("callargs_new", Some("ca"), None, &[]),
            op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
            op_with("call_bind", Some("r"), None, &["t", "ca"]),
            op_with("ret", None, None, &["r"]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    func.ops[4].source_line = Some(44);
    func.ops[4].col_offset = Some(6);
    func.ops[4].end_col_offset = Some(19);
    fuse_method_dispatch(&mut func);
    // The getattr / callargs_new / callargs_push_pos / call_bind quartet
    // collapses to a single call_method_ic; check_exception + ret survive.
    let fused: Vec<&str> = func.ops.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(fused, vec!["check_exception", "call_method_ic", "ret"]);
    let ic = func
        .ops
        .iter()
        .find(|o| o.kind == "call_method_ic")
        .unwrap();
    assert_eq!(ic.out.as_deref(), Some("r"));
    assert_eq!(ic.s_value.as_deref(), Some("compute"));
    assert_eq!(
        ic.args.as_ref().unwrap(),
        &vec!["recv".to_string(), "x".to_string()]
    );
    assert_source_site(ic, 44, 6, 19);
}

#[test]
fn fuse_method_dispatch_skips_multi_use_getattr() {
    // If the getattr result is used by something other than the call_bind
    // callee (here a second store_var), fusion must NOT fire (the bound
    // method escapes and its identity may be observed).
    let mut func = FunctionIR {
        name: "f".to_string(),
        params: vec!["recv".to_string(), "x".to_string()],
        ops: vec![
            op_with(
                "get_attr_generic_ptr",
                Some("t"),
                Some("compute"),
                &["recv"],
            ),
            op_with("store_var", Some("_s"), None, &["t"]),
            op_with("callargs_new", Some("ca"), None, &[]),
            op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
            op_with("call_bind", Some("r"), None, &["t", "ca"]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let before: Vec<String> = func.ops.iter().map(|o| o.kind.clone()).collect();
    fuse_method_dispatch(&mut func);
    let after: Vec<String> = func.ops.iter().map(|o| o.kind.clone()).collect();
    assert_eq!(before, after, "multi-use getattr must not be fused");
}

#[test]
fn fuse_method_dispatch_rewrites_super_idiom() {
    // super_new(class, self) -> get_attr_generic_obj -> callargs ->
    // call_indirect must collapse to call_super_method_ic(class, self, x).
    let mut func = FunctionIR {
        name: "m".to_string(),
        params: vec!["self".to_string(), "x".to_string()],
        ops: vec![
            op_with("super_new", Some("sup"), None, &["cls", "self"]),
            op_with("get_attr_generic_obj", Some("t"), Some("compute"), &["sup"]),
            op_with("callargs_new", Some("ca"), None, &[]),
            op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
            op_with("call_indirect", Some("r"), None, &["t", "ca"]),
            op_with("ret", None, None, &["r"]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    fuse_method_dispatch(&mut func);
    let kinds: Vec<&str> = func.ops.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, vec!["call_super_method_ic", "ret"]);
    let ic = func
        .ops
        .iter()
        .find(|o| o.kind == "call_super_method_ic")
        .unwrap();
    assert_eq!(ic.s_value.as_deref(), Some("compute"));
    assert_eq!(
        ic.args.as_ref().unwrap(),
        &vec!["cls".to_string(), "self".to_string(), "x".to_string()]
    );
}

#[test]
fn fuse_method_dispatch_disabled_by_env() {
    // The lever is exercised through the explicit parameter — mutating
    // the process-global env here races every concurrently-running test
    // that calls the env-reading wrapper.
    let mut func = FunctionIR {
        name: "f".to_string(),
        params: vec!["recv".to_string(), "x".to_string()],
        ops: vec![
            op_with(
                "get_attr_generic_ptr",
                Some("t"),
                Some("compute"),
                &["recv"],
            ),
            op_with("callargs_new", Some("ca"), None, &[]),
            op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
            op_with("call_bind", Some("r"), None, &["t", "ca"]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    fuse_method_dispatch_inner(&mut func, true);
    assert!(
        func.ops.iter().all(|o| o.kind != "call_method_ic"),
        "fusion must be a no-op when disabled by env"
    );
}
