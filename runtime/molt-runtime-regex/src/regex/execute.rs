use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MatchMode {
    Match,
    FullMatch,
    Search,
    SearchNonempty,
}

impl TryFrom<&str> for MatchMode {
    type Error = MatchError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "match" => Ok(Self::Match),
            "fullmatch" => Ok(Self::FullMatch),
            "search" => Ok(Self::Search),
            "search_nonempty" => Ok(Self::SearchNonempty),
            _ => Err(MatchError::InvalidMode),
        }
    }
}

/// Successful execution, including group closing order (not highest group id).
#[derive(Debug)]
pub(super) struct MatchResult {
    pub(super) start: usize,
    pub(super) end: usize,
    /// Slot zero is unused; group zero is the overall start/end pair.
    pub(super) groups: Vec<Option<(usize, usize)>>,
    pub(super) lastindex: Option<u32>,
}

/// Prepare Python code points once from admitted runtime string storage.
/// This is the same four-byte index as Rust char, but retains surrogates and
/// never passes through a replacement-producing UTF-8 String conversion.
pub(super) fn prepare_text(bytes: &[u8]) -> Result<Vec<u32>, MatchError> {
    let mut units = Vec::new();
    units.try_reserve_exact(bytes.len())?;
    units.extend(molt_obj_model::text_storage::code_points(bytes));
    Ok(units)
}

pub(super) fn execute_match(
    compiled: &CompiledPattern,
    text: &[u8],
    pos: usize,
    end: usize,
    mode: MatchMode,
) -> Result<Option<MatchResult>, MatchError> {
    let units = prepare_text(text)?;
    execute_match_subject(compiled, SubjectStorage::Text(&units), pos, end, mode)
}

pub(super) fn execute_match_bytes(
    compiled: &CompiledPattern,
    bytes: &[u8],
    pos: usize,
    end: usize,
    mode: MatchMode,
) -> Result<Option<MatchResult>, MatchError> {
    execute_match_subject(compiled, SubjectStorage::Bytes(bytes), pos, end, mode)
}

pub(super) fn execute_match_subject(
    compiled: &CompiledPattern,
    subject: SubjectStorage<'_>,
    pos: usize,
    end: usize,
    mode: MatchMode,
) -> Result<Option<MatchResult>, MatchError> {
    // CPython clamps each bound to the physical subject, independently.
    // Anchored match may still produce an empty result beyond endpos.
    let pos = pos.min(subject.len());
    let end = end.min(subject.len());
    if pos > end && mode != MatchMode::Match {
        return Ok(None);
    }
    let (search, fullmatch, require_advance) = match mode {
        MatchMode::Match => (false, false, None),
        MatchMode::FullMatch => (false, true, None),
        MatchMode::Search => (true, false, None),
        MatchMode::SearchNonempty => (true, false, Some(pos)),
    };
    let mut matcher = Matcher::new(compiled, subject, end)?;
    let last_start = if search { end } else { pos };
    for start in pos..=last_start {
        if let Some(end) = matcher.run(start, fullmatch, require_advance)? {
            return Ok(Some(matcher.result(start, end)?));
        }
    }
    Ok(None)
}
