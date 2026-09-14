use super::support::*;
use molt_codegen_abi::{HEADER_FLAG_HAS_PTRS, HEADER_FLAGS_OFFSET};

fn compile_field_op(kind: &str) -> Vec<u8> {
    let guarded = kind.starts_with("guarded_");
    let read = matches!(kind, "load" | "guarded_field_get");
    let mut params = if guarded {
        vec!["object", "class", "version"]
    } else {
        vec!["object"]
    };
    if !read {
        params.push("value");
    }
    let mut field = wasm_test_op(kind, read.then_some("result"), params.clone());
    field.value = Some(0);
    if guarded {
        field.s_value = Some("field".into());
    }
    let ret = if read {
        wasm_test_op("ret", None, vec!["result"])
    } else {
        wasm_test_op("ret_void", None, vec![])
    };
    wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            params,
            None,
            vec![field, ret],
        )],
        profile: None,
    })
    .wasm
}

#[test]
fn all_inline_field_paths_admit_receiver_and_backing_before_payload_access() {
    for (kind, runtime_name, read) in [
        ("store", "object_field_set", false),
        ("guarded_field_set", "object_field_set_ptr", false),
        ("load", "object_field_get", true),
        ("guarded_field_get", "guarded_field_get", true),
    ] {
        let wasm = compile_field_op(kind);
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let imports = wasm_function_import_indices(&wasm);
        let runtime = imports[runtime_name];
        let ops = wasm_operator_debug_for_export(&wasm, "molt_main");
        for initializer in [
            "object_field_init",
            "object_field_init_ptr",
            "guarded_field_init_ptr",
        ] {
            if let Some(index) = imports.get(initializer) {
                assert!(
                    !ops.contains(&format!("Call {{ function_index: {index} }}")),
                    "{kind}: a semantic field write cannot assert pristine ownership: {ops:?}"
                );
            }
        }
        let header_offset = format!("I32Const {{ value: {HEADER_FLAGS_OFFSET} }}");
        let flag_mask = format!("I32Const {{ value: {HEADER_FLAG_HAS_PTRS} }}");
        let call = format!("Call {{ function_index: {runtime} }}");
        let header = ops
            .iter()
            .position(|op| op == &header_offset)
            .unwrap_or_else(|| panic!("{kind} must inspect shared header; {ops:?}"));
        assert_eq!(ops[header + 1], "I32Add");
        assert!(ops[header + 2].starts_with("I32Load"));
        assert_eq!(ops[header + 3], flag_mask);
        assert_eq!(ops[header + 4], "I32And");
        // At least the receiver/shape branch must be open at the header read.
        let mut depth = 0usize;
        for op in &ops[..header] {
            if op.starts_with("If") || op.starts_with("Block") || op.starts_with("Loop") {
                depth += 1;
            } else if op == "End" {
                depth = depth.saturating_sub(1);
            }
        }
        assert!(
            depth > 0,
            "{kind}: receiver must dominate the header; {ops:?}"
        );
        if kind.starts_with("guarded_") {
            let guard = format!("Call {{ function_index: {} }}", imports["guard_layout"]);
            let guard_pos = ops[..header].iter().position(|op| op == &guard).unwrap();
            let resolve = format!("Call {{ function_index: {} }}", imports["handle_resolve"]);
            assert!(
                !ops[..guard_pos].contains(&resolve)
                    && ops[guard_pos + 1..header].contains(&resolve),
                "{kind}: admit tagged receiver before resolving payload: {ops:?}"
            );
            let fallback_name = if read {
                "guarded_field_get"
            } else {
                "guarded_field_set"
            };
            assert!(
                imports.contains_key(fallback_name),
                "{kind}: tagged fallback missing"
            );
        } else {
            assert!(
                ops[..header].iter().any(|op| op == "I64Eq"),
                "{kind}: {ops:?}"
            );
        }
        let runtime_call = ops.iter().position(|op| op == &call).unwrap();
        assert!(header < runtime_call, "{kind}: {ops:?}");
        assert!(
            ops[header..runtime_call]
                .iter()
                .any(|op| op.starts_with("If"))
        );
        if !read {
            assert!(ops[header..runtime_call].iter().any(|op| op == "I32Or"));
        }
        let payload_op = if read { "I64Load" } else { "I64Store" };
        let payload = header
            + ops[header..]
                .iter()
                .position(|op| op.starts_with(payload_op))
                .unwrap();
        assert!(
            runtime_call < payload,
            "{kind}: backing guard must select runtime or inline; {ops:?}"
        );
        assert!(ops[runtime_call..payload].iter().any(|op| op == "Else"));
        if read {
            let value_call = payload
                + ops[payload..]
                    .iter()
                    .position(|op| op == &call)
                    .expect("pointer-tagged inline candidate must use runtime missing resolution");
            let admission = &ops[payload + 1..value_call];
            assert!(admission.iter().any(|op| op == "I64And"), "{kind}: {ops:?}");
            assert!(admission.iter().any(|op| op == "I64Eq"), "{kind}: {ops:?}");
            assert!(
                admission.iter().any(|op| op.starts_with("If")),
                "{kind}: {ops:?}"
            );
            assert!(
                ops[value_call..].iter().any(|op| op == "Else"),
                "{kind}: immediate candidate retains inline output: {ops:?}"
            );
        }
    }
}
