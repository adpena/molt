#![allow(dead_code, unused_imports)]
//! Intrinsics for the `logging` stdlib module.
//!
//! Implements LogRecord, Formatter, Handler, StreamHandler, Logger, Manager,
//! and level utilities with CPython `logging` module semantics.
//!
//! All handle registries use `LazyLock<Mutex<HashMap<...>>>` for cross-thread
//! visibility.  The GIL serializes all Python-level access so the Mutex is
//! always uncontended — it only satisfies Rust's `Send + Sync` requirements.

use molt_runtime_core::prelude::*;
use crate::bridge::{
    alloc_string, dec_ref_bits, raise_exception,
    string_obj_to_owned, to_i64,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Standard logging levels (CPython) ────────────────────────────────────────

const CRITICAL: i64 = 50;
const ERROR: i64 = 40;
const WARNING: i64 = 30;
const INFO: i64 = 20;
const DEBUG: i64 = 10;
const NOTSET: i64 = 0;

// ── Handle counters ──────────────────────────────────────────────────────────

static NEXT_RECORD_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_FORMATTER_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_HANDLER_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_LOGGER_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_record_handle() -> i64 {
    NEXT_RECORD_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_formatter_handle() -> i64 {
    NEXT_FORMATTER_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_handler_handle() -> i64 {
    NEXT_HANDLER_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn next_logger_handle() -> i64 {
    NEXT_LOGGER_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn return_str(_py: &PyToken, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

fn handle_from_bits(_py: &PyToken, handle_bits: u64, kind: &str) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let msg = format!("{kind} handle must be an int");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    };
    Some(id)
}

fn opt_str(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        None
    } else {
        string_obj_to_owned(obj)
    }
}

fn current_time_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ── Level name mapping ───────────────────────────────────────────────────────

struct LevelNames {
    level_to_name: HashMap<i64, String>,
    name_to_level: HashMap<String, i64>,
}

impl LevelNames {
    fn new() -> Self {
        let mut level_to_name = HashMap::new();
        let mut name_to_level = HashMap::new();
        let defaults = [
            (CRITICAL, "CRITICAL"),
            (ERROR, "ERROR"),
            (WARNING, "WARNING"),
            (INFO, "INFO"),
            (DEBUG, "DEBUG"),
            (NOTSET, "NOTSET"),
        ];
        for &(level, name) in &defaults {
            level_to_name.insert(level, name.to_string());
            name_to_level.insert(name.to_string(), level);
        }
        // CPython also maps "FATAL" -> CRITICAL and "WARN" -> WARNING.
        name_to_level.insert("FATAL".to_string(), CRITICAL);
        name_to_level.insert("WARN".to_string(), WARNING);
        Self {
            level_to_name,
            name_to_level,
        }
    }

    fn get_name(&self, level: i64) -> String {
        self.level_to_name
            .get(&level)
            .cloned()
            .unwrap_or_else(|| format!("Level {level}"))
    }

    fn get_level(&self, name: &str) -> Option<i64> {
        self.name_to_level.get(name).copied()
    }

    fn add(&mut self, level: i64, name: String) {
        self.level_to_name.insert(level, name.clone());
        self.name_to_level.insert(name, level);
    }
}

static LEVEL_NAMES: LazyLock<Mutex<LevelNames>> = LazyLock::new(|| Mutex::new(LevelNames::new()));

// ── LogRecord ────────────────────────────────────────────────────────────────

struct LogRecordState {
    name: String,
    level: i64,
    levelname: String,
    pathname: String,
    filename: String,
    module: String,
    lineno: i64,
    msg: String,
    args: String,
    exc_info: String,
    func_name: String,
    created: f64,
    msecs: f64,
    relative_created: f64,
    thread: i64,
    thread_name: String,
    process: i64,
    process_name: String,
    message: Option<String>,
    asctime: Option<String>,
    exc_text: Option<String>,
    stack_info: Option<String>,
}

static RECORD_REGISTRY: LazyLock<Mutex<HashMap<i64, LogRecordState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Process start time for relativeCreated calculation.
static START_TIME: LazyLock<f64> = LazyLock::new(current_time_secs);

impl LogRecordState {
    fn get_message(&self) -> String {
        if let Some(ref cached) = self.message {
            return cached.clone();
        }
        if self.args.is_empty() || self.args == "()" || self.args == "None" {
            return self.msg.clone();
        }
        // In the intrinsic layer we store the already-formatted message
        // from the Python side (args have been applied there). Return msg as-is.
        self.msg.clone()
    }
}

fn filename_from_pathname(pathname: &str) -> String {
    match pathname.rfind('/') {
        Some(pos) => pathname[pos + 1..].to_string(),
        None => match pathname.rfind('\\') {
            Some(pos) => pathname[pos + 1..].to_string(),
            None => pathname.to_string(),
        },
    }
}

fn module_from_filename(filename: &str) -> String {
    match filename.rfind('.') {
        Some(pos) => filename[..pos].to_string(),
        None => filename.to_string(),
    }
}

// ── LogRecord intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_record_new(
    name_bits: u64,
    level_bits: u64,
    pathname_bits: u64,
    lineno_bits: u64,
    msg_bits: u64,
    args_bits: u64,
    exc_info_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "LogRecord name must be str");
        };
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "LogRecord level must be int");
        };
        let pathname = opt_str(pathname_bits).unwrap_or_default();
        let lineno = to_i64(obj_from_bits(lineno_bits)).unwrap_or(0);
        let msg = opt_str(msg_bits).unwrap_or_default();
        let args = opt_str(args_bits).unwrap_or_default();
        let exc_info = opt_str(exc_info_bits).unwrap_or_default();

        let filename = filename_from_pathname(&pathname);
        let module = module_from_filename(&filename);
        let levelname = LEVEL_NAMES.lock().unwrap().get_name(level);
        let created = current_time_secs();
        let msecs = (created - created.floor()) * 1000.0;
        let start = *START_TIME;
        let relative_created = (created - start) * 1000.0;

        let record = LogRecordState {
            name,
            level,
            levelname,
            pathname,
            filename,
            module,
            lineno,
            msg,
            args,
            exc_info,
            func_name: String::new(),
            created,
            msecs,
            relative_created,
            thread: 0,
            thread_name: "MainThread".to_string(),
            process: std::process::id() as i64,
            process_name: "MainProcess".to_string(),
            message: None,
            asctime: None,
            exc_text: None,
            stack_info: None,
        };

        let id = next_record_handle();
        RECORD_REGISTRY.lock().unwrap().insert(id, record);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_record_get_message(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let message = {
            let registry = RECORD_REGISTRY.lock().unwrap();
            let Some(record) = registry.get(&id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
            };
            record.get_message()
        };
        return_str(_py, &message)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_record_get_attr(handle_bits: u64, attr_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let Some(attr_name) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "attribute name must be str");
        };
        let registry = RECORD_REGISTRY.lock().unwrap();
        let Some(record) = registry.get(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
        };
        match attr_name.as_str() {
            "name" => return_str(_py, &record.name),
            "msg" => return_str(_py, &record.msg),
            "args" => return_str(_py, &record.args),
            "levelno" => MoltObject::from_int(record.level).bits(),
            "levelname" => return_str(_py, &record.levelname),
            "pathname" => return_str(_py, &record.pathname),
            "filename" => return_str(_py, &record.filename),
            "module" => return_str(_py, &record.module),
            "lineno" => MoltObject::from_int(record.lineno).bits(),
            "funcName" => return_str(_py, &record.func_name),
            "created" => MoltObject::from_float(record.created).bits(),
            "msecs" => MoltObject::from_float(record.msecs).bits(),
            "relativeCreated" => MoltObject::from_float(record.relative_created).bits(),
            "thread" => MoltObject::from_int(record.thread).bits(),
            "threadName" => return_str(_py, &record.thread_name),
            "process" => MoltObject::from_int(record.process).bits(),
            "processName" => return_str(_py, &record.process_name),
            "message" => {
                let msg = record.message.as_deref().unwrap_or("");
                return_str(_py, msg)
            }
            "asctime" => {
                let asc = record.asctime.as_deref().unwrap_or("");
                return_str(_py, asc)
            }
            "exc_info" => return_str(_py, &record.exc_info),
            "exc_text" => {
                let et = record.exc_text.as_deref().unwrap_or("");
                return_str(_py, et)
            }
            "stack_info" => {
                let si = record.stack_info.as_deref().unwrap_or("");
                return_str(_py, si)
            }
            _ => {
                let msg = format!("LogRecord has no attribute '{attr_name}'");
                raise_exception::<u64>(_py, "AttributeError", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_record_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        RECORD_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}

// ── Formatter ────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum FormatStyle {
    Percent,
    StrFormat,
    Dollar,
}

struct FormatterState {
    fmt: String,
    datefmt: Option<String>,
    style: FormatStyle,
    default_time_format: String,
    default_msec_format: String,
}

static FORMATTER_REGISTRY: LazyLock<Mutex<HashMap<i64, FormatterState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl FormatterState {
    fn uses_time(&self) -> bool {
        match self.style {
            FormatStyle::Percent => self.fmt.contains("%(asctime)"),
            FormatStyle::StrFormat => self.fmt.contains("{asctime"),
            FormatStyle::Dollar => self.fmt.contains("$asctime"),
        }
    }

    fn format_time(&self, record: &LogRecordState) -> String {
        let created = record.created;
        let msecs = record.msecs;

        if let Some(ref datefmt) = self.datefmt {
            // Apply the custom datefmt via strftime-style formatting.
            format_time_strftime(created, datefmt)
        } else {
            // Default: use default_time_format then apply msec format.
            let base = format_time_strftime(created, &self.default_time_format);
            // CPython default_msec_format is "%s,%03d"
            format!("{},{:03}", base, msecs as u32)
        }
    }

    fn format_record(&self, record: &LogRecordState) -> String {
        let message = record.get_message();
        let asctime = if self.uses_time() {
            self.format_time(record)
        } else {
            String::new()
        };

        let result = self.apply_format(record, &message, &asctime);

        let mut s = result;
        if !record.exc_info.is_empty()
            && record.exc_info != "None"
            && record.exc_info != "False"
            && let Some(ref exc_text) = record.exc_text
            && !exc_text.is_empty()
        {
            s.push('\n');
            s.push_str(exc_text);
        }
        if let Some(ref stack_info) = record.stack_info
            && !stack_info.is_empty()
        {
            s.push('\n');
            s.push_str(stack_info);
        }
        s
    }

    fn apply_format(&self, record: &LogRecordState, message: &str, asctime: &str) -> String {
        match self.style {
            FormatStyle::Percent => percent_format(&self.fmt, record, message, asctime),
            FormatStyle::StrFormat => strformat_format(&self.fmt, record, message, asctime),
            FormatStyle::Dollar => dollar_format(&self.fmt, record, message, asctime),
        }
    }
}

/// Apply CPython-style `%(key)s` / `%(key)d` / `%(key)f` formatting for log records.
fn percent_format(fmt: &str, record: &LogRecordState, message: &str, asctime: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + message.len());
    let chars: Vec<char> = fmt.chars().collect();
    let mut idx = 0;

    while idx < chars.len() {
        if chars[idx] != '%' {
            out.push(chars[idx]);
            idx += 1;
            continue;
        }
        // Look for %(key)s pattern
        if idx + 1 < chars.len() && chars[idx + 1] == '%' {
            out.push('%');
            idx += 2;
            continue;
        }
        if idx + 1 < chars.len() && chars[idx + 1] == '(' {
            // Parse %(key)X
            let start = idx + 2;
            let mut end = start;
            while end < chars.len() && chars[end] != ')' {
                end += 1;
            }
            if end < chars.len() && end + 1 < chars.len() {
                let key: String = chars[start..end].iter().collect();
                let spec = chars[end + 1];
                let value = lookup_record_field(record, &key, message, asctime);
                match spec {
                    's' => out.push_str(&value),
                    'd' => {
                        // Try to parse as integer for %d formatting.
                        if let Ok(v) = value.parse::<i64>() {
                            out.push_str(&v.to_string());
                        } else if let Ok(v) = value.parse::<f64>() {
                            out.push_str(&(v as i64).to_string());
                        } else {
                            out.push_str(&value);
                        }
                    }
                    'f' => {
                        if let Ok(v) = value.parse::<f64>() {
                            out.push_str(&format!("{v:.6}"));
                        } else {
                            out.push_str(&value);
                        }
                    }
                    _ => {
                        // Unknown specifier: output raw
                        out.push('%');
                        out.push('(');
                        out.push_str(&key);
                        out.push(')');
                        out.push(spec);
                    }
                }
                idx = end + 2;
            } else {
                // Malformed: emit raw
                out.push(chars[idx]);
                idx += 1;
            }
        } else {
            out.push(chars[idx]);
            idx += 1;
        }
    }
    out
}

/// Apply `{key}` style formatting for log records.
fn strformat_format(fmt: &str, record: &LogRecordState, message: &str, asctime: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + message.len());
    let chars: Vec<char> = fmt.chars().collect();
    let mut idx = 0;

    while idx < chars.len() {
        if chars[idx] == '{' && idx + 1 < chars.len() && chars[idx + 1] == '{' {
            out.push('{');
            idx += 2;
            continue;
        }
        if chars[idx] == '}' && idx + 1 < chars.len() && chars[idx + 1] == '}' {
            out.push('}');
            idx += 2;
            continue;
        }
        if chars[idx] == '{' {
            let start = idx + 1;
            let mut end = start;
            // Find closing }, handling optional format spec after ':'
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end < chars.len() {
                let inner: String = chars[start..end].iter().collect();
                // Split on ':' for format spec
                let key = if let Some(colon_pos) = inner.find(':') {
                    &inner[..colon_pos]
                } else {
                    &inner
                };
                let value = lookup_record_field(record, key, message, asctime);
                out.push_str(&value);
                idx = end + 1;
            } else {
                out.push(chars[idx]);
                idx += 1;
            }
        } else {
            out.push(chars[idx]);
            idx += 1;
        }
    }
    out
}

/// Apply `$key` / `${key}` style formatting for log records.
fn dollar_format(fmt: &str, record: &LogRecordState, message: &str, asctime: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + message.len());
    let chars: Vec<char> = fmt.chars().collect();
    let mut idx = 0;

    while idx < chars.len() {
        if chars[idx] == '$' && idx + 1 < chars.len() && chars[idx + 1] == '$' {
            out.push('$');
            idx += 2;
            continue;
        }
        if chars[idx] == '$' && idx + 1 < chars.len() && chars[idx + 1] == '{' {
            // ${key} form
            let start = idx + 2;
            let mut end = start;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end < chars.len() {
                let key: String = chars[start..end].iter().collect();
                let value = lookup_record_field(record, &key, message, asctime);
                out.push_str(&value);
                idx = end + 1;
            } else {
                out.push(chars[idx]);
                idx += 1;
            }
        } else if chars[idx] == '$'
            && idx + 1 < chars.len()
            && (chars[idx + 1].is_alphanumeric() || chars[idx + 1] == '_')
        {
            // $key form — consume identifier chars
            let start = idx + 1;
            let mut end = start;
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let key: String = chars[start..end].iter().collect();
            let value = lookup_record_field(record, &key, message, asctime);
            out.push_str(&value);
            idx = end;
        } else {
            out.push(chars[idx]);
            idx += 1;
        }
    }
    out
}

fn lookup_record_field(record: &LogRecordState, key: &str, message: &str, asctime: &str) -> String {
    match key {
        "name" => record.name.clone(),
        "msg" => record.msg.clone(),
        "args" => record.args.clone(),
        "levelno" => record.level.to_string(),
        "levelname" => record.levelname.clone(),
        "pathname" => record.pathname.clone(),
        "filename" => record.filename.clone(),
        "module" => record.module.clone(),
        "lineno" => record.lineno.to_string(),
        "funcName" => record.func_name.clone(),
        "created" => format!("{:.6}", record.created),
        "msecs" => format!("{:.6}", record.msecs),
        "relativeCreated" => format!("{:.6}", record.relative_created),
        "thread" => record.thread.to_string(),
        "threadName" => record.thread_name.clone(),
        "process" => record.process.to_string(),
        "processName" => record.process_name.clone(),
        "message" => message.to_string(),
        "asctime" => asctime.to_string(),
        "exc_info" => record.exc_info.clone(),
        "exc_text" => record.exc_text.as_deref().unwrap_or("").to_string(),
        "stack_info" => record.stack_info.as_deref().unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// Format a UNIX timestamp into a strftime-style string.
///
/// We implement a subset of strftime directives in pure Rust so the runtime
/// does not depend on libc `strftime`.  This covers the directives that
/// CPython's logging module uses by default.
fn format_time_strftime(timestamp: f64, fmt: &str) -> String {
    let secs = timestamp as i64;

    // Decompose into calendar components (UTC-like, matching CPython localtime
    // when TZ is unset — acceptable for Molt's deterministic model).
    let (year, month, day, hour, minute, second) = unix_to_calendar(secs);

    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut idx = 0;
    while idx < chars.len() {
        if chars[idx] == '%' && idx + 1 < chars.len() {
            let spec = chars[idx + 1];
            match spec {
                'Y' => out.push_str(&format!("{year:04}")),
                'm' => out.push_str(&format!("{month:02}")),
                'd' => out.push_str(&format!("{day:02}")),
                'H' => out.push_str(&format!("{hour:02}")),
                'M' => out.push_str(&format!("{minute:02}")),
                'S' => out.push_str(&format!("{second:02}")),
                '%' => out.push('%'),
                'I' => {
                    let h12 = if hour == 0 {
                        12
                    } else if hour > 12 {
                        hour - 12
                    } else {
                        hour
                    };
                    out.push_str(&format!("{h12:02}"));
                }
                'p' => {
                    if hour < 12 {
                        out.push_str("AM");
                    } else {
                        out.push_str("PM");
                    }
                }
                'j' => {
                    let yday = day_of_year(year, month, day);
                    out.push_str(&format!("{yday:03}"));
                }
                _ => {
                    out.push('%');
                    out.push(spec);
                }
            }
            idx += 2;
        } else {
            out.push(chars[idx]);
            idx += 1;
        }
    }
    out
}

/// Convert a UNIX timestamp (seconds since epoch) to calendar components.
/// Returns (year, month, day, hour, minute, second).
fn unix_to_calendar(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let day_secs = secs.rem_euclid(86400);
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    // Days since epoch (1970-01-01).
    let mut days = secs.div_euclid(86400);

    // Algorithm from Howard Hinnant's chrono-compatible date library.
    days += 719468; // shift epoch from 1970-01-01 to 0000-03-01
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    (y, m as i64, d as i64, hour, minute, second)
}

/// Day of year (1-indexed).
fn day_of_year(year: i64, month: i64, day: i64) -> i64 {
    let days_before = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let m = (month - 1) as usize;
    let mut doy = if m < 12 { days_before[m] } else { 0 } + day;
    if month > 2 && is_leap(year) {
        doy += 1;
    }
    doy
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

// ── Formatter intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_formatter_new(
    fmt_bits: u64,
    datefmt_bits: u64,
    style_bits: u64,
    _validate_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let fmt_str = opt_str(fmt_bits);
        let datefmt = opt_str(datefmt_bits);
        let style_str = opt_str(style_bits).unwrap_or_else(|| "%".to_string());

        let style = match style_str.as_str() {
            "%" => FormatStyle::Percent,
            "{" => FormatStyle::StrFormat,
            "$" => FormatStyle::Dollar,
            _ => {
                return raise_exception::<u64>(_py, "ValueError", "Style must be one of: %, {, $");
            }
        };

        let default_fmt = match style {
            FormatStyle::Percent => "%(message)s",
            FormatStyle::StrFormat => "{message}",
            FormatStyle::Dollar => "${message}",
        };

        let formatter = FormatterState {
            fmt: fmt_str.unwrap_or_else(|| default_fmt.to_string()),
            datefmt,
            style,
            default_time_format: "%Y-%m-%d %H:%M:%S".to_string(),
            default_msec_format: "%s,%03d".to_string(),
        };

        let id = next_formatter_handle();
        FORMATTER_REGISTRY.lock().unwrap().insert(id, formatter);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_formatter_format(formatter_bits: u64, record_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(fmt_id) = handle_from_bits(_py, formatter_bits, "Formatter") else {
            return MoltObject::none().bits();
        };
        let Some(rec_id) = handle_from_bits(_py, record_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let formatted = {
            let fmt_reg = FORMATTER_REGISTRY.lock().unwrap();
            let Some(formatter) = fmt_reg.get(&fmt_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Formatter handle");
            };
            let mut rec_reg = RECORD_REGISTRY.lock().unwrap();
            let Some(record) = rec_reg.get_mut(&rec_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
            };
            // Update cached message and asctime on the record, matching CPython.
            record.message = Some(record.get_message());
            if formatter.uses_time() {
                record.asctime = Some(formatter.format_time(record));
            }
            formatter.format_record(record)
        };
        return_str(_py, &formatted)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_formatter_format_time(
    formatter_bits: u64,
    record_bits: u64,
    datefmt_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(fmt_id) = handle_from_bits(_py, formatter_bits, "Formatter") else {
            return MoltObject::none().bits();
        };
        let Some(rec_id) = handle_from_bits(_py, record_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let override_datefmt = opt_str(datefmt_bits);
        let time_str = {
            let fmt_reg = FORMATTER_REGISTRY.lock().unwrap();
            let Some(formatter) = fmt_reg.get(&fmt_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Formatter handle");
            };
            let rec_reg = RECORD_REGISTRY.lock().unwrap();
            let Some(record) = rec_reg.get(&rec_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
            };
            if let Some(ref df) = override_datefmt {
                format_time_strftime(record.created, df)
            } else {
                formatter.format_time(record)
            }
        };
        return_str(_py, &time_str)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_formatter_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "Formatter") else {
            return MoltObject::none().bits();
        };
        FORMATTER_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}

// ── Handler ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum HandlerKind {
    Base,
    Stream,
}

struct HandlerState {
    kind: HandlerKind,
    level: i64,
    formatter_handle: Option<i64>,
    /// For base handler, emitted records are buffered here as formatted strings.
    buffer: Vec<String>,
    /// For StreamHandler: 0 = stderr (default), 1 = stdout, or raw stream bits.
    stream_target: StreamTarget,
    closed: bool,
}

#[derive(Clone, Copy)]
enum StreamTarget {
    Stderr,
    Stdout,
    /// Raw stream object bits from the Python side.
    Custom(u64),
}

static HANDLER_REGISTRY: LazyLock<Mutex<HashMap<i64, HandlerState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl HandlerState {
    fn new_base(level: i64) -> Self {
        Self {
            kind: HandlerKind::Base,
            level,
            formatter_handle: None,
            buffer: Vec::new(),
            stream_target: StreamTarget::Stderr,
            closed: false,
        }
    }

    fn new_stream(stream_target: StreamTarget, level: i64) -> Self {
        Self {
            kind: HandlerKind::Stream,
            level,
            formatter_handle: None,
            buffer: Vec::new(),
            stream_target,
            closed: false,
        }
    }

    fn format_record(&self, record: &mut LogRecordState) -> String {
        if let Some(fmt_id) = self.formatter_handle {
            let fmt_reg = FORMATTER_REGISTRY.lock().unwrap();
            if let Some(formatter) = fmt_reg.get(&fmt_id) {
                record.message = Some(record.get_message());
                if formatter.uses_time() {
                    record.asctime = Some(formatter.format_time(record));
                }
                return formatter.format_record(record);
            }
        }
        // Fallback: use BASIC_FORMAT = "%(levelname)s:%(name)s:%(message)s"
        let message = record.get_message();
        format!("{}:{}:{}", record.levelname, record.name, message)
    }
}

// ── Handler intrinsics ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_new(level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let handler = HandlerState::new_base(level);
        let id = next_handler_handle();
        HANDLER_REGISTRY.lock().unwrap().insert(id, handler);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_emit(handler_bits: u64, record_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let Some(r_id) = handle_from_bits(_py, record_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let formatted = {
            let mut rec_reg = RECORD_REGISTRY.lock().unwrap();
            let Some(record) = rec_reg.get_mut(&r_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
            };
            let handler_reg = HANDLER_REGISTRY.lock().unwrap();
            let Some(handler) = handler_reg.get(&h_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Handler handle");
            };
            handler.format_record(record)
        };
        // Store in handler's buffer
        let mut handler_reg = HANDLER_REGISTRY.lock().unwrap();
        if let Some(handler) = handler_reg.get_mut(&h_id) {
            handler.buffer.push(formatted);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_set_level(handler_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let mut registry = HANDLER_REGISTRY.lock().unwrap();
        if let Some(handler) = registry.get_mut(&h_id) {
            handler.level = level;
        } else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Handler handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_set_formatter(
    handler_bits: u64,
    formatter_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let fmt_id = if obj_from_bits(formatter_bits).is_none() {
            None
        } else {
            handle_from_bits(_py, formatter_bits, "Formatter")
        };
        let mut registry = HANDLER_REGISTRY.lock().unwrap();
        if let Some(handler) = registry.get_mut(&h_id) {
            handler.formatter_handle = fmt_id;
        } else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Handler handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_flush(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        // Flush buffered messages to stderr for base handlers.
        let messages: Vec<String> = {
            let mut registry = HANDLER_REGISTRY.lock().unwrap();
            let Some(handler) = registry.get_mut(&h_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Handler handle");
            };

            handler.buffer.drain(..).collect()
        };
        for msg in &messages {
            eprintln!("{msg}");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_close(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = HANDLER_REGISTRY.lock().unwrap();
        if let Some(handler) = registry.get_mut(&h_id) {
            handler.closed = true;
            handler.buffer.clear();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_drop(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        HANDLER_REGISTRY.lock().unwrap().remove(&h_id);
        MoltObject::none().bits()
    })
}

// ── StreamHandler intrinsics ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_stream_handler_new(stream_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let stream_target = if obj_from_bits(stream_bits).is_none() {
            StreamTarget::Stderr
        } else {
            // Check if stream_bits refers to a known name.
            if let Some(name) = string_obj_to_owned(obj_from_bits(stream_bits)) {
                match name.as_str() {
                    "stderr" | "<stderr>" => StreamTarget::Stderr,
                    "stdout" | "<stdout>" => StreamTarget::Stdout,
                    _ => StreamTarget::Custom(stream_bits),
                }
            } else {
                StreamTarget::Custom(stream_bits)
            }
        };
        let handler = HandlerState::new_stream(stream_target, level);
        let id = next_handler_handle();
        HANDLER_REGISTRY.lock().unwrap().insert(id, handler);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_stream_handler_emit(handler_bits: u64, record_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "StreamHandler") else {
            return MoltObject::none().bits();
        };
        let Some(r_id) = handle_from_bits(_py, record_bits, "LogRecord") else {
            return MoltObject::none().bits();
        };
        let (formatted, stream_target) = {
            let mut rec_reg = RECORD_REGISTRY.lock().unwrap();
            let Some(record) = rec_reg.get_mut(&r_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid LogRecord handle");
            };
            let handler_reg = HANDLER_REGISTRY.lock().unwrap();
            let Some(handler) = handler_reg.get(&h_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid StreamHandler handle");
            };
            if handler.closed {
                return MoltObject::none().bits();
            }
            let formatted = handler.format_record(record);
            (formatted, handler.stream_target)
        };

        // Write to the target stream.
        match stream_target {
            StreamTarget::Stderr => {
                eprintln!("{formatted}");
            }
            StreamTarget::Stdout => {
                println!("{formatted}");
            }
            StreamTarget::Custom(stream_obj_bits) => {
                // Write the formatted message + newline to the stream object
                // by looking up its .write() method and calling it.
                let msg_with_newline = format!("{formatted}\n");
                let msg_ptr = alloc_string(_py, msg_with_newline.as_bytes());
                if !msg_ptr.is_null() {
                    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
                    if let Some(stream_ptr) = obj_from_bits(stream_obj_bits).as_ptr() {
                        unsafe {
                            let write_name = crate::bridge::intern_static_name(_py, b"write");
                            if let Some(write_fn) =
                                crate::bridge::attr_lookup_ptr_allow_missing(_py, stream_ptr, write_name)
                            {
                                let result = crate::bridge::call_callable1(_py, write_fn, msg_bits);
                                if !obj_from_bits(result).is_none() {
                                    dec_ref_bits(_py, result);
                                }
                                dec_ref_bits(_py, write_fn);
                                if crate::bridge::exception_pending(_py) {
                                    crate::bridge::clear_exception(_py);
                                }
                            }
                        }
                    }
                    dec_ref_bits(_py, msg_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

// ── Logger ───────────────────────────────────────────────────────────────────

struct LoggerState {
    name: String,
    level: i64,
    handlers: Vec<i64>,
    propagate: bool,
    disabled: bool,
    parent: Option<i64>,
}

static LOGGER_REGISTRY: LazyLock<Mutex<HashMap<i64, LoggerState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl LoggerState {
    fn new(name: String, level: i64) -> Self {
        Self {
            name,
            level,
            handlers: Vec::new(),
            propagate: true,
            disabled: false,
            parent: None,
        }
    }

    fn get_effective_level(&self, registry: &HashMap<i64, LoggerState>) -> i64 {
        if self.level != NOTSET {
            return self.level;
        }
        if let Some(parent_id) = self.parent
            && let Some(parent) = registry.get(&parent_id)
        {
            return parent.get_effective_level(registry);
        }
        NOTSET
    }

    fn is_enabled_for(&self, level: i64, registry: &HashMap<i64, LoggerState>) -> bool {
        if self.disabled {
            return false;
        }
        level >= self.get_effective_level(registry)
    }
}

// ── Logger intrinsics ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_new(name_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "Logger name must be str");
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let logger = LoggerState::new(name, level);
        let id = next_logger_handle();
        LOGGER_REGISTRY.lock().unwrap().insert(id, logger);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_set_level(handle_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let mut registry = LOGGER_REGISTRY.lock().unwrap();
        if let Some(logger) = registry.get_mut(&id) {
            logger.level = level;
        } else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_add_handler(logger_bits: u64, handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let Some(handler_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = LOGGER_REGISTRY.lock().unwrap();
        if let Some(logger) = registry.get_mut(&logger_id) {
            if !logger.handlers.contains(&handler_id) {
                logger.handlers.push(handler_id);
            }
        } else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_remove_handler(logger_bits: u64, handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let Some(handler_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = LOGGER_REGISTRY.lock().unwrap();
        if let Some(logger) = registry.get_mut(&logger_id) {
            logger.handlers.retain(|&h| h != handler_id);
        } else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_log(
    logger_bits: u64,
    level_bits: u64,
    msg_bits: u64,
    args_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let msg = opt_str(msg_bits).unwrap_or_default();
        let args = opt_str(args_bits).unwrap_or_default();

        // Check if enabled, collect handler IDs, and get logger name.
        let (handler_ids, logger_name) = {
            let registry = LOGGER_REGISTRY.lock().unwrap();
            let Some(logger) = registry.get(&logger_id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Logger handle");
            };
            if !logger.is_enabled_for(level, &registry) {
                return MoltObject::none().bits();
            }
            // Collect handler IDs from this logger and its parents (propagation).
            let mut handler_ids: Vec<i64> = Vec::new();
            let mut current_id = Some(logger_id);
            while let Some(cid) = current_id {
                if let Some(lgr) = registry.get(&cid) {
                    for &h_id in &lgr.handlers {
                        handler_ids.push(h_id);
                    }
                    if lgr.propagate {
                        current_id = lgr.parent;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            (handler_ids, logger.name.clone())
        };

        if handler_ids.is_empty() {
            return MoltObject::none().bits();
        }

        // Create a temporary record.
        let levelname = LEVEL_NAMES.lock().unwrap().get_name(level);
        let created = current_time_secs();
        let msecs = (created - created.floor()) * 1000.0;
        let start = *START_TIME;
        let relative_created = (created - start) * 1000.0;

        let record = LogRecordState {
            name: logger_name,
            level,
            levelname,
            pathname: String::new(),
            filename: String::new(),
            module: String::new(),
            lineno: 0,
            msg,
            args,
            exc_info: String::new(),
            func_name: String::new(),
            created,
            msecs,
            relative_created,
            thread: 0,
            thread_name: "MainThread".to_string(),
            process: std::process::id() as i64,
            process_name: "MainProcess".to_string(),
            message: None,
            asctime: None,
            exc_text: None,
            stack_info: None,
        };

        let rec_id = next_record_handle();
        RECORD_REGISTRY.lock().unwrap().insert(rec_id, record);

        // Emit to all handlers.
        for h_id in &handler_ids {
            let (formatted, stream_target, is_stream) = {
                let mut rec_reg = RECORD_REGISTRY.lock().unwrap();
                let Some(record) = rec_reg.get_mut(&rec_id) else {
                    continue;
                };
                let handler_reg = HANDLER_REGISTRY.lock().unwrap();
                let Some(handler) = handler_reg.get(h_id) else {
                    continue;
                };
                if record.level < handler.level {
                    continue;
                }
                if handler.closed {
                    continue;
                }
                let formatted = handler.format_record(record);
                (
                    formatted,
                    handler.stream_target,
                    handler.kind == HandlerKind::Stream,
                )
            };

            if is_stream {
                match stream_target {
                    StreamTarget::Stderr => eprintln!("{formatted}"),
                    StreamTarget::Stdout => println!("{formatted}"),
                    StreamTarget::Custom(stream_obj_bits) => {
                        let msg_with_newline = format!("{formatted}\n");
                        let msg_ptr = alloc_string(_py, msg_with_newline.as_bytes());
                        if !msg_ptr.is_null() {
                            let msg_bits_val = MoltObject::from_ptr(msg_ptr).bits();
                            if let Some(stream_ptr) = obj_from_bits(stream_obj_bits).as_ptr() {
                                unsafe {
                                    let write_name = crate::bridge::intern_static_name(_py, b"write");
                                    if let Some(write_fn) = crate::bridge::attr_lookup_ptr_allow_missing(
                                        _py, stream_ptr, write_name,
                                    ) {
                                        let result =
                                            crate::bridge::call_callable1(_py, write_fn, msg_bits_val);
                                        if !obj_from_bits(result).is_none() {
                                            dec_ref_bits(_py, result);
                                        }
                                        dec_ref_bits(_py, write_fn);
                                        if crate::bridge::exception_pending(_py) {
                                            crate::bridge::clear_exception(_py);
                                        }
                                    }
                                }
                            }
                            dec_ref_bits(_py, msg_bits_val);
                        }
                    }
                }
            } else {
                // Base handler: buffer
                let mut handler_reg = HANDLER_REGISTRY.lock().unwrap();
                if let Some(handler) = handler_reg.get_mut(h_id) {
                    handler.buffer.push(formatted);
                }
            }
        }

        // Clean up the temporary record.
        RECORD_REGISTRY.lock().unwrap().remove(&rec_id);

        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_is_enabled_for(logger_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::from_bool(false).bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let registry = LOGGER_REGISTRY.lock().unwrap();
        let Some(logger) = registry.get(&logger_id) else {
            return MoltObject::from_bool(false).bits();
        };
        let enabled = logger.is_enabled_for(level, &registry);
        MoltObject::from_bool(enabled).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_get_effective_level(logger_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::from_int(NOTSET).bits();
        };
        let registry = LOGGER_REGISTRY.lock().unwrap();
        let Some(logger) = registry.get(&logger_id) else {
            return MoltObject::from_int(NOTSET).bits();
        };
        let effective = logger.get_effective_level(&registry);
        MoltObject::from_int(effective).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        LOGGER_REGISTRY.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}

// ── Manager / root logger ────────────────────────────────────────────────────

/// The root logger handle.  Created lazily on first access.
static ROOT_LOGGER_HANDLE: LazyLock<Mutex<Option<i64>>> = LazyLock::new(|| Mutex::new(None));

/// Named logger cache: name -> handle.
static LOGGER_CACHE: LazyLock<Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn ensure_root_logger() -> i64 {
    let mut root_opt = ROOT_LOGGER_HANDLE.lock().unwrap();
    if let Some(id) = *root_opt {
        return id;
    }
    let logger = LoggerState::new("root".to_string(), WARNING);
    let id = next_logger_handle();
    LOGGER_REGISTRY.lock().unwrap().insert(id, logger);
    *root_opt = Some(id);
    id
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_manager_get_logger(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "logger name must be str");
        };
        if name.is_empty() || name == "root" {
            let root_id = ensure_root_logger();
            return MoltObject::from_int(root_id).bits();
        }
        let mut cache = LOGGER_CACHE.lock().unwrap();
        if let Some(&id) = cache.get(&name) {
            return MoltObject::from_int(id).bits();
        }
        // Create a new logger with parent linkage.
        let root_id = ensure_root_logger();
        let mut parent_id = root_id;

        // Walk up the dot-separated hierarchy to find the nearest existing parent.
        let mut dot_pos = name.len();
        while let Some(pos) = name[..dot_pos].rfind('.') {
            let parent_name = &name[..pos];
            if let Some(&pid) = cache.get(parent_name) {
                parent_id = pid;
                break;
            }
            dot_pos = pos;
        }

        let mut logger = LoggerState::new(name.clone(), NOTSET);
        logger.parent = Some(parent_id);
        let id = next_logger_handle();
        LOGGER_REGISTRY.lock().unwrap().insert(id, logger);
        cache.insert(name, id);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_root_logger() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let root_id = ensure_root_logger();
        MoltObject::from_int(root_id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_basic_config(
    level_bits: u64,
    format_bits: u64,
    datefmt_bits: u64,
    stream_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let root_id = ensure_root_logger();

        // Check if root already has handlers — if so, basicConfig is a no-op
        // (matching CPython behavior).
        {
            let registry = LOGGER_REGISTRY.lock().unwrap();
            if let Some(logger) = registry.get(&root_id)
                && !logger.handlers.is_empty()
            {
                return MoltObject::none().bits();
            }
        }

        // Create a StreamHandler.
        let stream_target = if obj_from_bits(stream_bits).is_none() {
            StreamTarget::Stderr
        } else if let Some(name) = string_obj_to_owned(obj_from_bits(stream_bits)) {
            match name.as_str() {
                "stderr" | "<stderr>" => StreamTarget::Stderr,
                "stdout" | "<stdout>" => StreamTarget::Stdout,
                _ => StreamTarget::Custom(stream_bits),
            }
        } else {
            StreamTarget::Custom(stream_bits)
        };

        let handler = HandlerState::new_stream(stream_target, NOTSET);
        let handler_id = next_handler_handle();
        HANDLER_REGISTRY.lock().unwrap().insert(handler_id, handler);

        // Create a Formatter if format or datefmt was provided.
        let fmt_str = opt_str(format_bits);
        let datefmt = opt_str(datefmt_bits);
        if fmt_str.is_some() || datefmt.is_some() {
            let formatter = FormatterState {
                fmt: fmt_str.unwrap_or_else(|| "%(levelname)s:%(name)s:%(message)s".to_string()),
                datefmt,
                style: FormatStyle::Percent,
                default_time_format: "%Y-%m-%d %H:%M:%S".to_string(),
                default_msec_format: "%s,%03d".to_string(),
            };
            let fmt_id = next_formatter_handle();
            FORMATTER_REGISTRY.lock().unwrap().insert(fmt_id, formatter);
            let mut h_reg = HANDLER_REGISTRY.lock().unwrap();
            if let Some(h) = h_reg.get_mut(&handler_id) {
                h.formatter_handle = Some(fmt_id);
            }
        }

        // Add handler to root logger.
        let mut registry = LOGGER_REGISTRY.lock().unwrap();
        if let Some(logger) = registry.get_mut(&root_id) {
            logger.handlers.push(handler_id);
            // Set level if provided.
            if !obj_from_bits(level_bits).is_none() {
                let level = to_i64(obj_from_bits(level_bits)).unwrap_or(WARNING);
                logger.level = level;
            }
        }

        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_shutdown() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        // Flush and close all handlers.
        let handler_ids: Vec<i64> = {
            let registry = HANDLER_REGISTRY.lock().unwrap();
            registry.keys().copied().collect()
        };
        for h_id in handler_ids {
            // Flush buffered messages.
            let messages: Vec<String> = {
                let mut registry = HANDLER_REGISTRY.lock().unwrap();
                if let Some(handler) = registry.get_mut(&h_id) {
                    let msgs: Vec<String> = handler.buffer.drain(..).collect();
                    handler.closed = true;
                    msgs
                } else {
                    Vec::new()
                }
            };
            for msg in &messages {
                eprintln!("{msg}");
            }
        }
        MoltObject::none().bits()
    })
}

// ── Level utilities ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_get_level_name(level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "level must be an int");
        };
        let name = LEVEL_NAMES.lock().unwrap().get_name(level);
        return_str(_py, &name)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_add_level_name(level_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "level must be an int");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "level name must be str");
        };
        LEVEL_NAMES.lock().unwrap().add(level, name);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_level_to_int(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "level name must be str");
        };
        let level_names = LEVEL_NAMES.lock().unwrap();
        if let Some(level) = level_names.get_level(&name) {
            MoltObject::from_int(level).bits()
        } else {
            // CPython returns the string itself when unknown; in the intrinsic
            // layer we raise ValueError for type safety.
            let msg = format!("Unknown level: '{name}'");
            raise_exception::<u64>(_py, "ValueError", &msg)
        }
    })
}
