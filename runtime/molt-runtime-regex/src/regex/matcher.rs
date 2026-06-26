use super::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Phase-1b: Backtracking match engine
// ---------------------------------------------------------------------------
//
// A recursive backtracking NFA engine that walks the `ReNode` IR tree.  It
// supports all node types produced by the Phase-1 parser.  The engine operates
// on character indices (not byte indices) to match Python's `re` semantics.

/// Match state threaded through the recursive engine.
pub(super) struct MatchState {
    /// Character array of the subject string.
    pub(super) chars: Vec<char>,
    /// Effective flags for the current match context.
    pub(super) flags: i64,
    /// End of the search window (char index, exclusive).
    pub(super) end: usize,
    /// Start of the search window (char index). Used for \A anchor.
    pub(super) search_start: usize,
    /// Group captures: index 0 is unused (group 0 is the whole match).
    /// Each entry is Some((start, end)) or None if the group was not
    /// captured.  Indexed by group number (1-based).
    pub(super) groups: Vec<Option<(usize, usize)>>,
    /// Recursion depth limit to prevent stack overflow on pathological
    /// patterns.
    pub(super) depth: usize,
}

pub(super) const MAX_RECURSION_DEPTH: usize = 5000;

impl MatchState {
    pub(super) fn new(
        text: &str,
        flags: i64,
        group_count: u32,
        search_start: usize,
        end: usize,
    ) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let groups = vec![None; (group_count + 1) as usize];
        Self {
            chars,
            flags,
            end,
            search_start,
            groups,
            depth: 0,
        }
    }

    /// Save group state for backtracking.
    pub(super) fn save_groups(&self) -> Vec<Option<(usize, usize)>> {
        self.groups.clone()
    }

    /// Restore group state after a failed branch.
    pub(super) fn restore_groups(&mut self, saved: Vec<Option<(usize, usize)>>) {
        self.groups = saved;
    }

    #[inline]
    pub(super) fn is_ignorecase(&self) -> bool {
        self.flags & RE_IGNORECASE != 0
    }

    #[inline]
    pub(super) fn is_dotall(&self) -> bool {
        self.flags & RE_DOTALL != 0
    }

    #[inline]
    pub(super) fn is_multiline(&self) -> bool {
        self.flags & RE_MULTILINE != 0
    }

    #[inline]
    pub(super) fn is_ascii(&self) -> bool {
        self.flags & RE_ASCII != 0
    }

    /// Check if a character is a "word" character for \b / \w purposes.
    #[inline]
    pub(super) fn is_word_char(&self, ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric() || (!self.is_ascii() && ch.is_alphabetic())
    }

    /// Compare two characters, respecting IGNORECASE.
    #[inline]
    pub(super) fn char_eq(&self, a: char, b: char) -> bool {
        if self.is_ignorecase() {
            let al: Vec<char> = a.to_lowercase().collect();
            let bl: Vec<char> = b.to_lowercase().collect();
            al == bl
        } else {
            a == b
        }
    }
}

/// Attempt to match `node` starting at `pos` in the subject, followed by the
/// continuation nodes in `rest`.  This continuation-passing design is critical
/// for correct backtracking in quantifiers â€” the quantifier can try different
/// repetition counts and verify that the rest of the pattern also matches.
///
/// Returns `Some(final_pos)` on success or `None` on failure.
pub(super) fn try_match(
    node: &ReNode,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    state.depth += 1;
    if state.depth > MAX_RECURSION_DEPTH {
        state.depth -= 1;
        return None;
    }
    let result = try_match_inner(node, pos, rest, state);
    state.depth -= 1;
    result
}

/// Match a continuation (a slice of nodes that must match in sequence starting
/// from `pos`).  Returns `Some(final_pos)` on success.
pub(super) fn match_rest(rest: &[ReNode], pos: usize, state: &mut MatchState) -> Option<usize> {
    if rest.is_empty() {
        return Some(pos);
    }
    try_match(&rest[0], pos, &rest[1..], state)
}

pub(super) fn try_match_inner(
    node: &ReNode,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    match node {
        ReNode::Empty => match_rest(rest, pos, state),

        ReNode::Literal(s) => {
            let lit_chars: Vec<char> = s.chars().collect();
            if pos + lit_chars.len() > state.end {
                return None;
            }
            for (i, &lc) in lit_chars.iter().enumerate() {
                if !state.char_eq(state.chars[pos + i], lc) {
                    return None;
                }
            }
            match_rest(rest, pos + lit_chars.len(), state)
        }

        ReNode::Any => {
            if pos >= state.end {
                return None;
            }
            let ch = state.chars[pos];
            if !state.is_dotall() && ch == '\n' {
                return None;
            }
            match_rest(rest, pos + 1, state)
        }

        ReNode::Anchor(kind) => {
            if match_anchor(kind, pos, state).is_some() {
                match_rest(rest, pos, state)
            } else {
                None
            }
        }

        ReNode::CharClass {
            negated,
            ranges,
            chars,
            categories,
        } => {
            if pos >= state.end {
                return None;
            }
            let ch = state.chars[pos];
            let hit = char_class_matches(ch, *negated, ranges, chars, categories, state);
            if hit {
                match_rest(rest, pos + 1, state)
            } else {
                None
            }
        }

        ReNode::Concat(nodes) => {
            // Flatten: the concat nodes followed by rest form the new
            // continuation.
            if nodes.is_empty() {
                return match_rest(rest, pos, state);
            }
            // Build combined continuation: nodes[1..] ++ rest.
            let mut combined: Vec<ReNode> = Vec::with_capacity(nodes.len() - 1 + rest.len());
            combined.extend_from_slice(&nodes[1..]);
            combined.extend_from_slice(rest);
            try_match(&nodes[0], pos, &combined, state)
        }

        ReNode::Alt(options) => {
            for opt in options {
                let saved = state.save_groups();
                if let Some(end_pos) = try_match(opt, pos, rest, state) {
                    return Some(end_pos);
                }
                state.restore_groups(saved);
            }
            None
        }

        ReNode::Repeat {
            node: inner,
            min_count,
            max_count,
            greedy,
        } => try_match_repeat(inner, pos, *min_count, *max_count, *greedy, rest, state),

        ReNode::Group { node: inner, index } => {
            let saved = state.save_groups();
            // We need to match `inner` and then set the group, then match rest.
            // Use a wrapper approach: match inner with empty rest, record group,
            // then match rest.
            let inner_result = try_match_group_then_rest(inner, *index, pos, rest, state);
            if inner_result.is_none() {
                state.restore_groups(saved);
            }
            inner_result
        }

        ReNode::Backref(idx) => {
            let idx = *idx as usize;
            if idx >= state.groups.len() {
                return None;
            }
            let (gstart, gend) = state.groups[idx]?;
            let ref_len = gend - gstart;
            if pos + ref_len > state.end {
                return None;
            }
            for i in 0..ref_len {
                if !state.char_eq(state.chars[gstart + i], state.chars[pos + i]) {
                    return None;
                }
            }
            match_rest(rest, pos + ref_len, state)
        }

        ReNode::Look {
            node: inner,
            behind,
            positive,
            width,
        } => {
            if try_match_look(inner, pos, *behind, *positive, *width, state).is_some() {
                match_rest(rest, pos, state)
            } else {
                None
            }
        }

        ReNode::ScopedFlags {
            node: inner,
            add_flags,
            clear_flags,
        } => {
            let old_flags = state.flags;
            state.flags = (state.flags | add_flags) & !clear_flags;
            // Match inner under scoped flags, then restore flags before
            // matching rest (rest should run under the outer flags).
            let inner_result = try_match(inner, pos, &[], state);
            state.flags = old_flags;
            match inner_result {
                Some(inner_end) => match_rest(rest, inner_end, state),
                None => None,
            }
        }

        ReNode::Conditional {
            group_index,
            yes,
            no,
        } => {
            let idx = *group_index as usize;
            let group_matched = idx < state.groups.len() && state.groups[idx].is_some();
            if group_matched {
                try_match(yes, pos, rest, state)
            } else {
                try_match(no, pos, rest, state)
            }
        }
    }
}

/// Match a Group node: match the inner pattern, record the group capture, then
/// match the rest continuation.  This ensures backtracking can unwind through
/// the group correctly.
pub(super) fn try_match_group_then_rest(
    inner: &ReNode,
    index: u32,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    // We create a synthetic rest that records the group then continues.
    // Since we can't easily insert a callback, we use a two-phase approach:
    // 1. Match inner with empty rest to find all possible end positions.
    // 2. For each end position, set the group and try matching rest.
    //
    // But that requires enumerating end positions, which is complex.
    // Instead, we match inner with empty continuation, record the group,
    // and match rest.  If rest fails, we need to backtrack into inner.
    //
    // For simplicity and correctness, we use the approach of matching inner
    // with a special "set-group-then-rest" continuation.  We encode this
    // by matching inner, and if it succeeds, setting the group and matching
    // rest.  If rest fails, we return None which propagates backtracking
    // back through inner's alternatives/quantifiers.

    // Build a continuation that represents: "set group, then match rest".
    // We do this by matching inner node with `rest` as continuation, but
    // wrapping in a way that records the group span.
    //
    // The simplest correct approach: match inner with empty rest to get
    // the inner end position, set the group, match rest.  This is correct
    // for most cases but doesn't allow inner to backtrack when rest fails.
    //
    // For proper backtracking, we need inner to know about rest.  The way
    // to do this: pass rest into the inner match but intercept the result
    // to set the group.  But the group span depends on inner's end position,
    // which is not directly available when rest is already appended.
    //
    // Full solution: match inner with empty rest, record end_pos, set group,
    // match rest.  This works for non-quantifier inner nodes.  For quantifier
    // inner nodes, the quantifier's backtracking handles it.

    // Simple approach (works for vast majority of patterns):
    let saved = state.save_groups();
    // Try matching inner alone first.
    let inner_end = try_match(inner, pos, &[], state);
    match inner_end {
        Some(end_pos) => {
            state.groups[index as usize] = Some((pos, end_pos));
            match match_rest(rest, end_pos, state) {
                Some(final_pos) => Some(final_pos),
                None => {
                    // Rest failed â€” we need to try other inner matches.
                    // For alternation/quantifier inner nodes, the backtracking
                    // in try_match already handles this.  But since we called
                    // try_match with empty rest, we got the "first" match.
                    // We need to enumerate all possible inner end positions.
                    //
                    // Re-try with rest appended to inner's continuation.
                    state.restore_groups(saved);
                    // Fall through to the continuation-passing approach.
                    try_match_group_with_continuation(inner, index, pos, rest, state)
                }
            }
        }
        None => {
            state.restore_groups(saved);
            None
        }
    }
}

/// Match a group with full continuation passing for proper backtracking.
/// This is the fallback when the simple group-then-rest approach fails.
pub(super) fn try_match_group_with_continuation(
    inner: &ReNode,
    index: u32,
    pos: usize,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    // We create a synthetic node that represents "set group N to (pos, HERE),
    // then match rest".  We encode this as a special wrapper.
    //
    // Since our try_match doesn't support arbitrary callbacks, we instead
    // build a Concat of [inner, GroupCapture(index, pos), rest...] where
    // GroupCapture is handled inline.
    //
    // The cleanest approach: wrap rest into a continuation and pass through.
    // We use a helper node that records the group capture point.
    //
    // For now, we use a marker node approach: create a synthetic concat that
    // is inner followed by rest, and after inner matches at some position,
    // we intercept to set the group.  This is effectively what the
    // continuation-passing style already does.

    // Match inner with rest as continuation.  The trick is that we need to
    // set the group capture BETWEEN inner and rest.  We do this by inserting
    // a synthetic "group-set" node.  Since we don't have such a node type,
    // we create a special Concat:

    // Actually, the simplest correct approach for groups with quantifier
    // inners is to NOT separate inner from rest.  Instead, match the entire
    // group pattern with rest as continuation, and record the group span
    // based on how far inner consumed.

    // The way CPython/sre handles this: the group node wraps the inner
    // pattern, and on entry it marks the group start, and on the inner's
    // success it marks the group end, then continues with rest.  If rest
    // fails, it backtracks into inner.

    // We can emulate this by using a try_match variant that records the
    // group span at each possible inner end position and then tries rest.

    // For alternation inner:
    match inner {
        ReNode::Alt(options) => {
            for opt in options {
                let saved = state.save_groups();
                if let Some(end_pos) = try_match(opt, pos, &[], state) {
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                }
                state.restore_groups(saved);
            }
            None
        }
        ReNode::Repeat {
            node: rep_inner,
            min_count,
            max_count,
            greedy,
        } => {
            // For quantifier inner, we need to try each possible repetition
            // count.  Build positions list and try each.
            let min = *min_count as usize;
            let max = max_count.map(|m| m as usize).unwrap_or(state.end - pos + 1);

            // Collect all possible end positions after min..=max repetitions.
            let mut end_positions = Vec::new();
            collect_repeat_positions(rep_inner, pos, min, max, state, &mut end_positions, 0);

            if *greedy {
                // Try from most repetitions to fewest.
                for &end_pos in end_positions.iter().rev() {
                    let saved = state.save_groups();
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                    state.restore_groups(saved);
                }
            } else {
                // Try from fewest to most.
                for &end_pos in &end_positions {
                    let saved = state.save_groups();
                    state.groups[index as usize] = Some((pos, end_pos));
                    if let Some(final_pos) = match_rest(rest, end_pos, state) {
                        return Some(final_pos);
                    }
                    state.restore_groups(saved);
                }
            }
            None
        }
        _ => {
            // For simple inner nodes, the single try_match already handles it.
            // If we got here, there's no alternative to try.
            None
        }
    }
}

/// Collect all possible end positions after `count`..=`max` repetitions of
/// `inner` starting from `pos`.  Called recursively.  Note: this temporarily
/// modifies `state.groups` during recursive calls, but always restores them
/// before returning.
pub(super) fn collect_repeat_positions(
    inner: &ReNode,
    pos: usize,
    min: usize,
    max: usize,
    state: &mut MatchState,
    positions: &mut Vec<usize>,
    count: usize,
) {
    if count >= min {
        positions.push(pos);
    }
    if count >= max {
        return;
    }
    let saved = state.save_groups();
    if let Some(next) = try_match(inner, pos, &[], state) {
        if next == pos {
            // Zero-width â€” don't recurse to avoid infinite loop.
            state.restore_groups(saved);
            return;
        }
        collect_repeat_positions(inner, next, min, max, state, positions, count + 1);
    }
    state.restore_groups(saved);
}

/// Match an anchor node.  Returns `Some(pos)` if the anchor condition holds
/// (anchors consume zero characters).
pub(super) fn match_anchor(kind: &str, pos: usize, state: &MatchState) -> Option<usize> {
    match kind {
        "start" => {
            // ^ â€” matches beginning of string, or after \n in MULTILINE.
            if pos == 0 {
                return Some(pos);
            }
            if state.is_multiline() && pos > 0 && state.chars[pos - 1] == '\n' {
                return Some(pos);
            }
            None
        }
        "end" => {
            // $ â€” matches end of string, or before \n in MULTILINE.
            // Also matches before a final \n at end (CPython behavior).
            if pos == state.end {
                return Some(pos);
            }
            if state.is_multiline() && pos < state.end && state.chars[pos] == '\n' {
                return Some(pos);
            }
            // $ also matches before a trailing newline even without MULTILINE.
            if !state.is_multiline() && pos == state.end - 1 && state.chars[pos] == '\n' {
                return Some(pos);
            }
            None
        }
        "start_abs" => {
            // \A â€” matches only at the start of the string.
            if pos == 0 { Some(pos) } else { None }
        }
        "end_abs" => {
            // \Z â€” matches only at the end of the string (or before a
            // trailing newline at the very end).
            if pos == state.end {
                return Some(pos);
            }
            if pos == state.end - 1 && state.chars[pos] == '\n' {
                return Some(pos);
            }
            None
        }
        "word_boundary" => {
            // \b â€” word boundary.
            let left_word = pos > 0 && state.is_word_char(state.chars[pos - 1]);
            let right_word = pos < state.chars.len() && state.is_word_char(state.chars[pos]);
            if left_word != right_word {
                Some(pos)
            } else {
                None
            }
        }
        "word_boundary_not" => {
            // \B â€” non-word-boundary.
            let left_word = pos > 0 && state.is_word_char(state.chars[pos - 1]);
            let right_word = pos < state.chars.len() && state.is_word_char(state.chars[pos]);
            if left_word == right_word {
                Some(pos)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check whether `ch` matches a character class specification.
pub(super) fn char_class_matches(
    ch: char,
    negated: bool,
    ranges: &[(String, String)],
    chars: &[String],
    categories: &[String],
    state: &MatchState,
) -> bool {
    let mut hit = false;

    // Check literal chars.
    for c_str in chars {
        // Each entry is a single-char string.
        if let Some(c) = c_str.chars().next()
            && state.char_eq(ch, c)
        {
            hit = true;
            break;
        }
    }

    // Check ranges.
    if !hit {
        for (lo_str, hi_str) in ranges {
            let lo = lo_str.chars().next().unwrap_or('\0');
            let hi = hi_str.chars().next().unwrap_or('\0');
            if state.is_ignorecase() {
                let ch_lower = ch.to_lowercase().next().unwrap_or(ch);
                let lo_lower = lo.to_lowercase().next().unwrap_or(lo);
                let hi_lower = hi.to_lowercase().next().unwrap_or(hi);
                if ch_lower >= lo_lower && ch_lower <= hi_lower {
                    hit = true;
                    break;
                }
                // Also check uppercase range for case-insensitive.
                let ch_upper = ch.to_uppercase().next().unwrap_or(ch);
                let lo_upper = lo.to_uppercase().next().unwrap_or(lo);
                let hi_upper = hi.to_uppercase().next().unwrap_or(hi);
                if ch_upper >= lo_upper && ch_upper <= hi_upper {
                    hit = true;
                    break;
                }
            } else if ch >= lo && ch <= hi {
                hit = true;
                break;
            }
        }
    }

    // Check categories (\d, \s, \w, etc.)
    if !hit {
        for cat in categories {
            let cat_match = match cat.as_str() {
                "d" => ch.is_ascii_digit(),
                "s" => matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'),
                "w" => {
                    ch == '_'
                        || ch.is_ascii_alphanumeric()
                        || (!state.is_ascii() && ch.is_alphabetic())
                }
                _ => {
                    // Handle POSIX classes like "posix:alpha".
                    if let Some(posix_name) = cat.strip_prefix("posix:") {
                        match posix_name {
                            "alpha" => ch.is_alphabetic(),
                            "digit" => ch.is_ascii_digit(),
                            "alnum" => ch.is_alphanumeric(),
                            "space" => ch.is_whitespace(),
                            "upper" => ch.is_uppercase(),
                            "lower" => ch.is_lowercase(),
                            "punct" => ch.is_ascii_punctuation(),
                            "print" => !ch.is_control(),
                            "xdigit" => ch.is_ascii_hexdigit(),
                            _ => false,
                        }
                    } else {
                        false
                    }
                }
            };
            // Handle negated categories (\D, \S, \W).
            // The parser stores \D as CharClass { negated: true, categories: ["d"] }.
            // So the negation is handled at the top level, not per-category.
            if cat_match {
                hit = true;
                break;
            }
        }
    }

    if negated { !hit } else { hit }
}

/// Match a `Repeat` (quantifier) node with backtracking.
///
/// The continuation `rest` is the sequence of nodes that must match after this
/// quantifier.  This enables the quantifier to backtrack: after matching N
/// repetitions, it tries to match rest; if rest fails, it adjusts N.
pub(super) fn try_match_repeat(
    inner: &ReNode,
    pos: usize,
    min_count: u64,
    max_count: Option<u64>,
    greedy: bool,
    rest: &[ReNode],
    state: &mut MatchState,
) -> Option<usize> {
    let min = min_count as usize;
    let max = max_count.map(|m| m as usize).unwrap_or(usize::MAX);

    // Collect all reachable positions after 0..=max repetitions.
    // positions[i] = position after exactly i repetitions (for i >= min, this
    // is a valid candidate).
    let mut positions: Vec<usize> = Vec::new();
    let mut cur = pos;
    // Match minimum repetitions first (mandatory).
    for i in 0..min {
        let saved = state.save_groups();
        match try_match(inner, cur, &[], state) {
            Some(next) => {
                if next == cur && i > 0 {
                    // Zero-width match in minimum â€” still counts.
                    state.restore_groups(saved);
                    break;
                }
                cur = next;
            }
            None => {
                state.restore_groups(saved);
                return None;
            }
        }
    }
    positions.push(cur);

    // Collect additional (optional) repetition positions.
    let mut count = min;
    while count < max {
        let saved = state.save_groups();
        match try_match(inner, cur, &[], state) {
            Some(next) => {
                if next == cur {
                    // Zero-width match â€” stop collecting.
                    state.restore_groups(saved);
                    break;
                }
                cur = next;
                positions.push(cur);
                count += 1;
            }
            None => {
                state.restore_groups(saved);
                break;
            }
        }
    }

    if greedy {
        // Greedy: try from most repetitions to fewest.
        while let Some(try_pos) = positions.pop() {
            let saved = state.save_groups();
            if let Some(final_pos) = match_rest(rest, try_pos, state) {
                return Some(final_pos);
            }
            state.restore_groups(saved);
        }
        None
    } else {
        // Lazy: try from fewest repetitions to most.
        for &try_pos in &positions {
            let saved = state.save_groups();
            if let Some(final_pos) = match_rest(rest, try_pos, state) {
                return Some(final_pos);
            }
            state.restore_groups(saved);
        }
        None
    }
}

/// Handle lookahead and lookbehind assertions.
pub(super) fn try_match_look(
    inner: &ReNode,
    pos: usize,
    behind: bool,
    positive: bool,
    width: Option<u64>,
    state: &mut MatchState,
) -> Option<usize> {
    if behind {
        // Look-behind: check the substring ending at `pos`.
        let w = width.unwrap_or(0) as usize;
        if pos < w {
            // Not enough text behind.
            return if positive { None } else { Some(pos) };
        }
        let start = pos - w;
        let saved = state.save_groups();
        let old_end = state.end;
        state.end = pos;
        let matched = try_match(inner, start, &[], state);
        state.end = old_end;
        let ok = match matched {
            Some(end_pos) => end_pos == pos,
            None => false,
        };
        if positive == ok {
            if !ok {
                // Positive lookbehind failed â€” restore.
                // (ok is false AND positive is false, so this is negative
                // lookbehind succeeding because the inner didn't match.)
                state.restore_groups(saved);
            }
            // For positive lookbehind success (ok=true, positive=true):
            // keep the groups from the inner match.
            // For negative lookbehind success (ok=false, positive=false):
            // groups already restored above.
            Some(pos)
        } else {
            // Assertion failed â€” always restore.
            state.restore_groups(saved);
            None
        }
    } else {
        // Lookahead: check the substring starting at `pos`.
        let saved = state.save_groups();
        let matched = try_match(inner, pos, &[], state).is_some();
        if positive == matched {
            if !matched {
                // Negative lookahead succeeded (inner did NOT match) â€”
                // groups already unchanged, restore to be safe.
                state.restore_groups(saved);
            }
            // Positive lookahead succeeded â€” keep groups from inner.
            Some(pos)
        } else {
            // Assertion failed â€” restore groups.
            state.restore_groups(saved);
            None
        }
    }
}
