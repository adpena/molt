use super::program::Program;

// ---------------------------------------------------------------------------
// IR node enum for the shared text/byte matcher.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    Start,
    End,
    StartAbsolute,
    EndAbsolute,
    Boundary,
    NotBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    Digit,
    NotDigit,
    Space,
    NotSpace,
    Word,
    NotWord,
}

impl Category {
    /// Called only after the parser admits one of the six category escapes.
    pub(super) fn from_escape(ch: char) -> Self {
        match ch {
            'd' => Self::Digit,
            'D' => Self::NotDigit,
            's' => Self::Space,
            'S' => Self::NotSpace,
            'w' => Self::Word,
            'W' => Self::NotWord,
            _ => unreachable!("admitted category escape"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepeatMode {
    Greedy,
    Lazy,
    Possessive,
}

#[derive(Debug)]
pub(crate) enum ReNode {
    Empty,
    Literal(String),
    Any,
    Anchor(Anchor),
    CharClass {
        negated: bool,
        ranges: Vec<(String, String)>,
        chars: Vec<String>,
        categories: Vec<Category>,
    },
    Concat(Vec<ReNode>),
    Alt(Vec<ReNode>),
    Repeat {
        node: Box<ReNode>,
        min_count: u64,
        max_count: Option<u64>,
        mode: RepeatMode,
    },
    Atomic(Box<ReNode>),
    Group {
        node: Box<ReNode>,
        index: u32,
    },
    Backref(u32),
    Look {
        node: Box<ReNode>,
        behind: bool,
        positive: bool,
        width: Option<u64>,
    },
    ScopedFlags {
        node: Box<ReNode>,
        add_flags: i64,
        clear_flags: i64,
    },
    Conditional {
        group_index: u32,
        yes: Box<ReNode>,
        no: Box<ReNode>,
    },
}

// ---------------------------------------------------------------------------
// Immutable compiled payload owned by PatternHandle.
// ---------------------------------------------------------------------------

/// One name-to-ordinal catalog, inserted when each named group opens. Its
/// iteration order is Python declaration order, independent of hash buckets;
/// lookups stay average O(1) and public metadata projection is linear in names.
pub(crate) type NamedGroups = indexmap::IndexMap<String, u32>;

#[derive(Debug)]
pub(crate) struct ParsedPattern {
    pub root: ReNode,
    pub group_count: u32,
    pub group_names: NamedGroups,
    pub flags: i64,
    /// Whether the parsed source was a bytes pattern. Pattern units are
    /// represented one-to-one as Unicode scalar values in the range 0..=255.
    pub is_bytes: bool,
    /// Position (char index) of a nested-set-in-charclass warning, or None.
    pub warn_pos: Option<i64>,
}

/// A pattern owns exactly one executable program. The AST is compilation input.
#[derive(Debug)]
pub(crate) struct CompiledPattern {
    pub(super) program: Program,
    pub group_count: u32,
    pub group_names: NamedGroups,
    pub flags: i64,
    pub is_bytes: bool,
    pub warn_pos: Option<i64>,
}
