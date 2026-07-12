use crate::{PyToken, runtime_state};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Mutex, OnceLock};

pub(super) fn debug_oom() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| matches!(std::env::var("MOLT_DEBUG_OOM").ok().as_deref(), Some("1")))
}

thread_local! {
    pub(crate) static EXCEPTION_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    pub(crate) static EXCEPTION_STACK_BASELINE: Cell<usize> = const { Cell::new(0) };
    pub(crate) static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static GENERATOR_EXCEPTION_STACKS: RefCell<HashMap<usize, Vec<u64>>> =
        RefCell::new(HashMap::new());
    pub(crate) static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
    pub(crate) static TASK_RAISE_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

const EXCEPTIONS_OBJECT_SLOT_COUNT: usize = 26;

pub(crate) struct ExceptionsRuntimeState {
    pub(super) errno_attr_name: AtomicU64,
    pub(super) strerror_attr_name: AtomicU64,
    pub(super) filename_attr_name: AtomicU64,
    pub(super) characters_written_attr_name: AtomicU64,
    pub(super) exc_group_message_name: AtomicU64,
    pub(super) exc_group_exceptions_name: AtomicU64,
    pub(super) unicode_encoding_attr_name: AtomicU64,
    pub(super) unicode_object_attr_name: AtomicU64,
    pub(super) unicode_start_attr_name: AtomicU64,
    pub(super) unicode_end_attr_name: AtomicU64,
    pub(super) unicode_reason_attr_name: AtomicU64,
    pub(super) exception_with_traceback: AtomicU64,
    pub(super) base_exception_class_cache: AtomicU64,
    pub(super) exception_class_cache: AtomicU64,
    pub(super) key_error_class_cache: AtomicU64,
    pub(super) index_error_class_cache: AtomicU64,
    pub(super) value_error_class_cache: AtomicU64,
    pub(super) type_error_class_cache: AtomicU64,
    pub(super) runtime_error_class_cache: AtomicU64,
    pub(super) stop_iteration_class_cache: AtomicU64,
    pub(super) stop_async_iteration_class_cache: AtomicU64,
    pub(super) assertion_error_class_cache: AtomicU64,
    pub(super) import_error_class_cache: AtomicU64,
    pub(super) name_error_class_cache: AtomicU64,
    pub(super) unbound_local_error_class_cache: AtomicU64,
    pub(super) not_implemented_error_class_cache: AtomicU64,
}

impl ExceptionsRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            errno_attr_name: AtomicU64::new(0),
            strerror_attr_name: AtomicU64::new(0),
            filename_attr_name: AtomicU64::new(0),
            characters_written_attr_name: AtomicU64::new(0),
            exc_group_message_name: AtomicU64::new(0),
            exc_group_exceptions_name: AtomicU64::new(0),
            unicode_encoding_attr_name: AtomicU64::new(0),
            unicode_object_attr_name: AtomicU64::new(0),
            unicode_start_attr_name: AtomicU64::new(0),
            unicode_end_attr_name: AtomicU64::new(0),
            unicode_reason_attr_name: AtomicU64::new(0),
            exception_with_traceback: AtomicU64::new(0),
            base_exception_class_cache: AtomicU64::new(0),
            exception_class_cache: AtomicU64::new(0),
            key_error_class_cache: AtomicU64::new(0),
            index_error_class_cache: AtomicU64::new(0),
            value_error_class_cache: AtomicU64::new(0),
            type_error_class_cache: AtomicU64::new(0),
            runtime_error_class_cache: AtomicU64::new(0),
            stop_iteration_class_cache: AtomicU64::new(0),
            stop_async_iteration_class_cache: AtomicU64::new(0),
            assertion_error_class_cache: AtomicU64::new(0),
            import_error_class_cache: AtomicU64::new(0),
            name_error_class_cache: AtomicU64::new(0),
            unbound_local_error_class_cache: AtomicU64::new(0),
            not_implemented_error_class_cache: AtomicU64::new(0),
        }
    }

    pub(super) fn object_slots(&self) -> [&AtomicU64; EXCEPTIONS_OBJECT_SLOT_COUNT] {
        [
            &self.errno_attr_name,
            &self.strerror_attr_name,
            &self.filename_attr_name,
            &self.characters_written_attr_name,
            &self.exc_group_message_name,
            &self.exc_group_exceptions_name,
            &self.unicode_encoding_attr_name,
            &self.unicode_object_attr_name,
            &self.unicode_start_attr_name,
            &self.unicode_end_attr_name,
            &self.unicode_reason_attr_name,
            &self.exception_with_traceback,
            &self.base_exception_class_cache,
            &self.exception_class_cache,
            &self.key_error_class_cache,
            &self.index_error_class_cache,
            &self.value_error_class_cache,
            &self.type_error_class_cache,
            &self.runtime_error_class_cache,
            &self.stop_iteration_class_cache,
            &self.stop_async_iteration_class_cache,
            &self.assertion_error_class_cache,
            &self.import_error_class_cache,
            &self.name_error_class_cache,
            &self.unbound_local_error_class_cache,
            &self.not_implemented_error_class_cache,
        ]
    }
}

pub(super) static STOPASYNC_BT_PRINTED: AtomicBool = AtomicBool::new(false);

pub(super) fn exceptions_state(_py: &PyToken<'_>) -> &'static ExceptionsRuntimeState {
    &runtime_state(_py).exceptions
}

#[inline]
pub(super) fn debug_exception_flow() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_FLOW").as_deref() == Ok("1"))
}

#[inline]
pub(super) fn debug_exception_clear() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_CLEAR").as_deref() == Ok("1"))
}

#[inline]
pub(super) fn debug_exception_raise() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_RAISE").as_deref() == Ok("1"))
}

#[inline]
pub(super) fn debug_exception_pending() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_PENDING").as_deref() == Ok("1"))
}

#[inline]
pub(super) fn debug_exception_rc() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTION_RC").as_deref() == Ok("1"))
}

#[inline]
pub(super) fn trace_exception_stack() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_EXCEPTION_STACK").as_deref() == Ok("1"))
}

/// Cached `MOLT_DEBUG_EXCEPTIONS` flag. `record_exception_with_caller_frame`
/// runs on every exception raise, so reading the env var directly there takes
/// the libc environ lock and heap-allocates per raise — a measurable tax in
/// exception-heavy loops. Cache it like the sibling flags above.
#[inline]
pub(super) fn debug_exceptions() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_EXCEPTIONS").as_deref() == Ok("1"))
}

thread_local! {
    static EXCEPTION_CLEAR_REASON: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

thread_local! {
    /// Stashed col_offset/end_col_offset from the frame stack at the point
    /// an exception was recorded.  The traceback formatter reads this to
    /// produce correct caret annotations.  (-1, -1) = unknown.
    pub(super) static LAST_EXCEPTION_COL: RefCell<(i64, i64)> = const { RefCell::new((-1, -1)) };
}

pub(crate) fn exception_clear_reason_set(reason: &'static str) {
    EXCEPTION_CLEAR_REASON.with(|cell| {
        *cell.borrow_mut() = Some(reason);
    });
}

pub(super) fn exception_clear_reason_take() -> Option<&'static str> {
    EXCEPTION_CLEAR_REASON.with(|cell| cell.borrow_mut().take())
}

pub(crate) mod internals {
    use super::{HashMap, Mutex};
    use crate::{PyToken, runtime_state};

    pub(crate) fn module_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).module_cache
    }

    pub(crate) fn exception_type_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<String, u64>> {
        &runtime_state(_py).exception_type_cache
    }
}
