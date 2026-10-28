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

/// The interpreter-wide "once" registry: `true` when `(message, category)`
/// already warned, else records it.
fn once_already_warned(message: &str, category: &str) -> bool {
    let key = OnceKey {
        message: message.to_string(),
        category: category.to_string(),
    };
    !WARNINGS_STATE.lock().unwrap().once_registry.insert(key)
}

// ─── warning location (CPython 3.12 `setup_context`) ────────────────────────

/// The number of Python frames `warnings.warn` itself adds above its caller:
/// the shim calls `molt_warnings_warn` from its own body. CPython's C `warn`
/// has no frame, so the location walk starts below this one.
const WARN_ENTRY_FRAMES: usize = 1;

/// CPython's import machinery, which a stacklevel walk passes over: 3.12
/// `is_internal_frame` matches a filename containing both "importlib" and
/// "_bootstrap".
fn is_internal_filename(filename: &str) -> bool {
    filename.contains("importlib") && filename.contains("_bootstrap")
}

fn is_internal_frame(code_bits: u64) -> bool {
    let Some(code) = obj_from_bits(code_bits).as_ptr() else {
        return false;
    };
    string_obj_to_owned(obj_from_bits(unsafe { code_filename_bits(code) }))
        .is_some_and(|filename| is_internal_filename(&filename))
}

/// The frame CPython 3.12 `setup_context` attributes a warning to, walking
/// `stacklevel` frames from the newest. When the newest frame is internal, or
/// `stacklevel <= 0`, every frame counts; otherwise each step also passes
/// over internal frames (`next_external_frame`). `None`: the walk ran off the
/// stack, and the warning belongs to `sys`.
fn setup_context_frame<T>(
    mut frames: impl Iterator<Item = T>,
    stacklevel: i64,
    is_internal: impl Fn(&T) -> bool,
) -> Option<T> {
    let mut frame = frames.next();
    let skip_internal = stacklevel > 0 && !frame.as_ref().is_some_and(&is_internal);
    let mut level = stacklevel;
    loop {
        level = level.saturating_sub(1);
        if level <= 0 || frame.is_none() {
            return frame;
        }
        frame = frames.next();
        while skip_internal && frame.as_ref().is_some_and(&is_internal) {
            frame = frames.next();
        }
    }
}

/// Where a warning is attributed: CPython 3.12 `setup_context`. Every field
/// is owned. `module` is the frame's `__name__` (`str` or `None`, else
/// `"<string>"`); `registry` is its `__warningregistry__` and `globals` its
/// namespace, both `None` when no globals exist.
struct WarningLocation<'a, 'py> {
    filename: ExceptionValue<'a, 'py>,
    lineno: i64,
    module: ExceptionValue<'a, 'py>,
    registry: ExceptionValue<'a, 'py>,
    globals: ExceptionValue<'a, 'py>,
}

fn owned_str<'a, 'py>(py: &'a PyToken<'py>, text: &str) -> Option<ExceptionValue<'a, 'py>> {
    let ptr = alloc_string(py, text.as_bytes());
    (!ptr.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

fn owned_name<'a, 'py>(py: &'a PyToken<'py>, name: &[u8]) -> Option<ExceptionValue<'a, 'py>> {
    attr_name_bits_from_bytes(py, name).map(|bits| ExceptionValue::adopt(py, bits))
}

/// The dictionary storage of a namespace or registry, `None` for anything else.
fn dict_storage(py: &PyToken<'_>, mapping: u64) -> Result<Option<*mut u8>, ()> {
    let storage = crate::object::ops::dict_backing_bits(py, mapping)?;
    Ok(storage
        .and_then(|bits| obj_from_bits(bits).as_ptr())
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_DICT))
}

/// `sys.__dict__`, which CPython uses when the walk leaves the stack; `None`
/// before the `sys` module exists.
fn sys_globals<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    let name = owned_name(py, b"sys")?;
    let module = ExceptionValue::adopt(py, molt_module_cache_get(name.bits()));
    if exception_pending(py) {
        return None;
    }
    let none = || Some(ExceptionValue::adopt(py, MoltObject::none().bits()));
    let Some(ptr) = obj_from_bits(module.bits()).as_ptr() else {
        return none();
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_MODULE {
        return none();
    }
    Some(ExceptionValue::pin(py, unsafe { module_dict_bits(ptr) }))
}

/// Compute a warning's location from the runtime frame stack, after the
/// `entry_frames` the warning entry point itself added. `None`: an exception
/// is pending.
fn warning_location<'a, 'py>(
    py: &'a PyToken<'py>,
    stacklevel: i64,
    entry_frames: usize,
) -> Option<WarningLocation<'a, 'py>> {
    let site = crate::builtins::frames::python_frame_site(py, |frames| {
        setup_context_frame(frames.skip(entry_frames), stacklevel, |site| {
            is_internal_frame(site.code_bits)
        })
    });
    let (filename, lineno, globals) = match site {
        Some(site) => {
            let code = ExceptionValue::adopt(py, site.code_bits);
            let globals = ExceptionValue::adopt(py, site.globals_bits);
            // The frame stack admits only code objects as Python frames.
            let code_ptr = obj_from_bits(code.bits()).as_ptr()?;
            let filename = ExceptionValue::pin(py, unsafe { code_filename_bits(code_ptr) });
            (filename, site.line, globals)
        }
        None => (owned_str(py, "sys")?, 1, sys_globals(py)?),
    };
    let none = || ExceptionValue::adopt(py, MoltObject::none().bits());
    let Some(dict) = dict_storage(py, globals.bits()).ok()? else {
        return Some(WarningLocation {
            filename,
            lineno,
            module: owned_str(py, "<string>")?,
            registry: none(),
            globals: none(),
        });
    };
    let name_key = owned_name(py, b"__name__")?;
    let module = match unsafe { dict_get_in_place(py, dict, name_key.bits()) }.ok()? {
        Some(bits)
            if obj_from_bits(bits).is_none()
                || obj_from_bits(bits)
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_STRING) =>
        {
            ExceptionValue::pin(py, bits)
        }
        _ => owned_str(py, "<string>")?,
    };
    let registry_key = owned_name(py, b"__warningregistry__")?;
    let registry = match unsafe { dict_get_in_place(py, dict, registry_key.bits()) }.ok()? {
        Some(bits) => ExceptionValue::pin(py, bits),
        None => {
            let ptr = alloc_dict_with_pairs(py, &[]);
            if ptr.is_null() {
                return None;
            }
            let registry = ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits());
            if !unsafe { dict_set_in_place(py, dict, registry_key.bits(), registry.bits()) } {
                return None;
            }
            registry
        }
    };
    Some(WarningLocation {
        filename,
        lineno,
        module,
        registry,
        globals,
    })
}

/// Deliver a located warning. A loaded `warnings` module owns the capture and
/// display hooks, so its `warn_explicit` receives the location; without it the
/// native engine emits the warning under the native filters.
fn warn_at(
    py: &PyToken<'_>,
    message: u64,
    category: u64,
    location: WarningLocation<'_, '_>,
) -> bool {
    let Some(name) = owned_name(py, b"warnings") else {
        return false;
    };
    let module = ExceptionValue::adopt(py, molt_module_cache_get(name.bits()));
    if exception_pending(py) {
        return false;
    }
    if obj_from_bits(module.bits()).is_none() {
        let version = WARNINGS_STATE.lock().unwrap().filters_version;
        return registry_admit_version(py, location.registry.bits(), version)
            && emit_warning(
                py,
                message,
                category,
                location.filename.bits(),
                location.lineno,
                location.module.bits(),
                location.registry.bits(),
            );
    }
    let Some(name) = owned_name(py, b"warn_explicit") else {
        return false;
    };
    let warn_explicit = ExceptionValue::adopt(py, molt_get_attr_name(module.bits(), name.bits()));
    if exception_pending(py) {
        return false;
    }
    let lineno = ExceptionValue::adopt(py, int_bits_from_i64(py, location.lineno));
    if exception_pending(py) {
        return false;
    }
    let args = [
        message,
        category,
        location.filename.bits(),
        lineno.bits(),
        location.module.bits(),
        location.registry.bits(),
        location.globals.bits(),
    ];
    let result = ExceptionValue::adopt(py, unsafe {
        crate::call::bind::call_bind_borrowed(py, warn_explicit.bits(), None, &args, &[], &[])
    });
    drop(result);
    !exception_pending(py)
}

/// Clear a `__warningregistry__` whose `version` is not the live filter
/// version, as CPython's `already_warned` does, so a filter change re-admits
/// warnings it once suppressed. `None` (no registry) needs nothing.
fn registry_admit_version(py: &PyToken<'_>, registry: u64, version: i64) -> bool {
    let Ok(Some(dict)) = dict_storage(py, registry) else {
        return !exception_pending(py);
    };
    let Some(key) = owned_name(py, b"version") else {
        return false;
    };
    match unsafe { dict_get_in_place(py, dict, key.bits()) } {
        Err(()) => return false,
        Ok(Some(bits)) if obj_from_bits(bits).as_int() == Some(version) => return true,
        Ok(_) => {}
    }
    unsafe { dict_clear_in_place(py, dict) };
    if exception_pending(py) {
        return false;
    }
    let version = ExceptionValue::adopt(py, int_bits_from_i64(py, version));
    !exception_pending(py) && unsafe { dict_set_in_place(py, dict, key.bits(), version.bits()) }
}

/// Look up `key` in a registry dictionary. `Err`: an exception is pending.
fn registry_contains(py: &PyToken<'_>, registry: *mut u8, key: u64) -> Result<bool, ()> {
    match unsafe { dict_get_in_place(py, registry, key)? } {
        Some(bits) => crate::object::ops::truth_value(py, obj_from_bits(bits)),
        None => Ok(false),
    }
}

/// Record `key` as warned in a registry dictionary. `false`: an exception is
/// pending.
fn registry_mark(py: &PyToken<'_>, registry: *mut u8, key: u64) -> bool {
    unsafe { dict_set_in_place(py, registry, key, MoltObject::from_bool(true).bits()) }
}

fn registry_key<'a, 'py>(
    py: &'a PyToken<'py>,
    text: u64,
    category: u64,
    lineno: i64,
) -> Option<ExceptionValue<'a, 'py>> {
    let ptr = alloc_tuple(py, &[text, category, MoltObject::from_int(lineno).bits()]);
    (!ptr.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

/// Python's `str.strip()` whitespace, which differs from Unicode `White_Space`
/// in the four information separators.
fn strip_python_whitespace(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// The source text of `filename:lineno`, read through the compiled-source
/// reader the tracebacks use. Empty when unavailable.
fn warning_source_line(py: &PyToken<'_>, filename: u64, lineno: u64) -> String {
    let (Some(filename), Some(lineno)) = (
        string_from_bits(py, filename),
        to_i64(obj_from_bits(lineno)),
    ) else {
        return String::new();
    };
    let line =
        crate::object::ops_sys::traceback_source_line_native(py, filename.as_bytes(), lineno);
    String::from_utf8_lossy(&line).into_owned()
}

/// CPython's `_formatwarnmsg_impl`: `"{filename}:{lineno}: {category}:
/// {message}\n"`, then the stripped source line when it is non-empty. A
/// `None` line is read through [`warning_source_line`]. `None`: an exception
/// is pending.
fn format_warning(
    py: &PyToken<'_>,
    message: u64,
    category: &str,
    filename: u64,
    lineno: u64,
    line: u64,
) -> Option<String> {
    let message = crate::format_obj_str(py, obj_from_bits(message));
    let location = [filename, lineno].map(|bits| crate::format_obj_str(py, obj_from_bits(bits)));
    if exception_pending(py) {
        return None;
    }
    let [filename_text, lineno_text] = location;
    let mut text = format!("{filename_text}:{lineno_text}: {category}: {message}\n");
    let line = if obj_from_bits(line).is_none() {
        warning_source_line(py, filename, lineno)
    } else {
        crate::format_obj_str(py, obj_from_bits(line))
    };
    if exception_pending(py) {
        return None;
    }
    if !line.is_empty() {
        text.push_str("  ");
        text.push_str(strip_python_whitespace(&line));
        text.push('\n');
    }
    Some(text)
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

/// CPython 3.12 `warn_explicit` (Python/_warnings.c) under the native filter
/// state: the registry check, the filter action, the registry updates, then
/// the display. `registry` is a dictionary whose `version` the caller has
/// already admitted, or `None`. No callback runs with the state mutex held.
fn emit_warning(
    py: &PyToken<'_>,
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno: i64,
    module_bits: u64,
    registry_bits: u64,
) -> bool {
    // A module of None is a warning emitted while the interpreter shuts down.
    if obj_from_bits(module_bits).is_none() {
        return true;
    }
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
    let registry = match dict_storage(py, registry_bits) {
        Ok(registry) => registry,
        Err(()) => return false,
    };
    let Some(key) = registry_key(py, message_bits, category_bits, lineno) else {
        return false;
    };
    if let Some(registry) = registry {
        match registry_contains(py, registry, key.bits()) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(()) => return false,
        }
    }
    let module = string_from_bits_or(py, module_bits, "");
    let action = action_for(&message, &category, &module, lineno);
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
    if action == "ignore" || action == "off" {
        return true;
    }
    // Every action but "always" records the location, then "once" and
    // "module" consult their own keys.
    let suppressed = if action == "always" {
        false
    } else {
        if let Some(registry) = registry
            && !registry_mark(py, registry, key.bits())
        {
            return false;
        }
        match (action.as_str(), registry) {
            ("once", _) => once_already_warned(&message, &category),
            ("module", Some(registry)) => {
                let Some(module_key) = registry_key(py, message_bits, category_bits, 0) else {
                    return false;
                };
                match registry_contains(py, registry, module_key.bits()) {
                    Ok(true) => true,
                    Ok(false) if registry_mark(py, registry, module_key.bits()) => false,
                    _ => return false,
                }
            }
            _ => false,
        }
    };
    if !suppressed {
        let lineno = MoltObject::from_int(lineno).bits();
        let none = MoltObject::none().bits();
        let Some(text) = format_warning(py, warning.bits(), &category, filename_bits, lineno, none)
        else {
            return false;
        };
        eprint!("{text}");
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

/// Borrowed message/category emission used by the public C API and runtime
/// operations (`PyErr_WarnEx`'s `do_warn`). The runtime operation adds no
/// Python frame, so `stacklevel` 1 names the executing Python frame. A loaded
/// `warnings` module keeps its capture/display callbacks; absence uses the
/// native engine, without initiating an import.
pub(crate) fn emit_warning_object(
    py: &PyToken<'_>,
    message: u64,
    category: u64,
    stacklevel: i64,
) -> bool {
    let Some(location) = warning_location(py, stacklevel, 0) else {
        return false;
    };
    warn_at(py, message, category, location)
}

/// `warnings.warn`: attribute the warning `stacklevel` frames above the
/// shim's caller, then deliver it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn(
    message_bits: u64,
    category_bits: u64,
    stacklevel_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stacklevel = i64_from_bits_default(stacklevel_bits, 1);
        if let Some(location) = warning_location(_py, stacklevel, WARN_ENTRY_FRAMES) {
            warn_at(_py, message_bits, category_bits, location);
        }
        MoltObject::none().bits()
    })
}

/// `warnings.warn_explicit` under the native filters, for the shim's
/// uncaptured path. The shim has normalized `module` and admitted the
/// registry's `version`; `registry` is a dictionary or `None`, and
/// `module_globals` is unused because sources are read by file name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn_explicit(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    module_bits: u64,
    registry_bits: u64,
    _module_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(lineno) = to_i64(obj_from_bits(lineno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "warning lineno must be an int");
        };
        emit_warning(
            _py,
            message_bits,
            category_bits,
            filename_bits,
            lineno,
            module_bits,
            registry_bits,
        );
        MoltObject::none().bits()
    })
}

/// The `category.__name__` a formatted warning shows: a managed or native
/// class's name, or a name the caller passed as text.
fn display_category_name(py: &PyToken<'_>, category: u64) -> String {
    warning_category_name(py, category)
        .unwrap_or_else(|| category_name_from_bits(py, category, "UserWarning"))
}

/// `warnings.formatwarning`: the CPython warning text, with the source line
/// read from `filename` when `line` is `None`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_formatwarning(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    line_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let category = display_category_name(_py, category_bits);
        match format_warning(
            _py,
            message_bits,
            &category,
            filename_bits,
            lineno_bits,
            line_bits,
        ) {
            Some(text) => alloc_string_result(_py, &text),
            None => MoltObject::none().bits(),
        }
    })
}

/// `warnings.showwarning` to the process's standard error; the shim writes to
/// an explicit `file` itself.
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
        let category = display_category_name(_py, category_bits);
        if let Some(text) = format_warning(
            _py,
            message_bits,
            &category,
            filename_bits,
            lineno_bits,
            line_bits,
        ) {
            eprint!("{text}");
        }
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

    /// What a loaded module's `warn_explicit` received: message, category,
    /// filename, lineno, module and whether the registry is the dictionary
    /// stored in the passed globals as `__warningregistry__`.
    type ObservedWarning = (String, u64, String, i64, String, bool);

    thread_local! {
        static OBSERVED: RefCell<Option<ObservedWarning>> = const { RefCell::new(None) };
        static CALLBACK_ERROR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    #[allow(clippy::too_many_arguments)]
    extern "C" fn capture_warning(
        message: u64,
        category: u64,
        filename: u64,
        lineno: u64,
        module: u64,
        registry: u64,
        globals: u64,
    ) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let stored = owned_name(py, b"__warningregistry__").and_then(|key| {
                let dict = dict_storage(py, globals).ok()??;
                unsafe { dict_get_in_place(py, dict, key.bits()) }.ok()?
            });
            OBSERVED.with(|value| {
                *value.borrow_mut() = Some((
                    string_from_bits(py, message).unwrap(),
                    category,
                    string_from_bits(py, filename).unwrap(),
                    to_i64(obj_from_bits(lineno)).unwrap(),
                    string_from_bits(py, module).unwrap(),
                    stored == Some(registry),
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

    /// A pushed Python frame: a code object named after `filename`, a globals
    /// dictionary whose `__name__` is `module`, and the current `line`. The
    /// frame stack owns every edge; [`crate::builtins::frames::frame_stack_pop`]
    /// releases them.
    fn push_python_frame(py: &PyToken<'_>, filename: &str, module: &str, line: i64) -> u64 {
        let filename = ExceptionValue::adopt(py, alloc_string_result(py, filename));
        let name = ExceptionValue::adopt(py, alloc_string_result(py, "frame"));
        let empty = ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_tuple(py, &[])).bits());
        let code = alloc_code_obj(
            py,
            filename.bits(),
            name.bits(),
            1,
            MoltObject::none().bits(),
            empty.bits(),
            empty.bits(),
            0,
            0,
            0,
        );
        assert!(!code.is_null());
        let key = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"__name__").unwrap());
        let value = ExceptionValue::adopt(py, alloc_string_result(py, module));
        let globals = alloc_dict_with_pairs(py, &[key.bits(), value.bits()]);
        assert!(!globals.is_null());
        let globals = MoltObject::from_ptr(globals).bits();
        crate::builtins::frames::frame_stack_push_owned(
            py,
            MoltObject::from_ptr(code).bits(),
            globals,
            0,
            0,
        );
        crate::builtins::frames::frame_stack_set_line(line);
        globals
    }

    fn string_field(py: &PyToken<'_>, value: &ExceptionValue<'_, '_>) -> Option<String> {
        string_from_bits(py, value.bits())
    }

    #[test]
    fn setup_context_frame_follows_cpython_stacklevel_walk() {
        let internal = |name: &&str| is_internal_filename(name);
        let walk = |frames: &[&'static str], level| {
            setup_context_frame(frames.iter().copied(), level, internal)
        };
        // Newest first. An external newest frame skips internal frames.
        let stack = [
            "user_c.py",
            "<frozen importlib._bootstrap>",
            "user_b.py",
            "user_a.py",
        ];
        assert_eq!(walk(&stack, 1), Some("user_c.py"));
        assert_eq!(walk(&stack, 2), Some("user_b.py"));
        assert_eq!(walk(&stack, 3), Some("user_a.py"));
        assert_eq!(walk(&stack, 4), None);
        assert_eq!(walk(&stack, 0), Some("user_c.py"));
        assert_eq!(walk(&stack, -7), Some("user_c.py"));
        assert_eq!(walk(&stack, i64::MAX), None);
        assert_eq!(walk(&stack, i64::MIN), Some("user_c.py"));
        // An internal newest frame counts every frame, as CPython does.
        let internal_first = [
            "<frozen importlib._bootstrap>",
            "user_b.py",
            "<frozen importlib._bootstrap_external>",
            "user_a.py",
        ];
        assert_eq!(walk(&internal_first, 2), Some("user_b.py"));
        assert_eq!(
            walk(&internal_first, 3),
            Some("<frozen importlib._bootstrap_external>")
        );
        assert_eq!(walk(&[], 1), None);
        // Both words are required.
        assert!(!is_internal_filename("importlib/resources.py"));
        assert!(!is_internal_filename("/app/_bootstrap.py"));
    }

    #[test]
    fn warning_location_reads_the_frame_stack_and_owns_the_registry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let outer = push_python_frame(py, "/app/outer.py", "outer_mod", 40);
            let bootstrap = push_python_frame(
                py,
                "<frozen importlib._bootstrap>",
                "importlib._bootstrap",
                9,
            );
            let inner = push_python_frame(py, "/app/inner.py", "inner_mod", 12);
            // A runtime namespace context is not a Python frame.
            crate::builtins::frames::frame_stack_push_owned(py, 0, 0, 0, 0);

            let location = warning_location(py, 1, 0).expect("location");
            assert_eq!(
                string_field(py, &location.filename).as_deref(),
                Some("/app/inner.py")
            );
            assert_eq!(location.lineno, 12);
            assert_eq!(
                string_field(py, &location.module).as_deref(),
                Some("inner_mod")
            );
            assert_eq!(location.globals.bits(), inner);
            let registry = location.registry.bits();
            assert!(dict_storage(py, registry).unwrap().is_some());
            drop(location);
            // The registry is created once and stored in the frame's globals.
            let again = warning_location(py, 1, 0).expect("location");
            assert_eq!(again.registry.bits(), registry);
            drop(again);

            // stacklevel 2 passes over the import machinery frame.
            let location = warning_location(py, 2, 0).expect("location");
            assert_eq!(
                string_field(py, &location.filename).as_deref(),
                Some("/app/outer.py")
            );
            assert_eq!(location.lineno, 40);
            assert_eq!(
                string_field(py, &location.module).as_deref(),
                Some("outer_mod")
            );
            assert_eq!(location.globals.bits(), outer);
            drop(location);

            // The warn entry point's own frame is not counted. An internal
            // newest frame is itself the location at stacklevel 1, and the walk
            // from it counts every frame.
            let location = warning_location(py, 1, WARN_ENTRY_FRAMES).expect("location");
            assert_eq!(location.globals.bits(), bootstrap);
            assert_eq!(location.lineno, 9);
            drop(location);
            let location = warning_location(py, 2, WARN_ENTRY_FRAMES).expect("location");
            assert_eq!(location.globals.bits(), outer);
            drop(location);

            // Off the end of the stack the warning belongs to `sys`, line 1.
            let location = warning_location(py, 3, 0).expect("location");
            assert_eq!(string_field(py, &location.filename).as_deref(), Some("sys"));
            assert_eq!(location.lineno, 1);
            drop(location);
            assert!(!exception_pending(py));
            for _ in 0..4 {
                crate::builtins::frames::frame_stack_pop(py);
            }
        });
    }

    #[test]
    fn native_warning_registry_deduplicates_by_location() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            set_action(py, b"default");
            let category =
                crate::builtins::exceptions::exception_type_bits_from_name(py, "UserWarning");
            let message = ExceptionValue::adopt(py, alloc_string_result(py, "dedup"));
            let filename = ExceptionValue::adopt(py, alloc_string_result(py, "/app/m.py"));
            let module = ExceptionValue::adopt(py, alloc_string_result(py, "m"));
            let registry = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits(),
            );
            let warn = |line| {
                let result = molt_warnings_warn_explicit(
                    message.bits(),
                    category,
                    filename.bits(),
                    MoltObject::from_int(line).bits(),
                    module.bits(),
                    registry.bits(),
                    MoltObject::none().bits(),
                );
                dec_ref_bits(py, result);
                exception_pending(py)
            };
            let dict = dict_storage(py, registry.bits()).unwrap().unwrap();
            let keys = |py: &PyToken<'_>| {
                [7, 8, 0].map(|line| {
                    let key = registry_key(py, message.bits(), category, line).unwrap();
                    registry_contains(py, dict, key.bits()).unwrap()
                })
            };
            let admit = |py: &PyToken<'_>| {
                let version = WARNINGS_STATE.lock().unwrap().filters_version;
                assert!(registry_admit_version(py, registry.bits(), version));
            };
            // "default" records (text, category, lineno) per location.
            admit(py);
            assert!(!warn(7));
            assert!(!warn(7));
            assert!(!warn(8));
            assert_eq!(keys(py), [true, true, false]);
            // A filter change re-admits every location.
            set_action(py, b"module");
            admit(py);
            assert_eq!(keys(py), [false, false, false]);
            // "module" also records (text, category, 0), once per registry.
            assert!(!warn(7));
            assert_eq!(keys(py), [true, false, true]);
            assert!(!warn(8));
            assert_eq!(keys(py), [true, true, true]);
            // A recorded location is suppressed before the filter is consulted.
            set_action(py, b"error");
            admit(py);
            let key = registry_key(py, message.bits(), category, 7).unwrap();
            let marked = MoltObject::from_bool(true).bits();
            assert!(unsafe { dict_set_in_place(py, dict, key.bits(), marked) });
            assert!(!warn(7));
            assert!(warn(9));
            clear_exception(py);
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
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"warn_explicit").unwrap());
            let function = alloc_runtime_function_obj(
                py,
                runtime_fn_addr("warnings_test_capture", capture_warning as *const ()),
                7,
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
            // The C API names the executing Python frame at stacklevel 1 and
            // its caller at 2; `warn_explicit` receives that location.
            push_python_frame(py, "/app/caller.py", "caller_mod", 21);
            push_python_frame(py, "/app/callee.py", "callee_mod", 5);
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"live callback".as_ptr(), 2) },
                0
            );
            OBSERVED.with(|value| {
                assert_eq!(
                    *value.borrow(),
                    Some((
                        "live callback".into(),
                        category_bits.bits(),
                        "/app/caller.py".into(),
                        21,
                        "caller_mod".into(),
                        true,
                    ))
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
                unsafe { errors::PyErr_WarnEx(category, c"live failure".as_ptr(), 1) },
                -1
            );
            let actual = unsafe { errors::PyErr_GetRaisedException() };
            assert!(!actual.is_null());
            let actual = unsafe { OwnedPyObject::from_owned(actual) };
            assert_eq!(actual.as_ptr(), expected.as_ptr());
            OBSERVED.with(|value| {
                assert_eq!(
                    *value.borrow(),
                    Some((
                        "live failure".into(),
                        category_bits.bits(),
                        "/app/callee.py".into(),
                        5,
                        "callee_mod".into(),
                        true,
                    ))
                )
            });
            CALLBACK_ERROR.with(|value| value.set(0));
            crate::builtins::frames::frame_stack_pop(py);
            crate::builtins::frames::frame_stack_pop(py);
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
