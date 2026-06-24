use crate::PyToken;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use molt_obj_model::MoltObject;

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::exceptions::{exception_matches_builtin_name, molt_exception_last_pending};
use crate::{
    FIELD_OFFSET_IC_HIT_COUNT, FIELD_OFFSET_IC_MISS_COUNT, TYPE_ID_CALL_ITER, TYPE_ID_CLASSMETHOD,
    TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW,
    TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE, TYPE_ID_EXCEPTION, TYPE_ID_FILE_HANDLE,
    TYPE_ID_FILTER, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_ITER, TYPE_ID_LIST, TYPE_ID_MAP,
    TYPE_ID_MODULE, TYPE_ID_OBJECT, TYPE_ID_PROPERTY, TYPE_ID_REVERSED, TYPE_ID_STATICMETHOD,
    TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, TYPE_ID_ZIP, alloc_dict_with_pairs,
    alloc_function_obj, alloc_property_obj, alloc_string, alloc_tuple, attr_lookup_ptr,
    builtin_class_method_bits, builtin_classes, builtin_func_bits, call_callable1, call_callable3,
    call_function_obj1, class_bases_bits, class_bases_vec, class_dict_bits,
    class_layout_version_bits, class_mro_ref, class_mro_vec, class_name_bits, class_name_for_error,
    classmethod_func_bits, clear_exception, dataclass_desc_ptr, dataclass_dict_bits,
    dataclass_fields_ref, dataclass_set_dict_bits, dec_ref_bits, dict_get_in_place, dict_order,
    dict_set_in_place, exception_dict_bits, exception_kind_bits, exception_last_bits_noinc,
    exception_pending, exception_stack_pop, exception_stack_push, inc_ref_bits, init_atomic_bits,
    instance_dict_bits, instance_set_dict_bits, intern_static_name, is_builtin_class_bits,
    is_missing_bits, is_truthy, issubclass_bits, maybe_ptr_from_bits, module_dict_bits,
    molt_awaitable_await, molt_bound_method_new, molt_function_get_code, molt_function_get_globals,
    molt_iter, molt_iter_next, obj_eq, obj_from_bits, object_class_bits, object_field_get_ptr_raw,
    object_set_class_bits, object_type_id, profile_hit_unchecked, property_get_bits,
    raise_exception, runtime_state, seq_vec_ref, staticmethod_func_bits, string_bytes, string_len,
    string_obj_to_owned, type_name, type_of_bits,
};

const ATTR_NAME_INLINE_CAP: usize = 32;

enum AttrNameCacheKey {
    Inline {
        len: u8,
        bytes: [u8; ATTR_NAME_INLINE_CAP],
    },
    Heap(Vec<u8>),
}

impl AttrNameCacheKey {
    fn new(bytes: &[u8]) -> Self {
        if bytes.len() <= ATTR_NAME_INLINE_CAP {
            let mut inline = [0u8; ATTR_NAME_INLINE_CAP];
            inline[..bytes.len()].copy_from_slice(bytes);
            Self::Inline {
                len: bytes.len() as u8,
                bytes: inline,
            }
        } else {
            Self::Heap(bytes.to_vec())
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Inline { len, bytes } => &bytes[..usize::from(*len)],
            Self::Heap(bytes) => bytes.as_slice(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ATTR_NAME_INLINE_CAP, AttrNameCacheKey, clear_attr_tls_caches, descriptor_cache_lookup,
        descriptor_cache_store,
    };
    use crate::{MoltObject, alloc_string, dec_ref_bits, obj_from_bits};
    use std::sync::atomic::Ordering;

    fn heap_refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("expected heap bits");
        unsafe {
            (*crate::object::header_from_obj_ptr(ptr))
                .ref_count
                .load(Ordering::Acquire)
        }
    }

    #[test]
    fn attr_name_cache_key_inlines_common_attr_names() {
        let key = AttrNameCacheKey::new(b"__molt_arg_names__");

        assert!(matches!(key, AttrNameCacheKey::Inline { .. }));
        assert_eq!(key.as_slice(), b"__molt_arg_names__");
    }

    #[test]
    fn attr_name_cache_key_preserves_long_names() {
        let bytes = vec![b'x'; ATTR_NAME_INLINE_CAP + 1];
        let key = AttrNameCacheKey::new(&bytes);

        assert!(matches!(key, AttrNameCacheKey::Heap(_)));
        assert_eq!(key.as_slice(), bytes.as_slice());
    }

    #[test]
    fn descriptor_cache_store_owns_released_heap_bits() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_attr_tls_caches(_py);

            let class_ptr = alloc_string(_py, b"descriptor-cache-class-owner");
            assert!(!class_ptr.is_null());
            let attr_ptr = alloc_string(_py, b"cached_attr");
            assert!(!attr_ptr.is_null());
            let first_ptr = alloc_string(_py, b"first-cached-value");
            assert!(!first_ptr.is_null());
            let second_ptr = alloc_string(_py, b"second-cached-value");
            assert!(!second_ptr.is_null());

            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            let first_bits = MoltObject::from_ptr(first_ptr).bits();
            let second_bits = MoltObject::from_ptr(second_ptr).bits();

            let class_before = heap_refcount(class_bits);
            let first_before = heap_refcount(first_bits);
            let second_before = heap_refcount(second_bits);

            descriptor_cache_store(_py, class_bits, attr_bits, 1, None, Some(first_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before + 1);
            assert_eq!(heap_refcount(second_bits), second_before);
            let cached = descriptor_cache_lookup(_py, class_bits, attr_bits, 1)
                .expect("descriptor cache should contain first value");
            assert_eq!(cached.class_attr_bits, Some(first_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 2);
            assert_eq!(heap_refcount(first_bits), first_before + 2);
            cached.release(_py);
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before + 1);

            descriptor_cache_store(_py, class_bits, attr_bits, 2, None, Some(second_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before);
            assert_eq!(heap_refcount(second_bits), second_before + 1);
            let cached = descriptor_cache_lookup(_py, class_bits, attr_bits, 2)
                .expect("descriptor cache should contain replacement value");
            assert_eq!(cached.class_attr_bits, Some(second_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 2);
            assert_eq!(heap_refcount(second_bits), second_before + 2);
            cached.release(_py);
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(second_bits), second_before + 1);

            clear_attr_tls_caches(_py);
            assert_eq!(heap_refcount(class_bits), class_before);
            assert_eq!(heap_refcount(first_bits), first_before);
            assert_eq!(heap_refcount(second_bits), second_before);

            dec_ref_bits(_py, second_bits);
            dec_ref_bits(_py, first_bits);
            dec_ref_bits(_py, attr_bits);
            dec_ref_bits(_py, class_bits);
        });
    }
}

struct AttrNameCacheEntry {
    key: AttrNameCacheKey,
    bits: u64,
}

/// Direct-mapped cache with 16 slots for attribute name -> string bits.
/// Keyed by a simple hash of the byte slice.  This replaces the previous
/// single-entry cache that thrashed on every alternating attribute name
/// (e.g. `__iter__` / `__next__` in a for-loop body caused 2M+ allocs in
/// bench_sum_list).
const ATTR_NAME_CACHE_SIZE: usize = 16; // must be power of 2

struct AttrNameCache {
    slots: [Option<AttrNameCacheEntry>; ATTR_NAME_CACHE_SIZE],
}

impl AttrNameCache {
    const fn new() -> Self {
        // Work around const-init limitations: build the array element-by-element.
        const NONE: Option<AttrNameCacheEntry> = None;
        Self {
            slots: [NONE; ATTR_NAME_CACHE_SIZE],
        }
    }

    #[inline]
    fn slot_index(bytes: &[u8]) -> usize {
        // FNV-1a-inspired fast hash – only needs to spread common dunder
        // names across 16 buckets.
        let mut h: u32 = 0x811c_9dc5;
        for &b in bytes {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        (h as usize) & (ATTR_NAME_CACHE_SIZE - 1)
    }

    fn lookup(&self, bytes: &[u8]) -> Option<u64> {
        let idx = Self::slot_index(bytes);
        self.slots[idx]
            .as_ref()
            .filter(|e| e.key.as_slice() == bytes)
            .map(|e| e.bits)
    }

    fn insert(&mut self, _py: &PyToken<'_>, bytes: &[u8], bits: u64) {
        let idx = Self::slot_index(bytes);
        if let Some(prev) = self.slots[idx].take() {
            dec_ref_bits(_py, prev.bits);
        }
        inc_ref_bits(_py, bits);
        self.slots[idx] = Some(AttrNameCacheEntry {
            key: AttrNameCacheKey::new(bytes),
            bits,
        });
    }

    fn clear(&mut self, _py: &PyToken<'_>) {
        for slot in self.slots.iter_mut() {
            if let Some(prev) = slot.take() {
                dec_ref_bits(_py, prev.bits);
            }
        }
    }
}

fn debug_class_layout_filter() -> Option<&'static str> {
    static FILTER: OnceLock<Option<String>> = OnceLock::new();
    FILTER
        .get_or_init(|| {
            std::env::var("MOLT_DEBUG_CLASS_LAYOUT")
                .ok()
                .map(|raw| raw.trim().to_string())
                .filter(|val| !val.is_empty())
        })
        .as_deref()
}

fn debug_class_layout_match(class_name: &str) -> bool {
    match debug_class_layout_filter() {
        Some("1") => true,
        Some(filter) => class_name.contains(filter),
        None => false,
    }
}

pub(crate) struct DescriptorCacheEntry {
    pub(crate) class_bits: u64,
    pub(crate) attr_name: Vec<u8>,
    pub(crate) version: u64,
    pub(crate) data_desc_bits: Option<u64>,
    pub(crate) class_attr_bits: Option<u64>,
}

impl DescriptorCacheEntry {
    fn retain_from_entry(_py: &PyToken<'_>, entry: &Self) -> Self {
        Self::retain(
            _py,
            entry.class_bits,
            entry.attr_name.clone(),
            entry.version,
            entry.data_desc_bits,
            entry.class_attr_bits,
        )
    }

    fn retain(
        _py: &PyToken<'_>,
        class_bits: u64,
        attr_name: Vec<u8>,
        version: u64,
        data_desc_bits: Option<u64>,
        class_attr_bits: Option<u64>,
    ) -> Self {
        if class_bits != 0 {
            inc_ref_bits(_py, class_bits);
        }
        if let Some(bits) = data_desc_bits
            && bits != 0
        {
            inc_ref_bits(_py, bits);
        }
        if let Some(bits) = class_attr_bits
            && bits != 0
        {
            inc_ref_bits(_py, bits);
        }
        Self {
            class_bits,
            attr_name,
            version,
            data_desc_bits,
            class_attr_bits,
        }
    }

    pub(crate) fn release(self, _py: &PyToken<'_>) {
        if self.class_bits != 0 {
            dec_ref_bits(_py, self.class_bits);
        }
        if let Some(bits) = self.data_desc_bits
            && bits != 0
        {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = self.class_attr_bits
            && bits != 0
        {
            dec_ref_bits(_py, bits);
        }
    }
}

// ---------------------------------------------------------------------------
// Field-offset inline cache (IC) — CPython 3.12 LOAD_ATTR_INSTANCE_VALUE
// ---------------------------------------------------------------------------
// Direct-mapped, 32-slot TLS cache keyed by (class_bits, attr_name hash).
// On hit we skip the full MRO walk performed by `class_field_offset` and go
// straight to a single `object_field_get_ptr_raw` call.  Invalidated via the
// global type version counter (bumped when any class __dict__ is modified).

const FIELD_OFFSET_IC_SIZE: usize = 32; // must be power of 2

#[derive(Clone, Copy)]
struct FieldOffsetICEntry {
    /// NaN-boxed class bits (identity of the type object)
    class_bits: u64,
    /// Hash of the attribute name bytes (for fast comparison)
    name_hash: u64,
    /// Global type version when this entry was populated
    type_version: u64,
    /// Cached field offset within the object's field storage
    field_offset: u32,
    /// Length of the attribute name (for collision disambiguation)
    name_len: u32,
}

impl FieldOffsetICEntry {
    const EMPTY: Self = Self {
        class_bits: 0,
        name_hash: 0,
        type_version: 0,
        field_offset: 0,
        name_len: 0,
    };
}

struct FieldOffsetIC {
    slots: [FieldOffsetICEntry; FIELD_OFFSET_IC_SIZE],
}

impl FieldOffsetIC {
    const fn new() -> Self {
        Self {
            slots: [FieldOffsetICEntry::EMPTY; FIELD_OFFSET_IC_SIZE],
        }
    }

    /// FNV-1a hash of attr name bytes — same family as `AttrNameCache::slot_index`.
    #[inline]
    fn hash_name(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    #[inline]
    fn slot_index(class_bits: u64, name_hash: u64) -> usize {
        // Mix class identity with name hash to spread across slots.
        let mixed = class_bits.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ name_hash;
        (mixed as usize) & (FIELD_OFFSET_IC_SIZE - 1)
    }

    /// Try to look up a cached field offset.  Returns `Some(offset)` on hit.
    #[inline]
    fn lookup(&self, class_bits: u64, name_bytes: &[u8], current_version: u64) -> Option<usize> {
        let name_hash = Self::hash_name(name_bytes);
        let idx = Self::slot_index(class_bits, name_hash);
        let entry = &self.slots[idx];
        if entry.class_bits == class_bits
            && entry.name_hash == name_hash
            && entry.type_version == current_version
            && entry.name_len == name_bytes.len() as u32
        {
            Some(entry.field_offset as usize)
        } else {
            None
        }
    }

    /// Populate (or overwrite) a cache slot after a successful field-offset
    /// resolution via the slow `class_field_offset` path.
    #[inline]
    fn insert(
        &mut self,
        class_bits: u64,
        name_bytes: &[u8],
        current_version: u64,
        field_offset: usize,
    ) {
        let name_hash = Self::hash_name(name_bytes);
        let idx = Self::slot_index(class_bits, name_hash);
        self.slots[idx] = FieldOffsetICEntry {
            class_bits,
            name_hash,
            type_version: current_version,
            field_offset: field_offset as u32,
            name_len: name_bytes.len() as u32,
        };
    }

    fn clear(&mut self) {
        self.slots = [FieldOffsetICEntry::EMPTY; FIELD_OFFSET_IC_SIZE];
    }
}

thread_local! {
    static ATTR_NAME_TLS: RefCell<AttrNameCache> = const { RefCell::new(AttrNameCache::new()) };
    static DESCRIPTOR_CACHE_TLS: RefCell<Option<DescriptorCacheEntry>> = const { RefCell::new(None) };
    static FIELD_OFFSET_IC_TLS: RefCell<FieldOffsetIC> = const { RefCell::new(FieldOffsetIC::new()) };
}

pub(crate) fn clear_attr_tls_caches(_py: &PyToken<'_>) {
    crate::gil_assert();
    let _ = ATTR_NAME_TLS.try_with(|cell| {
        cell.borrow_mut().clear(_py);
    });
    let _ = DESCRIPTOR_CACHE_TLS.try_with(|cell| {
        if let Some(entry) = cell.borrow_mut().take() {
            entry.release(_py);
        }
    });
    let _ = FIELD_OFFSET_IC_TLS.try_with(|cell| {
        cell.borrow_mut().clear();
    });
}

/// Probe the field-offset IC for a cached (class, attr) -> offset mapping.
/// Returns `Some(offset)` on hit, `None` on miss.
#[inline]
pub(crate) fn field_offset_ic_lookup(
    class_bits: u64,
    attr_name_bytes: &[u8],
    current_version: u64,
) -> Option<usize> {
    FIELD_OFFSET_IC_TLS.with(|cell| {
        cell.borrow()
            .lookup(class_bits, attr_name_bytes, current_version)
    })
}

/// Populate the field-offset IC after a slow-path resolution.
#[inline]
pub(crate) fn field_offset_ic_insert(
    class_bits: u64,
    attr_name_bytes: &[u8],
    current_version: u64,
    field_offset: usize,
) {
    let _ = FIELD_OFFSET_IC_TLS.try_with(|cell| {
        cell.borrow_mut()
            .insert(class_bits, attr_name_bytes, current_version, field_offset);
    });
}

pub(crate) fn debug_last_attr_name() -> Option<String> {
    // Return the first populated slot for debugging purposes.
    ATTR_NAME_TLS
        .try_with(|cell| {
            let cache = cell.borrow();
            if let Some(entry) = cache.slots.iter().flatten().next() {
                return Some(String::from_utf8_lossy(entry.key.as_slice()).into_owned());
            }
            None
        })
        .ok()
        .flatten()
}

pub(crate) fn attr_error(_py: &PyToken<'_>, type_label: impl AsRef<str>, attr_name: &str) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    raise_exception(_py, "AttributeError", &msg)
}

/// CPython 3.13 added a trailing clause to the AttributeError raised when
/// SETTING (or deleting) an attribute on an object that has no `__dict__` and no
/// slot to hold it: `'X' object has no attribute 'Y' and no __dict__ for setting
/// new attributes`. The GET path keeps the bare `'X' object has no attribute
/// 'Y'` on every version, so this suffix is exclusive to the set/del-failure
/// path. Version-gate it via `runtime_target_at_least(3, 13)` so molt matches
/// CPython 3.12 (no suffix) and 3.13/3.14 (suffix) exactly.
fn setattr_no_dict_suffix(_py: &PyToken<'_>) -> &'static str {
    if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
        " and no __dict__ for setting new attributes"
    } else {
        ""
    }
}

/// [`attr_error_with_obj`] for the set/del-failure path: appends the
/// version-gated `and no __dict__ for setting new attributes` clause (3.13+) and
/// records the `name`/`obj` members on the raised `AttributeError`.
pub(crate) fn setattr_no_attr_error_with_obj(
    _py: &PyToken<'_>,
    type_label: impl AsRef<str>,
    attr_name: &str,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'{}",
        type_label.as_ref(),
        attr_name,
        setattr_no_dict_suffix(_py),
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_attrs(_py, exc_bits, attr_name, obj_bits);
    }
    res
}

fn set_attribute_error_members(_py: &PyToken<'_>, exc_bits: u64, attr_name: &str, obj_bits: u64) {
    crate::gil_assert();
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return;
    };
    // AttributeError.name and AttributeError.obj are not stored in `__dict__` on CPython.
    // Store the pair in the exception "value" slot (used for StopIteration/SystemExit),
    // and have getattr/setattr on exceptions treat this slot specially for AttributeError.
    let name_ptr = alloc_string(_py, attr_name.as_bytes());
    if name_ptr.is_null() {
        return;
    };
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[name_bits, obj_bits]);
    dec_ref_bits(_py, name_bits);
    if tuple_ptr.is_null() {
        return;
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    unsafe {
        let slot = exc_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != tuple_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, tuple_bits);
            *slot = tuple_bits;
        }
    }
    dec_ref_bits(_py, tuple_bits);
}

fn set_attribute_error_attrs(_py: &PyToken<'_>, exc_bits: u64, attr_name: &str, obj_bits: u64) {
    crate::gil_assert();
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return;
        }
        let kind =
            string_obj_to_owned(obj_from_bits(exception_kind_bits(exc_ptr))).unwrap_or_default();
        if kind != "AttributeError" {
            return;
        }
    }
    set_attribute_error_members(_py, exc_bits, attr_name, obj_bits);
}

pub(crate) fn attr_error_with_obj(
    _py: &PyToken<'_>,
    type_label: impl AsRef<str>,
    attr_name: &str,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_attrs(_py, exc_bits, attr_name, obj_bits);
    }
    res
}

pub(crate) fn attr_error_with_message(_py: &PyToken<'_>, msg: &str) -> i64 {
    crate::gil_assert();
    raise_exception(_py, "AttributeError", msg)
}

pub(crate) fn attr_error_with_obj_message(
    _py: &PyToken<'_>,
    msg: &str,
    attr_name: &str,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let res = raise_exception(_py, "AttributeError", msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_attrs(_py, exc_bits, attr_name, obj_bits);
    }
    res
}

pub(crate) fn property_no_setter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no setter");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn property_no_deleter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no deleter");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn descriptor_no_setter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn descriptor_no_deleter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn attr_name_bits_from_bytes(_py: &PyToken<'_>, slice: &[u8]) -> Option<u64> {
    crate::gil_assert();
    if let Some(bits) = ATTR_NAME_TLS.with(|cell| cell.borrow().lookup(slice)) {
        inc_ref_bits(_py, bits);
        return Some(bits);
    }
    let ptr = alloc_string(_py, slice);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    ATTR_NAME_TLS.with(|cell| {
        cell.borrow_mut().insert(_py, slice, bits);
    });
    Some(bits)
}

pub(crate) fn raise_attr_name_type_error(_py: &PyToken<'_>, name_bits: u64) -> u64 {
    crate::gil_assert();
    let name_obj = obj_from_bits(name_bits);
    let msg = format!(
        "attribute name must be string, not '{}'",
        type_name(_py, name_obj)
    );
    raise_exception(_py, "TypeError", &msg)
}

pub(crate) fn exception_is_attribute_error(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    crate::gil_assert();
    exception_matches_builtin_name(_py, exc_bits, "AttributeError")
}

pub(crate) fn clear_attribute_error_if_pending(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last_pending();
    let is_attr = exception_is_attribute_error(_py, exc_bits);
    if is_attr {
        clear_exception(_py);
        dec_ref_bits(_py, exc_bits);
        return true;
    }
    dec_ref_bits(_py, exc_bits);
    false
}

unsafe fn module_attr_lookup_impl(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
    allow_missing: bool,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let dict_bits = module_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = dict_obj.as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            inc_ref_bits(_py, dict_bits);
            return Some(dict_bits);
        }
        let annotations_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotations_name,
            b"__annotations__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(annotations_name_bits),
        ) {
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_name_bits) {
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            let res_bits = if pep649_enabled(_py) {
                let annotate_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                let annotate_bits = dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(annotate_bits).is_none() {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception(_py, "TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception(_py, "TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                }
            } else {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return None;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            };
            let complete_name_bits =
                attr_name_bits_from_bytes(_py, b"__molt_module_complete__").unwrap_or(0);
            let mut cache = false;
            if complete_name_bits != 0 {
                if let Some(complete_bits) = dict_get_in_place(_py, dict_ptr, complete_name_bits) {
                    cache = is_truthy(_py, obj_from_bits(complete_bits));
                }
                dec_ref_bits(_py, complete_name_bits);
            }
            if cache {
                dict_set_in_place(_py, dict_ptr, annotations_name_bits, res_bits);
            }
            return Some(res_bits);
        }
        let annotate_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotate_name,
            b"__annotate__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(annotate_name_bits),
        ) {
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotate_name_bits) {
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            let none_bits = MoltObject::none().bits();
            inc_ref_bits(_py, none_bits);
            return Some(none_bits);
        }
        if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
            inc_ref_bits(_py, val);
            return Some(val);
        }
        let getattr_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattr_name,
            b"__getattr__",
        );
        if !obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(getattr_name_bits),
        ) && let Some(getattr_bits) = dict_get_in_place(_py, dict_ptr, getattr_name_bits)
        {
            if allow_missing {
                exception_stack_push();
            }
            let res_bits = call_callable1(_py, getattr_bits, attr_bits);
            if exception_pending(_py) {
                if allow_missing && clear_attribute_error_if_pending(_py) {
                    exception_stack_pop(_py);
                    return None;
                }
                if allow_missing {
                    exception_stack_pop(_py);
                }
                return None;
            }
            if allow_missing {
                exception_stack_pop(_py);
            }
            return Some(res_bits);
        }
        None
    }
}

pub(crate) unsafe fn module_attr_lookup(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(_py, ptr, attr_bits, false) }
}

pub(crate) unsafe fn module_attr_lookup_allow_missing(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(_py, ptr, attr_bits, true) }
}

pub(crate) unsafe fn dir_collect_from_dict_ptr(
    dict_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        let order = dict_order(dict_ptr);
        for pair in order.chunks_exact(2) {
            let key_bits = pair[0];
            if let Some(name) = string_obj_to_owned(obj_from_bits(key_bits))
                && seen.insert(name)
            {
                out.push(key_bits);
            }
        }
    }
}

pub(crate) unsafe fn dir_collect_from_class_bits(
    class_bits: u64,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        for base_bits in class_mro_vec(class_bits) {
            let class_obj = obj_from_bits(base_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            dir_collect_from_dict_ptr(dict_ptr, seen, out);
        }
    }
}

pub(crate) unsafe fn dir_collect_from_instance(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        // CPython's `dir()` includes instance `__dict__` keys, but it must not call
        // `getattr(obj, "__dict__")` (which can run arbitrary user code and/or recurse).
        //
        // Instead, consult the runtime's internal dict storage for the handful of object
        // categories that actually have one.
        let dict_bits = match object_type_id(obj_ptr) {
            TYPE_ID_OBJECT => instance_dict_bits(obj_ptr),
            TYPE_ID_DATACLASS => dataclass_dict_bits(obj_ptr),
            TYPE_ID_EXCEPTION => exception_dict_bits(obj_ptr),
            TYPE_ID_MODULE => module_dict_bits(obj_ptr),
            _ => 0,
        };
        if dict_bits == 0 || obj_from_bits(dict_bits).is_none() {
            return;
        }
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
        dir_collect_from_dict_ptr(dict_ptr, seen, out);
    }
}

pub(crate) unsafe fn instance_bits_for_call(ptr: *mut u8) -> u64 {
    MoltObject::from_ptr(ptr).bits()
}

fn function_code_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_code_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_code), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                object_set_class_bits(_py, getter_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

fn function_globals_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_globals_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_globals), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                object_set_class_bits(_py, getter_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

pub(crate) unsafe fn class_attr_lookup_raw_mro(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        if let Some(name) = attr_name.as_deref()
            && (name == "__code__" || name == "__globals__")
        {
            let builtins = builtin_classes(_py);
            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            if class_bits == builtins.function {
                let bits = if name == "__code__" {
                    function_code_descriptor_bits(_py)
                } else {
                    function_globals_descriptor_bits(_py)
                };
                if bits != 0 {
                    return Some(bits);
                }
            }
        }
        let debug_bound = crate::builtins::attributes::debug_bound_method_enabled();
        if let Some(mro) = class_mro_ref(class_ptr) {
            for class_bits in mro.iter() {
                let class_obj = obj_from_bits(*class_bits);
                let Some(ptr) = class_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                let Some(dict_ptr) = dict_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                    if debug_bound && let Some(name) = attr_name.as_deref() {
                        let class_name_bits = class_name_bits(ptr);
                        let class_name = string_obj_to_owned(obj_from_bits(class_name_bits))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        let val_obj = obj_from_bits(val_bits);
                        let (val_type_id, val_type_name) = match val_obj.as_ptr() {
                            Some(val_ptr) => (
                                object_type_id(val_ptr),
                                type_name(_py, val_obj).into_owned(),
                            ),
                            None => (0, format!("immediate:{:#x}", val_bits)),
                        };
                        if class_name == "ThreadPoolExecutor" || class_name == "Executor" {
                            eprintln!(
                                "class_attr_lookup_raw_mro: attr={} class={} val_bits={:#x} val_type_id={} val_type={}",
                                name, class_name, val_bits, val_type_id, val_type_name
                            );
                        }
                    }
                    return Some(val_bits);
                }
                // Clear any exception left by the failed dict lookup
                clear_attribute_error_if_pending(_py);
                if let Some(name) = attr_name.as_deref()
                    && is_builtin_class_bits(_py, *class_bits)
                    && let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name)
                {
                    return Some(func_bits);
                }
            }
            // __doc__ defaults to None for all classes (CPython parity).
            // Builtin types and types.ModuleType don't store __doc__ in their
            // class dict, but cls.__doc__ must still return None, not raise.
            if attr_name.as_deref() == Some("__doc__") {
                return Some(MoltObject::none().bits());
            }
            return None;
        }
        let mut current_ptr = class_ptr;
        let mut depth = 0usize;
        loop {
            let dict_bits = class_dict_bits(current_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = dict_obj.as_ptr()?;
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                return Some(val_bits);
            }
            // Clear any exception left by the failed dict lookup
            clear_attribute_error_if_pending(_py);
            if let Some(name) = attr_name.as_deref() {
                let current_bits = MoltObject::from_ptr(current_ptr).bits();
                if is_builtin_class_bits(_py, current_bits)
                    && let Some(func_bits) = builtin_class_method_bits(_py, current_bits, name)
                {
                    return Some(func_bits);
                }
            }
            let bases_bits = class_bases_bits(current_ptr);
            let bases = class_bases_vec(bases_bits);
            let next_bits = bases.first().copied()?;
            let next_obj = obj_from_bits(next_bits);
            let next_ptr = next_obj.as_ptr()?;
            if object_type_id(next_ptr) != TYPE_ID_TYPE {
                return None;
            }
            if next_ptr == current_ptr {
                return None;
            }
            current_ptr = next_ptr;
            depth += 1;
            if depth > 64 {
                return None;
            }
        }
    }
}

pub(crate) unsafe fn class_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        crate::gil_assert();
        let fields_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
        };
        for class_bits in mro.iter().copied() {
            let Some(current_ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(current_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(current_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(offsets_bits) = dict_get_in_place(_py, dict_ptr, fields_bits) else {
                continue;
            };
            let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(offset_bits) = dict_get_in_place(_py, offsets_ptr, attr_bits) else {
                continue;
            };
            return obj_from_bits(offset_bits)
                .as_int()
                .and_then(|val| if val >= 0 { Some(val as usize) } else { None });
        }
        None
    }
}

unsafe fn slots_value_declares_attr(_py: &PyToken<'_>, slots_bits: u64, attr_bits: u64) -> bool {
    unsafe {
        let slots_obj = obj_from_bits(slots_bits);
        let Some(slots_ptr) = slots_obj.as_ptr() else {
            return false;
        };
        match object_type_id(slots_ptr) {
            TYPE_ID_STRING => obj_eq(_py, slots_obj, obj_from_bits(attr_bits)),
            TYPE_ID_TUPLE | TYPE_ID_LIST => seq_vec_ref(slots_ptr)
                .iter()
                .copied()
                .any(|slot_bits| obj_eq(_py, obj_from_bits(slot_bits), obj_from_bits(attr_bits))),
            _ => false,
        }
    }
}

pub(crate) unsafe fn class_own_slot_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        crate::gil_assert();
        if class_ptr.is_null() || object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let dict_bits = class_dict_bits(class_ptr);
        let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let slots_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
        let slots_bits = dict_get_in_place(_py, dict_ptr, slots_name_bits)?;
        if !slots_value_declares_attr(_py, slots_bits, attr_bits) {
            return None;
        }
        let fields_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let offsets_bits = dict_get_in_place(_py, dict_ptr, fields_bits)?;
        let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return None;
        }
        let offset_bits = dict_get_in_place(_py, offsets_ptr, attr_bits)?;
        obj_from_bits(offset_bits)
            .as_int()
            .and_then(|val| if val >= 0 { Some(val as usize) } else { None })
    }
}

/// Design A (#86 — single field-ownership authority): release every inline typed
/// attribute field of a heap `TYPE_ID_OBJECT` instance when it is freed.
///
/// An object's inline field slots are the SOLE owner of their pointer references:
/// `object_field_set_ptr_raw` / `object_field_init_ptr_raw` `inc_ref` the value on
/// store (and `dec_ref` the displaced old value). The runtime free path is the one
/// authority that releases them. Folded objects that release their fields via the
/// compiler drop pass are stack-promoted / immortal and NEVER reach the runtime
/// free path, so there is no double-free with this release.
///
/// Safety facts that make a blind per-slot `dec_ref` correct:
/// - inline fields are NaN-boxed (`object_field_set_ptr_raw` stores `val_bits`), so
///   a primitive field (`int`/`float`/`bool`/`None`) `dec_ref`s to a no-op;
/// - the payload is zero-initialised at alloc, so an unset field reads `0` (no-op);
/// - only POINTER slots (`as_ptr().is_some()`) are released, and each released slot
///   is cleared to `0` first so a resurrecting `__del__` re-entry cannot double-dec.
///
/// Offsets are deduplicated across the MRO so a field shared by base+subclass
/// layout is released exactly once. The caller gates on `HEADER_FLAG_HAS_PTRS`, so
/// primitive-only objects skip this walk entirely (zero hot-path cost).
pub(crate) unsafe fn dec_ref_object_inline_fields(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
) {
    unsafe {
        let fields_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
        };
        let payload = crate::object::object_payload_size(obj_ptr);
        let mut seen: Vec<usize> = Vec::new();
        for class_bits in mro.iter().copied() {
            let Some(current_ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(current_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(current_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(offsets_bits) = dict_get_in_place(_py, dict_ptr, fields_bits) else {
                continue;
            };
            let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            // Snapshot the offset-dict keys before the per-key lookups so we do not
            // alias the dict's `order` Vec across `dict_get_in_place`.
            let keys: Vec<u64> = crate::builtins::containers::dict_order(offsets_ptr).clone();
            for key in keys {
                let Some(offset) = dict_get_in_place(_py, offsets_ptr, key)
                    .and_then(|b| obj_from_bits(b).as_int())
                    .and_then(|v| if v >= 0 { Some(v as usize) } else { None })
                else {
                    continue;
                };
                // Bounds guard: a field slot must lie fully inside the payload (the
                // trailing `__dict__` slot is released separately by the caller, and
                // a registered field offset never aliases it).
                if offset.saturating_add(std::mem::size_of::<u64>()) > payload {
                    continue;
                }
                if seen.contains(&offset) {
                    continue;
                }
                seen.push(offset);
                let slot = obj_ptr.add(offset) as *mut u64;
                let val = *slot;
                if val != 0 && obj_from_bits(val).as_ptr().is_some() {
                    *slot = 0;
                    dec_ref_bits(_py, val);
                }
            }
        }
    }
}

pub(crate) unsafe fn is_iterator_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    unsafe {
        crate::gil_assert();
        let Some(ptr) = maybe_ptr_from_bits(bits) else {
            return false;
        };
        match object_type_id(ptr) {
            TYPE_ID_ITER
            | TYPE_ID_GENERATOR
            | TYPE_ID_ENUMERATE
            | TYPE_ID_CALL_ITER
            | TYPE_ID_REVERSED
            | TYPE_ID_ZIP
            | TYPE_ID_MAP
            | TYPE_ID_FILTER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_FILE_HANDLE => return true,
            _ => {}
        }
        let class_bits = if object_type_id(ptr) == TYPE_ID_TYPE {
            type_of_bits(_py, MoltObject::from_ptr(ptr).bits())
        } else {
            object_class_bits(ptr)
        };
        if class_bits == 0 {
            return false;
        }
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return false;
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return false;
        }
        let Some(next_bits) = attr_name_bits_from_bytes(_py, b"__next__") else {
            return false;
        };
        let has_next = class_attr_lookup_raw_mro(_py, class_ptr, next_bits).is_some();
        dec_ref_bits(_py, next_bits);
        has_next
    }
}

pub(crate) fn descriptor_cache_lookup(
    _py: &PyToken<'_>,
    class_bits: u64,
    attr_bits: u64,
    version: u64,
) -> Option<DescriptorCacheEntry> {
    crate::gil_assert();
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))?;
    let attr_bytes = attr_name.as_bytes();
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| {
                entry.class_bits == class_bits
                    && entry.version == version
                    && entry.attr_name == attr_bytes
            })
            .map(|entry| DescriptorCacheEntry::retain_from_entry(_py, entry))
    })
}

pub(crate) fn descriptor_cache_store(
    _py: &PyToken<'_>,
    class_bits: u64,
    attr_bits: u64,
    version: u64,
    data_desc_bits: Option<u64>,
    class_attr_bits: Option<u64>,
) {
    crate::gil_assert();
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        return;
    };
    let entry = DescriptorCacheEntry::retain(
        _py,
        class_bits,
        attr_name.into_bytes(),
        version,
        data_desc_bits,
        class_attr_bits,
    );
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        if let Some(old_entry) = cell.borrow_mut().replace(entry) {
            old_entry.release(_py);
        }
    });
}

pub(crate) unsafe fn descriptor_method_bits(
    _py: &PyToken<'_>,
    val_bits: u64,
    name_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let class_bits = if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => MoltObject::from_ptr(ptr).bits(),
                TYPE_ID_OBJECT => object_class_bits(ptr),
                _ => type_of_bits(_py, val_bits),
            }
        } else {
            type_of_bits(_py, val_bits)
        };
        let class_obj = obj_from_bits(class_bits);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        class_attr_lookup_raw_mro(_py, class_ptr, name_bits)
    }
}

pub(crate) unsafe fn descriptor_has_method(
    _py: &PyToken<'_>,
    val_bits: u64,
    name_bits: u64,
) -> bool {
    unsafe {
        crate::gil_assert();
        descriptor_method_bits(_py, val_bits, name_bits).is_some()
    }
}

pub(crate) unsafe fn descriptor_is_data(_py: &PyToken<'_>, val_bits: u64) -> bool {
    unsafe {
        crate::gil_assert();
        let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
            return false;
        };
        if object_type_id(val_ptr) == TYPE_ID_PROPERTY {
            return true;
        }
        let set_bits = intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
        let del_bits =
            intern_static_name(_py, &runtime_state(_py).interned.delete_name, b"__delete__");
        descriptor_has_method(_py, val_bits, set_bits)
            || descriptor_has_method(_py, val_bits, del_bits)
    }
}

pub(crate) unsafe fn attr_lookup_ptr_any(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        match object_type_id(obj_ptr) {
            TYPE_ID_OBJECT => object_attr_lookup_raw(_py, obj_ptr, attr_bits),
            TYPE_ID_DATACLASS => dataclass_attr_lookup_raw(_py, obj_ptr, attr_bits),
            _ => attr_lookup_ptr(_py, obj_ptr, attr_bits),
        }
    }
}

pub(crate) unsafe fn attr_lookup_ptr_allow_missing(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let res = if object_type_id(obj_ptr) == TYPE_ID_MODULE {
            module_attr_lookup_allow_missing(_py, obj_ptr, attr_bits)
        } else {
            attr_lookup_ptr_any(_py, obj_ptr, attr_bits)
        };
        if matches!(
            std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
            Some("1")
        ) && string_obj_to_owned(obj_from_bits(attr_bits)).as_deref()
            == Some("__init_subclass__")
        {
            match res {
                Some(bits) => {
                    let obj = obj_from_bits(bits);
                    eprintln!(
                        "molt init_subclass allow_missing res_bits=0x{:x} none={} ptr={}",
                        bits,
                        obj.is_none(),
                        obj.as_ptr().is_some(),
                    );
                }
                None => {
                    eprintln!("molt init_subclass allow_missing res=None");
                }
            }
        }
        if res.is_some() {
            return res;
        }
        if exception_pending(_py) {
            let _ = clear_attribute_error_if_pending(_py);
        }
        None
    }
}

pub(crate) unsafe fn descriptor_bind(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
            inc_ref_bits(_py, val_bits);
            return Some(val_bits);
        };
        // Descriptor binding is the canonical boundary where class-dict/cache
        // descriptor values can run arbitrary user code through property getters
        // or `__get__`. Own the descriptor for this full operation so class
        // mutation during the hook cannot invalidate the borrowed lookup source.
        inc_ref_bits(_py, val_bits);
        let result = match object_type_id(val_ptr) {
            TYPE_ID_FUNCTION => {
                let fn_ptr = crate::function_fn_ptr(val_ptr);
                if let Some(inst_ptr) = instance_ptr {
                    // CPython parity: descriptor access via class objects for object-level slot
                    // wrappers (object.__getattribute__/__setattr__/__delattr__) must remain
                    // unbound so callers pass the target instance explicitly.
                    let object_getattribute_ptr =
                        crate::molt_object_getattribute as *const () as usize as u64;
                    let object_setattr_ptr =
                        crate::molt_object_setattr as *const () as usize as u64;
                    let object_delattr_ptr =
                        crate::molt_object_delattr as *const () as usize as u64;
                    if object_type_id(inst_ptr) == TYPE_ID_TYPE
                        && (fn_ptr == object_getattribute_ptr
                            || fn_ptr == object_setattr_ptr
                            || fn_ptr == object_delattr_ptr)
                    {
                        inc_ref_bits(_py, val_bits);
                        Some(val_bits)
                    } else {
                        let inst_bits = instance_bits_for_call(inst_ptr);
                        let bound_bits = molt_bound_method_new(val_bits, inst_bits);
                        Some(bound_bits)
                    }
                } else {
                    inc_ref_bits(_py, val_bits);
                    Some(val_bits)
                }
            }
            TYPE_ID_CLASSMETHOD => {
                let func_bits = classmethod_func_bits(val_ptr);
                if owner_ptr.is_null() {
                    inc_ref_bits(_py, func_bits);
                    Some(func_bits)
                } else {
                    let class_bits = MoltObject::from_ptr(owner_ptr).bits();
                    Some(molt_bound_method_new(func_bits, class_bits))
                }
            }
            TYPE_ID_STATICMETHOD => {
                let func_bits = staticmethod_func_bits(val_ptr);
                inc_ref_bits(_py, func_bits);
                Some(func_bits)
            }
            TYPE_ID_PROPERTY => {
                if let Some(inst_ptr) = instance_ptr {
                    let get_bits = property_get_bits(val_ptr);
                    if obj_from_bits(get_bits).is_none() {
                        raise_exception(_py, "AttributeError", "unreadable property")
                    } else {
                        let inst_bits = instance_bits_for_call(inst_ptr);
                        let value_bits = call_function_obj1(_py, get_bits, inst_bits);
                        if exception_pending(_py) {
                            if clear_attribute_error_if_pending(_py) {
                                None
                            } else {
                                Some(MoltObject::none().bits())
                            }
                        } else {
                            Some(value_bits)
                        }
                    }
                } else {
                    inc_ref_bits(_py, val_bits);
                    Some(val_bits)
                }
            }
            _ => {
                let get_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.get_name, b"__get__");
                if let Some(method_bits) = descriptor_method_bits(_py, val_bits, get_bits) {
                    let self_bits = MoltObject::from_ptr(val_ptr).bits();
                    let inst_bits = instance_ptr
                        .map(|ptr| instance_bits_for_call(ptr))
                        .unwrap_or_else(|| MoltObject::none().bits());
                    let owner_bits = if owner_ptr.is_null() {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_ptr(owner_ptr).bits()
                    };
                    let res = call_callable3(_py, method_bits, self_bits, inst_bits, owner_bits);
                    if exception_pending(_py) {
                        if clear_attribute_error_if_pending(_py) {
                            None
                        } else {
                            Some(MoltObject::none().bits())
                        }
                    } else {
                        Some(res)
                    }
                } else {
                    inc_ref_bits(_py, val_bits);
                    Some(val_bits)
                }
            }
        };
        dec_ref_bits(_py, val_bits);
        result
    }
}

pub(crate) unsafe fn class_attr_lookup(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let val_bits = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
        descriptor_bind(_py, val_bits, owner_ptr, instance_ptr)
    }
}

pub(crate) fn awaitable_await_func_bits(_py: &PyToken<'_>) -> u64 {
    builtin_func_bits(
        _py,
        &runtime_state(_py).special_cache.awaitable_await,
        fn_addr!(molt_awaitable_await),
        1,
    )
}

pub(crate) struct SlotsInfo {
    pub(crate) allows_attr: bool,
    pub(crate) allows_dict: bool,
}

pub(crate) unsafe fn class_slots_info(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<SlotsInfo> {
    unsafe {
        crate::gil_assert();
        let slots_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let class_dict_bits_val = class_dict_bits(class_ptr);
        let class_dict_ptr = obj_from_bits(class_dict_bits_val).as_ptr()?;
        if object_type_id(class_dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        dict_get_in_place(_py, class_dict_ptr, slots_name_bits)?;
        let mut allows_attr = false;
        let mut allows_dict = false;
        let attr_obj = obj_from_bits(attr_bits);
        let dict_obj = obj_from_bits(dict_name_bits);
        let object_class_bits = builtin_classes(_py).object;
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
        };
        for class_bits in mro.iter().copied() {
            let Some(ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(slots_bits) = dict_get_in_place(_py, dict_ptr, slots_name_bits) else {
                // A user-defined base class with no `__slots__` contributes an
                // instance `__dict__` that subclasses inherit even when the
                // subclass declares slots. Builtin roots such as `object` do not
                // imply a managed dict.
                if class_bits != object_class_bits && !is_builtin_class_bits(_py, class_bits) {
                    allows_dict = true;
                }
                continue;
            };
            let slots_obj = obj_from_bits(slots_bits);
            if let Some(slots_ptr) = slots_obj.as_ptr() {
                match object_type_id(slots_ptr) {
                    TYPE_ID_STRING => {
                        if obj_eq(_py, attr_obj, slots_obj) {
                            allows_attr = true;
                        }
                        if obj_eq(_py, dict_obj, slots_obj) {
                            allows_dict = true;
                        }
                    }
                    TYPE_ID_TUPLE | TYPE_ID_LIST => {
                        for slot_bits in seq_vec_ref(slots_ptr).iter().copied() {
                            let slot_obj = obj_from_bits(slot_bits);
                            if obj_eq(_py, attr_obj, slot_obj) {
                                allows_attr = true;
                            }
                            if obj_eq(_py, dict_obj, slot_obj) {
                                allows_dict = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(SlotsInfo {
            allows_attr,
            allows_dict,
        })
    }
}

pub(crate) unsafe fn apply_class_slots_layout(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    unsafe {
        crate::gil_assert();
        if class_ptr.is_null() {
            return true;
        }
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return true;
        }
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return true;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return true;
        }
        let slots_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
        let Some(slots_bits) = dict_get_in_place(_py, dict_ptr, slots_name_bits) else {
            return true;
        };
        let mut slot_names: Vec<u64> = Vec::new();
        let slots_obj = obj_from_bits(slots_bits);
        let Some(slots_ptr) = slots_obj.as_ptr() else {
            raise_exception::<()>(_py, "TypeError", "__slots__ must be a string or iterable");
            return false;
        };
        match object_type_id(slots_ptr) {
            TYPE_ID_STRING => slot_names.push(slots_bits),
            TYPE_ID_TUPLE | TYPE_ID_LIST => {
                for slot_bits in seq_vec_ref(slots_ptr).iter().copied() {
                    let slot_obj = obj_from_bits(slot_bits);
                    let Some(slot_ptr) = slot_obj.as_ptr() else {
                        raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                        return false;
                    };
                    if object_type_id(slot_ptr) != TYPE_ID_STRING {
                        raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                        return false;
                    }
                    slot_names.push(slot_bits);
                }
            }
            _ => {
                let iter_bits = molt_iter(slots_bits);
                if obj_from_bits(iter_bits).is_none() {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "__slots__ must be a string or iterable",
                    );
                    return false;
                }
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    if exception_pending(_py) {
                        return false;
                    }
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__slots__ must be a string or iterable",
                        );
                        return false;
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__slots__ must be a string or iterable",
                        );
                        return false;
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__slots__ must be a string or iterable",
                        );
                        return false;
                    }
                    let done_bits = elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let slot_bits = elems[0];
                    let slot_obj = obj_from_bits(slot_bits);
                    let Some(slot_ptr) = slot_obj.as_ptr() else {
                        raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                        return false;
                    };
                    if object_type_id(slot_ptr) != TYPE_ID_STRING {
                        raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                        return false;
                    }
                    slot_names.push(slot_bits);
                }
            }
        }

        let offsets_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let layout_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );

        let mut offsets_bits = dict_get_in_place(_py, dict_ptr, offsets_name_bits).unwrap_or(0);
        if obj_from_bits(offsets_bits).is_none() || offsets_bits == 0 {
            let new_ptr = alloc_dict_with_pairs(_py, &[]);
            if new_ptr.is_null() {
                return false;
            }
            offsets_bits = MoltObject::from_ptr(new_ptr).bits();
            dict_set_in_place(_py, dict_ptr, offsets_name_bits, offsets_bits);
        }
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
            return false;
        };
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
            return false;
        }

        let mut layout_size = 0usize;
        let mut original_layout_size = 0usize;
        if let Some(size_bits) = dict_get_in_place(_py, dict_ptr, layout_name_bits)
            && let Some(size) = obj_from_bits(size_bits).as_int()
            && size > 0
        {
            layout_size = size as usize;
            original_layout_size = layout_size;
        }
        if layout_size == 0
            && let Some(size_bits) = class_attr_lookup_raw_mro(_py, class_ptr, layout_name_bits)
            && let Some(size) = obj_from_bits(size_bits).as_int()
            && size > 0
        {
            layout_size = size as usize;
            original_layout_size = layout_size;
        }
        if layout_size == 0 {
            layout_size = 8;
        }

        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        let reserved_tail = if issubclass_bits(class_bits, builtins.dict) {
            2 * std::mem::size_of::<u64>()
        } else {
            std::mem::size_of::<u64>()
        };
        if layout_size < reserved_tail {
            layout_size = reserved_tail;
        }
        layout_size = layout_size.saturating_sub(reserved_tail);

        let mut updated = false;
        let mut occupied_offsets: Vec<usize> = Vec::new();
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
        };
        for base_bits in mro.iter().copied().skip(1) {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let base_dict_bits = class_dict_bits(base_ptr);
            let Some(base_dict_ptr) = obj_from_bits(base_dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(base_dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(base_offsets_bits) = dict_get_in_place(_py, base_dict_ptr, offsets_name_bits)
            else {
                continue;
            };
            let Some(base_offsets_ptr) = obj_from_bits(base_offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(base_offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            let entries = dict_order(base_offsets_ptr).clone();
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                let key_bits = pair[0];
                let val_bits = pair[1];
                if dict_get_in_place(_py, offsets_ptr, key_bits).is_some() {
                    continue;
                }
                dict_set_in_place(_py, offsets_ptr, key_bits, val_bits);
                if let Some(offset) = obj_from_bits(val_bits).as_int() {
                    let offset = offset.max(0) as usize;
                    let end = offset.saturating_add(std::mem::size_of::<u64>());
                    if end > layout_size {
                        layout_size = end;
                    }
                    occupied_offsets.push(offset);
                }
                updated = true;
            }
        }

        let entries = dict_order(offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let offset = offset as usize;
                occupied_offsets.push(offset);
                let end = offset.saturating_add(std::mem::size_of::<u64>());
                if end > layout_size {
                    layout_size = end;
                }
            }
        }

        for slot_bits in slot_names {
            let slot_obj = obj_from_bits(slot_bits);
            if obj_eq(_py, slot_obj, obj_from_bits(dict_name_bits))
                || obj_eq(_py, slot_obj, obj_from_bits(weakref_name_bits))
            {
                continue;
            }
            let mut existing_offset = dict_get_in_place(_py, offsets_ptr, slot_bits)
                .and_then(|bits| obj_from_bits(bits).as_int())
                .and_then(|offset| {
                    if offset >= 0 {
                        Some(offset as usize)
                    } else {
                        None
                    }
                });
            if let Some(offset) = existing_offset
                && occupied_offsets
                    .iter()
                    .filter(|&&seen| seen == offset)
                    .count()
                    > 1
            {
                existing_offset = None;
            }
            let offset = if let Some(offset) = existing_offset {
                offset
            } else {
                while occupied_offsets.contains(&layout_size) {
                    layout_size = layout_size.saturating_add(std::mem::size_of::<u64>());
                }
                let offset = layout_size;
                let offset_bits = MoltObject::from_int(offset as i64).bits();
                dict_set_in_place(_py, offsets_ptr, slot_bits, offset_bits);
                updated = true;
                offset
            };
            occupied_offsets.push(offset);
            let end = offset.saturating_add(std::mem::size_of::<u64>());
            if end > layout_size {
                layout_size = end;
            }
        }
        layout_size = layout_size.saturating_add(reserved_tail);
        if layout_size != original_layout_size {
            updated = true;
        }
        if updated {
            let size_bits = MoltObject::from_int(layout_size as i64).bits();
            dict_set_in_place(_py, dict_ptr, layout_name_bits, size_bits);
        }
        if let Some(filter) = debug_class_layout_filter() {
            let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
                .unwrap_or_else(|| "<unknown>".to_string());
            if debug_class_layout_match(&class_name) {
                let mut offsets_dump: Vec<String> = Vec::new();
                let entries = dict_order(offsets_ptr).clone();
                for pair in entries.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits = pair[0];
                    let val_bits = pair[1];
                    let key = string_obj_to_owned(obj_from_bits(key_bits))
                        .unwrap_or_else(|| "<non-str>".to_string());
                    let val = obj_from_bits(val_bits).as_int().unwrap_or(-1);
                    offsets_dump.push(format!("{key}={val}"));
                }
                offsets_dump.sort();
                eprintln!(
                    "molt debug class_layout: {class_name} layout_size={} slots_filter={} offsets=[{}]",
                    layout_size,
                    filter,
                    offsets_dump.join(", ")
                );
            }
        }
        true
    }
}

pub(crate) unsafe fn object_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let class_bits = object_class_bits(obj_ptr);
        let mut class_ptr_opt: Option<*mut u8> = None;
        if class_bits == 0 {
            let await_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.await_name, b"__await__");
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(await_name_bits),
            ) && crate::object::object_poll_fn(obj_ptr) != 0
            {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let func_bits = awaitable_await_func_bits(_py);
                return Some(molt_bound_method_new(func_bits, self_bits));
            }
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
        {
            class_ptr_opt = Some(class_ptr);
            if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                if is_missing_bits(_py, bits) {
                    dec_ref_bits(_py, bits);
                    return None;
                }
                return Some(bits);
            }
            let class_version = class_layout_version_bits(class_ptr);
            let mut descriptor_cache_hit = false;
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                descriptor_cache_hit = true;
                if let Some(bits) = entry.data_desc_bits {
                    let bound = descriptor_bind(_py, bits, class_ptr, Some(obj_ptr));
                    let pending = exception_pending(_py);
                    entry.release(_py);
                    if let Some(bound) = bound {
                        return Some(bound);
                    }
                    if pending {
                        return None;
                    }
                } else {
                    entry.release(_py);
                }
            }
            if !descriptor_cache_hit {
                if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if descriptor_is_data(_py, val_bits) {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            Some(val_bits),
                            None,
                        );
                        if let Some(bound) =
                            descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                        {
                            return Some(bound);
                        }
                        if exception_pending(_py) {
                            return None;
                        }
                    } else {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            None,
                            Some(val_bits),
                        );
                    }
                } else {
                    descriptor_cache_store(_py, class_bits, attr_bits, class_version, None, None);
                }
            }
            // --- Field-offset IC fast path (CPython 3.12 LOAD_ATTR_INSTANCE_VALUE) ---
            // Try the TLS IC first to skip the expensive MRO walk in
            // `class_field_offset`.  The IC is keyed by (class_bits, attr_name
            // hash) and validated against the global type version.
            let attr_name_slice: Option<&[u8]> = obj_from_bits(attr_bits)
                .as_ptr()
                .filter(|&p| object_type_id(p) == TYPE_ID_STRING)
                .map(|p| std::slice::from_raw_parts(string_bytes(p), string_len(p)));

            let mut field_offset_resolved: Option<usize> = None;
            let current_type_version = crate::object::global_type_version();

            if let Some(name_bytes) = attr_name_slice
                && let Some(offset) =
                    field_offset_ic_lookup(class_bits, name_bytes, current_type_version)
            {
                profile_hit_unchecked(&FIELD_OFFSET_IC_HIT_COUNT);
                field_offset_resolved = Some(offset);
            }

            if field_offset_resolved.is_none()
                && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
            {
                profile_hit_unchecked(&FIELD_OFFSET_IC_MISS_COUNT);
                field_offset_resolved = Some(offset);
                // Populate IC for next time.
                if let Some(name_bytes) = attr_name_slice {
                    field_offset_ic_insert(class_bits, name_bytes, current_type_version, offset);
                }
            }

            if let Some(offset) = field_offset_resolved {
                let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                if is_missing_bits(_py, bits) {
                    dec_ref_bits(_py, bits);
                    // Don't return None here — fall through so the MRO class
                    // attribute stored in `cached_attr_bits` is still considered.
                } else {
                    return Some(bits);
                }
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
                return Some(class_bits);
            }
            let fallback = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            inc_ref_bits(_py, fallback);
            return Some(fallback);
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(weakref_name_bits),
        ) {
            if let Some(class_ptr) = class_ptr_opt
                && let Some(info) = class_slots_info(_py, class_ptr, attr_bits)
                && !info.allows_attr
            {
                return None;
            }
            return Some(MoltObject::none().bits());
        }
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            if let Some(class_ptr) = class_ptr_opt
                && let Some(info) = class_slots_info(_py, class_ptr, attr_bits)
                && !info.allows_dict
            {
                return None;
            }
            let mut dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits != 0 {
                let valid = obj_from_bits(dict_bits)
                    .as_ptr()
                    .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
                if !valid {
                    dict_bits = 0;
                    instance_set_dict_bits(_py, obj_ptr, 0);
                }
            }
            if dict_bits == 0 {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if !dict_ptr.is_null() {
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    instance_set_dict_bits(_py, obj_ptr, dict_bits);
                }
            }
            if dict_bits != 0 {
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            return None;
        }
        let mut dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            let valid = obj_from_bits(dict_bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
            if !valid {
                dict_bits = 0;
                instance_set_dict_bits(_py, obj_ptr, 0);
            }
        }
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
            && let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits)
        {
            inc_ref_bits(_py, val);
            return Some(val);
        }
        let class_ptr_opt = std::hint::black_box(class_ptr_opt);
        if let Some(class_ptr) = class_ptr_opt {
            let class_version = class_layout_version_bits(class_ptr);
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                if entry.data_desc_bits.is_none()
                    && let Some(val_bits) = entry.class_attr_bits
                {
                    let bound = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr));
                    let pending = exception_pending(_py);
                    entry.release(_py);
                    if let Some(bound) = bound {
                        return Some(bound);
                    }
                    if pending {
                        return None;
                    }
                } else {
                    entry.release(_py);
                }
            } else if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                if descriptor_is_data(_py, val_bits) {
                    descriptor_cache_store(
                        _py,
                        class_bits,
                        attr_bits,
                        class_version,
                        Some(val_bits),
                        None,
                    );
                    return None;
                }
                descriptor_cache_store(
                    _py,
                    class_bits,
                    attr_bits,
                    class_version,
                    None,
                    Some(val_bits),
                );
                if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                    return Some(bound);
                }
                if exception_pending(_py) {
                    return None;
                }
            }
        }
        None
    }
}

/// Resolution outcome for the per-site method inline cache.
pub(crate) struct MethodIcResolution {
    pub(crate) class_bits: u64,
    pub(crate) class_version: u64,
    pub(crate) func_bits: u64,
    /// Whether an instance of this class could carry an OWN attribute of this
    /// name (a managed field slot).  When false, instance-shadow checks may be
    /// skipped on IC hits — a non-data class method can never be shadowed.
    pub(crate) can_shadow: bool,
}

/// Class-side resolution for the fused method fast path (everything except the
/// per-instance shadow check).  Returns the resolved plain-function method plus
/// the `(class_bits, class_version)` IC key and a `can_shadow` flag.  Returns
/// `None` for any shape the unbound fast path does not cover (non-OBJECT type,
/// custom `__getattribute__`, data descriptor, non-function attr).
///
/// # Safety
/// `obj_ptr` must be live; the GIL must be held.
pub(crate) unsafe fn object_method_ic_resolve(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<MethodIcResolution> {
    unsafe {
        crate::gil_assert();
        let type_id = object_type_id(obj_ptr);
        if type_id != TYPE_ID_OBJECT && type_id != TYPE_ID_DATACLASS {
            return None;
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits == 0 {
            return None;
        }
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }

        // (2) Bail out if the class installs a custom __getattribute__ — its
        // observable behaviour must run.  A custom __getattr__ only fires on
        // AttributeError (i.e. a FAILED lookup), so it cannot change the result
        // of a SUCCESSFUL method resolution and is intentionally not checked.
        let getattribute_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattribute_name,
            b"__getattribute__",
        );
        let getattribute_raw = class_attr_lookup_raw_mro(_py, class_ptr, getattribute_bits);
        if let Some(raw_bits) = getattribute_raw {
            match crate::builtins::methods::object_method_bits(_py, "__getattribute__") {
                Some(default_bits) => {
                    if !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits)) {
                        return None;
                    }
                }
                None => return None,
            }
        }

        // (3) Resolve the class attribute, preferring the descriptor cache
        // (populated by object_attr_lookup_raw) and validating against the
        // class layout version.  A data descriptor of this name takes
        // precedence over both the instance and a plain method, so bail.
        let class_version = class_layout_version_bits(class_ptr);
        let class_attr_bits = {
            let mut resolved: Option<u64> = None;
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                if entry.data_desc_bits.is_some() {
                    entry.release(_py);
                    return None;
                }
                resolved = entry.class_attr_bits;
                entry.release(_py);
            }
            match resolved {
                Some(bits) => bits,
                None => {
                    let val_bits = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
                    if descriptor_is_data(_py, val_bits) {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            Some(val_bits),
                            None,
                        );
                        return None;
                    }
                    descriptor_cache_store(
                        _py,
                        class_bits,
                        attr_bits,
                        class_version,
                        None,
                        Some(val_bits),
                    );
                    val_bits
                }
            }
        };

        // (3 cont.) Only a plain function qualifies for the unbound fast path.
        let func_ptr = maybe_ptr_from_bits(class_attr_bits)?;
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return None;
        }

        // `can_shadow` is a CLASS-level property (the IC is keyed on the class,
        // not the instance, so it must hold for EVERY instance of the class).
        // A non-data class method is shadowed only by an instance OWN attribute,
        // which requires either a managed field slot for the name OR a dynamic
        // instance `__dict__`.  We only prove "cannot shadow" when the class
        // layout has no offset for `attr` AND the class forbids an instance
        // `__dict__` (slots-only without `__dict__`).  Otherwise stay
        // conservative (`true`) and keep the cheap per-call shadow check.
        let has_field_offset = class_field_offset(_py, class_ptr, attr_bits).is_some();
        let allows_instance_dict = match class_slots_info(_py, class_ptr, attr_bits) {
            // A slots class permits an instance dict when it declares
            // `__dict__` or inherits a dict-bearing user class.
            Some(info) => info.allows_dict,
            // No __slots__ anywhere in the MRO => instances carry a __dict__.
            None => true,
        };
        let can_shadow = has_field_offset || allows_instance_dict;

        Some(MethodIcResolution {
            class_bits,
            class_version,
            func_bits: class_attr_bits,
            can_shadow,
        })
    }
}

/// Public wrapper of [`instance_shadows_attr`] for the per-site method IC's
/// hit-time validation.
///
/// # Safety
/// `obj_ptr`/`class_ptr` must be live; the GIL must be held.
pub(crate) unsafe fn object_instance_shadows(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> bool {
    unsafe { instance_shadows_attr(_py, obj_ptr, class_ptr, attr_bits) }
}

/// True when `obj` has an OWN attribute named `attr_bits` (an instance field
/// slot holding a present value, or a `__dict__` entry).  Mirrors the
/// instance-precedence portion of `object_attr_lookup_raw`.
///
/// # Safety
/// Pointers must be live; the GIL must be held.
unsafe fn instance_shadows_attr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> bool {
    unsafe {
        // Instance field slot (CPython managed-dict / __slots__ value).
        if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
            let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
            let present = !is_missing_bits(_py, bits);
            dec_ref_bits(_py, bits);
            if present {
                return true;
            }
        }
        // Instance __dict__ entry.
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
            && dict_get_in_place(_py, dict_ptr, attr_bits).is_some()
        {
            return true;
        }
        false
    }
}

/// Resolution outcome for the per-site super-method inline cache.
pub(crate) struct SuperIcResolution {
    /// `type(self)` — the IC key class (super resolution depends on the runtime
    /// type of the instance, not the defining class alone).
    pub(crate) self_class_bits: u64,
    pub(crate) self_class_version: u64,
    pub(crate) func_bits: u64,
}

/// `super().method(args)` fast path: resolve the MRO-next plain method without
/// allocating a `super` object, a bound method, or a CallArgs builder.
///
/// `start_class_bits` is the defining class (`__class__`); `self_bits` is the
/// instance.  Mirrors the `TYPE_ID_SUPER` branch of `attr_lookup_ptr`: walk the
/// MRO of `type(self)` (the object-bound super form) starting AFTER
/// `start_class`, and return the first plain-`TYPE_ID_FUNCTION` attribute found
/// in a class dict, along with the `(type(self), version)` IC key.  Returns
/// `None` (caller falls back to the allocating `super_new` + `get_attr` + `call`
/// path) for any non-function descriptor, builtin-class hit, or unsupported
/// shape.  The returned `func_bits` is BORROWED (it lives in a class dict).
///
/// # Safety
/// `self_bits` must be a live object; the GIL must be held.
pub(crate) unsafe fn super_resolve_method_unbound(
    _py: &PyToken<'_>,
    start_class_bits: u64,
    self_bits: u64,
    attr_bits: u64,
) -> Option<SuperIcResolution> {
    unsafe {
        crate::gil_assert();
        // Object-bound super: walk the MRO of `type(self)`.  (The class-bound
        // `super(C, D)` form, where the target is itself a type, is left to the
        // slow path — it is rare and outside the per-call hot loop.)
        let self_ptr = maybe_ptr_from_bits(self_bits)?;
        if object_type_id(self_ptr) == TYPE_ID_TYPE {
            return None;
        }
        let obj_type_bits = type_of_bits(_py, self_bits);
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let self_class_version = class_layout_version_bits(obj_type_ptr);
        let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(obj_type_ptr) {
            Cow::Borrowed(mro.as_slice())
        } else {
            Cow::Owned(class_mro_vec(obj_type_bits))
        };
        let mut found_start = false;
        for class_bits in mro.iter().copied() {
            if !found_start {
                if class_bits == start_class_bits {
                    found_start = true;
                }
                continue;
            }
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            // Builtin classes resolve methods through a separate table; defer
            // those to the slow path for exact parity.
            if is_builtin_class_bits(_py, class_bits) {
                return None;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) else {
                continue;
            };
            // Only a plain function qualifies; anything else (classmethod /
            // staticmethod / property / data descriptor) needs descriptor_bind.
            let val_ptr = maybe_ptr_from_bits(val_bits)?;
            if object_type_id(val_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            return Some(SuperIcResolution {
                self_class_bits: obj_type_bits,
                self_class_version,
                func_bits: val_bits,
            });
        }
        None
    }
}

pub(crate) unsafe fn dataclass_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if desc_ptr.is_null() {
            return None;
        }
        let slots = (*desc_ptr).slots;
        let allows_dict = (*desc_ptr).allows_dict;
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        let class_bits = (*desc_ptr).class_bits;
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && class_own_slot_field_offset(_py, class_ptr, attr_bits).is_some()
            && let Some(name) = attr_name.as_deref()
            && let Some(&index) = (*desc_ptr).field_name_to_index.get(name)
        {
            let fields = dataclass_fields_ref(obj_ptr);
            if index < fields.len() {
                let val_bits = fields[index];
                if is_missing_bits(_py, val_bits) {
                    return None;
                }
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            return None;
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            && descriptor_is_data(_py, val_bits)
        {
            if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
                return Some(class_bits);
            }
            return None;
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            if allows_dict {
                let mut dict_bits = dataclass_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
                    }
                }
                if dict_bits != 0 {
                    if !slots
                        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        let fields = dataclass_fields_ref(obj_ptr);
                        let names = &(*desc_ptr).field_names;
                        let limit = std::cmp::min(fields.len(), names.len());
                        for idx in 0..limit {
                            let Some(key_bits) =
                                attr_name_bits_from_bytes(_py, names[idx].as_bytes())
                            else {
                                continue;
                            };
                            if dict_get_in_place(_py, dict_ptr, key_bits).is_none() {
                                let val_bits = fields[idx];
                                if !is_missing_bits(_py, val_bits) {
                                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                                }
                            }
                        }
                    }
                    inc_ref_bits(_py, dict_bits);
                    return Some(dict_bits);
                }
            }
            return None;
        }
        if !slots {
            let dict_bits = dataclass_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits)
            {
                inc_ref_bits(_py, val);
                return Some(val);
            }
        }
        if let Some(name) = attr_name {
            let fields = dataclass_fields_ref(obj_ptr);
            let names = &(*desc_ptr).field_names;
            let limit = std::cmp::min(fields.len(), names.len());
            for idx in 0..limit {
                if names[idx] == name {
                    let val_bits = fields[idx];
                    if is_missing_bits(_py, val_bits) {
                        return None;
                    }
                    inc_ref_bits(_py, val_bits);
                    return Some(val_bits);
                }
            }
        }
        if slots && allows_dict {
            let dict_bits = dataclass_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits)
            {
                inc_ref_bits(_py, val);
                return Some(val);
            }
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
        {
            if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        None
    }
}
