use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::run;

fn arg(id: ValueId, ty: TirType) -> TirValue {
    TirValue { id, ty }
}

fn copy(operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn check_exception(label: i64, operands: Vec<ValueId>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(label));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands,
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn try_start(label: i64, operands: Vec<ValueId>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(label));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TryStart,
        operands,
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn add_type(func: &mut TirFunction, id: ValueId, ty: TirType) {
    func.value_types.insert(id, ty);
}

#[test]
fn prunes_unused_branch_block_arg_and_incoming_payload() {
    let mut func = TirFunction::new("branch_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let join = func.fresh_block();
    let used = func.fresh_value();
    let dead = func.fresh_value();
    let out = func.fresh_value();
    for value in [used, dead, out] {
        add_type(&mut func, value, TirType::I64);
    }

    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: join,
        args: vec![used, dead],
    };
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![arg(used, TirType::I64), arg(dead, TirType::I64)],
            ops: vec![copy(vec![used], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 2);
    assert_eq!(func.blocks[&join].args.len(), 1);
    assert_eq!(func.blocks[&join].args[0].id, used);
    assert!(!func.value_types.contains_key(&dead));
    let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
        panic!("expected entry branch");
    };
    assert_eq!(args, &vec![used]);
}

#[test]
fn prunes_cond_branch_payloads_on_each_matching_edge() {
    let mut func = TirFunction::new("cond_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let cond = func.fresh_value();
    let then_used = func.fresh_value();
    let then_dead = func.fresh_value();
    let else_dead = func.fresh_value();
    let out = func.fresh_value();
    for value in [cond, then_used, then_dead, else_dead, out] {
        add_type(&mut func, value, TirType::I64);
    }
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block,
        then_args: vec![then_used, then_dead],
        else_block,
        else_args: vec![else_dead],
    };
    func.blocks.insert(
        then_block,
        TirBlock {
            id: then_block,
            args: vec![arg(then_used, TirType::I64), arg(then_dead, TirType::I64)],
            ops: vec![copy(vec![then_used], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![arg(else_dead, TirType::I64)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 4);
    let Terminator::CondBranch {
        then_args,
        else_args,
        ..
    } = &func.blocks[&entry].terminator
    else {
        panic!("expected cond branch");
    };
    assert_eq!(then_args, &vec![then_used]);
    assert!(else_args.is_empty());
    assert_eq!(func.blocks[&then_block].args.len(), 1);
    assert!(func.blocks[&else_block].args.is_empty());
}

#[test]
fn prunes_switch_payloads_on_cases_and_default() {
    let mut func = TirFunction::new("switch_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let case_block = func.fresh_block();
    let default_block = func.fresh_block();
    let selector = func.fresh_value();
    let case_used = func.fresh_value();
    let case_dead = func.fresh_value();
    let default_dead = func.fresh_value();
    let out = func.fresh_value();
    for value in [selector, case_used, case_dead, default_dead, out] {
        add_type(&mut func, value, TirType::I64);
    }
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Switch {
        value: selector,
        cases: vec![(0, case_block, vec![case_used, case_dead])],
        default: default_block,
        default_args: vec![default_dead],
    };
    func.blocks.insert(
        case_block,
        TirBlock {
            id: case_block,
            args: vec![arg(case_used, TirType::I64), arg(case_dead, TirType::I64)],
            ops: vec![copy(vec![case_used], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );
    func.blocks.insert(
        default_block,
        TirBlock {
            id: default_block,
            args: vec![arg(default_dead, TirType::I64)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 4);
    assert_eq!(func.blocks[&case_block].args.len(), 1);
    assert_eq!(func.blocks[&case_block].args[0].id, case_used);
    assert!(func.blocks[&default_block].args.is_empty());
    let Terminator::Switch {
        value,
        cases,
        default_args,
        ..
    } = &func.blocks[&entry].terminator
    else {
        panic!("expected switch");
    };
    assert_eq!(*value, selector);
    assert_eq!(cases[0].2, vec![case_used]);
    assert!(default_args.is_empty());
}

#[test]
fn prunes_unused_check_exception_handler_payloads() {
    let mut func = TirFunction::new("exception_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let handler = func.fresh_block();
    let used = func.fresh_value();
    let dead = func.fresh_value();
    let out = func.fresh_value();
    for value in [used, dead, out] {
        add_type(&mut func, value, TirType::DynBox);
    }
    func.label_id_map.insert(handler.0, 99);
    func.blocks.get_mut(&entry).unwrap().ops = vec![check_exception(99, vec![used, dead])];
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
            ops: vec![copy(vec![used], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 2);
    assert_eq!(func.blocks[&handler].args.len(), 1);
    assert_eq!(func.blocks[&entry].ops[0].operands, vec![used]);
}

#[test]
fn prunes_unused_try_start_handler_payloads() {
    let mut func = TirFunction::new("try_start_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let handler = func.fresh_block();
    let used = func.fresh_value();
    let dead = func.fresh_value();
    let out = func.fresh_value();
    for value in [used, dead, out] {
        add_type(&mut func, value, TirType::DynBox);
    }
    func.label_id_map.insert(handler.0, 77);
    func.blocks.get_mut(&entry).unwrap().ops = vec![try_start(77, vec![used, dead])];
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
            ops: vec![copy(vec![used], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 2);
    assert_eq!(func.blocks[&handler].args.len(), 1);
    assert_eq!(func.blocks[&handler].args[0].id, used);
    assert_eq!(func.blocks[&entry].ops[0].operands, vec![used]);
}

#[test]
fn prunes_state_dispatch_payloads() {
    let mut func = TirFunction::new("state_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let resume = func.fresh_block();
    let default = func.fresh_block();
    let used = func.fresh_value();
    let dead = func.fresh_value();
    for value in [used, dead] {
        add_type(&mut func, value, TirType::DynBox);
    }
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(1, resume, vec![used, dead])],
        default,
        default_args: vec![dead],
    };
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![used] },
        },
    );
    func.blocks.insert(
        default,
        TirBlock {
            id: default,
            args: vec![arg(dead, TirType::DynBox)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 4);
    let Terminator::StateDispatch {
        cases,
        default_args,
        ..
    } = &func.blocks[&entry].terminator
    else {
        panic!("expected state dispatch");
    };
    assert_eq!(cases[0].2, vec![used]);
    assert!(default_args.is_empty());
    assert_eq!(func.blocks[&resume].args.len(), 1);
    assert!(func.blocks[&default].args.is_empty());
}

#[test]
fn never_prunes_entry_parameters() {
    let mut func = TirFunction::new(
        "entry_params".into(),
        vec![TirType::I64, TirType::I64],
        TirType::None,
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks[&func.entry_block].args.len(), 2);
}

#[test]
fn fixed_point_prunes_forwarded_dead_arg_chain() {
    let mut func = TirFunction::new("chain_prune".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let middle = func.fresh_block();
    let exit = func.fresh_block();
    let forwarded = func.fresh_value();
    add_type(&mut func, forwarded, TirType::DynBox);
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: middle,
        args: vec![forwarded],
    };
    func.blocks.insert(
        middle,
        TirBlock {
            id: middle,
            args: vec![arg(forwarded, TirType::DynBox)],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![forwarded],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![arg(forwarded, TirType::DynBox)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 4);
    assert!(func.blocks[&middle].args.is_empty());
    assert!(func.blocks[&exit].args.is_empty());
    let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
        panic!("expected branch");
    };
    assert!(args.is_empty());
    let Terminator::Branch { args, .. } = &func.blocks[&middle].terminator else {
        panic!("expected branch");
    };
    assert!(args.is_empty());
}

#[test]
fn keeps_arg_used_only_in_dominated_descendant() {
    let mut func = TirFunction::new("descendant_use".into(), vec![], TirType::None);
    let entry = func.entry_block;
    let carrier = func.fresh_block();
    let use_block = func.fresh_block();
    let src = func.fresh_value();
    let phi = func.fresh_value();
    let out = func.fresh_value();
    for value in [src, phi, out] {
        add_type(&mut func, value, TirType::DynBox);
    }

    func.blocks.get_mut(&entry).unwrap().ops = vec![copy(vec![], vec![src])];
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: carrier,
        args: vec![src],
    };
    func.blocks.insert(
        carrier,
        TirBlock {
            id: carrier,
            args: vec![arg(phi, TirType::DynBox)],
            ops: vec![],
            terminator: Terminator::Branch {
                target: use_block,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        use_block,
        TirBlock {
            id: use_block,
            args: vec![],
            ops: vec![copy(vec![phi], vec![out])],
            terminator: Terminator::Return { values: vec![out] },
        },
    );

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks[&carrier].args.len(), 1);
    let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
        panic!("expected entry branch");
    };
    assert_eq!(args, &vec![src]);
}
