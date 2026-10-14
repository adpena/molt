use std::sync::{LazyLock, Mutex};

use crate::builtins::exceptions::ExceptionValue;
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

fn category_name_from_bits(py: &PyToken<'_>, bits: u64, default: &str) -> String {
    if let Some(ptr) = obj_from_bits(bits).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_TYPE
    {
        return class_name_for_error(bits);
    }
    string_from_bits_or(py, bits, default)
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
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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

fn warning_category_bits(py: &PyToken<'_>, bits: u64) -> u64 {
    if obj_from_bits(bits).is_none() {
        return crate::builtins::exceptions::exception_type_bits_from_name(py, "UserWarning");
    }
    // Preserve the intrinsic's existing category-name input while actual
    // category objects retain their identity all the way to construction.
    if let Some(name) = string_from_bits(py, bits) {
        return crate::builtins::exceptions::exception_type_bits_from_name(py, &name);
    }
    bits
}

fn warning_category_name(py: &PyToken<'_>, category: u64) -> Option<String> {
    let ptr = obj_from_bits(category).as_ptr()?;
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_TYPE => Some(class_name_for_error(category)),
        TYPE_ID_FOREIGN => {
            // Read canonical type metadata, not metaclass __getattribute__.
            // Filter classification must not add a Python callback.
            use crate::object::foreign::{NativeObjectOwner, foreign_ptr_from_obj};
            use molt_cpython_abi::api::{strings, typeobj};
            let native = std::ptr::with_exposed_provenance_mut::<
                molt_cpython_abi::abi_types::PyObject,
            >(unsafe { foreign_ptr_from_obj(ptr) });
            if unsafe { typeobj::PyType_Check(native) } == 0 {
                return None;
            }
            let name = unsafe { typeobj::PyType_GetName(native.cast()) };
            if name.is_null() {
                crate::cpython_abi_hooks::transfer_pending_cpython_exception();
                return None;
            }
            let name = unsafe { NativeObjectOwner::from_owned(name) };
            let text = unsafe { strings::PyUnicode_AsUTF8(name.as_ptr()) };
            let result = if text.is_null() {
                None
            } else {
                Some(
                    unsafe { std::ffi::CStr::from_ptr(text) }
                        .to_string_lossy()
                        .into_owned(),
                )
            };
            drop(name);
            if exception_pending(py) { None } else { result }
        }
        _ => None,
    }
}

/// Shared filter/emission authority; no callback runs with the state mutex held.
fn emit_warning(
    py: &PyToken<'_>,
    message_bits: u64,
    category_bits: u64,
    module: &str,
    filename: &str,
    lineno: i64,
) -> bool {
    let category_bits = warning_category_bits(py, category_bits);
    // CPython normalizes the warning instance before selecting a filter, even
    // for ignore. Constructor callbacks may mutate the live warning policy.
    let warning = ExceptionValue::adopt(py, unsafe {
        call_callable1(py, category_bits, message_bits)
    });
    if exception_pending(py) {
        return false;
    }
    let message = string_from_bits_or(py, message_bits, "");
    let Some(category) = warning_category_name(py, category_bits) else {
        if !exception_pending(py) {
            raise_exception::<()>(py, "TypeError", "warning category __name__ must be str");
        }
        return false;
    };
    let action = action_for(&message, &category, module, lineno);
    if !is_valid_action(&action) {
        raise_exception::<()>(
            py,
            "ValueError",
            &format!("invalid warnings action: {action:?}"),
        );
        return false;
    }
    if action == "error" {
        molt_raise(warning.bits());
        return false;
    }
    if !should_suppress(&action, &message, &category, lineno) {
        eprintln!("{filename}:{lineno}: {category}: {message}");
    }
    drop(warning);
    !exception_pending(py)
}

/// Runtime warnings share one filter/emission authority with warning intrinsics.
pub(crate) fn emit_deprecation_warning(py: &PyToken<'_>, message: &str) -> bool {
    emit_runtime_warning(py, message, "DeprecationWarning")
}

pub(crate) fn emit_runtime_warning(py: &PyToken<'_>, message: &str, category: &str) -> bool {
    let category = crate::builtins::exceptions::exception_type_bits_from_name(py, category);
    if category == 0 || exception_pending(py) {
        return false;
    }
    let message = ExceptionValue::adopt(py, alloc_string_result(py, message));
    if exception_pending(py) {
        return false;
    }
    emit_warning_object(py, message.bits(), category, 1)
}

/// Borrowed message/category emission used by the public C API and managed
/// protocols. A loaded public module keeps its capture/display callbacks;
/// absence uses the same intrinsic engine, without initiating an import.
pub(crate) fn emit_warning_object(
    py: &PyToken<'_>,
    message: u64,
    category: u64,
    stacklevel: i64,
) -> bool {
    let Some(name) = attr_name_bits_from_bytes(py, b"warnings") else {
        return false;
    };
    let name = ExceptionValue::adopt(py, name);
    let module = ExceptionValue::adopt(py, molt_module_cache_get(name.bits()));
    if exception_pending(py) {
        return false;
    }
    if obj_from_bits(module.bits()).is_none() {
        return emit_warning(py, message, category, "__main__", "<string>", 1);
    }
    let Some(name) = attr_name_bits_from_bytes(py, b"warn") else {
        return false;
    };
    let name = ExceptionValue::adopt(py, name);
    let warn = ExceptionValue::adopt(py, molt_get_attr_name(module.bits(), name.bits()));
    if exception_pending(py) {
        return false;
    }
    let stacklevel = ExceptionValue::adopt(py, int_bits_from_i64(py, stacklevel));
    if exception_pending(py) {
        return false;
    }
    let result = ExceptionValue::adopt(py, unsafe {
        call_callable3(py, warn.bits(), message, category, stacklevel.bits())
    });
    drop(result);
    drop(warn);
    drop(module);
    !exception_pending(py)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn(
    message_bits: u64,
    category_bits: u64,
    stacklevel_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _stacklevel = i64_from_bits_default(stacklevel_bits, 1);
        emit_warning(_py, message_bits, category_bits, "__main__", "<string>", 1);

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
        let filename = string_from_bits_or(_py, filename_bits, "<string>");
        let lineno = i64_from_bits_default(lineno_bits, 1);
        let module = string_from_bits_or(_py, module_bits, "__main__");

        emit_warning(_py, message_bits, category_bits, &module, &filename, lineno);

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
        let category = category_name_from_bits(_py, category_bits, "UserWarning");
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
        let category = category_name_from_bits(_py, category_bits, "UserWarning");
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

        let category = category_name_from_bits(_py, category_bits, "Warning");
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
        let category = category_name_from_bits(_py, category_bits, "Warning");
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
        // Filter rows contain only native strings and an integer. Snapshot
        // them while locked; runtime allocation and error retirement may
        // reenter resetwarnings/filterwarnings and must run after unlocking.
        let filters = WARNINGS_STATE.lock().unwrap().filters.clone();
        let list_ptr = crate::object::builders::alloc_list_with_capacity(_py, &[], filters.len());
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list = ExceptionValue::adopt(_py, MoltObject::from_ptr(list_ptr).bits());
        for filter in &filters {
            let mut text: [ExceptionValue<'_, '_>; 4] =
                std::array::from_fn(|_| ExceptionValue::adopt(_py, MoltObject::none().bits()));
            for (index, value) in [
                filter.action.as_str(),
                filter.message.as_str(),
                filter.category.as_str(),
                filter.module.as_str(),
            ]
            .into_iter()
            .enumerate()
            {
                if index != 0 && value.is_empty() {
                    continue;
                }
                let ptr = alloc_string(_py, value.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                text[index] = ExceptionValue::adopt(_py, MoltObject::from_ptr(ptr).bits());
            }
            let elems = [
                text[0].bits(),
                text[1].bits(),
                text[2].bits(),
                text[3].bits(),
                MoltObject::from_int(filter.lineno).bits(),
            ];
            let tuple_ptr = alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let tuple = ExceptionValue::adopt(_py, MoltObject::from_ptr(tuple_ptr).bits());
            if !crate::object::ops_list::molt_list_append_with_projection(
                list.bits(),
                tuple.bits(),
                std::ptr::null_mut(),
            ) {
                return MoltObject::none().bits();
            }
        }
        list.into_bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{
        PyExc_DeprecationWarning, PyExc_RuntimeWarning, PyExc_Warning,
    };
    use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    use std::cell::RefCell;

    thread_local! {
        static OBSERVED: RefCell<Option<(String, u64, i64)>> = const { RefCell::new(None) };
        static CALLBACK_ERROR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    extern "C" fn capture_warning(message: u64, category: u64, stacklevel: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            OBSERVED.with(|value| {
                *value.borrow_mut() = Some((
                    string_from_bits(py, message).unwrap(),
                    category,
                    to_i64(obj_from_bits(stacklevel)).unwrap(),
                ))
            });
            let error = CALLBACK_ERROR.with(std::cell::Cell::get);
            if error != 0 {
                return molt_raise(error);
            }
            MoltObject::none().bits()
        })
    }

    extern "C" fn warning_init_changes_filter(_receiver: u64, _message: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            set_action(py, b"error");
            MoltObject::none().bits()
        })
    }

    struct ResetWarnings;
    impl Drop for ResetWarnings {
        fn drop(&mut self) {
            CALLBACK_ERROR.with(|value| value.set(0));
            OBSERVED.with(|value| *value.borrow_mut() = None);
            crate::with_gil_entry_nopanic!(py, {
                unsafe { errors::PyErr_Clear() };
                clear_exception(py);
                molt_warnings_resetwarnings();
                if let Some(name) = attr_name_bits_from_bytes(py, b"warnings") {
                    let result = crate::builtins::modules::molt_module_cache_del(name);
                    dec_ref_bits(py, result);
                    dec_ref_bits(py, name);
                }
            });
        }
    }

    fn set_action(py: &PyToken<'_>, action: &[u8]) {
        molt_warnings_resetwarnings();
        let action = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, action).unwrap());
        let category = crate::builtins::exceptions::exception_type_bits_from_name(py, "Warning");
        let result = molt_warnings_simplefilter(
            action.bits(),
            category,
            MoltObject::from_int(0).bits(),
            MoltObject::from_bool(false).bits(),
        );
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
    }

    #[test]
    fn native_warning_emission_uses_cold_filters_and_preserves_category() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            let name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"warnings").unwrap());
            let module = ExceptionValue::adopt(
                py,
                crate::builtins::modules::molt_module_cache_get(name.bits()),
            );
            assert!(obj_from_bits(module.bits()).is_none());
            set_action(py, b"ignore");
            assert_eq!(
                unsafe {
                    errors::PyErr_WarnEx(
                        (&raw mut PyExc_DeprecationWarning).cast(),
                        c"cold native warning".as_ptr(),
                        3,
                    )
                },
                0
            );
            assert!(!exception_pending(py));
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
            set_action(py, b"error");
            for category in [
                std::ptr::null_mut(),
                (&raw mut PyExc_DeprecationWarning).cast(),
            ] {
                assert_eq!(
                    unsafe { errors::PyErr_WarnEx(category, c"cold warning error".as_ptr(), 3) },
                    -1
                );
                let error = unsafe { errors::PyErr_GetRaisedException() };
                assert!(!error.is_null());
                let error = unsafe { OwnedPyObject::from_owned(error) };
                let class = unsafe { typeobj::PyObject_Type(error.as_ptr()) };
                assert!(!class.is_null());
                let class = unsafe { OwnedPyObject::from_owned(class) };
                let expected = if category.is_null() {
                    (&raw mut PyExc_RuntimeWarning).cast()
                } else {
                    category
                };
                assert_eq!(class.as_ptr(), expected);
            }
            let custom = unsafe {
                errors::PyErr_NewException(
                    c"fixture.SpecialWarning".as_ptr(),
                    (&raw mut PyExc_Warning).cast(),
                    std::ptr::null_mut(),
                )
            };
            assert!(!custom.is_null());
            let custom = unsafe { OwnedPyObject::from_owned(custom) };
            assert_eq!(
                unsafe {
                    errors::PyErr_WarnEx(custom.as_ptr(), c"custom warning identity".as_ptr(), 2)
                },
                -1
            );
            let error = unsafe { errors::PyErr_GetRaisedException() };
            assert!(!error.is_null());
            let error = unsafe { OwnedPyObject::from_owned(error) };
            let class = unsafe { typeobj::PyObject_Type(error.as_ptr()) };
            assert!(!class.is_null());
            let class = unsafe { OwnedPyObject::from_owned(class) };
            assert_eq!(class.as_ptr(), custom.as_ptr());
            // Normalization must execute before even an ignore filter. The
            // constructor changes that live policy to error for this call.
            let category_bits =
                unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(custom.as_ptr()) }.unwrap();
            let category_bits = ExceptionValue::adopt(py, category_bits);
            let init_name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"__init__").unwrap());
            let init = alloc_runtime_function_obj(
                py,
                runtime_fn_addr(
                    "warning_test_init_changes_filter",
                    warning_init_changes_filter as *const (),
                ),
                2,
            );
            assert!(!init.is_null());
            let init = ExceptionValue::adopt(py, MoltObject::from_ptr(init).bits());
            let result =
                crate::molt_set_attr_name(category_bits.bits(), init_name.bits(), init.bits());
            dec_ref_bits(py, result);
            assert!(!exception_pending(py));
            set_action(py, b"ignore");
            assert_eq!(
                unsafe {
                    errors::PyErr_WarnEx(custom.as_ptr(), c"constructor changes policy".as_ptr(), 1)
                },
                -1
            );
            let mutated = unsafe { errors::PyErr_GetRaisedException() };
            assert!(!mutated.is_null());
            let mutated = unsafe { OwnedPyObject::from_owned(mutated) };
            let actual_class = unsafe { typeobj::PyObject_Type(mutated.as_ptr()) };
            assert!(!actual_class.is_null());
            let actual_class = unsafe { OwnedPyObject::from_owned(actual_class) };
            assert_eq!(actual_class.as_ptr(), custom.as_ptr());
        });
    }

    #[test]
    fn native_warning_emission_preserves_loaded_callback_arguments_and_failure() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            let name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"warnings").unwrap());
            let module = alloc_module_obj(py, name.bits());
            assert!(!module.is_null());
            let module = ExceptionValue::adopt(py, MoltObject::from_ptr(module).bits());
            let warn_name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"warn").unwrap());
            let function = alloc_runtime_function_obj(
                py,
                runtime_fn_addr("warnings_test_capture", capture_warning as *const ()),
                3,
            );
            assert!(!function.is_null());
            let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
            let result =
                crate::molt_set_attr_name(module.bits(), warn_name.bits(), function.bits());
            dec_ref_bits(py, result);
            let result =
                crate::builtins::modules::molt_module_cache_set(name.bits(), module.bits());
            dec_ref_bits(py, result);
            assert!(!exception_pending(py));
            let category = (&raw mut PyExc_DeprecationWarning).cast();
            let category_bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(category) }.unwrap();
            let category_bits = ExceptionValue::adopt(py, category_bits);
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"live callback".as_ptr(), 7) },
                0
            );
            OBSERVED.with(|value| {
                assert_eq!(
                    *value.borrow(),
                    Some(("live callback".into(), category_bits.bits(), 7))
                )
            });
            let failure = crate::builtins::exceptions::alloc_exception(
                py,
                "ValueError",
                "warning callback failed",
            );
            assert!(!failure.is_null());
            let failure = ExceptionValue::adopt(py, MoltObject::from_ptr(failure).bits());
            let expected = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(failure.bits()) };
            assert!(!expected.is_null());
            let expected = unsafe { OwnedPyObject::from_owned(expected) };
            CALLBACK_ERROR.with(|value| value.set(failure.bits()));
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"live failure".as_ptr(), 11) },
                -1
            );
            let actual = unsafe { errors::PyErr_GetRaisedException() };
            assert!(!actual.is_null());
            let actual = unsafe { OwnedPyObject::from_owned(actual) };
            assert_eq!(actual.as_ptr(), expected.as_ptr());
            OBSERVED.with(|value| {
                assert_eq!(
                    *value.borrow(),
                    Some(("live failure".into(), category_bits.bits(), 11))
                )
            });
            CALLBACK_ERROR.with(|value| value.set(0));
        });
    }

    thread_local! {
        static FILTER_REENTRY: std::cell::Cell<(usize, bool, usize, usize)> = const {
            std::cell::Cell::new((0, false, 0, 0))
        };
    }

    unsafe extern "C" fn warning_prior_payload_drop(
        object: *mut molt_cpython_abi::abi_types::PyObject,
    ) {
        let before = unsafe { errors::PyErr_Occurred() }.addr();
        // Detect the old deadlock without hanging the test process. Only enter
        // the real public mutations after observing that the mutex is free.
        let unlocked = WARNINGS_STATE.try_lock().is_ok();
        if unlocked {
            let none = MoltObject::none().bits();
            molt_warnings_resetwarnings();
            molt_warnings_simplefilter(none, none, MoltObject::from_int(0).bits(), none);
        }
        let after = unsafe { errors::PyErr_Occurred() }.addr();
        FILTER_REENTRY.with(|seen| seen.set((seen.get().0 + 1, unlocked, before, after)));
        unsafe { molt_cpython_abi::api::memory::PyObject_Free(object.cast()) };
    }

    struct RestoreWarningTracker;
    impl Drop for RestoreWarningTracker {
        fn drop(&mut self) {
            crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
        }
    }

    #[test]
    fn warning_filter_allocation_failure_retires_prior_after_unlocking() {
        use crate::cpython_abi_hooks::native_test_fixture::NativeType;
        use molt_cpython_abi::abi_types::{PyExc_MemoryError, PyExc_ValueError, PyTuple_Type};
        use molt_cpython_abi::api::sequences;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut payload_type = NativeType::subtype(
                    &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                    c"WarningFilterPriorPayload",
                );
                payload_type.tp_dealloc = Some(warning_prior_payload_drop);
                assert_eq!(payload_type.ready(), 0);
                // Clear pending owners before the native declaration dies on
                // either success or panic.
                let _reset = ResetWarnings;
                assert_eq!(typeobj::PyType_Ready(&raw mut PyTuple_Type), 0);
                assert_eq!(typeobj::PyType_Ready(&raw mut PyExc_ValueError), 0);
                for runtime_prior in [false, true] {
                    set_action(py, b"always");
                    let payload = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                        &raw mut *payload_type,
                        0,
                    ));
                    assert!(!payload.as_ptr().is_null());
                    let args = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                        &raw mut PyTuple_Type,
                        1,
                    ));
                    assert!(!args.as_ptr().is_null());
                    assert_eq!(
                        sequences::PyTuple_SetItem(args.as_ptr(), 0, payload.into_ptr()),
                        0
                    );
                    let prior = OwnedPyObject::from_owned(errors::molt_native_exception_new(
                        &raw mut PyExc_ValueError,
                        args.as_ptr(),
                        std::ptr::null_mut(),
                    ));
                    assert!(!prior.as_ptr().is_null());
                    drop(args);
                    errors::PyErr_SetRaisedException(prior.into_ptr());
                    if runtime_prior {
                        assert!(crate::cpython_abi_hooks::transfer_pending_cpython_exception());
                    }
                    FILTER_REENTRY.with(|seen| seen.set((0, false, 0, 0)));
                    crate::resource::set_tracker(Box::new(crate::resource::LimitedTracker::new(
                        &crate::resource::ResourceLimits {
                            max_memory: Some(0),
                            ..Default::default()
                        },
                    )));
                    let denied = RestoreWarningTracker;
                    let result = ExceptionValue::adopt(py, molt_warnings_filters_get());
                    assert!(obj_from_bits(result.bits()).is_none());
                    let expected = (&raw mut PyExc_MemoryError).addr();
                    assert_eq!(
                        FILTER_REENTRY.with(std::cell::Cell::get),
                        (1, true, expected, expected)
                    );
                    assert_eq!(errors::PyErr_Occurred().addr(), expected);
                    {
                        let state = WARNINGS_STATE.lock().unwrap();
                        assert_eq!(state.filters.len(), 1);
                        assert_eq!(state.filters[0].action, "default");
                    }
                    drop(denied);
                    errors::PyErr_Clear();
                }
            }
        });
    }

    #[test]
    fn warning_filter_snapshot_preserves_order_and_complete_rows() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            set_action(py, b"always");
            let action = ExceptionValue::adopt(py, alloc_string_result(py, "ignore"));
            let message = ExceptionValue::adopt(py, alloc_string_result(py, "message.*"));
            let category = ExceptionValue::adopt(py, alloc_string_result(py, "UserWarning"));
            let module = ExceptionValue::adopt(py, alloc_string_result(py, "package.*"));
            molt_warnings_filterwarnings(
                action.bits(),
                message.bits(),
                category.bits(),
                module.bits(),
                MoltObject::from_int(17).bits(),
                MoltObject::from_bool(true).bits(),
            );
            let result = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert!(!exception_pending(py));
            let ptr = obj_from_bits(result.bits()).as_ptr().unwrap();
            let rows =
                unsafe { crate::object::seq_access::snapshot(py, ptr, "filter snapshot test") }
                    .unwrap();
            assert_eq!(rows.len(), 2);
            for (bits, expected) in rows.iter().zip([
                ("always", None, Some("Warning"), None, 0),
                (
                    "ignore",
                    Some("message.*"),
                    Some("UserWarning"),
                    Some("package.*"),
                    17,
                ),
            ]) {
                let tuple = obj_from_bits(*bits).as_ptr().unwrap();
                unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(tuple, |fields| {
                        assert_eq!(fields.len(), 5);
                        assert_eq!(string_from_bits(py, fields[0]).as_deref(), Some(expected.0));
                        assert_eq!(string_from_bits(py, fields[1]).as_deref(), expected.1);
                        assert_eq!(string_from_bits(py, fields[2]).as_deref(), expected.2);
                        assert_eq!(string_from_bits(py, fields[3]).as_deref(), expected.3);
                        assert_eq!(to_i64(obj_from_bits(fields[4])), Some(expected.4));
                    })
                }
                .unwrap();
            }
        });
    }
}
