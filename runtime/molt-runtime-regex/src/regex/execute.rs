use super::*;

// ---------------------------------------------------------------------------
// Top-level match engine: execute a compiled pattern against text
// ---------------------------------------------------------------------------

/// Internal result of a successful match.
pub(super) struct MatchResult {
    pub(super) start: usize,
    pub(super) end: usize,
    /// Groups indexed 1..=group_count.  Index 0 is unused.
    pub(super) groups: Vec<Option<(usize, usize)>>,
}

/// Execute a compiled pattern in the given mode.
///
/// Returns `Some(MatchResult)` on success, `None` on no match.
pub(super) fn execute_match(
    compiled: &CompiledPattern,
    text: &str,
    pos: usize,
    end: usize,
    mode: &str,
) -> Option<MatchResult> {
    match mode {
        "match" => {
            // Anchored at start (pos), match from pos.
            let mut state = MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
            if pos > state.chars.len() || end > state.chars.len() {
                return None;
            }
            let result = try_match(&compiled.root, pos, &[], &mut state);
            result.map(|end_pos| MatchResult {
                start: pos,
                end: end_pos,
                groups: state.groups,
            })
        }
        "fullmatch" => {
            // Must match the entire text[pos..end].
            let mut state = MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
            if pos > state.chars.len() || end > state.chars.len() {
                return None;
            }
            let result = try_match(&compiled.root, pos, &[], &mut state);
            match result {
                Some(end_pos) if end_pos == end => Some(MatchResult {
                    start: pos,
                    end: end_pos,
                    groups: state.groups,
                }),
                _ => None,
            }
        }
        "search" => {
            // Search: try matching at each position from pos to end.
            let chars: Vec<char> = text.chars().collect();
            let text_len = chars.len();
            if pos > text_len || end > text_len {
                return None;
            }
            for start in pos..=end {
                let mut state =
                    MatchState::new(text, compiled.flags, compiled.group_count, pos, end);
                if let Some(end_pos) = try_match(&compiled.root, start, &[], &mut state) {
                    return Some(MatchResult {
                        start,
                        end: end_pos,
                        groups: state.groups,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

/// Build a MoltObject tuple representing the match result for the intrinsic
/// return value.
///
/// Format: `(match_start, match_end, groups_tuple)`
/// where `groups_tuple` is a tuple of `(start, end) | None` for each group.
pub(super) fn build_match_result_bits(
    _py: &CoreGilToken,
    result: &MatchResult,
    group_count: u32,
) -> u64 {
    let start_bits = MoltObject::from_int(result.start as i64).bits();
    let end_bits = MoltObject::from_int(result.end as i64).bits();

    // Build group spans tuple.
    let mut group_elems: Vec<u64> = Vec::with_capacity(group_count as usize);
    for i in 1..=(group_count as usize) {
        if i < result.groups.len() {
            match result.groups[i] {
                Some((gs, ge)) => {
                    let gs_bits = MoltObject::from_int(gs as i64).bits();
                    let ge_bits = MoltObject::from_int(ge as i64).bits();
                    let pair_ptr = alloc_tuple(_py, &[gs_bits, ge_bits]);
                    if pair_ptr.is_null() {
                        group_elems.push(MoltObject::none().bits());
                    } else {
                        group_elems.push(MoltObject::from_ptr(pair_ptr).bits());
                    }
                }
                None => {
                    group_elems.push(MoltObject::none().bits());
                }
            }
        } else {
            group_elems.push(MoltObject::none().bits());
        }
    }

    let groups_ptr = alloc_tuple(_py, &group_elems);
    let groups_bits = if groups_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(groups_ptr).bits()
    };

    let result_ptr = alloc_tuple(_py, &[start_bits, end_bits, groups_bits]);
    if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    }
}
