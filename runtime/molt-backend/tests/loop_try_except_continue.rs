use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn op_val_out(kind: &str, value: i64, out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        value: Some(value),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

fn op_args_out(kind: &str, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

/// When a loop is preceded by dead code (is_block_filled = true), the
/// `loop_start` / `loop_end` ops must still push and pop the loop stack
/// so that the stack stays balanced. Before the fix, `loop_start` was
/// missing from the unreachable-code pass-through list, causing "No loop
/// on stack" panics.
#[test]
fn loop_start_after_unreachable_code() {
    let mut ops: Vec<OpIR> = Vec::new();

    // Return immediately — everything after is dead code.
    ops.push(op("ret_void"));

    // Dead loop — must not panic.
    ops.push(op("loop_start"));
    ops.push(op("loop_end"));

    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_dead_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
           source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        let backend = SimpleBackend::new();
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok(), "loop_start in dead code caused a panic");
}

/// Same as above but for indexed loops (`loop_index_start`).
#[test]
fn loop_index_start_after_unreachable_code() {
    let mut ops: Vec<OpIR> = Vec::new();

    ops.push(op_val_out("const", 0, "v0"));

    // Return immediately — everything after is dead code.
    ops.push(op("ret_void"));

    // Dead indexed loop — must not panic.
    ops.push(op("loop_start"));
    ops.push(op_args_out("loop_index_start", &["v0"], "v1"));
    ops.push(op("loop_end"));

    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_dead_index_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
           source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        let backend = SimpleBackend::new();
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
    let mut ops: Vec<OpIR> = Vec::new();

    ops.push(op_val_out("const", 0, "v0"));
    ops.push(op_val_out("const", 3, "v1"));
    ops.push(op_val_out("const", 1, "v2"));

    ops.push(op("loop_start"));
    ops.push(op_args_out("loop_index_start", &["v0"], "v3"));

    ops.push(op_args_out("lt", &["v3", "v1"], "v4"));
    ops.push(op_args_out("loop_break_if_false", &["v4"], "none"));

    // Increment and continue.
    ops.push(op_args_out("add", &["v3", "v2"], "v5"));
    ops.push(op_args_out("loop_index_next", &["v5"], "v5"));
    ops.push(op("loop_continue"));

    ops.push(op("loop_end"));
    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_live_loop".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
           source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let result = std::panic::catch_unwind(|| {
        let backend = SimpleBackend::new();
        let _ = backend.compile(ir);
    });
    assert!(result.is_ok(), "live loop with continue caused a panic");
}
