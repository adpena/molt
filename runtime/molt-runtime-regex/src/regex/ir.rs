use super::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// IR node enum â€” mirrors the Python dataclasses in re/__init__.py
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum ReNode {
    Empty,
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
// Compiled pattern â€” stored in the global registry
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct CompiledPattern {
    pub root: ReNode,
    pub group_count: u32,
    pub group_names: HashMap<String, u32>,
    pub flags: i64,
    /// Position (char index) of a nested-set-in-charclass warning, or None.
    pub warn_pos: Option<i64>,
}
