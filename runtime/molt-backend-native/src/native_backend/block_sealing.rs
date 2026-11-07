//! Native ownership of Cranelift block construction and predecessor closure.
//! The shared SimpleIR CFG supplies possible incoming sources. Native lowering
//! consumes them on emitted edges, or closes them when their source is finished.
use super::*;
use cranelift_codegen::ir::{DataFlowGraph, Inst, InstBuilderBase, InstructionData, Type};
use cranelift_frontend::FunctionBuilder as CraneliftFunctionBuilder;

#[derive(Debug)]
pub enum NativeCompileError {
    Admission(molt_ir::ir_schema::FunctionOpShapeDiagnostic),
    Codegen(NativeBlockError),
}

impl From<molt_ir::ir_schema::FunctionOpShapeDiagnostic> for NativeCompileError {
    fn from(value: molt_ir::ir_schema::FunctionOpShapeDiagnostic) -> Self {
        Self::Admission(value)
    }
}

impl From<NativeBlockError> for NativeCompileError {
    fn from(value: NativeBlockError) -> Self {
        Self::Codegen(value)
    }
}

impl std::fmt::Display for NativeCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(f),
            Self::Codegen(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for NativeCompileError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeBlockIssue {
    Unsealed,
    UnexpectedPredecessor,
}

#[derive(Debug)]
pub struct NativeBlockError {
    pub function: String,
    pub block: Block,
    pub issue: NativeBlockIssue,
}
impl std::fmt::Display for NativeBlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "native codegen in function `{}`: block {} {:?}",
            self.function, self.block, self.issue
        )
    }
}
impl std::error::Error for NativeBlockError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IncomingState {
    Pending,
    Emitted,
    Closed,
}

#[derive(Default)]
struct NativeBlock {
    sealed: bool,
    source: Option<usize>,
    incoming: Option<Vec<(usize, IncomingState)>>,
    remaining: usize,
}

/// All native builders use this boundary, including refcount diamonds. The
/// pinned Switch helper borrows the underlying builder and owns its own blocks.
pub(crate) struct FunctionBuilder<'a> {
    inner: CraneliftFunctionBuilder<'a>,
    blocks: Vec<Option<NativeBlock>>,
    block_for_op: Vec<usize>,
    close_at: Vec<Vec<(Block, usize)>>,
    next_close: usize,
    current_op: Option<usize>,
    error: Option<(Block, NativeBlockIssue)>,
    completed_switches: usize,
}

impl<'a> std::ops::Deref for FunctionBuilder<'a> {
    type Target = CraneliftFunctionBuilder<'a>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for FunctionBuilder<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<'a> FunctionBuilder<'a> {
    pub(crate) fn new(func: &'a mut Function, ctx: &'a mut FunctionBuilderContext) -> Self {
        Self {
            inner: CraneliftFunctionBuilder::new(func, ctx),
            blocks: Vec::new(),
            block_for_op: Vec::new(),
            close_at: Vec::new(),
            next_close: 0,
            current_op: None,
            error: None,
            completed_switches: 0,
        }
    }

    pub(crate) fn create_block(&mut self) -> Block {
        let block = self.inner.create_block();
        self.blocks
            .resize_with(block.as_u32() as usize + 1, || None);
        let source = self.current_op.map(|op| self.block_for_op[op]);
        self.blocks[block.as_u32() as usize] = Some(NativeBlock {
            source,
            ..Default::default()
        });
        block
    }

    pub(crate) fn seal_block(&mut self, block: Block) {
        let state = self.blocks[block.as_u32() as usize]
            .as_mut()
            .expect("native-owned block");
        if !state.sealed {
            self.inner.seal_block(block);
            self.blocks[block.as_u32() as usize]
                .as_mut()
                .unwrap()
                .sealed = true;
        }
    }

    pub(crate) fn is_sealed(&self, block: Block) -> bool {
        self.blocks
            .get(block.as_u32() as usize)
            .and_then(Option::as_ref)
            .is_some_and(|state| state.sealed)
    }

    fn seal_ready(&mut self, block: Block) {
        let state = self.blocks[block.as_u32() as usize].as_ref().unwrap();
        if state.incoming.is_some()
            && state.remaining == 0
            && !state.sealed
            && self.inner.func.layout.is_block_inserted(block)
        {
            self.seal_block(block);
        }
    }

    pub(crate) fn switch_to_block(&mut self, block: Block) {
        self.inner.switch_to_block(block);
        if let Some(op) = self.current_op {
            self.blocks[block.as_u32() as usize]
                .as_mut()
                .expect("native switch target")
                .source = Some(self.block_for_op[op]);
        }
        self.seal_ready(block);
    }

    /// Reuse the CFG already built for liveness and dominance; no second CFG.
    pub(crate) fn plan_labels(
        &mut self,
        ops: &[OpIR],
        cfg: &crate::tir::cfg::CFG,
        labels: &BTreeMap<i64, Block>,
    ) {
        self.block_for_op = vec![cfg.entry; ops.len()];
        for block in &cfg.blocks {
            self.block_for_op[block.start_op..block.end_op].fill(block.id);
        }
        self.close_at = vec![Vec::new(); ops.len() + 2];
        for (op_idx, op) in ops
            .iter()
            .enumerate()
            .filter(|(_, op)| matches!(op.kind, OpKind::Label | OpKind::StateLabel))
        {
            let Some(&block) = op.value.and_then(|label| labels.get(&label)) else {
                continue;
            };
            let target = self.block_for_op[op_idx];
            let mut sources = cfg.predecessors[target].clone();
            sources.extend(cfg.exception_edges.iter().filter_map(|&(source, dest)| {
                (dest == target && ops[cfg.blocks[source].end_op - 1].kind.is_exception_check())
                    .then_some(source)
            }));
            sources.extend(
                cfg.state_resume_edges
                    .iter()
                    .filter_map(|&(source, dest, _)| (dest == target).then_some(source)),
            );
            sources.sort_unstable();
            sources.dedup();
            let incoming: Vec<_> = sources
                .into_iter()
                .map(|source| (source, IncomingState::Pending))
                .collect();
            for (index, &(source, _)) in incoming.iter().enumerate() {
                // A textual fallthrough is emitted while materializing the
                // following label (end_op). Close only after that operation.
                let deadline = cfg.blocks[source].end_op + 1;
                self.close_at[deadline].push((block, index));
            }
            let state = self.blocks[block.as_u32() as usize].as_mut().unwrap();
            state.remaining = incoming.len();
            state.incoming = Some(incoming);
        }
        if let Some(block) = self.inner.current_block() {
            self.blocks[block.as_u32() as usize]
                .as_mut()
                .unwrap()
                .source = Some(cfg.entry);
        }
    }

    pub(crate) fn begin_op(&mut self, index: usize, op: &OpIR) {
        self.close_sources(index);
        self.current_op = Some(index);
        // A label first emits the previous block's fallthrough; its switch
        // then establishes the new source identity.
        if !matches!(op.kind, OpKind::Label | OpKind::StateLabel)
            && let Some(block) = self.inner.current_block()
        {
            self.blocks[block.as_u32() as usize]
                .as_mut()
                .unwrap()
                .source = Some(self.block_for_op[index]);
        }
    }

    pub(crate) fn close_sources(&mut self, through: usize) {
        while self.next_close <= through && self.next_close < self.close_at.len() {
            let entries = std::mem::take(&mut self.close_at[self.next_close]);
            for (block, index) in entries {
                let state = self.blocks[block.as_u32() as usize].as_mut().unwrap();
                let incoming = &mut state.incoming.as_mut().unwrap()[index].1;
                if *incoming == IncomingState::Pending {
                    *incoming = IncomingState::Closed;
                    state.remaining -= 1;
                    self.seal_ready(block);
                }
            }
            self.next_close += 1;
        }
    }

    fn edge(&mut self, source: Option<usize>, target: Block) {
        let Some(state) = self
            .blocks
            .get_mut(target.as_u32() as usize)
            .and_then(Option::as_mut)
        else {
            return;
        };
        if let Some(incoming) = state.incoming.as_mut() {
            if let Some((_, status)) = incoming.iter_mut().find(|(id, _)| Some(*id) == source) {
                match status {
                    IncomingState::Pending => {
                        *status = IncomingState::Emitted;
                        state.remaining -= 1;
                    }
                    IncomingState::Emitted => {} // one source may lower to several physical edges
                    IncomingState::Closed => {
                        self.error
                            .get_or_insert((target, NativeBlockIssue::UnexpectedPredecessor));
                    }
                }
            } else {
                self.error
                    .get_or_insert((target, NativeBlockIssue::UnexpectedPredecessor));
            }
            self.seal_ready(target);
        }
    }

    pub(crate) fn ins(&mut self) -> NativeInstBuilder<'_, 'a> {
        NativeInstBuilder(self)
    }

    pub(crate) fn emit_switch(&mut self, switch: Switch, value: Value, otherwise: Block) {
        let origin = self.inner.current_block().unwrap();
        let source = self.blocks[origin.as_u32() as usize]
            .as_ref()
            .unwrap()
            .source;
        let first = self.inner.func.dfg.num_blocks();
        switch.emit(&mut self.inner, value, otherwise);
        self.completed_switches += 1;
        let mut targets = Vec::new();
        for block in self
            .inner
            .func
            .layout
            .blocks()
            .filter(|block| *block == origin || block.as_u32() as usize >= first)
        {
            for inst in self.inner.func.layout.block_insts(block) {
                targets.extend(
                    self.inner.func.dfg.insts[inst]
                        .branch_destination(
                            &self.inner.func.dfg.jump_tables,
                            &self.inner.func.dfg.exception_tables,
                        )
                        .iter()
                        .map(|dest| dest.block(&self.inner.func.dfg.value_lists)),
                );
            }
        }
        targets.sort_unstable();
        targets.dedup();
        for target in targets {
            self.edge(source, target);
        }
        self.seal_block(otherwise);
    }

    pub(crate) fn check_sealed(&self, function: &str) -> Result<(), NativeBlockError> {
        let missing = self.blocks.iter().enumerate().find_map(|(id, state)| {
            let state = state.as_ref()?;
            let block = Block::from_u32(id as u32);
            (self.inner.func.layout.is_block_inserted(block)
                && (!state.sealed || state.remaining != 0))
                .then_some((block, NativeBlockIssue::Unsealed))
        });
        match self.error.or(missing) {
            Some((block, issue)) => Err(NativeBlockError {
                function: function.into(),
                block,
                issue,
            }),
            None => Ok(()),
        }
    }

    pub(crate) fn finalize(self, config: cranelift_codegen::isa::TargetFrontendConfig) {
        self.inner.finalize(config);
    }
}

pub(crate) struct NativeInstBuilder<'b, 'a>(&'b mut FunctionBuilder<'a>);
impl<'b> InstBuilderBase<'b> for NativeInstBuilder<'b, '_> {
    fn data_flow_graph(&self) -> &DataFlowGraph {
        &self.0.inner.func.dfg
    }
    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph {
        &mut self.0.inner.func.dfg
    }
    fn build(self, data: InstructionData, ty: Type) -> (Inst, &'b mut DataFlowGraph) {
        let source = self.0.inner.current_block().and_then(|block| {
            self.0.blocks[block.as_u32() as usize]
                .as_ref()
                .and_then(|state| state.source)
        });
        let (inst, _) = self.0.inner.ins().build(data, ty);
        let destinations = self.0.inner.func.dfg.insts[inst].branch_destination(
            &self.0.inner.func.dfg.jump_tables,
            &self.0.inner.func.dfg.exception_tables,
        );
        if !destinations.is_empty() {
            let mut targets: Vec<_> = destinations
                .iter()
                .map(|dest| dest.block(&self.0.inner.func.dfg.value_lists))
                .collect();
            targets.sort_unstable();
            targets.dedup();
            for target in targets {
                self.0.edge(source, target);
            }
        }
        (inst, &mut self.0.inner.func.dfg)
    }
}
