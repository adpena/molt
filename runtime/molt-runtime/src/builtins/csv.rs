use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ── Quoting mode constants (mirroring CPython _csv.c) ────────────────────────
const QUOTE_MINIMAL: i64 = 0;
const QUOTE_ALL: i64 = 1;
const QUOTE_NONNUMERIC: i64 = 2;
const QUOTE_NONE: i64 = 3;
const QUOTE_STRINGS: i64 = 4;
const QUOTE_NOTNULL: i64 = 5;

// ── Default field-size limit (131072, matching CPython) ───────────────────────
const DEFAULT_FIELD_SIZE_LIMIT: i64 = 131_072;

// ── Thread-local storage: field size limit and per-thread handle tables ───────
thread_local! {
    static FIELD_SIZE_LIMIT: RefCell<i64> = const { RefCell::new(DEFAULT_FIELD_SIZE_LIMIT) };
    static READER_HANDLES: RefCell<HashMap<i64, ReaderState>> = RefCell::new(HashMap::new());
    static WRITER_HANDLES: RefCell<HashMap<i64, WriterState>> = RefCell::new(HashMap::new());
    static DIALECT_REGISTRY: RefCell<DialectRegistry> = RefCell::new(default_dialect_registry());
}

// ── Monotonic handle-ID counter ───────────────────────────────────────────────
static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Dialect config stored inside each reader / writer handle.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Dialect {
    delimiter: char,
    quotechar: Option<char>,
    escapechar: Option<char>,
    doublequote: bool,
    skipinitialspace: bool,
    quoting: i64,
    strict: bool,
    lineterminator: String,
}

impl Default for Dialect {
    fn default() -> Self {
        Self {
            delimiter: ',',
            quotechar: Some('"'),
            escapechar: None,
            doublequote: true,
            skipinitialspace: false,
            quoting: QUOTE_MINIMAL,
            strict: false,
            lineterminator: "\r\n".to_string(),
        }
    }
}

struct DialectRegistry {
    by_name: HashMap<String, Dialect>,
    order: Vec<String>,
}

impl DialectRegistry {
    fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn insert(&mut self, name: String, dialect: Dialect) {
        if !self.by_name.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.by_name.insert(name, dialect);
    }

    fn remove(&mut self, name: &str) -> Option<Dialect> {
        let removed = self.by_name.remove(name);
        if removed.is_some() {
            self.order.retain(|entry| entry != name);
        }
        removed
    }

    fn get(&self, name: &str) -> Option<&Dialect> {
        self.by_name.get(name)
    }

    fn names(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.order.len());
        for name in &self.order {
            if self.by_name.contains_key(name) {
                out.push(name.clone());
            }
        }
        out
    }
}

fn default_dialect_registry() -> DialectRegistry {
    let mut registry = DialectRegistry::new();
    registry.insert("excel".to_string(), Dialect::default());
    registry.insert(
        "excel-tab".to_string(),
        Dialect {
            delimiter: '\t',
            ..Dialect::default()
        },
    );
    registry.insert(
        "unix".to_string(),
        Dialect {
            quoting: QUOTE_ALL,
            lineterminator: "\n".to_string(),
            ..Dialect::default()
        },
    );
    registry
}

fn validate_dialect(_py: &PyToken<'_>, dialect: &Dialect) -> Result<(), u64> {
    if dialect.quoting != QUOTE_MINIMAL
        && dialect.quoting != QUOTE_ALL
        && dialect.quoting != QUOTE_NONNUMERIC
        && dialect.quoting != QUOTE_NONE
        && dialect.quoting != QUOTE_STRINGS
        && dialect.quoting != QUOTE_NOTNULL
    {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "bad \"quoting\" value",
        ));
    }
    if dialect.quotechar.is_none() && dialect.quoting != QUOTE_NONE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "quotechar must be set if quoting enabled",
        ));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Handle state types
// ─────────────────────────────────────────────────────────────────────────────

struct ReaderState {
    dialect: Dialect,
}

struct WriterState {
    dialect: Dialect,
}

// ─────────────────────────────────────────────────────────────────────────────
// Argument-extraction helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract an optional single-char string from a MoltObject.
/// Python None → Ok(None).  1-char str → Ok(Some(char)).  Anything else → Err.
fn opt_char_from_bits(_py: &PyToken<'_>, bits: u64, param_name: &str) -> Result<Option<char>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(s) = string_obj_to_owned(obj) else {
        let msg = format!("{param_name} must be a 1-character string");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(Some(ch)),
        _ => {
            let msg = format!("{param_name} must be a 1-character string");
            Err(raise_exception::<u64>(_py, "ValueError", &msg))
        }
    }
}

/// Extract a mandatory single-char string.
fn char_from_bits(_py: &PyToken<'_>, bits: u64, param_name: &str) -> Result<char, u64> {
    match opt_char_from_bits(_py, bits, param_name)? {
        Some(ch) => Ok(ch),
        None => {
            let msg = format!("{param_name} must be a 1-character string");
            Err(raise_exception::<u64>(_py, "TypeError", &msg))
        }
    }
}

fn release_bits(_py: &PyToken<'_>, bits: &[u64]) {
    for bit in bits {
        dec_ref_bits(_py, *bit);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CSV parse error enum
// ─────────────────────────────────────────────────────────────────────────────

enum CsvError {
    FieldTooLarge,
    UnexpectedEnd,
    BadInput(String),
    NeedEscapechar,
}

struct ParsedField {
    text: String,
    was_quoted: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Core CSV line parser (RFC 4180 + CPython _csv.c extensions).
//
// `line` may include a trailing \r\n / \r / \n; they are stripped internally.
// Returns the ordered list of field strings for one record.
// ─────────────────────────────────────────────────────────────────────────────
fn csv_parse_line(
    line: &str,
    dialect: &Dialect,
    field_limit: i64,
) -> Result<Vec<ParsedField>, CsvError> {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();

    // Strip trailing record-ending newlines so they never appear in a field.
    let effective_len = {
        let mut end = len;
        if end > 0 && chars[end - 1] == '\n' {
            end -= 1;
        }
        if end > 0 && chars[end - 1] == '\r' {
            end -= 1;
        }
        end
    };

    let mut fields: Vec<ParsedField> = Vec::new();
    let mut field = String::new();
    let mut pos = 0usize;
    let mut in_quotes = false;
    // After we see a closing quotechar; only delimiter / EOL is valid next.
    let mut after_quote = false;
    // Tracks whether the current field started with a quotechar.
    let mut field_was_quoted = false;

    macro_rules! append_char {
        ($ch:expr) => {{
            field.push($ch);
            if field.len() as i64 > field_limit {
                return Err(CsvError::FieldTooLarge);
            }
        }};
    }

    macro_rules! commit_field {
        () => {{
            fields.push(ParsedField {
                text: std::mem::take(&mut field),
                was_quoted: field_was_quoted,
            });
            in_quotes = false;
            after_quote = false;
            field_was_quoted = false;
        }};
    }

    while pos < effective_len {
        let ch = chars[pos];

        // ── State: just closed a quoted section ──────────────────────────────
        if after_quote {
            if ch == dialect.delimiter {
                commit_field!();
                pos += 1;
                if dialect.skipinitialspace {
                    while pos < effective_len && chars[pos] == ' ' {
                        pos += 1;
                    }
                }
                continue;
            }
            if dialect.strict {
                return Err(CsvError::BadInput(format!(
                    "'{}' expected after '{}'",
                    dialect.delimiter,
                    dialect.quotechar.unwrap_or('"')
                )));
            }
            // Non-strict: absorb the character into the field.
            append_char!(ch);
            after_quote = false;
            pos += 1;
            continue;
        }

        // ── State: inside quoted field ────────────────────────────────────────
        if in_quotes {
            // escapechar takes priority over quotechar inside quotes.
            if let Some(esc) = dialect.escapechar.filter(|&e| ch == e) {
                pos += 1;
                if pos >= effective_len {
                    if dialect.strict {
                        return Err(CsvError::UnexpectedEnd);
                    }
                    append_char!(esc);
                } else {
                    append_char!(chars[pos]);
                    pos += 1;
                }
                continue;
            }
            if let Some(qc) = dialect.quotechar.filter(|&q| ch == q) {
                if dialect.doublequote && pos + 1 < effective_len && chars[pos + 1] == qc {
                    // Doubled quotechar → literal quotechar.
                    append_char!(qc);
                    pos += 2;
                } else {
                    // Closing quotechar.
                    in_quotes = false;
                    after_quote = true;
                    pos += 1;
                }
                continue;
            }
            append_char!(ch);
            pos += 1;
            continue;
        }

        // ── State: unquoted ───────────────────────────────────────────────────

        if ch == dialect.delimiter {
            commit_field!();
            pos += 1;
            if dialect.skipinitialspace {
                while pos < effective_len && chars[pos] == ' ' {
                    pos += 1;
                }
            }
            continue;
        }

        // Opening quotechar at start of an empty field.
        if dialect
            .quotechar
            .is_some_and(|qc| ch == qc && field.is_empty() && !field_was_quoted)
        {
            in_quotes = true;
            field_was_quoted = true;
            pos += 1;
            continue;
        }

        // escapechar outside quotes.
        if let Some(esc) = dialect.escapechar.filter(|&e| ch == e) {
            pos += 1;
            if pos >= effective_len {
                if dialect.strict {
                    return Err(CsvError::UnexpectedEnd);
                }
                append_char!(esc);
            } else {
                append_char!(chars[pos]);
                pos += 1;
            }
            continue;
        }

        // skipinitialspace: skip leading spaces on a fresh (empty) field.
        if dialect.skipinitialspace && field.is_empty() && ch == ' ' {
            pos += 1;
            continue;
        }

        append_char!(ch);
        pos += 1;
    }

    // End of line.
    if in_quotes && dialect.strict {
        return Err(CsvError::UnexpectedEnd);
    }

    // Commit the trailing field (always when we have seen any data).
    if after_quote || !field.is_empty() || field_was_quoted || !fields.is_empty() {
        fields.push(ParsedField {
            text: std::mem::take(&mut field),
            was_quoted: field_was_quoted,
        });
    }

    Ok(fields)
}

// ─────────────────────────────────────────────────────────────────────────────
// Build a MoltObject list from parsed fields.
// QUOTE_NONNUMERIC converts unquoted non-empty fields to float.
// ─────────────────────────────────────────────────────────────────────────────
fn fields_to_list(
    _py: &PyToken<'_>,
    fields: &[ParsedField],
    dialect: &Dialect,
) -> Result<u64, u64> {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(fields.len());
    for field in fields {
        if dialect.quoting == QUOTE_NONNUMERIC && !field.was_quoted && !field.text.is_empty() {
            let parsed = match field.text.parse::<f64>() {
                Ok(value) => value,
                Err(_) => {
                    for b in &bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    let msg = format!("could not convert string to float: '{}'", field.text);
                    return Err(raise_exception::<u64>(_py, "ValueError", &msg));
                }
            };
            bits_vec.push(MoltObject::from_float(parsed).bits());
            continue;
        }

        let str_ptr = alloc_string(_py, field.text.as_bytes());
        if str_ptr.is_null() {
            for b in &bits_vec {
                dec_ref_bits(_py, *b);
            }
            return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
        }
        bits_vec.push(MoltObject::from_ptr(str_ptr).bits());
    }
    let list_ptr = alloc_list(_py, &bits_vec);
    // The list increments refs; release our temporary refs.
    for b in &bits_vec {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

// ─────────────────────────────────────────────────────────────────────────────
// Split a multi-line CSV text into individual logical lines.
// \r\n counts as one newline; lone \r and lone \n are also terminators.
// ─────────────────────────────────────────────────────────────────────────────
fn split_csv_lines(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0usize;

    while i < len {
        let ch = chars[i];
        if ch == '\r' {
            current.push('\r');
            if i + 1 < len && chars[i + 1] == '\n' {
                current.push('\n');
                i += 2;
            } else {
                i += 1;
            }
            lines.push(std::mem::take(&mut current));
            continue;
        }
        if ch == '\n' {
            current.push('\n');
            i += 1;
            lines.push(std::mem::take(&mut current));
            continue;
        }
        current.push(ch);
        i += 1;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

// ─────────────────────────────────────────────────────────────────────────────
// CSV writer helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Python-compatible float repr: shortest round-trip, handles nan/inf.
fn repr_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    format!("{f:?}")
}

/// Convert a MoltObject to its CSV text representation.
fn render_field_text(_py: &PyToken<'_>, bits: u64) -> String {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return String::new();
    }
    // Boolean before integer: bool is an int subtype in Python.
    if let Some(b) = obj.as_bool() {
        return if b {
            "True".to_string()
        } else {
            "False".to_string()
        };
    }
    if let Some(s) = string_obj_to_owned(obj) {
        return s;
    }
    if let Some(i) = to_i64(obj) {
        return i.to_string();
    }
    if let Some(f) = to_f64(obj) {
        return repr_float(f);
    }
    // Delegate to runtime `str()` semantics for all other objects.
    format_obj_str(_py, obj)
}

/// Returns (is_none, is_number) for quoting-mode decisions.
/// `is_number` is true for int, float, and bool (bool is int subtype in CPython).
fn obj_kind(bits: u64) -> (bool, bool) {
    let obj = obj_from_bits(bits);
    let is_none = obj.is_none();
    let is_number = !is_none && to_f64(obj).is_some();
    (is_none, is_number)
}

/// Encode one field string into its CSV representation per the dialect.
fn encode_field(
    text: &str,
    dialect: &Dialect,
    is_none: bool,
    is_number: bool,
) -> Result<String, CsvError> {
    let needs_quoting = match dialect.quoting {
        QUOTE_ALL => true,
        QUOTE_NONNUMERIC => !is_number,
        QUOTE_STRINGS => !is_none && !is_number,
        QUOTE_NOTNULL => !is_none,
        QUOTE_NONE => false,
        // QUOTE_MINIMAL (default)
        _ => {
            text.contains(dialect.delimiter)
                || text.contains('\n')
                || text.contains('\r')
                || dialect.quotechar.is_some_and(|q| text.contains(q))
                || (dialect.skipinitialspace && text.starts_with(' '))
        }
    };

    if dialect.quoting == QUOTE_NONE {
        let special_chars_present = text.contains(dialect.delimiter)
            || text.contains('\n')
            || text.contains('\r')
            || dialect.quotechar.is_some_and(|q| text.contains(q));
        let esc = match dialect.escapechar {
            Some(e) => e,
            None => {
                if special_chars_present {
                    return Err(CsvError::NeedEscapechar);
                }
                return Ok(text.to_string());
            }
        };
        let mut out = String::with_capacity(text.len() + 4);
        for ch in text.chars() {
            let is_special = ch == dialect.delimiter
                || ch == '\n'
                || ch == '\r'
                || ch == esc
                || dialect.quotechar.is_some_and(|q| ch == q);
            if is_special {
                out.push(esc);
            }
            out.push(ch);
        }
        return Ok(out);
    }

    if !needs_quoting {
        return Ok(text.to_string());
    }

    let qc = match dialect.quotechar {
        Some(q) => q,
        None => {
            return Err(CsvError::BadInput(
                "quotechar must be set to quote fields".to_string(),
            ));
        }
    };

    let mut out = String::with_capacity(text.len() + 8);
    out.push(qc);
    for ch in text.chars() {
        if ch == qc {
            if dialect.doublequote {
                out.push(qc);
                out.push(qc);
            } else if let Some(esc) = dialect.escapechar {
                out.push(esc);
                out.push(ch);
            } else {
                return Err(CsvError::BadInput(
                    "need escapechar when doublequote=False".to_string(),
                ));
            }
        } else {
            out.push(ch);
        }
    }
    out.push(qc);
    Ok(out)
}

/// Write one complete CSV record to a String.
fn write_row_to_string(_py: &PyToken<'_>, row_bits: u64, dialect: &Dialect) -> Result<String, u64> {
    let obj = obj_from_bits(row_bits);
    let Some(row_ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "writerow() argument must be a sequence",
        ));
    };
    // Snapshot the items so we don't hold a borrow across allocations.
    let items_copy: Vec<u64> = unsafe {
        let type_id = object_type_id(row_ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            let msg = format!(
                "writerow() argument must be a sequence, not {}",
                type_name(_py, obj)
            );
            return Err(raise_exception::<u64>(_py, "TypeError", &msg));
        }
        seq_vec_ref(row_ptr).to_vec()
    };

    let mut parts: Vec<String> = Vec::with_capacity(items_copy.len());
    for item_bits in items_copy {
        let (is_none, is_number) = obj_kind(item_bits);
        let text = render_field_text(_py, item_bits);
        let encoded = match encode_field(&text, dialect, is_none, is_number) {
            Ok(s) => s,
            Err(CsvError::NeedEscapechar) => {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "need to escape, but no escapechar set",
                ));
            }
            Err(CsvError::BadInput(msg)) => {
                return Err(raise_exception::<u64>(_py, "ValueError", &msg));
            }
            Err(_) => {
                return Err(raise_exception::<u64>(_py, "ValueError", "csv write error"));
            }
        };
        parts.push(encoded);
    }

    let mut record = parts.join(&dialect.delimiter.to_string());
    record.push_str(&dialect.lineterminator);
    Ok(record)
}

fn sequence_items_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    param_name: &str,
) -> Result<Vec<u64>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{param_name} must be a sequence");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        let msg = format!("{param_name} must be a sequence");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    }
    Ok(unsafe { seq_vec_ref(ptr).to_vec() })
}

// ─────────────────────────────────────────────────────────────────────────────
// Sniffer helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Pick the best delimiter from `candidates` by raw character frequency.
fn detect_delimiter(sample: &str, candidates: Option<&str>) -> char {
    let default_candidates = ",\t;|:";
    let cands: Vec<char> = candidates.unwrap_or(default_candidates).chars().collect();
    if cands.is_empty() {
        return ',';
    }
    let mut best = cands[0];
    let mut best_count = 0usize;
    for &d in &cands {
        let count = sample.chars().filter(|&c| c == d).count();
        if count > best_count {
            best_count = count;
            best = d;
        }
    }
    best
}

/// Detect quotechar patterns in the sample.
/// Returns (quotechar, skipinitialspace, doublequote).
fn detect_quote_char(sample: &str, delimiter: char) -> (Option<char>, bool, bool) {
    let candidates = ['"', '\''];
    let mut best_quote: Option<char> = None;
    let mut best_score = 0usize;
    let mut best_doublequote = true;
    let mut best_skipinitialspace = false;

    for &qc in &candidates {
        let chars: Vec<char> = sample.chars().collect();
        let len = chars.len();
        let mut score = 0usize;
        let mut doublequote = false;
        let mut skipinitialspace = false;
        let mut i = 0usize;

        while i < len {
            let at_field_start =
                i == 0 || chars[i - 1] == delimiter || chars[i - 1] == '\n' || chars[i - 1] == '\r';

            if at_field_start {
                let mut j = i;
                if j < len && chars[j] == ' ' {
                    skipinitialspace = true;
                    j += 1;
                }
                if j < len && chars[j] == qc {
                    j += 1;
                    let mut found_close = false;
                    while j < len {
                        if chars[j] == qc {
                            if j + 1 < len && chars[j + 1] == qc {
                                doublequote = true;
                                j += 2;
                            } else {
                                found_close = true;
                                j += 1;
                                break;
                            }
                        } else {
                            j += 1;
                        }
                    }
                    if found_close {
                        score += 1;
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }

        if score > best_score {
            best_score = score;
            best_quote = Some(qc);
            best_doublequote = doublequote;
            best_skipinitialspace = skipinitialspace;
        }
    }

    (best_quote, best_skipinitialspace, best_doublequote)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderColumnType {
    Unknown,
    Complex,
    Length(usize),
    Removed,
}

fn classify_header_cell_type(cell: &str) -> HeaderColumnType {
    if is_complex_literal(cell) {
        HeaderColumnType::Complex
    } else {
        HeaderColumnType::Length(cell.chars().count())
    }
}

fn parse_signed_float(part: &str) -> Option<f64> {
    if part == "+" {
        return Some(1.0);
    }
    if part == "-" {
        return Some(-1.0);
    }
    part.parse::<f64>().ok()
}

/// Return whether `complex(cell)` should succeed for header sniffing heuristics.
fn is_complex_literal(cell: &str) -> bool {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.parse::<f64>().is_ok() {
        return true;
    }

    let body = if let Some(body) = trimmed.strip_suffix('j') {
        body.trim()
    } else if let Some(body) = trimmed.strip_suffix('J') {
        body.trim()
    } else {
        return false;
    };

    if body.is_empty() || body == "+" || body == "-" {
        return true;
    }
    if body.parse::<f64>().is_ok() {
        return true;
    }

    let bytes = body.as_bytes();
    let mut split_index: Option<usize> = None;
    for idx in 1..bytes.len() {
        let byte = bytes[idx];
        if (byte == b'+' || byte == b'-') && bytes[idx - 1] != b'e' && bytes[idx - 1] != b'E' {
            split_index = Some(idx);
        }
    }
    let Some(split_index) = split_index else {
        return false;
    };
    let (real_part, imag_part) = body.split_at(split_index);
    if real_part.trim().is_empty() || real_part.trim().parse::<f64>().is_err() {
        return false;
    }
    parse_signed_float(imag_part.trim()).is_some()
}

fn sniff_dialect(sample: &str, delimiters: Option<&str>) -> Dialect {
    let delimiter = detect_delimiter(sample, delimiters);
    let (detected_quotechar, skipinitialspace, detected_doublequote) =
        detect_quote_char(sample, delimiter);
    let (quotechar, doublequote) = match detected_quotechar {
        Some(qc) => (Some(qc), detected_doublequote),
        None => (Some('"'), false),
    };
    Dialect {
        delimiter,
        quotechar,
        escapechar: None,
        doublequote,
        skipinitialspace,
        quoting: QUOTE_MINIMAL,
        strict: false,
        lineterminator: "\r\n".to_string(),
    }
}

fn parse_sample_rows(
    sample: &str,
    dialect: &Dialect,
    field_limit: i64,
) -> Result<Vec<Vec<String>>, CsvError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut pending = String::new();
    for physical in split_csv_lines(sample) {
        pending.push_str(&physical);
        match csv_parse_line(&pending, dialect, field_limit) {
            Ok(fields) => {
                rows.push(fields.into_iter().map(|field| field.text).collect());
                pending.clear();
            }
            Err(CsvError::UnexpectedEnd) => {}
            Err(err) => return Err(err),
        }
    }
    if !pending.is_empty() {
        let fields = csv_parse_line(&pending, dialect, field_limit)?;
        rows.push(fields.into_iter().map(|field| field.text).collect());
    }
    Ok(rows)
}

fn raise_csv_parse_error(_py: &PyToken<'_>, err: CsvError) -> u64 {
    match err {
        CsvError::FieldTooLarge => {
            raise_exception::<u64>(_py, "ValueError", "field larger than field limit")
        }
        CsvError::UnexpectedEnd => {
            raise_exception::<u64>(_py, "ValueError", "unexpected end of data")
        }
        CsvError::BadInput(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        CsvError::NeedEscapechar => {
            raise_exception::<u64>(_py, "ValueError", "need escapechar when quoting=QUOTE_NONE")
        }
    }
}

fn alloc_char_bits(_py: &PyToken<'_>, ch: char) -> Result<u64, u64> {
    let text = ch.to_string();
    let ptr = alloc_string(_py, text.as_bytes());
    if ptr.is_null() {
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn alloc_optional_char_bits(_py: &PyToken<'_>, value: Option<char>) -> Result<(u64, bool), u64> {
    if let Some(ch) = value {
        let bits = alloc_char_bits(_py, ch)?;
        Ok((bits, true))
    } else {
        Ok((MoltObject::none().bits(), false))
    }
}

// =============================================================================
// Public intrinsic FFI functions
// =============================================================================

// ── Constants (arity 0) ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_minimal() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_MINIMAL).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_all() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_ALL).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_nonnumeric() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_NONNUMERIC).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_none() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_NONE).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_strings() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_STRINGS).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_quote_notnull() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(QUOTE_NOTNULL).bits() })
}

// ── field_size_limit (arity 1) ────────────────────────────────────────────────

/// Get or set the per-thread field size limit.
/// Pass MoltObject::none() as `new_limit_bits` to query without changing.
/// Returns the previous (or current) limit.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_field_size_limit(new_limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let new_obj = obj_from_bits(new_limit_bits);
        let old = FIELD_SIZE_LIMIT.with(|lim| *lim.borrow());
        if !new_obj.is_none() {
            let Some(new_val) = to_i64(new_obj) else {
                return raise_exception::<u64>(_py, "TypeError", "limit must be an integer");
            };
            if new_val < 0 {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "field size limit must be a non-negative integer",
                );
            }
            FIELD_SIZE_LIMIT.with(|lim| {
                *lim.borrow_mut() = new_val;
            });
        }
        MoltObject::from_int(old).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_register_dialect(
    name_bits: u64,
    delimiter_bits: u64,
    quotechar_bits: u64,
    escapechar_bits: u64,
    doublequote_bits: u64,
    skipinitialspace_bits: u64,
    lineterminator_bits: u64,
    quoting_bits: u64,
    strict_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "dialect name must be a string");
        };
        let delimiter = match char_from_bits(_py, delimiter_bits, "delimiter") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let quotechar = match opt_char_from_bits(_py, quotechar_bits, "quotechar") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let escapechar = match opt_char_from_bits(_py, escapechar_bits, "escapechar") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(lineterminator) = string_obj_to_owned(obj_from_bits(lineterminator_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "\"lineterminator\" must be a string");
        };
        let Some(quoting) = to_i64(obj_from_bits(quoting_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "bad \"quoting\" value");
        };
        let dialect = Dialect {
            delimiter,
            quotechar,
            escapechar,
            doublequote: is_truthy(_py, obj_from_bits(doublequote_bits)),
            skipinitialspace: is_truthy(_py, obj_from_bits(skipinitialspace_bits)),
            quoting,
            strict: is_truthy(_py, obj_from_bits(strict_bits)),
            lineterminator,
        };
        if let Err(bits) = validate_dialect(_py, &dialect) {
            return bits;
        }
        DIALECT_REGISTRY.with(|registry| {
            registry.borrow_mut().insert(name, dialect);
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_unregister_dialect(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "dialect name must be a string");
        };
        let removed = DIALECT_REGISTRY.with(|registry| registry.borrow_mut().remove(&name));
        if removed.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "unknown dialect");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_list_dialects() -> u64 {
    crate::with_gil_entry!(_py, {
        let names = DIALECT_REGISTRY.with(|registry| registry.borrow().names());
        let mut bits_vec = Vec::with_capacity(names.len());
        for name in names {
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                release_bits(_py, &bits_vec);
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            bits_vec.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, &bits_vec);
        release_bits(_py, &bits_vec);
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_get_dialect(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "dialect name must be a string");
        };
        let dialect = DIALECT_REGISTRY.with(|registry| registry.borrow().get(&name).cloned());
        let Some(dialect) = dialect else {
            return raise_exception::<u64>(_py, "ValueError", "unknown dialect");
        };

        let delimiter_bits = match alloc_char_bits(_py, dialect.delimiter) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let (quotechar_bits, quotechar_is_ptr) =
            match alloc_optional_char_bits(_py, dialect.quotechar) {
                Ok(value) => value,
                Err(bits) => {
                    dec_ref_bits(_py, delimiter_bits);
                    return bits;
                }
            };
        let (escapechar_bits, escapechar_is_ptr) =
            match alloc_optional_char_bits(_py, dialect.escapechar) {
                Ok(value) => value,
                Err(bits) => {
                    dec_ref_bits(_py, delimiter_bits);
                    if quotechar_is_ptr {
                        dec_ref_bits(_py, quotechar_bits);
                    }
                    return bits;
                }
            };
        let lineterminator_ptr = alloc_string(_py, dialect.lineterminator.as_bytes());
        if lineterminator_ptr.is_null() {
            dec_ref_bits(_py, delimiter_bits);
            if quotechar_is_ptr {
                dec_ref_bits(_py, quotechar_bits);
            }
            if escapechar_is_ptr {
                dec_ref_bits(_py, escapechar_bits);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let lineterminator_bits = MoltObject::from_ptr(lineterminator_ptr).bits();

        let tuple_ptr = alloc_tuple(
            _py,
            &[
                delimiter_bits,
                quotechar_bits,
                escapechar_bits,
                MoltObject::from_bool(dialect.doublequote).bits(),
                MoltObject::from_bool(dialect.skipinitialspace).bits(),
                lineterminator_bits,
                MoltObject::from_int(dialect.quoting).bits(),
                MoltObject::from_bool(dialect.strict).bits(),
            ],
        );
        dec_ref_bits(_py, delimiter_bits);
        if quotechar_is_ptr {
            dec_ref_bits(_py, quotechar_bits);
        }
        if escapechar_is_ptr {
            dec_ref_bits(_py, escapechar_bits);
        }
        dec_ref_bits(_py, lineterminator_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

// ── Reader: new (arity 7) ─────────────────────────────────────────────────────

/// Create a stateful CSV reader handle.
///
/// Parameters (positional):
///   delimiter_bits      – mandatory 1-char str
///   quotechar_bits      – 1-char str or None
///   escapechar_bits     – 1-char str or None
///   doublequote_bits    – bool
///   skipinitialspace_bits – bool
///   quoting_bits        – int (QUOTE_* constant)
///   strict_bits         – bool
///
/// Returns the handle ID as an int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_reader_new(
    delimiter_bits: u64,
    quotechar_bits: u64,
    escapechar_bits: u64,
    doublequote_bits: u64,
    skipinitialspace_bits: u64,
    quoting_bits: u64,
    strict_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let delimiter = match char_from_bits(_py, delimiter_bits, "delimiter") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let quotechar = match opt_char_from_bits(_py, quotechar_bits, "quotechar") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let escapechar = match opt_char_from_bits(_py, escapechar_bits, "escapechar") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let doublequote = is_truthy(_py, obj_from_bits(doublequote_bits));
        let skipinitialspace = is_truthy(_py, obj_from_bits(skipinitialspace_bits));
        let quoting = to_i64(obj_from_bits(quoting_bits)).unwrap_or(QUOTE_MINIMAL);
        let strict = is_truthy(_py, obj_from_bits(strict_bits));

        if quotechar.is_none() && quoting != QUOTE_NONE {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "quotechar must be set if quoting != QUOTE_NONE",
            );
        }

        let dialect = Dialect {
            delimiter,
            quotechar,
            escapechar,
            doublequote,
            skipinitialspace,
            quoting,
            strict,
            lineterminator: "\r\n".to_string(),
        };
        let id = next_handle_id();
        READER_HANDLES.with(|map| {
            map.borrow_mut().insert(id, ReaderState { dialect });
        });
        MoltObject::from_int(id).bits()
    })
}

// ── Reader: parse_line (arity 2) ──────────────────────────────────────────────

/// Parse a single line of CSV text into a list<str> of fields.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_reader_parse_line(handle_bits: u64, line_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle_id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid csv reader handle");
        };
        let Some(line) = string_obj_to_owned(obj_from_bits(line_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "line must be str");
        };
        let field_limit = FIELD_SIZE_LIMIT.with(|lim| *lim.borrow());

        let dialect =
            READER_HANDLES.with(|map| map.borrow().get(&handle_id).map(|s| s.dialect.clone()));
        let Some(dialect) = dialect else {
            return raise_exception::<u64>(_py, "ValueError", "csv reader handle not found");
        };

        let fields = match csv_parse_line(&line, &dialect, field_limit) {
            Ok(f) => f,
            Err(CsvError::FieldTooLarge) => {
                return raise_exception::<u64>(_py, "ValueError", "field larger than field limit");
            }
            Err(CsvError::UnexpectedEnd) => {
                return raise_exception::<u64>(_py, "ValueError", "unexpected end of data");
            }
            Err(CsvError::BadInput(msg)) => {
                return raise_exception::<u64>(_py, "ValueError", &msg);
            }
            Err(CsvError::NeedEscapechar) => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "need escapechar when quoting=QUOTE_NONE",
                );
            }
        };

        match fields_to_list(_py, &fields, &dialect) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

// ── Reader: parse_lines (arity 2) ─────────────────────────────────────────────

/// Parse multi-line CSV text into a list<list<str>>.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_reader_parse_lines(handle_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle_id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid csv reader handle");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "text must be str");
        };
        let field_limit = FIELD_SIZE_LIMIT.with(|lim| *lim.borrow());

        let dialect =
            READER_HANDLES.with(|map| map.borrow().get(&handle_id).map(|s| s.dialect.clone()));
        let Some(dialect) = dialect else {
            return raise_exception::<u64>(_py, "ValueError", "csv reader handle not found");
        };

        let raw_lines = split_csv_lines(&text);
        let mut row_bits_vec: Vec<u64> = Vec::with_capacity(raw_lines.len());

        for line in &raw_lines {
            // Skip completely blank logical lines.
            let trimmed = line.trim_matches(|c: char| c == '\r' || c == '\n');
            if trimmed.is_empty() {
                continue;
            }
            let fields = match csv_parse_line(line, &dialect, field_limit) {
                Ok(f) => f,
                Err(CsvError::FieldTooLarge) => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "field larger than field limit",
                    );
                }
                Err(CsvError::UnexpectedEnd) => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return raise_exception::<u64>(_py, "ValueError", "unexpected end of data");
                }
                Err(CsvError::BadInput(msg)) => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return raise_exception::<u64>(_py, "ValueError", &msg);
                }
                Err(CsvError::NeedEscapechar) => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "need escapechar when quoting=QUOTE_NONE",
                    );
                }
            };
            match fields_to_list(_py, &fields, &dialect) {
                Ok(bits) => row_bits_vec.push(bits),
                Err(bits) => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return bits;
                }
            }
        }

        let outer_ptr = alloc_list(_py, &row_bits_vec);
        for b in &row_bits_vec {
            dec_ref_bits(_py, *b);
        }
        if outer_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(outer_ptr).bits()
    })
}

// ── Reader: drop (arity 1) ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_reader_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            READER_HANDLES.with(|map| {
                map.borrow_mut().remove(&id);
            });
        }
        MoltObject::none().bits()
    })
}

// ── Writer: new (arity 6) ─────────────────────────────────────────────────────

/// Create a stateful CSV writer handle.
///
/// Parameters (positional):
///   delimiter_bits       – mandatory 1-char str
///   quotechar_bits       – 1-char str or None
///   escapechar_bits      – 1-char str or None
///   doublequote_bits     – bool
///   quoting_bits         – int (QUOTE_* constant)
///   lineterminator_bits  – str (default "\r\n" when None is passed)
///
/// Returns the handle ID as an int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_writer_new(
    delimiter_bits: u64,
    quotechar_bits: u64,
    escapechar_bits: u64,
    doublequote_bits: u64,
    quoting_bits: u64,
    lineterminator_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let delimiter = match char_from_bits(_py, delimiter_bits, "delimiter") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let quotechar = match opt_char_from_bits(_py, quotechar_bits, "quotechar") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let escapechar = match opt_char_from_bits(_py, escapechar_bits, "escapechar") {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        let doublequote = is_truthy(_py, obj_from_bits(doublequote_bits));
        let quoting = to_i64(obj_from_bits(quoting_bits)).unwrap_or(QUOTE_MINIMAL);

        let lineterminator = {
            let obj = obj_from_bits(lineterminator_bits);
            if obj.is_none() {
                "\r\n".to_string()
            } else {
                match string_obj_to_owned(obj) {
                    Some(s) => s,
                    None => {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "lineterminator must be a string",
                        );
                    }
                }
            }
        };

        if quotechar.is_none() && quoting != QUOTE_NONE {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "quotechar must be set if quoting != QUOTE_NONE",
            );
        }

        let dialect = Dialect {
            delimiter,
            quotechar,
            escapechar,
            doublequote,
            skipinitialspace: false,
            quoting,
            strict: false,
            lineterminator,
        };
        let id = next_handle_id();
        WRITER_HANDLES.with(|map| {
            map.borrow_mut().insert(id, WriterState { dialect });
        });
        MoltObject::from_int(id).bits()
    })
}

// ── Writer: writerow (arity 2) ────────────────────────────────────────────────

/// Write a single row (list/tuple of objects) and return the CSV record string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_writer_writerow(handle_bits: u64, row_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle_id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid csv writer handle");
        };
        let dialect =
            WRITER_HANDLES.with(|map| map.borrow().get(&handle_id).map(|s| s.dialect.clone()));
        let Some(dialect) = dialect else {
            return raise_exception::<u64>(_py, "ValueError", "csv writer handle not found");
        };

        let record = match write_row_to_string(_py, row_bits, &dialect) {
            Ok(s) => s,
            Err(bits) => return bits,
        };

        let str_ptr = alloc_string(_py, record.as_bytes());
        if str_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(str_ptr).bits()
    })
}

// ── Writer: writerows (arity 2) ───────────────────────────────────────────────

/// Write multiple rows (list/tuple of list/tuple of objects) and return the
/// concatenated CSV records as a single string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_writer_writerows(handle_bits: u64, rows_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle_id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid csv writer handle");
        };
        let dialect =
            WRITER_HANDLES.with(|map| map.borrow().get(&handle_id).map(|s| s.dialect.clone()));
        let Some(dialect) = dialect else {
            return raise_exception::<u64>(_py, "ValueError", "csv writer handle not found");
        };

        let rows_obj = obj_from_bits(rows_bits);
        let Some(rows_ptr) = rows_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "rows must be a sequence");
        };

        // Snapshot the rows slice so we don't hold a borrow across allocations.
        let row_bits_vec: Vec<u64> = unsafe {
            let type_id = object_type_id(rows_ptr);
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                let msg = format!(
                    "writerows() argument must be a sequence, not {}",
                    type_name(_py, rows_obj)
                );
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            seq_vec_ref(rows_ptr).to_vec()
        };

        let mut out = String::new();
        for row_bits in row_bits_vec {
            let record = match write_row_to_string(_py, row_bits, &dialect) {
                Ok(s) => s,
                Err(bits) => return bits,
            };
            out.push_str(&record);
        }

        let str_ptr = alloc_string(_py, out.as_bytes());
        if str_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(str_ptr).bits()
    })
}

// ── Writer: drop (arity 1) ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_writer_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            WRITER_HANDLES.with(|map| {
                map.borrow_mut().remove(&id);
            });
        }
        MoltObject::none().bits()
    })
}

/// Project DictReader row semantics in Rust:
/// dict(zip(fieldnames, row)) + restkey/restval handling.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_dict_project(
    fieldnames_bits: u64,
    row_bits: u64,
    restkey_bits: u64,
    restval_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let fieldnames = match sequence_items_from_bits(_py, fieldnames_bits, "fieldnames") {
            Ok(items) => items,
            Err(bits) => return bits,
        };
        let row = match sequence_items_from_bits(_py, row_bits, "row") {
            Ok(items) => items,
            Err(bits) => return bits,
        };

        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }

        let field_len = fieldnames.len();
        let row_len = row.len();
        let shared_len = field_len.min(row_len);
        for idx in 0..shared_len {
            unsafe {
                dict_set_in_place(_py, dict_ptr, fieldnames[idx], row[idx]);
            }
        }

        if field_len < row_len {
            let extras_ptr = alloc_list(_py, &row[field_len..]);
            if extras_ptr.is_null() {
                let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                dec_ref_bits(_py, dict_bits);
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            let extras_bits = MoltObject::from_ptr(extras_ptr).bits();
            unsafe {
                dict_set_in_place(_py, dict_ptr, restkey_bits, extras_bits);
            }
            dec_ref_bits(_py, extras_bits);
        } else if field_len > row_len {
            for key in fieldnames.iter().skip(row_len) {
                unsafe {
                    dict_set_in_place(_py, dict_ptr, *key, restval_bits);
                }
            }
        }

        MoltObject::from_ptr(dict_ptr).bits()
    })
}

// ── Sniffer (arity 2) ─────────────────────────────────────────────────────────

/// Analyse a CSV sample and return a 4-tuple:
///   (delimiter: str, doublequote: bool, quotechar: str | None, skipinitialspace: bool)
///
/// `delimiters_bits` is an optional str of candidate delimiter characters, or
/// None to use the default set (",\t;|:").
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_sniff(sample_bits: u64, delimiters_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sample) = string_obj_to_owned(obj_from_bits(sample_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "sample must be str");
        };

        let del_obj = obj_from_bits(delimiters_bits);
        let delimiters_owned: Option<String> = if del_obj.is_none() {
            None
        } else {
            match string_obj_to_owned(del_obj) {
                Some(s) => Some(s),
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "delimiters must be a string or None",
                    );
                }
            }
        };

        let dialect = sniff_dialect(&sample, delimiters_owned.as_deref());

        // Allocate the delimiter string element.
        let delim_str = dialect.delimiter.to_string();
        let delim_ptr = alloc_string(_py, delim_str.as_bytes());
        if delim_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let delim_bits = MoltObject::from_ptr(delim_ptr).bits();

        // Allocate the quotechar string element (or use None).
        let (qc_bits, qc_is_ptr) = if let Some(qc) = dialect.quotechar {
            let qc_str = qc.to_string();
            let qc_ptr = alloc_string(_py, qc_str.as_bytes());
            if qc_ptr.is_null() {
                dec_ref_bits(_py, delim_bits);
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            (MoltObject::from_ptr(qc_ptr).bits(), true)
        } else {
            (MoltObject::none().bits(), false)
        };

        // Build the result tuple.
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                delim_bits,
                MoltObject::from_bool(dialect.doublequote).bits(),
                qc_bits,
                MoltObject::from_bool(dialect.skipinitialspace).bits(),
            ],
        );

        // Release our temporary refs; the tuple holds its own.
        dec_ref_bits(_py, delim_bits);
        if qc_is_ptr {
            dec_ref_bits(_py, qc_bits);
        }

        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

/// Apply CPython-compatible `Sniffer.has_header` voting against intrinsic csv parsing.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_has_header(sample_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sample) = string_obj_to_owned(obj_from_bits(sample_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "sample must be str");
        };

        let dialect = sniff_dialect(&sample, None);
        let field_limit = FIELD_SIZE_LIMIT.with(|lim| *lim.borrow());
        let rows = match parse_sample_rows(&sample, &dialect, field_limit) {
            Ok(rows) => rows,
            Err(err) => return raise_csv_parse_error(_py, err),
        };

        let Some(header) = rows.first() else {
            return MoltObject::from_bool(false).bits();
        };
        let columns = header.len();
        let mut column_types = vec![HeaderColumnType::Unknown; columns];

        let mut checked = 0usize;
        for row in rows.iter().skip(1) {
            if checked > 20 {
                break;
            }
            checked += 1;
            if row.len() != columns {
                continue;
            }
            for col in 0..columns {
                if column_types[col] == HeaderColumnType::Removed {
                    continue;
                }
                let this_type = classify_header_cell_type(&row[col]);
                let current_type = column_types[col];
                if this_type != current_type {
                    if current_type == HeaderColumnType::Unknown {
                        column_types[col] = this_type;
                    } else {
                        column_types[col] = HeaderColumnType::Removed;
                    }
                }
            }
        }

        let mut has_header_score = 0i64;
        for (col, col_type) in column_types.iter().enumerate() {
            match col_type {
                HeaderColumnType::Removed => continue,
                HeaderColumnType::Unknown => has_header_score += 1,
                HeaderColumnType::Length(length) => {
                    if header[col].chars().count() != *length {
                        has_header_score += 1;
                    } else {
                        has_header_score -= 1;
                    }
                }
                HeaderColumnType::Complex => {
                    if is_complex_literal(&header[col]) {
                        has_header_score -= 1;
                    } else {
                        has_header_score += 1;
                    }
                }
            }
        }

        MoltObject::from_bool(has_header_score > 0).bits()
    })
}

// ---------------------------------------------------------------------------
// New intrinsics for full intrinsic-backing of csv.py
// ---------------------------------------------------------------------------

/// Validate that all fmtparam keys are valid Dialect attribute names.
/// Returns None on success, raises TypeError on invalid key.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_validate_fmtparams(keys_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static VALID_KEYS: &[&str] = &[
            "delimiter", "quotechar", "escapechar", "doublequote",
            "skipinitialspace", "lineterminator", "quoting", "strict",
        ];
        let obj = obj_from_bits(keys_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST || tid == crate::TYPE_ID_TUPLE {
                    let items = seq_vec_ref(ptr);
                    for &item_bits in items.iter() {
                        if let Some(key) = string_obj_to_owned(obj_from_bits(item_bits)) {
                            if !VALID_KEYS.contains(&key.as_str()) {
                                return raise_exception::<u64>(
                                    _py,
                                    "TypeError",
                                    &format!("this function got an unexpected keyword argument {key:?}"),
                                );
                            }
                        }
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

/// Validate a dialect's field values. Returns None on success, raises TypeError on error.
/// Args: delimiter, quotechar, escapechar, lineterminator, quoting (as int)
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_validate_dialect(
    delimiter_bits: u64,
    quotechar_bits: u64,
    escapechar_bits: u64,
    lineterminator_bits: u64,
    quoting_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        // Validate delimiter
        let delimiter_obj = obj_from_bits(delimiter_bits);
        if let Some(d) = string_obj_to_owned(delimiter_obj) {
            if d.chars().count() != 1 {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    &format!("\"delimiter\" must be a unicode character, not a string of length {}", d.chars().count()),
                );
            }
        } else {
            let tname = type_name(_py, delimiter_obj);
            return raise_exception::<u64>(
                _py,
                "TypeError",
                &format!("\"delimiter\" must be a unicode character, not {tname}"),
            );
        }

        // Validate quotechar (can be None)
        let qc_obj = obj_from_bits(quotechar_bits);
        let qc_is_none = qc_obj.is_none();
        if !qc_is_none {
            if let Some(q) = string_obj_to_owned(qc_obj) {
                if q.chars().count() != 1 {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("\"quotechar\" must be a unicode character or None, not a string of length {}", q.chars().count()),
                    );
                }
            } else {
                let tname = type_name(_py, qc_obj);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    &format!("\"quotechar\" must be a unicode character or None, not {tname}"),
                );
            }
        }

        // Validate escapechar (can be None)
        let ec_obj = obj_from_bits(escapechar_bits);
        if !ec_obj.is_none() {
            if let Some(e) = string_obj_to_owned(ec_obj) {
                if e.chars().count() != 1 {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("\"escapechar\" must be a unicode character or None, not a string of length {}", e.chars().count()),
                    );
                }
            } else {
                let tname = type_name(_py, ec_obj);
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    &format!("\"escapechar\" must be a unicode character or None, not {tname}"),
                );
            }
        }

        // Validate lineterminator
        let lt_obj = obj_from_bits(lineterminator_bits);
        if string_obj_to_owned(lt_obj).is_none() {
            let tname = type_name(_py, lt_obj);
            return raise_exception::<u64>(
                _py,
                "TypeError",
                &format!("\"lineterminator\" must be a string, not {tname}"),
            );
        }

        // Validate quoting
        let Some(quoting) = to_i64(obj_from_bits(quoting_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "bad \"quoting\" value");
        };
        if ![QUOTE_MINIMAL, QUOTE_ALL, QUOTE_NONNUMERIC, QUOTE_NONE, QUOTE_STRINGS, QUOTE_NOTNULL].contains(&quoting) {
            return raise_exception::<u64>(_py, "TypeError", "bad \"quoting\" value");
        }

        // quotechar must be set if quoting enabled
        if qc_is_none && quoting != QUOTE_NONE {
            return raise_exception::<u64>(_py, "TypeError", "quotechar must be set if quoting enabled");
        }

        MoltObject::none().bits()
    })
}

/// Normalize a row into a list. If already a list/tuple, return as-is.
/// Otherwise try to iterate and collect into a list.
/// Raises csv.Error if not iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_normalize_row(row_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(row_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST || tid == crate::TYPE_ID_TUPLE {
                    // Already a list or tuple; return directly.
                    return row_bits;
                }
            }
        }
        // For other iterables, the Python side will handle via list().
        // Return None to signal "needs Python-side list() conversion".
        MoltObject::none().bits()
    })
}

/// Look up a dialect name, validating it's a string.
/// Returns the string on success.
/// Raises Error("unknown dialect") for non-string hashable values.
/// Raises TypeError for unhashable values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_dialect_lookup_name(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(name_bits);
        if let Some(s) = string_obj_to_owned(obj) {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        // For non-string types, check hashability (simplified: lists/dicts are unhashable)
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST || tid == TYPE_ID_DICT || tid == crate::TYPE_ID_SET {
                    let tname = type_name(_py, obj);
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("cannot use '{tname}' as a dict key (unhashable type: '{tname}')"),
                    );
                }
            }
        }
        raise_exception::<u64>(_py, "csv.Error", "unknown dialect")
    })
}
