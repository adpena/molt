use crate::{OpIR, SimpleIR};

/// Inject `molt_runtime_exit(0)` before the final `ret` in `molt_main`.
///
/// This calls `_exit(0)` after all user code and atexit callbacks have run,
/// skipping C-level global destructors and TLS teardown that cause
/// intermittent SIGSEGV on exit. Same approach as CPython.
pub fn inject_runtime_exit(ir: &mut SimpleIR) {
    for func in &mut ir.functions {
        if func.name != "molt_main" {
            continue;
        }
        // Find the last `ret` op and insert `call molt_runtime_exit` before it.
        let ret_idx = func.ops.iter().rposition(|op| op.kind == "ret");
        if let Some(idx) = ret_idx {
            let exit_op = OpIR {
                kind: "call".to_string(),
                args: Some(vec!["__molt_zero__".to_string()]),
                s_value: Some("molt_runtime_exit".to_string()),
                ..OpIR::default()
            };
            // Also need a const 0 for the exit code arg.
            let const_op = OpIR {
                kind: "const".to_string(),
                out: Some("__molt_zero__".to_string()),
                value: Some(0),
                ..OpIR::default()
            };
            func.ops.insert(idx, exit_op);
            func.ops.insert(idx, const_op);
        }
        break;
    }
}
