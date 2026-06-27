use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use object::{Object, ObjectSymbol};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

/// Compile a standalone codegen object for inspection.
///
/// These tests compile a single function to an object purely to inspect the
/// emitted symbols; the object is never linked into a final binary. Such
/// objects must NOT emit the per-app `molt_app_resolve_intrinsic` resolver —
/// emitting it would demand the linked runtime staticlib's intrinsic-symbol
/// set (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`), which a unit-level codegen test
/// neither has nor needs. Production uses the identical opt-out for every
/// non-primary object (stdlib-cache / batch objects; see
/// `runtime/molt-backend/src/main.rs`). The `cfg(test)` carve-out in
/// `runtime_intrinsic_symbols_required` only covers *in-crate* unit tests;
/// integration tests link `molt-backend` as a non-test library, so they take
/// the canonical `emit_app_intrinsic_resolver = false` path instead.
fn compile_standalone(ir: SimpleIR) -> molt_backend::CompileOutput {
    let mut backend = SimpleBackend::new();
    backend.emit_app_intrinsic_resolver = false;
    backend.compile(ir)
}

/// Assert that `func_name` was lowered to a real, defined symbol in the
/// emitted object — the structural successor to the old
/// `output.trap_stub_names.is_empty()` check.
///
/// The native backend no longer has a trap-stub fallback (removed in
/// `8649b923b` "native: fail closed on codegen failures"). It now *fails
/// closed*: if a function cannot be compiled, `SimpleBackend::compile` panics
/// ("Cranelift compilation failed for `…`" or "native backend left … exported
/// function declaration(s) undefined") rather than emitting an object with a
/// runtime-aborting trap-stub body. The original tests asserted "this program
/// produced no trap-stub fallbacks"; the equivalent invariant today is that
/// `compile` returns at all *and* the function under test is present as a
/// defined (non-undefined) Export symbol — a trap-stubbed function never
/// reaches this state because codegen aborts first.
fn assert_function_compiled(bytes: &[u8], func_name: &str) {
    let file = object::File::parse(bytes).expect("backend must emit a parseable object file");
    let defined = file.symbols().any(|symbol| {
        symbol_matches(symbol.name().ok(), func_name)
            && symbol.is_definition()
            && !symbol.is_undefined()
    });
    assert!(
        defined,
        "function `{func_name}` must be emitted as a defined symbol (no trap-stub fallback); \
         present symbols: {:?}",
        file.symbols()
            .filter_map(|s| s.name().ok().map(str::to_string))
            .collect::<Vec<_>>()
    );
}

/// Compare an object-file symbol name against an unmangled function name.
///
/// Mach-O (macOS) prefixes C-ABI symbols with a leading underscore in the
/// object's symbol table (`_foo`), while ELF (Linux) does not (`foo`). The
/// `object` crate surfaces the raw table name, so accept either form to keep
/// the assertion target-portable.
fn symbol_matches(symbol_name: Option<&str>, func_name: &str) -> bool {
    match symbol_name {
        Some(name) => name == func_name || name.strip_prefix('_') == Some(func_name),
        None => false,
    }
}

#[test]
fn entry_block_params_compile_with_int_shadow_targets() {
    let mut const_one = op("const");
    const_one.value = Some(1);
    const_one.out = Some("tmp".to_string());

    let mut store_slot = op("store_var");
    store_slot.var = Some("loop_slot".to_string());
    store_slot.args = Some(vec!["tmp".to_string()]);

    let mut ret_arg = op("ret");
    ret_arg.var = Some("arg".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "entry_param_shadow_regression".to_string(),
            params: vec!["arg".to_string()],
            ops: vec![const_one, store_slot, ret_arg],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = compile_standalone(ir);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "entry_param_shadow_regression");
}

#[test]
fn structured_if_phi_merges_compile() {
    let mut cond = op("const_bool");
    cond.value = Some(1);
    cond.out = Some("cond".to_string());

    let mut if_op = op("if");
    if_op.args = Some(vec!["cond".to_string()]);

    let mut then_val = op("const");
    then_val.value = Some(1);
    then_val.out = Some("then_val".to_string());

    let else_op = op("else");

    let mut else_val = op("const");
    else_val.value = Some(2);
    else_val.out = Some("else_val".to_string());

    let end_if = op("end_if");

    let mut phi = op("phi");
    phi.out = Some("joined".to_string());
    phi.args = Some(vec!["then_val".to_string(), "else_val".to_string()]);

    let mut ret_joined = op("ret");
    ret_joined.var = Some("joined".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "structured_if_phi_regression".to_string(),
            params: Vec::new(),
            ops: vec![
                cond, if_op, then_val, else_op, else_val, end_if, phi, ret_joined,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = compile_standalone(ir);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "structured_if_phi_regression");
}

#[test]
fn nested_structured_if_phi_merges_compile() {
    let mut outer_cond = op("const_bool");
    outer_cond.value = Some(1);
    outer_cond.out = Some("outer_cond".to_string());

    let mut inner_cond = op("const_bool");
    inner_cond.value = Some(1);
    inner_cond.out = Some("inner_cond".to_string());

    let mut base = op("const");
    base.value = Some(0);
    base.out = Some("base".to_string());

    let mut outer_if = op("if");
    outer_if.args = Some(vec!["outer_cond".to_string()]);

    let mut inner_if = op("if");
    inner_if.args = Some(vec!["inner_cond".to_string()]);

    let mut inner_then = op("const");
    inner_then.value = Some(1);
    inner_then.out = Some("inner_then".to_string());

    let inner_else = op("else");

    let mut inner_else_val = op("const");
    inner_else_val.value = Some(2);
    inner_else_val.out = Some("inner_else".to_string());

    let inner_end_if = op("end_if");

    let mut inner_phi = op("phi");
    inner_phi.out = Some("outer_then".to_string());
    inner_phi.args = Some(vec!["inner_then".to_string(), "inner_else".to_string()]);

    let outer_end_if = op("end_if");

    let mut outer_phi = op("phi");
    outer_phi.out = Some("joined".to_string());
    outer_phi.args = Some(vec!["outer_then".to_string(), "base".to_string()]);

    let mut ret_joined = op("ret");
    ret_joined.var = Some("joined".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "nested_structured_if_phi_regression".to_string(),
            params: Vec::new(),
            ops: vec![
                outer_cond,
                inner_cond,
                base,
                outer_if,
                inner_if,
                inner_then,
                inner_else,
                inner_else_val,
                inner_end_if,
                inner_phi,
                outer_end_if,
                outer_phi,
                ret_joined,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = compile_standalone(ir);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "nested_structured_if_phi_regression");
}
