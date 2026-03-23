//! Fuzz target: generate random SimpleIR and attempt WASM compilation.
//!
//! The compiler must never panic on structurally valid IR (even if semantically
//! nonsensical). This target derives `Arbitrary` for a subset of IR structures
//! and feeds them through the full WASM backend pipeline.

#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::{Arbitrary, Unstructured};
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use molt_backend::wasm::WasmBackend;

/// A constrained IR generator that produces structurally valid IR.
#[derive(Debug)]
struct FuzzIR {
    ir: SimpleIR,
}

/// Op kinds that the WASM backend can handle without needing external imports
/// to resolve (pure computational ops).
const SAFE_OP_KINDS: &[&str] = &[
    "const",
    "const_bool",
    "const_none",
    "const_float",
    "return",
    "nop",
    "label",
    "jump",
    "jump_if_false",
    "jump_if_true",
];

impl<'a> Arbitrary<'a> for FuzzIR {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let num_functions: usize = u.int_in_range(1..=4)?;
        let mut functions = Vec::with_capacity(num_functions);

        // Always generate molt_main as the entry point.
        let main_ops = gen_ops(u, &["__ret"])?;
        functions.push(FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: main_ops,
            param_types: None,
        });

        for i in 1..num_functions {
            let num_params: usize = u.int_in_range(0..=3)?;
            let params: Vec<String> = (0..num_params).map(|p| format!("p{p}")).collect();
            let mut all_vars: Vec<String> = params.clone();
            all_vars.push("__ret".to_string());
            let ops = gen_ops(u, &all_vars.iter().map(|s| s.as_str()).collect::<Vec<_>>())?;
            functions.push(FunctionIR {
                name: format!("func_{i}"),
                params,
                ops,
                param_types: None,
            });
        }

        Ok(FuzzIR {
            ir: SimpleIR {
                functions,
                profile: None,
            },
        })
    }
}

fn gen_ops(u: &mut Unstructured, vars: &[&str]) -> arbitrary::Result<Vec<OpIR>> {
    let num_ops: usize = u.int_in_range(1..=20)?;
    let mut ops = Vec::with_capacity(num_ops + 1);
    let mut next_var = 0usize;

    for _ in 0..num_ops {
        let kind_idx: usize = u.int_in_range(0..=SAFE_OP_KINDS.len() - 1)?;
        let kind = SAFE_OP_KINDS[kind_idx].to_string();
        let out_name = format!("v{next_var}");
        next_var += 1;

        let op = match kind.as_str() {
            "const" => OpIR {
                kind,
                value: Some(u.int_in_range(-1000..=1000)?),
                out: Some(out_name),
                ..OpIR::default()
            },
            "const_bool" => OpIR {
                kind,
                value: Some(if u.arbitrary()? { 1 } else { 0 }),
                out: Some(out_name),
                ..OpIR::default()
            },
            "const_none" => OpIR {
                kind,
                out: Some(out_name),
                ..OpIR::default()
            },
            "const_float" => OpIR {
                kind,
                f_value: Some(u.arbitrary::<f64>().unwrap_or(0.0)),
                out: Some(out_name),
                ..OpIR::default()
            },
            "return" => {
                // Pick a variable to return.
                let var = if !vars.is_empty() {
                    let idx = u.int_in_range(0..=vars.len() - 1)?;
                    vars[idx].to_string()
                } else {
                    "__ret".to_string()
                };
                OpIR {
                    kind,
                    args: Some(vec![var]),
                    ..OpIR::default()
                }
            }
            "nop" => OpIR {
                kind,
                ..OpIR::default()
            },
            "label" => OpIR {
                kind,
                s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                ..OpIR::default()
            },
            "jump" => OpIR {
                kind,
                s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                ..OpIR::default()
            },
            "jump_if_false" | "jump_if_true" => {
                let cond = if !vars.is_empty() {
                    let idx = u.int_in_range(0..=vars.len() - 1)?;
                    vars[idx].to_string()
                } else {
                    "__ret".to_string()
                };
                OpIR {
                    kind,
                    s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                    args: Some(vec![cond]),
                    ..OpIR::default()
                }
            }
            _ => OpIR {
                kind,
                ..OpIR::default()
            },
        };
        ops.push(op);
    }

    // Always end with a return.
    ops.push(OpIR {
        kind: "return".to_string(),
        args: Some(vec!["__ret".to_string()]),
        ..OpIR::default()
    });

    Ok(ops)
}

fuzz_target!(|input: FuzzIR| {
    // Attempt compilation — must not panic.
    let backend = WasmBackend::new();
    let _wasm_bytes = backend.compile(input.ir);
    // If we get here, compilation succeeded without panicking.
    // We don't validate the WASM module further since the IR is random,
    // but the compiler itself must be robust.
});
