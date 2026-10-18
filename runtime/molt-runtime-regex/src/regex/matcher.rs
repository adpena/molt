use super::*;
use std::num::NonZeroUsize;

#[derive(Clone, Copy)]
pub(super) enum SubjectStorage<'a> {
    Text(&'a [u32]),
    Bytes(&'a [u8]),
}
impl SubjectStorage<'_> {
    #[inline]
    pub(super) fn len(self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Bytes(bytes) => bytes.len(),
        }
    }
    #[inline]
    pub(super) fn unit(self, index: usize) -> u32 {
        match self {
            Self::Text(text) => text[index],
            Self::Bytes(bytes) => u32::from(bytes[index]),
        }
    }
}

/// Subject slices cannot exceed isize::MAX bytes, so position + 1 is nonzero
/// and fits usize. This keeps each optional mark one machine word.
fn position_mark(pos: usize) -> NonZeroUsize {
    NonZeroUsize::new(pos + 1).expect("subject position fits a nonzero mark")
}

#[derive(Clone, Copy, Default)]
struct Marks {
    start: Option<NonZeroUsize>,
    end: Option<NonZeroUsize>,
}

#[derive(Clone, Copy, Default)]
struct RepeatState {
    count: u64,
    last_optional_start: Option<NonZeroUsize>,
}

#[derive(Clone, Copy)]
enum Undo {
    Start(u32, Option<NonZeroUsize>),
    End(u32, Option<NonZeroUsize>),
    LastIndex(Option<u32>),
    Repeat(usize, RepeatState),
}

enum Retry {
    Resume { pc: usize, pos: usize },
    Single { pc: usize, count: u64, pos: usize },
}
struct Choice {
    retry: Retry,
    undo: usize,
}
#[derive(Clone, Copy)]
struct Possessive {
    entry: usize,
    min: u64,
    max: Option<u64>,
    count: u64,
    last_optional_start: Option<NonZeroUsize>,
    iteration_undo: usize,
    iteration_start: usize,
}

#[derive(Clone, Copy)]
enum ScopeKind {
    Assertion {
        expected_end: Option<usize>,
        positive: bool,
    },
    Atomic,
    Possessive(Possessive),
}

#[derive(Clone, Copy)]
struct Scope {
    next: usize,
    pos: usize,
    choices: usize,
    undo: usize,
    kind: ScopeKind,
}

/// One workspace per operation, reused across search starts. Undo records contain
/// changed slots only; alternatives never copy the capture catalog or program.
pub(super) struct Matcher<'a> {
    compiled: &'a CompiledPattern,
    subject: SubjectStorage<'a>,
    end: usize,
    marks: Vec<Marks>,
    lastindex: Option<u32>,
    repeats: Vec<RepeatState>,
    undo: Vec<Undo>,
    choices: Vec<Choice>,
    scopes: Vec<Scope>,
    undo_seen: Vec<usize>,
    undo_epoch: usize,
}

fn slots<T: Clone>(len: usize, value: T) -> Result<Vec<T>, MatchError> {
    let mut items = Vec::new();
    items.try_reserve_exact(len)?;
    items.resize(len, value);
    Ok(items)
}

impl<'a> Matcher<'a> {
    pub(super) fn new(
        compiled: &'a CompiledPattern,
        subject: SubjectStorage<'a>,
        end: usize,
    ) -> Result<Self, MatchError> {
        let count = compiled.group_count as usize + 1;
        Ok(Self {
            compiled,
            subject,
            end,
            marks: slots(count, Marks::default())?,
            lastindex: None,
            repeats: slots(compiled.program.repeats, RepeatState::default())?,
            undo: Vec::new(),
            choices: Vec::new(),
            scopes: Vec::new(),
            undo_seen: Vec::new(),
            undo_epoch: 0,
        })
    }

    fn rollback(&mut self, checkpoint: usize) {
        while self.undo.len() > checkpoint {
            match self.undo.pop().expect("undo checkpoint") {
                Undo::Start(index, value) => self.marks[index as usize].start = value,
                Undo::End(index, value) => self.marks[index as usize].end = value,
                Undo::LastIndex(value) => self.lastindex = value,
                Undo::Repeat(index, value) => self.repeats[index] = value,
            }
        }
    }

    /// After a possessive iteration commits, no live continuation points into
    /// its journal interval. Retain the first old value for each changed slot,
    /// enough for an outer alternative to undo the entire committed scope.
    /// This bounds history by touched slots rather than repetition count.
    fn compact_undo(&mut self, checkpoint: usize) -> Result<(), MatchError> {
        if self.undo.len() <= checkpoint + 1 {
            return Ok(());
        }
        let captures = self.marks.len() * 2;
        if self.undo_seen.is_empty() {
            self.undo_seen = slots(captures + 1 + self.repeats.len(), 0)?;
        }
        self.undo_epoch = self.undo_epoch.wrapping_add(1);
        if self.undo_epoch == 0 {
            self.undo_seen.fill(0);
            self.undo_epoch = 1;
        }
        let mut write = checkpoint;
        for read in checkpoint..self.undo.len() {
            let key = match self.undo[read] {
                Undo::Start(index, _) => index as usize * 2,
                Undo::End(index, _) => index as usize * 2 + 1,
                Undo::LastIndex(_) => captures,
                Undo::Repeat(index, _) => captures + 1 + index,
            };
            if self.undo_seen[key] != self.undo_epoch {
                self.undo_seen[key] = self.undo_epoch;
                self.undo[write] = self.undo[read];
                write += 1;
            }
        }
        self.undo.truncate(write);
        Ok(())
    }

    fn start_group(&mut self, index: u32, pos: usize) -> Result<(), MatchError> {
        let old = self.marks[index as usize].start;
        checked_push(&mut self.undo, Undo::Start(index, old))?;
        self.marks[index as usize].start = Some(position_mark(pos));
        Ok(())
    }

    fn capture(&mut self, index: u32, end: usize) -> Result<(), MatchError> {
        checked_push(
            &mut self.undo,
            Undo::End(index, self.marks[index as usize].end),
        )?;
        checked_push(&mut self.undo, Undo::LastIndex(self.lastindex))?;
        self.marks[index as usize].end = Some(position_mark(end));
        self.lastindex = Some(index);
        Ok(())
    }

    fn group(&self, index: u32) -> Option<(usize, usize)> {
        let marks = self.marks.get(index as usize)?;
        let start = marks.start?.get() - 1;
        let end = marks.end?.get() - 1;
        (end >= start).then_some((start, end))
    }

    fn repeat(&mut self, id: usize, value: RepeatState) -> Result<(), MatchError> {
        checked_push(&mut self.undo, Undo::Repeat(id, self.repeats[id]))?;
        self.repeats[id] = value;
        Ok(())
    }

    fn choose(&mut self, retry: Retry) -> Result<(), MatchError> {
        checked_push(
            &mut self.choices,
            Choice {
                retry,
                undo: self.undo.len(),
            },
        )
    }

    fn matches_one(&self, plan: usize, mut pos: usize) -> bool {
        let single = &self.compiled.program.singles[plan];
        if single.width > self.end.saturating_sub(pos) {
            return false;
        }
        for atom in &single.atoms {
            let Some(next) =
                atom_matches(atom, self.subject, pos, self.end, self.compiled.is_bytes)
            else {
                return false;
            };
            pos = next;
        }
        true
    }

    fn single_captures(&mut self, plan: usize, count: u64, pos: usize) -> Result<(), MatchError> {
        if count != 0 {
            let single = &self.compiled.program.singles[plan];
            let start = pos - single.width;
            for recipe in &single.captures {
                self.start_group(recipe.index, start + recipe.start)?;
                self.capture(recipe.index, start + recipe.end)?;
            }
        }
        Ok(())
    }

    fn single(
        &mut self,
        pc: usize,
        count: u64,
        pos: usize,
        retry: bool,
    ) -> Result<Option<(usize, usize)>, MatchError> {
        let Instruction::RepeatOne {
            plan,
            min,
            max,
            mode,
            next,
        } = self.compiled.program.code[pc]
        else {
            unreachable!("single retry opcode")
        };
        let width = self.compiled.program.singles[plan].width;
        let mut count = count;
        let mut pos = pos;
        if retry {
            if mode == RepeatMode::Greedy {
                if count == min {
                    return Ok(None);
                }
                count -= 1;
                pos -= width;
            } else {
                if max.is_some_and(|max| count >= max) || !self.matches_one(plan, pos) {
                    return Ok(None);
                }
                count += 1;
                pos += width;
            }
        } else {
            while count < min {
                if !self.matches_one(plan, pos) {
                    return Ok(None);
                }
                count += 1;
                pos += width;
            }
            if mode != RepeatMode::Lazy {
                while max.is_none_or(|max| count < max) && self.matches_one(plan, pos) {
                    count += 1;
                    pos += width;
                }
            }
        }
        // The checkpoint precedes capture projection: retries restore the state
        // before the loop, then reconstruct exactly the selected last iteration.
        if (mode == RepeatMode::Greedy && count > min)
            || (mode == RepeatMode::Lazy && max.is_none_or(|max| count < max))
        {
            self.choose(Retry::Single { pc, count, pos })?;
        }
        self.single_captures(plan, count, pos)?;
        Ok(Some((next, pos)))
    }

    /// Resume a priority alternative within the innermost scope. A scope may
    /// commit success or turn local failure into assertion/repetition success.
    fn backtrack(&mut self) -> Result<Option<(usize, usize)>, MatchError> {
        loop {
            let barrier = self.scopes.last().map_or(0, |frame| frame.choices);
            if self.choices.len() > barrier {
                let choice = self.choices.pop().expect("choice barrier");
                self.rollback(choice.undo);
                match choice.retry {
                    Retry::Resume { pc, pos } => return Ok(Some((pc, pos))),
                    Retry::Single { pc, count, pos } => {
                        if let Some(resume) = self.single(pc, count, pos, true)? {
                            return Ok(Some(resume));
                        }
                    }
                }
            } else if let Some(frame) = self.scopes.pop() {
                if let ScopeKind::Possessive(repeat) = frame.kind
                    && repeat.count >= repeat.min
                {
                    self.rollback(repeat.iteration_undo);
                    return Ok(Some((frame.next, repeat.iteration_start)));
                }
                self.rollback(frame.undo);
                if let ScopeKind::Assertion {
                    positive: false, ..
                } = frame.kind
                {
                    return Ok(Some((frame.next, frame.pos)));
                }
            } else {
                self.rollback(0);
                return Ok(None);
            }
        }
    }

    pub(super) fn run(
        &mut self,
        start: usize,
        fullmatch: bool,
        require_advance: Option<usize>,
    ) -> Result<Option<usize>, MatchError> {
        self.choices.clear();
        self.scopes.clear();
        self.rollback(0);
        let mut pc = self.compiled.program.entry;
        let mut pos = start;
        loop {
            let instruction = &self.compiled.program.code[pc];
            let mut failed = false;
            match *instruction {
                Instruction::Accept => {
                    if (!fullmatch || pos == self.end) && require_advance != Some(pos) {
                        return Ok(Some(pos));
                    }
                    failed = true;
                }
                Instruction::ScopeAccept => {
                    let frame = *self.scopes.last().expect("scope accept");
                    if matches!(frame.kind, ScopeKind::Assertion {
                        expected_end: Some(end), ..
                    } if pos != end)
                    {
                        failed = true;
                    } else {
                        self.choices.truncate(frame.choices);
                        match frame.kind {
                            ScopeKind::Assertion { positive, .. } => {
                                self.scopes.pop();
                                if positive {
                                    pc = frame.next;
                                    pos = frame.pos;
                                } else {
                                    self.rollback(frame.undo);
                                    failed = true;
                                }
                            }
                            ScopeKind::Atomic => {
                                self.scopes.pop();
                                self.compact_undo(frame.undo)?;
                                pc = frame.next;
                            }
                            ScopeKind::Possessive(mut repeat) => {
                                if repeat.count >= repeat.min {
                                    repeat.last_optional_start =
                                        Some(position_mark(repeat.iteration_start));
                                }
                                repeat.count += 1;
                                self.compact_undo(frame.undo)?;
                                if repeat.max.is_none_or(|max| repeat.count < max)
                                    && (repeat.count < repeat.min
                                        || repeat.last_optional_start != Some(position_mark(pos)))
                                {
                                    repeat.iteration_start = pos;
                                    repeat.iteration_undo = self.undo.len();
                                    self.scopes.last_mut().expect("possessive scope").kind =
                                        ScopeKind::Possessive(repeat);
                                    pc = repeat.entry;
                                } else {
                                    self.scopes.pop();
                                    pc = frame.next;
                                }
                            }
                        }
                    }
                }
                Instruction::Consume { ref atom, next } => {
                    if let Some(end) =
                        atom_matches(atom, self.subject, pos, self.end, self.compiled.is_bytes)
                    {
                        pos = end;
                        pc = next;
                    } else {
                        failed = true;
                    }
                }
                Instruction::Anchor { kind, flags, next } => {
                    if anchor_matches(
                        kind,
                        pos,
                        self.subject,
                        self.end,
                        flags,
                        self.compiled.is_bytes,
                    ) {
                        pc = next;
                    } else {
                        failed = true;
                    }
                }
                Instruction::Split { first, second } => {
                    self.choose(Retry::Resume { pc: second, pos })?;
                    pc = first;
                }
                Instruction::GroupStart { index, next } => {
                    self.start_group(index, pos)?;
                    pc = next;
                }
                Instruction::GroupEnd { index, next } => {
                    if self.marks[index as usize].start.is_none() {
                        return Err(MatchError::Internal);
                    }
                    self.capture(index, pos)?;
                    pc = next;
                }
                Instruction::Backref { index, flags, next } => {
                    if let Some((start, end)) = self.group(index) {
                        let width = end - start;
                        if width <= self.end.saturating_sub(pos)
                            && (0..width).all(|offset| {
                                char_eq(
                                    self.subject.unit(start + offset),
                                    self.subject.unit(pos + offset),
                                    flags,
                                    self.compiled.is_bytes,
                                )
                            })
                        {
                            pos += width;
                            pc = next;
                        } else {
                            failed = true;
                        }
                    } else {
                        failed = true;
                    }
                }
                Instruction::Conditional { index, yes, no } => {
                    pc = if self.group(index).is_some() { yes } else { no };
                }
                Instruction::Assert {
                    entry,
                    width,
                    positive,
                    next,
                } => {
                    if width.is_some_and(|width| width > pos as u64) {
                        if positive {
                            failed = true;
                        } else {
                            pc = next;
                        }
                    } else {
                        checked_push(
                            &mut self.scopes,
                            Scope {
                                next,
                                pos,
                                kind: ScopeKind::Assertion {
                                    expected_end: width.map(|_| pos),
                                    positive,
                                },
                                choices: self.choices.len(),
                                undo: self.undo.len(),
                            },
                        )?;
                        // Nested lookahead sees the original subject window even
                        // inside lookbehind; only assertion acceptance fixes end.
                        pos -= width.unwrap_or(0) as usize;
                        pc = entry;
                    }
                }
                Instruction::Atomic { entry, next } => {
                    checked_push(
                        &mut self.scopes,
                        Scope {
                            next,
                            pos,
                            choices: self.choices.len(),
                            undo: self.undo.len(),
                            kind: ScopeKind::Atomic,
                        },
                    )?;
                    pc = entry;
                }
                Instruction::Possessive {
                    entry,
                    min,
                    max,
                    next,
                } => {
                    if max == Some(0) {
                        pc = next;
                    } else {
                        checked_push(
                            &mut self.scopes,
                            Scope {
                                next,
                                pos,
                                choices: self.choices.len(),
                                undo: self.undo.len(),
                                kind: ScopeKind::Possessive(Possessive {
                                    entry,
                                    min,
                                    max,
                                    count: 0,
                                    last_optional_start: None,
                                    iteration_undo: self.undo.len(),
                                    iteration_start: pos,
                                }),
                            },
                        )?;
                        pc = entry;
                    }
                }
                Instruction::RepeatEnter { id, next } => {
                    self.repeat(id, RepeatState::default())?;
                    pc = next;
                }
                Instruction::Repeat {
                    id,
                    body,
                    optional,
                    min,
                    max,
                    greedy,
                    next,
                } => {
                    let state = self.repeats[id];
                    if state.count < min {
                        pc = body;
                    } else if max.is_none_or(|max| state.count < max)
                        && state.last_optional_start != Some(position_mark(pos))
                    {
                        let (first, second) = if greedy {
                            (optional, next)
                        } else {
                            (next, optional)
                        };
                        self.choose(Retry::Resume { pc: second, pos })?;
                        pc = first;
                    } else {
                        pc = next;
                    }
                }
                Instruction::RepeatOptional { id, next } => {
                    let state = self.repeats[id];
                    self.repeat(
                        id,
                        RepeatState {
                            last_optional_start: Some(position_mark(pos)),
                            ..state
                        },
                    )?;
                    pc = next;
                }
                Instruction::RepeatStep { id, next } => {
                    let state = self.repeats[id];
                    self.repeat(
                        id,
                        RepeatState {
                            count: state.count + 1,
                            ..state
                        },
                    )?;
                    pc = next;
                }
                Instruction::RepeatOne { .. } => {
                    if let Some((next, end)) = self.single(pc, 0, pos, false)? {
                        pc = next;
                        pos = end;
                    } else {
                        failed = true;
                    }
                }
            }
            if failed {
                match self.backtrack()? {
                    Some((next, end)) => {
                        pc = next;
                        pos = end;
                    }
                    None => return Ok(None),
                }
            }
        }
    }

    pub(super) fn result(self, start: usize, end: usize) -> Result<MatchResult, MatchError> {
        let mut groups = Vec::new();
        groups.try_reserve_exact(self.marks.len())?;
        for marks in self.marks {
            let group = match (marks.start, marks.end) {
                (Some(start), Some(end)) if end >= start => Some((start.get() - 1, end.get() - 1)),
                (Some(_), Some(_)) => return Err(MatchError::Internal),
                _ => None,
            };
            groups.push(group);
        }
        Ok(MatchResult {
            start,
            end,
            groups,
            lastindex: self.lastindex,
        })
    }
}

#[inline]
fn ascii(flags: i64, is_bytes: bool) -> bool {
    is_bytes || flags & RE_ASCII != 0
}
#[inline]
fn word(ch: u32, flags: i64, is_bytes: bool) -> bool {
    ch == u32::from(b'_')
        || char::from_u32(ch).is_some_and(|ch| {
            if ascii(flags, is_bytes) {
                ch.is_ascii_alphanumeric()
            } else {
                ch.is_alphanumeric()
            }
        })
}

#[inline]
fn ascii_lower(ch: u32) -> u32 {
    if (u32::from(b'A')..=u32::from(b'Z')).contains(&ch) {
        ch + u32::from(b'a' - b'A')
    } else {
        ch
    }
}

#[inline]
pub(super) fn char_eq(a: u32, b: u32, flags: i64, is_bytes: bool) -> bool {
    if a == b {
        true
    } else if flags & RE_IGNORECASE == 0 {
        false
    } else if ascii(flags, is_bytes) {
        ascii_lower(a) == ascii_lower(b)
    } else {
        match (char::from_u32(a), char::from_u32(b)) {
            (Some(a), Some(b)) => a.to_lowercase().eq(b.to_lowercase()),
            // Python surrogates are unchanged by case operations. Distinct
            // code points cannot become equal through a lossy replacement.
            _ => false,
        }
    }
}

fn category_matches(category: Category, ch: u32, flags: i64, is_bytes: bool) -> bool {
    let digit = (u32::from(b'0')..=u32::from(b'9')).contains(&ch);
    let space = matches!(ch, 0x20 | 0x09..=0x0d);
    match category {
        Category::Digit => digit,
        Category::NotDigit => !digit,
        Category::Space => space,
        Category::NotSpace => !space,
        Category::Word => word(ch, flags, is_bytes),
        Category::NotWord => !word(ch, flags, is_bytes),
    }
}

#[inline]
fn atom_matches(
    atom: &Atom,
    subject: SubjectStorage<'_>,
    pos: usize,
    end: usize,
    is_bytes: bool,
) -> Option<usize> {
    let flags = atom.flags;
    if let Predicate::Literal(expected) = &atom.predicate {
        return (expected.len() <= end.saturating_sub(pos)
            && expected
                .iter()
                .enumerate()
                .all(|(offset, &ch)| char_eq(subject.unit(pos + offset), ch, flags, is_bytes)))
        .then_some(pos + expected.len());
    }
    if pos >= end {
        return None;
    }
    let ch = subject.unit(pos);
    let matched = match &atom.predicate {
        Predicate::Unit(expected) => char_eq(ch, *expected, flags, is_bytes),
        Predicate::Literal(_) => unreachable!("literal handled before subject unit access"),
        Predicate::Any => flags & RE_DOTALL != 0 || ch != u32::from(b'\n'),
        Predicate::Class {
            negated,
            ranges,
            chars,
            categories,
        } => {
            let hit = chars
                .iter()
                .any(|expected| char_eq(ch, *expected, flags, is_bytes))
                || ranges.iter().any(|&(lo, hi)| {
                    if flags & RE_IGNORECASE == 0 {
                        (lo..=hi).contains(&ch)
                    } else if ascii(flags, is_bytes) {
                        (ascii_lower(lo)..=ascii_lower(hi)).contains(&ascii_lower(ch))
                    } else {
                        let lower = |ch: u32| {
                            char::from_u32(ch)
                                .and_then(|c| c.to_lowercase().next())
                                .map(u32::from)
                                .unwrap_or(ch)
                        };
                        let upper = |ch: u32| {
                            char::from_u32(ch)
                                .and_then(|c| c.to_uppercase().next())
                                .map(u32::from)
                                .unwrap_or(ch)
                        };
                        (lower(lo)..=lower(hi)).contains(&lower(ch))
                            || (upper(lo)..=upper(hi)).contains(&upper(ch))
                    }
                })
                || categories
                    .iter()
                    .any(|&category| category_matches(category, ch, flags, is_bytes));
            hit != *negated
        }
    };
    matched.then_some(pos + 1)
}

fn anchor_matches(
    kind: Anchor,
    pos: usize,
    subject: SubjectStorage<'_>,
    end: usize,
    flags: i64,
    is_bytes: bool,
) -> bool {
    match kind {
        Anchor::Start => {
            pos == 0
                || (flags & RE_MULTILINE != 0
                    && pos > 0
                    && subject.unit(pos - 1) == u32::from(b'\n'))
        }
        Anchor::End => {
            pos == end
                || (pos < end
                    && subject.unit(pos) == u32::from(b'\n')
                    && (flags & RE_MULTILINE != 0 || pos + 1 == end))
        }
        Anchor::StartAbsolute => pos == 0,
        Anchor::EndAbsolute => pos == end,
        Anchor::Boundary | Anchor::NotBoundary => {
            let left = pos > 0 && word(subject.unit(pos - 1), flags, is_bytes);
            let right = pos < end && word(subject.unit(pos), flags, is_bytes);
            if matches!(kind, Anchor::Boundary) {
                left != right
            } else {
                end != 0 && left == right
            }
        }
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    fn workspace_bytes(matcher: &Matcher<'_>) -> usize {
        matcher.marks.capacity() * size_of::<Marks>()
            + matcher.repeats.capacity() * size_of::<RepeatState>()
            + matcher.undo.capacity() * size_of::<Undo>()
            + matcher.choices.capacity() * size_of::<Choice>()
            + matcher.scopes.capacity() * size_of::<Scope>()
            + matcher.undo_seen.capacity() * size_of::<usize>()
    }

    #[test]
    fn possessive_commits_bound_history_and_preserve_outer_capture_rollback() {
        for len in [20_000, 200_000] {
            let mut text = vec![u32::from(b'a'); len];
            text.push(u32::from(b'z'));
            for (pattern, expected_groups, expected_lastindex) in [
                (
                    "((a|ab))++z",
                    vec![None, Some((len - 1, len)), Some((len - 1, len))],
                    Some(1),
                ),
                (
                    "(?:((a|ab))++x|(a+)z)",
                    vec![None, None, None, Some((0, len))],
                    Some(3),
                ),
            ] {
                let compiled =
                    parse_pattern_typed(pattern.chars().map(u32::from).collect(), 0, false)
                        .unwrap()
                        .compile()
                        .unwrap();
                let mut matcher =
                    Matcher::new(&compiled, SubjectStorage::Text(&text), text.len()).unwrap();
                assert_eq!(
                    matcher.run(0, true, None).unwrap(),
                    Some(text.len()),
                    "{pattern}"
                );
                let bytes = workspace_bytes(&matcher);
                assert!(
                    bytes < 8192,
                    "{pattern} retained {bytes} bytes for {len} units"
                );
                let result = matcher.result(0, text.len()).unwrap();
                assert_eq!(result.groups, expected_groups, "{pattern}");
                assert_eq!(result.lastindex, expected_lastindex, "{pattern}");
            }
        }
    }

    #[test]
    fn fixed_width_repeat_workspace_is_independent_of_subject_length() {
        let compiled = parse_pattern_typed("((a)())+a".chars().map(u32::from).collect(), 0, false)
            .unwrap()
            .compile()
            .unwrap();
        for len in [20_000, 200_000] {
            let text = vec![u32::from(b'a'); len];
            let mut matcher = Matcher::new(&compiled, SubjectStorage::Text(&text), len).unwrap();
            assert_eq!(matcher.run(0, true, None).unwrap(), Some(len));
            // Capacities retain peak allocations. This budget excludes the
            // deliberately prepared subject and includes every VM workspace.
            let bytes = workspace_bytes(&matcher);
            assert!(
                bytes < 8192,
                "fixed-width repeat retained {bytes} workspace bytes for {len} units"
            );
            let result = matcher.result(0, len).unwrap();
            assert_eq!(
                &result.groups[1..],
                &[
                    Some((len - 2, len - 1)),
                    Some((len - 2, len - 1)),
                    Some((len - 1, len - 1))
                ]
            );
            assert_eq!(result.lastindex, Some(1));
        }
    }

    #[test]
    fn allocation_refusal_is_an_error_not_no_match() {
        assert!(matches!(
            slots::<u64>(usize::MAX, 0),
            Err(MatchError::Memory)
        ));
    }

    #[test]
    fn long_literals_keep_one_program_block() {
        let text = "a".repeat(20_000);
        let compiled = parse_pattern_typed(text.chars().map(u32::from).collect(), 0, false)
            .unwrap()
            .compile()
            .unwrap();
        assert_eq!(
            compiled.program.code.len(),
            2,
            "literal length must not expand VM dispatch storage"
        );
        let matched = execute_match(
            &compiled,
            text.as_bytes(),
            0,
            text.len(),
            MatchMode::FullMatch,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (matched.start, matched.end, matched.lastindex),
            (0, text.len(), None)
        );
    }
}
