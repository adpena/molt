use molt_passes::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_passes::tir::function::{TirFunction, TirModule};
use molt_passes::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_passes::tir::passes::alias_analysis::AliasAnalysisResult;
use molt_passes::tir::passes::module_slot_promotion::run_module_slot_promotion;
use molt_passes::tir::types::TirType;
use molt_passes::tir::values::{TirValue, ValueId};

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn const_str(func: &mut TirFunction, s: &str) -> (TirOp, ValueId) {
    let r = func.fresh_value();
    let mut o = op(OpCode::ConstStr, vec![], vec![r]);
    o.attrs.insert("s_value".into(), AttrValue::Str(s.into()));
    (o, r)
}

fn const_int(func: &mut TirFunction, v: i64) -> (TirOp, ValueId) {
    let r = func.fresh_value();
    let mut o = op(OpCode::ConstInt, vec![], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(v));
    func.value_types.insert(r, TirType::I64);
    (o, r)
}

/// The bench_sum chunk shape: preheader sets total/i/N as module attrs,
/// a jump-shaped while loop reads/writes them per iteration with a
/// CheckException (handler label 7 → block 4), exit reads total.
fn module_loop_func() -> TirFunction {
    let mut f = TirFunction::new(
        "chunk".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let m = ValueId(0);
    let header = f.fresh_block();
    let body = f.fresh_block();
    let exit = f.fresh_block();
    let handler = f.fresh_block();

    // Preheader (entry): total = 0; i = 0; N = 100.
    let (ct0, ct0v) = const_str(&mut f, "total");
    let (zero_op, zero) = const_int(&mut f, 0);
    let (ci0, ci0v) = const_str(&mut f, "i");
    let (cn0, cn0v) = const_str(&mut f, "N");
    let (n_op, nval) = const_int(&mut f, 100);
    {
        let e = f.entry_block;
        let entry = f.blocks.get_mut(&e).unwrap();
        entry.ops = vec![
            ct0,
            zero_op,
            op(OpCode::ModuleSetAttr, vec![m, ct0v, zero], vec![]),
            ci0,
            op(OpCode::ModuleSetAttr, vec![m, ci0v, zero], vec![]),
            cn0,
            n_op,
            op(OpCode::ModuleSetAttr, vec![m, cn0v, nval], vec![]),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }

    // Header: vi = get i; vn = get N; cond = Lt(vi, vn); CondBranch.
    let (ci1, ci1v) = const_str(&mut f, "i");
    let vi = f.fresh_value();
    let (cn1, cn1v) = const_str(&mut f, "N");
    let vn = f.fresh_value();
    let cond = f.fresh_value();
    f.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![
                ci1,
                op(OpCode::ModuleGetAttr, vec![m, ci1v], vec![vi]),
                cn1,
                op(OpCode::ModuleGetAttr, vec![m, cn1v], vec![vn]),
                op(OpCode::Lt, vec![vi, vn], vec![cond]),
            ],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    // Body: vt = get total; sum = Add(vt, vi); CheckException(label 7);
    // set total = sum; ni = Add(vi, 1); set i = ni; Branch header.
    let (ct1, ct1v) = const_str(&mut f, "total");
    let vt = f.fresh_value();
    let sum = f.fresh_value();
    let (ct2, ct2v) = const_str(&mut f, "total");
    let (one_op, one) = const_int(&mut f, 1);
    let ni = f.fresh_value();
    let (ci2, ci2v) = const_str(&mut f, "i");
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(7));
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                ct1,
                op(OpCode::ModuleGetAttr, vec![m, ct1v], vec![vt]),
                op(OpCode::Add, vec![vt, vi], vec![sum]),
                check,
                ct2,
                op(OpCode::ModuleSetAttr, vec![m, ct2v, sum], vec![]),
                one_op,
                op(OpCode::Add, vec![vi, one], vec![ni]),
                ci2,
                op(OpCode::ModuleSetAttr, vec![m, ci2v, ni], vec![]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    // Exit: r = get total; return r.
    let (ct3, ct3v) = const_str(&mut f, "total");
    let r = f.fresh_value();
    f.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![ct3, op(OpCode::ModuleGetAttr, vec![m, ct3v], vec![r])],
            terminator: Terminator::Return { values: vec![r] },
        },
    );

    // Handler (label 7): bare return (the function exception exit).
    f.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f.label_id_map.insert(handler.0, 7);
    f
}

fn append_annotated_param(func: &mut TirFunction, ty: TirType) -> ValueId {
    let value = func.fresh_value();
    func.param_names
        .push(format!("p{}", func.param_names.len()));
    func.param_types.push(ty.clone());
    func.value_types.insert(value, ty.clone());
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .args
        .push(TirValue { id: value, ty });
    value
}

fn count_module_ops_in(func: &TirFunction, blocks: &[BlockId]) -> usize {
    blocks
        .iter()
        .map(|b| {
            func.blocks[b]
                .ops
                .iter()
                .filter(|o| matches!(o.opcode, OpCode::ModuleGetAttr | OpCode::ModuleSetAttr))
                .count()
        })
        .sum()
}

#[test]
fn promotes_bench_sum_shaped_loop() {
    let f = module_loop_func();
    let header = BlockId(1);
    let body = BlockId(2);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert_eq!(changed, vec!["chunk".to_string()], "function promoted");
    assert_eq!(stats.slots_promoted, 3, "total, i, N promoted");
    assert_eq!(stats.ops_eliminated, 5, "3 gets + 2 sets eliminated");
    let f = &module.functions[0];
    assert_eq!(
        count_module_ops_in(f, &[header, body]),
        0,
        "no module-attr traffic left inside the loop"
    );
    assert_eq!(f.blocks[&header].args.len(), 3, "carried phis added");
    // The merged function is structurally valid SSA.
    molt_passes::tir::verify::verify_function(f)
        .unwrap_or_else(|e| panic!("promoted fn invalid: {e:?}"));
    // A compensation block exists: some block (â‰  original handler) carries
    // ModuleSetAttr ops AND branches to the handler block (BlockId(4)).
    let handler = BlockId(4);
    let comp_exists = f.blocks.values().any(|b| {
        b.id != handler
            && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
            && matches!(
                &b.terminator,
                Terminator::Branch { target, .. } if *target == handler
            )
    });
    assert!(comp_exists, "CheckException compensation block present");
    // The exit path stores the dirty slots back (an edge block with sets
    // branching to the original exit block).
    let exit = BlockId(3);
    let exit_store_exists = f.blocks.values().any(|b| {
        b.id != exit
            && b.ops.iter().any(|o| o.opcode == OpCode::ModuleSetAttr)
            && matches!(
                &b.terminator,
                Terminator::Branch { target, .. } if *target == exit
            )
    });
    assert!(exit_store_exists, "exit-edge store-back block present");
}

#[test]
fn annotation_only_module_arithmetic_refuses_promotion() {
    let mut f = module_loop_func();
    let annotated_zero = append_annotated_param(&mut f, TirType::I64);
    let annotated_n = append_annotated_param(&mut f, TirType::I64);
    {
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops[2].operands[2] = annotated_zero;
        entry.ops[4].operands[2] = annotated_zero;
        entry.ops[7].operands[2] = annotated_n;
    }
    let before = format!("{f:?}");
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };

    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert!(
        changed.is_empty(),
        "annotations do not disprove Add callbacks"
    );
    assert_eq!(stats.slots_promoted, 0);
    assert_eq!(format!("{:?}", module.functions[0]), before);
}

#[test]
fn unknown_loop_writer_refuses_transaction_without_mutation() {
    let mut f = module_loop_func();
    let unknown = append_annotated_param(&mut f, TirType::DynBox);
    f.blocks.get_mut(&BlockId(2)).unwrap().ops[5].operands[2] = unknown;
    let before = format!("{f:?}");
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };

    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert!(
        changed.is_empty(),
        "an unknown mutable-slot writer must refuse"
    );
    assert_eq!(stats.slots_promoted, 0);
    assert_eq!(format!("{:?}", module.functions[0]), before);
}

#[test]
fn preheader_async_work_poll_refuses_transaction_without_mutation() {
    let mut f = module_loop_func();
    let mut poll = op(OpCode::CheckException, vec![], vec![]);
    assert!(poll.mark_async_work_poll());
    f.blocks.get_mut(&f.entry_block).unwrap().ops.push(poll);
    let before = format!("{f:?}");
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };

    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert!(
        changed.is_empty(),
        "a preheader poll can mutate module slots"
    );
    assert_eq!(stats.slots_promoted, 0);
    assert_eq!(format!("{:?}", module.functions[0]), before);
}

#[test]
fn unresolved_dirty_check_refuses_transaction_without_mutation() {
    let mut f = module_loop_func();
    f.label_id_map.clear();
    let before = format!("{f:?}");
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };

    let (stats, changed) = run_module_slot_promotion(&mut module);

    assert!(
        changed.is_empty(),
        "dirty state needs a resolvable compensation"
    );
    assert_eq!(stats.slots_promoted, 0);
    assert_eq!(format!("{:?}", module.functions[0]), before);
}

#[test]
fn threading_import_disables_promotion_module_wide() {
    let f = module_loop_func();
    // A second function importing `threading` â€” a concurrent observer of
    // module globals may then exist.
    let mut g = TirFunction::new(
        "spawner".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let imp_res = g.fresh_value();
    let mut imp = op(OpCode::Import, vec![], vec![imp_res]);
    imp.attrs
        .insert("s_value".into(), AttrValue::Str("threading".into()));
    {
        let e = g.entry_block;
        let entry = g.blocks.get_mut(&e).unwrap();
        entry.ops = vec![imp];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f, g],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert!(
        changed.is_empty(),
        "threading import => module-wide refusal"
    );
    assert_eq!(stats.slots_promoted, 0);
}

#[test]
fn thread_intrinsic_name_string_alone_does_not_refuse() {
    // The always-linked stdlib wrapper bodies carry `molt_thread_*` NAME
    // STRINGS (annotations, require_intrinsic args). A mere string must NOT
    // refuse promotion â€” only an Import of threading/_thread or a direct
    // molt_thread_* CALL does. (The over-broad string heuristic refused
    // every program: the needs_inlining trap, round two.)
    let f = module_loop_func();
    let mut g = TirFunction::new(
        "wrapper".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let (marker, _) = const_str(&mut g, "molt_thread_spawn");
    {
        let e = g.entry_block;
        let entry = g.blocks.get_mut(&e).unwrap();
        entry.ops = vec![marker];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f, g],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert_eq!(changed, vec!["chunk".to_string()], "string alone is benign");
    assert_eq!(stats.slots_promoted, 3);
}

#[test]
fn call_in_loop_refuses_promotion() {
    let mut f = module_loop_func();
    // Insert an opaque call into the loop body â€” GenericHeap aliases
    // ModuleDict, so the loop must be refused.
    let body = BlockId(2);
    let mut call = op(OpCode::Call, vec![], vec![]);
    call.attrs
        .insert("s_value".into(), AttrValue::Str("opaque".into()));
    f.blocks.get_mut(&body).unwrap().ops.insert(0, call);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert!(changed.is_empty(), "opaque call in loop => refusal");
    assert_eq!(stats.slots_promoted, 0);
}

fn typed_field_initialization_loop() -> TirFunction {
    let mut f = module_loop_func();
    let body = BlockId(2);
    let inst = f.fresh_value();
    let (value_op, val) = const_int(&mut f, 1);
    let mut allocation = op(OpCode::Alloc, vec![], vec![inst]);
    allocation.attrs.insert("value".into(), AttrValue::Int(8));
    let mut fset = op(OpCode::StoreAttr, vec![inst, val], vec![]);
    fset.attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    fset.attrs.insert("value".into(), AttrValue::Int(0));
    // Every iteration owns fresh storage. The shared exact-site analysis proves
    // this ordinary store release-neutral; no spelling or synthetic class
    // annotation is allowed to assert initialization.
    f.blocks
        .get_mut(&body)
        .unwrap()
        .ops
        .splice(0..0, [allocation, value_op, fset]);
    f
}

#[test]
fn typed_field_initialization_in_loop_does_not_refuse_promotion() {
    // Raw allocation plus first field publication has no old owner to release
    // and no Python callback. The allocation and store must use one local
    // region, disjoint from ModuleDict, without declaring either op pure.
    let f = typed_field_initialization_loop();
    molt_passes::tir::verify::verify_function(&f).expect("valid fresh-field loop");
    let alias = AliasAnalysisResult::compute(&f);
    let body = &f.blocks[&BlockId(2)];
    let expected = molt_passes::tir::passes::alias_analysis::MemRegion::LocalAllocation {
        root: body.ops[0].results[0],
    };
    assert_eq!(alias.region_of(&body.ops[0]), expected);
    assert_eq!(
        alias.region_of(&body.ops[2]),
        molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        "a context-free query cannot discharge old-value releases"
    );
    let mut am = molt_passes::tir::analysis::AnalysisManager::new();
    let stores = molt_passes::tir::passes::typed_slot_access::for_function(&f, &mut am);
    assert_eq!(
        stores.region_at(&alias, (BlockId(2), 2), &body.ops[2]),
        molt_passes::tir::passes::alias_analysis::MemRegion::Field {
            allocation: Some(body.ops[0].results[0]),
            offset: 0,
        }
    );
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert_eq!(
        changed,
        vec!["chunk".to_string()],
        "callback-free fresh-field initialization does not alias ModuleDict"
    );
    assert_eq!(stats.slots_promoted, 3);
    molt_passes::tir::verify::verify_function(&module.functions[0])
        .expect("valid promoted field loop");
}

#[test]
fn unrelated_raw_allocation_after_module_seeds_keeps_them_available() {
    let mut f = typed_field_initialization_loop();
    let unrelated = f.fresh_value();
    let mut allocation = op(OpCode::Alloc, vec![], vec![unrelated]);
    allocation.attrs.insert("value".into(), AttrValue::Int(8));
    f.blocks
        .get_mut(&f.entry_block)
        .unwrap()
        .ops
        .push(allocation);
    molt_passes::tir::verify::verify_function(&f).expect("valid allocation after module seeds");
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![f],
    };
    let (stats, changed) = run_module_slot_promotion(&mut module);
    assert_eq!(changed, vec!["chunk".to_string()]);
    assert_eq!(stats.slots_promoted, 3);
    molt_passes::tir::verify::verify_function(&module.functions[0])
        .expect("valid seeded promotion");
}

#[test]
fn field_initialization_does_not_hide_allocation_or_old_value_callbacks() {
    for boundary in [
        "object_new_bound",
        "alloc_class",
        "guarded_field_set",
        "poll",
    ] {
        let mut f = typed_field_initialization_loop();
        let class = append_annotated_param(&mut f, TirType::DynBox);
        let body = f.blocks.get_mut(&BlockId(2)).unwrap();
        match boundary {
            "object_new_bound" => {
                body.ops[0].opcode = OpCode::ObjectNewBound;
                body.ops[0].operands = vec![class];
            }
            "alloc_class" => {
                body.ops[0].opcode = OpCode::Copy;
                body.ops[0].operands = vec![class];
                body.ops[0]
                    .attrs
                    .insert("_original_kind".into(), AttrValue::Str(boundary.into()));
            }
            "guarded_field_set" => {
                body.ops[2]
                    .attrs
                    .insert("_original_kind".into(), AttrValue::Str(boundary.into()));
            }
            "poll" => {
                let mut poll = op(OpCode::CheckException, vec![], vec![]);
                assert!(poll.mark_async_work_poll());
                body.ops.insert(3, poll);
            }
            _ => unreachable!(),
        }
        molt_passes::tir::verify::verify_function(&f).expect("valid callback-control fixture");
        let before = format!("{f:?}");
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![f],
        };
        let (stats, changed) = run_module_slot_promotion(&mut module);
        assert!(
            changed.is_empty(),
            "{boundary} must preserve its callback floor"
        );
        assert_eq!(stats.slots_promoted, 0);
        assert_eq!(format!("{:?}", module.functions[0]), before);
    }
}
