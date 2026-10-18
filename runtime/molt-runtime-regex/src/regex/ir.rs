// ---------------------------------------------------------------------------
// IR node enum for the shared text/byte matcher.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum ReNode {
    Empty,
    /// Internal search continuation: accept only if the overall match has
    /// advanced beyond this attempt's starting position.
    RequireAdvance,
    Literal(String),
    Any,
    Anchor(String),
    CharClass {
        negated: bool,
        ranges: Vec<(String, String)>,
        chars: Vec<String>,
        categories: Vec<String>,
    },
    Concat(Vec<ReNode>),
    Alt(Vec<ReNode>),
    Repeat {
        node: Box<ReNode>,
        min_count: u64,
        max_count: Option<u64>,
        greedy: bool,
    },
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
pub(crate) struct CompiledPattern {
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
