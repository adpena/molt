//! The warnings engine: CPython 3.12 `Python/_warnings.c`.
//!
//! One authority owns the warning policy. The runtime state holds the filter
//! list, the once registry, the default action and the filters version, as
//! CPython's per-interpreter `WarningsState` does. The `_warnings` module
//! exposes those objects and `warnings` imports them from it, as in CPython.
//! While `warnings` is loaded, its `filters`, `onceregistry` and
//! `defaultaction` attributes are the policy and the state follows them, so
//! `catch_warnings` (which rebinds `warnings.filters`) and the native engine
//! always read one list.

use crate::BuiltinExc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::builtins::exceptions::ExceptionValue;
use crate::object::list_storage::ListStorage;
use crate::*;

// ─── warnings state (CPython `WarningsState`) ────────────────────────────────

/// The runtime's warning policy. Each object slot holds an owned reference
/// (0 until first use); `filters_version` ages every `__warningregistry__`.
/// Every access runs under the GIL.
struct WarningsState {
    filters: AtomicU64,
    once_registry: AtomicU64,
    default_action: AtomicU64,
    filters_version: AtomicI64,
}

const WARNINGS_STATE_KEY: &[u8] = b"molt-runtime/warnings/v1";

impl WarningsState {
    fn new() -> Self {
        Self {
            filters: AtomicU64::new(0),
            once_registry: AtomicU64::new(0),
            default_action: AtomicU64::new(0),
            filters_version: AtomicI64::new(0),
        }
    }

    /// Release every owned object. The runtime's extension-state teardown
    /// calls this with the GIL held.
    fn clear(&self) {
        for slot in [&self.filters, &self.once_registry, &self.default_action] {
            let bits = slot.swap(0, Ordering::AcqRel);
            if bits != 0 {
                crate::with_gil_entry_nopanic!(py, {
                    dec_ref_bits(py, bits);
                });
            }
        }
        self.filters_version.store(0, Ordering::Release);
    }
}

unsafe extern "C" fn warnings_state_init() -> *mut u8 {
    Box::into_raw(Box::new(WarningsState::new())).cast()
}

unsafe extern "C" fn warnings_state_clear(ptr: *mut u8) {
    if let Some(state) = unsafe { ptr.cast::<WarningsState>().as_ref() } {
        state.clear();
    }
}

unsafe extern "C" fn warnings_state_drop(ptr: *mut u8) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr.cast::<WarningsState>()) });
    }
}

fn warnings_state(py: &PyToken<'_>) -> &'static WarningsState {
    let ptr = crate::state::runtime_extension_state_get_or_init(
        crate::state::runtime_state::runtime_state(py),
        WARNINGS_STATE_KEY,
        warnings_state_init,
        warnings_state_clear,
        warnings_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt warnings runtime state initialization failed"
    );
    unsafe { &*ptr.cast::<WarningsState>() }
}

/// CPython `_filters_mutated`: a new filters version, which clears every
/// registry recorded under an older one.
fn filters_mutated(py: &PyToken<'_>) {
    warnings_state(py)
        .filters_version
        .fetch_add(1, Ordering::AcqRel);
}

/// The object in `slot`, created by `init` on first use, as a new reference.
/// `None`: an exception is pending.
fn state_object<'a, 'py>(
    py: &'a PyToken<'py>,
    slot: &AtomicU64,
    init: impl FnOnce(&'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>>,
) -> Option<ExceptionValue<'a, 'py>> {
    let bits = slot.load(Ordering::Acquire);
    if bits != 0 {
        return Some(ExceptionValue::pin(py, bits));
    }
    let value = init(py)?;
    hold_state_object(py, slot, value.bits());
    Some(value)
}

/// CPython's `Py_SETREF(st->field, value)`: the state holds `value`, a
/// borrowed reference.
fn hold_state_object(py: &PyToken<'_>, slot: &AtomicU64, value: u64) {
    if slot.load(Ordering::Acquire) == value {
        return;
    }
    inc_ref_bits(py, value);
    let previous = slot.swap(value, Ordering::AcqRel);
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn string_from_bits(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    string_obj_to_owned(obj)
}

fn is_exact_str(bits: u64) -> bool {
    obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_STRING)
}

fn category_name_from_bits(py: &PyToken<'_>, bits: u64, default: &str) -> String {
    if let Some(ptr) = obj_from_bits(bits).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_TYPE
    {
        return class_name_for_error(bits);
    }
    string_from_bits(py, bits).unwrap_or_else(|| default.to_string())
}

fn alloc_string_result(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn owned_int<'a, 'py>(py: &'a PyToken<'py>, value: i64) -> Option<ExceptionValue<'a, 'py>> {
    let int = ExceptionValue::adopt(py, int_bits_from_i64(py, value));
    (!exception_pending(py)).then_some(int)
}

fn owned_tuple<'a, 'py>(py: &'a PyToken<'py>, items: &[u64]) -> Option<ExceptionValue<'a, 'py>> {
    let ptr = alloc_tuple(py, items);
    (!ptr.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

/// A builtin exception class, borrowed from the runtime's class cache.
/// `None`: an exception is pending.
fn exception_class(py: &PyToken<'_>, kind: crate::BuiltinExc) -> Option<u64> {
    let bits = crate::builtins::exceptions::exception_class_bits(py, kind);
    (bits != 0 && !exception_pending(py)).then_some(bits)
}

fn call1<'a, 'py>(
    py: &'a PyToken<'py>,
    callable: u64,
    arg: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    let result = ExceptionValue::adopt(py, unsafe { call_callable1(py, callable, arg) });
    (!exception_pending(py)).then_some(result)
}

fn call_method1<'a, 'py>(
    py: &'a PyToken<'py>,
    receiver: u64,
    name: &[u8],
    arg: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    let name = owned_name(py, name)?;
    let method = ExceptionValue::adopt(py, molt_get_attr_name(receiver, name.bits()));
    if exception_pending(py) {
        return None;
    }
    call1(py, method.bits(), arg)
}

// ─── the loaded `warnings` module (CPython `get_warnings_attr`) ──────────────

/// CPython `get_warnings_attr(interp, attr, 0)`: the attribute of the
/// loaded `warnings` module, which is never imported here. `Ok(None)`: the
/// module is not loaded or lacks the attribute. `Err`: an exception is pending.
fn warnings_module_attr<'a, 'py>(
    py: &'a PyToken<'py>,
    attr: &[u8],
) -> Result<Option<ExceptionValue<'a, 'py>>, ()> {
    let name = owned_name(py, b"warnings").ok_or(())?;
    let module = ExceptionValue::adopt(py, molt_module_cache_get(name.bits()));
    if exception_pending(py) {
        return Err(());
    }
    if obj_from_bits(module.bits()).is_none() {
        return Ok(None);
    }
    let attr = owned_name(py, attr).ok_or(())?;
    match crate::builtins::platform::getattr_optional_bits(py, module.bits(), attr.bits()) {
        Ok(Some(bits)) => Ok(Some(ExceptionValue::adopt(py, bits))),
        Ok(None) => Ok(None),
        Err(_) => Err(()),
    }
}

/// One CPython default filter: action, category name and optional module.
type DefaultFilter = (&'static [u8], crate::BuiltinExc, Option<&'static [u8]>);

/// CPython `init_filters` for a release build: show `DeprecationWarning`
/// in `__main__`, and ignore it elsewhere along with
/// `PendingDeprecationWarning`, `ImportWarning` and `ResourceWarning`.
fn default_filters<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    const DEFAULTS: [DefaultFilter; 5] = [
        (
            b"default",
            crate::BuiltinExc::DeprecationWarning,
            Some(b"__main__"),
        ),
        (b"ignore", crate::BuiltinExc::DeprecationWarning, None),
        (
            b"ignore",
            crate::BuiltinExc::PendingDeprecationWarning,
            None,
        ),
        (b"ignore", crate::BuiltinExc::ImportWarning, None),
        (b"ignore", crate::BuiltinExc::ResourceWarning, None),
    ];
    let none = MoltObject::none().bits();
    let zero = MoltObject::from_int(0).bits();
    let mut rows = Vec::with_capacity(DEFAULTS.len());
    for (action, category, module) in DEFAULTS {
        let action = owned_name(py, action)?;
        let category = exception_class(py, category)?;
        let module = match module {
            Some(module) => Some(owned_name(py, module)?),
            None => None,
        };
        let module = module.as_ref().map_or(none, ExceptionValue::bits);
        rows.push(owned_tuple(
            py,
            &[action.bits(), none, category, module, zero],
        )?);
    }
    let rows: Vec<u64> = rows.iter().map(ExceptionValue::bits).collect();
    let list = crate::object::builders::alloc_list(py, &rows);
    (!list.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(list).bits()))
}

/// CPython `get_filters`: the loaded module's `filters`, which the state
/// then holds, else the state's own list. `None`: an exception is pending.
fn get_filters<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    let state = warnings_state(py);
    if let Some(filters) = warnings_module_attr(py, b"filters").ok()? {
        hold_state_object(py, &state.filters, filters.bits());
    }
    let filters = state_object(py, &state.filters, default_filters)?;
    if ListStorage::from_bits(py, filters.bits()).is_none() {
        return raise_exception(
            py,
            BuiltinExc::ValueError,
            "_warnings.filters must be a list",
        );
    }
    Some(filters)
}

/// CPython `get_once_registry`. `None`: an exception is pending.
fn get_once_registry<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    let state = warnings_state(py);
    if let Some(registry) = warnings_module_attr(py, b"onceregistry").ok()? {
        if dict_storage(py, registry.bits()).ok()?.is_none() {
            let message = format!(
                "_warnings.onceregistry must be a dict, not '{}'",
                type_name(py, obj_from_bits(registry.bits()))
            );
            return raise_exception(py, BuiltinExc::TypeError, &message);
        }
        hold_state_object(py, &state.once_registry, registry.bits());
        return Some(registry);
    }
    state_object(py, &state.once_registry, |py| {
        let dict = alloc_dict_with_pairs(py, &[]);
        (!dict.is_null()).then(|| ExceptionValue::adopt(py, MoltObject::from_ptr(dict).bits()))
    })
}

/// CPython `get_default_action`. `None`: an exception is pending.
fn get_default_action<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    let state = warnings_state(py);
    if let Some(action) = warnings_module_attr(py, b"defaultaction").ok()? {
        if string_from_bits(py, action.bits()).is_none() {
            let message = format!(
                "_warnings.defaultaction must be a string, not '{}'",
                type_name(py, obj_from_bits(action.bits()))
            );
            return raise_exception(py, BuiltinExc::TypeError, &message);
        }
        hold_state_object(py, &state.default_action, action.bits());
        return Some(action);
    }
    state_object(py, &state.default_action, |py| owned_name(py, b"default"))
}

// ─── filter selection (CPython `get_filter`) ─────────────────────────────────

/// CPython `check_matched`: `None` matches everything, an exact `str`
/// (an internal default filter) must equal `arg`, and any other object is a
/// pattern whose `match(arg)` decides. `None`: an exception is pending.
fn check_matched(py: &PyToken<'_>, pattern: u64, arg: u64) -> Option<bool> {
    if obj_from_bits(pattern).is_none() {
        return Some(true);
    }
    if is_exact_str(pattern) {
        let Some(arg_text) = string_from_bits(py, arg) else {
            let message = format!(
                "Can't compare str and {}",
                type_name(py, obj_from_bits(arg))
            );
            return raise_exception(py, BuiltinExc::TypeError, &message);
        };
        return Some(string_from_bits(py, pattern).as_deref() == Some(arg_text.as_str()));
    }
    let matched = call_method1(py, pattern, b"match", arg)?;
    crate::object::ops::truth_value(py, obj_from_bits(matched.bits())).ok()
}

/// The five fields of a filter item. `None`: an exception is pending.
fn filter_fields(py: &PyToken<'_>, item: u64, index: usize) -> Option<[u64; 5]> {
    let fields = obj_from_bits(item).as_ptr().and_then(|ptr| unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |fields| {
            <[u64; 5]>::try_from(fields).ok()
        })
        .flatten()
    });
    if fields.is_none() {
        let message = format!("_warnings.filters item {index} isn't a 5-tuple");
        return raise_exception(py, BuiltinExc::ValueError, &message);
    }
    fields
}

/// CPython `get_filter`: the action of the first filter matching the
/// warning, with that filter item; else the default action and `None`. Each
/// item is read afresh, since a pattern's `match` may change the list.
/// `None`: an exception is pending.
fn get_filter<'a, 'py>(
    py: &'a PyToken<'py>,
    category: u64,
    text: u64,
    lineno: i64,
    module: u64,
) -> Option<(ExceptionValue<'a, 'py>, ExceptionValue<'a, 'py>)> {
    let filters = get_filters(py)?;
    let storage = ListStorage::from_bits(py, filters.bits())?;
    let mut index = 0;
    while let Some(item) = storage.item(index) {
        // The tuple keeps its fields alive while the item is pinned.
        let [action, message, filter_category, filter_module, line] =
            filter_fields(py, item.bits(), index)?;
        if string_from_bits(py, action).is_none() {
            let message = format!(
                "action must be a string, not '{}'",
                type_name(py, obj_from_bits(action))
            );
            return raise_exception(py, BuiltinExc::TypeError, &message);
        }
        let good_message = check_matched(py, message, text)?;
        let good_module = check_matched(py, filter_module, module)?;
        let is_subclass =
            crate::builtins::type_ops::issubclass_runtime(py, category, filter_category)?;
        let Some(line) = to_i64(obj_from_bits(line)) else {
            let message = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(py, obj_from_bits(line))
            );
            return raise_exception(py, BuiltinExc::TypeError, &message);
        };
        if good_message && is_subclass && good_module && (line == 0 || line == lineno) {
            let item = ExceptionValue::pin(py, item.bits());
            return Some((ExceptionValue::pin(py, action), item));
        }
        index += 1;
    }
    if exception_pending(py) {
        return None;
    }
    let action = get_default_action(py)?;
    Some((action, ExceptionValue::adopt(py, MoltObject::none().bits())))
}

// ─── filter list mutation (CPython `_add_filter`, `resetwarnings`) ───────────

/// The index of the first element of `list` equal to `item`, comparing as
/// `list.remove` and `in` do. `Err`: an exception is pending.
fn filter_index(
    py: &PyToken<'_>,
    list: ListStorage<'_, '_>,
    item: u64,
) -> Result<Option<usize>, ()> {
    let mut index = 0;
    while let Some(element) = list.item(index) {
        match crate::object::ops_compare::compare_object_eq_bool(
            py,
            obj_from_bits(element.bits()),
            obj_from_bits(item),
        ) {
            crate::object::ops_compare::CompareBoolOutcome::True => return Ok(Some(index)),
            crate::object::ops_compare::CompareBoolOutcome::Error => return Err(()),
            _ => {}
        }
        index += 1;
    }
    if exception_pending(py) {
        Err(())
    } else {
        Ok(None)
    }
}

/// CPython `_add_filter`: put `fields` at the front of the filter list,
/// removing an equal filter first; when appending, add it only if no equal
/// filter exists. Then age every registry. `false`: an exception is pending.
fn add_filter(py: &PyToken<'_>, fields: [u64; 5], append: u64) -> bool {
    let Ok(append) = crate::object::ops::truth_value(py, obj_from_bits(append)) else {
        return false;
    };
    let Some(item) = owned_tuple(py, &fields) else {
        return false;
    };
    let Some(filters) = get_filters(py) else {
        return false;
    };
    let Some(list) = ListStorage::from_bits(py, filters.bits()) else {
        return false;
    };
    let Ok(existing) = filter_index(py, list, item.bits()) else {
        return false;
    };
    if append {
        if existing.is_none() {
            crate::molt_list_append(filters.bits(), item.bits());
        }
    } else {
        if let Some(index) = existing
            && !(list.promote() && list.remove_at(index))
        {
            return false;
        }
        crate::molt_list_insert(filters.bits(), MoltObject::from_int(0).bits(), item.bits());
    }
    if exception_pending(py) {
        return false;
    }
    filters_mutated(py);
    true
}

// ─── registries (CPython `already_warned`, `update_registry`) ────────────────

/// CPython `already_warned`: a registry recorded under an older filters
/// version is cleared and restamped; otherwise a true entry for `key` means
/// the warning was already shown. With `should_set`, an unshown `key` is
/// recorded. `Err`: an exception is pending.
fn already_warned(
    py: &PyToken<'_>,
    registry: *mut u8,
    key: u64,
    should_set: bool,
) -> Result<bool, ()> {
    let version = warnings_state(py).filters_version.load(Ordering::Acquire);
    let version_key = owned_name(py, b"version").ok_or(())?;
    let current = unsafe { dict_get_in_place(py, registry, version_key.bits())? }
        .is_some_and(|bits| obj_from_bits(bits).as_int() == Some(version));
    if current {
        if let Some(bits) = unsafe { dict_get_in_place(py, registry, key)? } {
            let bits = ExceptionValue::pin(py, bits);
            if crate::object::ops::truth_value(py, obj_from_bits(bits.bits()))? {
                return Ok(true);
            }
        }
    } else {
        unsafe { dict_clear_in_place(py, registry) };
        if exception_pending(py) {
            return Err(());
        }
        let version = owned_int(py, version).ok_or(())?;
        if !unsafe { dict_set_in_place(py, registry, version_key.bits(), version.bits()) } {
            return Err(());
        }
    }
    if should_set
        && !unsafe { dict_set_in_place(py, registry, key, MoltObject::from_bool(true).bits()) }
    {
        return Err(());
    }
    Ok(false)
}

/// CPython `update_registry` without `add_zero`, as the "once" and
/// "module" actions call it: record `(text, category)`. `Err`: an exception
/// is pending.
fn update_registry(
    py: &PyToken<'_>,
    registry: *mut u8,
    text: u64,
    category: u64,
) -> Result<bool, ()> {
    let key = owned_tuple(py, &[text, category]).ok_or(())?;
    already_warned(py, registry, key.bits(), true)
}

// ─── warning location (CPython 3.12 `setup_context`) ─────────────────────────

/// The number of Python frames `_warnings.warn` itself adds above its caller:
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
/// `"<string>"`); `registry` is its `__warningregistry__`, `None` when no
/// globals exist.
struct WarningLocation<'a, 'py> {
    filename: ExceptionValue<'a, 'py>,
    lineno: i64,
    module: ExceptionValue<'a, 'py>,
    registry: ExceptionValue<'a, 'py>,
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
    })
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

/// CPython `_formatwarnmsg_impl`: `"{filename}:{lineno}: {category}:
/// {message}\n"`, then the stripped source line when it is non-empty. A
/// `None` line is read through [`warning_source_line`], except for the made-up
/// file name `sys`. `None`: an exception is pending.
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
    let line = if !obj_from_bits(line).is_none() {
        crate::format_obj_str(py, obj_from_bits(line))
    } else if string_from_bits(py, filename).as_deref() == Some("sys") {
        String::new()
    } else {
        warning_source_line(py, filename, lineno)
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

fn warning_category_name(py: &PyToken<'_>, category: u64) -> Option<String> {
    let ptr = obj_from_bits(category).as_ptr()?;
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_TYPE => Some(class_name_for_error(category)),
        TYPE_ID_FOREIGN => {
            // Read canonical type metadata, not metaclass __getattribute__.
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

/// The `category.__name__` a formatted warning shows: a managed or native
/// class's name, or a name the caller passed as text.
fn display_category_name(py: &PyToken<'_>, category: u64) -> String {
    warning_category_name(py, category)
        .unwrap_or_else(|| category_name_from_bits(py, category, "UserWarning"))
}

// ─── emission (CPython `warn_explicit`) ──────────────────────────────────────

/// CPython `show_warning`, the display used while no `warnings` module
/// is loaded: the formatted warning is written to the `sys.stderr` object, so
/// a redirected stream receives it. Errors are discarded, as CPython clears
/// them.
fn show_warning(py: &PyToken<'_>, filename: u64, lineno: u64, text: u64, category: u64) {
    let category = display_category_name(py, category);
    let none = MoltObject::none().bits();
    if let Some(formatted) = format_warning(py, text, &category, filename, lineno, none) {
        write_sys_stderr(py, &formatted);
    }
    if exception_pending(py) {
        clear_exception(py);
    }
}

/// Write `text` with `sys.stderr.write`. A missing `sys.stderr` loses the
/// warning, as CPython reports.
fn write_sys_stderr(py: &PyToken<'_>, text: &str) {
    let stream = crate::builtins::modules::interpreter_sys_module(py)
        .and_then(|sys| warnings_attr_of(py, sys, b"stderr"));
    let Some(stream) = stream else {
        if !exception_pending(py) {
            eprintln!("lost sys.stderr");
        }
        return;
    };
    let Some(text) = owned_str(py, text) else {
        return;
    };
    drop(call_method1(py, stream.bits(), b"write", text.bits()));
}

/// `getattr(object, name)` without raising for a missing attribute.
fn warnings_attr_of<'a, 'py>(
    py: &'a PyToken<'py>,
    object: u64,
    name: &[u8],
) -> Option<ExceptionValue<'a, 'py>> {
    let name = owned_name(py, name)?;
    match crate::builtins::platform::getattr_optional_bits(py, object, name.bits()) {
        Ok(Some(bits)) => Some(ExceptionValue::adopt(py, bits)),
        _ => None,
    }
}

/// CPython `call_show_warning`: a loaded `warnings` module displays
/// through `_showwarnmsg(WarningMessage(...))`, so `showwarning`,
/// `formatwarning` and `catch_warnings(record=True)` apply; otherwise
/// [`show_warning`] writes to `sys.stderr`. `false`: an exception is pending.
fn call_show_warning(
    py: &PyToken<'_>,
    category: u64,
    text: u64,
    message: u64,
    filename: u64,
    lineno: u64,
) -> bool {
    let Ok(show) = warnings_module_attr(py, b"_showwarnmsg") else {
        return false;
    };
    let Some(show) = show else {
        show_warning(py, filename, lineno, text, category);
        return true;
    };
    if !crate::builtins::callable::is_callable_impl(py, show.bits()) {
        return raise_exception(
            py,
            BuiltinExc::TypeError,
            "warnings._showwarnmsg() must be set to a callable",
        );
    }
    let Ok(message_class) = warnings_module_attr(py, b"WarningMessage") else {
        return false;
    };
    let Some(message_class) = message_class else {
        return raise_exception(
            py,
            BuiltinExc::RuntimeError,
            "unable to get warnings.WarningMessage",
        );
    };
    let none = MoltObject::none().bits();
    let args = [message, category, filename, lineno, none, none];
    let record = ExceptionValue::adopt(py, unsafe {
        crate::call::bind::call_bind_borrowed(py, message_class.bits(), None, &args, &[], &[])
    });
    if exception_pending(py) {
        return false;
    }
    call1(py, show.bits(), record.bits()).is_some()
}

/// CPython `warn_explicit`: normalize the message,
/// consult the registry, select the filter action, record the warning, then
/// display it. `module` is `None` while the interpreter shuts down, which
/// ignores the warning; `registry` is a dictionary or `None`. `false`: an
/// exception is pending.
fn warn_explicit(
    py: &PyToken<'_>,
    category: u64,
    message: u64,
    filename: u64,
    lineno: i64,
    module: u64,
    registry: u64,
) -> bool {
    if obj_from_bits(module).is_none() {
        return true;
    }
    let registry = if obj_from_bits(registry).is_none() {
        None
    } else {
        match dict_storage(py, registry) {
            Ok(Some(registry)) => Some(registry),
            Ok(None) => {
                return raise_exception(
                    py,
                    BuiltinExc::TypeError,
                    "'registry' must be a dict or None",
                );
            }
            Err(()) => return false,
        }
    };
    let Some(warning_class) = exception_class(py, BuiltinExc::Warning) else {
        return false;
    };
    let Some(is_warning) =
        crate::builtins::type_ops::isinstance_runtime(py, message, warning_class)
    else {
        return false;
    };
    // The text keys the registries; the instance is what "error" raises and
    // what is displayed. Constructing it may run a callback that changes the
    // filters, so it precedes the filter search even for "ignore".
    let (text, category, message) = if is_warning {
        let text = ExceptionValue::adopt(py, molt_str_from_obj(message));
        if exception_pending(py) {
            return false;
        }
        let category = crate::builtins::type_ops::type_of_bits(py, message);
        (
            text,
            ExceptionValue::pin(py, category),
            ExceptionValue::pin(py, message),
        )
    } else {
        let Some(instance) = call1(py, category, message) else {
            return false;
        };
        (
            ExceptionValue::pin(py, message),
            ExceptionValue::pin(py, category),
            instance,
        )
    };
    let Some(lineno_obj) = owned_int(py, lineno) else {
        return false;
    };
    let Some(key) = owned_tuple(py, &[text.bits(), category.bits(), lineno_obj.bits()]) else {
        return false;
    };
    if let Some(registry) = registry {
        match already_warned(py, registry, key.bits(), false) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(()) => return false,
        }
    }
    let Some((action, item)) = get_filter(py, category.bits(), text.bits(), lineno, module) else {
        return false;
    };
    let action_text = string_from_bits(py, action.bits()).unwrap_or_default();
    match action_text.as_str() {
        "error" => {
            molt_raise(message.bits());
            return false;
        }
        "ignore" => return true,
        _ => {}
    }
    // Every action but "always" records the location in the registry.
    let mut already = false;
    if action_text != "always" {
        if let Some(registry) = registry {
            let marked = MoltObject::from_bool(true).bits();
            if !unsafe { dict_set_in_place(py, registry, key.bits(), marked) } {
                return false;
            }
        }
        let recorded = match action_text.as_str() {
            "once" => match registry {
                Some(registry) => update_registry(py, registry, text.bits(), category.bits()),
                None => {
                    let Some(once) = get_once_registry(py) else {
                        return false;
                    };
                    let Ok(Some(once)) = dict_storage(py, once.bits()) else {
                        return false;
                    };
                    update_registry(py, once, text.bits(), category.bits())
                }
            },
            "module" => match registry {
                Some(registry) => update_registry(py, registry, text.bits(), category.bits()),
                None => Ok(false),
            },
            "default" => Ok(false),
            _ => {
                let action = crate::format_obj(py, obj_from_bits(action.bits()));
                let item = crate::format_obj(py, obj_from_bits(item.bits()));
                if exception_pending(py) {
                    return false;
                }
                let message =
                    format!("Unrecognized action ({action}) in warnings.filters:\n {item}");
                return raise_exception(py, BuiltinExc::RuntimeError, &message);
            }
        };
        match recorded {
            Ok(recorded) => already = recorded,
            Err(()) => return false,
        }
    }
    if already {
        return true;
    }
    call_show_warning(
        py,
        category.bits(),
        text.bits(),
        message.bits(),
        filename,
        lineno_obj.bits(),
    )
}

/// CPython `get_category`: a `Warning` message's own class, else
/// `category` (`UserWarning` for `None`), which must subclass `Warning`.
/// `None`: an exception is pending.
fn get_category<'a, 'py>(
    py: &'a PyToken<'py>,
    message: u64,
    category: u64,
) -> Option<ExceptionValue<'a, 'py>> {
    let warning_class = exception_class(py, BuiltinExc::Warning)?;
    let category = if crate::builtins::type_ops::isinstance_runtime(py, message, warning_class)? {
        crate::builtins::type_ops::type_of_bits(py, message)
    } else if obj_from_bits(category).is_none() {
        exception_class(py, BuiltinExc::UserWarning)?
    } else {
        category
    };
    if crate::builtins::type_ops::issubclass_runtime(py, category, warning_class) == Some(true) {
        return Some(ExceptionValue::pin(py, category));
    }
    // CPython replaces the issubclass error with its own.
    if exception_pending(py) {
        clear_exception(py);
    }
    let message = format!(
        "category must be a Warning subclass, not '{}'",
        type_name(py, obj_from_bits(category))
    );
    raise_exception(py, BuiltinExc::TypeError, &message)
}

/// Runtime warnings share one filter/emission authority with warning intrinsics.
pub(crate) fn emit_deprecation_warning(py: &PyToken<'_>, message: &str) -> bool {
    emit_runtime_warning(py, message, crate::BuiltinExc::DeprecationWarning)
}

pub(crate) fn emit_runtime_warning(
    py: &PyToken<'_>,
    message: &str,
    category: crate::BuiltinExc,
) -> bool {
    let Some(category) = exception_class(py, category) else {
        return false;
    };
    let message = ExceptionValue::adopt(py, alloc_string_result(py, message));
    if exception_pending(py) {
        return false;
    }
    emit_warning_object(py, message.bits(), category, 1)
}

/// Borrowed message/category emission used by the public C API and runtime
/// operations (`PyErr_WarnEx`'s `do_warn`). The runtime operation adds no
/// Python frame, so `stacklevel` 1 names the executing Python frame. A cold
/// runtime never imports the `warnings` module.
pub(crate) fn emit_warning_object(
    py: &PyToken<'_>,
    message: u64,
    category: u64,
    stacklevel: i64,
) -> bool {
    let Some(location) = warning_location(py, stacklevel, 0) else {
        return false;
    };
    warn_explicit(
        py,
        category,
        message,
        location.filename.bits(),
        location.lineno,
        location.module.bits(),
        location.registry.bits(),
    )
}

// ─── public intrinsics ───────────────────────────────────────────────────────

/// `_warnings.warn`: validate the category, attribute the warning
/// `stacklevel` frames above the shim's caller, then emit it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn(
    message_bits: u64,
    category_bits: u64,
    stacklevel_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stacklevel = to_i64(obj_from_bits(stacklevel_bits)).unwrap_or(1);
        if let Some(category) = get_category(_py, message_bits, category_bits)
            && let Some(location) = warning_location(_py, stacklevel, WARN_ENTRY_FRAMES)
        {
            warn_explicit(
                _py,
                category.bits(),
                message_bits,
                location.filename.bits(),
                location.lineno,
                location.module.bits(),
                location.registry.bits(),
            );
        }
        MoltObject::none().bits()
    })
}

/// `_warnings.warn_explicit` with CPython's argument checks. The shim names an
/// omitted `module` after `filename`; an explicit `None` ignores the warning.
/// `module_globals` must be a dictionary or `None`; sources are read by file
/// name, so its loader is not consulted.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_warn_explicit(
    message_bits: u64,
    category_bits: u64,
    filename_bits: u64,
    lineno_bits: u64,
    module_bits: u64,
    registry_bits: u64,
    module_globals_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if string_from_bits(_py, filename_bits).is_none() {
            let message = format!(
                "warn_explicit() argument 3 must be str, not {}",
                type_name(_py, obj_from_bits(filename_bits))
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &message);
        }
        let Some(lineno) = to_i64(obj_from_bits(lineno_bits)) else {
            let message = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, obj_from_bits(lineno_bits))
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &message);
        };
        if !obj_from_bits(module_globals_bits).is_none() {
            match dict_storage(_py, module_globals_bits) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let message = format!(
                        "module_globals must be a dict, not '{}'",
                        type_name(_py, obj_from_bits(module_globals_bits))
                    );
                    return raise_exception::<_>(_py, BuiltinExc::TypeError, &message);
                }
                Err(()) => return MoltObject::none().bits(),
            }
        }
        warn_explicit(
            _py,
            category_bits,
            message_bits,
            filename_bits,
            lineno,
            module_bits,
            registry_bits,
        );
        MoltObject::none().bits()
    })
}

/// `warnings._formatwarnmsg_impl`: the CPython warning text, with the source
/// line read from `filename` when `line` is `None`.
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

/// `warnings.simplefilter` after its checks: CPython's
/// `_add_filter(action, None, category, None, lineno, append=append)`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_simplefilter(
    action_bits: u64,
    category_bits: u64,
    lineno_bits: u64,
    append_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        add_filter(
            _py,
            [action_bits, none, category_bits, none, lineno_bits],
            append_bits,
        );
        MoltObject::none().bits()
    })
}

/// `warnings.filterwarnings` after its checks: CPython's
/// `_add_filter(action, message, category, module, lineno, append=append)`.
/// `message` and `module` are compiled patterns or `None`; a plain `str`
/// matches exactly, as CPython's internal default filters do.
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
        add_filter(
            _py,
            [
                action_bits,
                message_bits,
                category_bits,
                module_bits,
                lineno_bits,
            ],
            append_bits,
        );
        MoltObject::none().bits()
    })
}

/// `warnings.resetwarnings`: empty the filter list and age every registry.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_resetwarnings() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(filters) = get_filters(_py) {
            crate::molt_list_clear(filters.bits());
            if !exception_pending(_py) {
                filters_mutated(_py);
            }
        }
        MoltObject::none().bits()
    })
}

/// `_warnings.filters`: the filter list the engine reads, which
/// `warnings.filters` aliases.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_filters_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        get_filters(_py).map_or_else(|| MoltObject::none().bits(), ExceptionValue::into_bits)
    })
}

/// `_warnings._onceregistry`: the registry of the "once" action for warnings
/// emitted without a registry, which `warnings.onceregistry` aliases.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_onceregistry_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        get_once_registry(_py).map_or_else(|| MoltObject::none().bits(), ExceptionValue::into_bits)
    })
}

/// `_warnings._filters_mutated`: age every registry after the filters change.
#[unsafe(no_mangle)]
pub extern "C" fn molt_warnings_filters_mutated() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        filters_mutated(_py);
        MoltObject::none().bits()
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

    /// One display a loaded module's `_showwarnmsg` received: the message
    /// text, the category, the file name and the line.
    type ShownWarning = (String, u64, String, i64);

    thread_local! {
        static SHOWN: RefCell<Vec<ShownWarning>> = const { RefCell::new(Vec::new()) };
        static CALLBACK_ERROR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    fn shown() -> Vec<ShownWarning> {
        SHOWN.with(|value| value.borrow_mut().drain(..).collect())
    }

    /// The fixture's `WarningMessage`: the record is the tuple of the four
    /// fields `call_show_warning` must pass, with `file` and `line` `None`.
    extern "C" fn warning_message(
        message: u64,
        category: u64,
        filename: u64,
        lineno: u64,
        file: u64,
        line: u64,
    ) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            assert!(obj_from_bits(file).is_none() && obj_from_bits(line).is_none());
            MoltObject::from_ptr(alloc_tuple(py, &[message, category, filename, lineno])).bits()
        })
    }

    /// The fixture's `_showwarnmsg`: record the display, or fail with the
    /// configured error.
    extern "C" fn show_warning_message(record: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let fields = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(record).as_ptr().unwrap(),
                    <[u64]>::to_vec,
                )
            }
            .unwrap();
            let text = ExceptionValue::adopt(py, molt_str_from_obj(fields[0]));
            SHOWN.with(|value| {
                value.borrow_mut().push((
                    string_from_bits(py, text.bits()).unwrap(),
                    fields[1],
                    string_from_bits(py, fields[2]).unwrap(),
                    to_i64(obj_from_bits(fields[3])).unwrap(),
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
            SHOWN.with(|value| value.borrow_mut().clear());
            crate::with_gil_entry_nopanic!(py, {
                unsafe { errors::PyErr_Clear() };
                clear_exception(py);
                if let Some(name) = attr_name_bits_from_bytes(py, b"warnings") {
                    let result = crate::builtins::modules::molt_module_cache_del(name);
                    dec_ref_bits(py, result);
                    dec_ref_bits(py, name);
                }
                clear_exception(py);
                molt_warnings_resetwarnings();
            });
        }
    }

    /// Start from a state that has never been used: the next access creates
    /// CPython's default filters.
    fn fresh_state(py: &PyToken<'_>) {
        crate::state::runtime_state::runtime_extension_state_clear_and_drop_key(
            crate::state::runtime_state::runtime_state(py),
            WARNINGS_STATE_KEY,
        );
    }

    fn class(py: &PyToken<'_>, name: crate::BuiltinExc) -> u64 {
        exception_class(py, name).unwrap()
    }

    fn set_action(py: &PyToken<'_>, action: &[u8]) {
        molt_warnings_resetwarnings();
        let action = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, action).unwrap());
        let result = molt_warnings_simplefilter(
            action.bits(),
            class(py, crate::BuiltinExc::Warning),
            MoltObject::from_int(0).bits(),
            MoltObject::from_bool(false).bits(),
        );
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
    }

    /// Install a `warnings` module that displays through the fixture's
    /// `_showwarnmsg` and `WarningMessage`.
    fn install_warnings_module<'a, 'py>(py: &'a PyToken<'py>) -> ExceptionValue<'a, 'py> {
        let name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"warnings").unwrap());
        let module = alloc_module_obj(py, name.bits());
        assert!(!module.is_null());
        let module = ExceptionValue::adopt(py, MoltObject::from_ptr(module).bits());
        let functions: [(&[u8], &str, *const (), u64); 2] = [
            (
                b"WarningMessage",
                "warnings_test_message",
                warning_message as *const (),
                6,
            ),
            (
                b"_showwarnmsg",
                "warnings_test_show",
                show_warning_message as *const (),
                1,
            ),
        ];
        for (attr, symbol, function, arity) in functions {
            let function = alloc_runtime_function_obj(py, runtime_fn_addr(symbol, function), arity);
            assert!(!function.is_null());
            let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
            set_module_attr(py, &module, attr, function.bits());
        }
        let result = crate::builtins::modules::molt_module_cache_set(name.bits(), module.bits());
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
        module
    }

    fn set_module_attr(py: &PyToken<'_>, module: &ExceptionValue<'_, '_>, attr: &[u8], value: u64) {
        let attr = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, attr).unwrap());
        let result = crate::molt_set_attr_name(module.bits(), attr.bits(), value);
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
    }

    /// The filter rows as (action, message, category, module, lineno), each
    /// object field as its text or class name.
    /// One filter row: action, message, category, module and lineno.
    type FilterRow = (String, Option<String>, String, Option<String>, i64);

    fn filter_rows(py: &PyToken<'_>, list: u64) -> Vec<FilterRow> {
        let ptr = obj_from_bits(list).as_ptr().unwrap();
        let rows =
            unsafe { crate::object::seq_access::snapshot(py, ptr, "filter rows test") }.unwrap();
        let text = |bits: u64| {
            (!obj_from_bits(bits).is_none()).then(|| string_from_bits(py, bits).unwrap())
        };
        rows.iter()
            .map(|&row| {
                let fields = unsafe {
                    crate::object::seq_access::with_immutable_tuple_slice(
                        obj_from_bits(row).as_ptr().unwrap(),
                        <[u64]>::to_vec,
                    )
                }
                .unwrap();
                (
                    string_from_bits(py, fields[0]).unwrap(),
                    text(fields[1]),
                    class_name_for_error(fields[2]),
                    text(fields[3]),
                    to_i64(obj_from_bits(fields[4])).unwrap(),
                )
            })
            .collect()
    }

    fn version(py: &PyToken<'_>) -> i64 {
        warnings_state(py).filters_version.load(Ordering::Acquire)
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
            let _outer = push_python_frame(py, "/app/outer.py", "outer_mod", 40);
            let _bootstrap = push_python_frame(
                py,
                "<frozen importlib._bootstrap>",
                "importlib._bootstrap",
                9,
            );
            let _inner = push_python_frame(py, "/app/inner.py", "inner_mod", 12);
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
            drop(location);

            // The warn entry point's own frame is not counted. An internal
            // newest frame is itself the location at stacklevel 1, and the walk
            // from it counts every frame.
            let location = warning_location(py, 1, WARN_ENTRY_FRAMES).expect("location");
            assert_eq!(
                string_field(py, &location.module).as_deref(),
                Some("importlib._bootstrap")
            );
            assert_eq!(location.lineno, 9);
            drop(location);
            let location = warning_location(py, 2, WARN_ENTRY_FRAMES).expect("location");
            assert_eq!(
                string_field(py, &location.module).as_deref(),
                Some("outer_mod")
            );
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
    fn first_use_installs_cpython_default_filters() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            fresh_state(py);
            let filters = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert!(!exception_pending(py));
            let row = |action: &str, category: &str, module: Option<&str>| {
                (
                    action.to_string(),
                    None,
                    category.to_string(),
                    module.map(str::to_string),
                    0,
                )
            };
            assert_eq!(
                filter_rows(py, filters.bits()),
                [
                    row("default", "DeprecationWarning", Some("__main__")),
                    row("ignore", "DeprecationWarning", None),
                    row("ignore", "PendingDeprecationWarning", None),
                    row("ignore", "ImportWarning", None),
                    row("ignore", "ResourceWarning", None),
                ]
            );
            // One list: every read returns the same object.
            let again = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert_eq!(again.bits(), filters.bits());
            // DeprecationWarning shows in `__main__`, nowhere else; other
            // categories fall through to the default action.
            let text = ExceptionValue::adopt(py, alloc_string_result(py, "text"));
            let action = |category: crate::BuiltinExc, module: &str| {
                let module = ExceptionValue::adopt(py, alloc_string_result(py, module));
                let (action, _) =
                    get_filter(py, class(py, category), text.bits(), 3, module.bits()).unwrap();
                string_from_bits(py, action.bits()).unwrap()
            };
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "__main__"),
                "default"
            );
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "__main__x"),
                "ignore"
            );
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "pkg"),
                "ignore"
            );
            assert_eq!(
                action(crate::BuiltinExc::PendingDeprecationWarning, "__main__"),
                "ignore"
            );
            assert_eq!(action(crate::BuiltinExc::ResourceWarning, "pkg"), "ignore");
            assert_eq!(action(crate::BuiltinExc::UserWarning, "pkg"), "default");
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn first_use_allocation_failure_leaves_the_state_unset() {
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            // Resolve the cached classes and names before memory is denied.
            fresh_state(py);
            drop(ExceptionValue::adopt(py, molt_warnings_filters_get()));
            fresh_state(py);
            crate::resource::set_tracker(Box::new(crate::resource::LimitedTracker::new(
                &crate::resource::ResourceLimits {
                    max_memory: Some(0),
                    ..Default::default()
                },
            )));
            let denied = RestoreTracker;
            let result = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert!(obj_from_bits(result.bits()).is_none());
            assert!(exception_pending(py));
            assert_eq!(warnings_state(py).filters.load(Ordering::Acquire), 0);
            drop(denied);
            clear_exception(py);
            let filters = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert!(!exception_pending(py));
            assert_eq!(filter_rows(py, filters.bits()).len(), 5);
        });
    }

    #[test]
    fn add_filter_moves_duplicates_and_ages_registries() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            let filters = ExceptionValue::adopt(py, molt_warnings_filters_get());
            molt_warnings_resetwarnings();
            let start = version(py);
            let user = class(py, crate::BuiltinExc::UserWarning);
            let none = MoltObject::none().bits();
            let zero = MoltObject::from_int(0).bits();
            let name = |text: &[u8]| {
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, text).unwrap())
            };
            let (ignore, error, always) = (name(b"ignore"), name(b"error"), name(b"always"));
            let simple = |action: u64, append: bool| {
                molt_warnings_simplefilter(
                    action,
                    user,
                    zero,
                    MoltObject::from_bool(append).bits(),
                );
                assert!(!exception_pending(py));
            };
            simple(ignore.bits(), false);
            simple(always.bits(), false);
            // Inserting an existing filter moves it to the front.
            simple(ignore.bits(), false);
            // Appending an existing filter changes nothing; a new one goes last.
            simple(always.bits(), true);
            simple(error.bits(), true);
            simple(error.bits(), true);
            let row = |action: &str| {
                (
                    action.to_string(),
                    None,
                    crate::BuiltinExc::UserWarning.to_string(),
                    None,
                    0,
                )
            };
            assert_eq!(
                filter_rows(py, filters.bits()),
                [row("ignore"), row("always"), row("error")]
            );
            // Every call ages the registries, even one that changed nothing.
            assert_eq!(version(py), start + 6);
            // `_filters_mutated` alone ages them too.
            molt_warnings_filters_mutated();
            assert_eq!(version(py), start + 7);
            // A plain str pattern matches exactly; the list is mutated in place.
            let exact = ExceptionValue::adopt(py, alloc_string_result(py, "exact"));
            let pkg = ExceptionValue::adopt(py, alloc_string_result(py, "pkg"));
            molt_warnings_filterwarnings(
                error.bits(),
                exact.bits(),
                class(py, crate::BuiltinExc::DeprecationWarning),
                pkg.bits(),
                MoltObject::from_int(5).bits(),
                MoltObject::from_bool(false).bits(),
            );
            assert!(!exception_pending(py));
            let again = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert_eq!(again.bits(), filters.bits());
            let action = |category: crate::BuiltinExc, text: &str, module: &str, line: i64| {
                let text = ExceptionValue::adopt(py, alloc_string_result(py, text));
                let module = ExceptionValue::adopt(py, alloc_string_result(py, module));
                let (action, _) =
                    get_filter(py, class(py, category), text.bits(), line, module.bits()).unwrap();
                string_from_bits(py, action.bits()).unwrap()
            };
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "exact", "pkg", 5),
                "error"
            );
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "exactly", "pkg", 5),
                "default"
            );
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "exact", "pkg.sub", 5),
                "default"
            );
            assert_eq!(
                action(crate::BuiltinExc::DeprecationWarning, "exact", "pkg", 6),
                "default"
            );
            // A filter category matches its subclasses.
            assert_eq!(
                action(crate::BuiltinExc::UserWarning, "any", "m", 1),
                "ignore"
            );
            assert_eq!(
                action(crate::BuiltinExc::RuntimeWarning, "any", "m", 1),
                "default"
            );
            molt_warnings_simplefilter(
                always.bits(),
                class(py, crate::BuiltinExc::Warning),
                zero,
                none,
            );
            assert_eq!(
                action(crate::BuiltinExc::RuntimeWarning, "any", "m", 1),
                "always"
            );
            molt_warnings_resetwarnings();
            assert!(filter_rows(py, filters.bits()).is_empty());
            let after_reset = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert_eq!(after_reset.bits(), filters.bits());
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn native_registries_follow_cpython_keys() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            let _module = install_warnings_module(py);
            let category = class(py, crate::BuiltinExc::UserWarning);
            let message = ExceptionValue::adopt(py, alloc_string_result(py, "dedup"));
            let filename = ExceptionValue::adopt(py, alloc_string_result(py, "/app/m.py"));
            let module = ExceptionValue::adopt(py, alloc_string_result(py, "m"));
            let registry = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits(),
            );
            let none = MoltObject::none().bits();
            let warn = |line, registry: u64| {
                let result = molt_warnings_warn_explicit(
                    message.bits(),
                    category,
                    filename.bits(),
                    MoltObject::from_int(line).bits(),
                    module.bits(),
                    registry,
                    none,
                );
                dec_ref_bits(py, result);
                exception_pending(py)
            };
            let lines = || shown().into_iter().map(|row| row.3).collect::<Vec<_>>();
            let dict = dict_storage(py, registry.bits()).unwrap().unwrap();
            let marked = |key: &[u64]| {
                let key = owned_tuple(py, key).unwrap();
                unsafe { dict_get_in_place(py, dict, key.bits()) }
                    .unwrap()
                    .is_some()
            };
            let line_key =
                |line: i64| marked(&[message.bits(), category, MoltObject::from_int(line).bits()]);
            let text_key = || marked(&[message.bits(), category]);
            // "default" records (text, category, lineno) per location.
            set_action(py, b"default");
            assert!(!warn(7, registry.bits()));
            assert!(!warn(7, registry.bits()));
            assert!(!warn(8, registry.bits()));
            assert_eq!(lines(), [7, 8]);
            assert!(line_key(7) && line_key(8) && !text_key());
            // A filter change clears the registry; "module" also records
            // (text, category), once per registry.
            set_action(py, b"module");
            assert!(!warn(7, registry.bits()));
            assert!(line_key(7) && !line_key(8) && text_key());
            assert!(!warn(8, registry.bits()));
            assert!(line_key(8));
            assert_eq!(lines(), [7]);
            // "once" uses the warning's own registry when it has one ...
            set_action(py, b"once");
            assert!(!warn(7, registry.bits()));
            assert!(!warn(9, registry.bits()));
            assert_eq!(lines(), [7]);
            // ... and the once registry when it has none.
            assert!(!warn(7, none));
            assert!(!warn(9, none));
            assert_eq!(lines(), [7]);
            let once = get_once_registry(py).unwrap();
            let once = dict_storage(py, once.bits()).unwrap().unwrap();
            let once_key = owned_tuple(py, &[message.bits(), category]).unwrap();
            assert!(
                unsafe { dict_get_in_place(py, once, once_key.bits()) }
                    .unwrap()
                    .is_some()
            );
            // A recorded location is suppressed before the filter is consulted.
            set_action(py, b"error");
            assert!(warn(9, registry.bits()));
            clear_exception(py);
            let key = owned_tuple(
                py,
                &[message.bits(), category, MoltObject::from_int(7).bits()],
            )
            .unwrap();
            let true_bits = MoltObject::from_bool(true).bits();
            assert!(unsafe { dict_set_in_place(py, dict, key.bits(), true_bits) });
            assert!(!warn(7, registry.bits()));
            // A registry that is not a dictionary is refused.
            let not_dict =
                ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_tuple(py, &[])).bits());
            assert!(warn(7, not_dict.bits()));
            clear_exception(py);
            assert!(shown().is_empty());
        });
    }

    #[test]
    fn native_emission_follows_the_loaded_module() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        let _reset = ResetWarnings;
        crate::with_gil_entry_nopanic!(py, {
            set_action(py, b"always");
            let module = install_warnings_module(py);
            // The loaded module's `filters` is the policy, and the engine's
            // list follows it.
            let ignore =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"ignore").unwrap());
            let none = MoltObject::none().bits();
            let zero = MoltObject::from_int(0).bits();
            let row = owned_tuple(
                py,
                &[
                    ignore.bits(),
                    none,
                    class(py, crate::BuiltinExc::Warning),
                    none,
                    zero,
                ],
            )
            .unwrap();
            let module_filters = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::object::builders::alloc_list(py, &[row.bits()])).bits(),
            );
            set_module_attr(py, &module, b"filters", module_filters.bits());
            let category = (&raw mut PyExc_DeprecationWarning).cast();
            let category_bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(category) }.unwrap();
            let category_bits = ExceptionValue::adopt(py, category_bits);
            let caller = push_python_frame(py, "/app/caller.py", "caller_mod", 21);
            push_python_frame(py, "/app/callee.py", "callee_mod", 5);
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"ignored".as_ptr(), 2) },
                0
            );
            assert!(shown().is_empty());
            let read = ExceptionValue::adopt(py, molt_warnings_filters_get());
            assert_eq!(read.bits(), module_filters.bits());
            // The module's `defaultaction` decides when no filter matches.
            molt_warnings_resetwarnings();
            let default =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"default").unwrap());
            set_module_attr(py, &module, b"defaultaction", default.bits());
            // `_showwarnmsg` receives a `WarningMessage` for the caller's
            // location, and the caller's registry records it.
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"live callback".as_ptr(), 2) },
                0
            );
            assert_eq!(
                shown(),
                [(
                    "live callback".to_string(),
                    category_bits.bits(),
                    "/app/caller.py".to_string(),
                    21
                )]
            );
            let registry_name = ExceptionValue::adopt(
                py,
                attr_name_bits_from_bytes(py, b"__warningregistry__").unwrap(),
            );
            let registry = unsafe {
                dict_get_in_place(
                    py,
                    dict_storage(py, caller).unwrap().unwrap(),
                    registry_name.bits(),
                )
            }
            .unwrap()
            .unwrap();
            let registry = dict_storage(py, registry).unwrap().unwrap();
            let text = ExceptionValue::adopt(py, alloc_string_result(py, "live callback"));
            let key = owned_tuple(
                py,
                &[
                    text.bits(),
                    category_bits.bits(),
                    MoltObject::from_int(21).bits(),
                ],
            )
            .unwrap();
            assert!(
                unsafe { dict_get_in_place(py, registry, key.bits()) }
                    .unwrap()
                    .is_some()
            );
            // A display failure is the warning call's failure.
            let failure = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::ValueError,
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
            assert_eq!(
                shown(),
                [(
                    "live failure".to_string(),
                    category_bits.bits(),
                    "/app/callee.py".to_string(),
                    5
                )]
            );
            CALLBACK_ERROR.with(|value| value.set(0));
            // A non-callable `_showwarnmsg` is refused as CPython refuses it.
            set_module_attr(py, &module, b"_showwarnmsg", none);
            assert_eq!(
                unsafe { errors::PyErr_WarnEx(category, c"no display".as_ptr(), 1) },
                -1
            );
            unsafe { errors::PyErr_Clear() };
            clear_exception(py);
            crate::builtins::frames::frame_stack_pop(py);
            crate::builtins::frames::frame_stack_pop(py);
        });
    }
}
