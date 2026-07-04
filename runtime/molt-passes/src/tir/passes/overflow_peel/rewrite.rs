use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_is_overflow_peel_body_pure_table, opcode_is_overflow_peel_guard_compare_table,
};
use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::Refusal;

/// One qualifying accumulator phi and its latch update. Plans are built in
/// header-arg order, so the vector index doubles as the arg index.
struct PhiPlan {
    /// The phi value itself.
    phi: ValueId,
    /// The init value passed by the preheader (chases to ConstInt).
    init_arg: ValueId,
    /// Index in the body ops of the pure arithmetic op (`Add` or `Mul`) that
    /// updates this phi.
    update_op_index: usize,
    /// The original update opcode (`Add` or `Mul`). The fast-loop swap maps it
    /// to the matching checked op (`CheckedAdd`/`CheckedMul`); the slow loop -
    /// cloned from the body BEFORE the swap - keeps this plain opcode, so its
    /// boxed `molt_add`/`molt_mul` path stays BigInt-exact on re-execution.
    update_opcode: OpCode,
}

/// Blocks reachable from the function entry via terminator edges.
fn reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let mut seen = HashSet::new();
    let mut work = vec![func.entry_block];
    while let Some(bid) = work.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        match &block.terminator {
            Terminator::Branch { target, .. } => work.push(*target),
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                work.push(*then_block);
                work.push(*else_block);
            }
            Terminator::Switch { cases, default, .. }
            | Terminator::StateDispatch { cases, default, .. } => {
                work.extend(cases.iter().map(|(_, b, _)| *b));
                work.push(*default);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
    seen
}

/// Chase a value backward through `Copy` results to its origin within `ops`
/// (a map from result id to the defining op). Marker copies (`store_var`
/// round-trips) are 2-operand same-value Copies; both shapes chase through
/// `operands[0]`.
fn chase_copies(start: ValueId, def_by_result: &HashMap<ValueId, &TirOp>) -> ValueId {
    let mut cur = start;
    let mut fuel = 64; // structural bound; copy chains are short
    while fuel > 0 {
        fuel -= 1;
        match def_by_result.get(&cur) {
            Some(op) if op.opcode == OpCode::Copy && !op.operands.is_empty() => {
                cur = op.operands[0];
            }
            _ => break,
        }
    }
    cur
}

/// Attempt to peel the loop rooted at `header`. Returns the number of ops
/// added on success.
pub(super) fn try_peel_loop(func: &mut TirFunction, header: BlockId) -> Result<usize, Refusal> {
    let reachable = reachable_blocks(func);

    // -- Shape: header(args) --Branch--> guard {..., cmp, CondBranch(body, exit)} --
    let header_block = func.blocks.get(&header).ok_or(Refusal::NoCanonicalGuard)?;
    if header_block.args.is_empty() || !header_block.ops.iter().all(is_ignorable_marker) {
        return Err(Refusal::NoCanonicalGuard);
    }
    let guard = match &header_block.terminator {
        Terminator::Branch { target, args } if args.is_empty() => *target,
        _ => return Err(Refusal::NoCanonicalGuard),
    };
    let phis: Vec<TirValue> = header_block.args.clone();

    let guard_block = func.blocks.get(&guard).ok_or(Refusal::NoCanonicalGuard)?;
    if !guard_block.args.is_empty() {
        return Err(Refusal::NoCanonicalGuard);
    }
    let (cond, body, body_args, exit, exit_args) = match &guard_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => (
            *cond,
            *then_block,
            then_args.clone(),
            *else_block,
            else_args.clone(),
        ),
        _ => return Err(Refusal::NoCanonicalGuard),
    };
    if !body_args.is_empty() || !exit_args.is_empty() {
        return Err(Refusal::GuardExitArgs);
    }

    // Guard ops: ignorable markers + Copies + ONE compare producing `cond`.
    let mut guard_compare: Option<&TirOp> = None;
    for op in &guard_block.ops {
        if op.opcode == OpCode::Copy {
            continue;
        }
        if opcode_is_overflow_peel_guard_compare_table(op.opcode)
            && op.results.first() == Some(&cond)
        {
            if guard_compare.is_some() {
                return Err(Refusal::ImpureBody);
            }
            guard_compare = Some(op);
        } else {
            return Err(Refusal::ImpureBody);
        }
    }
    if guard_compare.is_none() {
        return Err(Refusal::NoCanonicalGuard);
    }

    // -- Body: a single linear block latching straight back to the header --
    let body_block = func.blocks.get(&body).ok_or(Refusal::MultiBlockBody)?;
    if !body_block.args.is_empty() {
        return Err(Refusal::MultiBlockBody);
    }
    let latch_args = match &body_block.terminator {
        Terminator::Branch { target, args } if *target == header => args.clone(),
        _ => return Err(Refusal::MultiBlockBody),
    };
    if latch_args.len() != phis.len() {
        return Err(Refusal::MultiBlockBody);
    }
    // Body purity: Copies (incl. markers), `Add`/`Mul`s, and constant
    // materialisation only (the frontend leaves un-hoisted literal steps -
    // e.g. `total + -20000000` - as in-body ConstInts; they are pure).
    // Anything that can call, store, load, raise, or observe runtime state
    // disqualifies - re-execution of the failed iteration on the slow path
    // must be observationally identical. `Mul` is pure exactly like `Add`
    // (no side effect, deterministic), so a multiply accumulator
    // (`prod = prod * i`) re-executes BigInt-exact on the boxed slow loop.
    for op in &body_block.ops {
        if !opcode_is_overflow_peel_body_pure_table(op.opcode) {
            return Err(Refusal::ImpureBody);
        }
    }

    let loop_blocks: HashSet<BlockId> = [header, guard, body].into_iter().collect();

    // -- Header predecessors: one reachable preheader + the latch; any
    //    unreachable extras (the vestigial loop-else) get their edge args
    //    retargeted to the preheader's init args later. --
    let mut preheader: Option<BlockId> = None;
    let mut stray_preds: Vec<BlockId> = Vec::new();
    for (bid, block) in &func.blocks {
        if *bid == body {
            continue;
        }
        let targets_header = match &block.terminator {
            Terminator::Branch { target, .. } => *target == header,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == header || *else_block == header,
            Terminator::Switch { cases, default, .. } => {
                cases.iter().any(|(_, b, _)| *b == header) || *default == header
            }
            _ => false,
        };
        if !targets_header {
            continue;
        }
        if reachable.contains(bid) {
            if preheader.is_some() {
                return Err(Refusal::MultiplePreheaders);
            }
            preheader = Some(*bid);
        } else {
            stray_preds.push(*bid);
        }
    }
    let preheader = preheader.ok_or(Refusal::MultiplePreheaders)?;
    let init_args = match &func.blocks[&preheader].terminator {
        Terminator::Branch { target, args } if *target == header => args.clone(),
        // v1 keeps the preheader shape strict: a guarded preheader would need
        // per-edge arg extension on the right arm only.
        _ => return Err(Refusal::NonBranchPreheader),
    };
    if init_args.len() != phis.len() {
        return Err(Refusal::NonBranchPreheader);
    }

    // -- Exit: single predecessor (the guard), so post-loop uses of the phis
    //    can be rerouted through fresh exit args fed by both loops. --
    for (bid, block) in &func.blocks {
        if *bid == guard || !reachable.contains(bid) {
            continue;
        }
        let targets_exit = match &block.terminator {
            Terminator::Branch { target, .. } => *target == exit,
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => *then_block == exit || *else_block == exit,
            Terminator::Switch { cases, default, .. } => {
                cases.iter().any(|(_, b, _)| *b == exit) || *default == exit
            }
            _ => false,
        };
        if targets_exit {
            return Err(Refusal::ExitHasOtherPreds);
        }
    }

    // -- Qualify every phi: I64-typed, ConstInt init, latch update that is
    //    either a recognised Add/Mul accumulator or rejected. ALL phis must
    //    qualify (all-or-nothing: a single boxed phi would re-box the loop). --
    let def_by_result: HashMap<ValueId, &TirOp> = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .flat_map(|op| op.results.iter().map(move |r| (*r, op)))
        .collect();
    let phi_ids: HashSet<ValueId> = phis.iter().map(|v| v.id).collect();

    let body_block = &func.blocks[&body];
    let body_defs: HashMap<ValueId, usize> = body_block
        .ops
        .iter()
        .enumerate()
        .flat_map(|(i, op)| op.results.iter().map(move |r| (*r, i)))
        .collect();

    let mut plans: Vec<PhiPlan> = Vec::new();
    for (arg_index, phi) in phis.iter().enumerate() {
        if !matches!(func.value_types.get(&phi.id), Some(TirType::I64)) {
            return Err(Refusal::NonIntPhi);
        }
        let init_arg = init_args[arg_index];
        if !matches!(
            def_by_result.get(&chase_copies(init_arg, &def_by_result)),
            Some(op) if op.opcode == OpCode::ConstInt
        ) {
            return Err(Refusal::NonConstInit);
        }
        let update = chase_copies(latch_args[arg_index], &def_by_result);
        let Some(&update_op_index) = body_defs.get(&update) else {
            return Err(Refusal::NonArithmeticUpdate);
        };
        let add_op = &body_block.ops[update_op_index];
        // The update must be a binary I64 `Add` or `Mul`. Both have a total
        // hardware-overflow-flagged checked form (`CheckedAdd`/`CheckedMul`)
        // and are pure, so the dual-loop transform is sound for either.
        if !matches!(add_op.opcode, OpCode::Add | OpCode::Mul)
            || add_op.operands.len() != 2
            || add_op.results.len() != 1
            || !matches!(func.value_types.get(&add_op.results[0]), Some(TirType::I64))
        {
            return Err(Refusal::NonArithmeticUpdate);
        }
        // Each operand must chase to a header phi, a loop-invariant
        // value (defined outside the loop blocks), or an in-body ConstInt
        // (a literal step the frontend left un-hoisted - constant, so
        // trivially invariant).
        for &operand in &add_op.operands {
            let origin = chase_copies(operand, &def_by_result);
            let in_body_const = body_defs
                .get(&origin)
                .is_some_and(|&i| body_block.ops[i].opcode == OpCode::ConstInt);
            let invariant = in_body_const
                || (!body_defs.contains_key(&origin)
                    && !phi_ids.contains(&origin)
                    && !func.blocks[&guard]
                        .ops
                        .iter()
                        .any(|op| op.results.contains(&origin)));
            if !phi_ids.contains(&origin) && !invariant {
                return Err(Refusal::NonArithmeticUpdate);
            }
        }
        plans.push(PhiPlan {
            phi: phi.id,
            init_arg,
            update_op_index,
            update_opcode: add_op.opcode,
        });
    }
    if plans.is_empty() {
        return Err(Refusal::NonArithmeticUpdate);
    }
    // Two phis updated by the same arithmetic op (aliased accumulators) would
    // make the checked-op swap ambiguous.
    {
        let mut seen = HashSet::new();
        for p in &plans {
            if !seen.insert(p.update_op_index) {
                return Err(Refusal::NonArithmeticUpdate);
            }
        }
    }

    // -- Live-out audit: nothing defined inside the loop may be used outside,
    //    except the header phis (which are rerouted through exit args). --
    let mut loop_defined: HashSet<ValueId> = phi_ids.clone();
    for bid in &loop_blocks {
        for op in &func.blocks[bid].ops {
            loop_defined.extend(op.results.iter().copied());
        }
    }
    let mut exit_live_phis: Vec<ValueId> = Vec::new();
    for (bid, block) in &func.blocks {
        if loop_blocks.contains(bid) {
            continue;
        }
        let mut check_use = |v: ValueId| -> Result<(), Refusal> {
            if phi_ids.contains(&v) {
                if !exit_live_phis.contains(&v) {
                    exit_live_phis.push(v);
                }
                Ok(())
            } else if loop_defined.contains(&v) {
                Err(Refusal::InteriorLiveOut)
            } else {
                Ok(())
            }
        };
        for op in &block.ops {
            for &v in &op.operands {
                check_use(v)?;
            }
        }
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for &v in args {
                    check_use(v)?;
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                check_use(*cond)?;
                for &v in then_args.iter().chain(else_args.iter()) {
                    check_use(v)?;
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                check_use(*value)?;
                for (_, _, args) in cases {
                    for &v in args {
                        check_use(v)?;
                    }
                }
                for &v in default_args {
                    check_use(v)?;
                }
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for &v in args {
                        check_use(v)?;
                    }
                }
                for &v in default_args {
                    check_use(v)?;
                }
            }
            Terminator::Return { values } => {
                for &v in values {
                    check_use(v)?;
                }
            }
            Terminator::Unreachable => {}
        }
    }

    // ======================== TRANSFORM (infallible from here) =======================

    let mut ops_added = 0usize;

    // 1. Clone {header, guard, body} verbatim -> the slow (boxed) loop. The
    //    clone happens FIRST, from the pristine blocks, so the slow loop
    //    keeps plain `Add`s (the boxed BigInt-exact path).
    let slow_header = func.fresh_block();
    let slow_guard = func.fresh_block();
    let slow_body = func.fresh_block();
    let block_map: HashMap<BlockId, BlockId> = [
        (header, slow_header),
        (guard, slow_guard),
        (body, slow_body),
    ]
    .into_iter()
    .collect();

    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
    let mut new_value_types: Vec<(ValueId, TirType)> = Vec::new();
    {
        let mut remap = |old: ValueId,
                         func_next: &mut u32,
                         value_types: &HashMap<ValueId, TirType>|
         -> ValueId {
            let fresh = ValueId(*func_next);
            *func_next += 1;
            value_map.insert(old, fresh);
            if let Some(ty) = value_types.get(&old) {
                new_value_types.push((fresh, ty.clone()));
            }
            fresh
        };
        // Pre-allocate fresh ids for every value DEFINED inside the loop
        // (args + op results); operands defined outside remap to themselves.
        let mut next = func.next_value;
        for bid in [header, guard, body] {
            let block = &func.blocks[&bid];
            for arg in &block.args {
                remap(arg.id, &mut next, &func.value_types);
            }
            for op in &block.ops {
                for &r in &op.results {
                    remap(r, &mut next, &func.value_types);
                }
            }
        }
        func.next_value = next;
    }
    for (id, ty) in &new_value_types {
        func.value_types.insert(*id, ty.clone());
    }

    let map_value = |v: ValueId, value_map: &HashMap<ValueId, ValueId>| -> ValueId {
        value_map.get(&v).copied().unwrap_or(v)
    };
    let clone_block = |src: &TirBlock,
                       new_id: BlockId,
                       value_map: &HashMap<ValueId, ValueId>,
                       exit: BlockId,
                       exit_arg: Option<ValueId>|
     -> TirBlock {
        let args = src
            .args
            .iter()
            .map(|a| TirValue {
                id: map_value(a.id, value_map),
                ty: a.ty.clone(),
            })
            .collect();
        let ops = src
            .ops
            .iter()
            .map(|op| TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: op
                    .operands
                    .iter()
                    .map(|&v| map_value(v, value_map))
                    .collect(),
                results: op
                    .results
                    .iter()
                    .map(|&v| map_value(v, value_map))
                    .collect(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
            })
            .collect();
        let terminator = match &src.terminator {
            Terminator::Branch { target, args } => Terminator::Branch {
                target: *block_map.get(target).unwrap_or(target),
                args: args.iter().map(|&v| map_value(v, value_map)).collect(),
            },
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let mapped_else = *block_map.get(else_block).unwrap_or(else_block);
                let mut mapped_else_args: Vec<ValueId> =
                    else_args.iter().map(|&v| map_value(v, value_map)).collect();
                // The slow guard's exit edge feeds the (new) exit arg with the
                // slow accumulator phis.
                if mapped_else == exit
                    && let Some(arg) = exit_arg
                {
                    mapped_else_args.push(arg);
                }
                Terminator::CondBranch {
                    cond: map_value(*cond, value_map),
                    then_block: *block_map.get(then_block).unwrap_or(then_block),
                    then_args: then_args.iter().map(|&v| map_value(v, value_map)).collect(),
                    else_block: mapped_else,
                    else_args: mapped_else_args,
                }
            }
            other => other.clone(),
        };
        TirBlock {
            id: new_id,
            args,
            ops,
            terminator,
        }
    };

    // The exit args (one per exit-live phi, in `exit_live_phis` order) - the
    // slow guard passes its remapped phi values.
    let slow_exit_args: Vec<ValueId> = exit_live_phis
        .iter()
        .map(|&phi| map_value(phi, &value_map))
        .collect();

    let slow_header_block = {
        let src = &func.blocks[&header];
        clone_block(src, slow_header, &value_map, exit, None)
    };
    let slow_guard_block = {
        let src = &func.blocks[&guard];
        // Single exit-live phi is the common case; general case appends all.
        let mut blk = clone_block(src, slow_guard, &value_map, exit, None);
        if let Terminator::CondBranch {
            else_block,
            else_args,
            ..
        } = &mut blk.terminator
            && *else_block == exit
        {
            else_args.extend(slow_exit_args.iter().copied());
        }
        blk
    };
    let slow_body_block = {
        let src = &func.blocks[&body];
        clone_block(src, slow_body, &value_map, exit, None)
    };
    ops_added +=
        slow_header_block.ops.len() + slow_guard_block.ops.len() + slow_body_block.ops.len();
    func.blocks.insert(slow_header, slow_header_block);
    func.blocks.insert(slow_guard, slow_guard_block);
    func.blocks.insert(slow_body, slow_body_block);
    // Deliberately NO loop_roles/loop_pairs/loop_cond_blocks for the clones:
    // the cold loop linearises through the generic label/jump path.

    // 2. Extend the fast header with the new loop-carried phis:
    //    of (Bool), then prev_<phi> (I64) for every plan, in plan order.
    let of_phi = func.fresh_value();
    func.value_types.insert(of_phi, TirType::Bool);
    let prev_phis: Vec<ValueId> = plans
        .iter()
        .map(|_| {
            let v = func.fresh_value();
            func.value_types.insert(v, TirType::I64);
            v
        })
        .collect();
    {
        let header_block = func.blocks.get_mut(&header).expect("header exists");
        header_block.args.push(TirValue {
            id: of_phi,
            ty: TirType::Bool,
        });
        for &pv in &prev_phis {
            header_block.args.push(TirValue {
                id: pv,
                ty: TirType::I64,
            });
        }
    }

    // 3. Preheader: materialise `false` and extend the init edge args:
    //    [..init, false, init(plan_0), init(plan_1), ...].
    let false_const = func.fresh_value();
    func.value_types.insert(false_const, TirType::Bool);
    {
        let pre = func.blocks.get_mut(&preheader).expect("preheader exists");
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), crate::tir::ops::AttrValue::Bool(false));
        pre.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![false_const],
            attrs,
            source_span: None,
        });
        ops_added += 1;
        if let Terminator::Branch { args, .. } = &mut pre.terminator {
            args.push(false_const);
            for plan in &plans {
                args.push(plan.init_arg);
            }
        }
    }

    // 4. Stray (unreachable) preds: retarget their header-edge args to the
    //    preheader's init shape so no `None` ever appears as a phi incoming.
    let stray_args: Vec<ValueId> = {
        let mut v: Vec<ValueId> = plans.iter().map(|p| p.init_arg).collect();
        // Original arity might exceed the planned phis only if plans != args;
        // plans cover every header arg by construction (all-or-nothing).
        v.push(false_const);
        for plan in &plans {
            v.push(plan.init_arg);
        }
        v
    };
    debug_assert_eq!(stray_args.len(), phis.len() + 1 + plans.len());
    for stray in &stray_preds {
        let block = func.blocks.get_mut(stray).expect("stray pred exists");
        let retarget = |args: &mut Vec<ValueId>| {
            args.clear();
            args.extend(stray_args.iter().copied());
        };
        match &mut block.terminator {
            Terminator::Branch { target, args } if *target == header => retarget(args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == header {
                    retarget(then_args);
                }
                if *else_block == header {
                    retarget(else_args);
                }
            }
            _ => {}
        }
    }

    // 5. Guard: brk = And(cond, Not(of)); retarget the exit edge to the
    //    dispatch block.
    let dispatch = func.fresh_block();
    let not_of = func.fresh_value();
    let brk = func.fresh_value();
    func.value_types.insert(not_of, TirType::Bool);
    func.value_types.insert(brk, TirType::Bool);
    {
        let guard_block = func.blocks.get_mut(&guard).expect("guard exists");
        guard_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Not,
            operands: vec![of_phi],
            results: vec![not_of],
            attrs: AttrDict::new(),
            source_span: None,
        });
        guard_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::And,
            operands: vec![cond, not_of],
            results: vec![brk],
            attrs: AttrDict::new(),
            source_span: None,
        });
        ops_added += 2;
        if let Terminator::CondBranch {
            cond: c,
            else_block,
            ..
        } = &mut guard_block.terminator
        {
            *c = brk;
            *else_block = dispatch;
        }
    }

    // 6. Body: swap each plan's Add -> CheckedAdd / Mul -> CheckedMul (keeping
    //    the original result ValueId so the latch args stay valid), fan the
    //    flags in with Or, snapshot the pre-iteration phi values, and extend
    //    the latch args. The matching checked op is chosen from the recorded
    //    update opcode; the slow loop was cloned BEFORE this swap, so it keeps
    //    the plain `Add`/`Mul` (BigInt-exact on re-execution).
    let mut flag_values: Vec<ValueId> = Vec::new();
    {
        let body_block = func.blocks.get_mut(&body).expect("body exists");
        for plan in &plans {
            let flag = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(flag, TirType::Bool);
            let add_op = &mut body_block.ops[plan.update_op_index];
            add_op.opcode = match plan.update_opcode {
                OpCode::Add => OpCode::CheckedAdd,
                OpCode::Mul => OpCode::CheckedMul,
                // Unreachable: phi-qual admits only Add/Mul updates.
                other => unreachable!(
                    "overflow_peel: unexpected update opcode {other:?} (phi-qual \
                     admits only Add/Mul)"
                ),
            };
            add_op.results.push(flag);
            flag_values.push(flag);
        }
        // of' = Or(flag_0, flag_1, ...) - left fold.
        let mut of_next = flag_values[0];
        for &f in &flag_values[1..] {
            let folded = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(folded, TirType::Bool);
            body_block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Or,
                operands: vec![of_next, f],
                results: vec![folded],
                attrs: AttrDict::new(),
                source_span: None,
            });
            ops_added += 1;
            of_next = folded;
        }
        // prev_k = Copy(phi_k) - the pre-iteration snapshot that seeds the
        // slow loop's re-execution of the failed iteration.
        let mut prev_next: Vec<ValueId> = Vec::new();
        for plan in &plans {
            let snap = ValueId(func.next_value);
            func.next_value += 1;
            func.value_types.insert(snap, TirType::I64);
            body_block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![plan.phi],
                results: vec![snap],
                attrs: AttrDict::new(),
                source_span: None,
            });
            ops_added += 1;
            prev_next.push(snap);
        }
        if let Terminator::Branch { args, .. } = &mut body_block.terminator {
            args.push(of_next);
            args.extend(prev_next.iter().copied());
        }
    }

    // 7. Exit args: one fresh arg per exit-live phi; rewrite every use of
    //    those phis OUTSIDE the loop to the corresponding exit arg.
    let exit_arg_ids: Vec<ValueId> = exit_live_phis
        .iter()
        .map(|&phi| {
            let v = func.fresh_value();
            if let Some(ty) = func.value_types.get(&phi).cloned() {
                func.value_types.insert(v, ty);
            }
            v
        })
        .collect();
    {
        let exit_block = func.blocks.get_mut(&exit).expect("exit exists");
        for (i, &arg) in exit_arg_ids.iter().enumerate() {
            exit_block.args.push(TirValue {
                id: arg,
                ty: func
                    .value_types
                    .get(&exit_live_phis[i])
                    .cloned()
                    .unwrap_or(TirType::I64),
            });
        }
    }
    let phi_to_exit_arg: HashMap<ValueId, ValueId> = exit_live_phis
        .iter()
        .copied()
        .zip(exit_arg_ids.iter().copied())
        .collect();
    let rewrite_blocks: Vec<BlockId> = func
        .blocks
        .keys()
        .filter(|b| {
            !loop_blocks.contains(b)
                && **b != slow_header
                && **b != slow_guard
                && **b != slow_body
                && **b != dispatch
        })
        .copied()
        .collect();
    for bid in rewrite_blocks {
        let block = func.blocks.get_mut(&bid).expect("block exists");
        let rw = |v: &mut ValueId| {
            if let Some(&replacement) = phi_to_exit_arg.get(v) {
                *v = replacement;
            }
        };
        for op in &mut block.ops {
            for v in &mut op.operands {
                rw(v);
            }
        }
        match &mut block.terminator {
            Terminator::Branch { args, .. } => args.iter_mut().for_each(rw),
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                rw(cond);
                then_args.iter_mut().for_each(rw);
                else_args.iter_mut().for_each(rw);
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                rw(value);
                for (_, _, args) in cases {
                    args.iter_mut().for_each(rw);
                }
                default_args.iter_mut().for_each(rw);
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    args.iter_mut().for_each(rw);
                }
                default_args.iter_mut().for_each(rw);
            }
            Terminator::Return { values } => values.iter_mut().for_each(rw),
            Terminator::Unreachable => {}
        }
    }

    // 8. Dispatch: CondBranch(of, slow_entry, exit(fast phi values)). The
    //    fast path passes the (exact, non-overflowed) phis straight to the
    //    exit args; the boxing happens at the existing escape discipline
    //    (store into a non-raw slot / return) - no explicit BoxVal needed.
    let slow_entry = func.fresh_block();
    let fast_exit_args: Vec<ValueId> = exit_live_phis.clone();
    func.blocks.insert(
        dispatch,
        TirBlock {
            id: dispatch,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: of_phi,
                then_block: slow_entry,
                then_args: vec![],
                else_block: exit,
                else_args: fast_exit_args,
            },
        },
    );
    // slow_entry: -> slow_header(prev_0, prev_1, ...) - the pre-iteration
    // snapshot seeds the boxed re-execution of the failed iteration.
    func.blocks.insert(
        slow_entry,
        TirBlock {
            id: slow_entry,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: slow_header,
                args: prev_phis.clone(),
            },
        },
    );

    Ok(ops_added)
}

/// Header blocks may carry zero-result marker Copies (line markers). Any op
/// with results disqualifies the canonical empty-header shape.
fn is_ignorable_marker(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && op.results.is_empty()
}
