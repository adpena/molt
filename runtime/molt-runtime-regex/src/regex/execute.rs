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
    // Prepare the indexed subject once per public operation. Search attempts
    // borrow it, so work scales with the subject size plus attempted starts,
    // instead of copying the full subject at every start position.
    let chars: Vec<char> = text.chars().collect();
    execute_match_subject(compiled, SubjectStorage::Text(&chars), pos, end, mode)
}

pub(super) fn execute_match_bytes(
    compiled: &CompiledPattern,
    bytes: &[u8],
    pos: usize,
    end: usize,
    mode: &str,
) -> Option<MatchResult> {
    execute_match_subject(compiled, SubjectStorage::Bytes(bytes), pos, end, mode)
}

pub(super) fn execute_match_subject<'a>(
    compiled: &CompiledPattern,
    subject: SubjectStorage<'a>,
    pos: usize,
    end: usize,
    mode: &str,
) -> Option<MatchResult> {
    match mode {
        "match" => {
            // Anchored at start (pos), match from pos.
            let mut state = MatchState::new(
                subject,
                compiled.flags,
                compiled.is_bytes,
                compiled.group_count,
                end,
            );
            if pos > state.subject.len() || end > state.subject.len() {
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
            let mut state = MatchState::new(
                subject,
                compiled.flags,
                compiled.is_bytes,
                compiled.group_count,
                end,
            );
            if pos > state.subject.len() || end > state.subject.len() {
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
        "search" | "search_nonempty" => {
            // Search: try matching at each position from pos to end.
            let text_len = subject.len();
            if pos > text_len || end > text_len {
                return None;
            }
            // The trailing assertion is part of the same matcher continuation,
            // so an empty first alternative can backtrack to a non-empty
            // alternative at the same position (CPython scanner must_advance).
            let require_advance = [ReNode::RequireAdvance];
            for start in pos..=end {
                let mut state = MatchState::new(
                    subject,
                    compiled.flags,
                    compiled.is_bytes,
                    compiled.group_count,
                    end,
                );
                if mode == "search_nonempty" {
                    state.require_advance_from = Some(pos);
                }
                let rest = if mode == "search_nonempty" {
                    &require_advance[..]
                } else {
                    &[][..]
                };
                if let Some(end_pos) = try_match(&compiled.root, start, rest, &mut state) {
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
