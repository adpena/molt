use super::*;
use molt_ir::simple_verify::{EdgeRole, SimpleIrLogicalFlow};
use molt_ir::tir::op_kinds_generated::{
    simpleir_kind_is_exception_check, simpleir_kind_is_return_terminator,
    simpleir_kind_is_structural, simpleir_kind_is_suspend, simpleir_kind_is_wasm_stateful_dispatch,
};

impl LuauBackend {
    /// Project the canonical logical graph into Luau, whose language has no
    /// labelled jumps. Exception and ordinary edges use the same dispatcher;
    /// no source-order handler pairing or textual goto surgery participates.
    pub(super) fn emit_logical_flow(
        &mut self,
        ops: &[OpIR],
        flow: &SimpleIrLogicalFlow,
        params: &[String],
    ) {
        if flow.blocks.is_empty() {
            return;
        }
        self.emit_line("local __molt_block = 0");
        self.emit_line("while true do");
        self.push_indent();
        let mut capture_counter = 0;
        let observes_pending = ir_rewrites::observes_pending_exception(ops);
        for (block, &(start, end)) in flow.blocks.iter().enumerate() {
            if block > 0 {
                self.pop_indent();
            }
            self.emit_line(&format!(
                "{} __molt_block == {block} then",
                if block == 0 { "if" } else { "elseif" }
            ));
            self.push_indent();
            let mut local_values = BTreeSet::new();
            for op in &ops[start..=end] {
                molt_ir::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
                    let ident = sanitize_ident(name);
                    if name != "none"
                        && !params.iter().any(|param| param == name)
                        && !self.hoisted_vars.contains(&ident)
                    {
                        local_values.insert(ident);
                    }
                });
            }
            for value in local_values {
                self.emit_line(&format!("local {value}"));
            }
            let captured = ir_rewrites::lower_exception_captures_with_counter(
                &ops[start..=end],
                observes_pending,
                &mut capture_counter,
            );
            for op in &captured {
                if simpleir_kind_is_wasm_stateful_dispatch(&op.kind)
                    || simpleir_kind_is_suspend(&op.kind)
                {
                    self.emit_unsupported_op_with_reason(op,
                        "logical labelled-flow emission requires synchronous edges; coroutine state-machine lowering remains target-gated");
                } else if simpleir_kind_is_return_terminator(&op.kind)
                    || (!simpleir_kind_is_structural(&op.kind)
                        && !simpleir_kind_is_exception_check(&op.kind))
                {
                    self.emit_op(op);
                }
            }
            let tail = &ops[end];
            if simpleir_kind_is_return_terminator(&tail.kind) {
                continue;
            }
            let edges = &flow.edges[end];
            let target = |role| {
                edges
                    .iter()
                    .find(|edge| edge.role == role)
                    .map(|edge| flow.op_to_block[edge.target])
            };
            if edges
                .iter()
                .any(|edge| matches!(edge.role, EdgeRole::Exception | EdgeRole::Normal))
            {
                self.emit_dispatch_branch(
                    "molt_exception_pending()",
                    target(EdgeRole::Exception),
                    target(EdgeRole::Normal),
                );
            } else if edges
                .iter()
                .any(|edge| matches!(edge.role, EdgeRole::BranchTrue | EdgeRole::BranchFalse))
            {
                let condition = if tail.kind == "loop_break_if_exception" {
                    "molt_exception_pending()".to_string()
                } else if let Some(value) = tail.args.as_deref().and_then(|args| args.first()) {
                    self.guard_truthiness(value)
                } else {
                    self.emit_unsupported_op_with_reason(
                        tail,
                        "logical conditional edge has no condition",
                    );
                    "false".to_string()
                };
                self.emit_dispatch_branch(
                    &condition,
                    target(EdgeRole::BranchTrue),
                    target(EdgeRole::BranchFalse),
                );
            } else if let Some(next) = edges.first().map(|edge| flow.op_to_block[edge.target]) {
                self.emit_line(&format!("__molt_block = {next}"));
            } else {
                self.emit_line("molt_exception_propagate()");
                self.emit_line("return");
            }
        }
        self.pop_indent();
        self.emit_line("end");
        self.pop_indent();
        self.emit_line("end");
    }

    fn emit_dispatch_branch(
        &mut self,
        condition: &str,
        when_true: Option<usize>,
        when_false: Option<usize>,
    ) {
        self.emit_line(&format!("if {condition} then"));
        self.push_indent();
        self.emit_dispatch_target(when_true);
        self.pop_indent();
        self.emit_line("else");
        self.push_indent();
        self.emit_dispatch_target(when_false);
        self.pop_indent();
        self.emit_line("end");
    }

    fn emit_dispatch_target(&mut self, target: Option<usize>) {
        if let Some(target) = target {
            self.emit_line(&format!("__molt_block = {target}"));
        } else {
            self.emit_line("molt_exception_propagate()");
            self.emit_line("return");
        }
    }
}
