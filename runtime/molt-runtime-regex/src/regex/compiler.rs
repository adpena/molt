use super::*;

// ---------------------------------------------------------------------------
// Phase-1 regex compiler: IR parser + compiled-pattern registry
// ---------------------------------------------------------------------------
//
// This section implements the Rust-side `molt_re_compile` / `molt_re_pattern_info`
// intrinsics (Phase 1).  The match engine (`molt_re_execute` /
// `molt_re_finditer_collect`) is a Phase-1b stub that always returns None /
// empty list, signalling Python to fall back to its own match engine.
//
// Design notes:
// * No `regex` crate — backreferences are required and not supported there.
// * Hand-rolled recursive-descent parser that mirrors the Python `_Parser` class
//   in `src/molt/stdlib/re/__init__.py` exactly (same quirks, same IR shape).
// * Compiled patterns are owned by the active runtime state so handles do not
//   survive teardown/reinit.
// * Handle allocation starts at 1 so that 0 can serve as "invalid".

use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicI64, Ordering},
};

// ---------------------------------------------------------------------------
// Re-use the flag constants already defined at the top of this file.
// (RE_IGNORECASE = 2, RE_VERBOSE = 64, RE_ASCII = 256)
// Additional flags:
pub(super) const RE_MULTILINE: i64 = 8;
pub(super) const RE_DOTALL: i64 = 16;
pub(super) const RE_UNICODE: i64 = 32;
pub(super) const RE_LOCALE: i64 = 4;

// ---------------------------------------------------------------------------
// IR node enum — mirrors the Python dataclasses in re/__init__.py
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
// Compiled pattern — stored in the global registry
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

// ---------------------------------------------------------------------------
// Runtime-scoped pattern registry
// ---------------------------------------------------------------------------

pub(super) struct RegexRuntimeState {
    pub(super) next_handle: AtomicI64,
    pub(super) patterns: Mutex<HashMap<i64, CompiledPattern>>,
}

impl RegexRuntimeState {
    pub(super) fn new() -> Self {
        Self {
            next_handle: AtomicI64::new(1),
            patterns: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn clear(&self) {
        self.patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

pub(super) unsafe extern "C" fn regex_runtime_state_init() -> *mut u8 {
    Box::into_raw(Box::new(RegexRuntimeState::new())) as *mut u8
}

pub(super) unsafe extern "C" fn regex_runtime_state_clear(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (&*(ptr as *const RegexRuntimeState)).clear();
    }
}

pub(super) unsafe extern "C" fn regex_runtime_state_drop(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut RegexRuntimeState));
    }
}

pub(super) fn regex_state(_py: &CoreGilToken) -> &'static RegexRuntimeState {
    let ptr = crate::bridge::runtime_state_get_or_init(
        b"molt-runtime-regex/patterns/v1",
        regex_runtime_state_init,
        regex_runtime_state_clear,
        regex_runtime_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt regex runtime state initialization failed"
    );
    unsafe { &*(ptr as *const RegexRuntimeState) }
}

pub(super) fn re_alloc_handle(_py: &CoreGilToken) -> i64 {
    regex_state(_py).next_handle.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn re_store_pattern(_py: &CoreGilToken, handle: i64, pattern: CompiledPattern) {
    let mut guard = regex_state(_py)
        .patterns
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.insert(handle, pattern);
}

// ---------------------------------------------------------------------------
// Parser state — mirrors _Parser in re/__init__.py
// ---------------------------------------------------------------------------

pub(super) struct ReParser {
    pub(super) chars: Vec<char>,
    pub(super) pos: usize,
    pub(super) group_count: u32,
    pub(super) group_names: HashMap<String, u32>,
    /// Fixed widths keyed by group index — used for look-behind validation.
    pub(super) group_widths: HashMap<u32, Option<u64>>,
    pub(super) open_group_names: std::collections::HashSet<String>,
    pub(super) flags: i64,
    pub(super) inline_flags: i64,
    pub(super) nested_set_warning_pos: Option<i64>,
    pub(super) in_class: bool,
}

impl ReParser {
    pub(super) fn new(pattern: &str, flags: i64) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
            group_count: 0,
            group_names: HashMap::new(),
            group_widths: HashMap::new(),
            open_group_names: std::collections::HashSet::new(),
            flags,
            inline_flags: 0,
            nested_set_warning_pos: None,
            in_class: false,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.chars.len()
    }

    pub(super) fn is_verbose(&self) -> bool {
        (self.flags | self.inline_flags) & RE_VERBOSE != 0
    }

    pub(super) fn skip_verbose_whitespace(&mut self) {
        if self.in_class || !self.is_verbose() {
            return;
        }
        while self.pos < self.len() {
            let ch = self.chars[self.pos];
            if ch == '#' {
                self.pos += 1;
                while self.pos < self.len() && self.chars[self.pos] != '\n' {
                    self.pos += 1;
                }
                if self.pos < self.len() {
                    self.pos += 1; // consume '\n'
                }
            } else if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0C' | '\x0B') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    pub(super) fn peek(&mut self) -> Option<char> {
        self.skip_verbose_whitespace();
        self.chars.get(self.pos).copied()
    }

    pub(super) fn next_ch(&mut self) -> Result<char, String> {
        self.skip_verbose_whitespace();
        if self.pos >= self.len() {
            return Err("unexpected end of pattern".to_string());
        }
        let ch = self.chars[self.pos];
        self.pos += 1;
        Ok(ch)
    }

    pub(super) fn raw_peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    pub(super) fn raw_next(&mut self) -> Result<char, String> {
        if self.pos >= self.len() {
            return Err("unexpected end of pattern".to_string());
        }
        let ch = self.chars[self.pos];
        self.pos += 1;
        Ok(ch)
    }

    // -----------------------------------------------------------------------
    // Top-level parse entry
    // -----------------------------------------------------------------------

    pub(super) fn parse(&mut self) -> Result<ReNode, String> {
        let node = self.parse_expr()?;
        self.skip_verbose_whitespace();
        if self.pos != self.len() {
            return Err("unexpected pattern text".to_string());
        }
        Ok(node)
    }

    // -----------------------------------------------------------------------
    // Expression = alternation
    // -----------------------------------------------------------------------

    pub(super) fn parse_expr(&mut self) -> Result<ReNode, String> {
        let mut terms = vec![self.parse_term()?];
        while self.peek() == Some('|') {
            self.next_ch()?;
            terms.push(self.parse_term()?);
        }
        if terms.len() == 1 {
            Ok(terms.remove(0))
        } else {
            Ok(ReNode::Alt(terms))
        }
    }

    // -----------------------------------------------------------------------
    // Term = sequence of factors
    // -----------------------------------------------------------------------

    pub(super) fn parse_term(&mut self) -> Result<ReNode, String> {
        let mut nodes: Vec<ReNode> = Vec::new();
        loop {
            let ch = self.peek();
            if ch.is_none() || ch == Some(')') || ch == Some('|') {
                break;
            }
            let node = self.parse_factor()?;
            // Coalesce adjacent literals.
            if let ReNode::Literal(ref new_text) = node
                && let Some(ReNode::Literal(prev_text)) = nodes.last_mut()
            {
                let combined = prev_text.clone() + new_text;
                *prev_text = combined;
                continue;
            }
            nodes.push(node);
        }
        if nodes.is_empty() {
            return Ok(ReNode::Empty);
        }
        if nodes.len() == 1 {
            return Ok(nodes.remove(0));
        }
        Ok(ReNode::Concat(nodes))
    }

    // -----------------------------------------------------------------------
    // Factor = atom + optional quantifier
    // -----------------------------------------------------------------------

    pub(super) fn parse_factor(&mut self) -> Result<ReNode, String> {
        let node = self.parse_atom()?;
        let ch = self.peek();
        if ch.is_none() {
            return Ok(node);
        }
        match ch.unwrap() {
            '*' | '+' | '?' => {
                let quant = self.next_ch()?;
                let (min_count, max_count) = match quant {
                    '*' => (0, None),
                    '+' => (1, None),
                    '?' => (0, Some(1)),
                    _ => unreachable!(),
                };
                let greedy = if self.peek() == Some('?') {
                    self.next_ch()?;
                    false
                } else {
                    true
                };
                Ok(ReNode::Repeat {
                    node: Box::new(node),
                    min_count,
                    max_count,
                    greedy,
                })
            }
            '{' => {
                let start_pos = self.pos;
                self.next_ch()?; // consume '{'
                let min_res = self.parse_number();
                if min_res.is_err() {
                    // Not a valid quantifier — backtrack.
                    self.pos = start_pos;
                    return Ok(node);
                }
                let min_count = min_res.unwrap();
                let max_count = if self.peek() == Some(',') {
                    self.next_ch()?; // consume ','
                    if self.peek() == Some('}') {
                        None // {n,} — unbounded
                    } else {
                        let max_res = self.parse_number();
                        if max_res.is_err() {
                            self.pos = start_pos;
                            return Ok(node);
                        }
                        Some(max_res.unwrap())
                    }
                } else {
                    Some(min_count)
                };
                if self.peek() != Some('}') {
                    // Backtrack — not a valid quantifier.
                    self.pos = start_pos;
                    return Ok(node);
                }
                self.next_ch()?; // consume '}'
                if let Some(max) = max_count
                    && max < min_count
                {
                    return Err("invalid quantifier range".to_string());
                }
                let greedy = if self.peek() == Some('?') {
                    self.next_ch()?;
                    false
                } else {
                    true
                };
                Ok(ReNode::Repeat {
                    node: Box::new(node),
                    min_count,
                    max_count,
                    greedy,
                })
            }
            _ => Ok(node),
        }
    }

    pub(super) fn parse_number(&mut self) -> Result<u64, String> {
        let mut digits = String::new();
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    self.next_ch()?;
                    digits.push(c);
                }
                _ => break,
            }
        }
        if digits.is_empty() {
            return Err("expected number".to_string());
        }
        digits
            .parse::<u64>()
            .map_err(|_| "number overflow".to_string())
    }

    // -----------------------------------------------------------------------
    // Atom
    // -----------------------------------------------------------------------

    pub(super) fn parse_atom(&mut self) -> Result<ReNode, String> {
        let ch = self.next_ch()?;
        match ch {
            '.' => Ok(ReNode::Any),
            '^' => Ok(ReNode::Anchor("start".to_string())),
            '$' => Ok(ReNode::Anchor("end".to_string())),
            '(' => self.parse_group(),
            '[' => self.parse_class(),
            '\\' => self.parse_escape(),
            c if ")*+?{}|".contains(c) => Err(format!("unexpected character '{c}'")),
            c => Ok(ReNode::Literal(c.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Group: (...) and (?...)
    // -----------------------------------------------------------------------

    pub(super) fn parse_group(&mut self) -> Result<ReNode, String> {
        if self.peek() == Some('?') {
            self.next_ch()?; // consume '?'
            return self.parse_extension_group();
        }
        // Capturing group — assign index at open-paren time (CPython order).
        self.group_count += 1;
        let idx = self.group_count;
        let node = self.parse_expr()?;
        if self.peek() != Some(')') {
            return Err("missing )".to_string());
        }
        self.next_ch()?;
        let width = fixed_width(&node, Some(&self.group_widths));
        self.group_widths.insert(idx, width);
        Ok(ReNode::Group {
            node: Box::new(node),
            index: idx,
        })
    }

    pub(super) fn parse_extension_group(&mut self) -> Result<ReNode, String> {
        let marker = self.peek();
        match marker {
            Some('=') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: true,
                    width: None,
                })
            }
            Some('!') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: false,
                    width: None,
                })
            }
            Some('<') => {
                self.next_ch()?; // consume '<'
                let look_kind = self.peek();
                match look_kind {
                    Some('=') => {
                        self.next_ch()?;
                        let node = self.parse_expr()?;
                        if self.peek() != Some(')') {
                            return Err("missing )".to_string());
                        }
                        self.next_ch()?;
                        let width =
                            fixed_width(&node, Some(&self.group_widths)).ok_or_else(|| {
                                "look-behind requires fixed-width pattern".to_string()
                            })?;
                        Ok(ReNode::Look {
                            node: Box::new(node),
                            behind: true,
                            positive: true,
                            width: Some(width),
                        })
                    }
                    Some('!') => {
                        self.next_ch()?;
                        let node = self.parse_expr()?;
                        if self.peek() != Some(')') {
                            return Err("missing )".to_string());
                        }
                        self.next_ch()?;
                        let width =
                            fixed_width(&node, Some(&self.group_widths)).ok_or_else(|| {
                                "look-behind requires fixed-width pattern".to_string()
                            })?;
                        Ok(ReNode::Look {
                            node: Box::new(node),
                            behind: true,
                            positive: false,
                            width: Some(width),
                        })
                    }
                    _ => {
                        // Named group: (?P<name>...) already consumed '<', so
                        // this is a raw (?<name>...) style from parse_extension_group.
                        // Actually we get here only when parse_extension_group sees
                        // marker=='<' and dispatches here.  We have already consumed '<'.
                        // So we must handle the named-group body:
                        self.parse_named_group_body()
                    }
                }
            }
            Some('(') => {
                // Conditional: (?(id)yes|no)
                self.next_ch()?; // consume '('
                let mut digits = String::new();
                loop {
                    match self.peek() {
                        Some(c) if c.is_ascii_digit() => {
                            self.next_ch()?;
                            digits.push(c);
                        }
                        _ => break,
                    }
                }
                if digits.is_empty() || self.peek() != Some(')') {
                    return Err("bad character in group name".to_string());
                }
                self.next_ch()?; // consume ')'
                let group_index = digits
                    .parse::<u32>()
                    .map_err(|_| "group index overflow".to_string())?;
                let yes_node = self.parse_term()?;
                let no_node = if self.peek() == Some('|') {
                    self.next_ch()?;
                    self.parse_term()?
                } else {
                    ReNode::Empty
                };
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::Conditional {
                    group_index,
                    yes: Box::new(yes_node),
                    no: Box::new(no_node),
                })
            }
            Some(':') => {
                // Non-capturing group.
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(node)
            }
            Some('P') => {
                self.next_ch()?; // consume 'P'
                let name_marker = self.peek();
                match name_marker {
                    Some('=') => {
                        // Named back-reference: (?P=name)
                        self.next_ch()?;
                        let name = self.read_until_close_paren()?;
                        if name.is_empty() {
                            return Err("missing group name".to_string());
                        }
                        if self.open_group_names.contains(&name) {
                            return Err("cannot refer to an open group".to_string());
                        }
                        let idx = self
                            .group_names
                            .get(&name)
                            .copied()
                            .ok_or_else(|| format!("unknown group name '{name}'"))?;
                        Ok(ReNode::Backref(idx))
                    }
                    Some('<') => {
                        // Named capturing group: (?P<name>...)
                        self.next_ch()?; // consume '<'
                        self.parse_named_group_body()
                    }
                    _ => Err("bad character in group name".to_string()),
                }
            }
            // Inline flags: (?imsxauL) or (?i:...) or (?-i:...)
            Some(c) if "imsxaLu-".contains(c) || c == ')' => self.parse_inline_flags(),
            _ => Err("unknown extension".to_string()),
        }
    }

    /// Read a group name terminated by ')'.  Consumes characters but NOT the ')'.
    pub(super) fn read_until_close_paren(&mut self) -> Result<String, String> {
        let mut name = String::new();
        loop {
            match self.peek() {
                None => return Err("missing )".to_string()),
                Some(')') => {
                    self.next_ch()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.next_ch()?;
                    name.push(c);
                }
            }
        }
        Ok(name)
    }

    /// Parse the body of a named group after '<' has been consumed.
    pub(super) fn parse_named_group_body(&mut self) -> Result<ReNode, String> {
        let mut name = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated group name".to_string()),
                Some('>') => {
                    self.next_ch()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.next_ch()?;
                    name.push(c);
                }
            }
        }
        if name.is_empty() {
            return Err("missing group name".to_string());
        }
        if self.group_names.contains_key(&name) || self.open_group_names.contains(&name) {
            return Err("redefinition of group name".to_string());
        }
        // Assign index at open-paren time (CPython order).
        self.group_count += 1;
        let idx = self.group_count;
        self.group_names.insert(name.clone(), idx);
        self.open_group_names.insert(name.clone());
        let parse_result = self.parse_expr();
        let node = match parse_result {
            Ok(n) => {
                self.open_group_names.remove(&name);
                n
            }
            Err(e) => {
                self.open_group_names.remove(&name);
                return Err(e);
            }
        };
        if self.peek() != Some(')') {
            return Err("missing )".to_string());
        }
        self.next_ch()?;
        let width = fixed_width(&node, Some(&self.group_widths));
        self.group_widths.insert(idx, width);
        Ok(ReNode::Group {
            node: Box::new(node),
            index: idx,
        })
    }

    /// Parse inline flags (?imsxauL) or (?i:...) or (?-i:...)
    pub(super) fn parse_inline_flags(&mut self) -> Result<ReNode, String> {
        let mut add_flags: i64 = 0;
        let mut clear_flags: i64 = 0;
        let mut seen_minus = false;
        loop {
            match self.peek() {
                None => return Err("unterminated inline flag".to_string()),
                Some('-') => {
                    self.next_ch()?;
                    seen_minus = true;
                }
                Some(
                    c @ ('i' | 'm' | 's' | 'x' | 'a' | 'L' | 'u' | 'I' | 'M' | 'S' | 'X' | 'A'
                    | 'U'),
                ) => {
                    self.next_ch()?;
                    let bit = flag_char_to_bit(c);
                    if seen_minus {
                        clear_flags |= bit;
                    } else {
                        add_flags |= bit;
                    }
                }
                _ => break,
            }
        }
        match self.peek() {
            Some(')') => {
                self.next_ch()?;
                self.inline_flags |= add_flags;
                self.inline_flags &= !clear_flags;
                Ok(ReNode::Empty)
            }
            Some(':') => {
                self.next_ch()?;
                let node = self.parse_expr()?;
                if self.peek() != Some(')') {
                    return Err("missing )".to_string());
                }
                self.next_ch()?;
                Ok(ReNode::ScopedFlags {
                    node: Box::new(node),
                    add_flags,
                    clear_flags,
                })
            }
            _ => Err("unsupported group extension syntax".to_string()),
        }
    }

    // -----------------------------------------------------------------------
    // Escape sequence
    // -----------------------------------------------------------------------

    pub(super) fn parse_escape(&mut self) -> Result<ReNode, String> {
        let ch = self.raw_next()?;
        match ch {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let negated = ch.is_ascii_uppercase();
                let category = if negated {
                    ((ch as u8) + 32) as char
                } else {
                    ch
                };
                Ok(ReNode::CharClass {
                    negated,
                    ranges: vec![],
                    chars: vec![],
                    categories: vec![category.to_string()],
                })
            }
            'n' => Ok(ReNode::Literal("\n".to_string())),
            't' => Ok(ReNode::Literal("\t".to_string())),
            'r' => Ok(ReNode::Literal("\r".to_string())),
            'f' => Ok(ReNode::Literal("\x0C".to_string())),
            'v' => Ok(ReNode::Literal("\x0B".to_string())),
            c @ '0'..='9' => {
                let mut digits = String::from(c);
                loop {
                    match self.peek() {
                        Some(d) if d.is_ascii_digit() => {
                            self.next_ch()?;
                            digits.push(d);
                        }
                        _ => break,
                    }
                }
                let idx = digits
                    .parse::<u32>()
                    .map_err(|_| "backref index overflow".to_string())?;
                Ok(ReNode::Backref(idx))
            }
            'A' => Ok(ReNode::Anchor("start_abs".to_string())),
            'Z' => Ok(ReNode::Anchor("end_abs".to_string())),
            'b' => Ok(ReNode::Anchor("word_boundary".to_string())),
            'B' => Ok(ReNode::Anchor("word_boundary_not".to_string())),
            other => Ok(ReNode::Literal(other.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Character class: [...]
    // -----------------------------------------------------------------------

    pub(super) fn parse_class(&mut self) -> Result<ReNode, String> {
        self.in_class = true;
        let mut negated = false;
        let mut chars: Vec<String> = Vec::new();
        let mut ranges: Vec<(String, String)> = Vec::new();
        let mut categories: Vec<String> = Vec::new();

        if self.raw_peek() == Some('^') {
            self.raw_next()?;
            negated = true;
        }
        // A ']' immediately after '[' or '[^' is a literal ']'.
        if self.raw_peek() == Some(']') {
            let c = self.raw_next()?;
            chars.push(c.to_string());
        }

        loop {
            match self.raw_peek() {
                None => {
                    self.in_class = false;
                    return Err("unterminated character class".to_string());
                }
                Some(']') => {
                    self.raw_next()?;
                    break;
                }
                _ => {}
            }
            match self.class_item()? {
                ClassItem::Range(s, e) => ranges.push((s, e)),
                ClassItem::Category(cat) => categories.push(cat),
                ClassItem::Char(c) => chars.push(c),
            }
        }
        self.in_class = false;
        Ok(ReNode::CharClass {
            negated,
            ranges,
            chars,
            categories,
        })
    }

    pub(super) fn class_item(&mut self) -> Result<ClassItem, String> {
        let ch = self.raw_next()?;
        match ch {
            '\\' => {
                let esc = self.raw_next()?;
                match esc {
                    'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                        let category = if esc.is_ascii_uppercase() {
                            ((esc as u8) + 32) as char
                        } else {
                            esc
                        };
                        Ok(ClassItem::Category(category.to_string()))
                    }
                    'n' => Ok(ClassItem::Char("\n".to_string())),
                    't' => Ok(ClassItem::Char("\t".to_string())),
                    'r' => Ok(ClassItem::Char("\r".to_string())),
                    'f' => Ok(ClassItem::Char("\x0C".to_string())),
                    'v' => Ok(ClassItem::Char("\x0B".to_string())),
                    c if c.is_ascii_digit() => {
                        // Octal escape inside character classes.
                        let mut oct = String::from(c);
                        while oct.len() < 3 {
                            match self.raw_peek() {
                                Some(d) if ('0'..='7').contains(&d) => {
                                    self.raw_next()?;
                                    oct.push(d);
                                }
                                _ => break,
                            }
                        }
                        let code = u32::from_str_radix(&oct, 8)
                            .map_err(|_| "invalid octal escape".to_string())?;
                        let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                        Ok(ClassItem::Char(c.to_string()))
                    }
                    other => Ok(ClassItem::Char(other.to_string())),
                }
            }
            '[' if self.raw_peek() == Some(':') => {
                // POSIX class: [:alpha:]
                if self.nested_set_warning_pos.is_none() {
                    self.nested_set_warning_pos = Some((self.pos as i64) - 1);
                }
                self.raw_next()?; // consume ':'
                let mut name = String::new();
                loop {
                    match self.raw_peek() {
                        None => return Err("unterminated character class".to_string()),
                        Some(':') => {
                            self.raw_next()?;
                            if self.raw_peek() == Some(']') {
                                self.raw_next()?;
                                break;
                            }
                            name.push(':');
                        }
                        Some(c) => {
                            self.raw_next()?;
                            name.push(c);
                        }
                    }
                }
                Ok(ClassItem::Category(format!("posix:{name}")))
            }
            '-' | ']' => Ok(ClassItem::Char(ch.to_string())),
            _ => {
                // Maybe a range: X-Y
                if self.raw_peek() == Some('-') {
                    let saved_pos = self.pos;
                    self.raw_next()?; // consume '-'
                    match self.raw_peek() {
                        None | Some(']') => {
                            // Not a range — backtrack the '-'.
                            self.pos = saved_pos;
                            Ok(ClassItem::Char(ch.to_string()))
                        }
                        _ => {
                            let end_item = self.class_item()?;
                            match end_item {
                                ClassItem::Char(end) => Ok(ClassItem::Range(ch.to_string(), end)),
                                _ => Err("ranges over categories are not supported".to_string()),
                            }
                        }
                    }
                } else {
                    Ok(ClassItem::Char(ch.to_string()))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ClassItem helper enum (only used inside parser)
// ---------------------------------------------------------------------------

pub(super) enum ClassItem {
    Char(String),
    Range(String, String),
    Category(String),
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(super) const META_CHARS: &str = ".^$*+?{}[]\\|()";

pub(super) fn is_meta_char(c: char) -> bool {
    META_CHARS.contains(c)
}

pub(super) fn flag_char_to_bit(c: char) -> i64 {
    match c {
        'i' | 'I' => RE_IGNORECASE,
        'm' | 'M' => RE_MULTILINE,
        's' | 'S' => RE_DOTALL,
        'x' | 'X' => RE_VERBOSE,
        'a' | 'A' => RE_ASCII,
        'u' | 'U' => RE_UNICODE,
        'L' | 'l' => RE_LOCALE,
        _ => 0,
    }
}

/// Compute the fixed character-width of a regex node, analogous to
/// `_fixed_width()` in re/__init__.py.  Returns `None` for variable-width
/// nodes.
pub(super) fn fixed_width(
    node: &ReNode,
    group_widths: Option<&HashMap<u32, Option<u64>>>,
) -> Option<u64> {
    match node {
        ReNode::Empty => Some(0),
        ReNode::Literal(s) => Some(s.chars().count() as u64),
        ReNode::Any => Some(1),
        ReNode::Anchor(_) => Some(0),
        ReNode::CharClass { .. } => Some(1),
        ReNode::Backref(idx) => group_widths?.get(idx).copied().flatten(),
        ReNode::Group { node, .. } => fixed_width(node, group_widths),
        ReNode::Look { .. } => Some(0),
        ReNode::ScopedFlags { node, .. } => fixed_width(node, group_widths),
        ReNode::Conditional { yes, no, .. } => {
            let yw = fixed_width(yes, group_widths)?;
            let nw = fixed_width(no, group_widths)?;
            if yw != nw { None } else { Some(yw) }
        }
        ReNode::Concat(nodes) => {
            let mut total = 0u64;
            for n in nodes {
                total += fixed_width(n, group_widths)?;
            }
            Some(total)
        }
        ReNode::Alt(options) => {
            if options.is_empty() {
                return Some(0);
            }
            let first = fixed_width(&options[0], group_widths)?;
            for opt in &options[1..] {
                let w = fixed_width(opt, group_widths)?;
                if w != first {
                    return None;
                }
            }
            Some(first)
        }
        ReNode::Repeat {
            node,
            min_count,
            max_count,
            ..
        } => {
            let w = fixed_width(node, group_widths)?;
            let max = (*max_count)?;
            if *min_count != max {
                return None;
            }
            Some(w * max)
        }
    }
}

// ---------------------------------------------------------------------------
// parse_pattern: top-level entry point
// ---------------------------------------------------------------------------

pub(super) fn parse_pattern(pattern: &str, flags: i64) -> Result<CompiledPattern, String> {
    let mut parser = ReParser::new(pattern, flags);
    let root = parser.parse()?;
    let group_count = parser.group_count;
    let group_names = parser.group_names;
    let inline_flags = parser.inline_flags;
    let warn_pos = parser.nested_set_warning_pos;
    Ok(CompiledPattern {
        root,
        group_count,
        group_names,
        flags: flags | inline_flags,
        warn_pos,
    })
}

// ---------------------------------------------------------------------------
// molt_re_compile intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_compile(pattern: str, flags: int) -> int`
///
/// Parse a regex pattern string and return an opaque integer handle.  The
/// compiled `CompiledPattern` is stored in the active runtime registry.
/// Returns -1 and raises `re.error` on parse failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_compile(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        match parse_pattern(&pattern, flags) {
            Ok(compiled) => {
                let handle = re_alloc_handle(_py);
                re_store_pattern(_py, handle, compiled);
                MoltObject::from_int(handle).bits()
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_pattern_info intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_pattern_info(handle: int) -> (groups, group_names_dict, flags, warn_pos)`
///
/// Returns a 4-tuple:
///   0: groups      — int,   number of capturing groups
///   1: group_names — dict,  {name: index}
///   2: flags       — int,   effective flags (pattern flags | inline flags)
///   3: warn_pos    — int or None,  char position of nested-set warning (or None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_pattern_info(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        // Build group_names dict.
        let mut pairs: Vec<u64> = Vec::with_capacity(compiled.group_names.len() * 2);
        for (name, &idx) in &compiled.group_names {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let idx_bits = MoltObject::from_int(idx as i64).bits();
            pairs.push(name_bits);
            pairs.push(idx_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();

        let groups_bits = MoltObject::from_int(compiled.group_count as i64).bits();
        let flags_bits_out = MoltObject::from_int(compiled.flags).bits();
        let warn_bits = match compiled.warn_pos {
            Some(pos) => MoltObject::from_int(pos).bits(),
            None => MoltObject::none().bits(),
        };

        let tuple_ptr = alloc_tuple(_py, &[groups_bits, dict_bits, flags_bits_out, warn_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
