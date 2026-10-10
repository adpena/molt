//! Opt-in observations of the existing instance/dictionary storage owners.
//! No Python lookup, hashing, allocation, retain or release is performed here.
//! Raw keys are inspected only while the caller keeps their dictionary alive.

use crate::*;
use std::sync::OnceLock;

#[inline(always)]
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_FIELD").is_ok())
}

fn name_filter() -> Option<&'static [u8]> {
    static NAME: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::var("MOLT_DEBUG_FIELD_NAME")
            .ok()
            .map(String::into_bytes)
    })
    .as_deref()
}

unsafe fn string_bytes_view<'a>(bits: u64) -> Option<&'a [u8]> {
    unsafe {
        let string = obj_from_bits(bits).as_ptr()?;
        (object_type_id(string) == TYPE_ID_STRING)
            .then(|| std::slice::from_raw_parts(string_bytes(string), string_len(string)))
    }
}

#[inline(always)]
pub(crate) unsafe fn field_enabled(py: &PyToken<'_>, object: *mut u8, offset: usize) -> bool {
    enabled() && unsafe { observe_field_enabled(py, object, offset) }
}

#[cold]
#[inline(never)]
unsafe fn observe_field_enabled(py: &PyToken<'_>, object: *mut u8, offset: usize) -> bool {
    let Some(filter) = name_filter() else {
        return true;
    };
    unsafe {
        object_class_bits(object) != 0
            && super::field_at_offset(py, object, offset)
                .is_some_and(|field| string_bytes_view(field.name) == Some(filter))
    }
}

#[inline(always)]
pub(crate) unsafe fn lookup(py: &PyToken<'_>, object: *mut u8, name: u64, offset: Option<usize>) {
    if enabled() {
        unsafe { observe_lookup(py, object, name, offset) };
    }
}

#[cold]
#[inline(never)]
unsafe fn observe_lookup(py: &PyToken<'_>, object: *mut u8, name: u64, offset: Option<usize>) {
    unsafe {
        let bytes = string_bytes_view(name);
        if name_filter().is_some_and(|filter| bytes != Some(filter)) {
            return;
        }
        eprintln!(
            "[field_lookup] object=0x{:x} name={:?} offset={offset:?}",
            object as usize,
            bytes.map(String::from_utf8_lossy)
        );
        observe_dictionary(
            py,
            "lookup_storage",
            object,
            instance_dict_bits(object),
            Some(name),
            None,
        );
    }
}

/// Observe storage, never derive a semantic answer. `value=None` means the
/// caller supplied no result (or its lookup missed); raw entries below expose
/// whether storage agrees without invoking hash/equality or changing caches.
/// Object zero denotes a direct dictionary operation without an instance owner.
#[inline(always)]
pub(crate) unsafe fn dictionary(
    py: &PyToken<'_>,
    event: &str,
    object: *mut u8,
    dictionary: u64,
    name: Option<u64>,
    value: Option<u64>,
) {
    if enabled() {
        unsafe { observe_dictionary(py, event, object, dictionary, name, value) };
    }
}

/// Delay representation lookup until diagnostics are enabled. Callers must not
/// compute instance_dict_bits solely as a diagnostic argument on the hot path.
#[inline(always)]
pub(crate) unsafe fn instance(py: &PyToken<'_>, event: &str, object: *mut u8) {
    if enabled() {
        unsafe { observe_instance(py, event, object) };
    }
}

#[cold]
#[inline(never)]
unsafe fn observe_instance(py: &PyToken<'_>, event: &str, object: *mut u8) {
    unsafe { observe_dictionary(py, event, object, instance_dict_bits(object), None, None) };
}

/// The caller already collected these owners for the real transition. A disabled
/// observation performs no extra field walk; it also never dereferences slots.
#[inline(always)]
pub(crate) unsafe fn fields(
    py: &PyToken<'_>,
    event: &str,
    object: *mut u8,
    dictionary: u64,
    fields: &[(u64, *mut u64, u64)],
) {
    if enabled() {
        unsafe { observe_fields(py, event, object, dictionary, fields) };
    }
}

#[cold]
#[inline(never)]
unsafe fn observe_fields(
    py: &PyToken<'_>,
    event: &str,
    object: *mut u8,
    dictionary: u64,
    fields: &[(u64, *mut u64, u64)],
) {
    for &(name, _, value) in fields {
        unsafe { observe_dictionary(py, event, object, dictionary, Some(name), Some(value)) };
    }
}

/// Resolve a replacement key only inside the enabled observation. Copy its
/// bits before borrowing dictionary backing for output; no mutable Vec view
/// crosses the observer call.
#[inline(always)]
pub(crate) unsafe fn replacement(
    py: &PyToken<'_>,
    event: &str,
    dictionary: *mut u8,
    value_index: usize,
    value: u64,
) {
    if enabled() {
        unsafe { observe_replacement(py, event, dictionary, value_index, value) };
    }
}

#[cold]
#[inline(never)]
unsafe fn observe_replacement(
    py: &PyToken<'_>,
    event: &str,
    dictionary: *mut u8,
    value_index: usize,
    value: u64,
) {
    unsafe {
        let key = {
            let order = crate::builtins::containers::dict_entries_ptr(dictionary)
                .as_ref()
                .expect("live dictionary replacement requires backing");
            order[value_index].key
        };
        observe_dictionary(
            py,
            event,
            std::ptr::null_mut(),
            MoltObject::from_ptr(dictionary).bits(),
            Some(key),
            Some(value),
        );
    }
}

/// All views remain inside this synchronous observation. The caller holds the
/// GIL and live dictionary/name custody; call sites run before mutable backing
/// borrows are created or after their last use. Formatting uses only integers
/// and Rust byte views: no Python callback, retain/release, hash, allocation or
/// cache mutation can invalidate the views. A supplied value is printed as bits
/// only and is never dereferenced, including after a callback-capable mutation.
#[cold]
#[inline(never)]
unsafe fn observe_dictionary(
    py: &PyToken<'_>,
    event: &str,
    object: *mut u8,
    dictionary: u64,
    name: Option<u64>,
    value: Option<u64>,
) {
    unsafe {
        let requested = name.and_then(|name| string_bytes_view(name));
        if let Some(filter) = name_filter()
            && name.is_some()
            && requested != Some(filter)
        {
            return;
        }
        let selected = requested.or_else(name_filter);
        let dict = obj_from_bits(dictionary)
            .as_ptr()
            .filter(|&dict| object_type_id(dict) == TYPE_ID_DICT);
        let order =
            dict.and_then(|dict| crate::builtins::containers::dict_entries_ptr(dict).as_ref());
        // A direct clear/swap has no name operand. With a filter it is relevant
        // only when the old dictionary physically owns a matching key. Instance
        // publication also logs empty replacements, correlated by object address.
        if object.is_null()
            && name.is_none()
            && selected.is_some()
            && !order.is_some_and(|order| {
                order
                    .iter()
                    .filter(|row| row.hash.is_some())
                    .any(|row| string_bytes_view(row.key) == selected)
            })
        {
            return;
        }
        let object_rc =
            (!object.is_null()).then(|| (*header_from_obj_ptr(object)).ref_count_snapshot());
        let dict_rc = dict.map(|dict| (*header_from_obj_ptr(dict)).ref_count_snapshot());
        let name_state = name
            .and_then(|name| obj_from_bits(name).as_ptr())
            .filter(|&name| object_type_id(name) == TYPE_ID_STRING)
            .map(crate::object::object_state);
        eprintln!(
            "[field_dictionary] event={event} object=0x{:x} object_rc={object_rc:?} dictionary=0x{dictionary:x} dict_rc={dict_rc:?} name={:?} name_bits={name:x?} name_state={name_state:x?} value={value:x?} entries={} pending={}",
            object as usize,
            selected.map(String::from_utf8_lossy),
            dict.map_or(0, |dict| crate::dict_len(dict)),
            exception_pending(py),
        );
        let Some(_) = dict else {
            return;
        };
        let Some(order) = order else {
            return;
        };
        for (index, row) in order
            .iter()
            .enumerate()
            .filter(|(_, row)| row.hash.is_some())
        {
            let pair = [row.key, row.value];
            let Some(bytes) = string_bytes_view(pair[0]) else {
                continue;
            };
            if selected.is_some_and(|selected| selected != bytes) {
                continue;
            }
            let key = obj_from_bits(pair[0]).as_ptr().unwrap();
            eprintln!(
                "[field_dictionary_entry] event={event} dictionary=0x{dictionary:x} index={index} name={:?} key=0x{:x} value=0x{:x} stored_hash={:x?} key_state=0x{:x}",
                String::from_utf8_lossy(bytes),
                pair[0],
                pair[1],
                row.hash.map(crate::StoredHash::get),
                crate::object::object_state(key),
            );
        }
    }
}
