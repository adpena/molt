use super::super::control_flow::dispatch_control_panic;
use super::super::frame_locals::WasmDispatchFrameLocals;
use super::super::function_frame::WasmFrameControlMode;
use super::block_layout::{build_dispatch_block_map, build_dispatch_blocks};
use super::state_remap::{
    build_dense_state_remap_table, build_state_resume_maps, stateful_entry_prologue_end,
};
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use molt_ir::simple_verify::{EdgeRole, SimpleIrExecutableFlow, simple_ir_executable_flow};
use molt_ir::tir::op_kinds_generated::{SimpleIrVerifierRegionRole, simpleir_verifier_region_role};
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

/// Physical labels and transfer actions projected from the shared executable
/// operation graph. Planning creates no backend data segments or frame locals.
pub(in crate::wasm) struct NonLinearControlPlan {
    pub(super) block_starts: Vec<usize>,
    block_for_op: Vec<usize>,
    flow: SimpleIrExecutableFlow,
    reachable: Vec<bool>,
    transparent_region_ends: Vec<bool>,
    /// Physical block indices reached by dynamic dispatch, in source order.
    /// Stateful retains every block because its raw negative state decodes an
    /// arbitrary in-range operation; ordinary Jumpful has no such producer.
    pub(super) dispatch_targets: Vec<usize>,
    pub(super) state_resume: Option<StateResumePlan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StaticDispatchEdge {
    Forward { label_depth: u32 },
    Redispatch { operation: usize },
    ReturnNone,
}

/// Labels enclosing one emitted body. A forward-only body has no loop label;
/// return and redispatch consumers must use the same geometry.
#[derive(Clone, Copy)]
pub(super) struct ControlScopeDepth {
    forward_labels: u32,
    dispatch_loop: bool,
}

impl ControlScopeDepth {
    pub(super) fn return_depth(self, selection_depth: u32) -> u32 {
        self.forward_labels + u32::from(self.dispatch_loop) + selection_depth
    }

    pub(super) fn redispatch_depth(self, selection_depth: u32) -> u32 {
        assert!(self.dispatch_loop, "direct control has no redispatch loop");
        self.forward_labels + selection_depth
    }
}

pub(super) struct StateResumePlan {
    pub(super) state_map: BTreeMap<i64, usize>,
    pub(super) remap_table: Option<Vec<u8>>,
}

impl NonLinearControlPlan {
    pub(in crate::wasm) fn for_function(
        func_ir: &FunctionIR,
        control_mode: WasmFrameControlMode,
    ) -> Option<Self> {
        if matches!(control_mode, WasmFrameControlMode::Plain) {
            return None;
        }
        let stateful = control_mode.is_stateful();
        let flow = simple_ir_executable_flow(&func_ir.ops);
        let reachable = if stateful {
            // The external saved-state decoder accepts every in-range source
            // operation, independently of entry-only graph reachability.
            vec![true; func_ir.ops.len()]
        } else {
            flow.executable_reachable()
        };
        let transparent_region_ends = func_ir
            .ops
            .iter()
            .map(|op| {
                matches!(
                    simpleir_verifier_region_role(&op.kind),
                    Some((_, SimpleIrVerifierRegionRole::End))
                )
            })
            .collect();
        let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
        let mut dynamic_blocks = vec![stateful; block_starts.len()];
        if !stateful {
            for (source, edges) in flow.edges.iter().enumerate() {
                if !reachable[source] {
                    continue;
                }
                for edge in edges {
                    // Ordinary intra-block fallthrough is not reentry. Every
                    // actual self/backward operation transfer needs dispatch.
                    if edge.target <= source {
                        dynamic_blocks[block_for_op[edge.target]] = true;
                    }
                }
            }
            // Invocation entry is an actual producer only when a dispatch loop
            // remains. A forward-only function enters operation zero directly.
            if dynamic_blocks.iter().any(|&dynamic| dynamic) {
                dynamic_blocks[0] = true;
            }
        }
        let dispatch_targets = dynamic_blocks
            .into_iter()
            .enumerate()
            .filter_map(|(block, dynamic)| dynamic.then_some(block))
            .collect();
        let state_resume = stateful.then(|| {
            let entry_prologue_end = stateful_entry_prologue_end(&func_ir.ops);
            let state_map = build_state_resume_maps(&func_ir.ops, entry_prologue_end + 1);
            let remap_table = build_dense_state_remap_table(&state_map);
            StateResumePlan {
                state_map,
                remap_table,
            }
        });
        Some(Self {
            block_starts,
            block_for_op,
            flow,
            reachable,
            transparent_region_ends,
            dispatch_targets,
            state_resume,
        })
    }

    pub(in crate::wasm) fn needs_dispatch(&self) -> bool {
        !self.dispatch_targets.is_empty()
    }

    pub(super) fn operation_reachable(&self, operation: usize) -> bool {
        self.reachable[operation]
    }

    pub(super) fn scope_depth(&self, block: usize) -> ControlScopeDepth {
        ControlScopeDepth {
            forward_labels: (self.block_starts.len() - block - 1) as u32,
            dispatch_loop: self.needs_dispatch(),
        }
    }

    pub(in crate::wasm) fn emit_table_bases(
        &self,
        backend: &mut WasmBackend,
        func_index: u32,
        func: &mut Function,
        reloc_enabled: bool,
        locals: Option<WasmDispatchFrameLocals>,
    ) {
        if !self.needs_dispatch() {
            assert!(
                locals.is_none(),
                "direct control must not reserve dispatch locals"
            );
            return;
        }
        let locals = locals.expect("dynamic control requires dispatch locals");
        let bytes = build_dispatch_block_map(&self.block_for_op, &self.dispatch_targets);
        let segment = backend.add_data_segment(reloc_enabled, &bytes);
        backend.emit_data_ptr(reloc_enabled, func_index, func, segment);
        func.instruction(&Instruction::LocalSet(locals.block_map_base_local));
        if let Some(bytes) = self
            .state_resume
            .as_ref()
            .and_then(|resume| resume.remap_table.as_ref())
        {
            let segment = backend.add_data_segment(reloc_enabled, bytes);
            let local = locals
                .state_remap_base_local
                .expect("stateful remap local missing");
            backend.emit_data_ptr(reloc_enabled, func_index, func, segment);
            func.instruction(&Instruction::LocalSet(local));
        }
    }

    pub(super) fn successor(&self, source: usize, role: EdgeRole) -> usize {
        let edges = &self.flow.edges[source];
        if let Some(edge) = edges.iter().find(|edge| edge.role == role) {
            return edge.target;
        }
        // The canonical graph keeps executable exits outside its vertices. A
        // conditional tail can have one real successor and one implicit exit.
        if self.flow.implicit_exits.contains(&source) && edges.len() == 1 {
            return self.flow.edges.len();
        }
        panic!("missing canonical {role:?} successor at operation {source}")
    }

    pub(super) fn sole_successor(&self, source: usize) -> usize {
        let edges = &self.flow.edges[source];
        match edges.as_slice() {
            [edge] if !self.flow.implicit_exits.contains(&source) => edge.target,
            [] if self.flow.implicit_exits.contains(&source) => self.flow.edges.len(),
            _ => panic!("operation {source} does not have one canonical successor"),
        }
    }

    pub(super) fn fallthrough_successor(&self, source: usize) -> Option<usize> {
        let edges = &self.flow.edges[source];
        if let Some(edge) = edges.iter().find(|edge| {
            matches!(
                edge.role,
                EdgeRole::Fallthrough
                    | EdgeRole::Normal
                    | EdgeRole::BranchFalse
                    | EdgeRole::LoopEntry
            )
        }) {
            Some(edge.target)
        } else if self.flow.implicit_exits.contains(&source) {
            Some(self.flow.edges.len())
        } else {
            None
        }
    }

    fn physical_target(&self, mut target: usize) -> usize {
        while target < self.block_for_op.len()
            && self.block_starts[self.block_for_op[target]] != target
        {
            // A canonical edge can enter a zero-effect region closer inside a
            // physical block (notably END_IF). Follow its actual executable
            // successor; entering its containing body would execute skipped
            // work. Do not split persisted raw-state blocks or skip latches.
            assert!(
                self.transparent_region_ends[target],
                "WASM successor enters a nontransparent block interior"
            );
            match self.flow.edges[target].as_slice() {
                [edge]
                    if matches!(edge.role, EdgeRole::Fallthrough | EdgeRole::LoopEntry)
                        && edge.target > target =>
                {
                    target = edge.target;
                }
                [] if self.flow.implicit_exits.contains(&target) => {
                    target = self.block_for_op.len();
                }
                _ => panic!("WASM region closer has no transparent executable successor"),
            }
        }
        target
    }

    pub(super) fn static_edge(&self, source: usize, target: usize) -> StaticDispatchEdge {
        let source_block = self.block_for_op[source];
        let target = self.physical_target(target);
        if target == self.block_for_op.len() {
            return StaticDispatchEdge::ReturnNone;
        }
        let target_block = *self
            .block_for_op
            .get(target)
            .expect("WASM successor outside function");
        assert_eq!(
            self.block_starts[target_block], target,
            "WASM successor must enter a physical block leader"
        );
        if target_block > source_block {
            StaticDispatchEdge::Forward {
                label_depth: (target_block - source_block - 1) as u32,
            }
        } else {
            assert!(
                self.dispatch_targets.binary_search(&target_block).is_ok(),
                "unplanned dynamic WASM target"
            );
            StaticDispatchEdge::Redispatch { operation: target }
        }
    }

    pub(super) fn require_terminal_return(&self, function: &str, source: usize) {
        if self.state_resume.is_some() {
            dispatch_control_panic(
                function,
                source,
                "stateful fallthrough requires an explicit terminal return",
            );
        }
    }
}
