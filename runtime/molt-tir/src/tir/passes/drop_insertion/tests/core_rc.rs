use super::*;

/// Regression (RC drop-insertion substrate, design 20): the real `accumulate`
/// loop-slot shape from the frontend SimpleIR, run through the FULL pipeline.
/// The loop loads its carried accumulator via `load_var`→`Copy` every
/// iteration; a per-SSA-value drop pass double-frees the live accumulator
/// (the activation blocker — `invalid object header before dec_ref` /
/// use-after-free at n≥50k). The alias-root-aware drop pass must drop each
/// underlying heap object EXACTLY ONCE per program point. This test asserts
/// the no-double-drop invariant directly on the post-pipeline TIR: within any
/// block, no two `DecRef`s name values that share an alias root.
#[test]
fn loop_slot_accumulator_no_double_drop() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::passes::alias_analysis::build_alias_union_find;
    use crate::tir::passes::run_pipeline;
    use crate::tir::type_refine::refine_types;

    let mk = |kind: &str,
              out: Option<&str>,
              var: Option<&str>,
              args: Vec<&str>,
              val: Option<i64>,
              sval: Option<&str>| OpIR {
        kind: kind.into(),
        out: out.map(|s| s.to_string()),
        var: var.map(|s| s.to_string()),
        args: if args.is_empty() {
            None
        } else {
            Some(args.iter().map(|s| s.to_string()).collect())
        },
        value: val,
        s_value: sval.map(|s| s.to_string()),
        ..OpIR::default()
    };
    // Shape from tmp/.../native/final_ir/bigint_accumulator__accumulate.txt:
    // total = 1<<60 ; i=0 ; while i<n: total=total+1; total=total-1; total=total+1; i=i+1 ; return total
    let func_ir = FunctionIR {
        name: "diag__accumulate".into(),
        params: vec!["n".into()],
        ops: vec![
            mk("const", Some("v106"), None, vec![], Some(1), None),
            mk("const", Some("v107"), None, vec![], Some(60), None),
            mk(
                "lshift",
                Some("v108"),
                None,
                vec!["v106", "v107"],
                None,
                None,
            ),
            mk("const", Some("v109"), None, vec![], Some(0), None),
            mk("const", Some("v114"), None, vec![], Some(1), None),
            mk("const", Some("v117"), None, vec![], Some(1), None),
            mk("const", Some("v120"), None, vec![], Some(1), None),
            mk("const", Some("v123"), None, vec![], Some(1), None),
            mk(
                "store_var",
                None,
                Some("_bb1_arg0"),
                vec!["v108"],
                None,
                None,
            ),
            mk(
                "store_var",
                None,
                Some("_bb1_arg1"),
                vec!["v109"],
                None,
                None,
            ),
            mk("jump", None, None, vec![], Some(8), None),
            mk("label", None, None, vec![], Some(8), None),
            mk("loop_start", None, None, vec![], None, None),
            mk(
                "load_var",
                Some("_v19"),
                Some("_bb1_arg0"),
                vec![],
                None,
                None,
            ),
            mk(
                "load_var",
                Some("_v20"),
                Some("_bb1_arg1"),
                vec![],
                None,
                None,
            ),
            mk("lt", Some("v112"), None, vec!["_v20", "n"], None, None),
            mk("loop_break_if_false", None, None, vec!["v112"], None, None),
            mk("add", Some("v115"), None, vec!["_v19", "v114"], None, None),
            mk("sub", Some("v118"), None, vec!["v115", "v117"], None, None),
            mk("add", Some("v121"), None, vec!["v118", "v120"], None, None),
            mk("add", Some("v124"), None, vec!["_v20", "v123"], None, None),
            mk(
                "store_var",
                None,
                Some("_bb1_arg0"),
                vec!["v121"],
                None,
                None,
            ),
            mk(
                "store_var",
                None,
                Some("_bb1_arg1"),
                vec!["v124"],
                None,
                None,
            ),
            mk("loop_continue", None, None, vec![], None, None),
            mk("loop_end", None, None, vec![], None, None),
            mk("jump", None, None, vec![], Some(12), None),
            mk("label", None, None, vec![], Some(12), None),
            mk("ret", None, Some("_v19"), vec!["_v19"], None, None),
        ],
        param_types: Some(vec!["Any".into()]),
        source_file: None,
        is_extern: false,
    };

    let mut tir_func = lower_to_tir(&func_ir);
    refine_types(&mut tir_func);
    // Run the full optimization pipeline to reach the realistic lowered loop
    // shape (Copy-aliased loop-slot loads), THEN run drop insertion directly.
    // The pass is a complete primitive but intentionally NOT wired into
    // `build_default_pipeline` yet (Phase-5 native-RC retirement is the
    // remaining activation prerequisite — see the pass_manager activation
    // note), so we invoke it explicitly here to exercise the alias-root
    // placement on the production-shaped IR.
    run_pipeline(
        &mut tir_func,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );
    {
        let mut am = AnalysisManager::new();
        run(&mut tir_func, &mut am);
    }

    // Invariant: within any block, no two DecRefs share an alias root — a
    // double-drop of one heap object is the activation-blocker use-after-free.
    let aliases = build_alias_union_find(&tir_func);
    for block in tir_func.blocks.values() {
        let mut dropped_roots: HashSet<ValueId> = HashSet::new();
        for op in &block.ops {
            if op.opcode == OpCode::DecRef {
                let root = aliases.root(op.operands[0]);
                assert!(
                    dropped_roots.insert(root),
                    "double-drop of alias root {root:?} in one block: {:?}",
                    block.ops
                );
            }
        }
    }
    // The loop body must drop SOMETHING (the dead intermediates + the prev
    // accumulator) — a fully-inert pass would mean the leak is unclosed.
    let total_decrefs: usize = tir_func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::DecRef)
        .count();
    assert!(
        total_decrefs >= 2,
        "loop accumulator must insert drops, got {total_decrefs}"
    );
}

/// Branch-arg transfer to a successor must NOT be edge-dropped (design §2.5).
/// Regression for the `while True: break` shape: `v` is computed in `entry`,
/// passed as a branch arg to `join`, and received as `join`'s block param `p`.
/// `v`'s ownership transfers to `p` across the edge — the edge-dying rule must
/// recognize the per-edge transfer (`incoming_arg_roots` via `terminator_arcs`)
/// and NOT also drop `v` at `join`'s entry. Doing so double-frees the object the param now
/// owns (the observed `invalid object header before dec_ref` UAF). `p` is then
/// returned (transferred to the caller), so the function inserts ZERO drops.
#[test]
fn branch_arg_transfer_not_edge_dropped() {
    let mut func = TirFunction::new("xfer".into(), vec![], TirType::DynBox);
    let v = func.fresh_value();
    let p = func.fresh_value();
    func.value_types.insert(v, TirType::Str);
    func.value_types.insert(p, TirType::Str);
    let join = func.fresh_block();
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(v));
        b.terminator = Terminator::Branch {
            target: join,
            args: vec![v],
        };
    }
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: p,
                ty: TirType::Str,
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![p] },
        },
    );
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // No DecRef of `v` (transferred to `p`), and none of `p` (returned).
    let dropped: Vec<ValueId> = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(
        !dropped.contains(&v),
        "branch-arg `v` transferred to the successor param must NOT be edge-dropped (double-free); dropped={dropped:?}",
    );
    assert_eq!(
        count_decrefs(&func),
        0,
        "transfer-through-edge + return must insert zero drops; dropped={dropped:?}",
    );
}

/// `IterNextUnboxed` writes its value result only on the not-done edge. A
/// Python-bound local fed from that value may be released through the loop
/// phi that carries the previous valid element, but the raw value result
/// itself must not be scheduled for Return-boundary cleanup on the exhausted
/// edge. Regression for the dict-values/tinygrad UAF:
/// `DecRef(iter_value)` ran after `done == true`, where the value slot still
/// held the previous element's stale pointer.
#[test]
fn iter_next_unboxed_value_not_return_boundary_dropped_on_exhaustion_edge() {
    let mut func = TirFunction::new(
        "iter_next_unboxed_conditional_value_exit".into(),
        vec![],
        TirType::None,
    );
    let iter = func.fresh_value();
    let initial_local = func.fresh_value();
    let local_phi = func.fresh_value();
    let iter_value = func.fresh_value();
    let done = func.fresh_value();
    let stored_local = func.fresh_value();
    for value in [iter, initial_local, local_phi, iter_value, stored_local] {
        func.value_types.insert(value, TirType::DynBox);
    }
    func.value_types.insert(done, TirType::Bool);

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(iter));
        b.ops.push(finalizer_object(initial_local));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![initial_local],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: local_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![op(
                OpCode::IterNextUnboxed,
                vec![iter],
                vec![iter_value, done],
            )],
            terminator: Terminator::CondBranch {
                cond: done,
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![original_copy_with_operands(
                "store_var",
                vec![iter_value],
                vec![stored_local],
            )],
            terminator: Terminator::Branch {
                target: header,
                args: vec![stored_local],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![op(OpCode::DelBoundary, vec![local_phi], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let exit_drops: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        exit_drops
            .iter()
            .filter(|&&value| value == local_phi)
            .count(),
        1,
        "the valid carried local owner must be released at the loop exit exactly once; exit_drops={exit_drops:?}"
    );
    assert!(
        !exit_drops.contains(&iter_value),
        "the conditionally valid iter value must not be dropped on the \
             exhausted edge; exit_drops={exit_drops:?}",
    );
}

/// Straight-line temp: v1 = Call(a); v2 = Call(v1); Return(v2).
/// v1 dies after op 2 → exactly one DecRef(v1). v2 is returned (transferred)
/// → not dropped.
#[test]
fn straight_line_temp_dropped_once() {
    let mut func = TirFunction::new("sl".into(), vec![], TirType::DynBox);
    let a = func.fresh_value();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();
    for v in [a, v1, v2] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(a));
        b.ops.push(op(OpCode::Call, vec![a], vec![v1]));
        b.ops.push(op(OpCode::Call, vec![v1], vec![v2]));
        b.terminator = Terminator::Return { values: vec![v2] };
    }
    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);
    assert!(stats.ops_added >= 1);
    // a dies after op 1; v1 dies after op 2; v2 is returned. So DecRef(a) and
    // DecRef(v1), not DecRef(v2).
    let decrefs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(decrefs.contains(&a), "a must be dropped at last use");
    assert!(decrefs.contains(&v1), "v1 must be dropped at last use");
    assert!(!decrefs.contains(&v2), "returned value must not be dropped");
    assert!(func.attrs.contains_key(DROP_INSERTED_ATTR));
}

/// `list_pop` removes an element from the list and returns a fresh owned
/// reference to that removed element. If the Python result is discarded, the
/// returned owner must be released immediately at the pop statement; otherwise
/// finalizer-bearing elements survive until unrelated container teardown
/// (`finalizer_container_clear.py`: `bag2.pop()` must run A(11).__del__ before
/// the following print).
#[test]
fn unused_list_pop_result_is_dropped_at_pop_boundary() {
    let mut func = TirFunction::new("list_pop_dead_result".into(), vec![], TirType::DynBox);
    let list = func.fresh_value();
    let idx = func.fresh_value();
    let popped = func.fresh_value();
    let ret = func.fresh_value();
    for v in [list, idx, popped, ret] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(list));
        b.ops.push(op(OpCode::ConstNone, vec![], vec![idx]));
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("list_pop".into()));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![list, idx],
            results: vec![popped],
            attrs,
            source_span: None,
        });
        b.ops.push(op(OpCode::ConstNone, vec![], vec![ret]));
        b.terminator = Terminator::Return { values: vec![ret] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let ops = &func.blocks[&entry].ops;
    let pop_idx = ops
        .iter()
        .position(|o| {
            o.opcode == OpCode::Copy
                && matches!(
                    o.attrs.get("_original_kind"),
                    Some(AttrValue::Str(k)) if k == "list_pop"
                )
        })
        .expect("list_pop op present");
    let dec_popped_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![popped])
        .expect("unused list_pop result must be dropped");
    assert!(
        dec_popped_idx > pop_idx,
        "list_pop result must be released after the runtime removes/returns it; ops={ops:?}"
    );
}

/// `dataclass_new_values` returns the newly-created instance as an owned
/// reference. Class attachment (`dataclass_set_class`) mutates metadata and
/// returns `None`, so the constructor result remains the only owner that can
/// trigger the instance finalizer at function exit. Releasing only field
/// operands leaks the parent instance and skips child-finalizer teardown.
#[test]
fn dataclass_new_values_result_is_dropped_after_last_metadata_use() {
    let mut func = TirFunction::new(
        "dataclass_new_values_owner_drop".into(),
        vec![],
        TirType::DynBox,
    );
    let name = func.fresh_value();
    let fields = func.fresh_value();
    let flags = func.fresh_value();
    let child = func.fresh_value();
    let instance = func.fresh_value();
    let class_obj = func.fresh_value();
    let ret = func.fresh_value();
    for v in [name, fields, flags, child, instance, class_obj, ret] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(name));
        b.ops.push(const_str(fields));
        b.ops.push(op(OpCode::ConstNone, vec![], vec![flags]));
        b.ops.push(op(OpCode::Call, vec![], vec![child]));
        let mut ctor_attrs = AttrDict::new();
        ctor_attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("dataclass_new_values".into()),
        );
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![name, fields, flags, child],
            results: vec![instance],
            attrs: ctor_attrs,
            source_span: None,
        });
        b.ops.push(op(OpCode::Call, vec![], vec![class_obj]));
        let mut set_class_attrs = AttrDict::new();
        set_class_attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("dataclass_set_class".into()),
        );
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![instance, class_obj],
            results: vec![],
            attrs: set_class_attrs,
            source_span: None,
        });
        b.ops.push(op(OpCode::ConstNone, vec![], vec![ret]));
        b.terminator = Terminator::Return { values: vec![ret] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let ops = &func.blocks[&entry].ops;
    let set_class_idx = ops
        .iter()
        .position(|o| {
            o.opcode == OpCode::Copy
                && matches!(
                    o.attrs.get("_original_kind"),
                    Some(AttrValue::Str(k)) if k == "dataclass_set_class"
                )
        })
        .expect("dataclass_set_class op present");
    let dec_instance_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![instance])
        .expect("dataclass instance result must be dropped");
    assert!(
        dec_instance_idx > set_class_idx,
        "dataclass instance owner must survive metadata attachment and then release; ops={ops:?}"
    );
}

/// A CallArgs builder consumed by `call_bind` / `call_indirect` must NOT get
/// a trailing DecRef: the runtime entry (`molt_call_bind_ic`, via
/// `PtrDropGuard`) frees the builder internally, so an inserted DecRef would
/// double-free the `TYPE_ID_CALLARGS` object (design-20 finding #3C: the
/// method-call `'invalid object header before dec_ref'` abort). The callee
/// (operand 0) and the call RESULT are still dropped normally.
#[test]
fn call_bind_callargs_operand_not_dropped() {
    let mut func = TirFunction::new("cb".into(), vec![], TirType::DynBox);
    let callee = func.fresh_value(); // the bound method (a fresh owned ref)
    let builder = func.fresh_value(); // the CallArgs builder
    let result = func.fresh_value(); // the call result
    for v in [callee, builder, result] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        // callee = <fresh owned value> (model as a Call so it is owned).
        b.ops.push(op(OpCode::Call, vec![], vec![callee]));
        // builder = callargs_new (opaque Copy carrying _original_kind).
        let mut ca = AttrDict::new();
        ca.insert(
            "_original_kind".into(),
            AttrValue::Str("callargs_new".into()),
        );
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![builder],
            attrs: ca,
            source_span: None,
        });
        // result = call_bind(callee, builder) — Call carrying _original_kind.
        let mut cb = AttrDict::new();
        cb.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![callee, builder],
            results: vec![result],
            attrs: cb,
            source_span: None,
        });
        b.terminator = Terminator::Return {
            values: vec![result],
        };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let decrefs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(
        !decrefs.contains(&builder),
        "the CallArgs builder is consumed by call_bind; it must NOT be DecRef'd (double-free)"
    );
    assert!(
        decrefs.contains(&callee),
        "the callee (borrowed-then-dead) must be dropped at its last use (the call)"
    );
    // The result is returned → not dropped here.
    assert!(!decrefs.contains(&result));
}

/// Interior-borrow keepalive (round-6 BLOCKER-1). A heap object's LAST DIRECT
/// operand use is a `LoadAttr` that extracts a value the object's backing store
/// owns (the `Counter._handle` raw-int registry-handle shape: the wrapper's
/// finalizer destroys the registry entry the handle indexes). The extracted
/// value `h` is then consumed by a later `Call`. The source object `obj` MUST be
/// dropped AFTER `h`'s last use (the Call), NEVER right after the `LoadAttr` —
/// dropping it earlier runs the finalizer and invalidates `h` (the observed UAF:
/// `len(Counter(...))` returned 0). Mirrors the de-sugared fast-path lowering
/// `h = get_attr(counts, "_handle"); molt_counter_len(h)`.
#[test]
fn loadattr_source_kept_alive_through_borrow_result_use() {
    let mut func = TirFunction::new("borrow".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value(); // the wrapper (fresh owned)
    let h = func.fresh_value(); // LoadAttr(obj) — borrows into obj's store
    let len_fn = func.fresh_value(); // the `molt_counter_len` builtin
    let res = func.fresh_value(); // Call(len_fn, h) result
    for v in [obj, h, len_fn, res] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(obj)); // op 0: obj = fresh owned
        b.ops.push(op(OpCode::LoadAttr, vec![obj], vec![h])); // op 1: h = obj._handle (last DIRECT use of obj)
        b.ops.push(const_str(len_fn)); // op 2: the builtin
        b.ops.push(op(OpCode::Call, vec![len_fn, h], vec![res])); // op 3: len(h) — needs obj alive
        b.terminator = Terminator::Return { values: vec![res] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let ops = &func.blocks[&entry].ops;
    // Find the Call (the consumer of the borrow result) and the DecRef(obj).
    let call_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::Call)
        .expect("call present");
    let decref_obj_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![obj]);
    assert!(
        decref_obj_idx.is_some(),
        "source object must still be dropped (no leak); ops={ops:?}"
    );
    assert!(
        decref_obj_idx.unwrap() > call_idx,
        "source object must be dropped AFTER the borrow result's consuming Call \
             (interior-borrow keepalive), not at its last direct operand use; \
             decref@{:?} call@{call_idx} ops={ops:?}",
        decref_obj_idx.unwrap(),
    );
}

/// Interior-borrow keepalive across a transparent `Copy` of the source (the
/// `load_var` shape): `obj` is loaded via a `Copy` (alias root = obj), the alias
/// feeds a `LoadAttr`, and the LoadAttr result is consumed later. The drop of
/// the underlying object (alias root) must still be deferred past the consumer.
#[test]
fn loadattr_keepalive_through_copy_aliased_source() {
    let mut func = TirFunction::new("borrow_alias".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let obj_alias = func.fresh_value(); // Copy(obj) — load_var alias
    let h = func.fresh_value(); // LoadAttr(obj_alias)
    let consumer = func.fresh_value(); // Call(h) result
    for v in [obj, obj_alias, h, consumer] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(obj));
        b.ops.push({
            let mut o = op(OpCode::Copy, vec![obj], vec![obj_alias]);
            o.attrs
                .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
            o
        });
        b.ops.push(op(OpCode::LoadAttr, vec![obj_alias], vec![h]));
        b.ops.push(op(OpCode::Call, vec![h], vec![consumer]));
        b.terminator = Terminator::Return {
            values: vec![consumer],
        };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let ops = &func.blocks[&entry].ops;
    let call_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::Call)
        .expect("call present");
    // The underlying object is released through some alias of its root, exactly
    // once, AFTER the consumer. Find any DecRef whose operand aliases obj's root.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
    let obj_root = aliases.root(obj);
    let decref_positions: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| {
            o.opcode == OpCode::DecRef
                && o.operands
                    .first()
                    .is_some_and(|&v| aliases.root(v) == obj_root)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        decref_positions.len(),
        1,
        "the source object's group must be released exactly once; ops={ops:?}"
    );
    assert!(
        decref_positions[0] > call_idx,
        "source object drop must follow the borrow result's consumer; \
             decref@{} call@{call_idx} ops={ops:?}",
        decref_positions[0],
    );
}

/// Raw i64 values get ZERO drops (perf contract / design R3).
#[test]
fn raw_i64_gets_no_drops() {
    let mut func = TirFunction::new("raw".into(), vec![], TirType::I64);
    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let s = func.fresh_value();
    for v in [c0, c1, s] {
        func.value_types.insert(v, TirType::I64);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        let mut a0 = AttrDict::new();
        a0.insert("value".into(), AttrValue::Int(3));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![c0],
            attrs: a0,
            source_span: None,
        });
        let mut a1 = AttrDict::new();
        a1.insert("value".into(), AttrValue::Int(4));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![c1],
            attrs: a1,
            source_span: None,
        });
        b.ops.push(op(OpCode::Add, vec![c0, c1], vec![s]));
        b.terminator = Terminator::Return { values: vec![s] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    assert_eq!(count_decrefs(&func), 0, "raw i64 lane must get zero drops");
}

/// StackAlloc values get ZERO drops (design R6).
#[test]
fn stack_alloc_gets_no_drops() {
    let mut func = TirFunction::new("st".into(), vec![], TirType::DynBox);
    let s = func.fresh_value();
    let used = func.fresh_value();
    func.value_types.insert(s, TirType::DynBox);
    func.value_types.insert(used, TirType::DynBox);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::StackAlloc, vec![], vec![s]));
        b.ops.push(op(OpCode::LoadAttr, vec![s], vec![used]));
        b.terminator = Terminator::Return { values: vec![used] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let decrefs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(!decrefs.contains(&s), "stack value must never be dropped");
}

/// A lowered coroutine `_poll` STATE MACHINE (a `StateSwitch` dispatch) must
/// get ZERO drops — the pass bails (`has_state_machine`). Regression for the
/// LLVM verifier failure where a drop placed in a state-resume block
/// referenced a value defined only on the non-taken first-entry path
/// (`dec_ref %v` before `%v = ...`; a use-before-def that also double-frees on
/// native). A generator can carry `StateSwitch` WITHOUT `StateBlock*`
/// delimiters, so the handler bail alone misses it.
#[test]
fn state_machine_function_gets_no_drops() {
    let mut func = TirFunction::new("poll".into(), vec![], TirType::DynBox);
    let st = func.fresh_value();
    let v = func.fresh_value();
    func.value_types.insert(st, TirType::I64);
    func.value_types.insert(v, TirType::Str);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        // A state-machine dispatch op marks this as a lowered `_poll` body.
        b.ops.push(op(OpCode::StateSwitch, vec![st], vec![]));
        // A heap temp whose naive last-use drop would be unsound over the
        // re-entrant state CFG.
        b.ops.push(const_str(v));
        b.ops.push(op(OpCode::Call, vec![v], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }
    assert!(
        func.has_state_machine(),
        "fixture must look like a lowered state machine",
    );
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    assert_eq!(
        count_decrefs(&func),
        0,
        "state-machine `_poll` body must get zero drops (pass bails)",
    );
    assert_eq!(count_increfs(&func), 0);
}

/// Loop-carried phi `s` used on BOTH the loop body (new value computed, old
/// `s` dead on the back-edge path) AND the exit path (a non-alias consumer),
/// in the real-phi (LLVM) shape. The header phi must be dropped on the path
/// where it dies — the back-edge body block — exactly once. Regression for the
/// LLVM string-concat leak: the drop pass inserted NO `DecRef(s_phi)` for this
/// shape (the accumulator's old value leaked every iteration: `dealloc=5/n`).
///
/// Shape (mirrors `string_concat__concat` after lowering):
///   entry: s0 = ConstStr; br header(s0)
///   header(s_phi): cond_br c, body, exit
///   body: s_new = Add(s_phi, "x"); br header(s_new)   // old s_phi dies here
///   exit: r = Len(s_phi); return r                    // s_phi consumed, dies
#[test]
fn loop_carried_phi_dropped_on_backedge() {
    let mut func = TirFunction::new("acc".into(), vec![], TirType::I64);
    let s0 = func.fresh_value();
    let s_phi = func.fresh_value();
    let s_alias = func.fresh_value();
    let lit = func.fresh_value();
    let cond = func.fresh_value();
    let s_new = func.fresh_value();
    let r = func.fresh_value();
    func.value_types.insert(s0, TirType::Str);
    func.value_types.insert(s_phi, TirType::Str);
    func.value_types.insert(s_alias, TirType::Str);
    func.value_types.insert(lit, TirType::Str);
    func.value_types.insert(cond, TirType::Bool);
    func.value_types.insert(s_new, TirType::Str);
    func.value_types.insert(r, TirType::I64);

    // Mirror the lowered `string_concat__concat` CFG precisely: the cond lives
    // in a SEPARATE block (`cond_blk`, real bb3) reached from the header, and a
    // transparent `Copy` of the phi (`s_alias`, real `%11 = copy %9`) is the
    // value actually consumed on BOTH the loop body and the exit paths. The
    // exit goes through an intermediate `pre_exit` block (real bb6). This is
    // the shape the simpler direct-header-cond fixture did NOT reproduce.
    let header = func.fresh_block();
    let cond_blk = func.fresh_block();
    let body = func.fresh_block();
    let pre_exit = func.fresh_block();
    let exit = func.fresh_block();
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(s0));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![s0],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: s_phi,
                ty: TirType::Str,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cond_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        cond_blk,
        TirBlock {
            id: cond_blk,
            args: vec![],
            // `s_alias = Copy(s_phi)` — a transparent alias (root = s_phi) used by
            // both successors; plus the loop condition.
            ops: vec![
                op(OpCode::Copy, vec![s_phi], vec![s_alias]),
                op(OpCode::ConstBool, vec![], vec![cond]),
            ],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: pre_exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                const_str(lit),
                op(OpCode::Add, vec![s_alias, lit], vec![s_new]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![s_new],
            },
        },
    );
    func.blocks.insert(
        pre_exit,
        TirBlock {
            id: pre_exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![],
            },
        },
    );
    // A fresh (non-alias) consumer of the aliased phi → it dies after it.
    // `Call` borrows its operand and returns a fresh owned value (the real IR
    // uses a `len`-carrying op here; the only property that matters for
    // liveness is that the result is NOT a transparent alias).
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![s_alias], vec![r])],
            terminator: Terminator::Return { values: vec![r] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    // The header phi `s_phi` (and the literal `lit`) are owned heap values that
    // die — `s_phi` on the back-edge body path and on the exit path, `lit`
    // after the Add. The pass MUST drop the accumulator; a fully-inert result
    // is the leak. Assert `s_phi` is dropped somewhere.
    let dropped: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(
        dropped.contains(&s_phi),
        "loop-carried phi accumulator must be dropped (else it leaks every \
             iteration); drops={dropped:?}",
    );
    // And no double-drop of any root within a single block.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
    for block in func.blocks.values() {
        let mut roots: HashSet<ValueId> = HashSet::new();
        for o in &block.ops {
            if o.opcode == OpCode::DecRef {
                assert!(
                    roots.insert(aliases.root(o.operands[0])),
                    "double-drop in one block: {:?}",
                    block.ops,
                );
            }
        }
    }
}

/// Parameters are borrowed — never dropped.
/// `IterNextUnboxed` writes its value result only on the not-done edge. A
/// following `store_var` makes that result look like a Python local-store
/// root, but the done edge reaches loop exit with the value slot
/// uninitialized. The local lifetime rail must therefore not schedule an
/// unconditional return-boundary `DecRef(value)` in the exit block.
#[test]
fn iter_next_unboxed_del_boundary_not_dropped_on_done_return_boundary() {
    let mut func = TirFunction::new(
        "iter_next_conditional_local_boundary".into(),
        vec![TirType::DynBox],
        TirType::None,
    );
    let iter = func.blocks[&func.entry_block].args[0].id;
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let seed = func.fresh_value();
    let current = func.fresh_value();
    let value = func.fresh_value();
    let done = func.fresh_value();
    let stored = func.fresh_value();
    for v in [seed, current, value, stored] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(done, TirType::Bool);

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_str(seed));
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![seed],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: current,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::IterNextUnboxed, vec![iter], vec![value, done])],
            terminator: Terminator::CondBranch {
                cond: done,
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![original_copy_with_operands(
                "store_var",
                vec![value],
                vec![stored],
            )],
            terminator: Terminator::Branch {
                target: header,
                args: vec![value],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![
                op(OpCode::WarnStderr, vec![], vec![]),
                op(OpCode::DelBoundary, vec![value], vec![]),
            ],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let exit_decrefs: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        !exit_decrefs.contains(&value),
        "done-edge return must not drop the conditionally-valid iterator value; exit drops={exit_decrefs:?}"
    );
    assert!(
        func.blocks[&exit]
            .ops
            .iter()
            .all(|op| op.opcode != OpCode::DelBoundary),
        "drop insertion must consume DelBoundary markers even when the safe action is deletion"
    );
}

#[test]
fn params_not_dropped() {
    let mut func = TirFunction::new("p".into(), vec![TirType::Str], TirType::DynBox);
    let p0 = ValueId(0);
    let r = func.fresh_value();
    func.value_types.insert(r, TirType::Str);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::Call, vec![p0], vec![r]));
        b.terminator = Terminator::Return { values: vec![r] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    let decrefs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(!decrefs.contains(&p0), "parameter must not be dropped");
}

/// Borrow inference: a value whose only use is a call argument and is dead
/// after the call is dropped AFTER the call (last-use), never before.
#[test]
fn borrow_into_call_dropped_after() {
    let mut func = TirFunction::new("bc".into(), vec![], TirType::DynBox);
    let x = func.fresh_value();
    let res = func.fresh_value();
    let out = func.fresh_value();
    for v in [x, res, out] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(x));
        b.ops.push(op(OpCode::Call, vec![x], vec![res]));
        b.ops.push(op(OpCode::Call, vec![res], vec![out]));
        b.terminator = Terminator::Return { values: vec![out] };
    }
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // x's last use is op 1 (the call). DecRef(x) must come AFTER op 1, before
    // the next op. Find the index of DecRef(x) and assert it follows the call.
    let ops = &func.blocks[&entry].ops;
    let call_x_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::Call && o.operands == vec![x])
        .unwrap();
    let decref_x_idx = ops
        .iter()
        .position(|o| o.opcode == OpCode::DecRef && o.operands == vec![x]);
    assert!(decref_x_idx.is_some(), "x dropped at last use");
    assert!(decref_x_idx.unwrap() > call_x_idx, "drop AFTER the call");
}

/// Generator yield: a value live across the yield gets an IncRef before it.
#[test]
fn yield_increfs_live_across() {
    let mut func = TirFunction::new("g".into(), vec![], TirType::DynBox);
    let header = func.entry_block;
    let resume = func.fresh_block();
    let x = func.fresh_value();
    let yval = func.fresh_value();
    let used = func.fresh_value();
    for v in [x, yval, used] {
        func.value_types.insert(v, TirType::Str);
    }
    {
        let b = func.blocks.get_mut(&header).unwrap();
        b.ops.push(const_str(x));
        b.ops.push(const_str(yval));
        // Yield: x is live across (used in resume), yval is the yielded value.
        b.ops.push(op(OpCode::Yield, vec![yval], vec![]));
        b.terminator = Terminator::Branch {
            target: resume,
            args: vec![],
        };
    }
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![x], vec![used])],
            terminator: Terminator::Return { values: vec![used] },
        },
    );
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // x must be IncRef'd before the Yield (it survives into the frame).
    let header_ops = &func.blocks[&header].ops;
    let yield_idx = header_ops
        .iter()
        .position(|o| o.opcode == OpCode::Yield)
        .unwrap();
    let incref_x_before = header_ops[..yield_idx]
        .iter()
        .any(|o| o.opcode == OpCode::IncRef && o.operands == vec![x]);
    assert!(incref_x_before, "live-across-yield value must be IncRef'd");
    assert!(count_increfs(&func) >= 1);
}

/// Loop accumulator: a heap accumulator threaded through a header block arg
/// and updated on the back-edge gets a drop for the dead previous value, and
/// the loop-exit value is dropped (dead after the loop).
#[test]
fn loop_accumulator_dropped() {
    let mut func = TirFunction::new("loop".into(), vec![], TirType::DynBox);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let acc0 = func.fresh_value();
    let acc_phi = func.fresh_value();
    let cond = func.fresh_value();
    let acc_next = func.fresh_value();
    for v in [acc0, acc_phi, acc_next] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(acc0));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![acc0],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: acc_phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            // acc_next = Call(acc_phi): consumes the phi, produces a new owned acc.
            ops: vec![op(OpCode::Call, vec![acc_phi], vec![acc_next])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![acc_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            // The final acc_phi is dead (not returned).
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // The loop-exit value (acc_phi, live-out of header into exit but dead in
    // exit) must be dropped at the exit block entry (edge-dying rule).
    let exit_decrefs: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::DecRef)
        .map(|o| o.operands[0])
        .collect();
    assert!(
        exit_decrefs.contains(&acc_phi),
        "loop-exit dead accumulator must be dropped at exit entry; got {exit_decrefs:?}"
    );
}

#[test]
fn explicit_del_boundary_root_not_edge_dropped_at_loop_exit() {
    let mut func = TirFunction::new("explicit_boundary_loop".into(), vec![], TirType::None);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let stale_release_root = func.fresh_value();
    let current_seed = func.fresh_value();
    let current_phi = func.fresh_value();
    let cond = func.fresh_value();
    let next_value = func.fresh_value();
    let next_slot = func.fresh_value();
    for v in [
        stale_release_root,
        current_seed,
        current_phi,
        next_value,
        next_slot,
    ] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(stale_release_root));
        b.ops.push(const_str(current_seed));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![current_seed],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: current_phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    let mut body_boundary = op(OpCode::DelBoundary, vec![stale_release_root], vec![]);
    body_boundary
        .attrs
        .insert("s_value".into(), AttrValue::Str("value".into()));
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                body_boundary,
                const_str(next_value),
                original_copy_with_operands("store_var", vec![next_value], vec![next_slot]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next_slot],
            },
        },
    );
    let mut exit_boundary = op(OpCode::DelBoundary, vec![current_phi], vec![]);
    exit_boundary
        .attrs
        .insert("s_value".into(), AttrValue::Str("value".into()));
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![exit_boundary],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let body_drops: Vec<ValueId> = func.blocks[&body]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        body_drops.contains(&stale_release_root),
        "the explicit body boundary must remain the release authority; drops={body_drops:?}"
    );
    let exit_entry_drops: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .take_while(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        !exit_entry_drops.contains(&stale_release_root),
        "a path-conditioned explicit release root must not also be edge-dropped at the loop exit; drops={exit_entry_drops:?}"
    );
}

#[test]
fn explicit_del_boundary_splits_shared_return_keep_path_release() {
    let mut func = TirFunction::new("explicit_boundary_diamond".into(), vec![], TirType::None);
    let del_path = func.fresh_block();
    let keep_path = func.fresh_block();
    let exit = func.fresh_block();
    let owner = func.fresh_value();
    let stored = func.fresh_value();
    let cond = func.fresh_value();
    for v in [owner, stored] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(owner));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![owner],
            vec![stored],
        ));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block: del_path,
            then_args: vec![],
            else_block: keep_path,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        del_path,
        TirBlock {
            id: del_path,
            args: vec![],
            ops: vec![op(OpCode::DelBoundary, vec![stored], vec![])],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        keep_path,
        TirBlock {
            id: keep_path,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let del_drops: Vec<ValueId> = func.blocks[&del_path]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        del_drops,
        vec![owner],
        "the explicit del path must keep exactly its boundary release"
    );
    let exit_drops: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        !exit_drops.contains(&owner),
        "the shared return block cannot drop a root already released on another incoming path"
    );
    let keep_split_releases = func.blocks.iter().any(|(&bid, block)| {
        bid != entry
            && bid != del_path
            && bid != keep_path
            && bid != exit
            && block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![owner])
            && matches!(
                &block.terminator,
                Terminator::Branch { target, args }
                    if *target == exit && args.is_empty()
            )
    });
    assert!(
        keep_split_releases,
        "the keep path must get an edge-local release for the owner skipped by the del path"
    );
}

#[test]
fn explicit_del_boundary_join_before_return_splits_keep_edge() {
    let mut func = TirFunction::new(
        "explicit_boundary_join_before_return".into(),
        vec![],
        TirType::None,
    );
    let del_path = func.fresh_block();
    let keep_path = func.fresh_block();
    let join = func.fresh_block();
    let exit = func.fresh_block();
    let owner = func.fresh_value();
    let stored = func.fresh_value();
    let cond = func.fresh_value();
    for v in [owner, stored] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);

    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(owner));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![owner],
            vec![stored],
        ));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block: del_path,
            then_args: vec![],
            else_block: keep_path,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        del_path,
        TirBlock {
            id: del_path,
            args: vec![],
            ops: vec![op(OpCode::DelBoundary, vec![stored], vec![])],
            terminator: Terminator::Branch {
                target: join,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        keep_path,
        TirBlock {
            id: keep_path,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let del_drops: Vec<ValueId> = func.blocks[&del_path]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        del_drops,
        vec![owner],
        "the explicit del path must keep exactly its boundary release"
    );
    for (block_id, label) in [(join, "join"), (exit, "return")] {
        let drops: Vec<ValueId> = func.blocks[&block_id]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .map(|op| op.operands[0])
            .collect();
        assert!(
            !drops.contains(&owner),
            "the {label} block cannot drop a root already released on one incoming history: {drops:?}"
        );
    }
    let keep_split_releases = func.blocks.iter().any(|(&bid, block)| {
        bid != entry
            && bid != del_path
            && bid != keep_path
            && bid != join
            && bid != exit
            && block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![owner])
            && matches!(
                &block.terminator,
                Terminator::Branch { target, args }
                    if *target == join && args.is_empty()
            )
    });
    assert!(
        keep_split_releases,
        "the keep path must release the owner before histories merge at the join"
    );
}

/// Mixed-ownership phi, INCOMING side (§5 retain). A loop accumulator phi is
/// seeded on the loop-ENTRY edge with a transparent alias of a BORROWED
/// parameter (`x = base`), and updated on the back-edge with a fresh owned
/// value. Because the loop body drops the phi each iteration, the borrowed
/// entry value must be RETAINED on the entry edge (before the preheader's
/// terminator) so the phi uniformly owns a `+1`. The back-edge's fresh owned
/// value must NOT be retained (that would leak the accumulator each iteration).
#[test]
fn mixed_phi_borrowed_param_retained_on_entry_edge() {
    // param `base` (id 0), preheader binds the accumulator phi to Copy(base).
    let mut func = TirFunction::new("apply".into(), vec![TirType::Str], TirType::DynBox);
    let base = ValueId(0);
    let pre = func.fresh_block(); // preheader
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let x0 = func.fresh_value(); // Copy(base) — borrowed alias seeding the phi
    let acc_phi = func.fresh_value();
    let load_x = func.fresh_value(); // Copy(acc_phi) in body
    let cond = func.fresh_value();
    let acc_next = func.fresh_value(); // fresh owned (Call result)
    for v in [x0, acc_phi, load_x, acc_next] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.terminator = Terminator::Branch {
            target: pre,
            args: vec![],
        };
    }
    // preheader: x0 = copy_var(base) → transparent alias of the param.
    func.blocks.insert(
        pre,
        TirBlock {
            id: pre,
            args: vec![],
            ops: vec![{
                let mut o = op(OpCode::Copy, vec![base], vec![x0]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            }],
            terminator: Terminator::Branch {
                target: header,
                args: vec![x0],
            },
        },
    );
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: acc_phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                {
                    let mut o = op(OpCode::Copy, vec![acc_phi], vec![load_x]);
                    o.attrs
                        .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                    o
                },
                // acc_next = Call(load_x, base): fresh owned, reads base each iter.
                op(OpCode::Call, vec![load_x, base], vec![acc_next]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![acc_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // The preheader must IncRef the borrowed `x0` (alias of the param) before
    // its terminator — the entry-edge retain.
    let pre_increfs: Vec<ValueId> = func.blocks[&pre]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::IncRef)
        .flat_map(|o| o.operands.clone())
        .collect();
    assert!(
        pre_increfs.contains(&x0),
        "borrowed param alias seeding the loop phi must be retained on the entry edge; got {pre_increfs:?}"
    );
    // The back-edge (body) must NOT retain the fresh owned `acc_next` — that
    // would leak one accumulator per iteration.
    let body_increfs: Vec<ValueId> = func.blocks[&body]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::IncRef)
        .flat_map(|o| o.operands.clone())
        .collect();
    assert!(
        !body_increfs.contains(&acc_next),
        "fresh owned back-edge value must NOT be retained (would leak); got {body_increfs:?}"
    );
    // The param itself is never dropped (borrowed ABI).
    let any_decref_base = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .any(|o| o.opcode == OpCode::DecRef && o.operands == vec![base]);
    assert!(!any_decref_base, "parameter must never be directly dropped");
}

/// Mixed-ownership phi, OUTGOING side (§3 incoming-arg exclusion). An owned
/// value is FORWARDED as a branch arg into a join block's phi through a
/// multi-block chain (the shape the inliner produces). The value's ownership
/// transfers INTO the phi, so it must NOT be edge-dropped at the join entry —
/// the phi is released by its own last-use drop. A spurious join-entry drop
/// plus the phi's drop is a double-free.
#[test]
fn forwarded_owned_value_not_edge_dropped_at_join() {
    let mut func = TirFunction::new("fwd".into(), vec![], TirType::DynBox);
    let mid = func.fresh_block();
    let join = func.fresh_block();
    let owned = func.fresh_value(); // fresh owned (ConstStr)
    let fwd = func.fresh_value(); // Copy(owned) — alias forwarded to the phi
    let phi = func.fresh_value();
    let used = func.fresh_value();
    for v in [owned, fwd, phi, used] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.terminator = Terminator::Branch {
            target: mid,
            args: vec![],
        };
    }
    func.blocks.insert(
        mid,
        TirBlock {
            id: mid,
            args: vec![],
            ops: vec![const_str(owned), {
                let mut o = op(OpCode::Copy, vec![owned], vec![fwd]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            }],
            // Forward `fwd` (owned, via alias) into the join's phi.
            terminator: Terminator::Branch {
                target: join,
                args: vec![fwd],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![used] },
        },
    );
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // The forwarded owned value (`fwd`, alias root `owned`) must NOT be dropped
    // at the join entry — it transferred into the phi. The phi's own last-use
    // drop (after the Call) releases the object exactly once.
    let join_entry_decrefs: Vec<ValueId> = func.blocks[&join]
        .ops
        .iter()
        .take_while(|o| o.opcode == OpCode::DecRef)
        .flat_map(|o| o.operands.clone())
        .collect();
    assert!(
        !join_entry_decrefs.contains(&fwd) && !join_entry_decrefs.contains(&owned),
        "forwarded owned value must not be edge-dropped at the join; got {join_entry_decrefs:?}"
    );
    // Exactly one DecRef releases the group (the phi at its last use in join).
    let total_group_decrefs = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| {
            o.opcode == OpCode::DecRef
                && o.operands
                    .first()
                    .is_some_and(|&v| v == fwd || v == owned || v == phi)
        })
        .count();
    assert_eq!(
        total_group_decrefs, 1,
        "the owned forwarded group must be released exactly once, not double-freed"
    );
}

#[test]
fn phi_edge_clean_transfer_ignores_release_on_other_branch() {
    let mut func = TirFunction::new("branch_or_phi".into(), vec![], TirType::None);
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let join = func.fresh_block();
    let source = func.fresh_value(); // fresh owned result used by both arms
    let then_alias = func.fresh_value(); // transparent alias forwarded to the phi
    let fallback = func.fresh_value();
    let selected = func.fresh_value(); // `source or fallback` result on else arm
    let phi = func.fresh_value();
    for v in [source, then_alias, fallback, selected, phi] {
        func.value_types.insert(v, TirType::Str);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::Call, vec![], vec![source]));
        b.terminator = Terminator::CondBranch {
            cond: source,
            then_block,
            then_args: vec![],
            else_block,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        then_block,
        TirBlock {
            id: then_block,
            args: vec![],
            ops: vec![{
                let mut o = op(OpCode::Copy, vec![source], vec![then_alias]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
                o
            }],
            terminator: Terminator::Branch {
                target: join,
                args: vec![then_alias],
            },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![],
            ops: vec![
                const_str(fallback),
                op(OpCode::Or, vec![source, fallback], vec![selected]),
            ],
            terminator: Terminator::Branch {
                target: join,
                args: vec![selected],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::Call, vec![phi], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let then_increfs: Vec<ValueId> = func.blocks[&then_block]
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::IncRef)
        .flat_map(|o| o.operands.clone())
        .collect();
    assert!(
        then_increfs.is_empty(),
        "release planning is path-sensitive: an else-arm release must not \
             force a retain on the clean-transfer then edge, got {then_increfs:?}"
    );
}

/// Mixed-ownership phi, CRITICAL-EDGE SPLIT (§5 ambiguous-arc retain; round-4
/// Finding 2). When a predecessor reaches an OWNED phi via MORE THAN ONE arc
/// with DIFFERENT args, a before-terminator IncRef would wrongly fire on the
/// other arc, so the pass SPLITS the critical edge: it inserts a fresh block
/// holding the edge-exact `IncRef` + an unconditional `Branch` to the target,
/// and retargets exactly that arc to the new block. This is the only path that
/// allocates a block (it is why the pass is `Mutates::Cfg`), and it shipped
/// with ZERO coverage before this test.
///
/// Shape: `entry` ends in a `Switch` whose case-0 and DEFAULT arcs BOTH target
/// `join` but forward DIFFERENT args into `join`'s single owned phi — case-0
/// forwards a transparent alias of the borrowed param `base` (BORROWED → must
/// retain), default forwards a freshly minted owned `ConstStr` (clean transfer
/// → no retain). `join` consumes the phi (a `Call`) and returns nothing, so the
/// phi is dropped and the borrowed case-0 edge needs its `+1`. Because case-0
/// and default both go to `join`, the retain cannot be placed before `entry`'s
/// terminator (it would also fire on the default arc); it must be split onto
/// the case-0 arc.
#[test]
fn mixed_phi_critical_edge_split_inserts_fresh_incref_block() {
    // param `base` (id 0): borrowed heap Str.
    let mut func = TirFunction::new("split".into(), vec![TirType::Str], TirType::DynBox);
    let base = ValueId(0);
    let join = func.fresh_block();
    let case0_alias = func.fresh_value(); // Copy(base) — borrowed alias (case 0 arg)
    let sel = func.fresh_value(); // Switch selector (raw)
    let fresh_owned = func.fresh_value(); // ConstStr — fresh owned (default arg)
    let phi = func.fresh_value(); // join's owned obj-lane phi
    let used = func.fresh_value(); // Call(phi) result
    for v in [case0_alias, fresh_owned, phi, used] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(sel, TirType::I64);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        // case0_alias = copy_var(base): a transparent (borrowed) alias of the
        // param; fresh_owned = ConstStr: a freshly minted owned value; sel: the
        // raw Switch selector.
        b.ops.push({
            let mut o = op(OpCode::Copy, vec![base], vec![case0_alias]);
            o.attrs
                .insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
            o
        });
        b.ops.push(const_str(fresh_owned));
        b.ops.push(op(OpCode::ConstInt, vec![], vec![sel]));
        // Switch: case 0 → join(case0_alias); default → join(fresh_owned).
        // TWO arcs to `join` with DIFFERENT args ⇒ a critical edge.
        b.terminator = Terminator::Switch {
            value: sel,
            cases: vec![(0, join, vec![case0_alias])],
            default: join,
            default_args: vec![fresh_owned],
        };
    }
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: phi,
                ty: TirType::Str,
            }],
            // Consume the phi (drops it at its last use) and return nothing so the
            // phi dies in `join` — the case-0 borrowed edge therefore needs a +1.
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let n_blocks_before = func.blocks.len();
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    // A fresh block must have been inserted by the critical-edge split.
    assert!(
        func.blocks.len() > n_blocks_before,
        "the critical-edge split must allocate a fresh block; before={n_blocks_before} after={}",
        func.blocks.len()
    );

    // `entry`'s case-0 arc must now target a NEW block (not `join`): the retarget.
    // The default arc must still go to `join` (unsplit, clean transfer).
    let (case0_target, default_target) = match &func.blocks[&entry].terminator {
        Terminator::Switch { cases, default, .. } => (cases[0].1, *default),
        other => panic!("entry terminator must remain a Switch, got {other:?}"),
    };
    assert_ne!(
        case0_target, join,
        "the borrowed case-0 arc must be retargeted away from `join` to the split block"
    );
    assert_eq!(
        default_target, join,
        "the clean-transfer default arc must stay pointed at `join` (not split)"
    );

    // The split block must (a) hold an IncRef of the borrowed alias `case0_alias`
    // and (b) branch unconditionally to `join` forwarding that same arg.
    let split = &func.blocks[&case0_target];
    let split_increfs: Vec<ValueId> = split
        .ops
        .iter()
        .filter(|o| o.opcode == OpCode::IncRef)
        .flat_map(|o| o.operands.clone())
        .collect();
    assert!(
        split_increfs.contains(&case0_alias),
        "the split block must retain (IncRef) the borrowed case-0 value; got {split_increfs:?}"
    );
    match &split.terminator {
        Terminator::Branch { target, args } => {
            assert_eq!(
                *target, join,
                "split block must branch to the original target"
            );
            assert_eq!(
                args,
                &vec![case0_alias],
                "split block must forward the case-0 arg it took over"
            );
        }
        other => panic!("split block must end in an unconditional Branch, got {other:?}"),
    }

    // The default (clean-transfer, freshly owned) value must NOT be retained
    // anywhere — retaining it would leak.
    let any_incref_fresh = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .any(|o| o.opcode == OpCode::IncRef && o.operands.first() == Some(&fresh_owned));
    assert!(
        !any_incref_fresh,
        "the clean-transfer default value must not be retained (would leak)"
    );

    // The split result must be a valid CFG: re-run the analysis self-check over
    // the post-split function (mirrors MOLT_VERIFY_ANALYSIS=1) — a malformed
    // split (dangling edge / unreachable target) would diverge the recomputed
    // dominators from a fresh build.
    let mut verify_am = AnalysisManager::new();
    let preds = crate::tir::dominators::build_pred_map_with(
        &func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let reachable = crate::tir::dominators::reachable_blocks_with(
        &func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    assert!(
        reachable.contains(&case0_target),
        "the split block must be reachable from entry"
    );
    assert!(
        preds.get(&join).is_some_and(|p| p.contains(&case0_target)),
        "the split block must be a predecessor of the original target"
    );
    // Liveness recomputes cleanly over the mutated CFG (would panic on a
    // use-before-def introduced by a bad split).
    let _ = verify_am.get::<TirLiveness>(&func).clone();
}

/// FINDING 3 (round-4) fail-closed pin. `incoming_arg_roots` keys on alias
/// ROOT over ALL predecessors, so a root forwarded into a join's phi by ANY
/// predecessor is excluded from that join's edge-dying drop on EVERY path.
/// This test pins the load-bearing invariant the imprecision must preserve:
/// the exclusion can only ever LEAK, NEVER double-free (over-release → UAF).
///
/// Shape (a diamond where the SAME owned root reaches a join on BOTH edges):
/// `entry` mints one owned value `r`, then branches to `p1` / `p2`. `p1`
/// forwards `r` straight into the join's phi (a transfer). `p2` forwards `r`
/// into the join's phi too (through a transparent alias `r_alias`, the
/// load-var shape) — so `r`'s root is forwarded by MORE THAN ONE predecessor
/// and is a member of `incoming_arg_roots`. The join consumes the phi (a
/// `Call`) and returns nothing. There is exactly ONE underlying owned object;
/// the assertion is that the pass emits AT MOST ONE `DecRef` naming any member
/// of `r`'s group across the whole function — never two (the double-free the
/// global keying must not introduce). A leak (zero drops) would be acceptable
/// per the fail-closed contract; a double-free would be the UAF bug.
#[test]
fn forwarded_into_phi_other_pred_live_is_leak_not_uaf() {
    let mut func = TirFunction::new("diamond".into(), vec![], TirType::DynBox);
    let p1 = func.fresh_block();
    let p2 = func.fresh_block();
    let join = func.fresh_block();
    let r = func.fresh_value(); // fresh owned (ConstStr) defined in entry
    let cond = func.fresh_value();
    let r_alias = func.fresh_value(); // Copy(r) in p2 — transparent alias of r
    let phi = func.fresh_value(); // join's owned obj-lane phi
    let used = func.fresh_value(); // Call(phi) result
    for v in [r, r_alias, phi, used] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(r));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block: p1,
            then_args: vec![],
            else_block: p2,
            else_args: vec![],
        };
    }
    // p1: forward `r` straight into the join phi.
    func.blocks.insert(
        p1,
        TirBlock {
            id: p1,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join,
                args: vec![r],
            },
        },
    );
    // p2: r_alias = load_var(r) [transparent alias]; forward the alias into the
    // SAME phi position → `r`'s root is forwarded by a 2nd predecessor.
    func.blocks.insert(
        p2,
        TirBlock {
            id: p2,
            args: vec![],
            ops: vec![{
                let mut o = op(OpCode::Copy, vec![r], vec![r_alias]);
                o.attrs
                    .insert("_original_kind".into(), AttrValue::Str("load_var".into()));
                o
            }],
            terminator: Terminator::Branch {
                target: join,
                args: vec![r_alias],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::Call, vec![phi], vec![used])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    // The single owned object (`r`'s alias group: r, r_alias, phi) must be
    // released AT MOST once — never twice. (Fail-closed: a leak is allowed; a
    // double-free is the UAF the global keying must never introduce.)
    let group_decrefs = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| {
            o.opcode == OpCode::DecRef
                && o.operands
                    .first()
                    .is_some_and(|&v| v == r || v == r_alias || v == phi)
        })
        .count();
    assert!(
        group_decrefs <= 1,
        "incoming_arg_roots over-all-preds keying must never double-free a \
             forwarded root (fail-closed: leak ok, UAF never); got {group_decrefs} \
             DecRefs of the owned group"
    );
}
