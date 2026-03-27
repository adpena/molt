#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/configparser.rs ===
//! Intrinsics for the `configparser` stdlib module.
//!
//! Implements ConfigParser / RawConfigParser semantics:
//!   - INI file format: `[section]\nkey = value\n` and `key: value\n`
//!   - Comments: lines beginning with `#` or `;` (also inline after whitespace)
//!   - Case-insensitive option keys (folded to lower-case, CPython default)
//!   - `%(key)s` interpolation (BasicInterpolation; disabled via `interpolation=None`)
//!   - DEFAULT section folded into every section for fallback
//!   - `getint`, `getfloat`, `getboolean` type-coercion helpers
//!   - `write` serialises back to an INI-formatted file

use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ---------------------------------------------------------------------------
// Handle counter
// ---------------------------------------------------------------------------

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// ConfigParser data model
// ---------------------------------------------------------------------------

/// Whether basic `%(key)s` interpolation is enabled.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Interpolation {
    Basic,
    None,
}

/// Ordered map of option_key → value for one section.
type SectionMap = HashMap<String, String>;

struct ConfigState {
    interpolation: Interpolation,
    /// DEFAULT section values (folded into all sections on read).
    defaults: SectionMap,
    /// Ordered section list (preserves insertion order).
    section_order: Vec<String>,
    /// section_name → options (not including DEFAULT).
    sections: HashMap<String, SectionMap>,
}

impl ConfigState {
    fn new(defaults: SectionMap, interpolation: Interpolation) -> Self {
        Self {
            interpolation,
            defaults,
            section_order: Vec::new(),
            sections: HashMap::new(),
        }
    }

    /// Get an option from `section`, applying DEFAULT fallback and interpolation.
    fn get(&self, section: &str, option: &str, fallback: Option<&str>) -> Option<String> {
        let key = option.to_ascii_lowercase();
        let sec = self.sections.get(section)?;
        let raw_val: Option<&str> = sec
            .get(&key)
            .map(|s| s.as_str())
            .or_else(|| self.defaults.get(&key).map(|s| s.as_str()));
        let val: &str = raw_val.or(fallback)?;
        if self.interpolation == Interpolation::Basic {
            Some(self.interpolate(val.to_string(), section))
        } else {
            Some(val.to_string())
        }
    }

    /// Apply %(key)s substitution within `value` using `section` context.
    fn interpolate(&self, mut value: String, section: &str) -> String {
        // Simple iterative substitution with a depth guard.
        for _ in 0..10 {
            if !value.contains("%(") {
                break;
            }
            let mut result = String::with_capacity(value.len());
            let mut i = 0usize;
            let chars: Vec<char> = value.chars().collect();
            let len = chars.len();
            while i < len {
                if chars[i] == '%' && i + 1 < len && chars[i + 1] == '(' {
                    let start = i + 2;
                    let mut j = start;
                    while j < len && chars[j] != ')' {
                        j += 1;
                    }
                    if j + 1 < len && chars[j] == ')' && chars[j + 1] == 's' {
                        let key_str: String = chars[start..j].iter().collect();
                        let key = key_str.to_ascii_lowercase();
                        let resolved = self
                            .sections
                            .get(section)
                            .and_then(|s| s.get(&key))
                            .or_else(|| self.defaults.get(&key))
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        result.push_str(resolved);
                        i = j + 2;
                        continue;
                    }
                }
                result.push(chars[i]);
                i += 1;
            }
            if result == value {
                break;
            }
            value = result;
        }
        value
    }

    /// Parse an INI-formatted string and merge into self.
    fn read_string(&mut self, text: &str) {
        let mut current_section: Option<String> = None;
        let mut current_key: Option<String> = None;
        let mut current_val: String = String::new();

        let flush = |current_section: &Option<String>,
                     current_key: &mut Option<String>,
                     current_val: &mut String,
                     sections: &mut HashMap<String, SectionMap>,
                     defaults: &mut SectionMap| {
            if let Some(key) = current_key.take() {
                let val = std::mem::take(current_val);
                if let Some(sec) = current_section {
                    sections.entry(sec.clone()).or_default().insert(key, val);
                } else {
                    defaults.insert(key, val);
                }
            }
        };

        for raw_line in text.lines() {
            // Strip inline comments after whitespace: we look for ' #' or ' ;'.
            let line = strip_inline_comment(raw_line);
            let line = line.trim();

            // Blank line terminates multi-line values.
            if line.is_empty() {
                flush(
                    &current_section,
                    &mut current_key,
                    &mut current_val,
                    &mut self.sections,
                    &mut self.defaults,
                );
                continue;
            }

            // Full-line comment.
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            // Section header.
            if line.starts_with('[') {
                flush(
                    &current_section,
                    &mut current_key,
                    &mut current_val,
                    &mut self.sections,
                    &mut self.defaults,
                );
                if let Some(end) = line.find(']') {
                    let sec_name = line[1..end].trim().to_string();
                    if sec_name.eq_ignore_ascii_case("DEFAULT") {
                        current_section = None; // writes go to defaults
                    } else {
                        if !self.sections.contains_key(&sec_name) {
                            self.sections.insert(sec_name.clone(), SectionMap::new());
                            self.section_order.push(sec_name.clone());
                        }
                        current_section = Some(sec_name);
                    }
                }
                continue;
            }

            // Continuation line (starts with whitespace in raw line).
            if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
                if current_key.is_some() {
                    current_val.push('\n');
                    current_val.push_str(line);
                }
                continue;
            }

            // Key = value or Key: value
            flush(
                &current_section,
                &mut current_key,
                &mut current_val,
                &mut self.sections,
                &mut self.defaults,
            );

            let (key_raw, val_raw) = split_key_value(line);
            let key = key_raw.trim().to_ascii_lowercase();
            let val = val_raw.trim().to_string();
            current_key = Some(key);
            current_val = val;
        }

        flush(
            &current_section,
            &mut current_key,
            &mut current_val,
            &mut self.sections,
            &mut self.defaults,
        );
    }

    /// Serialise back to INI format.
    fn write(&self) -> String {
        let mut out = String::new();
        // Write DEFAULT section first if non-empty.
        if !self.defaults.is_empty() {
            let _ = writeln!(out, "[DEFAULT]");
            let mut keys: Vec<&String> = self.defaults.keys().collect();
            keys.sort();
            for k in keys {
                let _ = writeln!(out, "{k} = {}", self.defaults[k]);
            }
            let _ = writeln!(out);
        }
        for sec in &self.section_order {
            let _ = writeln!(out, "[{sec}]");
            if let Some(options) = self.sections.get(sec) {
                let mut keys: Vec<&String> = options.keys().collect();
                keys.sort();
                for k in keys {
                    let _ = writeln!(out, "{k} = {}", options[k]);
                }
            }
            let _ = writeln!(out);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// INI parsing helpers
// ---------------------------------------------------------------------------

/// Strip a trailing inline comment (` #...` or ` ;...` after whitespace).
fn strip_inline_comment(line: &str) -> &str {
    // Only strip if the comment marker is preceded by at least one space.
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;
    while i < len {
        let b = bytes[i];
        if !in_single && !in_double {
            if b == b'\'' {
                in_single = true;
            } else if b == b'"' {
                in_double = true;
            } else if (b == b'#' || b == b';') && i > 0 && bytes[i - 1] == b' ' {
                return &line[..i];
            }
        } else if in_single && b == b'\'' {
            in_single = false;
        } else if in_double && b == b'"' {
            in_double = false;
        }
        i += 1;
    }
    line
}

/// Split a key-value line on the first `=` or `:`.
fn split_key_value(line: &str) -> (&str, &str) {
    // Find first `=` or `:` that is not inside quotes (simple, not nested).
    for (idx, ch) in line.char_indices() {
        if ch == '=' || ch == ':' {
            return (&line[..idx], &line[idx + 1..]);
        }
    }
    (line, "")
}

// ---------------------------------------------------------------------------
// Process-wide handle registry
// ---------------------------------------------------------------------------

static CONFIG_REGISTRY: LazyLock<Mutex<HashMap<i64, ConfigState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Object helpers
// ---------------------------------------------------------------------------

fn alloc_str_or_err(_py: &PyToken, s: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn opt_str(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        None
    } else {
        string_obj_to_owned(obj)
    }
}

fn str_list_to_bits(_py: &PyToken, items: &[String]) -> u64 {
    let mut bits: Vec<u64> = Vec::with_capacity(items.len());
    for s in items {
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            for b in &bits {
                dec_ref_bits(_py, *b);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, &bits);
    for b in &bits {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        raise_exception::<u64>(_py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn string_pairs_to_list(_py: &PyToken, pairs: &[(String, String)]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        let k_ptr = alloc_string(_py, k.as_bytes());
        let v_ptr = alloc_string(_py, v.as_bytes());
        if k_ptr.is_null() || v_ptr.is_null() {
            for b in &item_bits {
                dec_ref_bits(_py, *b);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let tup = alloc_tuple(
            _py,
            &[
                MoltObject::from_ptr(k_ptr).bits(),
                MoltObject::from_ptr(v_ptr).bits(),
            ],
        );
        dec_ref_bits(_py, MoltObject::from_ptr(k_ptr).bits());
        dec_ref_bits(_py, MoltObject::from_ptr(v_ptr).bits());
        if tup.is_null() {
            for b in &item_bits {
                dec_ref_bits(_py, *b);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        item_bits.push(MoltObject::from_ptr(tup).bits());
    }
    let list_ptr = alloc_list(_py, &item_bits);
    for b in &item_bits {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        raise_exception::<u64>(_py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// Public FFI
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_new(defaults_bits: u64, interpolation_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // `defaults_bits` is a dict of str→str or None.
        let defaults = if obj_from_bits(defaults_bits).is_none() {
            SectionMap::new()
        } else {
            // Extract pairs from dict if provided; ignore non-string keys/values.
            let mut m = SectionMap::new();
            let obj = obj_from_bits(defaults_bits);
            if let Some(ptr) = obj.as_ptr() {
                unsafe {
                    if object_type_id(ptr) == TYPE_ID_DICT {
                        // dict_order returns &mut Vec<u64> of [k0, v0, k1, v1, ...].
                        let order = dict_order_clone(_py, ptr);
                        for pair in order.chunks(2) {
                            if pair.len() != 2 {
                                continue;
                            }
                            if let (Some(k), Some(v)) = (
                                string_obj_to_owned(obj_from_bits(pair[0])),
                                string_obj_to_owned(obj_from_bits(pair[1])),
                            ) {
                                m.insert(k.to_ascii_lowercase(), v);
                            }
                        }
                    }
                }
            }
            m
        };

        // `interpolation_bits` is None → Basic, "none" → None.
        let interp_str = opt_str(interpolation_bits);
        let interpolation = match interp_str.as_deref() {
            Some("none") | Some("None") => Interpolation::None,
            _ => Interpolation::Basic,
        };

        let id = next_handle_id();
        CONFIG_REGISTRY
            .lock()
            .unwrap()
            .insert(id, ConfigState::new(defaults, interpolation));
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_read(handle_bits: u64, filename_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "filename must be str");
        };

        // Read file from disk.
        let content = match std::fs::read_to_string(&filename) {
            Ok(c) => c,
            Err(_) => {
                // CPython silently ignores missing files; return empty list.
                let list_ptr = alloc_list(_py, &[]);
                if list_ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                }
                return MoltObject::from_ptr(list_ptr).bits();
            }
        };

        let ok = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            state.read_string(&content);
            true
        });
        if ok.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        }

        // Return [filename] — the list of successfully read files.
        let file_ptr = alloc_string(_py, filename.as_bytes());
        if file_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let file_bits = MoltObject::from_ptr(file_ptr).bits();
        let list_ptr = alloc_list(_py, &[file_bits]);
        dec_ref_bits(_py, file_bits);
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_read_string(handle_bits: u64, text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "text must be str");
        };
        let ok = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            state.read_string(&text);
        });
        if ok.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_sections(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let sections = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| state.section_order.to_vec());
        let Some(secs) = sections else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        str_list_to_bits(_py, &secs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_has_section(handle_bits: u64, section_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| state.sections.contains_key(&section));
        let Some(found) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_has_option(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let key = option.to_ascii_lowercase();
        let result = CONFIG_REGISTRY.lock().unwrap().get(&id).map(|state| {
            state
                .sections
                .get(&section)
                .is_some_and(|sec| sec.contains_key(&key) || state.defaults.contains_key(&key))
        });
        let Some(found) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_get(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
    fallback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        // Look up without fallback first; if missing, return fallback_bits
        // directly (avoids round-tripping fallback through string conversion).
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|state| state.get(&section, &option, None));
        match result {
            Some(val) => {
                let ptr = alloc_string(_py, val.as_bytes());
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            None => {
                if !obj_from_bits(fallback_bits).is_none() {
                    return fallback_bits;
                }
                let msg = format!("No option '{option}' in section '{section}'");
                raise_exception::<u64>(_py, "KeyError", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_getint(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
    fallback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|state| state.get(&section, &option, None));
        match result {
            Some(val) => match val.trim().parse::<i64>() {
                Ok(n) => MoltObject::from_int(n).bits(),
                Err(_) => raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("invalid literal for int(): {val}"),
                ),
            },
            None => {
                if !obj_from_bits(fallback_bits).is_none() {
                    return fallback_bits;
                }
                raise_exception::<u64>(
                    _py,
                    "KeyError",
                    &format!("No option '{option}' in section '{section}'"),
                )
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_getfloat(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
    fallback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|state| state.get(&section, &option, None));
        match result {
            Some(val) => match val.trim().parse::<f64>() {
                Ok(f) => MoltObject::from_float(f).bits(),
                Err(_) => raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("could not convert string to float: {val}"),
                ),
            },
            None => {
                if !obj_from_bits(fallback_bits).is_none() {
                    return fallback_bits;
                }
                raise_exception::<u64>(
                    _py,
                    "KeyError",
                    &format!("No option '{option}' in section '{section}'"),
                )
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_getboolean(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
    fallback_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|state| state.get(&section, &option, None));

        fn parse_bool(s: &str) -> Option<bool> {
            match s.trim().to_ascii_lowercase().as_str() {
                "1" | "yes" | "true" | "on" => Some(true),
                "0" | "no" | "false" | "off" => Some(false),
                _ => None,
            }
        }

        match result {
            Some(val) => match parse_bool(&val) {
                Some(b) => MoltObject::from_bool(b).bits(),
                None => raise_exception::<u64>(_py, "ValueError", &format!("not a boolean: {val}")),
            },
            None => {
                if !obj_from_bits(fallback_bits).is_none() {
                    return fallback_bits;
                }
                raise_exception::<u64>(
                    _py,
                    "KeyError",
                    &format!("No option '{option}' in section '{section}'"),
                )
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_set(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let value = string_obj_to_owned(obj_from_bits(value_bits)).unwrap_or_default();
        let key = option.to_ascii_lowercase();

        let ok = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            if section.eq_ignore_ascii_case("DEFAULT") {
                state.defaults.insert(key, value);
            } else if let Some(sec) = state.sections.get_mut(&section) {
                sec.insert(key, value);
            } else {
                return false;
            }
            true
        });
        match ok {
            None => raise_exception::<u64>(_py, "ValueError", "configparser handle not found"),
            Some(false) => {
                raise_exception::<u64>(_py, "KeyError", &format!("No section: '{section}'"))
            }
            Some(true) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_add_section(handle_bits: u64, section_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        if section.eq_ignore_ascii_case("DEFAULT") {
            return raise_exception::<u64>(_py, "ValueError", "Invalid section name: 'DEFAULT'");
        }
        let ok = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            if state.sections.contains_key(&section) {
                false // DuplicateSectionError
            } else {
                state.sections.insert(section.clone(), SectionMap::new());
                state.section_order.push(section);
                true
            }
        });
        match ok {
            None => raise_exception::<u64>(_py, "ValueError", "configparser handle not found"),
            Some(false) => raise_exception::<u64>(_py, "ValueError", "Section already exists"),
            Some(true) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_remove_section(handle_bits: u64, section_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let result = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            let removed = state.sections.remove(&section).is_some();
            if removed {
                state.section_order.retain(|s| s != &section);
            }
            removed
        });
        let Some(removed) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        MoltObject::from_bool(removed).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_remove_option(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let key = option.to_ascii_lowercase();
        let result = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            state
                .sections
                .get_mut(&section)
                .map(|sec| sec.remove(&key).is_some())
        });
        match result {
            None => raise_exception::<u64>(_py, "ValueError", "configparser handle not found"),
            Some(None) => {
                raise_exception::<u64>(_py, "KeyError", &format!("No section: '{section}'"))
            }
            Some(Some(removed)) => MoltObject::from_bool(removed).bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_options(handle_bits: u64, section_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let result = CONFIG_REGISTRY.lock().unwrap().get(&id).map(|state| {
            let sec_keys: Vec<String> = state
                .sections
                .get(&section)
                .map(|s| s.keys().cloned().collect())
                .unwrap_or_default();
            // Merge DEFAULT keys
            let mut all_keys: std::collections::HashSet<String> = sec_keys.into_iter().collect();
            for k in state.defaults.keys() {
                all_keys.insert(k.clone());
            }
            let mut keys: Vec<String> = all_keys.into_iter().collect();
            keys.sort();
            keys
        });
        let Some(keys) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        str_list_to_bits(_py, &keys)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_items(handle_bits: u64, section_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let result = CONFIG_REGISTRY.lock().unwrap().get(&id).and_then(|state| {
            state.sections.get(&section).map(|sec| {
                let mut pairs: Vec<(String, String)> =
                    sec.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                // Include DEFAULT fallbacks for keys not already present.
                for (dk, dv) in &state.defaults {
                    if !sec.contains_key(dk) {
                        pairs.push((dk.clone(), dv.clone()));
                    }
                }
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                pairs
            })
        });
        let Some(pairs) = result else {
            return raise_exception::<u64>(_py, "KeyError", &format!("No section: '{section}'"));
        };
        string_pairs_to_list(_py, &pairs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_write(handle_bits: u64, filename_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "filename must be str");
        };
        let content = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| state.write());
        let Some(content) = content else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        if let Err(e) = std::fs::write(&filename, content.as_bytes()) {
            return raise_exception::<u64>(_py, "OSError", &e.to_string());
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            CONFIG_REGISTRY.lock().unwrap().remove(&id);
        }
        MoltObject::none().bits()
    })
}

// ---------------------------------------------------------------------------
// New intrinsics for full intrinsic-backing of configparser.py
// ---------------------------------------------------------------------------

/// Serialize the config to an INI-format string and return it (instead of writing to file).
/// This allows the Python side to write to any file-like object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_write_string(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let content = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| state.write());
        let Some(content) = content else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        let ptr = alloc_string(_py, content.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Get a raw (uninterpolated) value for ConfigParser.get(raw=True).
/// Returns the raw string value, or None if not found.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_get_raw(
    handle_bits: u64,
    section_bits: u64,
    option_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(option) = string_obj_to_owned(obj_from_bits(option_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "option must be str");
        };
        let key = option.to_ascii_lowercase();
        let result = CONFIG_REGISTRY.lock().unwrap().get(&id).and_then(|state| {
            state
                .sections
                .get(&section)
                .and_then(|sec| sec.get(&key).or_else(|| state.defaults.get(&key)).cloned())
        });
        match result {
            Some(val) => {
                let ptr = alloc_string(_py, val.as_bytes());
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            None => MoltObject::none().bits(),
        }
    })
}

/// Perform basic %(key)s interpolation on a value string within a section context.
/// Returns the interpolated string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_interpolate_basic(
    handle_bits: u64,
    section_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "value must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| state.interpolate(value.clone(), &section));
        let Some(interpolated) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        let ptr = alloc_string(_py, interpolated.as_bytes());
        if ptr.is_null() {
            raise_exception::<u64>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Perform extended ${section:option} interpolation on a value string.
/// Returns the interpolated string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_interpolate_extended(
    handle_bits: u64,
    section_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(section) = string_obj_to_owned(obj_from_bits(section_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "section must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "value must be str");
        };
        let result = CONFIG_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|state| interpolate_extended(state, &value, &section));
        let Some(interpolated) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        let ptr = alloc_string(_py, interpolated.as_bytes());
        if ptr.is_null() {
            raise_exception::<u64>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Perform extended ${section:option} interpolation.
fn interpolate_extended(state: &ConfigState, value: &str, section: &str) -> String {
    let mut current = value.to_string();
    for _ in 0..10 {
        if !current.contains('$') {
            break;
        }
        let mut result = String::with_capacity(current.len());
        let chars: Vec<char> = current.chars().collect();
        let len = chars.len();
        let mut i = 0usize;
        while i < len {
            if chars[i] == '$' && i + 1 < len {
                let c = chars[i + 1];
                if c == '$' {
                    result.push('$');
                    i += 2;
                    continue;
                }
                if c == '{' {
                    // Find closing brace
                    let start = i + 2;
                    let mut j = start;
                    while j < len && chars[j] != '}' {
                        j += 1;
                    }
                    if j < len {
                        let path: String = chars[start..j].iter().collect();
                        i = j + 1;
                        let (sect, opt) = if let Some(colon_idx) = path.find(':') {
                            let s = path[..colon_idx].trim().to_string();
                            let o = path[colon_idx + 1..].trim().to_ascii_lowercase();
                            (s, o)
                        } else {
                            (section.to_string(), path.trim().to_ascii_lowercase())
                        };
                        // Look up the raw value
                        let resolved = state
                            .sections
                            .get(&sect)
                            .and_then(|sec| sec.get(&opt))
                            .or_else(|| state.defaults.get(&opt))
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        result.push_str(resolved);
                        continue;
                    }
                }
            }
            result.push(chars[i]);
            i += 1;
        }
        if result == current {
            break;
        }
        current = result;
    }
    current
}

/// Read from a file-like object (string content) into the configparser.
/// This wraps read_string but accepts an explicit source name for error messages.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_read_file(
    handle_bits: u64,
    content_bits: u64,
    source_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(content_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "content must be str");
        };
        let _source = string_obj_to_owned(obj_from_bits(source_bits));
        let ok = CONFIG_REGISTRY.lock().unwrap().get_mut(&id).map(|state| {
            state.read_string(&text);
        });
        if ok.is_none() {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        }
        MoltObject::none().bits()
    })
}

/// Get the defaults dict as a list of (key, value) pairs.
#[unsafe(no_mangle)]
pub extern "C" fn molt_configparser_defaults(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid configparser handle");
        };
        let result = CONFIG_REGISTRY.lock().unwrap().get(&id).map(|state| {
            let mut pairs: Vec<(String, String)> = state
                .defaults
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs
        });
        let Some(pairs) = result else {
            return raise_exception::<u64>(_py, "ValueError", "configparser handle not found");
        };
        string_pairs_to_list(_py, &pairs)
    })
}

// Suppress unused import warnings.
#[allow(dead_code)]
fn _suppress(_py: &PyToken, b: u64) {
    let _ = type_name(_py, obj_from_bits(b));
    let _ = is_truthy(_py, obj_from_bits(b));
    let _ = alloc_tuple(_py, &[]);
    let _ = alloc_dict_with_pairs(_py, &[]);
}
