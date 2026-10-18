use super::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Compile-only parser state for the shared typed regex program.
// ---------------------------------------------------------------------------

pub(super) struct ReParser {
    pub(super) chars: Vec<u32>,
    pub(super) pos: usize,
    pub(super) group_count: u32,
    pub(super) group_names: NamedGroups,
    /// Fixed widths keyed by group index â€” used for look-behind validation.
    pub(super) group_widths: HashMap<u32, Option<u64>>,
    pub(super) open_group_names: std::collections::HashSet<String>,
    pub(super) flags: i64,
    pub(super) is_bytes: bool,
    pub(super) inline_flags: i64,
    pub(super) nested_set_warning_pos: Option<i64>,
    pub(super) in_class: bool,
}

impl ReParser {
    pub(super) fn new(pattern: Vec<u32>, flags: i64) -> Self {
        Self {
            chars: pattern,
            pos: 0,
            group_count: 0,
            group_names: NamedGroups::new(),
            group_widths: HashMap::new(),
            open_group_names: std::collections::HashSet::new(),
            flags,
            is_bytes: false,
            inline_flags: 0,
            nested_set_warning_pos: None,
            in_class: false,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.chars.len()
    }

    pub(super) fn is_verbose(&self) -> bool {
        self.flags & RE_VERBOSE != 0
    }

    pub(super) fn skip_verbose_whitespace(&mut self) {
        if self.in_class || !self.is_verbose() {
            return;
        }
        while self.pos < self.len() {
            let ch = self.chars[self.pos];
            if ch == u32::from(b'#') {
                self.pos += 1;
                while self.pos < self.len() && self.chars[self.pos] != u32::from(b'\n') {
                    self.pos += 1;
                }
                if self.pos < self.len() {
                    self.pos += 1; // consume '\n'
                }
            } else if matches!(ch, 0x20 | 0x09..=0x0d) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    pub(super) fn peek(&mut self) -> Option<u32> {
        self.skip_verbose_whitespace();
        self.chars.get(self.pos).copied()
    }

    pub(super) fn next_ch(&mut self) -> Result<u32, String> {
        self.skip_verbose_whitespace();
        if self.pos >= self.len() {
            return Err("unexpected end of pattern".to_string());
        }
        let ch = self.chars[self.pos];
        self.pos += 1;
        Ok(ch)
    }

    pub(super) fn raw_peek(&self) -> Option<u32> {
        self.chars.get(self.pos).copied()
    }

    pub(super) fn raw_next(&mut self) -> Result<u32, String> {
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
        while self.peek() == Some(u32::from('|')) {
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
            if ch.is_none() || ch == Some(u32::from(')')) || ch == Some(u32::from('|')) {
                break;
            }
            let node = self.parse_factor()?;
            // Coalesce adjacent literals.
            if let ReNode::Literal(ref new_text) = node
                && let Some(ReNode::Literal(prev_text)) = nodes.last_mut()
            {
                prev_text.extend_from_slice(new_text);
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
        let bounds = match self.peek().and_then(char::from_u32) {
            Some('*') => {
                self.raw_next()?;
                Some((0, None))
            }
            Some('+') => {
                self.raw_next()?;
                Some((1, None))
            }
            Some('?') => {
                self.raw_next()?;
                Some((0, Some(1)))
            }
            Some('{') => self.parse_braced_repeat()?,
            _ => None,
        };
        let Some((min_count, max_count)) = bounds else {
            return Ok(node);
        };
        let mode = self.parse_repeat_mode()?;
        Ok(ReNode::Repeat {
            node: Box::new(node),
            min_count,
            max_count,
            mode,
        })
    }

    /// A quantifier is one contiguous token. Incomplete braces are literals;
    /// verbose whitespace/comments may occur around, never within, the token.
    fn parse_braced_repeat(&mut self) -> Result<Option<(u64, Option<u64>)>, String> {
        let start = self.pos;
        self.raw_next()?;
        let min_start = self.pos;
        while self.raw_peek().is_some_and(|c| (0x30..=0x39).contains(&c)) {
            self.pos += 1;
        }
        let min_end = self.pos;
        let comma = self.raw_peek() == Some(u32::from(','));
        let max_start;
        let max_end;
        if comma {
            self.pos += 1;
            max_start = self.pos;
            while self.raw_peek().is_some_and(|c| (0x30..=0x39).contains(&c)) {
                self.pos += 1;
            }
            max_end = self.pos;
        } else {
            max_start = min_start;
            max_end = min_end;
        }
        if self.raw_peek() != Some(u32::from('}')) || (!comma && min_start == min_end) {
            self.pos = start;
            return Ok(None);
        }
        self.pos += 1;
        let number = |start: usize, end: usize| {
            self.chars[start..end]
                .iter()
                .try_fold(0u64, |value, c| {
                    value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((*c as u64) - ('0' as u64)))
                })
                .ok_or_else(|| "number overflow".to_string())
        };
        let min = number(min_start, min_end)?;
        let max = if comma && max_start == max_end {
            None
        } else {
            Some(number(max_start, max_end)?)
        };
        if max.is_some_and(|max| max < min) {
            return Err("invalid quantifier range".to_string());
        }
        Ok(Some((min, max)))
    }

    fn parse_repeat_mode(&mut self) -> Result<RepeatMode, String> {
        Ok(match self.raw_peek().and_then(char::from_u32) {
            Some('?') => {
                self.raw_next()?;
                RepeatMode::Lazy
            }
            Some('+') => {
                self.raw_next()?;
                RepeatMode::Possessive
            }
            _ => RepeatMode::Greedy,
        })
    }

    // -----------------------------------------------------------------------
    // Atom
    // -----------------------------------------------------------------------

    pub(super) fn parse_atom(&mut self) -> Result<ReNode, String> {
        let ch = self.next_ch()?;
        match char::from_u32(ch) {
            Some('.') => Ok(ReNode::Any),
            Some('^') => Ok(ReNode::Anchor(Anchor::Start)),
            Some('$') => Ok(ReNode::Anchor(Anchor::End)),
            Some('(') => self.parse_group(),
            Some('[') => self.parse_class(),
            Some('{') => {
                self.pos -= 1;
                if self.parse_braced_repeat()?.is_some() {
                    return Err("nothing to repeat".to_string());
                }
                self.raw_next()?;
                Ok(ReNode::Literal(vec![u32::from(b'{')]))
            }
            Some('\\') => self.parse_escape(),
            Some(c) if ")*+?|".contains(c) => Err(format!("unexpected character '{c}'")),
            _ => Ok(ReNode::Literal(vec![ch])),
        }
    }

    // -----------------------------------------------------------------------
    // Group: (...) and (?...)
    // -----------------------------------------------------------------------

    pub(super) fn parse_group(&mut self) -> Result<ReNode, String> {
        if self.raw_peek() == Some(u32::from('?')) {
            self.raw_next()?; // consume '?'
            return self.parse_extension_group();
        }
        // Capturing group â€” assign index at open-paren time (CPython order).
        self.group_count += 1;
        let idx = self.group_count;
        let node = self.parse_expr()?;
        if self.peek() != Some(u32::from(')')) {
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
        let marker = self.raw_peek().and_then(char::from_u32);
        match marker {
            Some('>') => {
                self.raw_next()?;
                let node = self.parse_expr()?;
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
                Ok(ReNode::Atomic(Box::new(node)))
            }
            Some('=') => {
                self.raw_next()?;
                let node = self.parse_expr()?;
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: true,
                    width: None,
                })
            }
            Some('!') => {
                self.raw_next()?;
                let node = self.parse_expr()?;
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
                Ok(ReNode::Look {
                    node: Box::new(node),
                    behind: false,
                    positive: false,
                    width: None,
                })
            }
            Some('<') => {
                self.raw_next()?; // consume '<'
                let look_kind = self.raw_peek().and_then(char::from_u32);
                match look_kind {
                    Some('=') => {
                        self.raw_next()?;
                        let node = self.parse_expr()?;
                        if self.raw_peek() != Some(u32::from(')')) {
                            return Err("missing )".to_string());
                        }
                        self.raw_next()?;
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
                        self.raw_next()?;
                        let node = self.parse_expr()?;
                        if self.raw_peek() != Some(u32::from(')')) {
                            return Err("missing )".to_string());
                        }
                        self.raw_next()?;
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
                    _ => Err("unknown extension".to_string()),
                }
            }
            Some('(') => {
                // Conditional: (?(id)yes|no)
                self.raw_next()?; // consume '('
                let mut digits = String::new();
                loop {
                    match self.raw_peek().and_then(char::from_u32) {
                        Some(c) if c.is_ascii_digit() => {
                            self.raw_next()?;
                            digits.push(c);
                        }
                        _ => break,
                    }
                }
                if digits.is_empty() || self.raw_peek() != Some(u32::from(')')) {
                    return Err("bad character in group name".to_string());
                }
                self.raw_next()?; // consume ')'
                let group_index = digits
                    .parse::<u32>()
                    .map_err(|_| "group index overflow".to_string())?;
                let yes_node = self.parse_term()?;
                let no_node = if self.raw_peek() == Some(u32::from('|')) {
                    self.raw_next()?;
                    self.parse_term()?
                } else {
                    ReNode::Empty
                };
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
                Ok(ReNode::Conditional {
                    group_index,
                    yes: Box::new(yes_node),
                    no: Box::new(no_node),
                })
            }
            Some(':') => {
                // Non-capturing group.
                self.raw_next()?;
                let node = self.parse_expr()?;
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
                Ok(node)
            }
            Some('P') => {
                self.raw_next()?; // consume 'P'
                let name_marker = self.raw_peek().and_then(char::from_u32);
                match name_marker {
                    Some('=') => {
                        // Named back-reference: (?P=name)
                        self.raw_next()?;
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
                        self.raw_next()?; // consume '<'
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

    /// Read a raw group name and consume its terminating ')'.
    pub(super) fn read_until_close_paren(&mut self) -> Result<String, String> {
        let mut name = String::new();
        loop {
            let scalar = match self.raw_peek() {
                Some(unit) => Some(
                    char::from_u32(unit)
                        .ok_or_else(|| "bad character in group name".to_string())?,
                ),
                None => None,
            };
            match scalar {
                None => return Err("missing )".to_string()),
                Some(')') => {
                    self.raw_next()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' || c.is_ascii_whitespace() => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.raw_next()?;
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
            let scalar = match self.raw_peek() {
                Some(unit) => Some(
                    char::from_u32(unit)
                        .ok_or_else(|| "bad character in group name".to_string())?,
                ),
                None => None,
            };
            match scalar {
                None => return Err("unterminated group name".to_string()),
                Some('>') => {
                    self.raw_next()?;
                    break;
                }
                Some(c) if is_meta_char(c) || c == '<' || c == '>' || c.is_ascii_whitespace() => {
                    return Err("bad character in group name".to_string());
                }
                Some(c) => {
                    self.raw_next()?;
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
        if self.peek() != Some(u32::from(')')) {
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
            match self.raw_peek().and_then(char::from_u32) {
                None => return Err("unterminated inline flag".to_string()),
                Some('-') => {
                    self.raw_next()?;
                    seen_minus = true;
                }
                Some(
                    c @ ('i' | 'm' | 's' | 'x' | 'a' | 'L' | 'u' | 'I' | 'M' | 'S' | 'X' | 'A'
                    | 'U'),
                ) => {
                    self.raw_next()?;
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
        match self.raw_peek().and_then(char::from_u32) {
            Some(')') => {
                self.raw_next()?;
                self.inline_flags |= add_flags;
                self.inline_flags &= !clear_flags;
                self.flags = (self.flags | add_flags) & !clear_flags;
                Ok(ReNode::Empty)
            }
            Some(':') => {
                self.raw_next()?;
                let saved_flags = self.flags;
                self.flags = (self.flags | add_flags) & !clear_flags;
                let result = self.parse_expr();
                self.flags = saved_flags;
                let node = result?;
                if self.raw_peek() != Some(u32::from(')')) {
                    return Err("missing )".to_string());
                }
                self.raw_next()?;
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
        if self.is_bytes && matches!(char::from_u32(ch), Some('u' | 'U' | 'N')) {
            return Err(format!(
                "bad escape \\{} at position {}",
                char::from_u32(ch).unwrap(),
                self.pos - 2
            ));
        }
        match char::from_u32(ch) {
            Some('d' | 'D' | 's' | 'S' | 'w' | 'W') => Ok(ReNode::CharClass {
                negated: false,
                ranges: vec![],
                chars: vec![],
                categories: vec![Category::from_escape(char::from_u32(ch).unwrap())],
            }),
            Some('n') => Ok(ReNode::Literal(vec![u32::from('\n')])),
            Some('t') => Ok(ReNode::Literal(vec![u32::from('\t')])),
            Some('r') => Ok(ReNode::Literal(vec![u32::from('\r')])),
            Some('f') => Ok(ReNode::Literal(vec![u32::from('\x0C')])),
            Some('v') => Ok(ReNode::Literal(vec![u32::from('\x0B')])),
            Some(c @ '0'..='9') => {
                let mut digits = String::from(c);
                loop {
                    match self.raw_peek().and_then(char::from_u32) {
                        Some(d) if d.is_ascii_digit() => {
                            self.raw_next()?;
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
            Some('A') => Ok(ReNode::Anchor(Anchor::StartAbsolute)),
            Some('Z') => Ok(ReNode::Anchor(Anchor::EndAbsolute)),
            Some('b') => Ok(ReNode::Anchor(Anchor::Boundary)),
            Some('B') => Ok(ReNode::Anchor(Anchor::NotBoundary)),
            _ => Ok(ReNode::Literal(vec![ch])),
        }
    }

    // -----------------------------------------------------------------------
    // Character class: [...]
    // -----------------------------------------------------------------------

    pub(super) fn parse_class(&mut self) -> Result<ReNode, String> {
        self.in_class = true;
        let mut negated = false;
        let mut chars: Vec<u32> = Vec::new();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        let mut categories: Vec<Category> = Vec::new();

        // CPython warns about an initial nested '[' but parses it literally.
        if self.raw_peek() == Some(u32::from('[')) && self.nested_set_warning_pos.is_none() {
            self.nested_set_warning_pos = Some(self.pos as i64);
        }
        if self.raw_peek() == Some(u32::from('^')) {
            self.raw_next()?;
            negated = true;
        }
        let mut first = true;
        loop {
            if self.raw_peek().is_none() {
                self.in_class = false;
                return Err("unterminated character class".to_string());
            }
            if self.raw_peek() == Some(u32::from(b']')) && !first {
                self.raw_next()?;
                break;
            }
            first = false;
            // Decode each endpoint before composing a range. Escaped literals
            // participate exactly like ordinary literals; the upper endpoint
            // must not recursively consume another range (e.g. [a-b-c]).
            let item = self.class_atom()?;
            if self.raw_peek() == Some(u32::from('-'))
                && self.chars.get(self.pos + 1) != Some(&u32::from(b']'))
            {
                self.raw_next()?;
                let end = self.class_atom()?;
                match (item, end) {
                    (ClassItem::Char(start), ClassItem::Char(end)) if start <= end => {
                        ranges.push((start, end));
                    }
                    _ => return Err("bad character range".to_string()),
                }
            } else {
                match item {
                    ClassItem::Category(cat) => categories.push(cat),
                    ClassItem::Char(c) => chars.push(c),
                }
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

    pub(super) fn class_atom(&mut self) -> Result<ClassItem, String> {
        let ch = self.raw_next()?;
        match char::from_u32(ch) {
            Some('\\') => {
                let esc = self.raw_next()?;
                if self.is_bytes && matches!(char::from_u32(esc), Some('u' | 'U' | 'N')) {
                    return Err(format!(
                        "bad escape \\{} at position {}",
                        char::from_u32(esc).unwrap(),
                        self.pos - 2
                    ));
                }
                match char::from_u32(esc) {
                    Some('d' | 'D' | 's' | 'S' | 'w' | 'W') => Ok(ClassItem::Category(
                        Category::from_escape(char::from_u32(esc).unwrap()),
                    )),
                    Some('n') => Ok(ClassItem::Char(u32::from('\n'))),
                    Some('t') => Ok(ClassItem::Char(u32::from('\t'))),
                    Some('r') => Ok(ClassItem::Char(u32::from('\r'))),
                    Some('f') => Ok(ClassItem::Char(u32::from('\x0C'))),
                    Some('v') => Ok(ClassItem::Char(u32::from('\x0B'))),
                    Some(c) if c.is_ascii_digit() => {
                        // Octal escape inside character classes.
                        let mut oct = String::from(c);
                        while oct.len() < 3 {
                            match self.raw_peek().and_then(char::from_u32) {
                                Some(d) if ('0'..='7').contains(&d) => {
                                    self.raw_next()?;
                                    oct.push(d);
                                }
                                _ => break,
                            }
                        }
                        let code = u32::from_str_radix(&oct, 8)
                            .map_err(|_| "invalid octal escape".to_string())?;
                        Ok(ClassItem::Char(code))
                    }
                    _ => Ok(ClassItem::Char(esc)),
                }
            }
            _ => Ok(ClassItem::Char(ch)),
        }
    }
}

// ---------------------------------------------------------------------------
// ClassItem helper enum (only used inside parser)
// ---------------------------------------------------------------------------

pub(super) enum ClassItem {
    Char(u32),
    Category(Category),
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
        ReNode::Literal(s) => Some(s.len() as u64),
        ReNode::Any => Some(1),
        ReNode::Anchor(_) => Some(0),
        ReNode::CharClass { .. } => Some(1),
        ReNode::Backref(idx) => group_widths?.get(idx).copied().flatten(),
        ReNode::Group { node, .. } | ReNode::Atomic(node) => fixed_width(node, group_widths),
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

pub(super) fn parse_pattern_typed(
    pattern: Vec<u32>,
    flags: i64,
    is_bytes: bool,
) -> Result<ParsedPattern, String> {
    let mut parser = ReParser::new(pattern, flags);
    parser.is_bytes = is_bytes;
    let root = parser.parse()?;
    let group_count = parser.group_count;
    let group_names = parser.group_names;
    let inline_flags = parser.inline_flags;
    let warn_pos = parser.nested_set_warning_pos;
    Ok(ParsedPattern {
        root,
        group_count,
        group_names,
        flags: flags | inline_flags,
        is_bytes,
        warn_pos,
    })
}
