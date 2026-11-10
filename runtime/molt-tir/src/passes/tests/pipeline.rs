use super::*;

fn op(kind: molt_ir::OpKind, out: Option<&str>, s_value: Option<&str>, args: &[&str]) -> OpIR {
    OpIR {
        kind,
        out: out.map(str::to_string),
        s_value: s_value.map(str::to_string),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..OpIR::new(kind)
    }
}

/// `return 2 + 3` in the form `fold_constants` rewrites to one constant.
fn constant_sum_program(is_extern: bool) -> SimpleIR {
    let constant = |out: &str, value: i64| OpIR {
        value: Some(value),
        ..op(molt_ir::OpKind::Const, Some(out), None, &[])
    };
    let function = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "f".to_string(),
        params: Vec::new(),
        ops: vec![
            constant("a", 2),
            constant("b", 3),
            op(molt_ir::OpKind::Add, Some("r"), None, &["a", "b"]),
            op(molt_ir::OpKind::Ret, None, None, &["r"]),
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

fn kinds(ir: &SimpleIR) -> Vec<molt_ir::OpKind> {
    ir.functions[0].ops.iter().map(|op| op.kind).collect()
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
    // targets without an async runtime run the passes that only remove ops or
    // fold them to constants.
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
fn pipeline_leaves_extern_functions_untouched() {
    let mut defined = constant_sum_program(false);
    run_simple_ir_pipeline(&mut defined, &TargetInfo::native_release_fast());
    assert_eq!(
        kinds(&defined),
        [
            molt_ir::OpKind::Const,
            molt_ir::OpKind::Const,
            molt_ir::OpKind::Const,
            molt_ir::OpKind::Ret
        ]
    );

    let original = constant_sum_program(true);
    let mut ir = original.clone();
    run_simple_ir_pipeline(&mut ir, &TargetInfo::native_release_fast());
    assert_eq!(kinds(&ir), kinds(&original));
}

/// Both integer folders fold an exact `+`, `-` or `*` and keep one whose
/// Python result leaves i64. Each overflowing case wraps to an inline int in
/// i64 (-2, 1, 0, -2**18, 0), so a wrapping fold would yield a wrong small int.
#[test]
fn integer_folds_keep_ops_whose_result_leaves_i64() {
    use molt_ir::OpKind::{Add, InplaceAdd, InplaceMul, InplaceSub, Mul, Sub};
    let cases = [
        (Add, i64::MAX, i64::MAX),
        (Sub, i64::MIN, i64::MAX),
        (Mul, 1 << 40, 1 << 40),
        (Mul, (1 << 46) - 1, 1 << 18),
        (InplaceMul, -(1 << 46), 1 << 18),
        (InplaceSub, -(1 << 46), 1 << 46),
        (InplaceAdd, (1 << 46) - 1, (1 << 46) - 1),
        (Mul, 3_037_000_499, 3_037_000_499),
    ];
    let folders: [(&str, fn(&mut [OpIR])); 2] = [
        ("fold_constants", fold_constants),
        ("fold_constants_cross_block", fold_constants_cross_block),
    ];
    for (folder, fold) in folders {
        for (kind, a, b) in cases {
            let constant = |out: &str, value: i64| OpIR {
                value: Some(value),
                ..op(molt_ir::OpKind::Const, Some(out), None, &[])
            };
            let mut ops = vec![
                constant("a", a),
                constant("b", b),
                op(kind, Some("r"), None, &["a", "b"]),
                op(molt_ir::OpKind::Ret, None, None, &["r"]),
            ];
            fold(&mut ops);
            let exact = match kind {
                Add | InplaceAdd => i128::from(a) + i128::from(b),
                Sub | InplaceSub => i128::from(a) - i128::from(b),
                _ => i128::from(a) * i128::from(b),
            };
            let result = &ops[2];
            match i64::try_from(exact) {
                Ok(value) => assert_eq!(
                    (result.kind, result.value),
                    (molt_ir::OpKind::Const, Some(value)),
                    "{folder}: {kind} {a} {b} folds to its exact value"
                ),
                Err(_) => assert_eq!(
                    (result.kind, result.args.as_deref()),
                    (kind, Some(&["a".to_string(), "b".to_string()][..])),
                    "{folder}: {kind} {a} {b} leaves i64 and stays"
                ),
            }
        }
    }
}
