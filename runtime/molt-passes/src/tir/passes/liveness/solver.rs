use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{self, CfgEdgePolicy};
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::passes::alias_analysis::{AliasUnionFind, build_alias_union_find};
use crate::tir::values::ValueId;

use super::api::TirLivenessResult;
use super::raw::compute_raw_scalars;

/// Normal demand crosses the terminator. Exceptional demand crosses only
/// the prefix through the observation, including its own definitions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Exit {
    Normal,
    Observation(usize),
}

struct DemandEdge {
    source: BlockId,
    exit: Exit,
}

struct LivenessProblem<'a> {
    func: &'a TirFunction,
    aliases: &'a AliasUnionFind,
    raw: &'a HashSet<ValueId>,
    labels: HashMap<i64, BlockId>,
    rewritten_reads: Option<&'a HashSet<(BlockId, usize)>>,
}

impl<'a> LivenessProblem<'a> {
    fn new(func: &'a TirFunction, aliases: &'a AliasUnionFind, raw: &'a HashSet<ValueId>) -> Self {
        Self {
            func,
            aliases,
            raw,
            labels: dominators::exception_label_to_block(func),
            rewritten_reads: None,
        }
    }

    fn root(&self, value: ValueId) -> Option<ValueId> {
        let root = self.aliases.root(value);
        (!self.raw.contains(&root)).then_some(root)
    }

    fn definitions<'b>(&'b self, op: &'b TirOp) -> impl Iterator<Item = ValueId> + 'b {
        op.results
            .iter()
            .copied()
            // An alias names its owner's existing lifetime.
            .filter(|&result| self.root(result) == Some(result))
    }

    fn replay(
        &self,
        block: &TirBlock,
        live: &mut HashSet<ValueId>,
        mut observe: impl FnMut(usize, &TirOp, &mut HashSet<ValueId>),
        mut kill: impl FnMut(ValueId, usize),
    ) {
        let term_uses = match &block.terminator {
            Terminator::CondBranch { cond, .. } => std::slice::from_ref(cond),
            Terminator::Switch { value, .. } => std::slice::from_ref(value),
            term => term.exit_values().unwrap_or_default(),
        };
        live.extend(term_uses.iter().filter_map(|&value| self.root(value)));
        for (index, op) in block.ops.iter().enumerate().rev() {
            observe(index, op, live);
            for result in self.definitions(op) {
                kill(result, index + 1);
                live.remove(&result);
            }
            if self
                .rewritten_reads
                .is_none_or(|sites| !sites.contains(&(block.id, index)))
            {
                live.extend(op.operands.iter().filter_map(|&value| self.root(value)));
            }
        }
        for arg in &block.args {
            if let Some(root) = self.root(arg.id) {
                kill(root, 0);
                live.remove(&root);
            }
        }
    }

    fn solve(&self) -> Solution {
        let reachable = dominators::reverse_postorder_with(self.func, CfgEdgePolicy::Full);
        let mut live = TirLivenessResult {
            live_in: self
                .func
                .blocks
                .keys()
                .map(|&id| (id, HashSet::default()))
                .collect(),
            live_out: self
                .func
                .blocks
                .keys()
                .map(|&id| (id, HashSet::default()))
                .collect(),
            raw_scalars: self.raw.clone(),
        };
        let mut predecessors: HashMap<BlockId, Vec<DemandEdge>> = HashMap::default();
        let mut kills = HashMap::default();
        let mut pending = Vec::new();
        let mut observations = Vec::new();
        for &source in reachable.iter().rev() {
            let block = &self.func.blocks[&source];
            let out = live.live_out.get_mut(&source).unwrap();
            block.terminator.for_each_edge(|target, args| {
                out.extend(args.iter().filter_map(|&value| self.root(value)));
                let edges = predecessors.entry(target).or_default();
                if !edges.last().is_some_and(|edge| edge.source == source) {
                    edges.push(DemandEdge {
                        source,
                        exit: Exit::Normal,
                    });
                }
            });
            // The first observation to a particular handler admits every root
            // that any later observation to that handler could propagate to
            // entry. Point replay still visits each observation individually.
            // Seed local reads. Dependencies propagate the remaining demands;
            // there is no need to replay these operations after any update.
            let mut first_observations = HashMap::default();
            let mut has_observation = false;
            let input = live.live_in.get_mut(&source).unwrap();
            input.clone_from(out);
            // The walk is backwards, so each insertion leaves the earliest
            // observation or kill, including a kill on entry at a block arg.
            self.replay(
                block,
                input,
                |index, op, _| {
                    if let Some(target) = dominators::exception_transfer_target(op, &self.labels) {
                        first_observations.insert(target, index + 1);
                        has_observation |=
                            dominators::exception_edge_binds_handler_arguments(op.opcode);
                    }
                },
                |root, position| {
                    kills.insert((source, root), position);
                },
            );
            if has_observation {
                observations.push(source);
            }
            for (target, position) in first_observations {
                predecessors.entry(target).or_default().push(DemandEdge {
                    source,
                    exit: Exit::Observation(position),
                });
            }
            pending.extend(input.iter().map(|&root| (source, root)));
        }

        while let Some((target, root)) = pending.pop() {
            let Some(edges) = predecessors.get(&target) else {
                continue;
            };
            for &DemandEdge { source, exit } in edges {
                let position = match exit {
                    Exit::Normal => {
                        live.live_out.get_mut(&source).unwrap().insert(root);
                        usize::MAX
                    }
                    Exit::Observation(position) => position,
                };
                if kills
                    .get(&(source, root))
                    .is_none_or(|&kill| kill > position)
                    && live.live_in.get_mut(&source).unwrap().insert(root)
                {
                    pending.push((source, root));
                }
            }
        }
        Solution { live, observations }
    }
}

struct Solution {
    live: TirLivenessResult,
    observations: Vec<BlockId>,
}

/// The normal continuation may keep a root that the exceptional path
/// abandons. The exceptional roots come from the handler and this check's
/// operands, each already in the canonical owner domain.
pub(crate) struct ExceptionDemand<'a> {
    /// Ordinal among resolved transfers in this block, newest first,
    /// including region registrations used by the conservative graph.
    pub(crate) transfer_from_end: usize,
    normal: &'a HashSet<ValueId>,
    handler: &'a HashSet<ValueId>,
    operands: &'a HashSet<ValueId>,
}

impl ExceptionDemand<'_> {
    pub(crate) fn abandoned(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.normal
            .difference(self.handler)
            .copied()
            .filter(|root| !self.operands.contains(root))
    }
}

/// Backward dataflow over normal and point-specific exceptional successors.
/// `live_out` means the normal terminator boundary; exceptional demand enters
/// at its operation and propagates into `live_in`.
pub fn compute_liveness(func: &TirFunction) -> TirLivenessResult {
    let aliases = build_alias_union_find(func);
    let raw = compute_raw_scalars(func);
    compute_liveness_in_domain(func, &aliases, &raw)
}

/// Solve on the current operations with an already established value domain.
/// Alias and representation facts may be reused only if definitions and
/// transparent aliases are unchanged; operation uses are always read afresh.
pub(crate) fn compute_liveness_in_domain(
    func: &TirFunction,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
) -> TirLivenessResult {
    LivenessProblem::new(func, aliases, raw).solve().live
}

/// An under-approximation that excludes operand reads a planned rewrite may
/// erase. Definitions and exceptional/normal CFG transfers keep their usual
/// meaning. Only uses that survive the rewrite may witness an owner.
pub(crate) fn surviving_read_liveness(
    func: &TirFunction,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
    rewritten_reads: &HashSet<(BlockId, usize)>,
) -> TirLivenessResult {
    let mut problem = LivenessProblem::new(func, aliases, raw);
    if !rewritten_reads.is_empty() {
        problem.rewritten_reads = Some(rewritten_reads);
    }
    problem.solve().live
}

/// Replay only blocks containing exceptional observations. Each handler's
/// entry demand stays borrowed, and one buffer collects the current operands.
pub(crate) fn visit_exception_liveness(
    func: &TirFunction,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
    mut observe: impl FnMut(BlockId, usize, BlockId, ExceptionDemand<'_>),
) {
    let problem = LivenessProblem::new(func, aliases, raw);
    if problem.labels.is_empty()
        || !func.blocks.values().any(|block| {
            block.ops.iter().any(|op| {
                dominators::exception_edge_binds_handler_arguments(op.opcode)
                    && dominators::exception_transfer_target(op, &problem.labels).is_some()
            })
        })
    {
        return;
    }
    let Solution {
        mut live,
        mut observations,
    } = problem.solve();
    observations.sort_unstable_by_key(|block| block.0);
    let mut operands = HashSet::default();
    for block in observations {
        let mut normal = live.live_out.remove(&block).unwrap();
        let mut transfers = 0;
        problem.replay(
            &func.blocks[&block],
            &mut normal,
            |index, op, normal| {
                let Some(target) = dominators::exception_transfer_target(op, &problem.labels)
                else {
                    return;
                };
                // Live-in already crossed the handler's argument definitions.
                let handler = &live.live_in[&target];
                if dominators::exception_edge_binds_handler_arguments(op.opcode) {
                    operands.clear();
                    operands.extend(op.operands.iter().filter_map(|&value| problem.root(value)));
                    observe(
                        block,
                        index,
                        target,
                        ExceptionDemand {
                            transfer_from_end: transfers,
                            normal,
                            handler,
                            operands: &operands,
                        },
                    );
                }
                transfers += 1;
                normal.extend(handler);
            },
            |_, _| {},
        );
    }
}
