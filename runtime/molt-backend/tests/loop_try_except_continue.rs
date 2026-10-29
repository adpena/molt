use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

fn op_val_out(kind: molt_ir::OpKind, value: i64, out: &str) -> OpIR {
    OpIR {
        value: Some(value),
        out: Some(out.to_string()),
        ..OpIR::new(kind)
    }
}

fn op_args_out(kind: molt_ir::OpKind, args: &[&str], out: &str) -> OpIR {
    OpIR {
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..OpIR::new(kind)
    }
}

/// When a loop is preceded by dead code (is_block_filled = true), the
/// `loop_start` / `loop_end` ops must still push and pop the loop stack
/// so that the stack stays balanced. Before the fix, `loop_start` was
/// missing from the unreachable-code pass-through list, causing "No loop
/// on stack" panics.
#[test]
fn loop_start_after_unreachable_code() {
    let ops: Vec<OpIR> = vec![
        // Return immediately — everything after is dead code.
        op(molt_ir::OpKind::RetVoid),
        // Dead loop — must not panic.
        op(molt_ir::OpKind::LoopStart),
        op(molt_ir::OpKind::LoopEnd),
        op(molt_ir::OpKind::RetVoid),
    ];

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_dead_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        // Standalone codegen object — not linked into a final binary, so it
        // must not emit the per-app callable resolver (which would require the
        // runtime staticlib's callable-symbol set and panic without it,
        // masking the control-flow behavior this test actually exercises).
        let mut backend = SimpleBackend::new();
        backend.emit_app_callable_resolver = false;
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok(), "loop_start in dead code caused a panic");
}

/// Same as above but for indexed loops (`loop_index_start`).
#[test]
fn loop_index_start_after_unreachable_code() {
    let ops: Vec<OpIR> = vec![
        op_val_out(molt_ir::OpKind::Const, 0, "v0"),
        // Return immediately — everything after is dead code.
        op(molt_ir::OpKind::RetVoid),
        // Dead indexed loop — must not panic.
        op(molt_ir::OpKind::LoopStart),
        op_args_out(molt_ir::OpKind::LoopIndexStart, &["v0"], "v1"),
        op(molt_ir::OpKind::LoopEnd),
        op(molt_ir::OpKind::RetVoid),
    ];

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_dead_index_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        // Standalone codegen object — not linked into a final binary, so it
        // must not emit the per-app callable resolver (which would require the
        // runtime staticlib's callable-symbol set and panic without it,
        // masking the control-flow behavior this test actually exercises).
        let mut backend = SimpleBackend::new();
        backend.emit_app_callable_resolver = false;
        let _ = backend.compile(ir);
    });
    assert!(
        result.is_ok(),
        "loop_index_start in dead code caused a panic"
    );
}

/// Verify that the loop_start / loop_end pass-through does not regress
/// the existing test: a live loop with continue inside an if still works.
#[test]
fn loop_start_in_reachable_code_still_works() {
    let ops: Vec<OpIR> = vec![
        op_val_out(molt_ir::OpKind::Const, 0, "v0"),
        op_val_out(molt_ir::OpKind::Const, 3, "v1"),
        op_val_out(molt_ir::OpKind::Const, 1, "v2"),
        op(molt_ir::OpKind::LoopStart),
        op_args_out(molt_ir::OpKind::LoopIndexStart, &["v0"], "v3"),
        op_args_out(molt_ir::OpKind::Lt, &["v3", "v1"], "v4"),
        op_args_out(molt_ir::OpKind::LoopBreakIfFalse, &["v4"], "none"),
        // Increment and continue.
        op_args_out(molt_ir::OpKind::Add, &["v3", "v2"], "v5"),
        op_args_out(molt_ir::OpKind::LoopIndexNext, &["v5"], "v3"),
        op(molt_ir::OpKind::LoopContinue),
        op(molt_ir::OpKind::LoopEnd),
        op(molt_ir::OpKind::RetVoid),
    ];

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_live_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        // Standalone codegen object — not linked into a final binary, so it
        // must not emit the per-app callable resolver (which would require the
        // runtime staticlib's callable-symbol set and panic without it,
        // masking the control-flow behavior this test actually exercises).
        let mut backend = SimpleBackend::new();
        backend.emit_app_callable_resolver = false;
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok(), "live loop with continue caused a panic");
}
