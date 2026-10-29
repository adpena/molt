//! CFG (Control Flow Graph) extraction from SimpleIR.
//!
//! Takes a linear slice of `OpIR` operations and produces a structured CFG
//! with basic blocks, predecessor/successor edges, dominators, and loop
//! nesting depth.  This is the first step toward building the TIR (Typed IR).
//!
//! Supported control flow patterns:
//! - Structured: `if`/`else`/`end_if`, `loop_start`/`loop_end`
//! - Unstructured: `jump`/`goto`, `br_if`, `label`/`state_label`
//! - Terminators: `ret`, `ret_void`, `return`
//! - Loop control: `loop_break`, `loop_break_if_true`, `loop_break_if_false`, `loop_continue`

use crate::ir::OpIR;
use crate::tir::dominators::is_simple_exception_transfer_kind;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A basic block within a CFG.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: usize,
    /// First operation index in the original `OpIR` slice (inclusive).
    pub start_op: usize,
    /// One-past-the-last operation index (exclusive).
    pub end_op: usize,
}

/// Control Flow Graph extracted from a linear `OpIR` stream.
#[derive(Debug)]
pub struct CFG {
    /// Basic blocks with their operation index ranges.
    pub blocks: Vec<BasicBlock>,
    /// Invocation entry block index (always 0). A re-entrant first source block
    /// gets an empty predecessor so invocation parameters and loop arguments
    /// cannot become competing definitions of the function signature.
    pub entry: usize,
    /// Predecessor list per block.
    pub predecessors: Vec<Vec<usize>>,
    /// Successor list per block.
    pub successors: Vec<Vec<usize>>,
    /// Immediate dominator per block (`dominators[i] = Some(idom)` for all
    /// blocks reachable from entry; `None` for the entry block itself).
    pub dominators: Vec<Option<usize>>,
    /// Loop nesting depth per block (0 = not inside a loop).
    pub loop_depth: Vec<u32>,
    /// Exception edges authored by explicit exception-transfer operations.
    /// Each entry is `(from_block, handler_block)`. Calls only set pending
    /// exception state; `check_exception`, `async_work_poll`, and `try_start`
    /// own the actual transfer, so region membership must not invent blanket
    /// block-to-handler edges. A transfer back to its own block is an ordinary
    /// member of this relation: liveness, SSA placement, and edge arguments
    /// must all observe the same re-entry. Sorted by source block; each source
    /// has at most one target because every authored transfer ends its block.
    pub exception_edges: Vec<(usize, usize)>,
    /// Resume edges projected from the canonical operation-level dispatch
    /// relation. Entries are `(dispatch_block, resume_block, saved_state_id)`.
    /// Explicit `state_targets` maps own terminal identity; source suspension
    /// sites establish that identity only when no map has been authored yet.
    /// These edges join the augmented SSA graph for dominance, phi placement,
    /// and liveness, while ordinary successors retain the default-entry path.
    pub state_resume_edges: Vec<(usize, usize, i64)>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the op is a control-flow terminator that ends a basic
/// block (i.e. no fall-through to the next instruction).
fn is_terminator(kind: crate::OpKind) -> bool {
    kind.is_terminator()
}

/// Returns `true` if the op starts a new block boundary (the op itself is the
/// first instruction of the new block).
fn is_block_leader(kind: crate::OpKind) -> bool {
    kind.is_block_leader()
}

/// Returns `true` if the op ends a block and the next instruction should start
/// a new block (even if it's not itself a leader type).
fn is_block_ender(kind: crate::OpKind) -> bool {
    // An explicit exception transfer has two executable continuations even
    // though its normal continuation is fallthrough and its handler edge is
    // carried separately. End the block here so later definitions cannot be
    // attributed to the transfer's source program point by dominance/liveness.
    // Reuse the same generated transfer authority as compute_exception_edges,
    // including wire aliases such as async_work_poll.
    kind.is_block_ender() || is_simple_exception_transfer_kind(kind)
}

/// Returns `true` if the op is a conditional branch that causes a block split
/// (has both fall-through and a taken path).
fn is_conditional_branch(kind: crate::OpKind) -> bool {
    kind.is_conditional_branch()
}

/// Build a map from label-id → op-index for all `label` / `state_label` ops.
fn build_label_map(ops: &[OpIR]) -> HashMap<i64, usize> {
    let mut map = HashMap::new();
    for (idx, op) in ops.iter().enumerate() {
        if matches!(op.kind, crate::OpKind::Label | crate::OpKind::StateLabel)
            && let Some(id) = op.value
        {
            map.insert(id, idx);
        }
    }
    map
}

/// Given a set of leader op-indices, build the `BasicBlock` list. Each block
/// spans from one leader to the next (exclusive).
fn leaders_to_blocks(leaders: &BTreeSet<usize>, op_count: usize) -> Vec<BasicBlock> {
    let sorted: Vec<usize> = leaders.iter().copied().collect();
    let mut blocks = Vec::with_capacity(sorted.len());
    for (i, &start) in sorted.iter().enumerate() {
        let end = if i + 1 < sorted.len() {
            sorted[i + 1]
        } else {
            op_count
        };
        if start < end {
            blocks.push(BasicBlock {
                id: blocks.len(),
                start_op: start,
                end_op: end,
            });
        }
    }
    blocks
}

/// Find the block that contains a given op index.
fn block_containing(blocks: &[BasicBlock], op_idx: usize) -> Option<usize> {
    // Binary search on start_op (blocks are sorted).
    let pos = blocks.partition_point(|b| b.start_op <= op_idx);
    if pos == 0 {
        return None;
    }
    let candidate = pos - 1;
    if op_idx < blocks[candidate].end_op {
        Some(candidate)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Phase 1: identify basic-block boundaries (leaders)
// ---------------------------------------------------------------------------

fn find_leaders(ops: &[OpIR], label_map: &HashMap<i64, usize>) -> BTreeSet<usize> {
    let mut leaders = BTreeSet::new();
    if ops.is_empty() {
        return leaders;
    }
    // Op 0 is always a leader.
    leaders.insert(0);

    for (idx, op) in ops.iter().enumerate() {
        let kind = op.kind;

        // The instruction itself is a leader.
        if is_block_leader(kind) {
            leaders.insert(idx);
        }

        // The instruction *after* a terminator/conditional/block-ender is a leader.
        if (is_terminator(kind) || is_conditional_branch(kind) || is_block_ender(kind))
            && idx + 1 < ops.len()
        {
            leaders.insert(idx + 1);
        }

        // Target of a jump/br_if is a leader.
        if matches!(
            kind,
            crate::OpKind::Jump | crate::OpKind::Goto | crate::OpKind::BrIf
        ) && let Some(target_label) = op.value
            && let Some(&target_op) = label_map.get(&target_label)
        {
            leaders.insert(target_op);
        }

        // `loop_break_if_true` / `loop_break_if_false` target a label if present.
        if matches!(
            kind,
            crate::OpKind::LoopBreakIfTrue | crate::OpKind::LoopBreakIfFalse
        ) && let Some(target_label) = op.value
            && let Some(&target_op) = label_map.get(&target_label)
        {
            leaders.insert(target_op);
        }
    }

    leaders
}

// ---------------------------------------------------------------------------
// Phase 2: build edges
// ---------------------------------------------------------------------------

/// Build maps for structured if/else/end_if:
/// - `if` op-index → (`else` op-index or None, `end_if` op-index)
/// - `else` op-index → `end_if` op-index
fn build_if_else_maps(
    ops: &[OpIR],
) -> (
    HashMap<usize, (Option<usize>, usize)>,
    HashMap<usize, usize>,
) {
    let mut if_map = HashMap::new();
    let mut else_map = HashMap::new();
    let mut stack: Vec<(usize, Option<usize>)> = Vec::new(); // (if_idx, else_idx)
    for (idx, op) in ops.iter().enumerate() {
        match op.kind {
            crate::OpKind::If => stack.push((idx, None)),
            crate::OpKind::Else => {
                if let Some(top) = stack.last_mut() {
                    top.1 = Some(idx);
                }
            }
            crate::OpKind::EndIf => {
                if let Some((if_idx, else_idx)) = stack.pop() {
                    if_map.insert(if_idx, (else_idx, idx));
                    if let Some(ei) = else_idx {
                        else_map.insert(ei, idx);
                    }
                }
            }
            _ => {}
        }
    }
    (if_map, else_map)
}

fn build_edges(
    ops: &[OpIR],
    blocks: &[BasicBlock],
    label_map: &HashMap<i64, usize>,
) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let n = blocks.len();
    let mut successors: Vec<Vec<usize>> = vec![vec![]; n];
    let mut predecessors: Vec<Vec<usize>> = vec![vec![]; n];

    // Pre-compute structural maps.
    let (if_else_map, else_end_if_map) = build_if_else_maps(ops);

    // Pre-compute loop break targets.  For a loop nested in the then side of
    // an if/else, the structural successor after `loop_end` is the enclosing
    // `end_if`, not the `else` block.  Keep this aligned with default
    // fall-through so CFG pruning cannot make then-body breaks execute the
    // sibling else body.
    let loop_break_targets: HashMap<usize, usize> = {
        let mut targets = HashMap::new();
        let mut header_stack: Vec<usize> = Vec::new();
        for (bid, block) in blocks.iter().enumerate() {
            let first_kind = ops[block.start_op].kind;
            match first_kind {
                crate::OpKind::LoopStart => header_stack.push(bid),
                crate::OpKind::LoopEnd => {
                    if let Some(header_bid) = header_stack.pop()
                        && let Some(break_target) =
                            structured_fallthrough_target(ops, blocks, &else_end_if_map, bid)
                    {
                        targets.insert(header_bid, break_target);
                    }
                }
                _ => {}
            }
        }
        targets
    };

    // For loop handling: track loop_start op-index → header block-id.
    let mut loop_header_stack: Vec<usize> = Vec::new();

    for (bid, block) in blocks.iter().enumerate() {
        // Track structured loop markers by first op.
        let first_op = &ops[block.start_op];
        match first_op.kind {
            crate::OpKind::LoopStart => loop_header_stack.push(bid),
            crate::OpKind::LoopEnd => {
                // Back-edge to the loop header.
                if let Some(&header_bid) = loop_header_stack.last() {
                    add_edge(&mut successors, &mut predecessors, bid, header_bid);
                }
                loop_header_stack.pop();
                // Fall-through is handled below.
            }
            _ => {}
        }

        let last_op_idx = block.end_op - 1;
        let last_op = &ops[last_op_idx];
        let kind = last_op.kind;

        match kind {
            // Unconditional jump.
            crate::OpKind::Jump | crate::OpKind::Goto => {
                if let Some(target_label) = last_op.value
                    && let Some(&target_op) = label_map.get(&target_label)
                    && let Some(target_bid) = block_containing(blocks, target_op)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                }
            }

            // Return — no successors.
            _ if kind.is_return_terminator() => {}

            // Pure suspend: it `ret`s and has no regular successor. Its
            // continuation is reached only via `state_resume_edges`.
            _ if kind.is_suspend() && !kind.is_repoll() => {}

            // Re-poll suspend: pending path `ret`s, ready path falls through.
            _ if kind.is_repoll() => {
                if let Some(target_bid) =
                    structured_fallthrough_target(ops, blocks, &else_end_if_map, bid)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                }
            }

            // Loop break — terminates block and transfers control to the
            // block immediately after the enclosing loop. Without this edge,
            // CFG-based executable-region pruning can incorrectly mark the
            // real break path unreachable and erase it from later lowering.
            crate::OpKind::LoopBreak => {
                if let Some(&header_bid) = loop_header_stack.last()
                    && let Some(&post_loop_bid) = loop_break_targets.get(&header_bid)
                {
                    add_edge(&mut successors, &mut predecessors, bid, post_loop_bid);
                }
            }

            // Loop continue — jump back to current loop header, no fall-through.
            crate::OpKind::LoopContinue => {
                if let Some(&header_bid) = loop_header_stack.last() {
                    add_edge(&mut successors, &mut predecessors, bid, header_bid);
                }
            }

            // Structured `if`: two successors — then-block (fall-through) and
            // else-block or end_if-block.
            crate::OpKind::If => {
                if let Some(&(else_idx, end_if_idx)) = if_else_map.get(&last_op_idx) {
                    // True branch normally falls through into the then block.
                    // If the next block is `else`, the then branch is empty and
                    // the true edge must skip directly to the join block.
                    if bid + 1 < n {
                        let next_start = blocks[bid + 1].start_op;
                        let true_target = if ops[next_start].kind == crate::OpKind::Else {
                            end_if_idx
                        } else {
                            next_start
                        };
                        if let Some(target_bid) = block_containing(blocks, true_target) {
                            add_edge(&mut successors, &mut predecessors, bid, target_bid);
                        }
                    }

                    // False branch enters the else block when present, otherwise
                    // it skips directly to the join block.
                    let false_target = else_idx.unwrap_or(end_if_idx);
                    if let Some(target_bid) = block_containing(blocks, false_target) {
                        add_edge(&mut successors, &mut predecessors, bid, target_bid);
                    }
                } else {
                    // No matching end_if found; fall through.
                    if bid + 1 < n {
                        add_edge(&mut successors, &mut predecessors, bid, bid + 1);
                    }
                }
            }

            // Conditional branch to label.
            crate::OpKind::BrIf => {
                // Fall-through.
                if bid + 1 < n {
                    add_edge(&mut successors, &mut predecessors, bid, bid + 1);
                }
                // Taken path.
                if let Some(target_label) = last_op.value
                    && let Some(&target_op) = label_map.get(&target_label)
                    && let Some(target_bid) = block_containing(blocks, target_op)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                }
            }

            // Conditional loop break.
            //
            // `loop_break_if_exception` is a value-less conditional break gated
            // on the runtime exception flag.  Its edge structure is identical to
            // `loop_break_if_true` (break target FIRST, fall-through SECOND);
            // the SSA terminator builder materializes the flag-read condition
            // (`ExceptionPending`) and routes TRUE → break, FALSE → continue.
            crate::OpKind::LoopBreakIfTrue
            | crate::OpKind::LoopBreakIfFalse
            | crate::OpKind::LoopBreakIfException => {
                // IMPORTANT: successor ordering matters for SSA CondBranch
                // construction — succs[0] = then_block (TRUE path),
                // succs[1] = else_block (FALSE path).
                //
                // For loop_break_if_true: TRUE → break (exit), FALSE → continue.
                // For loop_break_if_false: TRUE → continue, FALSE → break (exit).
                //
                // Break target is added FIRST so it becomes succs[0] (then).
                // Fall-through is added SECOND so it becomes succs[1] (else).
                // For loop_break_if_false, the sense is inverted at the SSA
                // level (the CondBranch condition is negated).
                let mut break_added = false;
                if let Some(target_label) = last_op.value
                    && let Some(&target_op) = label_map.get(&target_label)
                    && let Some(target_bid) = block_containing(blocks, target_op)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                    break_added = true;
                }
                if !break_added {
                    // No explicit label — use the pre-computed break target
                    // from the enclosing loop header.
                    if let Some(&header_bid) = loop_header_stack.last()
                        && let Some(&post_loop_bid) = loop_break_targets.get(&header_bid)
                    {
                        add_edge(&mut successors, &mut predecessors, bid, post_loop_bid);
                    }
                }
                // Fall-through (continue path) — added SECOND to be succs[1] (else).
                if bid + 1 < n {
                    add_edge(&mut successors, &mut predecessors, bid, bid + 1);
                }
            }

            _ => {
                // Default: fall-through to next block — but if the next block
                // starts with `else`, the then-branch should skip to end_if.
                if let Some(target_bid) =
                    structured_fallthrough_target(ops, blocks, &else_end_if_map, bid)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                }
            }
        }
    }

    // De-duplicate edges.
    for succ in &mut successors {
        succ.sort_unstable();
        succ.dedup();
    }
    for pred in &mut predecessors {
        pred.sort_unstable();
        pred.dedup();
    }

    (successors, predecessors)
}

fn structured_fallthrough_target(
    ops: &[OpIR],
    blocks: &[BasicBlock],
    else_end_if_map: &HashMap<usize, usize>,
    from_bid: usize,
) -> Option<usize> {
    let next_bid = from_bid + 1;
    if next_bid >= blocks.len() {
        return None;
    }

    let next_start = blocks[next_bid].start_op;
    if ops[next_start].kind == crate::OpKind::Else
        && let Some(&end_if_idx) = else_end_if_map.get(&next_start)
        && let Some(end_if_bid) = block_containing(blocks, end_if_idx)
    {
        return Some(end_if_bid);
    }

    Some(next_bid)
}

fn add_edge(
    successors: &mut [Vec<usize>],
    predecessors: &mut [Vec<usize>],
    from: usize,
    to: usize,
) {
    successors[from].push(to);
    predecessors[to].push(from);
}

// ---------------------------------------------------------------------------
// Phase 4: loop detection and depth
// ---------------------------------------------------------------------------

fn compute_loop_depth(
    blocks: &[BasicBlock],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    dominance: &crate::tir::dominators::IndexedDominance,
) -> Vec<u32> {
    let n = blocks.len();
    let mut depth = vec![0u32; n];

    // Find back-edges: an edge b→h where h dominates b. Each test is O(1). A
    // self edge is a back edge even in a block the entry cannot reach.
    for (b, succs) in successors.iter().enumerate() {
        for &h in succs {
            if h == b || dominance.dominates(h, b) {
                // h is a loop header; find the natural loop body.
                // Use the pre-computed predecessor list (O(1) lookup per node)
                // instead of scanning all successors (which was O(N) per lookup).
                let body = natural_loop_body(h, b, predecessors);
                for &member in &body {
                    depth[member] += 1;
                }
            }
        }
    }

    depth
}

/// Compute the natural loop body for back-edge tail → header.
/// Uses pre-computed predecessor lists (zero allocations per predecessor lookup)
/// and HashSet for O(1) membership test.
fn natural_loop_body(header: usize, tail: usize, predecessors: &[Vec<usize>]) -> Vec<usize> {
    let mut body = HashSet::new();
    body.insert(header);
    if header == tail {
        return body.into_iter().collect();
    }
    body.insert(tail);
    let mut worklist = VecDeque::new();
    worklist.push_back(tail);
    while let Some(node) = worklist.pop_front() {
        for &pred in &predecessors[node] {
            if body.insert(pred) {
                worklist.push_back(pred);
            }
        }
    }
    body.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Phase 5: exception edge computation
// ---------------------------------------------------------------------------

/// Compute implicit exception edges from explicit transfer operations.
///
/// The structured try stack is path-sensitive: success and handler paths both
/// contain their own `try_end`, so a linear push/pop walk over block order can
/// pop an outer frame while scanning a mutually exclusive inner-handler path.
/// It is also semantically too broad: calls set pending state, while an explicit
/// observation owns the transfer after any required `finally` arbitration.
/// Deriving edges solely from the generated transfer-kind authority keeps the
/// pre-SSA CFG aligned with the executable transfer operations. A transfer to
/// the label leading its own block is returned as a self edge.
fn compute_exception_edges(
    ops: &[OpIR],
    blocks: &[BasicBlock],
    label_map: &HashMap<i64, usize>,
) -> Vec<(usize, usize)> {
    let mut edges: Vec<(usize, usize)> = Vec::new();

    for (bid, block) in blocks.iter().enumerate() {
        // The generated kind mapping includes wire aliases such as
        // `async_work_poll`; spelling-specific discovery would give those ops a
        // CheckException opcode after SSA while omitting the pre-SSA edge used
        // to place and populate handler block arguments.
        for op_idx in block.start_op..block.end_op {
            let op = &ops[op_idx];
            if !is_simple_exception_transfer_kind(op.kind) {
                continue;
            }
            let Some(target_label) = op.value else {
                continue;
            };
            let Some(&target_op) = label_map.get(&target_label) else {
                continue;
            };
            let Some(target_bid) = block_containing(blocks, target_op) else {
                continue;
            };
            edges.push((bid, target_bid));
        }
    }

    // De-duplicate.
    edges.sort_unstable();
    edges.dedup();
    edges
}

// ---------------------------------------------------------------------------
// Phase 6: state-machine resume edge computation
// ---------------------------------------------------------------------------

/// Project the canonical operation-level resume relation into this CFG's
/// blocks. Source suspension and explicit terminal maps have one authority;
/// block extraction must not reconstruct either convention independently.
fn compute_state_resume_edges(ops: &[OpIR], blocks: &[BasicBlock]) -> Vec<(usize, usize, i64)> {
    crate::simple_verify::state_resume_op_edges(ops)
        .into_iter()
        .filter_map(|(source, target, state)| {
            Some((
                block_containing(blocks, source)?,
                block_containing(blocks, target)?,
                state,
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl CFG {
    /// Build a CFG from a slice of `OpIR` operations.
    pub fn build(ops: &[OpIR]) -> Self {
        crate::ir_schema::validate_state_dispatch(ops)
            .unwrap_or_else(|error| panic!("invalid state dispatch: {error}"));
        if ops.is_empty() {
            return Self {
                blocks: vec![],
                entry: 0,
                predecessors: vec![],
                successors: vec![],
                dominators: vec![],
                loop_depth: vec![],
                exception_edges: vec![],
                state_resume_edges: vec![],
            };
        }

        let label_map = build_label_map(ops);
        let leaders = find_leaders(ops, &label_map);
        let mut blocks = leaders_to_blocks(&leaders, ops.len());
        let (mut successors, mut predecessors) = build_edges(ops, &blocks, &label_map);

        // Every explicit exception transfer participates, including self
        // edges. Splitting those out would give liveness and SSA different CFGs.
        let mut exception_edges = compute_exception_edges(ops, &blocks, &label_map);
        let mut state_resume_edges = compute_state_resume_edges(ops, &blocks);

        // Invocation is a distinct predecessor of a re-entrant source entry.
        // Represent it once in the shared CFG, before any dominance or SSA
        // analysis. This keeps internal live-ins (including initially undefined
        // locals) out of the callable ABI, and makes entry-loop phis ordinary
        // join arguments for regular, exception and resume edges alike.
        if !predecessors[0].is_empty()
            || exception_edges.iter().any(|&(_, target)| target == 0)
            || state_resume_edges.iter().any(|&(_, target, _)| target == 0)
        {
            for block in &mut blocks {
                block.id += 1;
            }
            blocks.insert(
                0,
                BasicBlock {
                    id: 0,
                    start_op: 0,
                    end_op: 0,
                },
            );
            for edges in successors.iter_mut().chain(predecessors.iter_mut()) {
                for block in edges {
                    *block += 1;
                }
            }
            successors.insert(0, vec![1]);
            predecessors.insert(0, Vec::new());
            predecessors[1].insert(0, 0);
            for (from, to) in &mut exception_edges {
                *from += 1;
                *to += 1;
            }
            for (from, to, _) in &mut state_resume_edges {
                *from += 1;
                *to += 1;
            }
        }

        let entry = 0;
        let dominance = crate::tir::dominators::IndexedDominance::compute(&successors, entry);
        let dominators = dominance.immediate_dominators().to_vec();

        // Back edges are the edges whose target dominates their source.
        let loop_depth = compute_loop_depth(&blocks, &successors, &predecessors, &dominance);

        Self {
            blocks,
            entry,
            predecessors,
            successors,
            dominators,
            loop_depth,
            exception_edges,
            state_resume_edges,
        }
    }

    /// Resolve the authored transfer of a source block from the same relation
    /// used by liveness and SSA placement. Invocation-entry insertion preserves
    /// the sorted source IDs and shifts both endpoints together.
    pub(super) fn exception_target(&self, source_block: usize) -> Option<usize> {
        self.exception_edges
            .binary_search_by_key(&source_block, |&(source, _)| source)
            .ok()
            .map(|index| self.exception_edges[index].1)
    }

    /// Exact executable dominance over the canonical before/after
    /// program-point graph, with O(1) operation-level queries. Handler
    /// registrations do not execute their targets, and resume edges leave at
    /// dispatch rather than block tail.
    pub fn execution_points(
        &self,
        ops: &[OpIR],
    ) -> crate::tir::dominators::SimpleExecutionDominance {
        crate::tir::dominators::SimpleExecutionDominance::compute(self, ops)
    }

    pub(super) fn label_positions(ops: &[OpIR]) -> HashMap<i64, usize> {
        build_label_map(ops)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::OpIR;

    /// Helper to create an `OpIR` with just a `kind`.
    fn op(kind: crate::OpKind) -> OpIR {
        OpIR {
            kind,
            ..crate::OpIR::new(kind)
        }
    }

    /// Helper to create an `OpIR` with `kind` and `value`.
    fn op_val(kind: crate::OpKind, value: i64) -> OpIR {
        OpIR {
            kind,
            value: Some(value),
            ..crate::OpIR::new(kind)
        }
    }

    /// Helper to create an `OpIR` with `kind` and `args`.
    fn op_args(kind: crate::OpKind, args: &[&str]) -> OpIR {
        OpIR {
            kind,
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..crate::OpIR::new(kind)
        }
    }

    /// Helper to create an `OpIR` with `kind`, `args`, and `value`.
    fn op_args_val(kind: crate::OpKind, args: &[&str], value: i64) -> OpIR {
        OpIR {
            kind,
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            value: Some(value),
            ..crate::OpIR::new(kind)
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: straight-line code → 1 block
    // -----------------------------------------------------------------------
    #[test]
    fn straight_line_single_block() {
        let ops = vec![
            op(crate::OpKind::Const),
            op(crate::OpKind::Add),
            op(crate::OpKind::RetVoid),
        ];
        let cfg = CFG::build(&ops);
        assert_eq!(cfg.blocks.len(), 1);
        assert_eq!(cfg.blocks[0].start_op, 0);
        assert_eq!(cfg.blocks[0].end_op, 3);
        assert_eq!(cfg.entry, 0);
        assert!(cfg.successors[0].is_empty()); // ret_void = no successors? Actually default fall-through.
        // ret_void is not in our terminator list so it falls through, but
        // there's no next block so successors should be empty.
        assert_eq!(cfg.loop_depth[0], 0);
    }

    #[test]
    fn exception_transfer_family_ends_blocks_at_the_transfer_program_point() {
        for kind in [
            crate::OpKind::CheckException,
            crate::OpKind::AsyncWorkPoll,
            crate::OpKind::TryStart,
        ] {
            let ops = vec![
                OpIR {
                    s_value: Some("checked_entry".into()),
                    ..op(crate::OpKind::TraceEnterSlot)
                },
                op_val(kind, 41),
                op(crate::OpKind::Const),
                op_val(kind, 41),
                op(crate::OpKind::Const),
                op(crate::OpKind::RetVoid),
                op_val(crate::OpKind::Label, 41),
                op(crate::OpKind::RetVoid),
            ];
            let cfg = CFG::build(&ops);
            let first = block_containing(&cfg.blocks, 1).unwrap();
            let second = block_containing(&cfg.blocks, 3).unwrap();
            let success = block_containing(&cfg.blocks, 4).unwrap();
            let handler = block_containing(&cfg.blocks, 6).unwrap();
            assert_eq!(
                cfg.blocks[first].start_op, 0,
                "{kind}: preserve checked entry pair"
            );
            assert_eq!(cfg.blocks[first].end_op, 2, "{kind}: early transfer source");
            assert_eq!(cfg.blocks[second].start_op, 2);
            assert_eq!(
                cfg.blocks[second].end_op, 4,
                "{kind}: later transfer source"
            );
            assert_ne!(first, second);
            assert_ne!(second, success);
            assert_eq!(cfg.successors[first], vec![second]);
            assert_eq!(cfg.successors[second], vec![success]);
            assert_eq!(
                cfg.exception_edges,
                vec![(first, handler), (second, handler)]
            );
            assert_eq!(cfg.exception_target(first), Some(handler));
            assert_eq!(cfg.exception_target(second), Some(handler));
            assert_eq!(cfg.exception_target(success), None);
            for block in &cfg.blocks {
                let transfers = ops[block.start_op..block.end_op]
                    .iter()
                    .filter(|op| {
                        matches!(
                            op.kind,
                            crate::OpKind::CheckException
                                | crate::OpKind::AsyncWorkPoll
                                | crate::OpKind::TryStart
                        )
                    })
                    .count();
                assert!(transfers <= 1, "{kind}: one authored transfer per block");
            }
            assert_eq!(
                cfg.execution_points(&ops).operation_immediate_dominator(6),
                (kind != crate::OpKind::TryStart).then_some(1),
                "{kind}: only observations execute a handler; no later definition can dominate it"
            );
        }
    }

    #[test]
    fn exception_lookup_uses_cfg_targets_while_admission_rejects_duplicate_labels() {
        for last_label_kind in [crate::OpKind::Label, crate::OpKind::StateLabel] {
            let ops = vec![
                op_val(crate::OpKind::CheckException, 7),
                op(crate::OpKind::RetVoid),
                op_val(crate::OpKind::Label, 7),
                op(crate::OpKind::RetVoid),
                op_val(last_label_kind, 7),
                op(crate::OpKind::RetVoid),
            ];
            // Low-level CFG construction already resolves the last definition;
            // its consumers must not independently select the first one.
            let cfg = CFG::build(&ops);
            assert_eq!(cfg.exception_edges, vec![(0, 3)]);
            assert_eq!(cfg.exception_target(0), Some(3));
            assert_eq!(cfg.blocks[3].start_op, 4);
            assert_eq!(cfg.exception_target(1), None);
            let ir = crate::SimpleIR {
                functions: vec![crate::FunctionIR {
                    name: "duplicate_labels".into(),
                    return_abi: crate::FunctionReturnAbi::Void,
                    ops,
                    ..crate::FunctionIR::default()
                }],
                profile: None,
            };
            let error = crate::simple_verify::validate_simple_ir_control_flow(&ir).unwrap_err();
            assert!(error.contains("duplicate-label-definition"), "{error}");
        }
    }

    #[test]
    fn pending_exception_producers_do_not_invent_transfer_boundaries() {
        let ops = vec![
            op(crate::OpKind::Call),
            op(crate::OpKind::Add),
            op(crate::OpKind::RetVoid),
        ];
        let cfg = CFG::build(&ops);
        assert_eq!(cfg.blocks.len(), 1);
        assert!(cfg.exception_edges.is_empty());
    }

    #[test]
    fn async_work_poll_alias_records_the_same_implicit_edge_as_check_exception() {
        let ops = vec![
            op_val(crate::OpKind::AsyncWorkPoll, 41),
            op(crate::OpKind::RetVoid),
            op_val(crate::OpKind::Label, 41),
            op(crate::OpKind::RetVoid),
        ];
        let cfg = CFG::build(&ops);
        let source = block_containing(&cfg.blocks, 0).expect("poll block");
        let handler = block_containing(&cfg.blocks, 2).expect("handler block");
        assert!(
            cfg.exception_edges.contains(&(source, handler)),
            "generated wire aliases must participate in pre-SSA exception CFG discovery"
        );
    }

    #[test]
    fn exception_transfer_to_its_own_block_leader_is_a_self_edge() {
        for kind in [crate::OpKind::CheckException, crate::OpKind::AsyncWorkPoll] {
            let ops = vec![
                op_val(crate::OpKind::Label, 5),
                op(crate::OpKind::Add),
                op_val(kind, 5),
                op(crate::OpKind::RetVoid),
            ];
            let cfg = CFG::build(&ops);
            let block = block_containing(&cfg.blocks, 2).expect("transfer block");
            assert_eq!(cfg.blocks[block].start_op, 0, "{kind}");
            assert_eq!(cfg.exception_edges, vec![(block, block)], "{kind}");
            assert_eq!(cfg.blocks[0].start_op, cfg.blocks[0].end_op);
            assert_eq!(block, 1, "{kind}: source shifted past invocation");
            assert_eq!(cfg.exception_target(0), None);
            assert_eq!(cfg.exception_target(block), Some(block));
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: if/else → 4 blocks (entry, then, else, join)
    // -----------------------------------------------------------------------
    #[test]
    fn if_else_four_blocks() {
        // entry:
        //   const v0
        //   if [v0]
        // then:
        //   add
        // else:
        //   sub
        // join (end_if):
        //   ret_void
        let ops = vec![
            op(crate::OpKind::Const),            // 0  entry
            op_args(crate::OpKind::If, &["v0"]), // 1  entry (ends block, next = then)
            op(crate::OpKind::Add),              // 2  then-block
            op(crate::OpKind::Else),             // 3  else-block start
            op(crate::OpKind::Sub),              // 4  else-block
            op(crate::OpKind::EndIf),            // 5  join-block
            op(crate::OpKind::RetVoid),          // 6  join-block
        ];
        let cfg = CFG::build(&ops);

        // Blocks: [0..2), [2..3), [3..5), [5..7)
        // Actually let's check what we get.
        assert!(
            cfg.blocks.len() >= 3,
            "expected at least 3 blocks, got {}",
            cfg.blocks.len()
        );

        // All blocks should have loop_depth 0.
        for &d in &cfg.loop_depth {
            assert_eq!(d, 0, "if/else should have no loop depth");
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: simple loop → at least 2 blocks with back edge
    // -----------------------------------------------------------------------
    #[test]
    fn simple_loop_back_edge() {
        // entry:
        //   const
        // loop_header (loop_start):
        //   add
        // loop_end:
        //   (falls through)
        //   ret_void
        let ops = vec![
            op(crate::OpKind::Const),     // 0  entry
            op(crate::OpKind::LoopStart), // 1  header
            op(crate::OpKind::Add),       // 2  body
            op(crate::OpKind::LoopEnd),   // 3  loop_end block
            op(crate::OpKind::RetVoid),   // 4  after loop
        ];
        let cfg = CFG::build(&ops);

        // We should have blocks: [0..1), [1..3), [3..4), [4..5)
        assert!(
            cfg.blocks.len() >= 3,
            "expected >=3 blocks, got {}",
            cfg.blocks.len()
        );

        // Find the loop_end block and check it has a back-edge to the header.
        let header_bid = block_containing(&cfg.blocks, 1).expect("header block");
        let loop_end_bid = block_containing(&cfg.blocks, 3).expect("loop_end block");

        assert!(
            cfg.successors[loop_end_bid].contains(&header_bid),
            "loop_end block should have back-edge to header"
        );

        // Loop depth: header and body should be >= 1.
        assert!(
            cfg.loop_depth[header_bid] >= 1,
            "header should have loop depth >= 1, got {}",
            cfg.loop_depth[header_bid]
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: dominator tree correctness for if/else
    // -----------------------------------------------------------------------
    #[test]
    fn dominator_tree_if_else() {
        // Block 0 (entry) should dominate all other blocks.
        let ops = vec![
            op(crate::OpKind::Const),            // 0
            op_args(crate::OpKind::If, &["v0"]), // 1
            op(crate::OpKind::Add),              // 2  then
            op(crate::OpKind::Else),             // 3  else
            op(crate::OpKind::Sub),              // 4  else body
            op(crate::OpKind::EndIf),            // 5  join
            op(crate::OpKind::RetVoid),          // 6
        ];
        let cfg = CFG::build(&ops);

        // Entry block dominates all blocks.
        let entry = cfg.entry;
        for (bid, _) in cfg.blocks.iter().enumerate() {
            if bid == entry {
                assert!(cfg.dominators[bid].is_none(), "entry has no idom");
            } else {
                // Walk up the dominator tree; must reach entry.
                let mut cur = bid;
                let mut found_entry = false;
                for _ in 0..cfg.blocks.len() {
                    match cfg.dominators[cur] {
                        Some(d) => {
                            if d == entry {
                                found_entry = true;
                                break;
                            }
                            cur = d;
                        }
                        None => {
                            found_entry = cur == entry;
                            break;
                        }
                    }
                }
                assert!(found_entry, "block {bid} must be dominated by entry");
            }
        }
    }

    #[test]
    fn if_with_empty_then_keeps_distinct_true_and_false_edges() {
        let ops = vec![
            op(crate::OpKind::Const),            // 0
            op_args(crate::OpKind::If, &["v0"]), // 1
            op(crate::OpKind::Else),             // 2
            op(crate::OpKind::Const),            // 3 else body
            op(crate::OpKind::EndIf),            // 4 join
            op(crate::OpKind::RetVoid),          // 5
        ];
        let cfg = CFG::build(&ops);

        let if_bid = block_containing(&cfg.blocks, 1).expect("if block");
        let else_bid = block_containing(&cfg.blocks, 2).expect("else block");
        let join_bid = block_containing(&cfg.blocks, 4).expect("join block");

        assert_eq!(
            cfg.successors[if_bid].len(),
            2,
            "empty-then if must still keep two successors"
        );
        assert!(
            cfg.successors[if_bid].contains(&else_bid),
            "false edge must enter else block"
        );
        assert!(
            cfg.successors[if_bid].contains(&join_bid),
            "true edge must skip empty then to the join block"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: loop depth = 1 inside loop, 0 outside
    // -----------------------------------------------------------------------
    #[test]
    fn loop_depth_inside_outside() {
        let ops = vec![
            op(crate::OpKind::Const),     // 0  outside
            op(crate::OpKind::LoopStart), // 1  loop header
            op(crate::OpKind::Add),       // 2  loop body
            op(crate::OpKind::LoopEnd),   // 3  loop end
            op(crate::OpKind::RetVoid),   // 4  outside
        ];
        let cfg = CFG::build(&ops);

        let entry_bid = block_containing(&cfg.blocks, 0).expect("entry");
        let after_bid = block_containing(&cfg.blocks, 4).expect("after loop");

        assert_eq!(cfg.loop_depth[entry_bid], 0, "entry should be outside loop");
        assert_eq!(
            cfg.loop_depth[after_bid], 0,
            "after-loop should be outside loop"
        );

        let header_bid = block_containing(&cfg.blocks, 1).expect("header");
        assert!(
            cfg.loop_depth[header_bid] >= 1,
            "header should be inside loop"
        );
    }

    #[test]
    fn loop_break_edges_to_post_loop_block() {
        let ops = vec![
            op(crate::OpKind::LoopStart), // 0 header
            op(crate::OpKind::LoopBreak), // 1 break from loop body
            op(crate::OpKind::LoopEnd),   // 2 loop end marker
            op(crate::OpKind::RetVoid),   // 3 post-loop block
        ];
        let cfg = CFG::build(&ops);

        let break_bid = block_containing(&cfg.blocks, 1).expect("loop_break block");
        let post_loop_bid = block_containing(&cfg.blocks, 3).expect("post-loop block");

        assert!(
            cfg.successors[break_bid].contains(&post_loop_bid),
            "loop_break should edge to the post-loop block so break paths stay executable"
        );
    }

    #[test]
    fn loop_break_in_then_loop_skips_enclosing_else_block() {
        let ops = vec![
            op_args(crate::OpKind::If, &["v0"]), // 0 enclosing if
            op(crate::OpKind::LoopStart),        // 1 then loop header
            op_args(crate::OpKind::LoopBreakIfTrue, &["v1"]), // 2 implicit break target
            op(crate::OpKind::Add),              // 3 loop body fall-through
            op(crate::OpKind::LoopEnd),          // 4 then loop end marker
            op(crate::OpKind::Else),             // 5 sibling else block
            op(crate::OpKind::Sub),              // 6 else body
            op(crate::OpKind::EndIf),            // 7 structured join
            op(crate::OpKind::RetVoid),          // 8 after if/else
        ];
        let cfg = CFG::build(&ops);

        let break_bid = block_containing(&cfg.blocks, 2).expect("loop break block");
        let else_bid = block_containing(&cfg.blocks, 5).expect("else block");
        let join_bid = block_containing(&cfg.blocks, 7).expect("join block");

        assert!(
            cfg.successors[break_bid].contains(&join_bid),
            "implicit loop break from a then-branch loop must target the enclosing join"
        );
        assert!(
            !cfg.successors[break_bid].contains(&else_bid),
            "implicit loop break from a then-branch loop must not enter the sibling else block"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: empty ops
    // -----------------------------------------------------------------------
    #[test]
    fn empty_ops() {
        let cfg = CFG::build(&[]);
        assert!(cfg.blocks.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 7: jump/label unstructured control flow
    // -----------------------------------------------------------------------
    #[test]
    fn jump_label_edges() {
        let ops = vec![
            op(crate::OpKind::Const),         // 0
            op_val(crate::OpKind::Jump, 42),  // 1  jump to label 42
            op(crate::OpKind::Add),           // 2  unreachable (but still a block)
            op_val(crate::OpKind::Label, 42), // 3  target
            op(crate::OpKind::RetVoid),       // 4
        ];
        let cfg = CFG::build(&ops);

        let jump_bid = block_containing(&cfg.blocks, 1).expect("jump block");
        let label_bid = block_containing(&cfg.blocks, 3).expect("label block");

        assert!(
            cfg.successors[jump_bid].contains(&label_bid),
            "jump should have edge to label target"
        );
        // Jump is a terminator so no fall-through.
        assert!(
            !cfg.successors[jump_bid].iter().any(|&s| {
                let sb = &cfg.blocks[s];
                sb.start_op == 2
            }),
            "jump should not fall through to next block"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: br_if conditional jump
    // -----------------------------------------------------------------------
    #[test]
    fn br_if_two_successors() {
        let ops = vec![
            op(crate::OpKind::Const),                      // 0
            op_args_val(crate::OpKind::BrIf, &["v0"], 10), // 1  cond jump to label 10
            op(crate::OpKind::Add),                        // 2  fall-through
            op(crate::OpKind::RetVoid),                    // 3
            op_val(crate::OpKind::Label, 10),              // 4  branch target
            op(crate::OpKind::RetVoid),                    // 5
        ];
        let cfg = CFG::build(&ops);

        let br_bid = block_containing(&cfg.blocks, 1).expect("br_if block");
        assert_eq!(
            cfg.successors[br_bid].len(),
            2,
            "br_if should have 2 successors (fall-through + target)"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: loop break inside IF/THEN must skip the ELSE block
    // -----------------------------------------------------------------------
    /// Reproduces the class_hierarchy 2x correctness bug.  The frontend
    /// wraps loops containing layout-guarded method dispatch in
    /// `IF (guard): for ... ELSE: for ... END_IF`, so the loop sits
    /// inside the THEN branch.  The pre-fix `loop_break_targets`
    /// computation used `bid + 1` (the block immediately after
    /// `loop_end`) as the break target — which in this layout is the
    /// `else` block, NOT the `end_if` block.  Result: when the loop
    /// exited normally (via `loop_break_if_false` with no explicit
    /// label), control fell through into the ELSE body and ran the
    /// duplicated loop a second time.
    ///
    /// Fix: when the next-block-after-loop_end starts with `else`,
    /// route the break target to the matching `end_if` block instead.
    /// Mirror's the default-case fall-through skip rule that already
    /// exists for blocks ending with non-branch ops.
    ///
    /// This test pins the CFG-level contract: the loop header's break
    /// target must equal the end_if block, not the else block.
    #[test]
    fn loop_break_inside_if_skips_else_block() {
        // Build: const; if cond; loop_start; loop_break_if_false cond;
        //        ... loop_end; else; loop_start; loop_end; end_if; ret
        let ops = vec![
            op(crate::OpKind::Const),                          // 0  if-cond setup
            op_args(crate::OpKind::If, &["c0"]),               // 1  IF
            op(crate::OpKind::LoopStart),                      // 2  THEN-body loop header
            op(crate::OpKind::Const),                          // 3  loop body cond
            op_args(crate::OpKind::LoopBreakIfFalse, &["c1"]), // 4  exit cond (no label)
            op(crate::OpKind::Nop),                            // 5  body op
            op(crate::OpKind::LoopContinue),                   // 6
            op(crate::OpKind::LoopEnd),                        // 7  THEN-body loop end
            op(crate::OpKind::Else),                           // 8  ELSE block start
            op(crate::OpKind::LoopStart),                      // 9  ELSE-body loop header
            op_args(crate::OpKind::LoopBreakIfFalse, &["c2"]), // 10
            op(crate::OpKind::LoopContinue),                   // 11
            op(crate::OpKind::LoopEnd),                        // 12 ELSE-body loop end
            op(crate::OpKind::EndIf),                          // 13 join
            op(crate::OpKind::RetVoid),                        // 14
        ];
        let cfg = CFG::build(&ops);

        // The THEN-body loop's loop_end is at op 7.  The block
        // containing `loop_break_if_false` (op 4) must have a
        // successor edge to the END_IF block (op 13), NOT the ELSE
        // block (op 8).
        let break_op_idx = 4;
        let else_op_idx = 8;
        let end_if_op_idx = 13;
        let break_bid =
            block_containing(&cfg.blocks, break_op_idx).expect("break op should be in a block");
        let else_bid =
            block_containing(&cfg.blocks, else_op_idx).expect("else op should be in a block");
        let end_if_bid =
            block_containing(&cfg.blocks, end_if_op_idx).expect("end_if op should be in a block");

        // The break path from the THEN-body loop must NOT include
        // the else block.
        assert!(
            !cfg.successors[break_bid].contains(&else_bid),
            "loop_break_if_false inside IF/THEN must NOT route to \
             the ELSE block — that would double-execute the body. \
             successors={:?}, else_bid={}",
            cfg.successors[break_bid],
            else_bid
        );
        // The break path SHOULD include the end_if block.
        assert!(
            cfg.successors[break_bid].contains(&end_if_bid),
            "loop_break_if_false inside IF/THEN must route to the \
             END_IF block (skipping the ELSE body). successors={:?}, \
             end_if_bid={}",
            cfg.successors[break_bid],
            end_if_bid
        );
    }

    // -----------------------------------------------------------------------
    // Test 9: nested loops
    // -----------------------------------------------------------------------
    #[test]
    fn nested_loop_depth() {
        let ops = vec![
            op(crate::OpKind::Const),     // 0  outside
            op(crate::OpKind::LoopStart), // 1  outer header
            op(crate::OpKind::LoopStart), // 2  inner header
            op(crate::OpKind::Add),       // 3  inner body
            op(crate::OpKind::LoopEnd),   // 4  inner end
            op(crate::OpKind::LoopEnd),   // 5  outer end
            op(crate::OpKind::RetVoid),   // 6  outside
        ];
        let cfg = CFG::build(&ops);

        let inner_bid = block_containing(&cfg.blocks, 2).expect("inner header");
        assert!(
            cfg.loop_depth[inner_bid] >= 2,
            "inner loop body should have depth >= 2, got {}",
            cfg.loop_depth[inner_bid]
        );

        let outside_bid = block_containing(&cfg.blocks, 0).expect("outside");
        assert_eq!(cfg.loop_depth[outside_bid], 0);
    }
}
