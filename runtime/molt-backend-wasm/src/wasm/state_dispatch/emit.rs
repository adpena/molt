use self::ops::{DispatchOpScratch, emit_dispatch_op};
use super::super::frame_locals::WasmDispatchFrameLocals;
use super::super::op_loop::WasmFunctionEmitContext;
use super::DispatchMode;
use super::block_layout::{RootSelection, emit_dispatch_block_lookup, emit_scope_node_selector};
use super::common::{
    emit_dispatch_trailing_return, emit_stateful_resume_prelude, emit_static_dispatch_edge,
};
use super::plan::NonLinearControlPlan;
use wasm_encoder::{BlockType, Function, Instruction};

mod ops;

pub(in crate::wasm) fn emit_non_linear_control(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    route_local: Option<u32>,
) {
    assert!(
        !plan.block_starts.is_empty(),
        "non-linear control requires a physical block"
    );
    let mode = if plan.state_resume.is_some() {
        DispatchMode::Stateful
    } else {
        DispatchMode::Jumpful
    };
    match mode {
        DispatchMode::Stateful => emit_stateful_resume_prelude(
            func,
            op_emitter,
            plan,
            locals.expect("stateful control requires dispatch locals"),
        ),
        DispatchMode::Jumpful => {
            if let Some(locals) = locals {
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::LocalSet(locals.state_local));
            }
        }
    }
    if plan.needs_dispatch() {
        func.instruction(&Instruction::Loop(BlockType::Empty));
    }
    let mut emitter = ScopeEmitter {
        func,
        op_emitter,
        plan,
        locals,
        route_local,
        mode,
        scratch: DispatchOpScratch::default(),
    };
    emitter.emit_node(plan.scopes.levels(), 0);
    emit_dispatch_trailing_return(func, op_emitter, plan.needs_dispatch());
}

struct ScopeEmitter<'f, 'e, 'a, 'b> {
    func: &'f mut Function,
    op_emitter: &'e mut WasmFunctionEmitContext<'a, 'b>,
    plan: &'f NonLinearControlPlan,
    locals: Option<WasmDispatchFrameLocals>,
    route_local: Option<u32>,
    mode: DispatchMode,
    scratch: DispatchOpScratch,
}

impl ScopeEmitter<'_, '_, '_, '_> {
    /// Emit one scope node: one `block` per child scope, the optional child
    /// selector inside the innermost one, then each child after the `end` of
    /// its own scope. Level 1 children are physical blocks.
    fn emit_node(&mut self, level: u32, node: usize) {
        let scopes = self.plan.scopes;
        let children = scopes.child_count(level, node);
        for _ in 0..children {
            self.func.instruction(&Instruction::Block(BlockType::Empty));
        }
        if self.plan.node_selects(level, node) {
            if level == scopes.levels() {
                // The invocation and every backward transfer enter here.
                let selection = match self.route_local {
                    None => RootSelection::Ordinals {
                        targets: &self.plan.dispatch_targets,
                    },
                    Some(route_local) => RootSelection::Routed {
                        route_local,
                        digit_shift: scopes.digit_shift(level),
                        children,
                    },
                };
                emit_dispatch_block_lookup(
                    self.func,
                    self.op_emitter.func_ir.ops.len(),
                    selection,
                    self.locals
                        .expect("dynamic control requires dispatch locals"),
                );
            } else {
                emit_scope_node_selector(
                    self.func,
                    self.route_local
                        .expect("scope node selection requires the routing local"),
                    scopes.digit_shift(level),
                    scopes.fanout() - 1,
                    children,
                );
            }
        }
        // Without a selector the innermost scope ends at once: the node is
        // entered only at its first child.
        self.func.instruction(&Instruction::End);
        for child in 0..children {
            if level == 1 {
                self.emit_block(scopes.node_start(level, node) + child);
            } else {
                self.emit_node(level - 1, scopes.child_node(node, child));
            }
            if child + 1 < children {
                self.func.instruction(&Instruction::End);
            }
        }
    }

    fn emit_block(&mut self, block_idx: usize) {
        let plan = self.plan;
        let op_count = self.op_emitter.func_ir.ops.len();
        let start = plan.block_starts[block_idx];
        let end = plan
            .block_starts
            .get(block_idx + 1)
            .copied()
            .unwrap_or(op_count);
        let depth = plan.scope_depth(block_idx, self.route_local);
        let mut block_terminated = false;
        for idx in start..end {
            if !plan.operation_reachable(idx) {
                // Canonical entry reachability excludes dead ordinary Jumpful
                // edges. Stateful's admitted external state roots are broader.
                self.func.instruction(&Instruction::Unreachable);
                block_terminated = true;
                break;
            }
            let op = &self.op_emitter.func_ir.ops[idx];
            block_terminated = emit_dispatch_op(
                self.func,
                self.op_emitter,
                plan,
                self.locals,
                self.mode,
                op,
                idx,
                depth,
                &mut self.scratch,
            );
            if block_terminated {
                break;
            }
        }
        if !block_terminated {
            let source = end - 1;
            if let Some(target) = plan.fallthrough_successor(source) {
                emit_static_dispatch_edge(
                    self.func,
                    self.op_emitter,
                    plan,
                    self.locals,
                    source,
                    target,
                    depth,
                    0,
                );
            } else {
                self.func.instruction(&Instruction::Unreachable);
            }
        }
    }
}
