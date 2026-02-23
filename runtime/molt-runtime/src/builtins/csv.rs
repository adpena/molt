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

// ─────────────────────────────────────────────────────────────────────────────
// CSV parse error enum
// ─────────────────────────────────────────────────────────────────────────────

enum CsvError {
    FieldTooLarge,
    UnexpectedEnd,
    BadInput(String),
    NeedEscapechar,
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
) -> Result<Vec<String>, CsvError> {
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

    let mut fields: Vec<String> = Vec::new();
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
            fields.push(std::mem::take(&mut field));
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
        fields.push(std::mem::take(&mut field));
    }

    Ok(fields)
}

// ─────────────────────────────────────────────────────────────────────────────
// Build a MoltObject list<str> from a Vec<String>.
// Returns None on allocation failure.
// ─────────────────────────────────────────────────────────────────────────────
fn fields_to_list(_py: &PyToken<'_>, fields: &[String]) -> Option<u64> {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(fields.len());
    for field in fields {
        let str_ptr = alloc_string(_py, field.as_bytes());
        if str_ptr.is_null() {
            for b in &bits_vec {
                dec_ref_bits(_py, *b);
            }
            return None;
        }
        bits_vec.push(MoltObject::from_ptr(str_ptr).bits());
    }
    let list_ptr = alloc_list(_py, &bits_vec);
    // The list increments refs; release our temporary refs.
    for b in &bits_vec {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
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
fn render_field_text(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Some(String::new());
    }
    // Boolean before integer: bool is an int subtype in Python.
    if let Some(b) = obj.as_bool() {
        return Some(if b {
            "True".to_string()
        } else {
            "False".to_string()
        });
    }
    if let Some(s) = string_obj_to_owned(obj) {
        return Some(s);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i.to_string());
    }
    if let Some(f) = to_f64(obj) {
        return Some(repr_float(f));
    }
    // Fallback for heap objects that have no direct string/int/float repr.
    Some(format!("<object@{bits:016x}>"))
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
        let text = match render_field_text(_py, item_bits) {
            Some(s) => s,
            None => return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory")),
        };
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

        match fields_to_list(_py, &fields) {
            Some(bits) => bits,
            None => raise_exception::<u64>(_py, "MemoryError", "out of memory"),
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
            match fields_to_list(_py, &fields) {
                Some(b) => row_bits_vec.push(b),
                None => {
                    for b in &row_bits_vec {
                        dec_ref_bits(_py, *b);
                    }
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
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

        let delimiter = detect_delimiter(&sample, delimiters_owned.as_deref());
        let (quotechar, skipinitialspace, doublequote) = detect_quote_char(&sample, delimiter);

        // Allocate the delimiter string element.
        let delim_str = delimiter.to_string();
        let delim_ptr = alloc_string(_py, delim_str.as_bytes());
        if delim_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let delim_bits = MoltObject::from_ptr(delim_ptr).bits();

        // Allocate the quotechar string element (or use None).
        let (qc_bits, qc_is_ptr) = if let Some(qc) = quotechar {
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
                MoltObject::from_bool(doublequote).bits(),
                qc_bits,
                MoltObject::from_bool(skipinitialspace).bits(),
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
