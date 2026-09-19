//! Rust projection of the admitted, canonical SimpleIR executable graph.

use super::RustBackend;
use super::emit_helpers::rust_value;
use super::lowering::op_definition_vars;
use crate::OpIR;
use molt_ir::simple_verify::{EdgeRole, SimpleIrLogicalFlow};
use molt_ir::tir::op_kinds_generated::{
    simpleir_kind_is_exception_check, simpleir_kind_is_return_terminator,
    simpleir_kind_is_structural, simpleir_kind_is_suspend, simpleir_kind_is_verifier_phi,
    simpleir_kind_is_wasm_stateful_dispatch,
};
use std::collections::BTreeSet;

impl RustBackend {
    pub(super) fn emit_logical_flow(&mut self, ops: &[OpIR], flow: &SimpleIrLogicalFlow) {
        if flow.blocks.is_empty() {
            self.emit_dispatch_target(None);
            return;
        }

        // Rust typechecks unreachable match arms too. Only the canonical
        // executable graph decides which blocks exist in this projection.
        let mut reachable = BTreeSet::new();
        let mut pending = vec![0];
        while let Some(block) = pending.pop() {
            if reachable.insert(block) {
                let (_, end) = flow.blocks[block];
                pending.extend(
                    flow.edges[end]
                        .iter()
                        .map(|edge| flow.op_to_block[edge.target]),
                );
            }
        }

        let function_values = self.hoisted_vars.clone();
        self.emit_line("let mut __molt_block: usize = 0;");
        self.emit_line("loop {");
        self.push_indent();
        self.emit_line("match __molt_block {");
        self.push_indent();
        for block in reachable {
            let (start, end) = flow.blocks[block];
            self.emit_line(&format!("{block} => {{"));
            self.push_indent();

            // Source-order alias hints cannot flow from one textual case to
            // another: their runtime predecessor may be entirely different.
            // Python object/alias semantics remain gated by target admission.
            self.aliases.clear();
            self.phi_to_frame.clear();
            self.hoisted_vars = function_values.clone();
            self.hoisted_vars
                .extend(self.current_params.iter().cloned());
            let local_values: BTreeSet<_> = ops[start..=end]
                .iter()
                .flat_map(op_definition_vars)
                .filter(|name| name != "_" && !self.hoisted_vars.contains(name))
                .collect();
            for value in &local_values {
                self.emit_line(&format!("let mut {value}: MoltValue = MoltValue::None;"));
            }
            self.hoisted_vars.extend(local_values);

            for op in &ops[start..=end] {
                if simpleir_kind_is_suspend(&op.kind)
                    || simpleir_kind_is_wasm_stateful_dispatch(&op.kind)
                    || simpleir_kind_is_exception_check(&op.kind)
                    || op.kind == "loop_break_if_exception"
                {
                    self.emit_unsupported_op(
                        op,
                        "labelled-flow emission requires admitted synchronous, non-exception edges",
                    );
                } else if simpleir_kind_is_verifier_phi(&op.kind)
                    || matches!(op.kind.as_str(), "loop_index_start" | "loop_index_next")
                {
                    self.emit_unsupported_op(
                        op,
                        "canonical SSA lowering left an unresolved predecessor value",
                    );
                } else if simpleir_kind_is_return_terminator(&op.kind)
                    || !simpleir_kind_is_structural(&op.kind)
                    || op.kind == "nop"
                {
                    self.emit_op(op);
                }
            }

            let tail = &ops[end];
            if !simpleir_kind_is_return_terminator(&tail.kind) {
                let edges = &flow.edges[end];
                if edges
                    .iter()
                    .any(|edge| matches!(edge.role, EdgeRole::BranchTrue | EdgeRole::BranchFalse))
                {
                    if let Some(condition) = tail.args.as_deref().and_then(|args| args.first()) {
                        let target = |role| {
                            edges
                                .iter()
                                .find(|edge| edge.role == role)
                                .map(|edge| flow.op_to_block[edge.target])
                        };
                        self.emit_line(&format!("if molt_bool(&{}) {{", rust_value(condition)));
                        self.push_indent();
                        self.emit_dispatch_target(target(EdgeRole::BranchTrue));
                        self.pop_indent();
                        self.emit_line("} else {");
                        self.push_indent();
                        self.emit_dispatch_target(target(EdgeRole::BranchFalse));
                        self.pop_indent();
                        self.emit_line("}");
                    } else {
                        self.emit_unsupported_op(tail, "logical conditional edge has no condition");
                    }
                } else if edges.len() <= 1
                    && edges.iter().all(|edge| {
                        matches!(
                            edge.role,
                            EdgeRole::LoopEntry
                                | EdgeRole::LoopLatch
                                | EdgeRole::LoopExit
                                | EdgeRole::Fallthrough
                                | EdgeRole::Taken
                        )
                    })
                {
                    self.emit_dispatch_target(
                        edges.first().map(|edge| flow.op_to_block[edge.target]),
                    );
                } else {
                    self.emit_unsupported_op(
                        tail,
                        "logical transfer requires an unavailable runtime edge protocol",
                    );
                }
            }
            self.pop_indent();
            self.emit_line("}");
        }
        self.emit_line("_ => unreachable!(\"invalid canonical Rust control-flow block\"),");
        self.pop_indent();
        self.emit_line("}");
        self.pop_indent();
        self.emit_line("}");
        self.hoisted_vars = function_values;
        self.aliases.clear();
        self.phi_to_frame.clear();
    }

    fn emit_dispatch_target(&mut self, target: Option<usize>) {
        if let Some(target) = target {
            self.emit_line(&format!("__molt_block = {target};"));
        } else {
            self.emit_param_writeback();
            self.emit_line(if self.current_is_main {
                "return;"
            } else {
                "return MoltValue::None;"
            });
        }
    }
}
