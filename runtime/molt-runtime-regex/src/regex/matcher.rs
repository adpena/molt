use super::*;

#[derive(Clone, Copy)]
pub(super) enum SubjectStorage<'a> {
    Text(&'a [char]),
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
    pub(super) fn unit(self, index: usize) -> char {
        match self {
            Self::Text(text) => text[index],
            Self::Bytes(bytes) => char::from(bytes[index]),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct RepeatState {
    count: u64,
    cursor: usize,
    stalled: bool,
}

enum Undo {
    Start(u32, Option<usize>),
    Capture(u32, Option<(usize, usize)>),
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
struct Assertion {
    next: usize,
    pos: usize,
    expected_end: Option<usize>,
    positive: bool,
    choices: usize,
    undo: usize,
}

/// One workspace per operation, reused across search starts. Undo records contain
/// changed slots only; alternatives never copy the capture catalog or program.
pub(super) struct Matcher<'a> {
    compiled: &'a CompiledPattern,
    subject: SubjectStorage<'a>,
    end: usize,
    starts: Vec<Option<usize>>,
    groups: Vec<Option<(usize, usize)>>,
    lastindex: Option<u32>,
    repeats: Vec<RepeatState>,
    undo: Vec<Undo>,
    choices: Vec<Choice>,
    assertions: Vec<Assertion>,
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
            starts: slots(count, None)?,
            groups: slots(count, None)?,
            lastindex: None,
            repeats: slots(compiled.program.repeats, RepeatState::default())?,
            undo: Vec::new(),
            choices: Vec::new(),
            assertions: Vec::new(),
        })
    }

    fn rollback(&mut self, checkpoint: usize) {
        while self.undo.len() > checkpoint {
            match self.undo.pop().expect("undo checkpoint") {
                Undo::Start(index, value) => self.starts[index as usize] = value,
                Undo::Capture(index, value) => self.groups[index as usize] = value,
                Undo::LastIndex(value) => self.lastindex = value,
                Undo::Repeat(index, value) => self.repeats[index] = value,
            }
        }
    }

    fn start_group(&mut self, index: u32, pos: usize) -> Result<(), MatchError> {
        let old = self.starts[index as usize];
        checked_push(&mut self.undo, Undo::Start(index, old))?;
        self.starts[index as usize] = Some(pos);
        Ok(())
    }

    fn capture(&mut self, index: u32, start: usize, end: usize) -> Result<(), MatchError> {
        checked_push(
            &mut self.undo,
            Undo::Capture(index, self.groups[index as usize]),
        )?;
        checked_push(&mut self.undo, Undo::LastIndex(self.lastindex))?;
        self.groups[index as usize] = Some((start, end));
        self.lastindex = Some(index);
        Ok(())
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
                self.capture(recipe.index, start + recipe.start, start + recipe.end)?;
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
            greedy,
            next,
        } = self.compiled.program.code[pc]
        else {
            unreachable!("single retry opcode")
        };
        let width = self.compiled.program.singles[plan].width;
        let mut count = count;
        let mut pos = pos;
        if retry {
            if greedy {
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
            if greedy {
                while max.is_none_or(|max| count < max) && self.matches_one(plan, pos) {
                    count += 1;
                    pos += width;
                }
            }
        }
        // The checkpoint precedes capture projection: retries restore the state
        // before the loop, then reconstruct exactly the selected last iteration.
        if (greedy && count > min) || (!greedy && max.is_none_or(|max| count < max)) {
            self.choose(Retry::Single { pc, count, pos })?;
        }
        self.single_captures(plan, count, pos)?;
        Ok(Some((next, pos)))
    }

    /// Resume a priority alternative. Assertion-local alternatives cannot escape
    /// their barrier; successful positive assertions discard their retries.
    fn backtrack(&mut self) -> Result<Option<(usize, usize)>, MatchError> {
        loop {
            let barrier = self.assertions.last().map_or(0, |frame| frame.choices);
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
            } else if let Some(frame) = self.assertions.pop() {
                self.rollback(frame.undo);
                if !frame.positive {
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
        self.assertions.clear();
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
                Instruction::AssertionAccept => {
                    let frame = self.assertions.last().expect("assertion accept");
                    if frame.expected_end.is_some_and(|end| pos != end) {
                        failed = true;
                    } else {
                        let frame = self.assertions.pop().expect("assertion frame");
                        self.choices.truncate(frame.choices);
                        if frame.positive {
                            pc = frame.next;
                            pos = frame.pos;
                        } else {
                            self.rollback(frame.undo);
                            failed = true;
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
                    let start =
                        self.starts[index as usize].expect("compiled group start dominates end");
                    self.capture(index, start, pos)?;
                    pc = next;
                }
                Instruction::Backref { index, flags, next } => {
                    if let Some((start, end)) = self.groups.get(index as usize).copied().flatten() {
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
                    pc = if self.groups.get(index as usize).is_some_and(Option::is_some) {
                        yes
                    } else {
                        no
                    };
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
                            &mut self.assertions,
                            Assertion {
                                next,
                                pos,
                                expected_end: width.map(|_| pos),
                                positive,
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
                Instruction::RepeatEnter { id, next } => {
                    self.repeat(
                        id,
                        RepeatState {
                            count: 0,
                            cursor: pos,
                            stalled: false,
                        },
                    )?;
                    pc = next;
                }
                Instruction::Repeat {
                    id,
                    body,
                    min,
                    max,
                    greedy,
                    next,
                } => {
                    let state = self.repeats[id];
                    let more = max.is_none_or(|max| state.count < max)
                        && (!state.stalled || state.count < min);
                    if state.count < min {
                        if more {
                            pc = body;
                        } else {
                            failed = true;
                        }
                    } else if more {
                        let (first, second) = if greedy { (body, next) } else { (next, body) };
                        self.choose(Retry::Resume { pc: second, pos })?;
                        pc = first;
                    } else {
                        pc = next;
                    }
                }
                Instruction::RepeatStep { id, next } => {
                    let old = self.repeats[id];
                    self.repeat(
                        id,
                        RepeatState {
                            count: old.count + 1,
                            cursor: pos,
                            stalled: pos == old.cursor,
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

    pub(super) fn result(self, start: usize, end: usize) -> MatchResult {
        MatchResult {
            start,
            end,
            groups: self.groups,
            lastindex: self.lastindex,
        }
    }
}

#[inline]
fn ascii(flags: i64, is_bytes: bool) -> bool {
    is_bytes || flags & RE_ASCII != 0
}
#[inline]
fn word(ch: char, flags: i64, is_bytes: bool) -> bool {
    ch == '_'
        || if ascii(flags, is_bytes) {
            ch.is_ascii_alphanumeric()
        } else {
            ch.is_alphanumeric()
        }
}
#[inline]
pub(super) fn char_eq(a: char, b: char, flags: i64, is_bytes: bool) -> bool {
    if flags & RE_IGNORECASE == 0 {
        a == b
    } else if ascii(flags, is_bytes) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a.to_lowercase().eq(b.to_lowercase())
    }
}

fn category_matches(category: Category, ch: char, flags: i64, is_bytes: bool) -> bool {
    match category {
        Category::Digit => ch.is_ascii_digit(),
        Category::NotDigit => !ch.is_ascii_digit(),
        Category::Space => matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'),
        Category::NotSpace => !matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}'),
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
        Predicate::Any => flags & RE_DOTALL != 0 || ch != '\n',
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
                        (lo.to_ascii_lowercase()..=hi.to_ascii_lowercase())
                            .contains(&ch.to_ascii_lowercase())
                    } else {
                        let lower = |ch: char| ch.to_lowercase().next().unwrap_or(ch);
                        let upper = |ch: char| ch.to_uppercase().next().unwrap_or(ch);
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
            pos == 0 || (flags & RE_MULTILINE != 0 && pos > 0 && subject.unit(pos - 1) == '\n')
        }
        Anchor::End => {
            pos == end
                || (pos < end
                    && subject.unit(pos) == '\n'
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

    #[test]
    fn fixed_width_repeat_workspace_is_independent_of_subject_length() {
        let compiled = parse_pattern_typed("((a)())+a", 0, false)
            .unwrap()
            .compile()
            .unwrap();
        for len in [20_000, 200_000] {
            let text = vec!['a'; len];
            let mut matcher = Matcher::new(&compiled, SubjectStorage::Text(&text), len).unwrap();
            assert_eq!(matcher.run(0, true, None).unwrap(), Some(len));
            // Capacities retain peak allocations. This budget excludes the
            // deliberately prepared subject and includes every VM workspace.
            let bytes = matcher.starts.capacity() * size_of::<Option<usize>>()
                + matcher.groups.capacity() * size_of::<Option<(usize, usize)>>()
                + matcher.repeats.capacity() * size_of::<RepeatState>()
                + matcher.undo.capacity() * size_of::<Undo>()
                + matcher.choices.capacity() * size_of::<Choice>()
                + matcher.assertions.capacity() * size_of::<Assertion>();
            assert!(
                bytes < 8192,
                "fixed-width repeat retained {bytes} workspace bytes for {len} units"
            );
            let result = matcher.result(0, len);
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
        let compiled = parse_pattern_typed(&text, 0, false)
            .unwrap()
            .compile()
            .unwrap();
        assert_eq!(
            compiled.program.code.len(),
            2,
            "literal length must not expand VM dispatch storage"
        );
        let matched = execute_match(&compiled, &text, 0, text.len(), "fullmatch")
            .unwrap()
            .unwrap();
        assert_eq!(
            (matched.start, matched.end, matched.lastindex),
            (0, text.len(), None)
        );
    }
}
