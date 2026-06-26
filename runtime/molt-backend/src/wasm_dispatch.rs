use crate::OpIR;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, Function, Instruction};

const STATE_REMAP_TABLE_MAX_ENTRIES: usize = 4096;
const STATE_REMAP_TABLE_MAX_SPARSITY: usize = 8;
/// Minimum number of sparse remap entries before we attempt `br_table` dispatch.
const BR_TABLE_MIN_ENTRIES: usize = 5;

fn is_stateful_dispatch_terminator(kind: &str) -> bool {
    matches!(
        kind,
        "state_switch"
            | "state_transition"
            | "state_yield"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "if"
            | "else"
            | "end_if"
            | "loop_start"
            | "loop_index_start"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break"
            | "loop_continue"
            | "loop_end"
            | "jump"
            | "try_start"
            | "try_end"
            | "label"
            | "state_label"
            | "check_exception"
            | "ret"
            | "ret_void"
    )
}

pub(crate) fn has_non_linear_control_flow(ops: &[OpIR]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "if" | "else"
                | "end_if"
                | "loop_start"
                | "loop_index_start"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_break_if_exception"
                | "loop_break"
                | "loop_continue"
                | "loop_end"
                | "for_iter_start"
                | "for_iter_end"
                | "while_start"
                | "while_end"
                | "try_start"
                | "try_end"
                | "async_for_start"
                | "async_for_end"
                | "jump"
                | "br_if"
                | "label"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
                | "state_label"
                | "check_exception"
                | "ret"
                | "ret_void"
        )
    })
}

pub(crate) fn build_dispatch_blocks(ops: &[OpIR]) -> (Vec<usize>, Vec<usize>) {
    let op_count = ops.len();
    if op_count == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut is_start = vec![false; op_count];
    is_start[0] = true;
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" | "state_label" | "loop_start" | "loop_index_start" | "loop_end" => {
                is_start[idx] = true;
            }
            _ => {}
        }
        if is_stateful_dispatch_terminator(op.kind.as_str()) && idx + 1 < op_count {
            is_start[idx + 1] = true;
        }
    }

    let mut starts = Vec::new();
    for (idx, start) in is_start.iter().enumerate() {
        if *start {
            starts.push(idx);
        }
    }

    let mut block_for_op = vec![0; op_count];
    let mut block_idx = 0usize;
    let mut next_start = starts.get(1).copied().unwrap_or(op_count);
    for (idx, block_slot) in block_for_op.iter_mut().enumerate().take(op_count) {
        if idx == next_start {
            block_idx += 1;
            next_start = starts.get(block_idx + 1).copied().unwrap_or(op_count);
        }
        *block_slot = block_idx;
    }

    (starts, block_for_op)
}

pub(crate) fn build_dispatch_block_map(block_for_op: &[usize]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(block_for_op.len() * 4);
    for &block_idx in block_for_op {
        bytes.extend_from_slice(&(block_idx as u32).to_le_bytes());
    }
    bytes
}

#[derive(Default)]
pub(crate) struct DispatchControlMaps {
    pub(crate) label_to_index: BTreeMap<i64, usize>,
    pub(crate) else_for_if: BTreeMap<usize, usize>,
    pub(crate) end_for_if: BTreeMap<usize, usize>,
    pub(crate) end_for_else: BTreeMap<usize, usize>,
    pub(crate) loop_continue_target: BTreeMap<usize, usize>,
    pub(crate) loop_break_target: BTreeMap<usize, usize>,
}

pub(crate) fn dispatch_control_panic(
    function_name: &str,
    op_idx: usize,
    message: impl std::fmt::Display,
) -> ! {
    panic!("invalid WASM dispatch control in function `{function_name}` op {op_idx}: {message}")
}

pub(crate) fn build_dispatch_control_maps(
    ops: &[OpIR],
    include_state_labels: bool,
    function_name: &str,
) -> DispatchControlMaps {
    struct LoopFrame {
        start_idx: usize,
        break_ops: Vec<usize>,
    }

    let mut valid_labels: BTreeSet<i64> = BTreeSet::new();
    for op in ops {
        match op.kind.as_str() {
            "label" | "state_label" if include_state_labels => {
                if let Some(label_id) = op.value {
                    valid_labels.insert(label_id);
                }
            }
            "label" => {
                if let Some(label_id) = op.value {
                    valid_labels.insert(label_id);
                }
            }
            _ => {}
        }
    }

    let mut maps = DispatchControlMaps::default();
    let mut if_stack: Vec<usize> = Vec::new();
    let mut loop_stack: Vec<LoopFrame> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "jump" | "br_if" => {
                let Some(label_id) = op.value else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!("{} missing target label id", op.kind),
                    );
                };
                if !valid_labels.contains(&label_id) {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!(
                            "{} target label {} is not present in dispatch label map",
                            op.kind, label_id
                        ),
                    );
                }
            }
            "label" => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "state_label" if include_state_labels => {
                if let Some(label_id) = op.value {
                    maps.label_to_index.insert(label_id, idx);
                }
            }
            "if" => if_stack.push(idx),
            "else" => {
                let Some(if_idx) = if_stack.last().copied() else {
                    dispatch_control_panic(function_name, idx, "else without matching if");
                };
                maps.else_for_if.insert(if_idx, idx);
            }
            "end_if" => {
                let Some(if_idx) = if_stack.pop() else {
                    dispatch_control_panic(function_name, idx, "end_if without matching if");
                };
                maps.end_for_if.insert(if_idx, idx);
                if let Some(else_idx) = maps.else_for_if.get(&if_idx).copied() {
                    maps.end_for_else.insert(else_idx, idx);
                }
            }
            "loop_start" => {
                loop_stack.push(LoopFrame {
                    start_idx: idx,
                    break_ops: Vec::new(),
                });
            }
            "loop_index_start" => {
                // loop_index_start is always preceded by loop_start,
                // which already pushed a LoopFrame. Update the
                // start_idx to point here (the actual loop body start)
                // instead of pushing a duplicate frame.
                let Some(frame) = loop_stack.last_mut() else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        "loop_index_start without matching loop_start",
                    );
                };
                frame.start_idx = idx;
            }
            "loop_continue" => {
                let Some(frame) = loop_stack.last() else {
                    dispatch_control_panic(function_name, idx, "loop_continue without loop");
                };
                maps.loop_continue_target.insert(idx, frame.start_idx);
            }
            "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break" => {
                let Some(frame) = loop_stack.last_mut() else {
                    dispatch_control_panic(
                        function_name,
                        idx,
                        format_args!("{} without loop", op.kind),
                    );
                };
                frame.break_ops.push(idx);
            }
            "loop_end" => {
                let Some(frame) = loop_stack.pop() else {
                    dispatch_control_panic(function_name, idx, "loop_end without loop_start");
                };
                for break_idx in frame.break_ops {
                    maps.loop_break_target.insert(break_idx, idx);
                }
            }
            _ => {}
        }
    }
    if let Some(if_idx) = if_stack.last().copied() {
        dispatch_control_panic(function_name, if_idx, "if without matching end_if");
    }
    if let Some(frame) = loop_stack.last() {
        dispatch_control_panic(
            function_name,
            frame.start_idx,
            "loop_start without matching loop_end",
        );
    }

    maps
}

pub(crate) fn build_state_resume_maps(
    ops: &[OpIR],
) -> (BTreeMap<i64, usize>, BTreeMap<String, i64>) {
    let mut state_map: BTreeMap<i64, usize> = BTreeMap::new();
    state_map.insert(0, 0);
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "state_transition" | "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx + 1);
                }
            }
            "label" | "state_label" => {
                if let Some(state_id) = op.value {
                    state_map.insert(state_id, idx);
                }
            }
            "const" => {
                if let (Some(out), Some(value)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), value);
                }
            }
            _ => {}
        }
    }

    (state_map, const_ints)
}

pub(crate) fn build_dense_state_remap_table(state_map: &BTreeMap<i64, usize>) -> Option<Vec<u8>> {
    let mut non_negative_entries: Vec<(usize, i64)> = Vec::new();
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        let Ok(state_idx) = usize::try_from(state_id) else {
            return None;
        };
        non_negative_entries.push((state_idx, target_idx as i64));
    }
    if non_negative_entries.is_empty() {
        return None;
    }

    let max_state_idx = non_negative_entries
        .iter()
        .map(|(state_idx, _)| *state_idx)
        .max()?;
    let entry_count = max_state_idx.checked_add(1)?;
    if entry_count > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    if entry_count
        > non_negative_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }

    let mut table = vec![-1i64; entry_count];
    for (state_idx, target_idx) in non_negative_entries {
        table[state_idx] = target_idx;
    }
    let mut bytes = Vec::with_capacity(entry_count * std::mem::size_of::<i64>());
    for target_idx in table {
        bytes.extend_from_slice(&target_idx.to_le_bytes());
    }
    Some(bytes)
}

pub(crate) fn build_sparse_state_remap_entries(
    state_map: &BTreeMap<i64, usize>,
) -> Vec<(i64, i64)> {
    let mut entries = Vec::with_capacity(state_map.len());
    for (&state_id, &target_idx) in state_map {
        if state_id < 0 {
            continue;
        }
        entries.push((state_id, target_idx as i64));
    }
    entries.sort_unstable_by_key(|(state_id, _)| *state_id);
    entries
}

/// Check whether `sorted_entries` form a dense-enough range suitable for
/// `br_table` dispatch.  Returns `Some((min_state, table_size))` when the
/// sparsity ratio (table_size / entry_count) is within
/// `STATE_REMAP_TABLE_MAX_SPARSITY` and there are at least
/// `BR_TABLE_MIN_ENTRIES` entries.
pub(crate) fn br_table_state_remap_params(sorted_entries: &[(i64, i64)]) -> Option<(i64, usize)> {
    if sorted_entries.len() < BR_TABLE_MIN_ENTRIES {
        return None;
    }
    let min_state = sorted_entries.first()?.0;
    let max_state = sorted_entries.last()?.0;
    // table_size covers [min_state, max_state] inclusive.
    let table_size = (max_state - min_state + 1) as usize;
    if table_size
        > sorted_entries
            .len()
            .saturating_mul(STATE_REMAP_TABLE_MAX_SPARSITY)
    {
        return None;
    }
    if table_size > STATE_REMAP_TABLE_MAX_ENTRIES {
        return None;
    }
    Some((min_state, table_size))
}

/// Emit a `br_table`-based O(1) state remap lookup.
///
/// Structure emitted (N = number of remap targets + 1 default):
/// ```wasm
///   block $default          ;; depth 0 – fall-through = no remap
///     block $case_0         ;; depth 1
///       block $case_1       ;; depth 2
///         ...
///       block $case_{N-1}   ;; depth N
///         (local.get state_local)
///         (i64.const min_state)
///         (i64.sub)
///         (i32.wrap_i64)
///         br_table [targets...] $default
///       end  ;; $case_{N-1}
///       ;; set state_local = target for case N-1
///       br $default
///     ...
///   end  ;; $default
/// ```
fn emit_br_table_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
    min_state: i64,
    table_size: usize,
) {
    // Build a mapping from (state_id - min_state) -> target_idx.
    let mut slot_to_target: Vec<Option<i64>> = vec![None; table_size];
    for &(state_id, target_idx) in sorted_entries {
        let slot = (state_id - min_state) as usize;
        slot_to_target[slot] = Some(target_idx);
    }

    // Deduplicate targets to minimise block count: each unique target_idx
    // gets its own block.  Unmapped slots branch to the default (no-op).
    let mut unique_targets: Vec<i64> = sorted_entries.iter().map(|&(_, t)| t).collect();
    unique_targets.sort_unstable();
    unique_targets.dedup();
    let target_block_count = unique_targets.len(); // number of case blocks

    // Map target_idx -> index into unique_targets (0-based).
    let target_to_case: BTreeMap<i64, usize> = unique_targets
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, i))
        .collect();

    // Block nesting (outermost to innermost):
    //   block $default             depth 0 from br perspective
    //     block $case_0            depth 1
    //       block $case_1          depth 2
    //         ...
    //         block $case_{N-1}    depth N   (= target_block_count)
    //           br_table ...
    //         end $case_{N-1}
    //         <code for case N-1>
    //         br $default          (depth = target_block_count)
    //       end $case_{N-2}
    //       ...
    //     end $case_0
    //   end $default
    //
    // When `br_table` branches to label L, it targets block depth L from
    // the `br_table` instruction.  We want:
    //   - default (unmapped) -> depth 0 ($default, outermost) = skip remap
    //   - case_i             -> depth (target_block_count - i) so that
    //     after `end` of that block we land in code that sets state_local.

    let default_depth: u32 = target_block_count as u32; // reaches $default

    // Build br_table target vector: one entry per table slot.
    let br_targets: Vec<u32> = slot_to_target
        .iter()
        .map(|slot| match slot {
            Some(target_idx) => {
                let case_idx = target_to_case[target_idx];
                // case_idx 0 is outermost case block (depth 1 from br_table).
                // After br_table, we want to land *after* the end of
                // $case_{case_idx}.  The innermost block ($case_0) is at
                // depth target_block_count-1; each subsequent case is one
                // level further out.  So $case_{case_idx} sits at depth
                // (target_block_count - 1 - case_idx).
                (target_block_count - 1 - case_idx) as u32
            }
            None => default_depth,
        })
        .collect();

    // Emit blocks: $default, then $case_0 .. $case_{N-1}.
    func.instruction(&Instruction::Block(BlockType::Empty)); // $default
    for _ in 0..target_block_count {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    // Compute table index: (state_local - min_state), then i32.wrap.
    func.instruction(&Instruction::LocalGet(state_local));
    if min_state != 0 {
        func.instruction(&Instruction::I64Const(min_state));
        func.instruction(&Instruction::I64Sub);
    }
    func.instruction(&Instruction::I32WrapI64);

    // br_table dispatch.
    let targets_cow: Cow<[u32]> = br_targets.into();
    func.instruction(&Instruction::BrTable(targets_cow, default_depth));

    // Emit case bodies (innermost block ends first).
    // After `end $case_{N-1-i}`, we're inside $case_{N-2-i}, so we emit
    // the set + branch-to-default for case (N-1-i).
    for rev_i in 0..target_block_count {
        let case_idx = target_block_count - 1 - rev_i;
        func.instruction(&Instruction::End); // end $case_{case_idx}
        let target_idx = unique_targets[case_idx];
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        // Branch to $default to skip remaining cases.
        // Depth from here to $default = case_idx + 1 (because we just
        // closed one block).  Actually, after closing $case_{case_idx},
        // the remaining nesting depth above us is (case_idx) case blocks
        // + 1 default block.  We want to branch to $default which is the
        // outermost, so depth = case_idx.
        if rev_i < target_block_count - 1 {
            func.instruction(&Instruction::Br(case_idx as u32));
        }
        // For the last case (case_idx == 0), we fall through to $default's End.
    }

    func.instruction(&Instruction::End); // end $default
}

pub(crate) fn emit_sparse_state_remap_lookup(
    func: &mut Function,
    state_local: u32,
    sorted_entries: &[(i64, i64)],
) {
    // When the entries are dense enough, use br_table for O(1) dispatch.
    if let Some((min_state, table_size)) = br_table_state_remap_params(sorted_entries) {
        emit_br_table_state_remap_lookup(func, state_local, sorted_entries, min_state, table_size);
        return;
    }

    // Fallback: binary-search tree of nested if/else.
    fn emit_node(func: &mut Function, state_local: u32, entries: &[(i64, i64)]) {
        if entries.is_empty() {
            return;
        }

        let mid = entries.len() / 2;
        let (state_id, target_idx) = entries[mid];
        let left = &entries[..mid];
        let right = &entries[mid + 1..];

        func.instruction(&Instruction::LocalGet(state_local));
        func.instruction(&Instruction::I64Const(state_id));
        func.instruction(&Instruction::I64Eq);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::I64Const(target_idx));
        func.instruction(&Instruction::LocalSet(state_local));
        if !left.is_empty() || !right.is_empty() {
            func.instruction(&Instruction::Else);
            match (!left.is_empty(), !right.is_empty()) {
                (true, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::Else);
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (true, false) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64LtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, left);
                    func.instruction(&Instruction::End);
                }
                (false, true) => {
                    func.instruction(&Instruction::LocalGet(state_local));
                    func.instruction(&Instruction::I64Const(state_id));
                    func.instruction(&Instruction::I64GtS);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_node(func, state_local, right);
                    func.instruction(&Instruction::End);
                }
                (false, false) => {}
            }
        }
        func.instruction(&Instruction::End);
    }

    emit_node(func, state_local, sorted_entries);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str, value: Option<i64>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value,
            ..OpIR::default()
        }
    }

    #[test]
    fn dispatch_control_accepts_forward_jump_labels() {
        let maps = build_dispatch_control_maps(
            &[
                op("jump", Some(7)),
                op("const_none", None),
                op("label", Some(7)),
            ],
            false,
            "forward_jump",
        );

        assert_eq!(maps.label_to_index.get(&7), Some(&2));
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `missing_jump_label` op 0: jump missing target label id"
    )]
    fn dispatch_control_rejects_jump_without_label() {
        build_dispatch_control_maps(&[op("jump", None)], false, "missing_jump_label");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `orphan_jump_label` op 0: jump target label 99 is not present in dispatch label map"
    )]
    fn dispatch_control_rejects_unknown_jump_label() {
        build_dispatch_control_maps(&[op("jump", Some(99))], false, "orphan_jump_label");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `unbalanced_if` op 0: if without matching end_if"
    )]
    fn dispatch_control_rejects_unbalanced_if() {
        build_dispatch_control_maps(&[op("if", None)], false, "unbalanced_if");
    }

    #[test]
    #[should_panic(
        expected = "invalid WASM dispatch control in function `break_without_loop` op 0: loop_break without loop"
    )]
    fn dispatch_control_rejects_loop_break_without_loop() {
        build_dispatch_control_maps(&[op("loop_break", None)], false, "break_without_loop");
    }
}
