use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};

const ATTRIBUTES_OBJECT_SLOT_COUNT: usize = 5;

/// Result-level inline cache entry for attribute lookups.
/// Caches the full lookup result alongside the attribute name to skip
/// MRO traversal when the global type version hasn't changed.
pub(super) struct AttrICEntry {
    /// Cached attribute name string (NaN-boxed bits)
    pub(super) name_bits: u64,
    /// Cached lookup result (the attribute value, NaN-boxed bits)
    pub(super) result_bits: u64,
    /// Global type version when result was cached
    pub(super) type_version: u64,
    /// type_id of the object this was cached for
    pub(super) obj_type_id: u32,
    /// Logical lookup owner for result IC correctness: the class object for
    /// TYPE_ID_TYPE, otherwise the receiver's class object.
    pub(super) class_bits: u64,
}

impl AttrICEntry {
    pub(super) fn retain_owned_refs(&self, _py: &PyToken<'_>) {
        for bits in [self.name_bits, self.result_bits, self.class_bits] {
            if bits != 0 {
                inc_ref_bits(_py, bits);
            }
        }
    }

    pub(super) fn release_owned_refs(&self, _py: &PyToken<'_>) {
        for bits in [self.name_bits, self.result_bits, self.class_bits] {
            if bits != 0 {
                dec_ref_bits(_py, bits);
            }
        }
    }
}

pub(crate) struct AttributesRuntimeState {
    pub(super) property_docs: Mutex<HashMap<PtrSlot, u64>>,
    pub(super) property_doc_name: AtomicU64,
    pub(super) attr_site_name_cache: Mutex<HashMap<u64, u64>>,
    pub(super) generic_alias_mro_entries: AtomicU64,
    pub(super) attr_ic_result_cache: Mutex<HashMap<u64, AttrICEntry>>,
    pub(super) bytes_fromhex: AtomicU64,
    pub(super) bytearray_fromhex: AtomicU64,
    pub(super) memoryview_from_flags: AtomicU64,
}

impl AttributesRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            property_docs: Mutex::new(HashMap::new()),
            property_doc_name: AtomicU64::new(0),
            attr_site_name_cache: Mutex::new(HashMap::new()),
            generic_alias_mro_entries: AtomicU64::new(0),
            attr_ic_result_cache: Mutex::new(HashMap::new()),
            bytes_fromhex: AtomicU64::new(0),
            bytearray_fromhex: AtomicU64::new(0),
            memoryview_from_flags: AtomicU64::new(0),
        }
    }

    pub(super) fn object_slots(&self) -> [&AtomicU64; ATTRIBUTES_OBJECT_SLOT_COUNT] {
        [
            &self.property_doc_name,
            &self.generic_alias_mro_entries,
            &self.bytes_fromhex,
            &self.bytearray_fromhex,
            &self.memoryview_from_flags,
        ]
    }
}

static ATTR_LOOKUP_TRACE_DEPTH: AtomicUsize = AtomicUsize::new(0);
pub(super) static ATTR_LOOKUP_TRACE_LINES: AtomicUsize = AtomicUsize::new(0);

/// Cached `MOLT_TRACE_ATTR_LOOKUP` flag.
///
/// `attr_lookup_ptr` is on the hot path of EVERY attribute access (`obj.attr`,
/// `self.x`, method binding, dataclass field reads, ...) - one of the most
/// frequent operations in any Python program. Reading the env var directly per
/// lookup (`std::env::var`) takes the libc environ lock (`__findenv_locked`)
/// and heap-allocates a `String` each time; profiling a dataclass-heavy ETL
/// loop showed `getenv` internals as the dominant frame. Cache it once.
#[inline]
pub(super) fn trace_attr_lookup_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_ATTR_LOOKUP").as_deref() == Ok("1"))
}

/// Cached `MOLT_DEBUG_BOUND_METHOD` flag. Read during bound-method resolution
/// inside the attribute-lookup path (every `obj.method`), so reading the env
/// var directly there takes the libc environ lock per method bind.
#[inline]
pub(crate) fn debug_bound_method_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var_os("MOLT_DEBUG_BOUND_METHOD").is_some())
}

pub(super) fn attributes_state(_py: &PyToken<'_>) -> &'static AttributesRuntimeState {
    &crate::runtime_state(_py).attributes
}

pub(super) fn attr_ic_result_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, AttrICEntry>> {
    &attributes_state(_py).attr_ic_result_cache
}

pub(super) struct AttrLookupTraceGuard {
    enabled: bool,
}

impl AttrLookupTraceGuard {
    pub(super) fn new(enabled: bool) -> Self {
        if enabled {
            ATTR_LOOKUP_TRACE_DEPTH.fetch_add(1, Ordering::Relaxed);
        }
        Self { enabled }
    }

    pub(super) fn depth(&self) -> usize {
        if self.enabled {
            ATTR_LOOKUP_TRACE_DEPTH.load(Ordering::Relaxed)
        } else {
            0
        }
    }
}

impl Drop for AttrLookupTraceGuard {
    fn drop(&mut self) {
        if self.enabled {
            ATTR_LOOKUP_TRACE_DEPTH.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

pub(super) fn is_task_trampoline_attr_name(attr_name: &str) -> bool {
    matches!(
        attr_name,
        "__molt_is_generator__" | "__molt_is_coroutine__" | "__molt_is_async_generator__"
    )
}

pub(super) fn property_docs(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, u64>> {
    &attributes_state(_py).property_docs
}

fn clear_property_docs(_py: &PyToken<'_>, attributes: &AttributesRuntimeState) {
    let mut guard = attributes.property_docs.lock().unwrap();
    let old = std::mem::take(&mut *guard);
    drop(guard);
    for (_ptr, bits) in old {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

pub(super) fn attr_site_name_cache(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &attributes_state(_py).attr_site_name_cache
}

fn clear_attr_site_name_cache(_py: &PyToken<'_>, attributes: &AttributesRuntimeState) {
    let mut cache = attributes
        .attr_site_name_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (_site, bits) in cache.drain() {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
    // Also clear the result IC cache.
    let mut rc = attributes
        .attr_ic_result_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (_site, entry) in rc.drain() {
        entry.release_owned_refs(_py);
    }
}

pub(crate) fn attributes_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) {
    crate::gil_assert();
    let attributes = &state.attributes;
    clear_attr_site_name_cache(_py, attributes);
    clear_property_docs(_py, attributes);
    let slots = attributes.object_slots();
    crate::state::cache::clear_atomic_slots(_py, &slots);
}
