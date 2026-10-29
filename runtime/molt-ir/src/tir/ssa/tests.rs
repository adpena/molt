use super::*;
use crate::ir::OpIR;
use crate::tir::cfg::CFG;
use std::collections::HashSet;

/// Helper to create an `OpIR` with just a `kind`.
pub(super) fn op(kind: crate::OpKind) -> OpIR {
    OpIR {
        kind: kind,
        ..crate::OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind`, `args`, and `out`.
pub(super) fn op_args_out(kind: crate::OpKind, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind,
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..crate::OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind` and `args`.
pub(super) fn op_args(kind: crate::OpKind, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind,
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        ..crate::OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind` and `value`.
pub(super) fn op_val(kind: crate::OpKind, value: i64) -> OpIR {
    OpIR {
        kind: kind,
        value: Some(value),
        ..crate::OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind`, `out`, and `value`.
pub(super) fn op_val_out(kind: crate::OpKind, value: i64, out: &str) -> OpIR {
    OpIR {
        kind: kind,
        value: Some(value),
        out: Some(out.to_string()),
        ..crate::OpIR::new(kind)
    }
}

#[test]
fn compact_ssa_liveness_retains_unresolved_reads_and_parameter_entry_definitions() {
    let ops = vec![
        op_val(crate::OpKind::Jump, 7),
        op_val(crate::OpKind::Label, 7),
        op_args(crate::OpKind::Print, &["parameter", "unresolved"]),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let params = vec!["parameter".to_owned()];
    let mut context = SsaContext::new("liveness_census", &cfg, &ops, &params);
    context.gather_defs_uses();
    context.build_augmented_cfg();
    let (names, live_in) = context.compute_live_in_vars();
    let parameter = names.id("parameter").unwrap();
    let unresolved = names.id("unresolved").unwrap();
    assert!(
        !context.all_vars.contains("unresolved"),
        "a read must not invent a definition"
    );
    assert!(!live_in[cfg.entry].contains(parameter));
    assert!(live_in[cfg.entry].contains(unresolved));
    let target = cfg.successors[cfg.entry][0];
    assert!(live_in[target].contains(parameter));
    assert!(live_in[target].contains(unresolved));
}

// Helper: count block arguments across all blocks.
fn total_block_args(output: &SsaOutput) -> usize {
    output.blocks.iter().map(|b| b.args.len()).sum()
}

mod iter_next_fusion {
    use super::*;

    fn lower(ops: &[OpIR], params: &[&str]) -> SsaOutput {
        let cfg = CFG::build(ops);
        let params: Vec<String> = params.iter().map(|name| (*name).to_string()).collect();
        convert_to_ssa_with_params(&cfg, ops, &params)
    }

    fn opcode_count(output: &SsaOutput, opcode: OpCode) -> usize {
        output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == opcode)
            .count()
    }

    fn assert_fused(output: &SsaOutput) {
        assert_eq!(opcode_count(output, OpCode::IterNextUnboxed), 1);
        assert_eq!(opcode_count(output, OpCode::IterNext), 0);
        assert_eq!(opcode_count(output, OpCode::Index), 0);
    }

    fn assert_materialized(output: &SsaOutput) {
        assert_eq!(opcode_count(output, OpCode::IterNextUnboxed), 0);
        assert_eq!(opcode_count(output, OpCode::IterNext), 1);
        assert_eq!(opcode_count(output, OpCode::Index), 2);
    }

    fn direct_guarded_blocks() -> Vec<TirBlock> {
        let mut idx1_attrs = AttrDict::new();
        idx1_attrs.insert("value".into(), AttrValue::Int(1));
        let mut idx0_attrs = AttrDict::new();
        idx0_attrs.insert("value".into(), AttrValue::Int(0));
        let mut pair_attrs = AttrDict::new();
        pair_attrs.insert("_simple_out".into(), AttrValue::Str("pair".into()));
        let mut done_attrs = AttrDict::new();
        done_attrs.insert("_simple_out".into(), AttrValue::Str("done".into()));
        let mut value_attrs = AttrDict::new();
        value_attrs.insert("_simple_out".into(), AttrValue::Str("value".into()));

        vec![
            TirBlock {
                id: BlockId(0),
                args: vec![],
                ops: vec![
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![ValueId(1)],
                        attrs: idx1_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![ValueId(2)],
                        attrs: idx0_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::IterNext,
                        operands: vec![ValueId(0)],
                        results: vec![ValueId(3)],
                        attrs: pair_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::Index,
                        operands: vec![ValueId(3), ValueId(1)],
                        results: vec![ValueId(4)],
                        attrs: done_attrs,
                        source_span: None,
                    },
                ],
                terminator: Terminator::CondBranch {
                    cond: ValueId(4),
                    then_block: BlockId(2),
                    then_args: vec![],
                    else_block: BlockId(1),
                    else_args: vec![],
                },
            },
            TirBlock {
                id: BlockId(1),
                args: vec![],
                ops: vec![TirOp {
                    origin: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::Index,
                    operands: vec![ValueId(3), ValueId(2)],
                    results: vec![ValueId(5)],
                    attrs: value_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![] },
            },
            TirBlock {
                id: BlockId(2),
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        ]
    }

    fn guarded_projection_ops() -> Vec<OpIR> {
        vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op(crate::OpKind::RetVoid),
        ]
    }

    #[test]
    fn exact_ordered_pair_projections_fuse() {
        let ops = guarded_projection_ops();
        let output = lower(&ops, &["iter"]);
        assert_fused(&output);
        let fused = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == OpCode::IterNextUnboxed)
            .expect("iter_next projections must fuse");
        assert_eq!(
            fused.attrs.get("_simple_result_0"),
            Some(&AttrValue::Str("value".into()))
        );
        assert_eq!(
            fused.attrs.get("_simple_result_1"),
            Some(&AttrValue::Str("done".into()))
        );
    }

    #[test]
    fn intervening_effect_is_preserved() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_args_out(crate::OpKind::ModuleCacheSet, &["done", "iter"], "none"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op(crate::OpKind::RetVoid),
        ];
        let output = lower(&ops, &["iter"]);

        assert_fused(&output);
        assert_eq!(opcode_count(&output, OpCode::ModuleCacheSet), 1);
    }

    #[test]
    fn pre_guard_effect_rejects_exhaustion_finalizer_reordering() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::ModuleCacheSet, &["prior", "iter"], "none"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter", "prior"]));
    }

    #[test]
    fn bare_projections_without_done_guard_reject_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn done_true_item_projection_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn done_paths_rejoining_before_item_reject_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 10),
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 20),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn same_target_done_branch_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_val(crate::OpKind::Label, 10),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn pre_guard_cycle_rejects_stale_step_validity() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_val(crate::OpKind::Jump, 10),
            op_val(crate::OpKind::Label, 10),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["spin".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_val(crate::OpKind::Label, 20),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(30),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 30),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter", "spin"]));
    }

    #[test]
    fn augmented_exception_and_resume_edges_reject_fusion() {
        let cfg_ops = vec![
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["cond".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op(crate::OpKind::RetVoid),
        ];

        for resume_edge in [false, true] {
            let mut cfg = CFG::build(&cfg_ops);
            if resume_edge {
                cfg.state_resume_edges.push((0, 1, 1));
            } else {
                cfg.exception_edges.push((0, 1));
            }
            let mut context = SsaContext::new("augmented_bypass", &cfg, &cfg_ops, &[]);
            context.build_augmented_cfg();
            let mut blocks = direct_guarded_blocks();

            context.fuse_iter_next_projections(&mut blocks);

            assert_eq!(
                blocks
                    .iter()
                    .flat_map(|block| block.ops.iter())
                    .filter(|op| op.opcode == OpCode::IterNext)
                    .count(),
                1
            );
            assert_eq!(
                blocks
                    .iter()
                    .flat_map(|block| block.ops.iter())
                    .filter(|op| op.opcode == OpCode::IterNextUnboxed)
                    .count(),
                0
            );
        }
    }

    #[test]
    fn extra_pair_consumer_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_args_out(crate::OpKind::Is, &["pair", "iter"], "extra"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn shadowed_selector_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Copy, &["selector_override"], "idx1"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter", "selector_override"]));
    }

    #[test]
    fn branch_reassignment_ambiguity_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["cond".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 10),
            op_args_out(crate::OpKind::Copy, &["other_pair"], "pair"),
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 20),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter", "cond", "other_pair"]));
    }

    #[test]
    fn unused_mixed_forwarding_rejects_pair_elimination() {
        let cfg_ops = vec![
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["cond".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 10),
            op_val(crate::OpKind::Jump, 20),
            op_val(crate::OpKind::Label, 20),
            op(crate::OpKind::RetVoid),
        ];
        let cfg = CFG::build(&cfg_ops);
        assert_eq!(cfg.blocks.len(), 4, "synthetic diamond CFG shape changed");
        let mut context = SsaContext::new("mixed_forwarding", &cfg, &cfg_ops, &[]);
        context.build_augmented_cfg();

        let mut idx1_attrs = AttrDict::new();
        idx1_attrs.insert("value".into(), AttrValue::Int(1));
        let mut idx0_attrs = AttrDict::new();
        idx0_attrs.insert("value".into(), AttrValue::Int(0));
        let mut iter_attrs = AttrDict::new();
        iter_attrs.insert("_simple_out".into(), AttrValue::Str("pair".into()));
        let mut done_attrs = AttrDict::new();
        done_attrs.insert("_simple_out".into(), AttrValue::Str("done".into()));
        let mut value_attrs = AttrDict::new();
        value_attrs.insert("_simple_out".into(), AttrValue::Str("value".into()));

        let mut blocks = vec![
            TirBlock {
                id: BlockId(0),
                args: vec![],
                ops: vec![
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![ValueId(1)],
                        attrs: idx1_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![ValueId(2)],
                        attrs: idx0_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::IterNext,
                        operands: vec![ValueId(0)],
                        results: vec![ValueId(3)],
                        attrs: iter_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::Index,
                        operands: vec![ValueId(3), ValueId(1)],
                        results: vec![ValueId(4)],
                        attrs: done_attrs,
                        source_span: None,
                    },
                    TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::Index,
                        operands: vec![ValueId(3), ValueId(2)],
                        results: vec![ValueId(5)],
                        attrs: value_attrs,
                        source_span: None,
                    },
                ],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: BlockId(1),
                    then_args: vec![],
                    else_block: BlockId(2),
                    else_args: vec![],
                },
            },
            TirBlock {
                id: BlockId(1),
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: BlockId(3),
                    args: vec![ValueId(3)],
                },
            },
            TirBlock {
                id: BlockId(2),
                args: vec![],
                ops: vec![TirOp {
                    origin: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstNone,
                    operands: vec![],
                    results: vec![ValueId(6)],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: BlockId(3),
                    args: vec![ValueId(6)],
                },
            },
            TirBlock {
                id: BlockId(3),
                args: vec![TirValue {
                    id: ValueId(7),
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        ];

        context.fuse_iter_next_projections(&mut blocks);

        assert_eq!(
            blocks[0]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::IterNext)
                .count(),
            1
        );
        assert_eq!(
            blocks[0]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::IterNextUnboxed)
                .count(),
            0
        );
        assert!(matches!(
            &blocks[1].terminator,
            Terminator::Branch { args, .. } if args == &[ValueId(3)]
        ));
    }

    #[test]
    fn projection_distance_has_no_correctness_window() {
        let mut ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
        ];
        for value in 0..30 {
            ops.push(op_val_out(
                crate::OpKind::Const,
                value,
                &format!("gap_{value}"),
            ));
        }
        ops.extend([
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            OpIR {
                kind: crate::OpKind::BrIf,
                args: Some(vec!["done".to_string()]),
                value: Some(10),
                ..crate::OpIR::new(crate::OpKind::BrIf)
            },
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 10),
            op(crate::OpKind::RetVoid),
        ]);
        let output = lower(&ops, &["iter"]);

        assert_fused(&output);
        assert_eq!(opcode_count(&output, OpCode::ConstInt), 32);
    }

    #[test]
    fn value_projection_before_done_rejects_fusion() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op(crate::OpKind::RetVoid),
        ];

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn exception_aware_loop_keeps_fast_path_and_pending_check() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op(crate::OpKind::LoopStart),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op(crate::OpKind::LoopBreakIfException),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_args(crate::OpKind::LoopBreakIfTrue, &["done"]),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op(crate::OpKind::LoopContinue),
            op(crate::OpKind::LoopEnd),
            op(crate::OpKind::RetVoid),
        ];
        let output = lower(&ops, &["iter"]);

        assert_fused(&output);
        assert_eq!(opcode_count(&output, OpCode::ExceptionPending), 1);
    }

    #[test]
    fn frontend_inline_exception_checks_preserve_unboxed_loop_steps() {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 1, "idx1"),
            op_val_out(crate::OpKind::Const, 0, "idx0"),
            op(crate::OpKind::LoopStart),
            op_args_out(crate::OpKind::IterNext, &["iter"], "pair"),
            op_val(crate::OpKind::CheckException, 90),
            op_args_out(crate::OpKind::Index, &["pair", "idx1"], "done"),
            op_val(crate::OpKind::CheckException, 90),
            op_args(crate::OpKind::LoopBreakIfTrue, &["done"]),
            op_args_out(crate::OpKind::Index, &["pair", "idx0"], "value"),
            op_val(crate::OpKind::CheckException, 90),
            op_args_out(crate::OpKind::Call, &["consume", "value"], "used"),
            op(crate::OpKind::LoopContinue),
            op(crate::OpKind::LoopEnd),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 90),
            op(crate::OpKind::RetVoid),
        ];
        let output = lower(&ops, &["iter", "consume"]);

        assert_fused(&output);
        assert_eq!(opcode_count(&output, OpCode::CheckException), 3);
        assert_eq!(opcode_count(&output, OpCode::Call), 1);
    }

    #[test]
    fn inline_exception_handler_that_observes_the_pair_keeps_it_materialized() {
        let mut ops = guarded_projection_ops();
        ops.insert(3, op_val(crate::OpKind::CheckException, 90));
        ops.extend([
            op_val(crate::OpKind::Label, 90),
            op_args_out(crate::OpKind::Is, &["pair", "iter"], "observed"),
            op(crate::OpKind::RetVoid),
        ]);

        assert_materialized(&lower(&ops, &["iter"]));
    }

    #[test]
    fn pre_guard_eval_breaker_observation_keeps_finalizer_order() {
        let mut ops = guarded_projection_ops();
        // The wire alias lifts to a CheckException with the typed poll marker;
        // that marker is not a legal SimpleIR field on check_exception itself.
        ops.insert(3, op_val(crate::OpKind::AsyncWorkPoll, 90));
        ops.extend([op_val(crate::OpKind::Label, 90), op(crate::OpKind::RetVoid)]);

        assert_materialized(&lower(&ops, &["iter"]));
    }
}

#[test]
fn unpack_sequence_lowers_to_first_class_variable_result_op() {
    let ops = vec![
        op_args_out(crate::OpKind::TupleNew, &[], "seq"),
        OpIR {
            kind: crate::OpKind::UnpackSequence,
            args: Some(vec!["seq".into(), "left".into(), "right".into()]),
            value: Some(2),
            ..crate::OpIR::new(crate::OpKind::UnpackSequence)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let unpack = output
        .blocks
        .iter()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::UnpackSequence)
        .expect("unpack_sequence must have first-class TIR authority");

    assert_eq!(unpack.operands.len(), 1);
    assert_eq!(unpack.results.len(), 2);
    assert_eq!(unpack.attrs.get("value"), Some(&AttrValue::Int(2)));
    assert!(!unpack.origin.is_some());
}

#[test]
fn empty_unpack_sequence_has_zero_first_class_results() {
    let ops = vec![
        op_args_out(crate::OpKind::TupleNew, &[], "seq"),
        OpIR {
            kind: crate::OpKind::UnpackSequence,
            args: Some(vec!["seq".into()]),
            value: Some(0),
            ..crate::OpIR::new(crate::OpKind::UnpackSequence)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let unpack = output
        .blocks
        .iter()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::UnpackSequence)
        .expect("empty unpack_sequence must retain first-class TIR authority");

    assert_eq!(unpack.operands.len(), 1);
    assert!(unpack.results.is_empty());
    assert_eq!(unpack.attrs.get("value"), Some(&AttrValue::Int(0)));
}

#[test]
fn module_get_attr_does_not_lower_as_fallback_copy() {
    let ops = vec![
        op_args_out(crate::OpKind::ModuleCacheGet, &["mod_name"], "module"),
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("Point".to_string()),
            out: Some("name".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_args_out(
            crate::OpKind::ModuleGetAttr,
            &["module", "name"],
            "class_ref",
        ),
        op_args(crate::OpKind::Ret, &["class_ref"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let module_lookup_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == OpCode::ModuleGetAttr)
        .count();
    let fallback_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(kind) if kind == crate::OpKind::ModuleGetAttr
                )
        })
        .count();

    assert_eq!(
        module_lookup_count, 1,
        "module_get_attr must lower to its first-class TIR opcode"
    );
    assert_eq!(
        fallback_count, 0,
        "module_get_attr must be a first-class TIR op, not Copy[_original_kind]"
    );
}

#[test]
fn active_line_marker_stamps_tir_source_site() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::Line,
            value: Some(17),
            source_line: Some(17),
            col_offset: Some(4),
            end_col_offset: Some(13),
            ..crate::OpIR::new(crate::OpKind::Line)
        },
        op_val_out(crate::OpKind::Const, 5, "x"),
        op_args(crate::OpKind::Ret, &["x"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let attributed_ops: Vec<_> = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter_map(|op| op.source_site().map(|site| (op.opcode, site)))
        .collect();

    assert!(
        attributed_ops.iter().any(|(opcode, site)| {
            *opcode == OpCode::ConstInt
                && site.line == 17
                && site.col == Some(4)
                && site.end_col == Some(13)
        }),
        "SSA lift must stamp executable ops from the active source line marker"
    );
}

#[test]
fn ssa_lift_stamps_source_op_index_on_ops_and_inline_consts() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 5, "x"),
        op_args_out(crate::OpKind::Add, &["x", "3"], "y"),
        op_args(crate::OpKind::Ret, &["y"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let const_five = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(5)))
        })
        .expect("literal const op should be emitted");
    assert_eq!(const_five.source_op_index(), Some(0));

    let inline_three = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(3)))
        })
        .expect("inline const arg should be emitted");
    assert_eq!(inline_three.source_op_index(), Some(1));

    let add = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::Add)
        .expect("add op should be emitted");
    assert_eq!(add.source_op_index(), Some(1));
}

#[test]
fn ssa_lift_preserves_transport_source_op_index() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 5, "x"),
        OpIR {
            kind: crate::OpKind::Add,
            args: Some(vec!["x".into(), "3".into()]),
            out: Some("y".into()),
            source_op_idx: Some(41),
            ..crate::OpIR::new(crate::OpKind::Add)
        },
        op_args(crate::OpKind::Ret, &["y"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let inline_three = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(3)))
        })
        .expect("inline const arg should be emitted");
    assert_eq!(inline_three.source_op_index(), Some(41));

    let add = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::Add)
        .expect("add op should be emitted");
    assert_eq!(add.source_op_index(), Some(41));
}

#[test]
fn store_var_survives_ssa_as_local_lifetime_marker() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ListNew,
            args: Some(vec![]),
            out: Some("list".to_string()),
            ..crate::OpIR::new(crate::OpKind::ListNew)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("bag".to_string()),
            args: Some(vec!["list".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let store_markers: Vec<_> = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(kind) if kind == crate::OpKind::StoreVar
                )
                && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "bag")
        })
        .collect();
    assert!(
        !store_markers.is_empty(),
        "store_var must survive SSA as a local lifetime marker for finalizer ordering"
    );
    let marker = store_markers[0];
    assert_eq!(marker.operands.len(), 1);
    assert_eq!(marker.results.len(), 1);
    assert!(
        matches!(marker.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "bag"),
        "store_var marker must preserve the Python local name"
    );
}

#[test]
fn copy_var_preserves_source_local_name_as_transport_attr() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ConstNone,
            out: Some("seed".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstNone)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("x".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::CopyVar,
            var: Some("x".to_string()),
            out: Some("read_x".to_string()),
            ..crate::OpIR::new(crate::OpKind::CopyVar)
        },
        op_args(crate::OpKind::Ret, &["read_x"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let copy = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::Copy
                && !op.origin.is_some()
                && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "x")
        })
        .expect("copy_var must carry its original SimpleIR source-local name");
    assert_eq!(
        copy.operands.len(),
        1,
        "copy_var local-name metadata must not replace the SSA value operand"
    );
    assert_eq!(
        copy.results.len(),
        1,
        "copy_var must keep the copied SSA result while carrying local-name metadata"
    );
}

#[test]
fn copy_var_with_args_treats_var_as_transport_not_second_operand() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ConstNone,
            out: Some("seed".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstNone)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("x".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::CopyVar,
            var: Some("x".to_string()),
            args: Some(vec!["seed".to_string()]),
            out: Some("alias".to_string()),
            ..crate::OpIR::new(crate::OpKind::CopyVar)
        },
        op_args(crate::OpKind::Ret, &["alias"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let copy = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| {
            op.opcode == OpCode::Copy
                && !op.origin.is_some()
                && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "x")
        })
        .expect("args-based copy_var must still carry its local-name metadata");

    assert_eq!(
        copy.operands.len(),
        1,
        "args-based copy_var must not also resolve var as a second value operand"
    );
}

#[test]
fn ord_at_lowers_to_first_class_tir_opcode() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("Aé".to_string()),
            out: Some("text".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_val_out(crate::OpKind::Const, 1, "idx"),
        op_args_out(crate::OpKind::OrdAt, &["text", "idx"], "code"),
        op_args(crate::OpKind::Ret, &["code"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let ord_at_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == OpCode::OrdAt)
        .count();
    let fallback_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(kind) if kind == crate::OpKind::OrdAt
                )
        })
        .count();

    assert_eq!(ord_at_count, 1, "ord_at must be a first-class TIR opcode");
    assert_eq!(
        fallback_count, 0,
        "ord_at must not lower as Copy[_original_kind]"
    );
}

fn assert_first_class_module_lookup(
    simple_kind: crate::OpKind,
    expected_opcode: OpCode,
    args: &[&str],
) {
    let mut ops = vec![
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("mod".to_string()),
            out: Some("module_name".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_args_out(crate::OpKind::ModuleCacheGet, &["module_name"], "module"),
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("answer".to_string()),
            out: Some("name".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_args_out(simple_kind, args, "resolved"),
        op_args(crate::OpKind::Ret, &["resolved"]),
    ];
    if simple_kind == crate::OpKind::ModuleCacheGet {
        ops = vec![
            OpIR {
                kind: crate::OpKind::ConstStr,
                s_value: Some("mod".to_string()),
                out: Some("module_name".to_string()),
                ..crate::OpIR::new(crate::OpKind::ConstStr)
            },
            op_args_out(crate::OpKind::ModuleCacheGet, &["module_name"], "resolved"),
            op_args(crate::OpKind::Ret, &["resolved"]),
        ];
    }

    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let first_class_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == expected_opcode)
        .count();
    let fallback_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(kind) if kind == simple_kind
                )
        })
        .count();

    assert_eq!(
        first_class_count, 1,
        "{simple_kind} must lower to its first-class TIR opcode"
    );
    assert_eq!(
        fallback_count, 0,
        "{simple_kind} must not lower as Copy[_original_kind]"
    );
}

#[test]
fn module_cache_get_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_lookup(
        crate::OpKind::ModuleCacheGet,
        OpCode::ModuleCacheGet,
        &["module_name"],
    );
}

#[test]
fn module_get_global_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_lookup(
        crate::OpKind::ModuleGetGlobal,
        OpCode::ModuleGetGlobal,
        &["module", "name"],
    );
}

#[test]
fn module_get_name_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_lookup(
        crate::OpKind::ModuleGetName,
        OpCode::ModuleGetName,
        &["module", "name"],
    );
}

fn assert_first_class_module_mutation(
    simple_kind: crate::OpKind,
    expected_opcode: OpCode,
    args: &[&str],
) {
    let ops = vec![
        op_args_out(crate::OpKind::ModuleImport, &["builtins"], "module"),
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("answer".to_string()),
            out: Some("name".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_val_out(crate::OpKind::Const, 42, "value"),
        OpIR {
            kind: simple_kind,
            args: Some(args.iter().map(|arg| arg.to_string()).collect()),
            out: Some("none".to_string()),
            ..crate::OpIR::new(simple_kind)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let first_class_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == expected_opcode)
        .count();
    let fallback_count = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.origin,
                    Some(kind) if kind == simple_kind
                )
        })
        .count();

    assert_eq!(
        first_class_count, 1,
        "{simple_kind} must lower to its first-class TIR opcode"
    );
    assert_eq!(
        fallback_count, 0,
        "{simple_kind} must not lower as Copy[_original_kind]"
    );
}

#[test]
fn module_cache_set_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_mutation(
        crate::OpKind::ModuleCacheSet,
        OpCode::ModuleCacheSet,
        &["name", "module"],
    );
}

#[test]
fn module_cache_del_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_mutation(
        crate::OpKind::ModuleCacheDel,
        OpCode::ModuleCacheDel,
        &["name"],
    );
}

#[test]
fn module_set_attr_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_mutation(
        crate::OpKind::ModuleSetAttr,
        OpCode::ModuleSetAttr,
        &["module", "name", "value"],
    );
}

#[test]
fn module_del_global_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_mutation(
        crate::OpKind::ModuleDelGlobal,
        OpCode::ModuleDelGlobal,
        &["module", "name"],
    );
}

#[test]
fn module_del_global_if_present_lowers_to_first_class_tir_opcode() {
    assert_first_class_module_mutation(
        crate::OpKind::ModuleDelGlobalIfPresent,
        OpCode::ModuleDelGlobalIfPresent,
        &["module", "name"],
    );
}

// Helper: collect all unique ValueIds from block arguments and op results.
fn all_value_ids(output: &SsaOutput) -> HashSet<ValueId> {
    let mut ids = HashSet::new();
    for block in &output.blocks {
        for arg in &block.args {
            ids.insert(arg.id);
        }
        for op in &block.ops {
            for &r in &op.results {
                ids.insert(r);
            }
        }
    }
    ids
}

// =======================================================================
// Test 1: Straight-line code — no block arguments needed
// =======================================================================
#[test]
fn straight_line_no_block_args() {
    // x = 1; y = x + 1
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "x"),     // x = 1
        op_args_out(crate::OpKind::Add, &["x"], "y"), // y = x + 1 (simplified)
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    assert_eq!(output.blocks.len(), 1, "straight-line code = 1 block");
    assert_eq!(
        total_block_args(&output),
        0,
        "no join points → no block args"
    );

    // Two ops that define variables → two distinct ValueIds in results.
    let ids = all_value_ids(&output);
    assert!(
        ids.len() >= 2,
        "need at least 2 ValueIds for x and y, got {}",
        ids.len()
    );

    // Check all values are typed.
    for id in &ids {
        assert!(
            output.types.contains_key(id),
            "ValueId {:?} should have a type",
            id
        );
    }
}

#[test]
fn import_name_lowers_to_import_opcode_instead_of_copy_fallback() {
    let ops = vec![
        op_args_out(crate::OpKind::ImportName, &["pathlib"], "mod"),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[0];
    assert!(
        entry.ops.iter().any(|op| op.opcode == OpCode::Import),
        "expected import_name to lower to OpCode::Import, got {:?}",
        entry.ops.iter().map(|op| op.opcode).collect::<Vec<_>>()
    );
}

#[test]
fn module_import_lowers_to_import_opcode_instead_of_copy_fallback() {
    let ops = vec![
        op_args_out(crate::OpKind::ModuleImport, &["pathlib"], "mod"),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[0];
    assert!(
        entry.ops.iter().any(|op| op.opcode == OpCode::Import),
        "expected module_import to lower to OpCode::Import, got {:?}",
        entry.ops.iter().map(|op| op.opcode).collect::<Vec<_>>()
    );
}

#[test]
fn module_set_attr_does_not_lower_as_store_attr_transport() {
    let ops = vec![
        op_args_out(crate::OpKind::ModuleImport, &["builtins"], "mod"),
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("answer".to_string()),
            out: Some("name".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_val_out(crate::OpKind::Const, 42, "value"),
        OpIR {
            kind: crate::OpKind::ModuleSetAttr,
            args: Some(vec![
                "mod".to_string(),
                "name".to_string(),
                "value".to_string(),
            ]),
            out: Some("none".to_string()),
            ..crate::OpIR::new(crate::OpKind::ModuleSetAttr)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[0];
    let module_store_count = entry
        .ops
        .iter()
        .filter(|op| {
            op.opcode == OpCode::StoreAttr && op.origin == Some(crate::OpKind::ModuleSetAttr)
        })
        .count();
    assert_eq!(
        module_store_count, 0,
        "module_set_attr must remain a module op instead of StoreAttr transport"
    );
}

#[test]
fn module_import_preserves_variable_operand_through_ssa() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("builtins".to_string()),
            out: Some("v62".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_args_out(crate::OpKind::ModuleImport, &["v62"], "v63"),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[0];
    let import_op = entry
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Import)
        .expect("expected import op");
    assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
}

#[test]
fn module_import_preserves_operand_with_module_obj_param_and_checks() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::Line,
            value: Some(7),
            ..crate::OpIR::new(crate::OpKind::Line)
        },
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("builtins".to_string()),
            out: Some("v62".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        op_args_out(crate::OpKind::ModuleImport, &["v62"], "v63"),
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(3),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("_builtins".to_string()),
            out: Some("v64".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        OpIR {
            kind: crate::OpKind::ModuleSetAttr,
            args: Some(vec![
                "__molt_module_obj__".to_string(),
                "v64".to_string(),
                "v63".to_string(),
            ]),
            out: Some("none".to_string()),
            ..crate::OpIR::new(crate::OpKind::ModuleSetAttr)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa_with_params(&cfg, &ops, &["__molt_module_obj__".into()]);
    let import_op = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::Import)
        .expect("expected import op");
    assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
}

// =======================================================================
// Test 2: Join point — if/else assigns x, merge needs block arg
// =======================================================================
#[test]
fn join_point_block_argument() {
    // if c: x = 1 else: x = 2; ret
    //
    // SimpleIR layout:
    //   const c → v0          (block 0)
    //   if [v0]               (block 0, ends it)
    //   const 1 → x           (block 1: then)
    //   else                   (block 2: else)
    //   const 2 → x           (block 2: else body)
    //   end_if                 (block 3: join)
    //   ret [x]               (block 3: join)
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "v0"), // 0
        op_args(crate::OpKind::If, &["v0"]),       // 1 — terminates block 0
        op_val_out(crate::OpKind::Const, 1, "x"),  // 2 — then block
        op(crate::OpKind::Else),                   // 3 — else block start
        op_val_out(crate::OpKind::Const, 2, "x"),  // 4 — else body
        op(crate::OpKind::EndIf),                  // 5 — join block
        op_args(crate::OpKind::Ret, &["x"]),       // 6 — return x
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    assert!(
        total_block_args(&output) > 0,
        "SSA conversion should insert a merge arg for x, got 0 total block args"
    );
    assert_eq!(
        total_block_args(&output),
        1,
        "exactly one merged block arg should exist for x"
    );
}

// =======================================================================
// Test 3: Loop variable — block arg at loop header
// =======================================================================
#[test]
fn loop_variable_block_argument() {
    // x = 0; loop { x = x + 1 }
    //
    // SimpleIR:
    //   const 0 → x          (block 0)
    //   loop_start            (block 1: header)
    //   add [x] → x          (block 1: body, or block 2)
    //   loop_end              (block 2/3: back-edge)
    //   ret_void              (after loop)
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "x"),     // 0
        op(crate::OpKind::LoopStart),                 // 1 — loop header
        op_args_out(crate::OpKind::Add, &["x"], "x"), // 2 — x = x + 1
        op(crate::OpKind::LoopEnd),                   // 3 — back-edge
        op(crate::OpKind::RetVoid),                   // 4 — after
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // The loop header block should have a block argument for x.
    let header_bid = cfg
        .blocks
        .iter()
        .position(|b| b.start_op <= 1 && b.end_op > 1)
        .unwrap();
    let header_block = &output.blocks[header_bid];

    assert!(
        !header_block.args.is_empty(),
        "loop header should have block arg for x, got {} args",
        header_block.args.len()
    );
}

// =======================================================================
// Test 4: Multiple variables — two block args at join
// =======================================================================
#[test]
fn multiple_variables_at_join() {
    // if c: x = 1; y = 2 else: x = 3; y = 4; use x, y
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "v0"), // 0
        op_args(crate::OpKind::If, &["v0"]),       // 1
        op_val_out(crate::OpKind::Const, 1, "x"),  // 2 then
        op_val_out(crate::OpKind::Const, 2, "y"),  // 3 then
        op(crate::OpKind::Else),                   // 4 else
        op_val_out(crate::OpKind::Const, 3, "x"),  // 5 else
        op_val_out(crate::OpKind::Const, 4, "y"),  // 6 else
        op(crate::OpKind::EndIf),                  // 7 join
        op_args(crate::OpKind::Ret, &["x", "y"]),  // 8
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    assert_eq!(
        total_block_args(&output),
        2,
        "SSA conversion should insert exactly 2 merged block args (x and y), got {}",
        total_block_args(&output)
    );

    // All block arg ValueIds should be unique.
    let arg_ids: HashSet<ValueId> = output
        .blocks
        .iter()
        .flat_map(|block| block.args.iter().map(|arg| arg.id))
        .collect();
    assert_eq!(arg_ids.len(), 2, "block arg ValueIds should be unique");
}

#[test]
fn block_argument_and_edge_order_is_deterministic() {
    fn structural_signature(output: &SsaOutput) -> String {
        output
            .blocks
            .iter()
            .map(|block| {
                let args: Vec<_> = block.args.iter().map(|arg| arg.id).collect();
                let ops: Vec<_> = block
                    .ops
                    .iter()
                    .map(|op| (op.opcode, op.operands.clone(), op.results.clone()))
                    .collect();
                format!("{:?}:{args:?}:{ops:?}:{:?}", block.id, block.terminator)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "condition"),
        op_args(crate::OpKind::If, &["condition"]),
        op_val_out(crate::OpKind::Const, 1, "zeta"),
        op_val_out(crate::OpKind::Const, 2, "alpha"),
        op_val_out(crate::OpKind::Const, 3, "middle"),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 4, "zeta"),
        op_val_out(crate::OpKind::Const, 5, "alpha"),
        op_val_out(crate::OpKind::Const, 6, "middle"),
        op(crate::OpKind::EndIf),
        op_args(crate::OpKind::Ret, &["zeta", "alpha", "middle"]),
    ];
    let cfg = CFG::build(&ops);
    let expected = structural_signature(&convert_to_ssa(&cfg, &ops));
    for _ in 0..32 {
        assert_eq!(
            structural_signature(&convert_to_ssa(&cfg, &ops)),
            expected,
            "SSA block argument and predecessor edge order must not inherit HashSet seeds"
        );
    }
}

// =======================================================================
// Test 5: Empty CFG
// =======================================================================
#[test]
fn empty_cfg_produces_no_blocks() {
    let cfg = CFG::build(&[]);
    let output = convert_to_ssa(&cfg, &[]);
    assert!(output.blocks.is_empty());
}

// =======================================================================
// Test 6: SSA property — each ValueId defined exactly once
// =======================================================================
#[test]
fn ssa_property_unique_definitions() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "v0"),
        op_args(crate::OpKind::If, &["v0"]),
        op_val_out(crate::OpKind::Const, 1, "x"),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 2, "x"),
        op(crate::OpKind::EndIf),
        op_args(crate::OpKind::Ret, &["x"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // Collect all definition sites (op results + block args).
    let mut def_ids = Vec::new();
    for block in &output.blocks {
        for arg in &block.args {
            def_ids.push(arg.id);
        }
        for op in &block.ops {
            for &r in &op.results {
                def_ids.push(r);
            }
        }
    }

    // All ValueIds should be unique (SSA property).
    let unique: HashSet<ValueId> = def_ids.iter().copied().collect();
    assert_eq!(
        def_ids.len(),
        unique.len(),
        "SSA property violated: {} definitions but only {} unique ValueIds",
        def_ids.len(),
        unique.len()
    );
}

// =======================================================================
// Test 7: Branch args match block arg count
// =======================================================================
#[test]
fn branch_args_match_block_arg_count() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "v0"),
        op_args(crate::OpKind::If, &["v0"]),
        op_val_out(crate::OpKind::Const, 1, "x"),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 2, "x"),
        op(crate::OpKind::EndIf),
        op_args(crate::OpKind::Ret, &["x"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // For every branch to a block, the number of passed args must match
    // the target block's argument count.
    let block_map: HashMap<BlockId, &TirBlock> = output.blocks.iter().map(|b| (b.id, b)).collect();

    for block in &output.blocks {
        block.terminator.for_each_edge(|target, args| {
            if let Some(target_block) = block_map.get(&target) {
                assert_eq!(
                    args.len(),
                    target_block.args.len(),
                    "edge from {} to {} passes {} args but target expects {}",
                    block.id,
                    target,
                    args.len(),
                    target_block.args.len()
                );
            }
        });
    }
}

// =======================================================================
// Test 7b: Empty THEN (`if cond: pass / else: <body>`) must NOT swap
//          THEN/ELSE bodies in CondBranch.  Regression for module-level
//          `except*` residual ExceptionGroup propagation: the frontend
//          emits `if exc IS None: pass / else: <recovery>` after every
//          guarded `del` in `_emit_module_global_del_safe` and the SSA
//          builder used to mis-route the TRUE edge into the recovery
//          body, inverting the guard.
// =======================================================================
#[test]
fn empty_then_with_else_does_not_swap_branch_bodies() {
    // Pattern:
    //   if cond:
    //       pass
    //   else:
    //       y = 7   <-- recovery body must run when cond is FALSE
    let ops = vec![
        op_val_out(crate::OpKind::ConstBool, 1, "cond"), // 0: cond
        op_args(crate::OpKind::If, &["cond"]),           // 1: if (empty THEN)
        op(crate::OpKind::Else),                         // 2: else
        op_val_out(crate::OpKind::Const, 7, "y"),        // 3: ELSE body
        op(crate::OpKind::EndIf),                        // 4: join
        op(crate::OpKind::RetVoid),                      // 5
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // Locate the `if` block.
    let if_block_id = {
        let mut found = None;
        for block in &output.blocks {
            if matches!(&block.terminator, Terminator::CondBranch { .. }) {
                found = Some(block.id);
                break;
            }
        }
        found.expect("if block must exist")
    };
    let if_block = output
        .blocks
        .iter()
        .find(|b| b.id == if_block_id)
        .expect("if block lookup");

    let (then_block, else_block) = match &if_block.terminator {
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => (*then_block, *else_block),
        _ => unreachable!("verified above"),
    };

    // The ELSE block (`y = 7`) must be the FALSE-path destination.
    // Before the fix, ssa.rs treated fall-through as TRUE, which
    // routed the recovery body into the THEN edge and broke
    // `if cond: pass / else: <recovery>` semantics.
    let else_target = output
        .blocks
        .iter()
        .find(|b| b.id == else_block)
        .expect("else block lookup");
    let else_has_recovery = else_target
        .ops
        .iter()
        .any(|op| op.opcode == OpCode::ConstInt);
    assert!(
        else_has_recovery,
        "FALSE path (else_block) must contain the recovery `const 7`; \
         builder is mis-routing branches"
    );

    // The THEN target must NOT contain the recovery body.
    if then_block != else_block {
        let then_target = output
            .blocks
            .iter()
            .find(|b| b.id == then_block)
            .expect("then block lookup");
        let then_has_recovery = then_target
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ConstInt);
        assert!(
            !then_has_recovery,
            "TRUE path (then_block) must NOT contain the FALSE-only recovery body"
        );
    }
}

// =======================================================================
// Test 8: Exception handlers should not receive dead future vars
// =======================================================================
#[test]
fn check_exception_does_not_capture_future_dead_vars() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "x"),
        op_val(crate::OpKind::TryStart, 100),
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(100),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        op_val_out(crate::OpKind::Const, 2, "y"),
        op_val(crate::OpKind::TryEnd, 100),
        op(crate::OpKind::RetVoid),
        op_val(crate::OpKind::Label, 100),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let handler_bid = cfg
        .blocks
        .iter()
        .position(|b| b.start_op <= 6 && b.end_op > 6)
        .expect("handler block should exist");
    let handler_block = &output.blocks[handler_bid];
    assert!(
        handler_block.args.is_empty(),
        "handler block should not receive dead future vars, got {} args",
        handler_block.args.len()
    );

    let check_op = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::CheckException)
        .expect("check_exception op should survive SSA conversion");
    assert!(
        check_op.operands.is_empty(),
        "check_exception should not carry dead handler args, got operands {:?}",
        check_op.operands
    );
}

#[test]
fn every_simple_exception_transfer_spelling_keeps_dominating_handler_state() {
    for transfer_kind in [
        crate::OpKind::CheckException,
        crate::OpKind::AsyncWorkPoll,
        crate::OpKind::TryStart,
    ] {
        let ops = vec![
            op_val_out(crate::OpKind::Const, 7, "live"),
            op_val(transfer_kind, 100),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 100),
            op_args(crate::OpKind::Print, &["live"]),
            op(crate::OpKind::RetVoid),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let handler_bid = cfg
            .blocks
            .iter()
            .position(|block| block.start_op <= 3 && block.end_op > 3)
            .expect("handler block");
        let handler = &output.blocks[handler_bid];
        assert!(
            handler.args.is_empty(),
            "{transfer_kind}: direct dominating live value"
        );
        let live = output.blocks[cfg.entry]
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstInt)
            .unwrap()
            .results[0];
        let observed = handler
            .ops
            .iter()
            .find(|op| {
                op.source_op_index()
                    .is_some_and(|index| ops[index].kind == crate::OpKind::Print)
            })
            .expect("handler observer must remain");
        assert_eq!(
            observed.operands,
            vec![live],
            "{transfer_kind}: original binding"
        );
        let transfer = output
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .find(|op| matches!(op.opcode, OpCode::CheckException | OpCode::TryStart))
            .expect("exception transfer op");
        assert_eq!(
            transfer.operands.len(),
            handler.args.len(),
            "{transfer_kind} must serialize one operand per handler block argument"
        );
    }
}

#[test]
fn check_exception_in_multi_block_try_does_not_capture_dead_vars() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "c"),
        op_val(crate::OpKind::TryStart, 100),
        op_args(crate::OpKind::If, &["c"]),
        op_val_out(crate::OpKind::Const, 1, "x"),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 2, "x"),
        op(crate::OpKind::EndIf),
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(100),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        op_val_out(crate::OpKind::Const, 3, "y"),
        op_val(crate::OpKind::TryEnd, 100),
        op(crate::OpKind::RetVoid),
        op_val(crate::OpKind::Label, 100),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let handler_bid = cfg
        .blocks
        .iter()
        .position(|b| b.start_op <= 11 && b.end_op > 11)
        .expect("handler block should exist");
    let handler_block = &output.blocks[handler_bid];
    assert!(
        handler_block.args.is_empty(),
        "multi-block handler should not receive dead vars, got {} args",
        handler_block.args.len()
    );

    let check_ops: Vec<_> = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == OpCode::CheckException)
        .collect();
    assert_eq!(
        check_ops.len(),
        1,
        "expected exactly one check_exception op"
    );
    assert!(
        check_ops[0].operands.is_empty(),
        "check_exception should not carry dead handler args, got operands {:?}",
        check_ops[0].operands
    );
}

#[test]
fn check_exception_threads_live_cleanup_state_for_method_guarded_field_set() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::ExceptionStackEnter,
            out: Some("v88".to_string()),
            ..crate::OpIR::new(crate::OpKind::ExceptionStackEnter)
        },
        OpIR {
            kind: crate::OpKind::ExceptionStackDepth,
            out: Some("v89".to_string()),
            ..crate::OpIR::new(crate::OpKind::ExceptionStackDepth)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("self".to_string()),
            args: Some(vec!["self".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::Line,
            value: Some(3),
            ..crate::OpIR::new(crate::OpKind::Line)
        },
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(1),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        op_val_out(crate::OpKind::Const, 1, "v90"),
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("C".to_string()),
            out: Some("v91".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("method_trace".to_string()),
            out: Some("v92".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        OpIR {
            kind: crate::OpKind::ModuleCacheGet,
            args: Some(vec!["v92".to_string()]),
            out: Some("v93".to_string()),
            ..crate::OpIR::new(crate::OpKind::ModuleCacheGet)
        },
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(1),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        OpIR {
            kind: crate::OpKind::ModuleGetAttr,
            args: Some(vec!["v93".to_string(), "v91".to_string()]),
            out: Some("v94".to_string()),
            ..crate::OpIR::new(crate::OpKind::ModuleGetAttr)
        },
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(1),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        op_val_out(crate::OpKind::Const, 3, "v95"),
        OpIR {
            kind: crate::OpKind::GuardedFieldSet,
            args: Some(vec![
                "self".to_string(),
                "v95".to_string(),
                "v90".to_string(),
            ]),
            s_value: Some("x".to_string()),
            value: Some(0),
            class_definition: Some("class:method_trace:C".to_string()),
            out: Some("none".to_string()),
            ..crate::OpIR::new(crate::OpKind::GuardedFieldSet)
        },
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(1),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        op_val_out(crate::OpKind::Const, 0, "v96"),
        OpIR {
            kind: crate::OpKind::Ret,
            args: Some(vec!["v96".to_string()]),
            ..crate::OpIR::new(crate::OpKind::Ret)
        },
        op_val(crate::OpKind::Label, 1),
        OpIR {
            kind: crate::OpKind::ExceptionStackSetDepth,
            args: Some(vec!["v89".to_string()]),
            out: Some("none".to_string()),
            ..crate::OpIR::new(crate::OpKind::ExceptionStackSetDepth)
        },
        OpIR {
            kind: crate::OpKind::ExceptionStackExit,
            args: Some(vec!["v88".to_string()]),
            out: Some("none".to_string()),
            ..crate::OpIR::new(crate::OpKind::ExceptionStackExit)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let handler_bid = cfg
        .blocks
        .iter()
        .position(|b| b.start_op <= 17 && b.end_op > 17)
        .expect("handler block should exist");
    let handler_block = &output.blocks[handler_bid];
    assert!(
        handler_block.args.is_empty(),
        "cleanup state already dominates every entry"
    );
    let result_at = |source| {
        output
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .find(|op| op.source_op_index() == Some(source))
            .unwrap()
            .results[0]
    };
    let stack = result_at(0);
    let depth = result_at(1);
    for (kind, expected) in [
        (crate::OpKind::ExceptionStackSetDepth, depth),
        (crate::OpKind::ExceptionStackExit, stack),
    ] {
        let cleanup = handler_block
            .ops
            .iter()
            .find(|op| {
                op.source_op_index()
                    .is_some_and(|index| ops[index].kind == kind)
            })
            .expect("actual cleanup operation must remain");
        assert_eq!(
            cleanup.operands,
            vec![expected],
            "{kind}: preserve original cleanup state"
        );
    }
    let checks: Vec<_> = output
        .blocks
        .iter()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::CheckException)
        .collect();
    assert_eq!(checks.len(), 4, "all authored exception checks remain");
    assert!(checks.iter().all(|op| op.operands.is_empty()));
}

#[test]
fn join_block_does_not_receive_dead_branch_only_vars() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "c"),
        op_args(crate::OpKind::If, &["c"]),
        op_val_out(crate::OpKind::Const, 1, "x"),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 2, "y"),
        op(crate::OpKind::EndIf),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let join_bid = cfg
        .blocks
        .iter()
        .position(|b| b.start_op <= 5 && b.end_op > 5)
        .expect("join block should exist");
    let join_block = &output.blocks[join_bid];
    assert!(
        join_block.args.is_empty(),
        "join block must not receive dead branch-only vars, got {} args",
        join_block.args.len()
    );
}

#[test]
fn branch_defined_live_var_gets_none_on_missing_edge() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "c"),
        op_args(crate::OpKind::If, &["c"]),
        op_val_out(crate::OpKind::Const, 1, "x"),
        op(crate::OpKind::EndIf),
        op_args(crate::OpKind::Ret, &["x"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    assert_eq!(
        total_block_args(&output),
        1,
        "live branch-defined var should still require one merge arg"
    );

    let undef_vid = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::ConstNone)
        .and_then(|op| op.results.first().copied())
        .expect("SSA should materialize None at the missing edge");

    let has_undef_branch_arg = output.blocks.iter().any(|block| {
        let mut found = false;
        block
            .terminator
            .for_each_edge(|_, args| found |= args.contains(&undef_vid));
        found
    });
    assert!(
        has_undef_branch_arg,
        "missing branch edge must pass the explicit undef None value"
    );
}

#[test]
fn post_if_store_var_reaches_following_load() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::Missing,
            out: Some("seed".to_string()),
            ..crate::OpIR::new(crate::OpKind::Missing)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("v".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::ConstNone,
            out: Some("cond".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstNone)
        },
        op_args(crate::OpKind::If, &["cond"]),
        op(crate::OpKind::Else),
        op_val_out(crate::OpKind::Const, 1, "one"),
        op_args_out(crate::OpKind::Or, &["cond", "one"], "picked"),
        op(crate::OpKind::EndIf),
        op_args_out(crate::OpKind::Phi, &["cond", "picked"], "joined"),
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("v".to_string()),
            args: Some(vec!["joined".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::LoadVar,
            var: Some("v".to_string()),
            out: Some("loaded".to_string()),
            ..crate::OpIR::new(crate::OpKind::LoadVar)
        },
        op_args(crate::OpKind::Ret, &["loaded"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let seed_values: HashSet<ValueId> = output
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::ConstNone
                || matches!(
                    op.origin,
                    Some(kind) if kind == crate::OpKind::Missing
                )
        })
        .flat_map(|op| op.results.iter().copied())
        .collect();
    let returned = output
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::Return { values } if !values.is_empty() => values.first().copied(),
            _ => None,
        })
        .expect("expected return value");

    assert!(
        !seed_values.contains(&returned),
        "load_var after the if/join must not collapse back to any seed missing/None value"
    );
}

#[test]
fn delete_var_uses_explicit_old_operand_not_target_metadata() {
    let ops = vec![
        OpIR {
            kind: crate::OpKind::Missing,
            out: Some("seed".to_string()),
            ..crate::OpIR::new(crate::OpKind::Missing)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("item".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::ConstStr,
            s_value: Some("old".to_string()),
            out: Some("old_value".to_string()),
            ..crate::OpIR::new(crate::OpKind::ConstStr)
        },
        OpIR {
            kind: crate::OpKind::StoreVar,
            var: Some("item".to_string()),
            args: Some(vec!["old_value".to_string()]),
            ..crate::OpIR::new(crate::OpKind::StoreVar)
        },
        OpIR {
            kind: crate::OpKind::LoadVar,
            var: Some("item".to_string()),
            out: Some("old_loaded".to_string()),
            ..crate::OpIR::new(crate::OpKind::LoadVar)
        },
        OpIR {
            kind: crate::OpKind::Missing,
            out: Some("gone".to_string()),
            ..crate::OpIR::new(crate::OpKind::Missing)
        },
        OpIR {
            kind: crate::OpKind::DeleteVar,
            var: Some("item".to_string()),
            args: Some(vec!["gone".to_string(), "old_loaded".to_string()]),
            ..crate::OpIR::new(crate::OpKind::DeleteVar)
        },
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let entry = &output.blocks[cfg.entry];
    assert!(entry.ops.iter().all(|op| op.opcode != OpCode::ConstNone));
    let seed = entry
        .ops
        .iter()
        .find(|op| {
            matches!(
                op.attrs.get("_simple_out"),
                Some(AttrValue::Str(name)) if name == "seed"
            )
        })
        .and_then(|op| op.results.first().copied())
        .expect("seed missing value must exist");
    let gone = entry
        .ops
        .iter()
        .find(|op| {
            matches!(
                op.attrs.get("_simple_out"),
                Some(AttrValue::Str(name)) if name == "gone"
            )
        })
        .and_then(|op| op.results.first().copied())
        .expect("delete missing value must exist");
    let delete = entry
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::DeleteVar)
        .expect("delete_var must lower to first-class TIR");

    assert_eq!(delete.operands.len(), 2);
    assert_eq!(delete.operands[0], gone);
    let old_loaded = entry
        .ops
        .iter()
        .find(|op| op.attrs.get("_simple_out") == Some(&AttrValue::Str("old_loaded".into())))
        .expect("explicit old-slot load");
    assert_eq!(delete.operands[1], old_loaded.results[0]);
    assert_ne!(
        delete.operands[1], seed,
        "old-slot operand must not collapse to the initial missing store"
    );
}

// =======================================================================
// Regression: try/finally body assignment must thread through the
// post-handler join via a phi (block argument).
//
// Shape (mirrors `copy.deepcopy`'s try/finally that exposed the bug):
//
//   r = missing            // pre-init in entry
//   try_start L_handler
//     check_exception L_handler
//     r = ...               // assignment inside try body
//     check_exception L_handler
//   try_end
//   ret r                   // use of r AFTER the try/finally
//   L_handler:              // implicit handler entry (exception edge only)
//     ret
//
// Without the augmented dominator construction, the post-handler join
// block sees only the success-path predecessor in the regular CFG, and
// the handler-side definition of `r` is invisible. The result-using
// block then references `r` defined on a path that does not dominate
// it, which the LIR verifier flags as an SSA dominance violation.
// =======================================================================
#[test]
fn try_finally_threads_assignment_through_post_handler_join() {
    // CPython try/finally lowering, simplified:
    //
    //   r = missing                      // entry-block initializer
    //   try_start L_handler
    //     ... if c:                       // structured branch inside try
    //       r = 1                         // try-body assignment
    //     else:
    //       r = 2
    //   try_end
    //   jump L_join                      // skip handler on success
    //   L_handler:                        // exception path enters here
    //     // (handler runs cleanup, then merges to L_join)
    //   L_join:
    //     ret r                           // use of r AFTER handler merges
    //
    // The structured `if` inside the try forces a block split, so the
    // try body is multiple blocks. Without the augmented dominance
    // analysis, the post-handler `L_join` block sees only the
    // success-path predecessor (because the handler block has no
    // regular CFG predecessors) and the dominator analysis treats the
    // try body as dominating the join — producing a return that
    // references `r` on a path that does not actually dominate it
    // through every runtime control flow.
    let ops = vec![
        // r = missing
        op_val_out(crate::OpKind::Const, 0, "r"),
        // c = 1
        op_val_out(crate::OpKind::Const, 1, "c"),
        // try_start L100  (handler at label 100)
        op_val(crate::OpKind::TryStart, 100),
        // if c:
        op_args(crate::OpKind::If, &["c"]),
        //   r = 1
        op_val_out(crate::OpKind::Const, 1, "r"),
        // else:
        op(crate::OpKind::Else),
        //   r = 2
        op_val_out(crate::OpKind::Const, 2, "r"),
        // end_if
        op(crate::OpKind::EndIf),
        // check_exception L100
        OpIR {
            kind: crate::OpKind::CheckException,
            value: Some(100),
            ..crate::OpIR::new(crate::OpKind::CheckException)
        },
        // try_end
        op_val(crate::OpKind::TryEnd, 100),
        // jump L_join
        op_val(crate::OpKind::Jump, 200),
        // L_handler:
        op_val(crate::OpKind::Label, 100),
        // (handler cleanup body — no assignment to r)
        // jump L_join (handler merges back into the post-try control flow)
        op_val(crate::OpKind::Jump, 200),
        // L_join:
        op_val(crate::OpKind::Label, 200),
        // ret r
        op_args(crate::OpKind::Ret, &["r"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // Locate the return block.
    let return_block = output
        .blocks
        .iter()
        .find(|b| matches!(b.terminator, Terminator::Return { .. }))
        .expect("must produce at least one return block");
    let returned_value = match &return_block.terminator {
        Terminator::Return { values } => *values.first().expect("return must carry one value (r)"),
        _ => unreachable!(),
    };

    // Collect which block each value is defined in.
    let mut def_block: HashMap<ValueId, BlockId> = HashMap::new();
    for block in &output.blocks {
        for arg in &block.args {
            def_block.insert(arg.id, block.id);
        }
        for op in &block.ops {
            for &r in &op.results {
                def_block.insert(r, block.id);
            }
        }
    }
    let def_bid = *def_block
        .get(&returned_value)
        .expect("returned value must be defined somewhere");

    // Build the augmented predecessor relation used at runtime: regular
    // CFG predecessors PLUS exception edges. The returned value's
    // defining block must dominate the return block under THIS
    // relation. The bug being regressed is precisely that the
    // dominance check using only regular predecessors says the def
    // block dominates the return, but the augmented (runtime)
    // relation reveals it does not.
    let n = cfg.blocks.len();
    let mut aug_preds: Vec<Vec<usize>> = cfg.predecessors.clone();
    for &(from_bid, handler_bid) in &cfg.exception_edges {
        if !aug_preds[handler_bid].contains(&from_bid) {
            aug_preds[handler_bid].push(from_bid);
        }
    }
    // BFS from each block to determine which blocks each block is
    // reachable from in the augmented CFG (i.e., what its augmented
    // predecessors transitively reach to). Then for every block on a
    // path from entry to the return block, the def block must lie on
    // every such path — i.e., must be a dominator. Compute the set of
    // blocks that strictly dominate the return block under augmented
    // edges by a simple intersection-of-paths algorithm.
    let return_bid_idx = return_block.id.0 as usize;
    // Build augmented successors.
    let mut aug_succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (bid, preds) in aug_preds.iter().enumerate() {
        for &p in preds {
            aug_succs[p].push(bid);
        }
    }
    // For every path from entry to return_bid in aug graph, def_bid
    // must appear. Equivalent: removing def_bid from the graph makes
    // return_bid unreachable from entry.
    let def_bid_idx = def_bid.0 as usize;
    let entry_bid = output.blocks[0].id.0 as usize;
    let dominates = if def_bid_idx == return_bid_idx || def_bid_idx == entry_bid {
        true
    } else {
        // Reachability of return_bid from entry, skipping def_bid.
        let mut visited = vec![false; n];
        visited[def_bid_idx] = true; // mark blocked
        let mut stack = vec![entry_bid];
        visited[entry_bid] = true;
        let mut reaches_return = false;
        while let Some(b) = stack.pop() {
            if b == return_bid_idx {
                reaches_return = true;
                break;
            }
            for &succ in &aug_succs[b] {
                if !visited[succ] {
                    visited[succ] = true;
                    stack.push(succ);
                }
            }
        }
        !reaches_return
    };

    assert!(
        dominates,
        "returned `r` (value {returned_value:?}) is defined in bb{} but \
         that block does not dominate the return block bb{} under the \
         *augmented* CFG (regular edges + exception edges). This is the \
         deepcopy try/finally SSA dominance regression: without \
         folding exception edges into the dominance analysis, a \
         try-body assignment leaks past the handler-side control \
         flow into a return that the success-path definition does \
         not actually dominate at runtime.",
        def_bid.0, return_block.id.0,
    );
}

// =======================================================================
// Test 9: All output values are typed as DynBox (when no hint)
// =======================================================================
#[test]
fn all_values_typed_dynbox() {
    let ops = vec![
        op_val_out(crate::OpKind::Const, 1, "x"),
        op_args_out(crate::OpKind::Add, &["x"], "y"),
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    for (&vid, ty) in &output.types {
        assert_eq!(
            *ty,
            TirType::DynBox,
            "ValueId {:?} should be DynBox, got {:?}",
            vid,
            ty
        );
    }
}

// =======================================================================
// Test 10: type_hint="<UserClass>" refines result type to UserClass
// =======================================================================
/// SSA lift refines a result value's type from DynBox to
/// `UserClass(class_id)` when the SimpleIR op carries
/// `type_hint="<class_id>"` and the hint is identifier-shaped.
/// This is the live use of `TirType::UserClass` — without it,
/// the type system has no way to distinguish typed-class
/// instances from arbitrary boxed values, and downstream
/// passes (escape, devirt, GVN) cannot reason about class
/// identity.
#[test]
fn user_class_type_hint_refines_result_type() {
    let mut alloc_op = op_args_out(crate::OpKind::ObjectNewBound, &["cls"], "p");
    alloc_op.type_hint = Some("Point".to_string());
    // Carry the size attr so the op is well-formed for the
    // Phase 5 step 3 lowering (24 bytes = 2 ints + __dict__
    // slot).  Not strictly required for the type-refine test
    // but matches what the frontend actually emits.
    alloc_op.value = Some(24);
    let ops = vec![
        op_val_out(crate::OpKind::Const, 0, "cls"),
        alloc_op,
        op(crate::OpKind::RetVoid),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    // Find the ObjectNewBound op's result and verify its type.
    let entry = &output.blocks[0];
    let alloc_result = entry
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::ObjectNewBound)
        .and_then(|op| op.results.first().copied())
        .expect("expected ObjectNewBound op with a result");
    assert_eq!(
        output.types[&alloc_result],
        TirType::UserClass("Point".into()),
        "type_hint=Point on object_new_bound must refine the \
         result value's type from DynBox to UserClass(\"Point\")"
    );
}

/// Builtin-tagged hints (`type_hint=int`, etc.) are intentionally NOT
/// refined by this lift logic. Scalar representation must flow through
/// type-refine and function-owned value facts, not TIR attrs copied from
/// SimpleIR transport hints.
#[test]
fn builtin_type_hint_does_not_refine_at_lift() {
    let mut const_op = op_val_out(crate::OpKind::Const, 42, "x");
    const_op.type_hint = Some("int".to_string());
    let ops = vec![const_op, op(crate::OpKind::RetVoid)];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);

    let entry = &output.blocks[0];
    let const_result = entry
        .ops
        .iter()
        .find(|op| matches!(op.opcode, OpCode::ConstInt))
        .and_then(|op| op.results.first().copied())
        .expect("expected const op with a result");
    assert_eq!(
        output.types[&const_result],
        TirType::DynBox,
        "builtin type hint `int` must NOT refine at lift — \
         the refinement path for scalars goes through type-refine \
         and function-owned value facts, not the lift itself"
    );
}
