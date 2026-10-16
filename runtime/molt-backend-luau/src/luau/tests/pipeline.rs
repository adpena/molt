//! The shared SimpleIR pipeline (design 76) on the Luau target, executed.
use super::*;

fn op(kind: &str, out: Option<&str>, value: Option<i64>, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        out: out.map(str::to_string),
        value,
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..OpIR::default()
    }
}

/// `compute()` returns `6 * 7 + 1`, which constant folding reduces to `43`.
fn folding_program() -> SimpleIR {
    SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".to_string(),
                ops: vec![op("ret_void", None, None, &[])],
                ..FunctionIR::default()
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "compute".to_string(),
                ops: vec![
                    op("const", Some("a"), Some(6), &[]),
                    op("const", Some("b"), Some(7), &[]),
                    op("mul", Some("c"), None, &["a", "b"]),
                    op("const", Some("d"), Some(1), &[]),
                    op("add", Some("e"), None, &["c", "d"]),
                    op("ret", None, None, &["e"]),
                ],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    }
}

fn compute_kinds(ir: &SimpleIR) -> Vec<&str> {
    ir.functions[1]
        .ops
        .iter()
        .map(|op| op.kind.as_str())
        .collect()
}

#[test]
fn lune_oracle_shared_pipeline_folding_admits_and_executes_exact_integer_programs() {
    let plain = folding_program();
    // Without the pipeline Luau cannot prove the integer domain of `mul`.
    let refusal = LuauBackend::new()
        .compile_checked(&plain)
        .expect_err("unfolded integer arithmetic is outside the Luau contract");
    assert!(
        refusal.contains("lacks arbitrary-precision integers"),
        "{refusal}"
    );

    let mut optimized = plain.clone();
    molt_tir::passes::run_simple_ir_pipeline(
        &mut optimized,
        &crate::tir::target_info::TargetInfo::luau_release_fast(),
    );
    // Constant folding ran on the Luau target: no arithmetic op survives, and
    // the folded literal is exact on Luau.
    assert!(
        !compute_kinds(&optimized)
            .iter()
            .any(|kind| matches!(*kind, "mul" | "add")),
        "{:?}",
        compute_kinds(&optimized)
    );
    let compiled = LuauBackend::new()
        .compile_checked(&optimized)
        .expect("folded program is inside the Luau contract");
    // CPython: 6 * 7 + 1 == 43.
    let source = format!(
        "{compiled}\n;(function()\nassert(compute() == 43)\nprint(\"luau-pipeline-\" .. tostring(compute()))\nend)()\n"
    );
    execute_lune_oracle(
        "pipeline_folded",
        &source,
        LuneStdout::Contains(&["luau-pipeline-43"]),
    );
}
