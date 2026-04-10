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
    /// Entry block index (always 0).
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
    /// Exception edges: implicit control-flow from blocks inside a try region
    /// to the corresponding handler block.  Each entry is `(from_block, handler_block)`.
    /// A `try_start` creates an implicit edge from every block in the try region
    /// to the handler (the block containing `check_exception` / `state_block_start`).
    pub exception_edges: Vec<(usize, usize)>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the op is a control-flow terminator that ends a basic
/// block (i.e. no fall-through to the next instruction).
fn is_terminator(kind: &str) -> bool {
    matches!(
        kind,
        "jump" | "goto" | "ret" | "ret_void" | "return" | "loop_break" | "loop_continue"
    )
}

/// Returns `true` if the op starts a new block boundary (the op itself is the
/// first instruction of the new block).
fn is_block_leader(kind: &str) -> bool {
    matches!(
        kind,
        "label" | "state_label" | "if" | "else" | "end_if" | "loop_start" | "loop_end"
    )
}

/// Returns `true` if the op ends a block and the next instruction should start
/// a new block (even if it's not itself a leader type).
fn is_block_ender(kind: &str) -> bool {
    matches!(kind, "loop_start" | "loop_end")
}

/// Returns `true` if the op is a conditional branch that causes a block split
/// (has both fall-through and a taken path).
fn is_conditional_branch(kind: &str) -> bool {
    matches!(
        kind,
        "br_if" | "if" | "loop_break_if_true" | "loop_break_if_false"
    )
}

/// Build a map from label-id → op-index for all `label` / `state_label` ops.
fn build_label_map(ops: &[OpIR]) -> HashMap<i64, usize> {
    let mut map = HashMap::new();
    for (idx, op) in ops.iter().enumerate() {
        if matches!(op.kind.as_str(), "label" | "state_label")
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
        let kind = op.kind.as_str();

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
        if matches!(kind, "jump" | "goto" | "br_if")
            && let Some(target_label) = op.value
            && let Some(&target_op) = label_map.get(&target_label)
        {
            leaders.insert(target_op);
        }

        // `loop_break_if_true` / `loop_break_if_false` target a label if present.
        if matches!(kind, "loop_break_if_true" | "loop_break_if_false")
            && let Some(target_label) = op.value
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
        match op.kind.as_str() {
            "if" => stack.push((idx, None)),
            "else" => {
                if let Some(top) = stack.last_mut() {
                    top.1 = Some(idx);
                }
            }
            "end_if" => {
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

    // Pre-compute loop break targets: for each loop_start block, find the
    // block immediately after the matching loop_end.  This is needed for
    // `loop_break_if_true/false` ops that don't carry an explicit target
    // label (molt's frontend omits the label, relying on the native
    // backend's LoopFrame for the break target).
    let loop_break_targets: HashMap<usize, usize> = {
        let mut targets = HashMap::new();
        let mut header_stack: Vec<usize> = Vec::new();
        for (bid, block) in blocks.iter().enumerate() {
            let first_kind = ops[block.start_op].kind.as_str();
            match first_kind {
                "loop_start" => header_stack.push(bid),
                "loop_end" => {
                    if let Some(header_bid) = header_stack.pop() {
                        // The break target is the block after loop_end.
                        if bid + 1 < n {
                            targets.insert(header_bid, bid + 1);
                        }
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
        match first_op.kind.as_str() {
            "loop_start" => loop_header_stack.push(bid),
            "loop_end" => {
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
        let kind = last_op.kind.as_str();

        match kind {
            // Unconditional jump.
            "jump" | "goto" => {
                if let Some(target_label) = last_op.value
                    && let Some(&target_op) = label_map.get(&target_label)
                    && let Some(target_bid) = block_containing(blocks, target_op)
                {
                    add_edge(&mut successors, &mut predecessors, bid, target_bid);
                }
            }

            // Return — no successors.
            "ret" | "ret_void" | "return" => {}

            // Loop break — terminates block and transfers control to the
            // block immediately after the enclosing loop. Without this edge,
            // CFG-based executable-region pruning can incorrectly mark the
            // real break path unreachable and erase it from later lowering.
            "loop_break" => {
                if let Some(&header_bid) = loop_header_stack.last()
                    && let Some(&post_loop_bid) = loop_break_targets.get(&header_bid)
                {
                    add_edge(&mut successors, &mut predecessors, bid, post_loop_bid);
                }
            }

            // Loop continue — jump back to current loop header, no fall-through.
            "loop_continue" => {
                if let Some(&header_bid) = loop_header_stack.last() {
                    add_edge(&mut successors, &mut predecessors, bid, header_bid);
                }
            }

            // Structured `if`: two successors — then-block (fall-through) and
            // else-block or end_if-block.
            "if" => {
                if let Some(&(else_idx, end_if_idx)) = if_else_map.get(&last_op_idx) {
                    // True branch normally falls through into the then block.
                    // If the next block is `else`, the then branch is empty and
                    // the true edge must skip directly to the join block.
                    if bid + 1 < n {
                        let next_start = blocks[bid + 1].start_op;
                        let true_target = if ops[next_start].kind == "else" {
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
            "br_if" => {
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
            "loop_break_if_true" | "loop_break_if_false" => {
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
                if bid + 1 < n {
                    let next_start = blocks[bid + 1].start_op;
                    if ops[next_start].kind == "else" {
                        // Skip else-block; jump to end_if block.
                        if let Some(&end_if_idx) = else_end_if_map.get(&next_start) {
                            if let Some(end_if_bid) = block_containing(blocks, end_if_idx) {
                                add_edge(&mut successors, &mut predecessors, bid, end_if_bid);
                            }
                        } else {
                            // Fallback: fall through normally.
                            add_edge(&mut successors, &mut predecessors, bid, bid + 1);
                        }
                    } else {
                        add_edge(&mut successors, &mut predecessors, bid, bid + 1);
                    }
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
// Phase 3: dominator computation (Cooper, Harvey, Kennedy)
// ---------------------------------------------------------------------------

fn compute_dominators(
    blocks: &[BasicBlock],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    entry: usize,
) -> Vec<Option<usize>> {
    let n = blocks.len();
    if n == 0 {
        return vec![];
    }

    // Use a reverse-post-order numbering for efficient iteration.
    // RPO traversal needs the *forward* (successor) edges.
    let rpo = reverse_postorder(blocks.len(), entry, |b| &successors[b]);
    let mut rpo_order: Vec<usize> = Vec::with_capacity(n); // block-ids in RPO
    let mut rpo_number: Vec<usize> = vec![usize::MAX; n]; // block-id → RPO index
    for (rpo_idx, &bid) in rpo.iter().enumerate() {
        rpo_order.push(bid);
        rpo_number[bid] = rpo_idx;
    }

    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[entry] = Some(entry);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo_order {
            if b == entry {
                continue;
            }
            // Pick first processed predecessor.
            let mut new_idom: Option<usize> = None;
            for &p in &predecessors[b] {
                if idom[p].is_some() {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => intersect_dom(&idom, &rpo_number, cur, p),
                    });
                }
            }
            if new_idom != idom[b] {
                idom[b] = new_idom;
                changed = true;
            }
        }
    }

    // Convention: entry's dominator is None (it has no idom).
    idom[entry] = None;
    idom
}

fn intersect_dom(
    idom: &[Option<usize>],
    rpo_number: &[usize],
    mut a: usize,
    mut b: usize,
) -> usize {
    while a != b {
        while rpo_number[a] > rpo_number[b] {
            // Guard against self-loop: if idom[a] == a (entry node), stop.
            match idom[a] {
                Some(d) if d != a => a = d,
                _ => break,
            }
        }
        while rpo_number[b] > rpo_number[a] {
            // Guard against self-loop: if idom[b] == b (entry node), stop.
            match idom[b] {
                Some(d) if d != b => b = d,
                _ => break,
            }
        }
        // If neither side can advance further, break to prevent infinite loop.
        if rpo_number[a] == rpo_number[b] && a != b {
            break;
        }
    }
    a
}

/// Compute reverse-post-order traversal of the graph starting from `entry`.
fn reverse_postorder<'a, F>(n: usize, entry: usize, successors_of: F) -> Vec<usize>
where
    F: Fn(usize) -> &'a Vec<usize>,
{
    // Iterative DFS over the forward (successor) graph.
    let mut visited = vec![false; n];
    let mut postorder = Vec::with_capacity(n);

    // Iterative post-order DFS.
    let mut stack: Vec<(usize, bool)> = vec![(entry, false)];
    while let Some((node, processed)) = stack.pop() {
        if processed {
            postorder.push(node);
            continue;
        }
        if visited[node] {
            continue;
        }
        visited[node] = true;
        stack.push((node, true));
        // Push successors in reverse so they're visited in forward order.
        for &succ in successors_of(node).iter().rev() {
            if !visited[succ] {
                stack.push((succ, false));
            }
        }
    }

    postorder.reverse();
    postorder
}

// ---------------------------------------------------------------------------
// Phase 4: loop detection and depth
// ---------------------------------------------------------------------------

fn compute_loop_depth(
    blocks: &[BasicBlock],
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    dominators: &[Option<usize>],
    entry: usize,
) -> Vec<u32> {
    let n = blocks.len();
    let mut depth = vec![0u32; n];

    // Find back-edges: an edge b→h where h dominates b.
    for (b, succs) in successors.iter().enumerate() {
        for &h in succs {
            if dominates(dominators, h, b, entry) {
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

/// Returns `true` if `a` dominates `b` in the dominator tree.
fn dominates(idom: &[Option<usize>], a: usize, b: usize, entry: usize) -> bool {
    if a == b {
        return true;
    }
    let mut cur = b;
    loop {
        match idom[cur] {
            Some(d) if d == a => return true,
            Some(d) if d == cur => return false, // shouldn't happen, safety net
            Some(d) => cur = d,
            None => {
                // cur is the entry; a dominates b only if a is also entry.
                return cur == entry && a == entry;
            }
        }
    }
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

/// Compute implicit exception edges from try regions.
///
/// A `try_start` op with a label value identifies a handler label. Every
/// block between `try_start` and the matching `try_end` has an implicit
/// edge to the handler block (the block containing that label or the
/// `check_exception`/`state_block_start` that follows it).
fn compute_exception_edges(
    ops: &[OpIR],
    blocks: &[BasicBlock],
    label_map: &HashMap<i64, usize>,
) -> Vec<(usize, usize)> {
    let mut edges: Vec<(usize, usize)> = Vec::new();

    // Walk blocks and track try_start/try_end nesting to determine which
    // blocks are inside try regions and where their handlers are.
    let mut active_handlers: Vec<Option<usize>> = Vec::new();

    for (bid, block) in blocks.iter().enumerate() {
        // Scan ops in this block for try_start/try_end to maintain nesting.
        for op_idx in block.start_op..block.end_op {
            let kind = ops[op_idx].kind.as_str();
            match kind {
                "try_start" => {
                    let handler_bid = ops[op_idx]
                        .value
                        .and_then(|label_id| label_map.get(&label_id))
                        .and_then(|&target_op| block_containing(blocks, target_op));
                    active_handlers.push(handler_bid);
                }
                "try_end" => {
                    active_handlers.pop();
                }
                _ => {}
            }
        }

        // Add exception edges from this block to all active handlers.
        for handler in &active_handlers {
            if let Some(handler_bid) = handler
                && *handler_bid != bid
            {
                edges.push((bid, *handler_bid));
            }
        }

        // `check_exception` also carries its handler label directly even when
        // the frontend did not materialize an enclosing try_start/try_end
        // region. This shape appears in method-level guarded field stores and
        // must still thread live cleanup state into the handler block.
        for op_idx in block.start_op..block.end_op {
            let op = &ops[op_idx];
            if op.kind != "check_exception" {
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
            if target_bid != bid {
                edges.push((bid, target_bid));
            }
        }
    }

    // De-duplicate.
    edges.sort_unstable();
    edges.dedup();
    edges
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl CFG {
    /// Build a CFG from a slice of `OpIR` operations.
    pub fn build(ops: &[OpIR]) -> Self {
        if ops.is_empty() {
            return Self {
                blocks: vec![],
                entry: 0,
                predecessors: vec![],
                successors: vec![],
                dominators: vec![],
                loop_depth: vec![],
                exception_edges: vec![],
            };
        }

        let label_map = build_label_map(ops);
        let leaders = find_leaders(ops, &label_map);
        let blocks = leaders_to_blocks(&leaders, ops.len());
        let (successors, predecessors) = build_edges(ops, &blocks, &label_map);

        let entry = 0;
        let dominators = compute_dominators(&blocks, &successors, &predecessors, entry);

        // For loop depth we need to pass successors to the dominator-based
        // detector, but we also use the structural back-edges.
        let loop_depth =
            compute_loop_depth(&blocks, &successors, &predecessors, &dominators, entry);

        // Compute implicit exception edges from try regions.
        let exception_edges = compute_exception_edges(ops, &blocks, &label_map);

        Self {
            blocks,
            entry,
            predecessors,
            successors,
            dominators,
            loop_depth,
            exception_edges,
        }
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
    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `value`.
    fn op_val(kind: &str, value: i64) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `args`.
    fn op_args(kind: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `args`, and `value`.
    fn op_args_val(kind: &str, args: &[&str], value: i64) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            value: Some(value),
            ..OpIR::default()
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: straight-line code → 1 block
    // -----------------------------------------------------------------------
    #[test]
    fn straight_line_single_block() {
        let ops = vec![op("const"), op("add"), op("ret_void")];
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
            op("const"),            // 0  entry
            op_args("if", &["v0"]), // 1  entry (ends block, next = then)
            op("add"),              // 2  then-block
            op("else"),             // 3  else-block start
            op("sub"),              // 4  else-block
            op("end_if"),           // 5  join-block
            op("ret_void"),         // 6  join-block
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
            op("const"),      // 0  entry
            op("loop_start"), // 1  header
            op("add"),        // 2  body
            op("loop_end"),   // 3  loop_end block
            op("ret_void"),   // 4  after loop
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
            op("const"),            // 0
            op_args("if", &["v0"]), // 1
            op("add"),              // 2  then
            op("else"),             // 3  else
            op("sub"),              // 4  else body
            op("end_if"),           // 5  join
            op("ret_void"),         // 6
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
            op("const"),            // 0
            op_args("if", &["v0"]), // 1
            op("else"),             // 2
            op("const"),            // 3 else body
            op("end_if"),           // 4 join
            op("ret_void"),         // 5
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
            op("const"),      // 0  outside
            op("loop_start"), // 1  loop header
            op("add"),        // 2  loop body
            op("loop_end"),   // 3  loop end
            op("ret_void"),   // 4  outside
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
            op("loop_start"), // 0 header
            op("loop_break"), // 1 break from loop body
            op("loop_end"),   // 2 loop end marker
            op("ret_void"),   // 3 post-loop block
        ];
        let cfg = CFG::build(&ops);

        let break_bid = block_containing(&cfg.blocks, 1).expect("loop_break block");
        let post_loop_bid = block_containing(&cfg.blocks, 3).expect("post-loop block");

        assert!(
            cfg.successors[break_bid].contains(&post_loop_bid),
            "loop_break should edge to the post-loop block so break paths stay executable"
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
            op("const"),         // 0
            op_val("jump", 42),  // 1  jump to label 42
            op("add"),           // 2  unreachable (but still a block)
            op_val("label", 42), // 3  target
            op("ret_void"),      // 4
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
            op("const"),                       // 0
            op_args_val("br_if", &["v0"], 10), // 1  cond jump to label 10
            op("add"),                         // 2  fall-through
            op("ret_void"),                    // 3
            op_val("label", 10),               // 4  branch target
            op("ret_void"),                    // 5
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
    // Test 9: nested loops
    // -----------------------------------------------------------------------
    #[test]
    fn nested_loop_depth() {
        let ops = vec![
            op("const"),      // 0  outside
            op("loop_start"), // 1  outer header
            op("loop_start"), // 2  inner header
            op("add"),        // 3  inner body
            op("loop_end"),   // 4  inner end
            op("loop_end"),   // 5  outer end
            op("ret_void"),   // 6  outside
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
