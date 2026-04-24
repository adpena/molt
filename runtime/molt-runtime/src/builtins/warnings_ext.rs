use std::sync::{LazyLock, Mutex};

use crate::*;

// ─── Warning filter and registry types ──────────────────────────────────────

/// A single warning filter entry.
/// Fields mirror CPython's `warnings.filters` 5-tuple:
///   (action, message_pattern, category, module_pattern, lineno)
#[derive(Clone, Debug)]
struct WarningFilter {
    action: String,
    message: String,
    category: String,
    module: String,
    lineno: i64,
}

/// Key for the "once" registry: (message_text, category_name).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OnceKey {
    message: String,
    category: String,
}

/// Global warnings state. Protected by a Mutex for cross-thread safety.
/// The GIL serializes all Python-level access, so the Mutex is always
/// uncontended.
struct WarningsState {
    filters: Vec<WarningFilter>,
    once_registry: std::collections::HashSet<OnceKey>,
    default_action: String,
    filters_version: i64,
}

impl WarningsState {
    fn new() -> Self {
        Self {
            filters: Vec::new(),
            once_registry: std::collections::HashSet::new(),
            default_action: "default".to_string(),
            filters_version: 0,
        }
    }
}

static WARNINGS_STATE: LazyLock<Mutex<WarningsState>> =
    LazyLock::new(|| Mutex::new(WarningsState::new()));

// ─── Valid actions ──────────────────────────────────────────────────────────

const VALID_ACTIONS: &[&str] = &[
    "default", "error", "ignore", "always", "module", "once", "off",
];

fn is_valid_action(action: &str) -> bool {
    VALID_ACTIONS.contains(&action)
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn string_from_bits(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    string_obj_to_owned(obj)
}

fn string_from_bits_or(_py: &PyToken<'_>, bits: u64, default: &str) -> String {
    string_from_bits(_py, bits).unwrap_or_else(|| default.to_string())
}

fn i64_from_bits_default(bits: u64, default: i64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return default;
    }
    if let Some(i) = to_i64(obj) {
        return i;
    }
    default
}

fn bool_from_bits_default(bits: u64, default: bool) -> bool {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return default;
    }
    if let Some(i) = to_i64(obj) {
        return i != 0;
    }
    default
}

fn alloc_string_result(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

/// Simple pattern matching for warning filter patterns.
/// Supports: literal match, empty pattern (match all), and basic `.*` globbing.
fn pattern_matches(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    // Exact match
    if pattern == text {
        return true;
    }
    // Simple regex-like: if the pattern starts and ends with `.*`, match substring
    if pattern == ".*" {
        return true;
    }
    // Simple prefix match with trailing `.*`
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return text.starts_with(prefix);
    }
    // Simple suffix match with leading `.*`
    if let Some(suffix) = pattern.strip_prefix(".*") {
        return text.ends_with(suffix);
    }
    false
}

/// Check if a specific filter entry matches the given warning parameters.
fn filter_matches(
    filter: &WarningFilter,
    message: &str,
    category: &str,
    module: &str,
    lineno: i64,
) -> bool {
    if !filter.message.is_empty() && !pattern_matches(&filter.message, message) {
        return false;
    }
    if !filter.module.is_empty() && !pattern_matches(&filter.module, module) {
        return false;
    }
    if filter.lineno != 0 && filter.lineno != lineno {
        return false;
    }
    if !filter.category.is_empty() {
        // Simple category matching: exact name match or subclass check
        // For the intrinsic, we compare category name strings.
        if filter.category != category && filter.category != "Warning" {
            return false;
        }
    }
    true
}

/// Determine the action for a warning based on the current filter list.
fn action_for(message: &str, category: &str, module: &str, lineno: i64) -> String {
    let state = WARNINGS_STATE.lock().unwrap();
    for filter in &state.filters {
        if filter_matches(filter, message, category, module, lineno) {
            return filter.action.clone();
        }
    }
    state.default_action.clone()
}

/// Check whether a warning should be suppressed based on action and registries.
fn should_suppress(action: &str, message: &str, category: &str, lineno: i64) -> bool {
    let mut state = WARNINGS_STATE.lock().unwrap();
    match action {
        "ignore" | "off" => true,
        "always" => false,
        "once" => {
            let key = OnceKey {
                message: message.to_string(),
                category: category.to_string(),
            };
            if state.once_registry.contains(&key) {
                return true;
            }
            state.once_registry.insert(key);
            false
        }
        "module" => {
            // Module action: suppress if we've already warned for this message+category
            // (lineno=0 in the key). We use the once_registry with a special key.
            let key = OnceKey {
                message: format!("{message}\x00module"),
                category: category.to_string(),
            };
            if state.once_registry.contains(&key) {
                return true;
            }
            state.once_registry.insert(key);
            false
        }
        "default" => {
            // Default action: suppress if we've warned for this message+category+lineno
            let key = OnceKey {
                message: format!("{message}\x00{lineno}"),
                category: category.to_string(),
            };
            if state.once_registry.contains(&key) {
                return true;
            }
            state.once_registry.insert(key);
            false
        }
        _ => false,
    }
}

// ─── public intrinsics ──────────────────────────────────────────────────────

/// Issue a warning. Determines the action based on the current filter list
/// and either raises an exception (for "error"), suppresses the warning,
/// or formats and emits it to stderr.
///
/// Arguments:
///   message_bits: The warning message (str)
///   category_bits: The warning category name (str), e.g., "UserWarning"
///   stacklevel_bits: Stack level for determining the caller location (int)
///
/// Returns None.
/// Emit a DeprecationWarning to stderr.  Used by runtime checks that
/// detect deprecated patterns (e.g. `~bool`).  Deduplicates by message
/// so the same warning is only printed once per process.
pub(crate) fn emit_deprecation_warning(_py: &crate::PyToken<'_>, message: &str) {
    use std::sync::{LazyLock, Mutex};
    static SEEN: LazyLock<Mutex<std::collections::HashSet<u64>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        message.hash(&mut hasher);
        hasher.finish()
    };
    if let Ok(mut seen) = SEEN.lock()
        && !seen.insert(hash)
    {
        return; // Already emitted
    }
    eprintln!("DeprecationWarning: {message}");
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn(
    message_bits: u64,
    category_bits: u64,
    stacklevel_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let message = string_from_bits_or(_py, message_bits, "");
        let category = string_from_bits_or(_py, category_bits, "UserWarning");
        let _stacklevel = i64_from_bits_default(stacklevel_bits, 1);

        // Determine action
        let action = action_for(&message, &category, "__main__", 0);

        if !is_valid_action(&action) {
            let msg = format!("invalid warnings action: {action:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }

        if action == "error" {
            return raise_exception::<_>(_py, &category, &message);
        }

        if should_suppress(&action, &message, &category, 0) {
            return MoltObject::none().bits();
        }

        // Format and emit to stderr
        let formatted = format!("<string>:1: {category}: {message}\n");
        eprint!("{}", formatted);

        MoltObject::none().bits()
    })
}

/// Issue a warning with explicit source location information.
/// This bypasses the stack introspection that `warn()` performs.
///
/// Arguments:
///   message_bits: The warning message (str)
///   category_bits: The warning category name (str)
///   filename_bits: Source filename (str)
///   lineno_bits: Source line number (int)
///   module_bits: Module name (str or None)
///   registry_bits: Warning registry dict (ignored in intrinsic, handled by shim)
///   module_globals_bits: Module globals (ignored in intrinsic, handled by shim)
///
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn_explicit(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    module_bits: u64,
    _registry_bits: u64,
    _module_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let message = string_from_bits_or(_py, message_bits, "");
        let category = string_from_bits_or(_py, category_bits, "UserWarning");
        let filename = string_from_bits_or(_py, filename_bits, "<string>");
        let lineno = i64_from_bits_default(lineno_bits, 1);
        let module = string_from_bits_or(_py, module_bits, "__main__");

        let action = action_for(&message, &category, &module, lineno);

        if !is_valid_action(&action) {
            let msg = format!("invalid warnings action: {action:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }

        if action == "error" {
            return raise_exception::<_>(_py, &category, &message);
        }

        if should_suppress(&action, &message, &category, lineno) {
            return MoltObject::none().bits();
        }

        let formatted = format!("{filename}:{lineno}: {category}: {message}\n");
        eprint!("{}", formatted);

        MoltObject::none().bits()
    })
}

/// Format a warning message into the standard CPython warning string format:
///   "filename:lineno: category: message\n  source_line\n"
///
/// Arguments:
///   message_bits: The warning message (str)
///   category_bits: The warning category name (str)
///   filename_bits: Source filename (str)
///   lineno_bits: Source line number (int)
///   line_bits: Source code line text (str or None)
///
/// Returns a formatted string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_formatwarning(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    line_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let message = string_from_bits_or(_py, message_bits, "");
        let category = string_from_bits_or(_py, category_bits, "UserWarning");
        let filename = string_from_bits_or(_py, filename_bits, "<string>");
        let lineno = i64_from_bits_default(lineno_bits, 1);
        let line = string_from_bits(_py, line_bits);

        let formatted = if let Some(line_text) = line {
            let trimmed = line_text.trim();
            if trimmed.is_empty() {
                format!("{filename}:{lineno}: {category}: {message}\n")
            } else {
                format!("{filename}:{lineno}: {category}: {message}\n  {trimmed}\n")
            }
        } else {
            format!("{filename}:{lineno}: {category}: {message}\n")
        };

        alloc_string_result(_py, &formatted)
    })
}

/// Show a warning by writing it to a file or stderr.
///
/// Arguments:
///   message_bits: The warning message (str)
///   category_bits: The warning category name (str)
///   filename_bits: Source filename (str)
///   lineno_bits: Source line number (int)
///   file_bits: Output file object (or None for stderr)
///   line_bits: Source code line text (str or None)
///
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_showwarning(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    _file_bits: u64,
    line_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let message = string_from_bits_or(_py, message_bits, "");
        let category = string_from_bits_or(_py, category_bits, "UserWarning");
        let filename = string_from_bits_or(_py, filename_bits, "<string>");
        let lineno = i64_from_bits_default(lineno_bits, 1);
        let line = string_from_bits(_py, line_bits);

        let formatted = if let Some(line_text) = line {
            let trimmed = line_text.trim();
            if trimmed.is_empty() {
                format!("{filename}:{lineno}: {category}: {message}\n")
            } else {
                format!("{filename}:{lineno}: {category}: {message}\n  {trimmed}\n")
            }
        } else {
            format!("{filename}:{lineno}: {category}: {message}\n")
        };

        // Write to stderr (the file_bits parameter is handled by the Python shim)
        eprint!("{}", formatted);

        MoltObject::none().bits()
    })
}

/// Add a simple filter entry. This is equivalent to calling
/// `filterwarnings(action, "", category, "", lineno, append)`.
///
/// Arguments:
///   action_bits: Filter action string (str)
///   category_bits: Warning category name (str or None)
///   lineno_bits: Line number filter (int, 0 = all lines)
///   append_bits: If true, append to end of filters; else insert at front (bool)
///
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_simplefilter(
    action_bits: u64,
    category_bits: u64,
    lineno_bits: u64,
    append_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let action = string_from_bits_or(_py, action_bits, "default").to_ascii_lowercase();
        if !is_valid_action(&action) {
            let msg = format!("invalid warnings action: {action:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }

        let category = string_from_bits_or(_py, category_bits, "Warning");
        let lineno = i64_from_bits_default(lineno_bits, 0);
        let append = bool_from_bits_default(append_bits, false);

        let filter = WarningFilter {
            action,
            message: String::new(),
            category,
            module: String::new(),
            lineno,
        };

        let mut state = WARNINGS_STATE.lock().unwrap();
        if append {
            state.filters.push(filter);
        } else {
            state.filters.insert(0, filter);
        }
        state.filters_version += 1;

        MoltObject::none().bits()
    })
}

/// Add a warning filter with message and module patterns.
///
/// Arguments:
///   action_bits: Filter action string (str)
///   message_bits: Message pattern (str, empty = match all)
///   category_bits: Warning category name (str or None)
///   module_bits: Module pattern (str, empty = match all)
///   lineno_bits: Line number filter (int, 0 = all lines)
///   append_bits: If true, append to end of filters; else insert at front (bool)
///
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_filterwarnings(
    action_bits: u64,
    message_bits: u64,
    category_bits: u64,
    module_bits: u64,
    lineno_bits: u64,
    append_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let action = string_from_bits_or(_py, action_bits, "default").to_ascii_lowercase();
        if !is_valid_action(&action) {
            let msg = format!("invalid warnings action: {action:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }

        let message = string_from_bits_or(_py, message_bits, "");
        let category = string_from_bits_or(_py, category_bits, "Warning");
        let module = string_from_bits_or(_py, module_bits, "");
        let lineno = i64_from_bits_default(lineno_bits, 0);
        let append = bool_from_bits_default(append_bits, false);

        let filter = WarningFilter {
            action,
            message,
            category,
            module,
            lineno,
        };

        let mut state = WARNINGS_STATE.lock().unwrap();
        if append {
            state.filters.push(filter);
        } else {
            state.filters.insert(0, filter);
        }
        state.filters_version += 1;

        MoltObject::none().bits()
    })
}

/// Clear all warning filters and the "once" registry.
/// Equivalent to `warnings.resetwarnings()`.
///
/// Returns None.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_resetwarnings() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut state = WARNINGS_STATE.lock().unwrap();
        state.filters.clear();
        state.once_registry.clear();
        state.filters_version += 1;
        MoltObject::none().bits()
    })
}

/// Get the current filter list as a list of tuples.
/// Each tuple is (action, message, category, module, lineno).
///
/// Returns a list of 5-tuples.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_filters_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let state = WARNINGS_STATE.lock().unwrap();
        let mut tuple_bits: Vec<u64> = Vec::with_capacity(state.filters.len());

        for filter in &state.filters {
            let action_ptr = alloc_string(_py, filter.action.as_bytes());
            if action_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let action_bits = MoltObject::from_ptr(action_ptr).bits();

            let message_bits = if filter.message.is_empty() {
                MoltObject::none().bits()
            } else {
                let ptr = alloc_string(_py, filter.message.as_bytes());
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(ptr).bits()
            };

            let category_bits = if filter.category.is_empty() {
                MoltObject::none().bits()
            } else {
                let ptr = alloc_string(_py, filter.category.as_bytes());
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(ptr).bits()
            };

            let module_bits = if filter.module.is_empty() {
                MoltObject::none().bits()
            } else {
                let ptr = alloc_string(_py, filter.module.as_bytes());
                if ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(ptr).bits()
            };

            let lineno_bits = MoltObject::from_int(filter.lineno).bits();

            let elems = [
                action_bits,
                message_bits,
                category_bits,
                module_bits,
                lineno_bits,
            ];
            let tuple_ptr = alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
        }

        let list_ptr = alloc_list(_py, &tuple_bits);
        if list_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}
