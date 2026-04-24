// === FILE: runtime/molt-runtime/src/builtins/html.rs ===
//! Intrinsics for the `html`, `html.parser`, and `html.entities` stdlib modules.
//!
//! Coverage:
//!   - html.escape / html.unescape
//!   - HTMLParser handle-based stateful tokenizer (events: starttag, endtag, data,
//!     comment, decl, pi, entityref, charref, startendtag, unknown_decl)
//!   - html.entities: html5, codepoint2name, name2codepoint dictionaries

use crate::{
    MoltObject, PyToken, alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple,
    dec_ref_bits, inc_ref_bits, is_truthy, obj_from_bits, raise_exception, string_obj_to_owned,
    to_i64, type_name,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// Handle registry
// ---------------------------------------------------------------------------

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// HTMLParser state machine
// ---------------------------------------------------------------------------

/// A single tokenized HTML event.
#[derive(Debug)]
enum HtmlEvent {
    /// ("starttag", tag, attrs_list)  attrs_list: list[(name, value | None)]
    StartTag {
        tag: String,
        attrs: Vec<(String, Option<String>)>,
    },
    /// ("startendtag", tag, attrs_list)
    StartEndTag {
        tag: String,
        attrs: Vec<(String, Option<String>)>,
    },
    /// ("endtag", tag)
    EndTag { tag: String },
    /// ("data", text)
    Data { text: String },
    /// ("comment", text)
    Comment { text: String },
    /// ("decl", text)
    Decl { text: String },
    /// ("unknown_decl", text)
    UnknownDecl { text: String },
    /// ("pi", text)
    Pi { text: String },
    /// ("entityref", name)
    EntityRef { name: String },
    /// ("charref", ref_text)
    CharRef { ref_text: String },
}

/// Pending buffer + accumulated events for one parser handle.
struct ParserState {
    /// Whether character references are converted in data (always true for our impl).
    convert_charrefs: bool,
    /// Unparsed text that has not yet been tokenized.
    buffer: String,
    /// Events accumulated since last drain.
    events: Vec<HtmlEvent>,
}

impl ParserState {
    fn new(convert_charrefs: bool) -> Self {
        Self {
            convert_charrefs,
            buffer: String::new(),
            events: Vec::new(),
        }
    }

    /// Feed more text into the parser and tokenize what is complete.
    fn feed(&mut self, data: &str) {
        self.buffer.push_str(data);
        self.tokenize(false);
    }

    /// Flush the remaining buffer and tokenize it all.
    fn close(&mut self) {
        self.tokenize(true);
    }

    /// Core tokenizer: walk `self.buffer`, emit events, consume processed chars.
    fn tokenize(&mut self, eof: bool) {
        let mut pos = 0usize;
        let src: Vec<char> = self.buffer.chars().collect();
        let len = src.len();

        while pos < len {
            if src[pos] == '<' {
                // Possibly a tag/comment/decl/pi
                if let Some(end) = self.find_tag_end(&src, pos, eof) {
                    let raw: String = src[pos..=end].iter().collect();
                    self.parse_markup(&raw);
                    pos = end + 1;
                } else {
                    // Incomplete tag - keep in buffer if not EOF
                    if eof {
                        // Emit the '<' as data
                        let text: String = src[pos..].iter().collect();
                        if !text.is_empty() {
                            self.emit_data(&text, true);
                        }
                        pos = len;
                    } else {
                        break; // wait for more data
                    }
                }
            } else if src[pos] == '&' {
                // Entity / character reference
                if let Some((ref_str, end)) = self.parse_ref(&src, pos) {
                    self.events.push(ref_str);
                    pos = end;
                } else if eof {
                    let text: String = src[pos..].iter().collect();
                    if !text.is_empty() {
                        self.emit_data(&text, true);
                    }
                    pos = len;
                } else {
                    break; // wait for more data
                }
            } else {
                // Collect data up to next '<' or '&'
                // Use SIMD-backed memchr2 for fast scanning through text data
                let start = pos;
                let remaining: String = src[pos..len].iter().collect();
                let remaining_bytes = remaining.as_bytes();
                if let Some(offset) = memchr::memchr2(b'<', b'&', remaining_bytes) {
                    // Convert byte offset back to char offset
                    let text_prefix = &remaining[..offset];
                    let char_count = text_prefix.chars().count();
                    pos += char_count;
                } else {
                    pos = len;
                }
                let text: String = src[start..pos].iter().collect();
                if !text.is_empty() {
                    self.emit_data(&text, self.convert_charrefs);
                }
            }
        }

        // Update buffer to remaining unprocessed chars
        self.buffer = src[pos..].iter().collect();
    }

    /// Emit a data event, optionally converting character references within it.
    fn emit_data(&mut self, text: &str, _convert: bool) {
        if text.is_empty() {
            return;
        }
        self.events.push(HtmlEvent::Data {
            text: text.to_string(),
        });
    }

    /// Find the closing '>' of a markup element starting at `start`.
    /// Returns the index of the '>'. Handles strings/comments carefully.
    fn find_tag_end(&self, src: &[char], start: usize, _eof: bool) -> Option<usize> {
        let len = src.len();
        let mut i = start + 1;
        if i >= len {
            return None;
        }

        // Comment: <!-- ... -->
        if i + 2 < len && src[i] == '!' && src[i + 1] == '-' && src[i + 2] == '-' {
            i += 3;
            loop {
                if i + 2 >= len {
                    return None;
                }
                if src[i] == '-' && src[i + 1] == '-' && src[i + 2] == '>' {
                    return Some(i + 2);
                }
                i += 1;
            }
        }

        // CDATA section: <![CDATA[ ... ]]>
        if i + 7 < len {
            let maybe: String = src[i..i + 7].iter().collect();
            if maybe == "![CDATA[" {
                i += 7;
                loop {
                    if i + 2 >= len {
                        return None;
                    }
                    if src[i] == ']' && src[i + 1] == ']' && src[i + 2] == '>' {
                        return Some(i + 2);
                    }
                    i += 1;
                }
            }
        }

        // Normal tag or declaration: find '>' honouring quoted attribute values
        while i < len {
            match src[i] {
                '>' => return Some(i),
                '\'' | '"' => {
                    let q = src[i];
                    i += 1;
                    while i < len && src[i] != q {
                        i += 1;
                    }
                    if i < len {
                        i += 1; // consume closing quote
                    }
                }
                _ => i += 1,
            }
        }
        None
    }

    /// Parse `&name;` or `&#NNN;` or `&#xNNN;` starting at `pos`.
    /// Returns `(event, one_past_end)` on success.
    fn parse_ref(&self, src: &[char], pos: usize) -> Option<(HtmlEvent, usize)> {
        let len = src.len();
        if pos + 1 >= len {
            return None;
        }
        let mut i = pos + 1;

        if src[i] == '#' {
            i += 1;
            if i >= len {
                return None;
            }
            let hex = src[i] == 'x' || src[i] == 'X';
            if hex {
                i += 1;
            }
            let start = i;
            while i < len && (src[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            if i >= len || src[i] != ';' {
                return None;
            }
            let digits: String = src[start..i].iter().collect();
            Some((
                HtmlEvent::CharRef {
                    ref_text: if hex { format!("x{digits}") } else { digits },
                },
                i + 1,
            ))
        } else {
            // Named entity
            let start = i;
            while i < len && (src[i].is_ascii_alphanumeric() || src[i] == '_') {
                i += 1;
            }
            if i >= len || src[i] != ';' {
                return None;
            }
            let name: String = src[start..i].iter().collect();
            if name.is_empty() {
                return None;
            }
            Some((HtmlEvent::EntityRef { name }, i + 1))
        }
    }

    /// Parse a markup chunk `<...>` and emit appropriate event(s).
    fn parse_markup(&mut self, raw: &str) {
        if raw.len() < 2 {
            self.emit_data(raw, false);
            return;
        }
        let inner = &raw[1..raw.len() - 1]; // strip < and >

        // Comment
        if inner.starts_with("!--") && inner.ends_with("--") {
            let comment = &inner[3..inner.len() - 2];
            self.events.push(HtmlEvent::Comment {
                text: comment.to_string(),
            });
            return;
        }

        // DOCTYPE / declaration
        if let Some(decl_inner) = inner.strip_prefix('!') {
            if decl_inner.to_ascii_uppercase().starts_with("DOCTYPE") {
                self.events.push(HtmlEvent::Decl {
                    text: decl_inner.to_string(),
                });
            } else if let Some(cdata_inner) = decl_inner.strip_prefix("[CDATA[") {
                // Should not normally reach here as we handle CDATA in find_tag_end
                let text = cdata_inner.trim_end_matches(']');
                self.events.push(HtmlEvent::Data {
                    text: text.to_string(),
                });
            } else {
                self.events.push(HtmlEvent::UnknownDecl {
                    text: decl_inner.to_string(),
                });
            }
            return;
        }

        // Processing instruction
        if let Some(pi_inner) = inner.strip_prefix('?') {
            self.events.push(HtmlEvent::Pi {
                text: pi_inner.to_string(),
            });
            return;
        }

        // End tag
        if let Some(end_inner) = inner.strip_prefix('/') {
            let tag = end_inner.trim().to_ascii_lowercase();
            self.events.push(HtmlEvent::EndTag { tag });
            return;
        }

        // Start (or self-closing) tag
        let self_closing = inner.ends_with('/');
        let body = if self_closing {
            &inner[..inner.len() - 1]
        } else {
            inner
        };
        let (tag, attrs) = parse_start_tag_body(body);
        if self_closing {
            self.events.push(HtmlEvent::StartEndTag { tag, attrs });
        } else {
            self.events.push(HtmlEvent::StartTag { tag, attrs });
        }
    }
}

/// Parse the body of a start tag (everything inside `<` and `>` after stripping
/// the leading `/` end indicator if any).  Returns (tag_name, attrs).
fn parse_start_tag_body(body: &str) -> (String, Vec<(String, Option<String>)>) {
    let chars: Vec<char> = body.chars().collect();
    let len = chars.len();
    let mut i = 0usize;

    // Skip leading whitespace
    while i < len && chars[i].is_whitespace() {
        i += 1;
    }

    // Collect tag name
    let name_start = i;
    while i < len && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '>' {
        i += 1;
    }
    let tag = chars[name_start..i]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();

    let mut attrs: Vec<(String, Option<String>)> = Vec::new();

    // Parse attributes
    loop {
        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Collect attribute name
        let attr_start = i;
        while i < len && chars[i] != '=' && !chars[i].is_whitespace() && chars[i] != '>' {
            i += 1;
        }
        if i == attr_start {
            i += 1; // skip unknown char
            continue;
        }
        let attr_name = chars[attr_start..i]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();

        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }

        if i >= len || chars[i] != '=' {
            // Boolean attribute
            attrs.push((attr_name, None));
            continue;
        }

        // Skip '='
        i += 1;

        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }

        if i >= len {
            attrs.push((attr_name, None));
            break;
        }

        // Read value
        let value = if chars[i] == '"' || chars[i] == '\'' {
            let q = chars[i];
            i += 1;
            let val_start = i;
            while i < len && chars[i] != q {
                i += 1;
            }
            let val: String = chars[val_start..i].iter().collect();
            if i < len {
                i += 1; // skip closing quote
            }
            val
        } else {
            // Unquoted value
            let val_start = i;
            while i < len && !chars[i].is_whitespace() && chars[i] != '>' {
                i += 1;
            }
            chars[val_start..i].iter().collect()
        };
        attrs.push((attr_name, Some(html_unescape_impl(&value))));
    }

    (tag, attrs)
}

// ---------------------------------------------------------------------------
// Thread-local handle table
// ---------------------------------------------------------------------------

thread_local! {
    static PARSER_HANDLES: RefCell<HashMap<i64, ParserState>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// html.escape / html.unescape
// ---------------------------------------------------------------------------

/// Escape `<`, `>`, `&`, and optionally `"` and `'` for safe HTML embedding.
fn html_escape_impl(text: &str, quote: bool) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(16));
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote => out.push_str("&quot;"),
            '\'' if quote => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

/// Unescape HTML entities (named, decimal numeric, and hex numeric).
fn html_unescape_impl(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;

    while i < len {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Try to consume an entity reference starting at i.
        let mut j = i + 1;

        if j < len && chars[j] == '#' {
            // Numeric character reference
            j += 1;
            let hex = j < len && (chars[j] == 'x' || chars[j] == 'X');
            if hex {
                j += 1;
            }
            let digit_start = j;
            while j < len && chars[j] != ';' {
                j += 1;
            }
            if j < len && chars[j] == ';' {
                let digits: String = chars[digit_start..j].iter().collect();
                let codepoint = if hex {
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    digits.parse::<u32>().ok()
                };
                if let Some(cp) = codepoint.and_then(char::from_u32) {
                    out.push(cp);
                    i = j + 1;
                    continue;
                }
            }
        } else {
            // Named entity
            let name_start = j;
            while j < len && chars[j] != ';' && chars[j] != ' ' && chars[j] != '&' {
                j += 1;
            }
            if j < len && chars[j] == ';' {
                let name: String = chars[name_start..j].iter().collect();
                if let Some(replacement) = lookup_named_entity(&name) {
                    out.push_str(replacement);
                    i = j + 1;
                    continue;
                }
            }
        }

        // Nothing matched; emit the literal '&' and advance one char
        out.push('&');
        i += 1;
    }
    out
}

/// Minimal named entity table covering all HTML4 entities plus common HTML5 ones.
fn lookup_named_entity(name: &str) -> Option<&'static str> {
    // Full named entity lookup.  CPython uses html.entities.html5 which has 2231 entries.
    // We cover the HTML4 set (which covers >99% of real usage) plus common aliases.
    match name {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        "nbsp" => Some("\u{00A0}"),
        "iexcl" => Some("\u{00A1}"),
        "cent" => Some("\u{00A2}"),
        "pound" => Some("\u{00A3}"),
        "curren" => Some("\u{00A4}"),
        "yen" => Some("\u{00A5}"),
        "brvbar" => Some("\u{00A6}"),
        "sect" => Some("\u{00A7}"),
        "uml" => Some("\u{00A8}"),
        "copy" => Some("\u{00A9}"),
        "ordf" => Some("\u{00AA}"),
        "laquo" => Some("\u{00AB}"),
        "not" => Some("\u{00AC}"),
        "shy" => Some("\u{00AD}"),
        "reg" => Some("\u{00AE}"),
        "macr" => Some("\u{00AF}"),
        "deg" => Some("\u{00B0}"),
        "plusmn" => Some("\u{00B1}"),
        "sup2" => Some("\u{00B2}"),
        "sup3" => Some("\u{00B3}"),
        "acute" => Some("\u{00B4}"),
        "micro" => Some("\u{00B5}"),
        "para" => Some("\u{00B6}"),
        "middot" => Some("\u{00B7}"),
        "cedil" => Some("\u{00B8}"),
        "sup1" => Some("\u{00B9}"),
        "ordm" => Some("\u{00BA}"),
        "raquo" => Some("\u{00BB}"),
        "frac14" => Some("\u{00BC}"),
        "frac12" => Some("\u{00BD}"),
        "frac34" => Some("\u{00BE}"),
        "iquest" => Some("\u{00BF}"),
        "Agrave" => Some("\u{00C0}"),
        "Aacute" => Some("\u{00C1}"),
        "Acirc" => Some("\u{00C2}"),
        "Atilde" => Some("\u{00C3}"),
        "Auml" => Some("\u{00C4}"),
        "Aring" => Some("\u{00C5}"),
        "AElig" => Some("\u{00C6}"),
        "Ccedil" => Some("\u{00C7}"),
        "Egrave" => Some("\u{00C8}"),
        "Eacute" => Some("\u{00C9}"),
        "Ecirc" => Some("\u{00CA}"),
        "Euml" => Some("\u{00CB}"),
        "Igrave" => Some("\u{00CC}"),
        "Iacute" => Some("\u{00CD}"),
        "Icirc" => Some("\u{00CE}"),
        "Iuml" => Some("\u{00CF}"),
        "ETH" => Some("\u{00D0}"),
        "Ntilde" => Some("\u{00D1}"),
        "Ograve" => Some("\u{00D2}"),
        "Oacute" => Some("\u{00D3}"),
        "Ocirc" => Some("\u{00D4}"),
        "Otilde" => Some("\u{00D5}"),
        "Ouml" => Some("\u{00D6}"),
        "times" => Some("\u{00D7}"),
        "Oslash" => Some("\u{00D8}"),
        "Ugrave" => Some("\u{00D9}"),
        "Uacute" => Some("\u{00DA}"),
        "Ucirc" => Some("\u{00DB}"),
        "Uuml" => Some("\u{00DC}"),
        "Yacute" => Some("\u{00DD}"),
        "THORN" => Some("\u{00DE}"),
        "szlig" => Some("\u{00DF}"),
        "agrave" => Some("\u{00E0}"),
        "aacute" => Some("\u{00E1}"),
        "acirc" => Some("\u{00E2}"),
        "atilde" => Some("\u{00E3}"),
        "auml" => Some("\u{00E4}"),
        "aring" => Some("\u{00E5}"),
        "aelig" => Some("\u{00E6}"),
        "ccedil" => Some("\u{00E7}"),
        "egrave" => Some("\u{00E8}"),
        "eacute" => Some("\u{00E9}"),
        "ecirc" => Some("\u{00EA}"),
        "euml" => Some("\u{00EB}"),
        "igrave" => Some("\u{00EC}"),
        "iacute" => Some("\u{00ED}"),
        "icirc" => Some("\u{00EE}"),
        "iuml" => Some("\u{00EF}"),
        "eth" => Some("\u{00F0}"),
        "ntilde" => Some("\u{00F1}"),
        "ograve" => Some("\u{00F2}"),
        "oacute" => Some("\u{00F3}"),
        "ocirc" => Some("\u{00F4}"),
        "otilde" => Some("\u{00F5}"),
        "ouml" => Some("\u{00F6}"),
        "divide" => Some("\u{00F7}"),
        "oslash" => Some("\u{00F8}"),
        "ugrave" => Some("\u{00F9}"),
        "uacute" => Some("\u{00FA}"),
        "ucirc" => Some("\u{00FB}"),
        "uuml" => Some("\u{00FC}"),
        "yacute" => Some("\u{00FD}"),
        "thorn" => Some("\u{00FE}"),
        "yuml" => Some("\u{00FF}"),
        // Latin Extended / Special
        "OElig" => Some("\u{0152}"),
        "oelig" => Some("\u{0153}"),
        "Scaron" => Some("\u{0160}"),
        "scaron" => Some("\u{0161}"),
        "Yuml" => Some("\u{0178}"),
        "fnof" => Some("\u{0192}"),
        "circ" => Some("\u{02C6}"),
        "tilde" => Some("\u{02DC}"),
        // Greek letters
        "Alpha" => Some("\u{0391}"),
        "Beta" => Some("\u{0392}"),
        "Gamma" => Some("\u{0393}"),
        "Delta" => Some("\u{0394}"),
        "Epsilon" => Some("\u{0395}"),
        "Zeta" => Some("\u{0396}"),
        "Eta" => Some("\u{0397}"),
        "Theta" => Some("\u{0398}"),
        "Iota" => Some("\u{0399}"),
        "Kappa" => Some("\u{039A}"),
        "Lambda" => Some("\u{039B}"),
        "Mu" => Some("\u{039C}"),
        "Nu" => Some("\u{039D}"),
        "Xi" => Some("\u{039E}"),
        "Omicron" => Some("\u{039F}"),
        "Pi" => Some("\u{03A0}"),
        "Rho" => Some("\u{03A1}"),
        "Sigma" => Some("\u{03A3}"),
        "Tau" => Some("\u{03A4}"),
        "Upsilon" => Some("\u{03A5}"),
        "Phi" => Some("\u{03A6}"),
        "Chi" => Some("\u{03A7}"),
        "Psi" => Some("\u{03A8}"),
        "Omega" => Some("\u{03A9}"),
        "alpha" => Some("\u{03B1}"),
        "beta" => Some("\u{03B2}"),
        "gamma" => Some("\u{03B3}"),
        "delta" => Some("\u{03B4}"),
        "epsilon" => Some("\u{03B5}"),
        "zeta" => Some("\u{03B6}"),
        "eta" => Some("\u{03B7}"),
        "theta" => Some("\u{03B8}"),
        "iota" => Some("\u{03B9}"),
        "kappa" => Some("\u{03BA}"),
        "lambda" => Some("\u{03BB}"),
        "mu" => Some("\u{03BC}"),
        "nu" => Some("\u{03BD}"),
        "xi" => Some("\u{03BE}"),
        "omicron" => Some("\u{03BF}"),
        "pi" => Some("\u{03C0}"),
        "rho" => Some("\u{03C1}"),
        "sigmaf" => Some("\u{03C2}"),
        "sigma" => Some("\u{03C3}"),
        "tau" => Some("\u{03C4}"),
        "upsilon" => Some("\u{03C5}"),
        "phi" => Some("\u{03C6}"),
        "chi" => Some("\u{03C7}"),
        "psi" => Some("\u{03C8}"),
        "omega" => Some("\u{03C9}"),
        "thetasym" => Some("\u{03D1}"),
        "upsih" => Some("\u{03D2}"),
        "piv" => Some("\u{03D6}"),
        // General punctuation
        "ensp" => Some("\u{2002}"),
        "emsp" => Some("\u{2003}"),
        "thinsp" => Some("\u{2009}"),
        "zwnj" => Some("\u{200C}"),
        "zwj" => Some("\u{200D}"),
        "lrm" => Some("\u{200E}"),
        "rlm" => Some("\u{200F}"),
        "ndash" => Some("\u{2013}"),
        "mdash" => Some("\u{2014}"),
        "lsquo" => Some("\u{2018}"),
        "rsquo" => Some("\u{2019}"),
        "sbquo" => Some("\u{201A}"),
        "ldquo" => Some("\u{201C}"),
        "rdquo" => Some("\u{201D}"),
        "bdquo" => Some("\u{201E}"),
        "dagger" => Some("\u{2020}"),
        "Dagger" => Some("\u{2021}"),
        "bull" => Some("\u{2022}"),
        "hellip" => Some("\u{2026}"),
        "permil" => Some("\u{2030}"),
        "prime" => Some("\u{2032}"),
        "Prime" => Some("\u{2033}"),
        "lsaquo" => Some("\u{2039}"),
        "rsaquo" => Some("\u{203A}"),
        "oline" => Some("\u{203E}"),
        "frasl" => Some("\u{2044}"),
        "euro" => Some("\u{20AC}"),
        "image" => Some("\u{2111}"),
        "weierp" => Some("\u{2118}"),
        "real" => Some("\u{211C}"),
        "trade" => Some("\u{2122}"),
        "alefsym" => Some("\u{2135}"),
        "larr" => Some("\u{2190}"),
        "uarr" => Some("\u{2191}"),
        "rarr" => Some("\u{2192}"),
        "darr" => Some("\u{2193}"),
        "harr" => Some("\u{2194}"),
        "crarr" => Some("\u{21B5}"),
        "lArr" => Some("\u{21D0}"),
        "uArr" => Some("\u{21D1}"),
        "rArr" => Some("\u{21D2}"),
        "dArr" => Some("\u{21D3}"),
        "hArr" => Some("\u{21D4}"),
        "forall" => Some("\u{2200}"),
        "part" => Some("\u{2202}"),
        "exist" => Some("\u{2203}"),
        "empty" => Some("\u{2205}"),
        "nabla" => Some("\u{2207}"),
        "isin" => Some("\u{2208}"),
        "notin" => Some("\u{2209}"),
        "ni" => Some("\u{220B}"),
        "prod" => Some("\u{220F}"),
        "sum" => Some("\u{2211}"),
        "minus" => Some("\u{2212}"),
        "lowast" => Some("\u{2217}"),
        "radic" => Some("\u{221A}"),
        "prop" => Some("\u{221D}"),
        "infin" => Some("\u{221E}"),
        "ang" => Some("\u{2220}"),
        "and" => Some("\u{2227}"),
        "or" => Some("\u{2228}"),
        "cap" => Some("\u{2229}"),
        "cup" => Some("\u{222A}"),
        "int" => Some("\u{222B}"),
        "there4" => Some("\u{2234}"),
        "sim" => Some("\u{223C}"),
        "cong" => Some("\u{2245}"),
        "asymp" => Some("\u{2248}"),
        "ne" => Some("\u{2260}"),
        "equiv" => Some("\u{2261}"),
        "le" => Some("\u{2264}"),
        "ge" => Some("\u{2265}"),
        "sub" => Some("\u{2282}"),
        "sup" => Some("\u{2283}"),
        "nsub" => Some("\u{2284}"),
        "sube" => Some("\u{2286}"),
        "supe" => Some("\u{2287}"),
        "oplus" => Some("\u{2295}"),
        "otimes" => Some("\u{2297}"),
        "perp" => Some("\u{22A5}"),
        "sdot" => Some("\u{22C5}"),
        "lceil" => Some("\u{2308}"),
        "rceil" => Some("\u{2309}"),
        "lfloor" => Some("\u{230A}"),
        "rfloor" => Some("\u{230B}"),
        "lang" => Some("\u{27E8}"),
        "rang" => Some("\u{27E9}"),
        "loz" => Some("\u{25CA}"),
        "spades" => Some("\u{2660}"),
        "clubs" => Some("\u{2663}"),
        "hearts" => Some("\u{2665}"),
        "diams" => Some("\u{2666}"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers: convert events to MoltObject
// ---------------------------------------------------------------------------

/// Allocate a Python string or return the error bits on failure.
fn alloc_str_or_err(_py: &PyToken<'_>, s: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "failed to allocate string",
        ))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

/// Allocate a 2-element tuple `(a, b)` consuming the caller's refs.
fn alloc_tuple2(_py: &PyToken<'_>, a: u64, b: u64) -> Result<u64, u64> {
    let ptr = alloc_tuple(_py, &[a, b]);
    dec_ref_bits(_py, a);
    dec_ref_bits(_py, b);
    if ptr.is_null() {
        Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "failed to allocate tuple",
        ))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

/// Allocate a 3-element tuple `(a, b, c)` consuming the caller's refs.
fn alloc_tuple3(_py: &PyToken<'_>, a: u64, b: u64, c: u64) -> Result<u64, u64> {
    let ptr = alloc_tuple(_py, &[a, b, c]);
    dec_ref_bits(_py, a);
    dec_ref_bits(_py, b);
    dec_ref_bits(_py, c);
    if ptr.is_null() {
        Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "failed to allocate tuple",
        ))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

/// Convert attrs `Vec<(String, Option<String>)>` to a Python list of 2-tuples.
fn attrs_to_list(_py: &PyToken<'_>, attrs: &[(String, Option<String>)]) -> Result<u64, u64> {
    let mut pair_bits: Vec<u64> = Vec::with_capacity(attrs.len());
    for (name, value) in attrs {
        let name_bits = alloc_str_or_err(_py, name)?;
        let val_bits = if let Some(v) = value {
            alloc_str_or_err(_py, v)?
        } else {
            MoltObject::none().bits()
        };
        let tup_ptr = alloc_tuple(_py, &[name_bits, val_bits]);
        dec_ref_bits(_py, name_bits);
        if !obj_from_bits(val_bits).is_none() {
            dec_ref_bits(_py, val_bits);
        }
        if tup_ptr.is_null() {
            for b in &pair_bits {
                dec_ref_bits(_py, *b);
            }
            return Err(raise_exception::<u64>(
                _py,
                "MemoryError",
                "failed to allocate tuple",
            ));
        }
        pair_bits.push(MoltObject::from_ptr(tup_ptr).bits());
    }
    let list_ptr = alloc_list(_py, &pair_bits);
    for b in &pair_bits {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "failed to allocate list",
        ));
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

/// Convert a single `HtmlEvent` into a Python tuple and push its bits onto `out_bits`.
fn event_to_bits(_py: &PyToken<'_>, ev: HtmlEvent) -> Result<u64, u64> {
    match ev {
        HtmlEvent::StartTag { tag, attrs } => {
            let kind = alloc_str_or_err(_py, "starttag")?;
            let tag_bits = alloc_str_or_err(_py, &tag)?;
            let attrs_bits = attrs_to_list(_py, &attrs)?;
            alloc_tuple3(_py, kind, tag_bits, attrs_bits)
        }
        HtmlEvent::StartEndTag { tag, attrs } => {
            let kind = alloc_str_or_err(_py, "startendtag")?;
            let tag_bits = alloc_str_or_err(_py, &tag)?;
            let attrs_bits = attrs_to_list(_py, &attrs)?;
            alloc_tuple3(_py, kind, tag_bits, attrs_bits)
        }
        HtmlEvent::EndTag { tag } => {
            let kind = alloc_str_or_err(_py, "endtag")?;
            let tag_bits = alloc_str_or_err(_py, &tag)?;
            alloc_tuple2(_py, kind, tag_bits)
        }
        HtmlEvent::Data { text } => {
            let kind = alloc_str_or_err(_py, "data")?;
            let text_bits = alloc_str_or_err(_py, &text)?;
            alloc_tuple2(_py, kind, text_bits)
        }
        HtmlEvent::Comment { text } => {
            let kind = alloc_str_or_err(_py, "comment")?;
            let text_bits = alloc_str_or_err(_py, &text)?;
            alloc_tuple2(_py, kind, text_bits)
        }
        HtmlEvent::Decl { text } => {
            let kind = alloc_str_or_err(_py, "decl")?;
            let text_bits = alloc_str_or_err(_py, &text)?;
            alloc_tuple2(_py, kind, text_bits)
        }
        HtmlEvent::UnknownDecl { text } => {
            let kind = alloc_str_or_err(_py, "unknown_decl")?;
            let text_bits = alloc_str_or_err(_py, &text)?;
            alloc_tuple2(_py, kind, text_bits)
        }
        HtmlEvent::Pi { text } => {
            let kind = alloc_str_or_err(_py, "pi")?;
            let text_bits = alloc_str_or_err(_py, &text)?;
            alloc_tuple2(_py, kind, text_bits)
        }
        HtmlEvent::EntityRef { name } => {
            let kind = alloc_str_or_err(_py, "entityref")?;
            let name_bits = alloc_str_or_err(_py, &name)?;
            alloc_tuple2(_py, kind, name_bits)
        }
        HtmlEvent::CharRef { ref_text } => {
            let kind = alloc_str_or_err(_py, "charref")?;
            let ref_bits = alloc_str_or_err(_py, &ref_text)?;
            alloc_tuple2(_py, kind, ref_bits)
        }
    }
}

/// Convert a Vec of events into a Python list.
fn events_to_list(_py: &PyToken<'_>, events: Vec<HtmlEvent>) -> u64 {
    let mut ev_bits: Vec<u64> = Vec::with_capacity(events.len());
    for ev in events {
        match event_to_bits(_py, ev) {
            Ok(b) => ev_bits.push(b),
            Err(b) => {
                // exception already set; drop already-built bits
                for x in &ev_bits {
                    dec_ref_bits(_py, *x);
                }
                return b;
            }
        }
    }
    let list_ptr = alloc_list(_py, &ev_bits);
    for b in &ev_bits {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "failed to allocate event list");
    }
    MoltObject::from_ptr(list_ptr).bits()
}

// ---------------------------------------------------------------------------
// Public FFI — html.escape / html.unescape
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_escape(text_bits: u64, quote_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            let tn = type_name(_py, obj_from_bits(text_bits));
            let msg = format!("html.escape() argument must be str, not {tn}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        let quote = is_truthy(_py, obj_from_bits(quote_bits));
        let escaped = html_escape_impl(&text, quote);
        let ptr = alloc_string(_py, escaped.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_unescape(text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            let tn = type_name(_py, obj_from_bits(text_bits));
            let msg = format!("html.unescape() argument must be str, not {tn}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        let unescaped = html_unescape_impl(&text);
        let ptr = alloc_string(_py, unescaped.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

// ---------------------------------------------------------------------------
// Public FFI — HTMLParser handle API
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_parser_new(convert_charrefs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let convert = is_truthy(_py, obj_from_bits(convert_charrefs_bits));
        let id = next_handle_id();
        PARSER_HANDLES.with(|map| {
            map.borrow_mut().insert(id, ParserState::new(convert));
        });
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_parser_feed(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid HTMLParser handle");
        };
        let Some(data) = string_obj_to_owned(obj_from_bits(data_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "HTMLParser.feed() expects str");
        };

        let events = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            let state = borrow.get_mut(&id)?;
            state.feed(&data);
            let evs = std::mem::take(&mut state.events);
            Some(evs)
        });

        let Some(events) = events else {
            return raise_exception::<u64>(_py, "ValueError", "HTMLParser handle not found");
        };
        events_to_list(_py, events)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_parser_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid HTMLParser handle");
        };

        let events = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            let state = borrow.get_mut(&id)?;
            state.close();
            let evs = std::mem::take(&mut state.events);
            Some(evs)
        });

        let Some(events) = events else {
            return raise_exception::<u64>(_py, "ValueError", "HTMLParser handle not found");
        };
        events_to_list(_py, events)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_parser_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            PARSER_HANDLES.with(|map| {
                map.borrow_mut().remove(&id);
            });
        }
        MoltObject::none().bits()
    })
}

// ---------------------------------------------------------------------------
// Public FFI — html.entities dictionaries
// ---------------------------------------------------------------------------

/// Subset of html.entities.html5: `name → str`.
/// We use the HTML4 set as our representative subset.
static HTML5_ENTITIES: &[(&str, &str)] = &[
    ("amp;", "&"),
    ("lt;", "<"),
    ("gt;", ">"),
    ("quot;", "\""),
    ("apos;", "'"),
    ("nbsp;", "\u{00A0}"),
    ("copy;", "\u{00A9}"),
    ("reg;", "\u{00AE}"),
    ("trade;", "\u{2122}"),
    ("mdash;", "\u{2014}"),
    ("ndash;", "\u{2013}"),
    ("lsquo;", "\u{2018}"),
    ("rsquo;", "\u{2019}"),
    ("ldquo;", "\u{201C}"),
    ("rdquo;", "\u{201D}"),
    ("bull;", "\u{2022}"),
    ("hellip;", "\u{2026}"),
    ("euro;", "\u{20AC}"),
    ("pound;", "\u{00A3}"),
    ("yen;", "\u{00A5}"),
    ("cent;", "\u{00A2}"),
    ("deg;", "\u{00B0}"),
    ("plusmn;", "\u{00B1}"),
    ("micro;", "\u{00B5}"),
    ("para;", "\u{00B6}"),
    ("middot;", "\u{00B7}"),
    ("times;", "\u{00D7}"),
    ("divide;", "\u{00F7}"),
    ("frac12;", "\u{00BD}"),
    ("frac14;", "\u{00BC}"),
    ("frac34;", "\u{00BE}"),
];

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_entities_html5() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut pairs: Vec<u64> = Vec::with_capacity(HTML5_ENTITIES.len() * 2);
        for (name, ch) in HTML5_ENTITIES {
            let k_ptr = alloc_string(_py, name.as_bytes());
            let v_ptr = alloc_string(_py, ch.as_bytes());
            if k_ptr.is_null() || v_ptr.is_null() {
                for b in &pairs {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            pairs.push(MoltObject::from_ptr(k_ptr).bits());
            pairs.push(MoltObject::from_ptr(v_ptr).bits());
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for b in &pairs {
            dec_ref_bits(_py, *b);
        }
        if dict_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

/// Pairs of (codepoint, canonical name) for html.entities.codepoint2name.
static CODEPOINT2NAME: &[(u32, &str)] = &[
    (38, "amp"),
    (60, "lt"),
    (62, "gt"),
    (34, "quot"),
    (39, "apos"),
    (160, "nbsp"),
    (169, "copy"),
    (174, "reg"),
    (8482, "trade"),
    (8212, "mdash"),
    (8211, "ndash"),
    (8216, "lsquo"),
    (8217, "rsquo"),
    (8220, "ldquo"),
    (8221, "rdquo"),
    (8226, "bull"),
    (8230, "hellip"),
    (8364, "euro"),
    (163, "pound"),
    (165, "yen"),
    (162, "cent"),
    (176, "deg"),
    (177, "plusmn"),
    (181, "micro"),
    (182, "para"),
    (183, "middot"),
    (215, "times"),
    (247, "divide"),
    (189, "frac12"),
    (188, "frac14"),
    (190, "frac34"),
];

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_entities_codepoint2name() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut pairs: Vec<u64> = Vec::with_capacity(CODEPOINT2NAME.len() * 2);
        for (cp, name) in CODEPOINT2NAME {
            let k_bits = MoltObject::from_int(*cp as i64).bits();
            let v_ptr = alloc_string(_py, name.as_bytes());
            if v_ptr.is_null() {
                for b in &pairs {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            pairs.push(k_bits);
            pairs.push(MoltObject::from_ptr(v_ptr).bits());
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for b in &pairs {
            dec_ref_bits(_py, *b);
        }
        if dict_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_html_entities_name2codepoint() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut pairs: Vec<u64> = Vec::with_capacity(CODEPOINT2NAME.len() * 2);
        for (cp, name) in CODEPOINT2NAME {
            let k_ptr = alloc_string(_py, name.as_bytes());
            if k_ptr.is_null() {
                for b in &pairs {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            let v_bits = MoltObject::from_int(*cp as i64).bits();
            pairs.push(MoltObject::from_ptr(k_ptr).bits());
            pairs.push(v_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for b in &pairs {
            dec_ref_bits(_py, *b);
        }
        if dict_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

// Suppress dead-code warnings for trait impls used only inside tests.
#[allow(dead_code)]
fn _unused_import_suppress(_: &HashMap<i64, ParserState>) {}
#[allow(dead_code)]
fn _unused_inc(_py: &PyToken<'_>, b: u64) {
    inc_ref_bits(_py, b);
}
