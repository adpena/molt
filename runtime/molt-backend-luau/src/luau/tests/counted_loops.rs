//! Execute admitted normalized float carriers and reject raw integer transport.
use super::*;

fn op(kind: molt_ir::OpKind, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        args: (!args.is_empty()).then(|| args.iter().map(|arg| (*arg).into()).collect()),
        out: out.map(str::to_string),
        ..OpIR::new(kind)
    }
}

fn float(name: &str, value: f64) -> OpIR {
    OpIR {
        f_value: Some(value),
        ..op(molt_ir::OpKind::ConstFloat, &[], Some(name))
    }
}

fn label(kind: molt_ir::OpKind, value: i64) -> OpIR {
    OpIR {
        value: Some(value),
        ..op(kind, &[], None)
    }
}

fn store(name: &str, value: &str) -> OpIR {
    OpIR {
        var: Some(name.into()),
        ..op(molt_ir::OpKind::StoreVar, &[value], None)
    }
}

fn load(name: &str, out: &str) -> OpIR {
    OpIR {
        var: Some(name.into()),
        ..op(molt_ir::OpKind::LoadVar, &[], Some(out))
    }
}

/// Authored normalized float dataflow, not a conversion of integer transport.
/// `None` has a direct latch; `Some` transfers past a younger plain/carried loop.
fn normalized_float_function(name: &str, inner_carried: Option<bool>) -> FunctionIR {
    let mut ops = vec![
        float("seed", 7.0),
        float("one", 1.0),
        float("zero", 0.0),
        float("visit_cap", 8.0),
        float("inner_seed", 40.0),
        store("visits", "zero"),
        store("total", "zero"),
        store("observed_inner", "zero"),
        label(molt_ir::OpKind::Jump, 10),
        label(molt_ir::OpKind::Label, 10),
        store("outer", "seed"),
        op(molt_ir::OpKind::LoopStart, &[], None),
        load("outer", "outer"),
        op(molt_ir::OpKind::Lt, &["outer", "limit"], Some("in_range")),
        op(molt_ir::OpKind::LoopBreakIfFalse, &["in_range"], None),
        load("visits", "old_visits"),
        op(
            molt_ir::OpKind::Add,
            &["old_visits", "one"],
            Some("next_visits"),
        ),
        store("visits", "next_visits"),
        // This independent counter makes a reset/dropped recurrence fail with
        // a wrong result instead of hanging the required Lune process.
        op(
            molt_ir::OpKind::Lt,
            &["next_visits", "visit_cap"],
            Some("within_cap"),
        ),
        op(molt_ir::OpKind::LoopBreakIfFalse, &["within_cap"], None),
        load("total", "old_total"),
        op(
            molt_ir::OpKind::Add,
            &["old_total", "outer"],
            Some("next_total"),
        ),
        store("total", "next_total"),
    ];
    if let Some(carried) = inner_carried {
        if carried {
            ops.push(store("inner", "inner_seed"));
        }
        ops.push(op(molt_ir::OpKind::LoopStart, &[], None));
        if carried {
            ops.push(load("inner", "inner"));
        }
    }
    ops.extend([
        op(molt_ir::OpKind::Add, &["outer", "one"], Some("advanced")),
        store("outer", "advanced"),
    ]);
    if let Some(carried) = inner_carried {
        if carried {
            // The inner binding stays live while an explicit store updates
            // the outer slot and a jump leaves the younger physical loop.
            ops.push(load("inner", "inner_after_update"));
            ops.push(store("observed_inner", "inner_after_update"));
        }
        ops.extend([
            label(molt_ir::OpKind::Jump, 99),
            op(molt_ir::OpKind::LoopEnd, &[], None),
            label(molt_ir::OpKind::Label, 99),
        ]);
    }
    ops.extend([
        op(molt_ir::OpKind::LoopContinue, &[], None),
        op(molt_ir::OpKind::LoopEnd, &[], None),
        load("visits", "final_visits"),
        load("total", "final_total"),
        load("observed_inner", "final_inner"),
        op(
            molt_ir::OpKind::TupleNew,
            &["outer", "final_visits", "final_total", "final_inner"],
            Some("result"),
        ),
        op(molt_ir::OpKind::Ret, &["result"], None),
    ]);
    FunctionIR {
        name: name.into(),
        params: vec!["limit".into()],
        param_types: Some(vec!["float".into()]),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops,
        ..FunctionIR::default()
    }
}

#[test]
#[ignore = "requires the declared Lune runner; run rust.test.compiler-authorities"]
fn lune_oracle_labelled_normalized_float_carriers_preserve_recurrence_and_outer_updates() {
    let ir = SimpleIR {
        functions: vec![
            normalized_float_function("float_carrier_direct", None),
            normalized_float_function("float_carrier_under_plain", Some(false)),
            normalized_float_function("float_carrier_under_carried", Some(true)),
        ],
        profile: None,
    };
    let compiled = LuauBackend::new()
        .compile_checked(&ir)
        .expect("normalized labelled float dataflow is inside the Luau target contract");
    let source = format!(
        r#"{compiled}
;(function()
    local function expect(actual, index, visits, total, inner)
        assert(actual[1] == index and actual[2] == visits)
        assert(actual[3] == total and actual[4] == inner)
    end
    -- Authored recurrence: visit 7, 8, 9; finish at 10; sum = 24.
    expect(float_carrier_direct(10), 10, 3, 24, 0)
    expect(float_carrier_under_plain(10), 10, 3, 24, 0)
    expect(float_carrier_under_carried(10), 10, 3, 24, 40)
    -- Initial entry still defines the carrier when the body never executes.
    expect(float_carrier_direct(7), 7, 0, 0, 0)
    expect(float_carrier_under_plain(7), 7, 0, 0, 0)
    expect(float_carrier_under_carried(7), 7, 0, 0, 0)
    print("labelled-normalized-float-carriers-ok")
end)()
"#
    );
    execute_lune_oracle(
        "labelled_normalized_float_carriers",
        &source,
        LuneStdout::TrimmedExact("labelled-normalized-float-carriers-ok"),
    );
}

#[test]
fn checked_luau_rejects_counted_integer_transport_even_with_float_operands() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "raw_counted_float_transport".into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            ops: vec![
                float("seed", 7.0),
                float("updated", 8.0),
                label(molt_ir::OpKind::Jump, 10),
                label(molt_ir::OpKind::Label, 10),
                op(molt_ir::OpKind::LoopStart, &[], None),
                op(molt_ir::OpKind::LoopIndexStart, &["seed"], Some("index")),
                op(molt_ir::OpKind::LoopIndexNext, &["updated"], Some("index")),
                op(molt_ir::OpKind::LoopBreak, &[], None),
                op(molt_ir::OpKind::LoopEnd, &[], None),
                op(molt_ir::OpKind::Ret, &["index"], None),
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    backend.output.push_str("prior-buffer");
    let error = backend.compile_checked(&ir).expect_err(
        "float operands cannot erase the integer-producer requirement of counted transport",
    );
    assert!(
        error.contains("luau target rejected before source generation"),
        "{error}"
    );
    assert!(
        error.contains("raw_counted_float_transport") && error.contains("loop_index_start"),
        "{error}"
    );
    assert!(
        error.contains(
            "Python integer semantics require the canonical arbitrary-precision value authority"
        ),
        "{error}"
    );
    assert_eq!(
        backend.output, "prior-buffer",
        "target refusal must precede source writes"
    );
}

#[test]
fn labelled_counted_consumer_rejects_orphan_and_malformed_transport_before_emission() {
    let start = || op(molt_ir::OpKind::LoopIndexStart, &["seed"], Some("index"));
    let malformed_next = OpIR {
        source_op_idx: Some(73),
        ..op(
            molt_ir::OpKind::LoopIndexNext,
            &["seed", "extra"],
            Some("index"),
        )
    };
    let orphan_start = OpIR {
        source_op_idx: Some(73),
        ..start()
    };
    let orphan_next = OpIR {
        source_op_idx: Some(73),
        ..op(molt_ir::OpKind::LoopIndexNext, &["seed"], Some("index"))
    };
    for (kind, body) in [
        (
            molt_ir::OpKind::LoopIndexStart,
            vec![
                op(molt_ir::OpKind::LoopStart, &[], None),
                start(),
                op(molt_ir::OpKind::LoopBreak, &[], None),
                op(molt_ir::OpKind::LoopEnd, &[], None),
                // Reusing a formerly valid name cannot borrow a closed owner.
                orphan_start,
            ],
        ),
        (molt_ir::OpKind::LoopIndexNext, vec![orphan_next]),
        (
            molt_ir::OpKind::LoopIndexNext,
            vec![
                op(molt_ir::OpKind::LoopStart, &[], None),
                start(),
                malformed_next,
                op(molt_ir::OpKind::LoopEnd, &[], None),
            ],
        ),
    ] {
        let mut ops = vec![
            float("seed", 7.0),
            label(molt_ir::OpKind::Jump, 10),
            label(molt_ir::OpKind::Label, 10),
        ];
        ops.extend(body);
        ops.push(op(molt_ir::OpKind::RetVoid, &[], None));
        let function = FunctionIR {
            name: "rejected_counted_consumer".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops,
            ..FunctionIR::default()
        };
        // Enter the actual labelled body consumer directly: checked target
        // admission also builds a representation plan and could reject earlier,
        // concealing a missing normalization check in this emission boundary.
        let mut backend = LuauBackend::new();
        backend.output.push_str("prior-buffer");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            backend.emit_function_body(&function);
        }))
        .expect_err("residual transport cannot reach labelled source emission");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("normalization diagnostic must be text");
        assert!(
            message.contains("counted-loop normalization failed"),
            "{message}"
        );
        assert!(
            message.contains("function `rejected_counted_consumer`"),
            "{message}"
        );
        assert!(
            message.contains(kind.as_str()) && message.contains("source op#73"),
            "{message}"
        );
        assert!(message.contains("pre-SSA operation remains"), "{message}");
        assert_eq!(
            backend.output, "prior-buffer",
            "refusal must precede source writes"
        );
    }
}
