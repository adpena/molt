use super::*;

fn op(kind: &str, out: Option<&str>, s_value: Option<&str>, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        out: out.map(str::to_string),
        s_value: s_value.map(str::to_string),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..Default::default()
    }
}

/// `recv.compute(x)` in the getattr/callargs/call_bind form method fusion rewrites.
fn method_call_program(is_extern: bool) -> SimpleIR {
    let function = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "f".to_string(),
        params: vec!["recv".to_string(), "x".to_string()],
        ops: vec![
            op(
                "get_attr_generic_ptr",
                Some("t"),
                Some("compute"),
                &["recv"],
            ),
            op("check_exception", None, None, &[]),
            op("callargs_new", Some("ca"), None, &[]),
            op("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
            op("call_bind", Some("r"), None, &["t", "ca"]),
            op("ret", None, None, &["r"]),
        ],
        param_types: None,
        source_file: None,
        is_extern,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    SimpleIR {
        functions: vec![function],
        profile: None,
    }
}

fn kinds(ir: &SimpleIR) -> Vec<&str> {
    ir.functions[0]
        .ops
        .iter()
        .map(|op| op.kind.as_str())
        .collect()
}

fn applied(target: &TargetInfo) -> Vec<&'static str> {
    SIMPLE_IR_FUNCTION_PASSES
        .iter()
        .filter(|pass| pass.applies_to(target))
        .map(|pass| pass.name)
        .collect()
}

const TARGET_NEUTRAL: [&str; 7] = [
    "eliminate_unbound_local_checks",
    "eliminate_redundant_guard_tags",
    "elide_dead_struct_allocs",
    "rc_coalescing",
    "fold_constants",
    "fold_constants_cross_block",
    "elide_safe_exception_checks",
];

#[test]
fn pipeline_gates_follow_each_targets_contract() {
    // Design 76: native Cranelift lowers every shape; WASM and LLVM run all
    // but escape analysis, whose marks only Cranelift scalarizes; source
    // targets without an async runtime or inline caches run the passes that
    // only remove ops or fold them to constants.
    let all: Vec<&str> = SIMPLE_IR_FUNCTION_PASSES
        .iter()
        .map(|pass| pass.name)
        .collect();
    assert_eq!(applied(&TargetInfo::native_release_fast()), all);
    let without_escape: Vec<&str> = all
        .iter()
        .copied()
        .filter(|name| *name != "escape_analysis")
        .collect();
    assert_eq!(applied(&TargetInfo::wasm_release_fast()), without_escape);
    assert_eq!(applied(&TargetInfo::llvm_release_fast()), without_escape);
    for target in [
        TargetInfo::luau_release_fast(),
        TargetInfo::rust_release_fast(),
        TargetInfo::mlir_release_fast(),
    ] {
        assert_eq!(applied(&target), TARGET_NEUTRAL, "{:?}", target.target);
    }
}

#[test]
fn every_pass_classifies_the_op_kinds_it_introduces() {
    for pass in SIMPLE_IR_FUNCTION_PASSES {
        assert!(
            pass.introduced_requirements().is_some(),
            "{} introduces an op kind the generated runtime authority does not classify",
            pass.name
        );
    }
}

#[test]
fn method_fusion_runs_only_where_the_backend_lowers_inline_caches() {
    let mut native = method_call_program(false);
    run_simple_ir_pipeline(&mut native, &TargetInfo::native_release_fast());
    assert!(
        kinds(&native).contains(&"call_method_ic"),
        "{:?}",
        kinds(&native)
    );

    let mut luau = method_call_program(false);
    run_simple_ir_pipeline(&mut luau, &TargetInfo::luau_release_fast());
    assert!(
        !kinds(&luau).contains(&"call_method_ic"),
        "{:?}",
        kinds(&luau)
    );
    assert!(kinds(&luau).contains(&"call_bind"), "{:?}", kinds(&luau));
}

#[test]
fn pipeline_leaves_extern_functions_untouched() {
    let original = method_call_program(true);
    let mut ir = original.clone();
    run_simple_ir_pipeline(&mut ir, &TargetInfo::native_release_fast());
    assert_eq!(kinds(&ir), kinds(&original));
}
