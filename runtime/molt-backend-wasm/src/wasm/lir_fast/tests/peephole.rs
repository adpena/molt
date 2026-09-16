use super::*;

#[test]
fn peephole_collapses_set_get_to_tee() {
    let input = vec![
        Instruction::I64Const(42),
        Instruction::LocalSet(3),
        Instruction::LocalGet(3),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::I64Const(42)),
        "const preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalTee(3)),
        "set+get collapsed to tee"
    );
    assert!(matches!(output[2], Instruction::End), "end preserved");
}

#[test]
fn peephole_preserves_mismatched_set_get() {
    let input = vec![
        Instruction::LocalSet(1),
        Instruction::LocalGet(2), // different local
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::LocalSet(1)),
        "set preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalGet(2)),
        "get preserved"
    );
}

#[test]
fn peephole_handles_consecutive_tee_chains() {
    // Pattern: set(1) get(1) set(2) get(2) → tee(1) tee(2)
    let input = vec![
        Instruction::I64Const(10),
        Instruction::LocalSet(1),
        Instruction::LocalGet(1),
        Instruction::LocalSet(2),
        Instruction::LocalGet(2),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 4);
    assert!(matches!(output[1], Instruction::LocalTee(1)));
    assert!(matches!(output[2], Instruction::LocalTee(2)));
}

#[test]
fn peephole_empty_and_single() {
    assert!(peephole_instrs(vec![]).is_empty());
    let single = vec![Instruction::End];
    assert_eq!(peephole_instrs(single).len(), 1);
}

#[test]
fn peephole_applied_in_const_return() {
    // A const-return function should have tee instead of set+get.
    let func = make_const_return_func(99);
    let output = lower_tir_to_wasm(&func).test_view();

    // After peephole, the pattern: i64.const 99; local.set X; local.get X; return
    // becomes: i64.const 99; local.tee X; return
    let has_tee = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::LocalTee(_)));
    assert!(has_tee, "expected local.tee from peephole optimization");

    // Should have no set+get pairs for the same local.
    for window in output.instructions.windows(2) {
        if let (Instruction::LocalSet(s), Instruction::LocalGet(g)) = (&window[0], &window[1]) {
            assert_ne!(
                s, g,
                "found redundant set+get pair for local {s} that peephole should have eliminated"
            );
        }
    }
}

#[test]
fn floating_arithmetic_preserves_zero_signs_and_quiets_signaling_nans() {
    use super::execution_support::executable_module;
    use crate::wasm::body::WasmBody;
    use crate::wasm::test_execution::{
        real_execution_tool, run_node_test_script, wasm_test_temp_dir,
    };
    use std::{fs, path::PathBuf};

    let mut modules = Vec::new();
    for (name, constant, arithmetic) in [
        ("add_positive_zero", 0.0_f64, Instruction::F64Add),
        ("add_negative_zero", -0.0_f64, Instruction::F64Add),
        ("multiply_one", 1.0_f64, Instruction::F64Mul),
    ] {
        // Enter and leave through integer bits so the host cannot quiet the
        // signaling NaN before the actual emitted arithmetic executes.
        let input = vec![
            Instruction::LocalGet(0),
            Instruction::F64ReinterpretI64,
            Instruction::F64Const(constant.into()),
            arithmetic,
            Instruction::I64ReinterpretF64,
            Instruction::End,
        ];
        let ops = peephole_set_get_to_tee(WasmBodyOps::from_instructions(input)).into_vec();
        assert_eq!(
            ops.len(),
            6,
            "{name}: unproved float identity removed arithmetic"
        );
        let body = WasmBody {
            param_types: vec![ValType::I64],
            result_types: vec![ValType::I64],
            locals: vec![],
            ops,
        };
        modules.push((name, executable_module(&body)));
    }
    let Some(node) = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "floating-point peephole execution",
    ) else {
        return;
    };
    let (directory, _cleanup) = wasm_test_temp_dir();
    let mut paths = serde_json::Map::new();
    for (name, bytes) in modules {
        let path = directory.join(format!("{name}.wasm"));
        fs::write(&path, bytes).expect("write floating-point executable");
        paths.insert(name.into(), serde_json::json!(path));
    }
    let config = directory.join("float-cases.json");
    fs::write(&config, serde_json::to_vec(&paths).unwrap()).expect("write float cases");
    run_node_test_script(
        &node,
        r#"
const fs = require('fs'), assert = require('assert/strict');
const paths = JSON.parse(fs.readFileSync(process.argv[1], 'utf8'));
const negativeZero = -(1n << 63n);
for (const [name, path] of Object.entries(paths)) {
  const run = new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(path))).exports.run;
  assert.equal(run(0n), 0n, name + ' positive zero');
  assert.equal(run(negativeZero), name === 'add_positive_zero' ? 0n : negativeZero,
               name + ' negative zero');
  for (const bits of [0x4000000000000000n, 0xc000000000000000n,
                     0x7ff0000000000000n, 0xfff0000000000000n]) {
    assert.equal(BigInt.asUintN(64, run(BigInt.asIntN(64, bits))), bits, name + ' finite/infinite');
  }
  for (const bits of [0x7ff0000000000001n, 0xfff0000000000001n,
                     0x7ff8000000000042n, 0xfff8000000000042n]) {
    const actual = BigInt.asUintN(64, run(BigInt.asIntN(64, bits)));
    assert.equal(actual & 0x7ff8000000000000n, 0x7ff8000000000000n,
                 name + ' arithmetic must produce a quiet NaN');
  }
}
"#,
        &[&config],
        "execute floating identities at their raw-bit peephole boundary",
    );
}
