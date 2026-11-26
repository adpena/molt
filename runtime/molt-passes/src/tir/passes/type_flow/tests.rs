//! Type flow (design 78, slice 3): contexts, relevance, recursion, merging
//! and the target gate, on hand-built modules.

use super::*;
use crate::tir::blocks::Terminator;
use crate::tir::ops::{AttrValue, OpCode, TirOp};

fn llvm() -> TargetInfo {
    TargetInfo::from_llvm_feature_string("")
}

fn cranelift() -> TargetInfo {
    TargetInfo::native_release_fast()
}

fn function(name: &str, params: usize) -> TirFunction {
    TirFunction::new(
        name.into(),
        vec![TirType::DynBox; params],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    )
}

fn push(func: &mut TirFunction, block: crate::tir::blocks::BlockId, op: TirOp) {
    func.blocks.get_mut(&block).unwrap().ops.push(op);
}

fn const_int(func: &mut TirFunction, block: crate::tir::blocks::BlockId, value: i64) -> ValueId {
    let result = func.fresh_value();
    let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    push(func, block, op);
    func.value_types.insert(result, TirType::I64);
    result
}

fn const_float(func: &mut TirFunction, block: crate::tir::blocks::BlockId, value: f64) -> ValueId {
    let result = func.fresh_value();
    let mut op = TirOp::new(OpCode::ConstFloat, vec![], vec![result]);
    op.attrs.insert("f_value".into(), AttrValue::Float(value));
    push(func, block, op);
    func.value_types.insert(result, TirType::F64);
    result
}

fn binary(
    func: &mut TirFunction,
    block: crate::tir::blocks::BlockId,
    opcode: OpCode,
    a: ValueId,
    b: ValueId,
) -> ValueId {
    let result = func.fresh_value();
    push(func, block, TirOp::new(opcode, vec![a, b], vec![result]));
    result
}

/// A proven guarded call of `callee` (HF-846 slice 2): operand 0 is the
/// callee word.
fn proven_call(
    func: &mut TirFunction,
    block: crate::tir::blocks::BlockId,
    callee: FunctionId,
    args: &[ValueId],
) -> ValueId {
    let word = func.fresh_value();
    push(
        func,
        block,
        TirOp::new(OpCode::ConstNone, vec![], vec![word]),
    );
    let result = func.fresh_value();
    let mut operands = vec![word];
    operands.extend_from_slice(args);
    let mut op = TirOp::new(OpCode::Call, operands, vec![result]);
    op.origin = Some(molt_ir::OpKind::CallGuarded);
    op.callee = Some(callee);
    op.set_proven_target(true);
    push(func, block, op);
    observe(func, block);
    result
}

/// The label of each fixture's exception exit block.
const EXCEPTION_LABEL: i64 = 900;

/// A `CheckException` after a call, as lowering writes one: a pending
/// exception leaves through the function's exception exit.
fn observe(func: &mut TirFunction, block: crate::tir::blocks::BlockId) {
    if !func
        .label_id_map
        .values()
        .any(|label| *label == EXCEPTION_LABEL)
    {
        let exit = func.fresh_block();
        func.blocks.insert(
            exit,
            crate::tir::blocks::TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unwind,
            },
        );
        func.label_id_map.insert(exit.0, EXCEPTION_LABEL);
    }
    func.has_exception_handling = true;
    let mut check = TirOp::new(OpCode::CheckException, vec![], vec![]);
    check
        .attrs
        .insert("value".into(), AttrValue::Int(EXCEPTION_LABEL));
    check.attrs.insert(
        "exception_edge".into(),
        AttrValue::Int(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin as i64),
    );
    push(func, block, check);
}

/// `def fib(n): return n if n < 2 else fib(n - 1) + fib(n - 2)`, row 0.
fn fib() -> TirFunction {
    let mut f = function("fib", 1);
    let entry = f.entry_block;
    let n = f.blocks[&entry].args[0].id;
    let two = const_int(&mut f, entry, 2);
    let small = binary(&mut f, entry, OpCode::Lt, n, two);
    let base = f.fresh_block();
    let step = f.fresh_block();
    for id in [base, step] {
        f.blocks.insert(
            id,
            crate::tir::blocks::TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );
    }
    f.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: small,
        then_block: base,
        then_args: vec![],
        else_block: step,
        else_args: vec![],
    };
    f.blocks.get_mut(&base).unwrap().terminator = Terminator::Return { values: vec![n] };
    let one = const_int(&mut f, step, 1);
    let n1 = binary(&mut f, step, OpCode::Sub, n, one);
    let two_again = const_int(&mut f, step, 2);
    let n2 = binary(&mut f, step, OpCode::Sub, n, two_again);
    let a = proven_call(&mut f, step, FunctionId(0), &[n1]);
    let b = proven_call(&mut f, step, FunctionId(0), &[n2]);
    let sum = binary(&mut f, step, OpCode::Add, a, b);
    f.blocks.get_mut(&step).unwrap().terminator = Terminator::Return { values: vec![sum] };
    f
}

/// `def main(): return callee(<args>)`, with each argument a literal.
fn caller(name: &str, callee: FunctionId, literals: &[Literal]) -> TirFunction {
    let mut f = function(name, 0);
    let entry = f.entry_block;
    let args: Vec<ValueId> = literals
        .iter()
        .map(|literal| match literal {
            Literal::Int(value) => const_int(&mut f, entry, *value),
            Literal::Float(value) => const_float(&mut f, entry, *value),
        })
        .collect();
    let result = proven_call(&mut f, entry, callee, &args);
    f.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };
    f
}

#[derive(Clone, Copy)]
enum Literal {
    Int(i64),
    Float(f64),
}

fn calls(func: &TirFunction) -> Vec<&TirOp> {
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    blocks
        .into_iter()
        .flat_map(|id| func.blocks[&id].ops.iter())
        .filter(|op| op.opcode == OpCode::Call && op.origin == Some(molt_ir::OpKind::CallGuarded))
        .collect()
}

fn body<'m>(module: &'m TirModule, name: &str) -> &'m TirFunction {
    module
        .functions
        .iter()
        .find(|func| func.name == name)
        .unwrap_or_else(|| panic!("no body {name}"))
}

fn id_of(module: &TirModule, name: &str) -> FunctionId {
    let index = module
        .functions
        .iter()
        .position(|func| func.name == name)
        .unwrap_or_else(|| panic!("no body {name}"));
    module.ids[index]
}

#[test]
fn factory_result_specializes_a_field_only_clone() {
    use molt_ir::OpKind;
    use molt_ir::class_identity::{EXACT_FIELD_SLOTS_ATTR, encode_field_slots};
    use molt_ir::tir::op_kinds_generated::kind_to_opcode_table;

    let symbol = "class:m:Point";
    let class = TirType::Class(molt_ir::ClassId::of_definition(symbol));
    let field_op = |kind, operands, results| {
        let opcode = kind_to_opcode_table(kind).unwrap_or(OpCode::Copy);
        let mut op = TirOp::new(opcode, operands, results);
        op.origin = Some(kind);
        op
    };

    let mut factory = function("factory", 1);
    let entry = factory.entry_block;
    let class_object = factory.blocks[&entry].args[0].id;
    let mut stamp = field_op(OpKind::ClassSetDefinition, vec![class_object], vec![]);
    stamp
        .attrs
        .insert("class_definition".into(), AttrValue::Str(symbol.into()));
    stamp.attrs.insert(
        EXACT_FIELD_SLOTS_ATTR.into(),
        AttrValue::Bytes(encode_field_slots(&[("x".into(), 8)])),
    );
    push(&mut factory, entry, stamp);
    let allocated = factory.fresh_value();
    let mut new = field_op(OpKind::ObjectNewBound, vec![class_object], vec![allocated]);
    new.attrs
        .insert("class_definition".into(), AttrValue::Str(symbol.into()));
    new.attrs.insert(
        molt_ir::class_identity::STABLE_INSTANCE_CLASS_ATTR.into(),
        AttrValue::Bool(true),
    );
    push(&mut factory, entry, new);
    observe(&mut factory, entry);
    factory.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![allocated],
    };

    let mut read = function("read", 1);
    let entry = read.entry_block;
    let point = read.blocks[&entry].args[0].id;
    let result = read.fresh_value();
    let mut get = field_op(OpKind::GetAttrGenericObj, vec![point], vec![result]);
    get.attrs.insert("name".into(), AttrValue::Str("x".into()));
    push(&mut read, entry, get);
    observe(&mut read, entry);
    read.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut main = function("main", 1);
    let entry = main.entry_block;
    let class_object = main.blocks[&entry].args[0].id;
    let point = proven_call(&mut main, entry, FunctionId(0), &[class_object]);
    let result = proven_call(&mut main, entry, FunctionId(1), &[point]);
    main.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut module = TirModule::with_own_table("m", vec![factory, read, main]);
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);

    let calls = calls(body(&module, "main"));
    assert_eq!(calls[0].exact_result_proof(), Some(class));
    assert_eq!(outcome.clones.len(), 1, "{:#?}", outcome.report);
    let clone = outcome.clones[0];
    assert_eq!(clone.origin, FunctionId(1));
    assert_eq!(calls[1].callee, Some(clone.id));
    let has_get = |name: &str, kind: OpKind| {
        body(&module, name)
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| {
                op.origin == Some(kind)
                    && (kind != OpKind::Load || op.attrs.get("value") == Some(&AttrValue::Int(8)))
            })
    };
    assert!(has_get("read", OpKind::GetAttrGenericObj));
    assert!(has_get(module.table.symbol(clone.id), OpKind::Load));
}

#[test]
fn generic_contexts_include_resolved_leaf_results_and_skip_isolated_bodies() {
    let mut callee = function("leaf", 0);
    let entry = callee.entry_block;
    let value = const_int(&mut callee, entry, 41);
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![value],
    };
    let mut module = TirModule::with_own_table(
        "m",
        vec![
            callee,
            function("unseen_entry", 0),
            caller("main", FunctionId(0), &[]),
        ],
    );

    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);

    assert_eq!(outcome.contexts, 2);
    assert_eq!(
        calls(body(&module, "main"))[0].exact_result_proof(),
        Some(TirType::I64)
    );
    assert!(!outcome.changed.contains(&id_of(&module, "unseen_entry")));
}

#[test]
fn bounded_checked_returns_keep_the_python_hull_across_a_wrapper() {
    // The same mask on negative, small and heap-sized Python integers gives
    // the residues 0..7; the wrapper translates every result by -12.
    let mut leaf = function("residue", 1);
    let entry = leaf.entry_block;
    let arg = leaf.blocks[&entry].args[0].id;
    let mask = const_int(&mut leaf, entry, 7);
    let result = binary(&mut leaf, entry, OpCode::BitAnd, arg, mask);
    leaf.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut wrapper = function("translate", 1);
    let entry = wrapper.entry_block;
    let arg = wrapper.blocks[&entry].args[0].id;
    let word = proven_call(&mut wrapper, entry, FunctionId(0), &[arg]);
    let twelve = const_int(&mut wrapper, entry, 12);
    let result = binary(&mut wrapper, entry, OpCode::Sub, word, twelve);
    wrapper.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };
    let main = caller("driver", FunctionId(1), &[Literal::Int(-19)]);
    let mut module = TirModule::with_own_table("m", vec![leaf, wrapper, main]);
    run(&mut module, &llvm(), &HashSet::default(), false);

    let body = body(&module, "driver");
    let call = calls(body)[0];
    let range = call.exact_result_range().expect("all exact-int callers");
    let oracle = (-16..16)
        .map(|i| ((1i128 << 99) + i) & 7)
        .map(|i| (i - 12) as i64);
    assert_eq!(Some(range.lo), oracle.clone().min());
    assert_eq!(Some(range.hi), oracle.max());
    let value_ranges = crate::tir::passes::value_range::compute_with_scalar_facts(
        &crate::tir::passes::value_identity::ScalarFacts::new(body),
    );
    assert_eq!(value_ranges.global_range_of(call.results[0]), Some(range));
    assert!(
        crate::representation_facts::raw_i64_safe_values_for(body, &value_ranges)
            .contains(&call.results[0])
    );
}

#[test]
fn checked_forwarders_preserve_each_actuals_range_on_success() {
    use crate::tir::passes::value_identity::ScalarFacts;
    use crate::tir::passes::value_range::compute_with_scalar_facts;

    let mut identity = function("identity", 1);
    let entry = identity.entry_block;
    let arg = identity.blocks[&entry].args[0].id;
    identity.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![arg] };

    let mut driver = function("driver", 1);
    let entry = driver.entry_block;
    let external = driver.blocks[&entry].args[0].id;
    let left = const_int(&mut driver, entry, -41);
    let right = const_int(&mut driver, entry, 300);
    let first = proven_call(&mut driver, entry, FunctionId(0), &[left]);
    let nested = proven_call(&mut driver, entry, FunctionId(0), &[first]);
    let distinct = proven_call(&mut driver, entry, FunctionId(0), &[right]);
    let unknown = proven_call(&mut driver, entry, FunctionId(0), &[external]);
    let rebound = proven_call(&mut driver, entry, FunctionId(0), &[right]);
    driver
        .blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .iter_mut()
        .find(|op| op.results == [rebound])
        .unwrap()
        .set_proven_target(false);
    let tuple = driver.fresh_value();
    let mut pack = TirOp::new(
        OpCode::BuildTuple,
        vec![nested, distinct, unknown, rebound],
        vec![tuple],
    );
    pack.origin = Some(molt_ir::OpKind::TupleNew);
    push(&mut driver, entry, pack);
    driver.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![tuple],
    };

    for target in [llvm(), cranelift()] {
        let mut module = TirModule::with_own_table("m", vec![identity.clone(), driver.clone()]);
        run(
            &mut module,
            &target,
            &HashSet::from_iter(["identity".into()]),
            false,
        );
        let driver = body(&module, "driver");
        let calls = calls(driver);
        let ranges = compute_with_scalar_facts(&ScalarFacts::new(driver));
        let raw = crate::representation_facts::raw_i64_safe_values_for(driver, &ranges);
        assert_eq!(calls.len(), 5);
        for (call, expected) in calls
            .iter()
            .zip([Some(-41), Some(-41), Some(300), None, None])
        {
            let expected = expected.map(IntRange::point);
            assert_eq!(call.exact_result_range(), expected);
            assert_eq!(ranges.global_range_of(call.results[0]), expected);
            assert_eq!(raw.contains(&call.results[0]), expected.is_some());
        }
    }
}

#[test]
fn a_mixed_call_ranges_only_its_checked_success_value() {
    use crate::tir::passes::value_identity::ScalarFacts;
    use crate::tir::passes::value_range::compute_with_scalar_facts;
    use crate::tir::values::TirValue;

    let mut func = caller("mixed", FunctionId(0), &[]);
    let entry = func.entry_block;
    let exit = func.blocks.keys().copied().find(|id| *id != entry).unwrap();
    let argument = func.fresh_value();
    func.blocks.get_mut(&exit).unwrap().args.push(TirValue {
        id: argument,
        ty: TirType::DynBox,
    });
    let ops = &mut func.blocks.get_mut(&entry).unwrap().ops;
    let call = ops.iter_mut().find(|op| op.opcode == OpCode::Call).unwrap();
    let word = call.results[0];
    let range = IntRange::point(-17);
    call.mark_exact_result_proof(&TirType::I64);
    call.set_exact_result_range(Some(range));
    ops.last_mut().unwrap().operands.push(word);

    let before = compute_with_scalar_facts(&ScalarFacts::new(&func));
    assert_eq!(before.global_range_of(word), None);
    assert!(crate::tir::type_refine::project_checked_call_results(
        &mut func,
        |_| true
    ));
    let [success] = func.blocks[&entry].ops.last().unwrap().results[..] else {
        unreachable!()
    };
    let after = compute_with_scalar_facts(&ScalarFacts::new(&func));
    assert_eq!(after.global_range_of(success), Some(range));
    assert_eq!(after.global_range_of(word), None);
    assert_eq!(after.global_range_of(argument), None);

    // Moving the projection before its observer cannot make the return ABI
    // word safe to unbox on either path.
    let ops = &mut func.blocks.get_mut(&entry).unwrap().ops;
    let last = ops.len() - 1;
    ops.swap(last, last - 1);
    let early = compute_with_scalar_facts(&ScalarFacts::new(&func));
    assert_eq!(early.global_range_of(success), None);
    assert_eq!(early.global_range_of(word), None);
}

#[test]
fn a_discarded_result_still_specializes_its_call() {
    let mut main = caller("main", FunctionId(0), &[Literal::Int(30)]);
    let entry = main.entry_block;
    let answer = const_int(&mut main, entry, 0);
    main.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![answer],
    };
    let mut module = TirModule::with_own_table("m", vec![fib(), main]);

    run(&mut module, &llvm(), &HashSet::default(), false);

    let call = calls(body(&module, "main"))[0];
    assert_eq!(call.callee, Some(id_of(&module, "fib__tf_i")));
    assert_eq!(call.exact_result_proof(), Some(TirType::I64));
}

#[test]
fn recursive_int_context_enters_its_own_clone() {
    let mut module = TirModule::with_own_table(
        "m",
        vec![fib(), caller("main", FunctionId(0), &[Literal::Int(30)])],
    );
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);

    assert_eq!(outcome.clones.len(), 1, "{:#?}", outcome.report);
    let clone = id_of(&module, "fib__tf_i");
    assert_eq!(
        outcome.clones[0],
        SpecializedClone {
            id: clone,
            origin: FunctionId(0)
        }
    );
    assert_eq!(module.table.symbol(clone), "fib__tf_i");
    assert_eq!(
        body(&module, "fib__tf_i").exact_param_types,
        vec![TirType::I64]
    );

    // main enters the clone, and fib(int) returns int.
    let main_calls = calls(body(&module, "main"));
    assert_eq!(main_calls[0].callee, Some(clone));
    assert_eq!(main_calls[0].exact_result_proof(), Some(TirType::I64));
    // The clone's recursive calls enter the clone itself.
    let clone_calls = calls(body(&module, "fib__tf_i"));
    assert_eq!(clone_calls.len(), 2);
    for call in clone_calls {
        assert_eq!(call.callee, Some(clone));
        assert_eq!(call.exact_result_range(), None, "fib grows past i64");
        assert_eq!(call.exact_result_proof(), Some(TirType::I64));
    }
    // The generic body serves every other caller and proves nothing.
    let generic_calls = calls(body(&module, "fib"));
    for call in generic_calls {
        assert_eq!(call.callee, Some(FunctionId(0)));
        assert_eq!(call.exact_result_proof(), None);
    }
    assert!(body(&module, "fib").exact_param_types.is_empty());
    crate::tir::verify::verify_function(body(&module, "fib__tf_i")).unwrap();
}

#[test]
fn widened_call_does_not_emit_the_unreachable_specialization_tree() {
    // choose(flag, value) may return an int or the value from opaque().
    // Its generic result initially looks exact while opaque() is unsolved.
    let mut choose = function("choose", 2);
    let entry = choose.entry_block;
    let flag = choose.blocks[&entry].args[0].id;
    let value = choose.blocks[&entry].args[1].id;
    let int_arm = choose.fresh_block();
    let other_arm = choose.fresh_block();
    for id in [int_arm, other_arm] {
        choose.blocks.insert(
            id,
            crate::tir::blocks::TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );
    }
    choose.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: flag,
        then_block: int_arm,
        then_args: vec![],
        else_block: other_arm,
        else_args: vec![],
    };
    let int = const_int(&mut choose, int_arm, 2);
    choose.blocks.get_mut(&int_arm).unwrap().terminator = Terminator::Return { values: vec![int] };
    let other = proven_call(&mut choose, other_arm, FunctionId(3), &[value]);
    choose.blocks.get_mut(&other_arm).unwrap().terminator = Terminator::Return {
        values: vec![other],
    };

    // main(flag, value) calls fib through a forwarding body. The initially
    // exact context can discover recursive fib(int), but once choose widens,
    // only a separate live entry may keep that recursive clone.
    let mut main = function("main", 2);
    let entry = main.entry_block;
    let args: Vec<_> = main.blocks[&entry].args.iter().map(|arg| arg.id).collect();
    let selected = proven_call(&mut main, entry, FunctionId(0), &args);
    let result = proven_call(&mut main, entry, FunctionId(2), &[selected]);
    main.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut forward = function("forward", 1);
    let entry = forward.entry_block;
    let value = forward.blocks[&entry].args[0].id;
    let result = proven_call(&mut forward, entry, FunctionId(4), &[value]);
    forward.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut opaque = function("opaque", 1);
    let entry = opaque.entry_block;
    let value = opaque.blocks[&entry].args[0].id;
    opaque.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![value],
    };

    let mut recursive = fib();
    for block in recursive.blocks.values_mut() {
        for op in &mut block.ops {
            if op.callee == Some(FunctionId(0)) {
                op.callee = Some(FunctionId(4));
            }
        }
    }
    let bodies = vec![choose, main, forward, opaque, recursive];
    let mut module = TirModule::with_own_table("m", bodies.clone());
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);

    assert!(
        outcome.contexts > bodies.len(),
        "must discover a transient key"
    );
    assert_eq!(
        calls(body(&module, "main"))[1].callee,
        Some(id_of(&module, "forward"))
    );
    assert!(outcome.clones.is_empty(), "{:?}", outcome.clones);

    let mut bodies = bodies;
    bodies.push(caller("known_int", FunctionId(2), &[Literal::Int(8)]));
    let mut module = TirModule::with_own_table("m", bodies);
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);
    let fib = id_of(&module, "fib__tf_i");
    assert!(outcome.clones.iter().any(|clone| clone.id == fib));
    assert!(
        calls(body(&module, "fib__tf_i"))
            .iter()
            .all(|call| call.callee == Some(fib))
    );
}

#[test]
fn each_argument_family_gets_its_own_context() {
    // def area(w): return w * w ; main calls it with an int and a float.
    let mut area = function("area", 1);
    let entry = area.entry_block;
    let w = area.blocks[&entry].args[0].id;
    let product = binary(&mut area, entry, OpCode::Mul, w, w);
    area.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![product],
    };
    let mut module = TirModule::with_own_table(
        "m",
        vec![
            area,
            caller("ints", FunctionId(0), &[Literal::Int(3)]),
            caller("floats", FunctionId(0), &[Literal::Float(2.5)]),
        ],
    );
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);
    assert_eq!(outcome.clones.len(), 2, "{:#?}", outcome.report);
    let int_clone = id_of(&module, "area__tf_i");
    let float_clone = id_of(&module, "area__tf_f");
    assert_eq!(calls(body(&module, "ints"))[0].callee, Some(int_clone));
    assert_eq!(calls(body(&module, "floats"))[0].callee, Some(float_clone));
    assert_eq!(
        calls(body(&module, "floats"))[0].exact_result_proof(),
        Some(TirType::F64)
    );
}

#[test]
fn proven_closure_specializes_user_arguments_and_keeps_the_captured_callable() {
    let mut closure = function("captured", 2);
    closure.param_names[0] = crate::MOLT_CLOSURE_PARAM_NAME.into();
    let entry = closure.entry_block;
    let argument = closure.blocks[&entry].args[1].id;
    let square = binary(&mut closure, entry, OpCode::Mul, argument, argument);
    closure.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![square],
    };
    // Both calls have the same proven code and different live function objects.
    // No object identity or captured cell contents may enter the clone key.
    let mut caller = function("floats", 2);
    let entry = caller.entry_block;
    let owners = [
        caller.blocks[&entry].args[0].id,
        caller.blocks[&entry].args[1].id,
    ];
    let scalar = const_float(&mut caller, entry, 2.5);
    let mut results = Vec::new();
    for owner in owners {
        let result = caller.fresh_value();
        let mut call = TirOp::new(OpCode::Call, vec![owner, scalar], vec![result]);
        call.origin = Some(molt_ir::OpKind::CallGuarded);
        call.callee = Some(FunctionId(0));
        call.set_proven_target(true);
        push(&mut caller, entry, call);
        observe(&mut caller, entry);
        results.push(result);
    }
    let sum = binary(&mut caller, entry, OpCode::Add, results[0], results[1]);
    caller.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![sum] };

    // Demand for the user parameter must cross the closure call in a forwarding
    // body too, even when the hidden environment is unused.
    let mut forward = function("forward", 1);
    let entry = forward.entry_block;
    let input = forward.blocks[&entry].args[0].id;
    let result = proven_call(&mut forward, entry, FunctionId(0), &[input]);
    forward.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };
    let mut module = TirModule::with_own_table("m", vec![closure, caller, forward]);
    let body_index = module.body_index();
    let members = exact_receiver_fields::exact_members(&module, &body_index);
    let mut sites: Vec<_> = module
        .functions
        .iter()
        .map(|func| sites::sites_of(func, &body_index, &module, &members))
        .collect();
    let relevant = relevance::solve(&module, &mut sites, &body_index, &[true; 3]);
    assert_eq!(relevant[2], [true]);

    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);
    assert_eq!(outcome.clones.len(), 1, "{:#?}", outcome.report);
    let calls = calls(body(&module, "floats"));
    assert_eq!(calls.len(), owners.len());
    for (call, owner) in calls.iter().zip(owners) {
        assert_eq!(call.callee, Some(id_of(&module, "captured__tf_df")));
        assert_eq!(call.operands, [owner, scalar]);
        assert_eq!(call.exact_result_proof(), Some(TirType::F64));
    }
    let clone = body(&module, "captured__tf_df");
    assert!(clone.has_closure_parameter());
    assert_eq!(clone.exact_param_types, [TirType::DynBox, TirType::F64]);
}

#[test]
fn an_irrelevant_parameter_never_splits_a_context() {
    // def pick(x, y): return y + 1 ; x is never read.
    let mut pick = function("pick", 2);
    let entry = pick.entry_block;
    let y = pick.blocks[&entry].args[1].id;
    let one = const_int(&mut pick, entry, 1);
    let sum = binary(&mut pick, entry, OpCode::Add, y, one);
    pick.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![sum] };
    let mut module = TirModule::with_own_table(
        "m",
        vec![
            pick,
            caller("a", FunctionId(0), &[Literal::Float(1.0), Literal::Int(2)]),
            caller("b", FunctionId(0), &[Literal::Int(1), Literal::Int(2)]),
        ],
    );
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);
    // Both callers share the one context the relevant `y` selects.
    assert_eq!(outcome.clones.len(), 1, "{:#?}", outcome.report);
    let clone = id_of(&module, "pick__tf_di");
    assert_eq!(calls(body(&module, "a"))[0].callee, Some(clone));
    assert_eq!(calls(body(&module, "b"))[0].callee, Some(clone));
    assert_eq!(
        body(&module, "pick__tf_di").exact_param_types,
        vec![TirType::DynBox, TirType::I64]
    );
}

#[test]
fn a_target_that_checks_guarded_code_gets_typed_results_but_no_clone() {
    // def three(x): return 3 ; its result is an int whatever x is.
    let mut three = function("three", 1);
    let entry = three.entry_block;
    let value = const_int(&mut three, entry, 3);
    three.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![value],
    };
    let mut module = TirModule::with_own_table(
        "m",
        vec![three, caller("main", FunctionId(0), &[Literal::Int(1)])],
    );
    let outcome = run(&mut module, &cranelift(), &HashSet::default(), true);
    assert!(outcome.clones.is_empty());
    assert_eq!(module.functions.len(), 2);
    let call = calls(body(&module, "main"))[0];
    assert_eq!(call.callee, Some(FunctionId(0)));
    assert_eq!(call.exact_result_proof(), Some(TirType::I64));
}

#[test]
fn shared_return_preserves_the_supplied_argument_type_without_a_clone() {
    use crate::tir::lower_from_simple::lower_to_tir;
    use molt_ir::OpKind;
    // Hidden closure, ignored class argument, then the object to validate.
    // A shared validator's success path returns the saved user argument.
    let validator: molt_ir::FunctionIR = serde_json::from_value(serde_json::json!({
        "name": "validate", "params": [crate::MOLT_CLOSURE_PARAM_NAME, "cls", "x"], "return_abi":"value",
        "ops":[
            {"kind":"frame_home_store","args":["x"],"value":0,"out":"saved"},
            {"kind":"isinstance","args":["saved","cls"],"out":"ok"},
            {"kind":"if","args":["ok"]},
            {"kind":"binding_alias","args":["saved"],"out":"returned"},
            {"kind":"ret","args":["returned"]},
            {"kind":"end_if"},
            {"kind":"raise","args":["cls"]},
            {"kind":"ret_void","unwind":true}
        ]
    })).unwrap();
    let validator = lower_to_tir(&validator);
    let mut driver = function("driver", 2);
    let entry = driver.entry_block;
    let cls = driver.blocks[&entry].args[0].id;
    let unknown = driver.blocks[&entry].args[1].id;
    let child = driver.fresh_value();
    let mut alloc = TirOp::new(OpCode::ObjectNewBound, vec![cls], vec![child]);
    alloc.origin = Some(OpKind::ObjectNewBound);
    let symbol = "class:m:Child";
    alloc
        .attrs
        .insert("class_definition".into(), AttrValue::Str(symbol.into()));
    alloc.attrs.insert(
        molt_ir::class_identity::STABLE_INSTANCE_CLASS_ATTR.into(),
        AttrValue::Bool(true),
    );
    push(&mut driver, entry, alloc);
    let accepted = proven_call(&mut driver, entry, FunctionId(0), &[cls, child]);
    let opaque = proven_call(&mut driver, entry, FunctionId(0), &[cls, unknown]);
    let discarded = proven_call(&mut driver, entry, FunctionId(0), &[cls, child]);
    push(
        &mut driver,
        entry,
        TirOp::new(OpCode::DecRef, vec![discarded], vec![]),
    );
    let dynamic = proven_call(&mut driver, entry, FunctionId(0), &[cls, child]);
    driver
        .blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .iter_mut()
        .find(|op| op.results == [dynamic])
        .unwrap()
        .set_proven_target(false);
    let result = driver.fresh_value();
    let mut tuple = TirOp::new(
        OpCode::BuildTuple,
        vec![accepted, opaque, dynamic],
        vec![result],
    );
    tuple.origin = Some(OpKind::TupleNew);
    push(&mut driver, entry, tuple);
    driver.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };
    // Turning refusal into an ordinary empty return or a return of a
    // different parameter defeats the projection in both cases.
    for keep_operand in [false, true] {
        let mut changed = validator.clone();
        for block in changed.blocks.values_mut() {
            if let Some(raise) = block.ops.pop_if(|op| op.opcode == OpCode::Raise) {
                block.terminator = Terminator::Return {
                    values: if keep_operand {
                        raise.operands
                    } else {
                        Vec::new()
                    },
                };
            }
        }
        assert_eq!(
            crate::tir::type_refine::ExactScalarPlan::new(&changed).forwarded_return_parameter(),
            None
        );
    }
    for target in [llvm(), cranelift()] {
        let mut module = TirModule::with_own_table("m", vec![validator.clone(), driver.clone()]);
        let outcome = run(
            &mut module,
            &target,
            &HashSet::from_iter(["validate".into()]),
            false,
        );
        assert_eq!(outcome.contexts, 2);
        assert!(outcome.clones.is_empty());
        let calls = calls(body(&module, "driver"));
        assert_eq!(calls.len(), 4);
        assert_eq!(
            calls[0].exact_result_proof(),
            Some(TirType::Class(molt_ir::ClassId::of_definition(symbol)))
        );
        for op in &calls[1..] {
            assert_eq!(op.exact_result_proof(), None);
        }
    }
}

#[test]
fn shared_call_returns_the_same_formal_through_an_ssa_join_and_backedge() {
    use crate::tir::blocks::TirBlock;
    use crate::tir::values::TirValue;

    let mut callee = function("preserve", 2);
    let entry = callee.entry_block;
    let x = callee.blocks[&entry].args[0].id;
    let test = callee.blocks[&entry].args[1].id;
    let loop_block = callee.fresh_block();
    let exit = callee.fresh_block();
    let carried = callee.fresh_value();
    let result = callee.fresh_value();
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: loop_block,
        args: vec![x],
    };
    callee.blocks.insert(
        loop_block,
        TirBlock {
            id: loop_block,
            args: vec![TirValue {
                id: carried,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: test,
                then_block: loop_block,
                then_args: vec![carried],
                else_block: exit,
                else_args: vec![carried],
            },
        },
    );
    callee.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![TirValue {
                id: result,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );
    let driver = caller(
        "driver",
        FunctionId(0),
        &[Literal::Int(41), Literal::Int(0)],
    );
    let partitioned = HashSet::from_iter(["preserve".into()]);
    for target in [llvm(), cranelift()] {
        let mut module = TirModule::with_own_table("m", vec![callee.clone(), driver.clone()]);
        let outcome = run(&mut module, &target, &partitioned, false);
        assert!(outcome.clones.is_empty());
        assert_eq!(
            calls(body(&module, "driver"))[0].exact_result_proof(),
            Some(TirType::I64)
        );
    }
    let Terminator::CondBranch { then_args, .. } =
        &mut callee.blocks.get_mut(&loop_block).unwrap().terminator
    else {
        unreachable!()
    };
    *then_args = vec![test];
    assert_eq!(
        crate::tir::type_refine::ExactScalarPlan::new(&callee).forwarded_return_parameter(),
        None
    );
}

#[test]
fn an_unobserved_raise_can_be_cleared_before_an_empty_return() {
    use crate::tir::lower_from_simple::lower_to_tir;
    let ir: molt_ir::FunctionIR = serde_json::from_value(serde_json::json!({
        "name":"recover", "params":["x","test"], "return_abi":"value",
        "ops":[
            {"kind":"if","args":["test"]},
            {"kind":"ret","args":["x"]},
            {"kind":"end_if"},
            {"kind":"raise","args":["x"]},
            {"kind":"check_exception"},
            {"kind":"jump","value":7},
            {"kind":"label","value":7},
            {"kind":"exception_clear"},
            {"kind":"ret_void"}
        ]
    }))
    .unwrap();
    let body = lower_to_tir(&ir);
    assert_eq!(
        crate::tir::type_refine::ExactScalarPlan::new(&body).forwarded_return_parameter(),
        None
    );
}

#[test]
fn clones_that_compile_alike_fold_into_their_origin() {
    // def keep(x): opaque(x); return None. The opaque use makes x relevant
    // by rule, but the body compiles the same for every argument type.
    let mut keep = function("keep", 1);
    let entry = keep.entry_block;
    let x = keep.blocks[&entry].args[0].id;
    let sink = keep.fresh_value();
    let word = keep.fresh_value();
    push(
        &mut keep,
        entry,
        TirOp::new(OpCode::ConstNone, vec![], vec![word]),
    );
    let mut opaque = TirOp::new(OpCode::Call, vec![word, x], vec![sink]);
    opaque.origin = Some(molt_ir::OpKind::CallFunc);
    push(&mut keep, entry, opaque);
    observe(&mut keep, entry);
    keep.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![sink] };
    let mut module = TirModule::with_own_table(
        "m",
        vec![
            keep,
            caller("a", FunctionId(0), &[Literal::Int(1)]),
            caller("b", FunctionId(0), &[Literal::Float(1.0)]),
        ],
    );
    // Every body arrives optimized, as the per-function pipeline leaves it.
    for func in &mut module.functions {
        crate::tir::type_refine::refine_types(func);
        let _ = crate::tir::passes::run_pipeline(func, &llvm());
        crate::tir::type_refine::refine_types(func);
    }
    let outcome = run(&mut module, &llvm(), &HashSet::default(), true);
    assert!(outcome.clones.is_empty(), "{:#?}", outcome.report);
    assert_eq!(module.functions.len(), 3);
    assert_eq!(module.table.len(), 3);
    for name in ["a", "b"] {
        assert_eq!(calls(body(&module, name))[0].callee, Some(FunctionId(0)));
    }
}

#[test]
fn clone_merge_reports_the_representative_after_compaction() {
    let constant_body = |name, value| {
        let mut func = function(name, 0);
        let entry = func.entry_block;
        let result = const_int(&mut func, entry, value);
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
            values: vec![result],
        };
        func
    };
    let mut module = TirModule::with_own_table(
        "m",
        vec![
            constant_body("f", 1),
            constant_body("f__tf_i", 1),
            constant_body("f__tf_f", 2),
            constant_body("f__tf_b", 2),
        ],
    );
    let mut outcome = TypeFlowOutcome::default();
    let mut report = Vec::new();
    merge_identical(&mut module, 1, &[0, 0, 0], 1, &mut outcome, &mut report);

    assert_eq!(module.table.symbol(FunctionId(1)), "f__tf_f");
    assert_eq!(
        report,
        [
            "[type-flow] merged f__tf_b into f__tf_f",
            "[type-flow] merged f__tf_i into f",
        ]
    );
}

#[test]
fn merged_seeds_keep_only_what_both_prove() {
    assert_eq!(
        merged_seeds(
            &[TirType::I64, TirType::F64, TirType::DynBox],
            &[TirType::I64, TirType::I64, TirType::Bool],
        ),
        vec![TirType::I64, TirType::DynBox, TirType::DynBox]
    );
}

#[test]
fn return_join_treats_the_int_family_as_int() {
    assert_eq!(
        sites::join_return(&TirType::Never, &TirType::BigInt),
        TirType::BigInt
    );
    assert_eq!(
        sites::join_return(&TirType::I64, &TirType::BigInt),
        TirType::I64
    );
    assert_eq!(
        sites::join_return(&TirType::I64, &TirType::F64),
        TirType::DynBox
    );
    assert_eq!(
        sites::join_return(&TirType::Bool, &TirType::Bool),
        TirType::Bool
    );
}

#[test]
fn relevance_propagates_through_forwarded_values_and_reversed_call_order() {
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::values::TirValue;

    // Every earlier row forwards its parameters, reversed, to the next row.
    // The leaf observes only y. Refcount-only uses and unseeded recursive
    // edges must not manufacture demand for the other argument.
    let chain = 80;
    let mut functions = Vec::new();
    for row in 0..chain {
        let mut func = function(&format!("f{row}"), 2);
        let entry = func.entry_block;
        let [x, y] = [
            func.blocks[&entry].args[0].id,
            func.blocks[&entry].args[1].id,
        ];
        if row + 1 == chain {
            func.blocks.get_mut(&entry).unwrap().terminator =
                Terminator::Return { values: vec![y] };
        } else {
            let next = func.fresh_block();
            let a = func.fresh_value();
            let b = func.fresh_value();
            func.blocks.insert(
                next,
                TirBlock {
                    id: next,
                    args: vec![
                        TirValue {
                            id: a,
                            ty: TirType::DynBox,
                        },
                        TirValue {
                            id: b,
                            ty: TirType::DynBox,
                        },
                    ],
                    ops: Vec::new(),
                    terminator: Terminator::Unreachable,
                },
            );
            func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
                target: next,
                args: vec![x, y],
            };
            let copy = func.fresh_value();
            push(
                &mut func,
                next,
                TirOp::new(OpCode::Copy, vec![a], vec![copy]),
            );
            push(&mut func, next, TirOp::new(OpCode::IncRef, vec![a], vec![]));
            let result = proven_call(&mut func, next, FunctionId::from_index(row + 1), &[b, copy]);
            func.blocks.get_mut(&next).unwrap().terminator = Terminator::Return {
                values: vec![result],
            };
        }
        functions.push(func);
    }
    for row in chain..chain + 2 {
        let mut func = function(&format!("cycle{row}"), 1);
        let entry = func.entry_block;
        let x = func.blocks[&entry].args[0].id;
        let other = if row == chain { chain + 1 } else { chain };
        let result = proven_call(&mut func, entry, FunctionId::from_index(other), &[x]);
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
            values: vec![result],
        };
        functions.push(func);
    }
    let module = TirModule::with_own_table("m", functions);
    let body_index = module.body_index();
    let members = exact_receiver_fields::exact_members(&module, &body_index);
    let mut sites: Vec<_> = module
        .functions
        .iter()
        .map(|func| sites::sites_of(func, &body_index, &module, &members))
        .collect();
    let relevant = relevance::solve(&module, &mut sites, &body_index, &vec![true; chain + 2]);

    for (row, actual) in relevant[..chain].iter().enumerate() {
        let x = (chain - 1 - row) % 2 != 0;
        assert_eq!(actual, &[x, !x], "row {row}");
    }
    assert_eq!(relevant[chain..], [vec![false], vec![false]]);
}

#[test]
fn relevance_depends_on_uses_past_the_sixty_fourth_parameter() {
    let mut leaf = function("leaf", 66);
    let entry = leaf.entry_block;
    let used = leaf.blocks[&entry].args[64].id;
    leaf.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![used] };
    let mut caller = function("entry", 66);
    let entry = caller.entry_block;
    let args: Vec<_> = caller.blocks[&entry]
        .args
        .iter()
        .map(|arg| arg.id)
        .collect();
    let result = proven_call(&mut caller, entry, FunctionId::from_index(1), &args);
    caller.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };
    let module = TirModule::with_own_table("m", vec![caller, leaf]);
    let body_index = module.body_index();
    let members = exact_receiver_fields::exact_members(&module, &body_index);
    let mut sites: Vec<_> = module
        .functions
        .iter()
        .map(|func| sites::sites_of(func, &body_index, &module, &members))
        .collect();
    let mut expected = vec![false; 66];
    expected[64] = true;
    assert_eq!(
        relevance::solve(&module, &mut sites, &body_index, &[true, true]),
        vec![expected.clone(), expected]
    );
}

fn method_fixture_function(name: &str, params: &[&str], ops: serde_json::Value) -> TirFunction {
    use crate::tir::lower_from_simple::lower_to_tir;
    use molt_ir::FunctionIR;
    lower_to_tir(&FunctionIR {
        name: name.into(),
        params: params.iter().map(|p| p.to_string()).collect(),
        ops: serde_json::from_value(ops).unwrap(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ..Default::default()
    })
}

#[test]
fn exact_method_feeds_arguments_and_results_back_into_type_flow() {
    use molt_ir::{OpKind, ParameterCustody};
    use serde_json::json;

    let definition = "class:m:Reader";
    let mut factory = method_fixture_function(
        "factory",
        &["cls", "cells"],
        json!([
            {"kind":"func_new_closure", "value":2, "args":["cells"], "out":"fn",
             "exact_method":[definition, "read"]},
            {"kind":"class_set_definition", "args":["cls"], "class_definition":definition},
            {"kind":"object_new_bound", "args":["cls"], "out":"receiver",
             "class_definition":definition, "stable_instance_class":true},
            {"kind":"ret","args":["receiver"]}
        ]),
    );
    factory
        .blocks
        .values_mut()
        .flat_map(|b| &mut b.ops)
        .find(|op| op.origin == Some(OpKind::FuncNewClosure))
        .unwrap()
        .callee = Some(FunctionId(1));
    let mut read = method_fixture_function(
        "read",
        &[molt_ir::MOLT_CLOSURE_PARAM_NAME, "self", "arg"],
        json!([
            {"kind":"const", "value":1, "out":"one"},
            {"kind":"add", "args":["arg","one"], "out":"sum"},
            {"kind":"ret","args":["sum"]}
        ]),
    );
    read.set_parameter_custody(&[
        ParameterCustody::Borrowed,
        ParameterCustody::Transferred,
        ParameterCustody::Transferred,
    ]);
    let wrapper = method_fixture_function(
        "wrapper",
        &["receiver", "arg"],
        json!([
            {"kind":"load_method","args":["receiver"],"s_value":"read","var":"f","out":"s"},
            {"kind":"call_loaded_method","args":["f","s","arg"],"out":"answer",
             "argument_custody":["transferred","transferred","transferred"]},
            {"kind":"ret","args":["answer"]}
        ]),
    );
    let mut main = function("main", 2);
    let entry = main.entry_block;
    let args: Vec<_> = main.blocks[&entry].args.iter().map(|arg| arg.id).collect();
    let receiver = proven_call(&mut main, entry, FunctionId(0), &args);
    let arg = const_int(&mut main, entry, 40);
    let result = proven_call(&mut main, entry, FunctionId(2), &[receiver, arg]);
    main.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    let mut module = TirModule::with_own_table("m", vec![factory, read, wrapper, main]);
    let flow = run(&mut module, &llvm(), &HashSet::default(), true);
    let find_clone = |origin| {
        flow.clones
            .iter()
            .find(|clone| clone.origin == FunctionId(origin))
            .unwrap_or_else(|| panic!("{:#?}", flow.report))
            .id
    };
    let read_clone = find_clone(1);
    let wrapper_clone = find_clone(2);
    let specialized = calls(body(&module, module.table.symbol(wrapper_clone)));
    assert_eq!(specialized[0].callee, Some(read_clone));
    assert_eq!(specialized[0].exact_result_proof(), Some(TirType::I64));
    assert_eq!(
        calls(body(&module, "main"))[1].exact_result_proof(),
        Some(TirType::I64)
    );
    let generic = body(&module, "wrapper");
    assert!(
        generic.blocks.values().flat_map(|b| &b.ops).any(|op| {
            op.opcode == OpCode::CallLoadedMethod && op.exact_result_proof().is_none()
        })
    );
}

#[test]
fn exact_method_uses_the_original_load_pair_and_checks_physical_binding() {
    use crate::tir::lower_to_simple::lower_to_simple_ir;
    use molt_ir::{OpKind, ParameterCustody};
    use serde_json::json;

    let definition = "class:m:Reader";
    let class = TirType::Class(molt_ir::ClassId::of_definition(definition));
    let mut factory = method_fixture_function(
        "factory",
        &["cls", "cells"],
        json!([
            {"kind":"func_new_closure", "value":2, "args":["cells"], "out":"fn",
             "exact_method":[definition, "read"]},
            {"kind":"class_set_definition", "args":["cls"], "class_definition":definition},
            {"kind":"ret","args":["fn"]}
        ]),
    );
    let creation = factory
        .blocks
        .values_mut()
        .flat_map(|b| &mut b.ops)
        .find(|op| op.origin == Some(OpKind::FuncNewClosure))
        .unwrap();
    creation.callee = Some(FunctionId(1));
    let wire = lower_to_simple_ir(&factory);
    let publication = wire
        .iter()
        .find(|op| op.kind == OpKind::FuncNewClosure)
        .unwrap();
    assert_eq!(publication.callee, Some(FunctionId(1)));
    let binding = publication.exact_method.as_ref().unwrap();
    assert_eq!((&*binding.0, &*binding.1), (definition, "read"));

    let mut target = method_fixture_function(
        "reader_body",
        &[molt_ir::MOLT_CLOSURE_PARAM_NAME, "self", "arg"],
        json!([{"kind":"ret","args":["arg"]}]),
    );
    target.set_parameter_custody(&[
        ParameterCustody::Borrowed,
        ParameterCustody::Transferred,
        ParameterCustody::Transferred,
    ]);
    let mut caller = method_fixture_function(
        "reader",
        &["receiver", "other", "callback"],
        json!([
            {"kind":"load_method","args":["receiver"],"s_value":"read","var":"f","out":"s"},
            {"kind":"load_method","args":["other"],"s_value":"read","var":"g","out":"t"},
            {"kind":"call_indirect","args":["callback"],"out":"arg"},
            {"kind":"call_loaded_method","args":["f","s","arg"],"out":"chosen","argument_custody":["transferred","transferred","transferred"]},
            {"kind":"call_loaded_method","args":["g","t","arg"],"out":"dynamic","argument_custody":["transferred","transferred","transferred"]},
            {"kind":"call_loaded_method","args":["f","t","arg"],"out":"mixed","argument_custody":["transferred","transferred","transferred"]},
            {"kind":"call_loaded_method","args":["f","s"],"out":"short","argument_custody":["transferred","transferred"]},
            {"kind":"ret","args":["chosen"]}
        ]),
    );
    caller.exact_param_types = vec![class, TirType::DynBox, TirType::DynBox];
    let module = TirModule::with_own_table("m", vec![factory, target, caller.clone()]);
    let entries = |m: &TirModule| exact_receiver_fields::exact_members(m, &m.body_index());
    let ops = &caller.blocks[&caller.entry_block].ops;
    let call_index = ops
        .iter()
        .position(|op| op.opcode == OpCode::CallLoadedMethod)
        .unwrap();
    let loaded = ops[call_index].clone();
    let mut selected = caller.clone();
    assert_eq!(
        exact_receiver_fields::run(&mut selected, &entries(&module)),
        2
    );
    let rewritten = &selected.blocks[&selected.entry_block].ops;
    let mut prefix = ops[..call_index].to_vec();
    prefix
        .iter_mut()
        .find(|op| op.opcode == OpCode::LoadMethod)
        .unwrap()
        .set_proven_target(true);
    assert_eq!(
        serde_json::to_value(&rewritten[..call_index]).unwrap(),
        serde_json::to_value(&prefix).unwrap()
    );
    let call = &rewritten[call_index];
    assert_eq!(call.operands, loaded.operands);
    assert_eq!(call.argument_custody(), loaded.argument_custody());
    assert_eq!(
        molt_ir::tir::call_targets::static_call(call)
            .unwrap()
            .callee,
        FunctionId(1)
    );
    assert_eq!(
        serde_json::to_value(&rewritten[call_index + 1..]).unwrap(),
        serde_json::to_value(&ops[call_index + 1..]).unwrap()
    );
    // Remove the intentionally crossed dynamic pair before whole-body verification.
    let block = selected.blocks.get_mut(&selected.entry_block).unwrap();
    block.ops.remove(call_index + 2);
    molt_ir::tir::verify::verify_function(&selected).unwrap();
    for duplicate in [false, true] {
        let mut inconsistent = module.clone();
        if duplicate {
            let block = inconsistent.functions[0]
                .blocks
                .values_mut()
                .find(|b| b.ops.iter().any(|op| op.callee.is_some()))
                .unwrap();
            let row = block
                .ops
                .iter()
                .find(|op| op.callee.is_some())
                .unwrap()
                .clone();
            block.ops.push(row);
        } else {
            inconsistent.functions[1].set_parameter_custody(&[ParameterCustody::Borrowed; 3]);
        }
        assert_eq!(
            exact_receiver_fields::run(&mut caller.clone(), &entries(&inconsistent)),
            0
        );
    }
}

#[test]
fn late_exact_method_reads_its_published_object_before_arguments() {
    use crate::tir::lower_to_simple::lower_to_simple_ir;
    use molt_ir::{OpKind, ParameterCustody};
    use serde_json::json;
    let definition = "class:m:Reader";
    let class = TirType::Class(molt_ir::ClassId::of_definition(definition));
    #[derive(Debug, PartialEq)]
    enum Case {
        Published,
        Overwritten,
        Uncertified,
        LateStore,
        MissingArgument,
        AdditionalConsumer,
    }
    use Case::*;
    for case in [
        Published,
        Overwritten,
        Uncertified,
        LateStore,
        MissingArgument,
        AdditionalConsumer,
    ] {
        let once = case != Uncertified;
        let slot = json!({"module":"m","index":4,"cell":true,"init_once":once});
        let stamp =
            json!({"kind":"class_set_definition","args":["cls"],"class_definition":definition});
        let store = json!({"kind":"static_cell_set","args":["base","fn"],"global_slot":slot});
        let (first, second) = if case != LateStore {
            (store, stamp)
        } else {
            (stamp, store)
        };
        let later = if case == Overwritten {
            json!({"kind":"static_cell_set","args":["base","env"],"global_slot":{"module":"m","index":4,"cell":true}})
        } else {
            json!({"kind":"nop"})
        };
        let mut factory = method_fixture_function(
            "factory",
            &["cls", "env"],
            json!([
                {"kind":"func_new_closure","value":2,"args":["env"],"out":"fn","exact_method":[definition,"read"]},
                {"kind":"static_cell_base","s_value":"m","out":"base"},
                first, second, later, {"kind":"ret","args":["fn"]}
            ]),
        );
        factory
            .blocks
            .values_mut()
            .flat_map(|b| &mut b.ops)
            .find(|op| op.origin == Some(OpKind::FuncNewClosure))
            .unwrap()
            .callee = Some(FunctionId(1));
        let mut target = method_fixture_function(
            "read",
            &[molt_ir::MOLT_CLOSURE_PARAM_NAME, "self", "arg"],
            json!([{"kind":"ret","args":["arg"]}]),
        );
        target.set_parameter_custody(&[
            ParameterCustody::Borrowed,
            ParameterCustody::Transferred,
            ParameterCustody::Transferred,
        ]);
        let other_use = if case == AdditionalConsumer {
            json!({"kind":"tuple_new","args":["fn"],"out":"held"})
        } else {
            json!({"kind":"nop"})
        };
        let args = if case == MissingArgument {
            vec!["fn", "self_arg"]
        } else {
            vec!["fn", "self_arg", "arg"]
        };
        let mut caller = method_fixture_function(
            "use",
            &["receiver", "callback"],
            json!([
                {"kind":"load_method","args":["receiver"],"s_value":"read","var":"fn","out":"self_arg"},
                {"kind":"call_indirect","args":["callback"],"out":"arg"},
                other_use,
                {"kind":"call_loaded_method","args":args,"out":"answer","argument_custody":vec!["transferred";args.len()]},
                {"kind":"ret","args":["answer"]}
            ]),
        );
        caller.exact_param_types = vec![class.clone(), TirType::DynBox];
        let module = TirModule::with_own_table("m", vec![factory, target, caller.clone()]);
        let members = exact_receiver_fields::exact_members(&module, &module.body_index());
        exact_receiver_fields::run(&mut caller, &members);
        molt_ir::tir::verify::verify_function(&caller).unwrap();
        let ops = &caller.blocks[&caller.entry_block].ops;
        assert_eq!(
            ops.iter().any(|op| op.opcode == OpCode::LoadMethod),
            case != Published,
            "{case:?}"
        );
        if case == Published {
            let load = ops
                .iter()
                .position(|op| op.opcode == OpCode::StaticCellLoad)
                .unwrap();
            let chosen = ops
                .iter()
                .find(|op| op.origin == Some(OpKind::CallGuarded))
                .unwrap();
            let callback = ops
                .iter()
                .position(|op| op.origin == Some(OpKind::CallIndirect))
                .unwrap();
            assert!(load + 2 < callback);
            let slot = ops[load].global_slot().unwrap();
            assert_eq!(
                (slot.module.as_str(), slot.index, slot.cell, slot.init_once),
                ("m", 4, true, true)
            );
            let callable = &ops[load + 1];
            let receiver = &ops[load + 2];
            assert_eq!(callable.origin, Some(OpKind::BindingAlias));
            assert_eq!(receiver.origin, Some(OpKind::BindingAlias));
            assert_eq!(callable.operands, ops[load].results);
            assert_eq!(
                chosen.operands[..2],
                [callable.results[0], receiver.results[0]]
            );
            assert_eq!(chosen.callee, Some(FunctionId(1)));
            let emitted = lower_to_simple_ir(&caller);
            assert!(emitted.iter().any(|op| op.kind == OpKind::StaticCellLoad));
            assert!(!emitted.iter().any(|op| op.kind == OpKind::LoadMethod));
        }
    }
}

#[test]
fn late_method_cell_is_allocated_only_by_the_matching_storage_owner_and_use() {
    use molt_ir::ir::{ModuleRegistryIR, ModuleRegistryRowIR};
    use molt_ir::runtime_fn_generated::RuntimeFn;
    use molt_ir::{OpKind, ParameterCustody};
    use serde_json::json;

    #[derive(Debug, PartialEq)]
    enum Case {
        Demanded,
        Generic,
        SplitMaker,
        SplitBind,
        SplitCaller,
        FrozenStorage,
        WrongBase,
        SharedPair,
        RepeatedBind,
        MissingCheck,
        InterveningCall,
        HandlerStamp,
    }
    use Case::*;
    let class = "class:m:C";
    for case in [
        Demanded,
        Generic,
        SplitMaker,
        SplitBind,
        SplitCaller,
        FrozenStorage,
        WrongBase,
        SharedPair,
        RepeatedBind,
        MissingCheck,
        InterveningCall,
        HandlerStamp,
    ] {
        let mut create =
            vec![json!({"kind":"func_new","out":"fn","value":1,"exact_method":[class,"read","m"]})];
        if case == InterveningCall {
            create.push(json!({"kind":"call_indirect","args":["cls"],"out":"side"}));
        }
        if case != MissingCheck {
            create.push(json!({"kind":"check_exception","value":9,"exception_edge":"origin"}));
        }
        let stamp = json!({"kind":"class_set_definition","args":["cls"],"class_definition":class});
        if case != HandlerStamp {
            create.push(stamp.clone());
        }
        create.extend([
            json!({"kind":"ret","args":["fn"]}),
            json!({"kind":"label","value":9}),
        ]);
        if case == HandlerStamp {
            create.push(stamp);
        }
        create.push(json!({"kind":"ret","args":["cls"]}));
        let mut factory = method_fixture_function("make", &["cls"], json!(create));
        factory
            .blocks
            .values_mut()
            .flat_map(|b| &mut b.ops)
            .find(|op| op.origin == Some(OpKind::FuncNew))
            .unwrap()
            .callee = Some(FunctionId(1));
        let mut target =
            method_fixture_function("target", &["self"], json!([{"kind":"ret","args":["self"]}]));
        target.set_parameter_custody(&[ParameterCustody::Transferred]);
        let owner = if case == WrongBase { "other" } else { "m" };
        let bind = json!({"kind":"call","s_value":"molt_static_cells_bind","args":["mod","base","length","length"],"out":"bound"});
        let mut ops = vec![
            json!({"kind":"static_cell_base","s_value":owner,"out":"base"}),
            json!({"kind":"const","value":1,"out":"length"}),
            bind.clone(),
        ];
        if case == RepeatedBind {
            ops.push(bind);
        }
        ops.push(json!({"kind":"ret","args":["length"]}));
        let initializer = method_fixture_function("init", &["mod"], json!(ops));
        let mut operations = vec![
            json!({"kind":"load_method","args":["obj"],"s_value":"read","var":"fn","out":"receiver"}),
        ];
        if case == SharedPair {
            operations.push(json!({"kind":"call_indirect","args":["callback","fn"],"out":"extra"}));
        }
        operations.extend([json!({"kind":"call_loaded_method","args":["fn","receiver"],"out":"answer","argument_custody":["transferred","transferred"]}), json!({"kind":"ret","args":["answer"]})]);
        let mut caller = method_fixture_function("caller", &["obj", "callback"], json!(operations));
        if case != Generic {
            caller.exact_param_types = vec![
                TirType::Class(molt_ir::ClassId::of_definition(class)),
                TirType::DynBox,
            ];
        }
        let mut module = TirModule::with_own_table("m", vec![factory, target, initializer, caller]);
        let mut registry = ModuleRegistryIR {
            rows: vec![ModuleRegistryRowIR {
                name: "m".into(),
                init_symbol: Some("init".into()),
                static_cells: Some(9),
            }],
            ..Default::default()
        };
        let external = match case {
            SplitMaker => ["make"].as_slice(),
            SplitBind => &["init"],
            SplitCaller => &["caller"],
            _ => &[],
        };
        let external = external.iter().map(|s| (*s).to_string()).collect();
        let admitted = (case != FrozenStorage).then_some(&mut registry);
        run_with_registry(&mut module, &llvm(), &external, false, admitted);
        let demanded = case == Demanded;
        assert_eq!(
            registry.static_cells("m"),
            Some(9 + u32::from(demanded)),
            "{case:?}"
        );
        let cell_ops: Vec<_> = module
            .functions
            .iter()
            .flat_map(|f| f.blocks.values())
            .flat_map(|b| &b.ops)
            .filter(|op| {
                op.global_slot()
                    .is_some_and(|s| s.index == 9 && s.cell && s.init_once)
            })
            .collect();
        assert_eq!(
            cell_ops
                .iter()
                .filter(|op| op.opcode == OpCode::StaticCellSet)
                .count(),
            usize::from(demanded),
            "{case:?}"
        );
        assert_eq!(
            cell_ops
                .iter()
                .filter(|op| op.opcode == OpCode::StaticCellLoad)
                .count(),
            usize::from(demanded),
            "{case:?}"
        );
        if demanded {
            let make = &module.functions[0];
            let store_block = make
                .blocks
                .values()
                .find(|b| b.ops.iter().any(|op| op.opcode == OpCode::StaticCellSet))
                .unwrap();
            let at = store_block
                .ops
                .iter()
                .position(|op| op.opcode == OpCode::StaticCellSet)
                .unwrap();
            assert!(
                store_block.ops[..at]
                    .iter()
                    .any(|op| op.opcode == OpCode::CheckException)
            );
            let ops: Vec<_> = module.functions[2]
                .blocks
                .values()
                .flat_map(|b| &b.ops)
                .collect();
            let bind = ops
                .iter()
                .find(|op| op.runtime == Some(RuntimeFn::StaticCellsBind))
                .unwrap();
            let definition = |v: ValueId| {
                ops.iter()
                    .find(|op| op.results == [v])
                    .unwrap()
                    .attrs
                    .get("value")
            };
            assert_eq!(definition(bind.operands[2]), Some(&AttrValue::Int(1)));
            assert_eq!(definition(bind.operands[3]), Some(&AttrValue::Int(2)));
        }
    }
}
