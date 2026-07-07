use std::collections::HashMap;

pub(super) fn indexed_track(index: &mut Vec<u64>, positions: &mut HashMap<u64, usize>, id: u64) {
    if positions.contains_key(&id) {
        return;
    }
    let pos = index.len();
    index.push(id);
    positions.insert(id, pos);
}

pub(super) fn indexed_untrack(
    index: &mut Vec<u64>,
    positions: &mut HashMap<u64, usize>,
    cursor: &mut usize,
    id: u64,
) {
    let Some(pos) = positions.remove(&id) else {
        return;
    };
    let last = index.len().saturating_sub(1);
    index.swap_remove(pos);
    if pos < last
        && let Some(moved) = index.get(pos).copied()
    {
        positions.insert(moved, pos);
    }
    if index.is_empty() || *cursor >= index.len() {
        *cursor = 0;
    }
}

pub(super) fn indexed_next_batch(index: &[u64], cursor: &mut usize, max_batch: usize) -> Vec<u64> {
    if index.is_empty() || max_batch == 0 {
        return Vec::new();
    }
    let batch = max_batch.min(index.len());
    let mut out = Vec::with_capacity(batch);
    for _ in 0..batch {
        if *cursor >= index.len() {
            *cursor = 0;
        }
        out.push(index[*cursor]);
        *cursor += 1;
        if *cursor >= index.len() {
            *cursor = 0;
        }
    }
    out
}
