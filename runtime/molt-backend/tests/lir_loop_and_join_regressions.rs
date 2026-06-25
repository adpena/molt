use cranelift_object::object::{Object, ObjectSymbol};
use molt_backend::tir::lower_from_simple::lower_to_tir;
use molt_backend::tir::lower_to_simple::lower_to_simple_ir;
use molt_backend::tir::passes::run_pipeline;
use molt_backend::tir::type_refine::refine_types;
use molt_backend::tir::verify::verify_function;
use molt_backend::{CompileOutput, FunctionIR, OpIR, SimpleBackend, SimpleIR};

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
fn compile_standalone(ir: SimpleIR) -> CompileOutput {
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
    let file = cranelift_object::object::File::parse(bytes)
        .expect("backend must emit a parseable object file");
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

fn roundtrip_compile(func: FunctionIR) -> CompileOutput {
    let mut typed = lower_to_tir(&func);
    refine_types(&mut typed);
    let _stats = run_pipeline(
        &mut typed,
        &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
    );
    refine_types(&mut typed);
    verify_function(&typed).expect("typed TIR must verify");
    eprintln!("TYPED_TIR_DEBUG: {typed:#?}");
    let lir_debug =
        molt_backend::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction(&typed);
    eprintln!("LIR_DEBUG: {lir_debug:#?}");
    let round_tripped = lower_to_simple_ir(&typed);
    eprintln!("ROUNDTRIPPED_SIMPLE_DEBUG: {round_tripped:#?}");
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: func.name,
            params: func.params,
            ops: round_tripped,
            param_types: func.param_types,
            source_file: func.source_file,
            is_extern: func.is_extern,
        }],
        profile: None,
    };
    compile_standalone(ir)
}

#[test]
fn nested_loop_carried_values_with_inner_if_phi_compile() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut zero = op("const");
    zero.value = Some(0);
    zero.out = Some("zero".to_string());
    ops.push(zero);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("one".to_string());
    ops.push(one);

    let mut two = op("const");
    two.value = Some(2);
    two.out = Some("two".to_string());
    ops.push(two);

    let mut outer_stop = op("const");
    outer_stop.value = Some(3);
    outer_stop.out = Some("outer_stop".to_string());
    ops.push(outer_stop);

    let mut inner_stop = op("const");
    inner_stop.value = Some(4);
    inner_stop.out = Some("inner_stop".to_string());
    ops.push(inner_stop);

    let mut acc_init = op("const");
    acc_init.value = Some(0);
    acc_init.out = Some("acc_init".to_string());
    ops.push(acc_init);

    let mut store_acc = op("store_var");
    store_acc.var = Some("acc".to_string());
    store_acc.args = Some(vec!["acc_init".to_string()]);
    ops.push(store_acc);

    ops.push(op("loop_start"));
    let mut outer_idx = op("loop_index_start");
    outer_idx.args = Some(vec!["zero".to_string()]);
    outer_idx.out = Some("i".to_string());
    ops.push(outer_idx);

    let mut outer_lt = op("lt");
    outer_lt.args = Some(vec!["i".to_string(), "outer_stop".to_string()]);
    outer_lt.out = Some("outer_cond".to_string());
    ops.push(outer_lt);

    let mut outer_break = op("loop_break_if_false");
    outer_break.args = Some(vec!["outer_cond".to_string()]);
    ops.push(outer_break);

    ops.push(op("loop_start"));
    let mut inner_idx = op("loop_index_start");
    inner_idx.args = Some(vec!["zero".to_string()]);
    inner_idx.out = Some("j".to_string());
    ops.push(inner_idx);

    let mut inner_lt = op("lt");
    inner_lt.args = Some(vec!["j".to_string(), "inner_stop".to_string()]);
    inner_lt.out = Some("inner_cond".to_string());
    ops.push(inner_lt);

    let mut inner_break = op("loop_break_if_false");
    inner_break.args = Some(vec!["inner_cond".to_string()]);
    ops.push(inner_break);

    let mut cmp = op("lt");
    cmp.args = Some(vec!["j".to_string(), "two".to_string()]);
    cmp.out = Some("pick_then".to_string());
    ops.push(cmp);

    let mut if_op = op("if");
    if_op.args = Some(vec!["pick_then".to_string()]);
    ops.push(if_op);

    let mut then_val = op("add");
    then_val.args = Some(vec!["i".to_string(), "j".to_string()]);
    then_val.out = Some("then_val".to_string());
    ops.push(then_val);

    ops.push(op("else"));

    let mut else_val = op("add");
    else_val.args = Some(vec!["j".to_string(), "one".to_string()]);
    else_val.out = Some("else_val".to_string());
    ops.push(else_val);

    ops.push(op("end_if"));

    let mut phi = op("phi");
    phi.out = Some("picked".to_string());
    phi.args = Some(vec!["then_val".to_string(), "else_val".to_string()]);
    ops.push(phi);

    let mut load_acc = op("load_var");
    load_acc.var = Some("acc".to_string());
    load_acc.out = Some("acc_cur".to_string());
    ops.push(load_acc);

    let mut add_acc = op("add");
    add_acc.args = Some(vec!["acc_cur".to_string(), "picked".to_string()]);
    add_acc.out = Some("acc_next".to_string());
    ops.push(add_acc);

    let mut store_acc_next = op("store_var");
    store_acc_next.var = Some("acc".to_string());
    store_acc_next.args = Some(vec!["acc_next".to_string()]);
    ops.push(store_acc_next);

    let mut inc_j = op("add");
    inc_j.args = Some(vec!["j".to_string(), "one".to_string()]);
    inc_j.out = Some("j_inc".to_string());
    ops.push(inc_j);

    let mut next_j = op("loop_index_next");
    next_j.args = Some(vec!["j_inc".to_string()]);
    next_j.out = Some("j_inc".to_string());
    ops.push(next_j);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));

    let mut inc_i = op("add");
    inc_i.args = Some(vec!["i".to_string(), "one".to_string()]);
    inc_i.out = Some("i_inc".to_string());
    ops.push(inc_i);

    let mut next_i = op("loop_index_next");
    next_i.args = Some(vec!["i_inc".to_string()]);
    next_i.out = Some("i_inc".to_string());
    ops.push(next_i);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));

    let mut ret_acc = op("ret");
    ret_acc.var = Some("acc".to_string());
    ops.push(ret_acc);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "nested_loop_if_phi_regression".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = compile_standalone(ir);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "nested_loop_if_phi_regression");
}

#[test]
fn loop_body_if_join_then_continue_compiles() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut zero = op("const");
    zero.value = Some(0);
    zero.out = Some("zero".to_string());
    ops.push(zero);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("one".to_string());
    ops.push(one);

    let mut stop = op("const");
    stop.value = Some(5);
    stop.out = Some("stop".to_string());
    ops.push(stop);

    ops.push(op("loop_start"));
    let mut idx = op("loop_index_start");
    idx.args = Some(vec!["zero".to_string()]);
    idx.out = Some("idx".to_string());
    ops.push(idx);

    let mut cond = op("lt");
    cond.args = Some(vec!["idx".to_string(), "stop".to_string()]);
    cond.out = Some("keep_going".to_string());
    ops.push(cond);

    let mut break_if = op("loop_break_if_false");
    break_if.args = Some(vec!["keep_going".to_string()]);
    ops.push(break_if);

    let mut branch_cond = op("lt");
    branch_cond.args = Some(vec!["idx".to_string(), "one".to_string()]);
    branch_cond.out = Some("branch_cond".to_string());
    ops.push(branch_cond);

    let mut if_op = op("if");
    if_op.args = Some(vec!["branch_cond".to_string()]);
    ops.push(if_op);

    let mut then_val = op("add");
    then_val.args = Some(vec!["idx".to_string(), "one".to_string()]);
    then_val.out = Some("then_next".to_string());
    ops.push(then_val);

    ops.push(op("else"));

    let mut else_val = op("add");
    else_val.args = Some(vec!["idx".to_string(), "stop".to_string()]);
    else_val.out = Some("else_next".to_string());
    ops.push(else_val);

    ops.push(op("end_if"));

    let mut phi = op("phi");
    phi.out = Some("joined_next".to_string());
    phi.args = Some(vec!["then_next".to_string(), "else_next".to_string()]);
    ops.push(phi);

    let mut next_idx = op("loop_index_next");
    next_idx.args = Some(vec!["joined_next".to_string()]);
    next_idx.out = Some("joined_next".to_string());
    ops.push(next_idx);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));
    ops.push(op("ret_void"));

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "loop_body_if_join_continue".to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = compile_standalone(ir);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "loop_body_if_join_continue");
}

#[test]
fn tir_roundtrip_loop_body_if_return_then_continue_compiles() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut zero = op("const");
    zero.value = Some(0);
    zero.out = Some("zero".to_string());
    ops.push(zero);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("one".to_string());
    ops.push(one);

    let mut stop = op("const");
    stop.value = Some(3);
    stop.out = Some("stop".to_string());
    ops.push(stop);

    let mut true_val = op("const_bool");
    true_val.value = Some(1);
    true_val.out = Some("true_val".to_string());
    ops.push(true_val);

    let mut false_val = op("const_bool");
    false_val.value = Some(0);
    false_val.out = Some("false_val".to_string());
    ops.push(false_val);

    ops.push(op("loop_start"));

    let mut idx = op("loop_index_start");
    idx.args = Some(vec!["zero".to_string()]);
    idx.out = Some("idx".to_string());
    ops.push(idx);

    let mut cond = op("lt");
    cond.args = Some(vec!["idx".to_string(), "stop".to_string()]);
    cond.out = Some("keep_going".to_string());
    ops.push(cond);

    let mut break_if = op("loop_break_if_false");
    break_if.args = Some(vec!["keep_going".to_string()]);
    ops.push(break_if);

    let mut hit = op("eq");
    hit.args = Some(vec!["idx".to_string(), "one".to_string()]);
    hit.out = Some("hit".to_string());
    ops.push(hit);

    let mut if_op = op("if");
    if_op.args = Some(vec!["hit".to_string()]);
    ops.push(if_op);

    let mut ret_true = op("ret");
    ret_true.var = Some("true_val".to_string());
    ret_true.args = Some(vec!["true_val".to_string()]);
    ops.push(ret_true);

    ops.push(op("end_if"));

    let mut next_idx = op("add");
    next_idx.args = Some(vec!["idx".to_string(), "one".to_string()]);
    next_idx.out = Some("idx_next".to_string());
    ops.push(next_idx);

    let mut loop_index_next = op("loop_index_next");
    loop_index_next.args = Some(vec!["idx_next".to_string()]);
    loop_index_next.out = Some("idx_next".to_string());
    ops.push(loop_index_next);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));

    let mut ret_false = op("ret");
    ret_false.var = Some("false_val".to_string());
    ret_false.args = Some(vec!["false_val".to_string()]);
    ops.push(ret_false);

    let func = FunctionIR {
        name: "loop_if_return_continue_roundtrip".to_string(),
        params: Vec::new(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let output = roundtrip_compile(func);
    assert!(!output.bytes.is_empty());
    assert_function_compiled(&output.bytes, "loop_if_return_continue_roundtrip");
}

#[test]
fn nested_loop_if_phi_survives_tir_pipeline_without_fallback() {
    let mut ops: Vec<OpIR> = Vec::new();

    let mut zero = op("const");
    zero.value = Some(0);
    zero.out = Some("zero".to_string());
    ops.push(zero);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("one".to_string());
    ops.push(one);

    let mut two = op("const");
    two.value = Some(2);
    two.out = Some("two".to_string());
    ops.push(two);

    let mut outer_stop = op("const");
    outer_stop.value = Some(3);
    outer_stop.out = Some("outer_stop".to_string());
    ops.push(outer_stop);

    let mut inner_stop = op("const");
    inner_stop.value = Some(4);
    inner_stop.out = Some("inner_stop".to_string());
    ops.push(inner_stop);

    let mut acc_init = op("const");
    acc_init.value = Some(0);
    acc_init.out = Some("acc_init".to_string());
    ops.push(acc_init);

    let mut store_acc = op("store_var");
    store_acc.var = Some("acc".to_string());
    store_acc.args = Some(vec!["acc_init".to_string()]);
    ops.push(store_acc);

    ops.push(op("loop_start"));
    let mut outer_idx = op("loop_index_start");
    outer_idx.args = Some(vec!["zero".to_string()]);
    outer_idx.out = Some("i".to_string());
    ops.push(outer_idx);

    let mut outer_lt = op("lt");
    outer_lt.args = Some(vec!["i".to_string(), "outer_stop".to_string()]);
    outer_lt.out = Some("outer_cond".to_string());
    ops.push(outer_lt);

    let mut outer_break = op("loop_break_if_false");
    outer_break.args = Some(vec!["outer_cond".to_string()]);
    ops.push(outer_break);

    ops.push(op("loop_start"));
    let mut inner_idx = op("loop_index_start");
    inner_idx.args = Some(vec!["zero".to_string()]);
    inner_idx.out = Some("j".to_string());
    ops.push(inner_idx);

    let mut inner_lt = op("lt");
    inner_lt.args = Some(vec!["j".to_string(), "inner_stop".to_string()]);
    inner_lt.out = Some("inner_cond".to_string());
    ops.push(inner_lt);

    let mut inner_break = op("loop_break_if_false");
    inner_break.args = Some(vec!["inner_cond".to_string()]);
    ops.push(inner_break);

    let mut cmp = op("lt");
    cmp.args = Some(vec!["j".to_string(), "two".to_string()]);
    cmp.out = Some("pick_then".to_string());
    ops.push(cmp);

    let mut if_op = op("if");
    if_op.args = Some(vec!["pick_then".to_string()]);
    ops.push(if_op);

    let mut then_val = op("add");
    then_val.args = Some(vec!["i".to_string(), "j".to_string()]);
    then_val.out = Some("then_val".to_string());
    ops.push(then_val);

    ops.push(op("else"));

    let mut else_val = op("add");
    else_val.args = Some(vec!["j".to_string(), "one".to_string()]);
    else_val.out = Some("else_val".to_string());
    ops.push(else_val);

    ops.push(op("end_if"));

    let mut phi = op("phi");
    phi.out = Some("picked".to_string());
    phi.args = Some(vec!["then_val".to_string(), "else_val".to_string()]);
    ops.push(phi);

    let mut load_acc = op("load_var");
    load_acc.var = Some("acc".to_string());
    load_acc.out = Some("acc_cur".to_string());
    ops.push(load_acc);

    let mut add_acc = op("add");
    add_acc.args = Some(vec!["acc_cur".to_string(), "picked".to_string()]);
    add_acc.out = Some("acc_next".to_string());
    ops.push(add_acc);

    let mut store_acc_next = op("store_var");
    store_acc_next.var = Some("acc".to_string());
    store_acc_next.args = Some(vec!["acc_next".to_string()]);
    ops.push(store_acc_next);

    let mut inc_j = op("add");
    inc_j.args = Some(vec!["j".to_string(), "one".to_string()]);
    inc_j.out = Some("j_inc".to_string());
    ops.push(inc_j);

    let mut next_j = op("loop_index_next");
    next_j.args = Some(vec!["j_inc".to_string()]);
    next_j.out = Some("j_inc".to_string());
    ops.push(next_j);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));

    let mut inc_i = op("add");
    inc_i.args = Some(vec!["i".to_string(), "one".to_string()]);
    inc_i.out = Some("i_inc".to_string());
    ops.push(inc_i);

    let mut next_i = op("loop_index_next");
    next_i.args = Some(vec!["i_inc".to_string()]);
    next_i.out = Some("i_inc".to_string());
    ops.push(next_i);

    ops.push(op("loop_continue"));
    ops.push(op("loop_end"));

    let mut ret_acc = op("ret");
    ret_acc.var = Some("acc".to_string());
    ops.push(ret_acc);

    let func_ir = FunctionIR {
        name: "nested_loop_if_phi_direct_tir".to_string(),
        params: Vec::new(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let mut tir = lower_to_tir(&func_ir);
    refine_types(&mut tir);
    let _stats = run_pipeline(
        &mut tir,
        &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
    );
    refine_types(&mut tir);
    assert!(
        verify_function(&tir).is_ok(),
        "TIR pipeline must verify without backend fallback"
    );
    let roundtripped = lower_to_simple_ir(&tir);
    assert!(
        !roundtripped.is_empty(),
        "roundtrip should produce lowered ops without fallback"
    );
}
