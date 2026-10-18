use super::*;

/// The sole executable representation. Parser nodes are consumed at compilation;
/// instructions carry their effective flags and typed successors.
#[derive(Debug)]
pub(super) struct Program {
    pub(super) code: Vec<Instruction>,
    pub(super) entry: usize,
    pub(super) repeats: usize,
    pub(super) singles: Vec<SingleRepeat>,
}

#[derive(Debug)]
pub(super) enum Predicate {
    Unit(u32),
    Literal(Vec<u32>),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(u32, u32)>,
        chars: Vec<u32>,
        categories: Vec<Category>,
    },
}

#[derive(Debug)]
pub(super) struct Atom {
    pub(super) predicate: Predicate,
    pub(super) flags: i64,
}

#[derive(Debug)]
pub(super) struct CaptureRecipe {
    pub(super) index: u32,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug)]
pub(super) struct SingleRepeat {
    pub(super) atoms: Vec<Atom>,
    /// Closing order matters for lastindex, including nested captures.
    pub(super) captures: Vec<CaptureRecipe>,
    pub(super) width: usize,
}

#[derive(Debug)]
pub(super) enum Instruction {
    Accept,
    ScopeAccept,
    Consume {
        atom: Atom,
        next: usize,
    },
    Anchor {
        kind: Anchor,
        flags: i64,
        next: usize,
    },
    Split {
        first: usize,
        second: usize,
    },
    GroupStart {
        index: u32,
        next: usize,
    },
    GroupEnd {
        index: u32,
        next: usize,
    },
    Backref {
        index: u32,
        flags: i64,
        next: usize,
    },
    Conditional {
        index: u32,
        yes: usize,
        no: usize,
    },
    Assert {
        entry: usize,
        width: Option<u64>,
        positive: bool,
        next: usize,
    },
    Atomic {
        entry: usize,
        next: usize,
    },
    Possessive {
        entry: usize,
        min: u64,
        max: Option<u64>,
        next: usize,
    },
    RepeatEnter {
        id: usize,
        next: usize,
    },
    Repeat {
        id: usize,
        body: usize,
        optional: usize,
        min: u64,
        max: Option<u64>,
        greedy: bool,
        next: usize,
    },
    RepeatStep {
        id: usize,
        next: usize,
    },
    RepeatOptional {
        id: usize,
        next: usize,
    },
    RepeatOne {
        plan: usize,
        min: u64,
        max: Option<u64>,
        mode: RepeatMode,
        next: usize,
    },
}

#[derive(Debug)]
pub(super) enum MatchError {
    Memory,
    InvalidMode,
    Internal,
}

impl From<std::collections::TryReserveError> for MatchError {
    fn from(_: std::collections::TryReserveError) -> Self {
        Self::Memory
    }
}

/// Amortized growth, with allocation failure distinguished from no match.
pub(super) fn checked_push<T>(items: &mut Vec<T>, item: T) -> Result<(), MatchError> {
    if items.len() == items.capacity() {
        items.try_reserve(1)?;
    }
    items.push(item);
    Ok(())
}

impl ParsedPattern {
    pub(super) fn compile(self) -> Result<CompiledPattern, MatchError> {
        let mut program = Program {
            code: Vec::new(),
            entry: 0,
            repeats: 0,
            singles: Vec::new(),
        };
        let accept = program.emit(Instruction::Accept)?;
        program.entry = program.lower(self.root, self.flags, accept)?;
        Ok(CompiledPattern {
            program,
            group_count: self.group_count,
            group_names: self.group_names,
            flags: self.flags,
            is_bytes: self.is_bytes,
            warn_pos: self.warn_pos,
        })
    }
}

impl Program {
    fn emit(&mut self, instruction: Instruction) -> Result<usize, MatchError> {
        let pc = self.code.len();
        checked_push(&mut self.code, instruction)?;
        Ok(pc)
    }

    fn lower(&mut self, node: ReNode, flags: i64, next: usize) -> Result<usize, MatchError> {
        Ok(match node {
            ReNode::Empty => next,
            ReNode::Literal(text) => self.emit(Instruction::Consume {
                atom: Atom {
                    predicate: literal_predicate(text),
                    flags,
                },
                next,
            })?,
            ReNode::Any => self.emit(Instruction::Consume {
                atom: Atom {
                    predicate: Predicate::Any,
                    flags,
                },
                next,
            })?,
            ReNode::CharClass {
                negated,
                ranges,
                chars,
                categories,
            } => self.emit(Instruction::Consume {
                atom: Atom {
                    predicate: Predicate::Class {
                        negated,
                        ranges,
                        chars,
                        categories,
                    },
                    flags,
                },
                next,
            })?,
            ReNode::Anchor(kind) => self.emit(Instruction::Anchor { kind, flags, next })?,
            ReNode::Concat(nodes) => {
                let mut entry = next;
                for node in nodes.into_iter().rev() {
                    entry = self.lower(node, flags, entry)?;
                }
                entry
            }
            ReNode::Alt(nodes) => {
                let mut entries = nodes.into_iter().rev();
                let mut entry = match entries.next() {
                    Some(node) => self.lower(node, flags, next)?,
                    None => next,
                };
                for node in entries {
                    let first = self.lower(node, flags, next)?;
                    entry = self.emit(Instruction::Split {
                        first,
                        second: entry,
                    })?;
                }
                entry
            }
            ReNode::Group { node, index } => {
                let close = self.emit(Instruction::GroupEnd { index, next })?;
                let body = self.lower(*node, flags, close)?;
                self.emit(Instruction::GroupStart { index, next: body })?
            }
            ReNode::ScopedFlags {
                node,
                add_flags,
                clear_flags,
            } => self.lower(*node, (flags | add_flags) & !clear_flags, next)?,
            ReNode::Backref(index) => self.emit(Instruction::Backref { index, flags, next })?,
            ReNode::Conditional {
                group_index,
                yes,
                no,
            } => {
                let yes = self.lower(*yes, flags, next)?;
                let no = self.lower(*no, flags, next)?;
                self.emit(Instruction::Conditional {
                    index: group_index,
                    yes,
                    no,
                })?
            }
            ReNode::Look {
                node,
                behind,
                positive,
                width,
            } => {
                let accept = self.emit(Instruction::ScopeAccept)?;
                let entry = self.lower(*node, flags, accept)?;
                self.emit(Instruction::Assert {
                    entry,
                    width: if behind { width } else { None },
                    positive,
                    next,
                })?
            }
            ReNode::Atomic(node) => {
                let accept = self.emit(Instruction::ScopeAccept)?;
                let entry = self.lower(*node, flags, accept)?;
                self.emit(Instruction::Atomic { entry, next })?
            }
            ReNode::Repeat {
                node,
                min_count,
                max_count,
                mode,
            } => {
                if let Some(plan) = single_repeat(&node, flags)? {
                    let index = self.singles.len();
                    checked_push(&mut self.singles, plan)?;
                    self.emit(Instruction::RepeatOne {
                        plan: index,
                        min: min_count,
                        max: max_count,
                        mode,
                        next,
                    })?
                } else if mode == RepeatMode::Possessive {
                    // CPython commits each body match separately, including
                    // mandatory iterations. One atomic wrapper around a normal
                    // repeat would wrongly reconsider prior iterations.
                    let accept = self.emit(Instruction::ScopeAccept)?;
                    let entry = self.lower(*node, flags, accept)?;
                    self.emit(Instruction::Possessive {
                        entry,
                        min: min_count,
                        max: max_count,
                        next,
                    })?
                } else {
                    let id = self.repeats;
                    self.repeats += 1;
                    let header = self.emit(Instruction::Accept)?;
                    let step = self.emit(Instruction::RepeatStep { id, next: header })?;
                    let body = self.lower(*node, flags, step)?;
                    let optional = self.emit(Instruction::RepeatOptional { id, next: body })?;
                    self.code[header] = Instruction::Repeat {
                        id,
                        body,
                        optional,
                        min: min_count,
                        max: max_count,
                        greedy: mode == RepeatMode::Greedy,
                        next,
                    };
                    self.emit(Instruction::RepeatEnter { id, next: header })?
                }
            }
        })
    }
}

fn literal_predicate(text: Vec<u32>) -> Predicate {
    if text.len() == 1 {
        Predicate::Unit(text[0])
    } else {
        Predicate::Literal(text)
    }
}

fn copy_units<T: Copy>(units: &[T]) -> Result<Vec<T>, MatchError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(units.len())?;
    copy.extend_from_slice(units);
    Ok(copy)
}

/// Structural specialization: every iteration consumes a fixed positive number
/// of units and closes the same groups. No captures or alternatives are lost.
fn single_repeat(node: &ReNode, flags: i64) -> Result<Option<SingleRepeat>, MatchError> {
    fn visit(node: &ReNode, flags: i64, plan: &mut SingleRepeat) -> Result<bool, MatchError> {
        match node {
            ReNode::Empty => {}
            ReNode::Literal(text) => {
                plan.width += text.len();
                checked_push(
                    &mut plan.atoms,
                    Atom {
                        predicate: match text.as_slice() {
                            [unit] => Predicate::Unit(*unit),
                            units => Predicate::Literal(copy_units(units)?),
                        },
                        flags,
                    },
                )?;
            }
            ReNode::Any => {
                checked_push(
                    &mut plan.atoms,
                    Atom {
                        predicate: Predicate::Any,
                        flags,
                    },
                )?;
                plan.width += 1;
            }
            ReNode::CharClass {
                negated,
                ranges,
                chars,
                categories,
            } => {
                checked_push(
                    &mut plan.atoms,
                    Atom {
                        predicate: Predicate::Class {
                            negated: *negated,
                            ranges: copy_units(ranges)?,
                            chars: copy_units(chars)?,
                            categories: copy_units(categories)?,
                        },
                        flags,
                    },
                )?;
                plan.width += 1;
            }
            ReNode::Concat(nodes) => {
                for node in nodes {
                    if !visit(node, flags, plan)? {
                        return Ok(false);
                    }
                }
            }
            ReNode::Group { node, index } => {
                let start = plan.width;
                if !visit(node, flags, plan)? {
                    return Ok(false);
                }
                checked_push(
                    &mut plan.captures,
                    CaptureRecipe {
                        index: *index,
                        start,
                        end: plan.width,
                    },
                )?;
            }
            ReNode::ScopedFlags {
                node,
                add_flags,
                clear_flags,
            } => return visit(node, (flags | add_flags) & !clear_flags, plan),
            _ => return Ok(false),
        }
        Ok(true)
    }
    let mut plan = SingleRepeat {
        atoms: Vec::new(),
        captures: Vec::new(),
        width: 0,
    };
    Ok(if visit(node, flags, &mut plan)? && plan.width != 0 {
        Some(plan)
    } else {
        None
    })
}
