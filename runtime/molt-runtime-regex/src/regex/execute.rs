use super::*;

/// Successful execution, including group closing order (not highest group id).
#[derive(Debug)]
pub(super) struct MatchResult {
    pub(super) start: usize,
    pub(super) end: usize,
    /// Slot zero is unused; group zero is the overall start/end pair.
    pub(super) groups: Vec<Option<(usize, usize)>>,
    pub(super) lastindex: Option<u32>,
}

pub(super) fn execute_match(
    compiled: &CompiledPattern,
    text: &str,
    pos: usize,
    end: usize,
    mode: &str,
) -> Result<Option<MatchResult>, MatchError> {
    let mut chars = Vec::new();
    // Reserve from the byte bound once; per-start work borrows this index.
    chars.try_reserve_exact(text.len())?;
    chars.extend(text.chars());
    execute_match_subject(compiled, SubjectStorage::Text(&chars), pos, end, mode)
}

pub(super) fn execute_match_bytes(
    compiled: &CompiledPattern,
    bytes: &[u8],
    pos: usize,
    end: usize,
    mode: &str,
) -> Result<Option<MatchResult>, MatchError> {
    execute_match_subject(compiled, SubjectStorage::Bytes(bytes), pos, end, mode)
}

pub(super) fn execute_match_subject(
    compiled: &CompiledPattern,
    subject: SubjectStorage<'_>,
    pos: usize,
    end: usize,
    mode: &str,
) -> Result<Option<MatchResult>, MatchError> {
    if pos > end || end > subject.len() {
        return Ok(None);
    }
    let (search, fullmatch, require_advance) = match mode {
        "match" => (false, false, None),
        "fullmatch" => (false, true, None),
        "search" => (true, false, None),
        "search_nonempty" => (true, false, Some(pos)),
        _ => return Ok(None),
    };
    let mut matcher = Matcher::new(compiled, subject, end)?;
    let last_start = if search { end } else { pos };
    for start in pos..=last_start {
        if let Some(end) = matcher.run(start, fullmatch, require_advance)? {
            return Ok(Some(matcher.result(start, end)));
        }
    }
    Ok(None)
}
