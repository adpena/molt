use super::class_storage::ClassReferenceSlot;
use crate::{
    MoltObject, PyToken, TYPE_ID_CLASSMETHOD, TYPE_ID_CODE, TYPE_ID_DICT,
    TYPE_ID_NATIVE_DESCRIPTOR, TYPE_ID_PROPERTY, TYPE_ID_STATICMETHOD, TYPE_ID_STRING,
    TYPE_ID_TUPLE, alloc_code_obj, alloc_string, alloc_tuple, builtin_classes_if_initialized,
    dec_ref_bits, dict_get_in_place, inc_ref_bits, intern_static_name, obj_from_bits,
    object_class_bits, object_type_id, runtime_state, string_bytes, string_len,
};

pub(crate) unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

/// Exact inline storage for `TYPE_ID_TUPLE` objects.
///
/// The storage begins at the runtime object's data pointer. `items` is a
/// variable-length trailing array whose exact length is recorded in `len`; it
/// has no capacity field and no separately allocated owner or element buffer.
/// The complete allocation is therefore `[MoltHeader, len, items..]`.
#[repr(C)]
pub(crate) struct TupleStorage {
    len: usize,
    items: [u64; 0],
}

impl TupleStorage {
    #[inline]
    pub(crate) fn object_size(len: usize) -> Option<usize> {
        std::mem::size_of::<crate::MoltHeader>()
            .checked_add(std::mem::size_of::<Self>())?
            .checked_add(len.checked_mul(std::mem::size_of::<u64>())?)
    }
}

#[inline]
pub(crate) unsafe fn tuple_storage_len(ptr: *mut u8) -> usize {
    unsafe { (*(ptr.cast::<TupleStorage>())).len }
}

#[inline]
pub(crate) unsafe fn tuple_storage_set_len_unpublished(ptr: *mut u8, len: usize) {
    unsafe {
        (*(ptr.cast::<TupleStorage>())).len = len;
    }
}

#[inline]
pub(crate) unsafe fn tuple_storage_items(ptr: *mut u8) -> *const u64 {
    unsafe { ptr.add(std::mem::size_of::<TupleStorage>()).cast::<u64>() }
}

#[inline]
pub(crate) unsafe fn tuple_storage_items_mut(ptr: *mut u8) -> *mut u64 {
    unsafe { ptr.add(std::mem::size_of::<TupleStorage>()).cast::<u64>() }
}

/// Layout-stable storage for `TYPE_ID_LIST_INT` objects.
///
/// `#[repr(C)]` guarantees field order: `[data, len, cap]` at offsets `[0, 8, 16]`.
/// The Cranelift inline codegen depends on these offsets for direct load/store
/// without function calls.  Changing field order here WILL break the JIT.
///
/// Replaces `Box<Vec<i64>>` which has `#[repr(Rust)]` layout — the compiler
/// reorders fields arbitrarily between versions (on aarch64-apple-darwin
/// with Rust 1.94 it is `[cap@0, data@8, len@16]`, NOT `[data@0, len@8, cap@16]`).
#[repr(C)]
pub struct ListIntStorage {
    pub data: *mut i64,
    pub len: usize,
    pub cap: usize,
    owner_bytes: usize,
    buffer_bytes: usize,
}

impl ListIntStorage {
    #[inline]
    fn owner_bytes() -> usize {
        std::mem::size_of::<Self>()
    }

    #[inline]
    fn buffer_bytes(capacity: usize) -> Option<usize> {
        capacity.checked_mul(std::mem::size_of::<i64>())
    }

    pub fn with_capacity(capacity: usize) -> Option<*mut ListIntStorage> {
        let requested_buffer = Self::buffer_bytes(capacity)?;
        let charge = Self::owner_bytes().checked_add(requested_buffer)?;
        if !crate::object::backing::charge_alloc(charge) {
            return None;
        }
        let mut vec = Vec::new();
        if capacity > 0 && vec.try_reserve_exact(capacity).is_err() {
            crate::object::backing::release_alloc(charge);
            return None;
        }
        let Some(actual_buffer) = Self::buffer_bytes(vec.capacity()) else {
            drop(vec);
            crate::object::backing::release_alloc(charge);
            return None;
        };
        if actual_buffer > requested_buffer {
            let extra = actual_buffer - requested_buffer;
            if !crate::object::backing::charge_grow(extra) {
                drop(vec);
                crate::object::backing::release_alloc(charge);
                return None;
            }
        } else if actual_buffer < requested_buffer {
            crate::object::backing::release_grow(requested_buffer - actual_buffer);
        }
        Self::from_reserved_vec(vec, Self::owner_bytes(), actual_buffer)
    }

    pub fn filled(len: usize, value: i64) -> Option<*mut ListIntStorage> {
        let ptr = Self::with_capacity(len)?;
        unsafe {
            let storage = &mut *ptr;
            let vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            let mut vec = vec;
            vec.resize(len, value);
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    pub fn from_slice(slice: &[i64]) -> Option<*mut ListIntStorage> {
        let ptr = Self::with_capacity(slice.len())?;
        unsafe {
            let storage = &mut *ptr;
            let mut vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            vec.extend_from_slice(slice);
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    pub fn repeated_slice(slice: &[i64], times: usize) -> Option<*mut ListIntStorage> {
        let total = slice.len().checked_mul(times)?;
        let ptr = Self::with_capacity(total)?;
        unsafe {
            let storage = &mut *ptr;
            let mut vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            for _ in 0..times {
                vec.extend_from_slice(slice);
            }
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    fn from_reserved_vec(
        mut vec: Vec<i64>,
        owner_bytes: usize,
        buffer_bytes: usize,
    ) -> Option<*mut ListIntStorage> {
        let storage = ListIntStorage {
            data: vec.as_mut_ptr(),
            len: vec.len(),
            cap: vec.capacity(),
            owner_bytes,
            buffer_bytes,
        };
        let layout = std::alloc::Layout::new::<ListIntStorage>();
        let raw = unsafe { std::alloc::alloc(layout) as *mut ListIntStorage };
        if raw.is_null() {
            crate::object::backing::release_alloc(owner_bytes.saturating_add(buffer_bytes));
            return None;
        }
        std::mem::forget(vec);
        unsafe {
            std::ptr::write(raw, storage);
        }
        Some(raw)
    }

    /// Reconstruct a `Vec<i64>` that owns the buffer.
    ///
    /// # Safety
    /// Must only be called once (e.g. during dealloc).  After this call
    /// the `ListIntStorage`'s `data` pointer is invalid.
    pub unsafe fn into_vec(self) -> Vec<i64> {
        let vec = unsafe { Vec::from_raw_parts(self.data, self.len, self.cap) };
        crate::object::backing::release_alloc(self.owner_bytes.saturating_add(self.buffer_bytes));
        vec
    }

    pub unsafe fn reserve_for_len(&mut self, required_len: usize) -> bool {
        if required_len <= self.cap {
            return true;
        }
        let target_cap = required_len.max(self.cap.saturating_mul(2)).max(4);
        let Some(old_bytes) = self.cap.checked_mul(std::mem::size_of::<i64>()) else {
            return false;
        };
        let Some(target_bytes) = target_cap.checked_mul(std::mem::size_of::<i64>()) else {
            return false;
        };
        if !crate::object::backing::charge_grow(target_bytes) {
            return false;
        }

        let mut replacement = Vec::new();
        if replacement.try_reserve_exact(target_cap).is_err() {
            crate::object::backing::release_grow(target_bytes);
            return false;
        }

        let Some(actual_bytes) = replacement
            .capacity()
            .checked_mul(std::mem::size_of::<i64>())
        else {
            crate::object::backing::release_grow(target_bytes);
            return false;
        };
        if actual_bytes > target_bytes {
            let extra = actual_bytes - target_bytes;
            if !crate::object::backing::charge_grow(extra) {
                drop(replacement);
                crate::object::backing::release_grow(target_bytes);
                return false;
            }
        } else if actual_bytes < target_bytes {
            crate::object::backing::release_grow(target_bytes - actual_bytes);
        }

        let vec = unsafe { Vec::from_raw_parts(self.data, self.len, self.cap) };
        replacement.extend_from_slice(vec.as_slice());
        drop(vec);
        self.data = replacement.as_mut_ptr();
        self.len = replacement.len();
        self.cap = replacement.capacity();
        self.buffer_bytes = actual_bytes;
        crate::object::backing::release_grow(old_bytes);
        std::mem::forget(replacement);
        true
    }

    /// Append an i64 value to the storage, growing the buffer if needed.
    ///
    /// Uses the same growth strategy as `Vec<i64>::push`: doubles capacity
    /// when full, amortizing allocation cost to O(1) per element.
    ///
    /// This avoids the full promote→NaN-box→push→re-wrap path that
    /// `molt_list_append` would otherwise take, keeping the list in its
    /// compact `TYPE_ID_LIST_INT` representation.
    ///
    /// # Safety
    /// `self` must be a valid, heap-allocated `ListIntStorage` whose `data`
    /// pointer owns its buffer (as established by `from_vec`).
    pub unsafe fn push(&mut self, value: i64) -> bool {
        if self.len == self.cap && !unsafe { self.reserve_for_len(self.len.saturating_add(1)) } {
            return false;
        }
        unsafe {
            std::ptr::write(self.data.add(self.len), value);
        }
        self.len += 1;
        true
    }
}

/// Read the `ListIntStorage` pointer from a `TYPE_ID_LIST_INT` object's data area.
#[inline]
pub(crate) unsafe fn list_int_storage_ptr(ptr: *mut u8) -> *mut ListIntStorage {
    unsafe { *(ptr as *mut *mut ListIntStorage) }
}

/// Read the backing data from a `TYPE_ID_LIST_INT` object as a slice.
/// The layout stores raw i64 values (not NaN-boxed).
pub(crate) unsafe fn list_int_vec_ref(ptr: *mut u8) -> ListIntSliceRef {
    unsafe {
        let storage = &*list_int_storage_ptr(ptr);
        ListIntSliceRef {
            data: storage.data,
            len: storage.len,
        }
    }
}

/// Thin wrapper providing `Vec<i64>`-like interface for `ListIntStorage`.
/// Avoids depending on `Vec` internal layout while maintaining call-site compatibility.
pub(crate) struct ListIntSliceRef {
    data: *const i64,
    len: usize,
}

impl ListIntSliceRef {
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn as_slice(&self) -> &[i64] {
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }

    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, i64> {
        self.as_slice().iter()
    }
}

impl std::ops::Index<usize> for ListIntSliceRef {
    type Output = i64;
    #[inline]
    fn index(&self, index: usize) -> &i64 {
        assert!(index < self.len, "ListIntSliceRef index out of bounds");
        unsafe { &*self.data.add(index) }
    }
}

/// Layout-stable storage for `TYPE_ID_LIST_BOOL` objects.
///
/// `#[repr(C)]` guarantees field order: `[data, len, cap]` at offsets `[0, 8, 16]`.
/// Each element is a single `u8` (0 = False, 1 = True), giving 8x memory savings
/// over storing NaN-boxed bools in a `Vec<u64>`.
///
/// No refcounting needed — bools are inline NaN-boxed values with no heap allocation.
#[repr(C)]
pub struct ListBoolStorage {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
    owner_bytes: usize,
    buffer_bytes: usize,
}

impl ListBoolStorage {
    #[inline]
    fn owner_bytes() -> usize {
        std::mem::size_of::<Self>()
    }

    #[inline]
    fn buffer_bytes(capacity: usize) -> Option<usize> {
        capacity.checked_mul(std::mem::size_of::<u8>())
    }

    pub fn with_capacity(capacity: usize) -> Option<*mut ListBoolStorage> {
        let requested_buffer = Self::buffer_bytes(capacity)?;
        let charge = Self::owner_bytes().checked_add(requested_buffer)?;
        if !crate::object::backing::charge_alloc(charge) {
            return None;
        }
        let mut vec = Vec::new();
        if capacity > 0 && vec.try_reserve_exact(capacity).is_err() {
            crate::object::backing::release_alloc(charge);
            return None;
        }
        let Some(actual_buffer) = Self::buffer_bytes(vec.capacity()) else {
            drop(vec);
            crate::object::backing::release_alloc(charge);
            return None;
        };
        if actual_buffer > requested_buffer {
            let extra = actual_buffer - requested_buffer;
            if !crate::object::backing::charge_grow(extra) {
                drop(vec);
                crate::object::backing::release_alloc(charge);
                return None;
            }
        } else if actual_buffer < requested_buffer {
            crate::object::backing::release_grow(requested_buffer - actual_buffer);
        }
        Self::from_reserved_vec(vec, Self::owner_bytes(), actual_buffer)
    }

    pub fn filled(len: usize, value: u8) -> Option<*mut ListBoolStorage> {
        let ptr = Self::with_capacity(len)?;
        unsafe {
            let storage = &mut *ptr;
            let mut vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            vec.resize(len, value);
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    pub fn from_slice(slice: &[u8]) -> Option<*mut ListBoolStorage> {
        let ptr = Self::with_capacity(slice.len())?;
        unsafe {
            let storage = &mut *ptr;
            let mut vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            vec.extend_from_slice(slice);
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    pub fn repeated_slice(slice: &[u8], times: usize) -> Option<*mut ListBoolStorage> {
        let total = slice.len().checked_mul(times)?;
        let ptr = Self::with_capacity(total)?;
        unsafe {
            let storage = &mut *ptr;
            let mut vec = Vec::from_raw_parts(storage.data, storage.len, storage.cap);
            for _ in 0..times {
                vec.extend_from_slice(slice);
            }
            storage.data = vec.as_mut_ptr();
            storage.len = vec.len();
            storage.cap = vec.capacity();
            std::mem::forget(vec);
        }
        Some(ptr)
    }

    fn from_reserved_vec(
        mut vec: Vec<u8>,
        owner_bytes: usize,
        buffer_bytes: usize,
    ) -> Option<*mut ListBoolStorage> {
        let storage = ListBoolStorage {
            data: vec.as_mut_ptr(),
            len: vec.len(),
            cap: vec.capacity(),
            owner_bytes,
            buffer_bytes,
        };
        let layout = std::alloc::Layout::new::<ListBoolStorage>();
        let raw = unsafe { std::alloc::alloc(layout) as *mut ListBoolStorage };
        if raw.is_null() {
            crate::object::backing::release_alloc(owner_bytes.saturating_add(buffer_bytes));
            return None;
        }
        std::mem::forget(vec);
        unsafe {
            std::ptr::write(raw, storage);
        }
        Some(raw)
    }

    /// Reconstruct a `Vec<u8>` that owns the buffer.
    ///
    /// # Safety
    /// Must only be called once (e.g. during dealloc).  After this call
    /// the `ListBoolStorage`'s `data` pointer is invalid.
    pub unsafe fn into_vec(self) -> Vec<u8> {
        let vec = unsafe { Vec::from_raw_parts(self.data, self.len, self.cap) };
        crate::object::backing::release_alloc(self.owner_bytes.saturating_add(self.buffer_bytes));
        vec
    }

    pub unsafe fn reserve_for_len(&mut self, required_len: usize) -> bool {
        if required_len <= self.cap {
            return true;
        }
        let target_cap = required_len.max(self.cap.saturating_mul(2)).max(8);
        let Some(old_bytes) = self.cap.checked_mul(std::mem::size_of::<u8>()) else {
            return false;
        };
        let Some(target_bytes) = target_cap.checked_mul(std::mem::size_of::<u8>()) else {
            return false;
        };
        if !crate::object::backing::charge_grow(target_bytes) {
            return false;
        }

        let mut replacement = Vec::new();
        if replacement.try_reserve_exact(target_cap).is_err() {
            crate::object::backing::release_grow(target_bytes);
            return false;
        }

        let Some(actual_bytes) = replacement
            .capacity()
            .checked_mul(std::mem::size_of::<u8>())
        else {
            crate::object::backing::release_grow(target_bytes);
            return false;
        };
        if actual_bytes > target_bytes {
            let extra = actual_bytes - target_bytes;
            if !crate::object::backing::charge_grow(extra) {
                drop(replacement);
                crate::object::backing::release_grow(target_bytes);
                return false;
            }
        } else if actual_bytes < target_bytes {
            crate::object::backing::release_grow(target_bytes - actual_bytes);
        }

        let vec = unsafe { Vec::from_raw_parts(self.data, self.len, self.cap) };
        replacement.extend_from_slice(vec.as_slice());
        drop(vec);
        self.data = replacement.as_mut_ptr();
        self.len = replacement.len();
        self.cap = replacement.capacity();
        self.buffer_bytes = actual_bytes;
        crate::object::backing::release_grow(old_bytes);
        std::mem::forget(replacement);
        true
    }

    /// Append a u8 value (0 = False, 1 = True) to the storage, growing
    /// the buffer if needed.
    ///
    /// Uses the same growth strategy as `Vec<u8>::push`: doubles capacity
    /// when full, amortizing allocation cost to O(1) per element.
    ///
    /// This keeps the list in its compact `TYPE_ID_LIST_BOOL` representation
    /// when building bool lists via repeated append (comprehension pattern),
    /// avoiding the promote-to-generic-list path.
    ///
    /// # Safety
    /// `self` must be a valid, heap-allocated `ListBoolStorage` whose `data`
    /// pointer owns its buffer (as established by `from_vec`).
    pub unsafe fn push(&mut self, value: u8) -> bool {
        if self.len == self.cap && !unsafe { self.reserve_for_len(self.len.saturating_add(1)) } {
            return false;
        }
        unsafe {
            std::ptr::write(self.data.add(self.len), value);
        }
        self.len += 1;
        true
    }
}

/// Read the `ListBoolStorage` pointer from a `TYPE_ID_LIST_BOOL` object's data area.
#[inline]
pub(crate) unsafe fn list_bool_storage_ptr(ptr: *mut u8) -> *mut ListBoolStorage {
    unsafe { *(ptr as *mut *mut ListBoolStorage) }
}

/// Read the backing data from a `TYPE_ID_LIST_BOOL` object as a slice.
/// The layout stores raw u8 values (0 = False, 1 = True).
pub(crate) unsafe fn list_bool_vec_ref(ptr: *mut u8) -> ListBoolSliceRef {
    unsafe {
        let storage = &*list_bool_storage_ptr(ptr);
        ListBoolSliceRef {
            data: storage.data,
            len: storage.len,
        }
    }
}

/// Thin wrapper providing slice-like interface for `ListBoolStorage`.
pub(crate) struct ListBoolSliceRef {
    data: *const u8,
    len: usize,
}

impl ListBoolSliceRef {
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }

    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, u8> {
        self.as_slice().iter()
    }
}

impl std::ops::Index<usize> for ListBoolSliceRef {
    type Output = u8;
    #[inline]
    fn index(&self, index: usize) -> &u8 {
        assert!(index < self.len, "ListBoolSliceRef index out of bounds");
        unsafe { &*self.data.add(index) }
    }
}

pub(crate) unsafe fn bytearray_vec_ptr(ptr: *mut u8) -> *mut Vec<u8> {
    unsafe { *(ptr as *mut *mut Vec<u8>) }
}

pub(crate) unsafe fn bytearray_vec(ptr: *mut u8) -> &'static mut Vec<u8> {
    unsafe {
        let vec_ptr = bytearray_vec_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn bytearray_vec_ref(ptr: *mut u8) -> &'static Vec<u8> {
    unsafe {
        let vec_ptr = bytearray_vec_ptr(ptr);
        &*vec_ptr
    }
}

pub(crate) unsafe fn bytearray_len(ptr: *mut u8) -> usize {
    unsafe { bytearray_vec_ref(ptr).len() }
}

pub(crate) unsafe fn bytearray_data(ptr: *mut u8) -> *const u8 {
    unsafe { bytearray_vec_ref(ptr).as_ptr() }
}

pub(crate) unsafe fn iter_target_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn iter_index(ptr: *mut u8) -> usize {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const usize) }
}

pub(crate) unsafe fn iter_set_index(ptr: *mut u8, idx: usize) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
    }
}

/// Offset of the cached (value, done) tuple pointer inside a TYPE_ID_ITER object.
const ITER_CACHED_TUPLE_OFFSET: usize = std::mem::size_of::<u64>() + std::mem::size_of::<usize>();

/// Read the cached 2-tuple pointer from an iter object (may be null).
pub(crate) unsafe fn iter_cached_tuple(ptr: *mut u8) -> *mut u8 {
    unsafe { *(ptr.add(ITER_CACHED_TUPLE_OFFSET) as *const *mut u8) }
}

/// Store a cached 2-tuple pointer in an iter object.
pub(crate) unsafe fn iter_set_cached_tuple(ptr: *mut u8, tuple_ptr: *mut u8) {
    unsafe {
        *(ptr.add(ITER_CACHED_TUPLE_OFFSET) as *mut *mut u8) = tuple_ptr;
    }
}

const ITER_EXPECTED_VERSION_OFFSET: usize =
    ITER_CACHED_TUPLE_OFFSET + std::mem::size_of::<*mut u8>();
const ITER_PROJECTION_OFFSET: usize = ITER_EXPECTED_VERSION_OFFSET + std::mem::size_of::<u64>();

pub(crate) unsafe fn iter_expected_version(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(ITER_EXPECTED_VERSION_OFFSET) as *const u64) }
}

pub(crate) unsafe fn iter_set_expected_version(ptr: *mut u8, version: u64) {
    unsafe { *(ptr.add(ITER_EXPECTED_VERSION_OFFSET) as *mut u64) = version }
}

pub(crate) unsafe fn iter_projection(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(ITER_PROJECTION_OFFSET) as *const u64) }
}

pub(crate) unsafe fn iter_set_projection(ptr: *mut u8, projection: u64) {
    unsafe { *(ptr.add(ITER_PROJECTION_OFFSET) as *mut u64) = projection }
}

pub(crate) unsafe fn iter_set_target_bits(ptr: *mut u8, target_bits: u64) {
    unsafe { *(ptr as *mut u64) = target_bits }
}

pub(crate) unsafe fn enumerate_target_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn enumerate_index_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn enumerate_set_index_bits(ptr: *mut u8, idx_bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = idx_bits;
    }
}

/// Offset of the cached inner `(idx, val)` 2-tuple pointer inside a
/// TYPE_ID_ENUMERATE object. This is the user-visible tuple yielded
/// from each `next()` call.
const ENUMERATE_CACHED_INNER_OFFSET: usize = 2 * std::mem::size_of::<u64>();
/// Offset of the cached outer `(item, done_false)` wrapper tuple pointer
/// inside a TYPE_ID_ENUMERATE object.
const ENUMERATE_CACHED_OUTER_OFFSET: usize =
    ENUMERATE_CACHED_INNER_OFFSET + std::mem::size_of::<*mut u8>();

/// Total payload bytes for a TYPE_ID_ENUMERATE object (after the header).
pub(crate) const ENUMERATE_PAYLOAD_SIZE: usize =
    ENUMERATE_CACHED_OUTER_OFFSET + std::mem::size_of::<*mut u8>();

pub(crate) unsafe fn enumerate_cached_inner(ptr: *mut u8) -> *mut u8 {
    unsafe { *(ptr.add(ENUMERATE_CACHED_INNER_OFFSET) as *const *mut u8) }
}

pub(crate) unsafe fn enumerate_set_cached_inner(ptr: *mut u8, tuple_ptr: *mut u8) {
    unsafe {
        *(ptr.add(ENUMERATE_CACHED_INNER_OFFSET) as *mut *mut u8) = tuple_ptr;
    }
}

pub(crate) unsafe fn enumerate_cached_outer(ptr: *mut u8) -> *mut u8 {
    unsafe { *(ptr.add(ENUMERATE_CACHED_OUTER_OFFSET) as *const *mut u8) }
}

pub(crate) unsafe fn enumerate_set_cached_outer(ptr: *mut u8, tuple_ptr: *mut u8) {
    unsafe {
        *(ptr.add(ENUMERATE_CACHED_OUTER_OFFSET) as *mut *mut u8) = tuple_ptr;
    }
}

pub(crate) unsafe fn call_iter_callable_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn call_iter_sentinel_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

/// Offset of the cached `(value, done)` wrapper tuple pointer inside a
/// TYPE_ID_CALL_ITER object.
const CALL_ITER_CACHED_OFFSET: usize = 2 * std::mem::size_of::<u64>();

/// Total payload bytes for a TYPE_ID_CALL_ITER object (after the header).
pub(crate) const CALL_ITER_PAYLOAD_SIZE: usize =
    CALL_ITER_CACHED_OFFSET + std::mem::size_of::<*mut u8>();

pub(crate) unsafe fn call_iter_cached_tuple(ptr: *mut u8) -> *mut u8 {
    unsafe { *(ptr.add(CALL_ITER_CACHED_OFFSET) as *const *mut u8) }
}

pub(crate) unsafe fn call_iter_set_cached_tuple(ptr: *mut u8, tuple_ptr: *mut u8) {
    unsafe {
        *(ptr.add(CALL_ITER_CACHED_OFFSET) as *mut *mut u8) = tuple_ptr;
    }
}

pub(crate) unsafe fn reversed_target_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn reversed_index(ptr: *mut u8) -> usize {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const usize) }
}

pub(crate) unsafe fn reversed_set_index(ptr: *mut u8, idx: usize) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
    }
}

pub(crate) unsafe fn zip_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn zip_strict_bits(ptr: *mut u8) -> u64 {
    unsafe { std::ptr::read_unaligned(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *const u64) }
}

pub(crate) unsafe fn zip_set_strict_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        std::ptr::write_unaligned(
            ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut u64,
            bits,
        );
    }
}

pub(crate) unsafe fn map_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn map_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *mut *mut Vec<u64>) }
}

/// Offset of the cached `(value, done)` wrapper tuple pointer inside a
/// TYPE_ID_MAP object.
const MAP_CACHED_OFFSET: usize = std::mem::size_of::<u64>() + std::mem::size_of::<*mut Vec<u64>>();

/// Total payload bytes for a TYPE_ID_MAP object (after the header).
pub(crate) const MAP_PAYLOAD_SIZE: usize = MAP_CACHED_OFFSET + std::mem::size_of::<*mut u8>();

pub(crate) unsafe fn map_cached_tuple(ptr: *mut u8) -> *mut u8 {
    unsafe { *(ptr.add(MAP_CACHED_OFFSET) as *const *mut u8) }
}

pub(crate) unsafe fn map_set_cached_tuple(ptr: *mut u8, tuple_ptr: *mut u8) {
    unsafe {
        *(ptr.add(MAP_CACHED_OFFSET) as *mut *mut u8) = tuple_ptr;
    }
}

pub(crate) unsafe fn filter_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn filter_iter_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn range_start_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn range_stop_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn range_step_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn slice_start_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn slice_stop_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn slice_step_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn generic_alias_origin_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn generic_alias_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn union_type_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

#[allow(dead_code)]
pub(crate) unsafe fn function_fn_ptr(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

#[allow(dead_code)]
pub(crate) unsafe fn function_arity(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

/// Decode the function metadata arity for host indexing/allocation without
/// allowing a malformed 64-bit metadata word to alias a smaller 32-bit value.
#[inline(always)]
pub(crate) unsafe fn function_arity_usize(ptr: *mut u8) -> Option<usize> {
    unsafe { usize::try_from(function_arity(ptr)).ok() }
}

#[allow(dead_code)]
pub(crate) unsafe fn function_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn function_name_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let dict_bits = function_dict_bits(ptr);
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            let qual_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.qualname_name,
                b"__qualname__",
            );
            if let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_bits) {
                return bits;
            }
            let name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.name_name, b"__name__");
            if let Some(bits) = dict_get_in_place(_py, dict_ptr, name_bits) {
                return bits;
            }
        }
        MoltObject::none().bits()
    }
}

pub(crate) unsafe fn function_set_dict_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

pub(crate) unsafe fn function_closure_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

/// Immutable physical-call convention for a function object's first native
/// argument. Public lexical closure storage is deliberately separate: an
/// explicitly supplied empty `__closure__` tuple remains observable while its
/// validated zero-freevar callable keeps the positional ABI. Internal runtime
/// contexts publish `OpaqueContextFirst` directly, independent of representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub(crate) enum FunctionCallAbi {
    Positional = 0,
    LexicalClosureFirst = 1,
    OpaqueContextFirst = 2,
}

impl FunctionCallAbi {
    #[inline]
    pub(crate) fn requires_context(self) -> bool {
        self != Self::Positional
    }

    #[inline]
    pub(crate) fn is_reconstructible(self) -> bool {
        self != Self::OpaqueContextFirst
    }
}

pub(crate) unsafe fn function_call_abi(ptr: *mut u8) -> FunctionCallAbi {
    unsafe {
        match *(ptr.add(12 * std::mem::size_of::<u64>()) as *const u64) {
            0 => FunctionCallAbi::Positional,
            1 => FunctionCallAbi::LexicalClosureFirst,
            2 => FunctionCallAbi::OpaqueContextFirst,
            raw => panic!("invalid function call ABI {raw}"),
        }
    }
}

/// Return the payload for the hidden first native argument, or zero when the
/// callable's physical ABI has no such argument.
pub(crate) unsafe fn function_execution_closure_bits(ptr: *mut u8) -> u64 {
    unsafe {
        let bits = function_closure_bits(ptr);
        function_call_abi(ptr)
            .requires_context()
            .then_some(bits)
            .unwrap_or(0)
    }
}

pub(crate) unsafe fn function_has_execution_closure(ptr: *mut u8) -> bool {
    unsafe { function_call_abi(ptr).requires_context() }
}

pub(crate) unsafe fn function_set_closure_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
    call_abi: FunctionCallAbi,
) {
    unsafe {
        crate::gil_assert();
        assert!(
            !call_abi.requires_context() || bits != 0,
            "context-first function publication requires a retained context"
        );
        if bits != 0 {
            inc_ref_bits(_py, bits);
        }
        let closure_slot = ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = closure_slot.replace(bits);
        *(ptr.add(12 * std::mem::size_of::<u64>()) as *mut u64) = call_abi as u64;
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
    }
}

pub(crate) unsafe fn function_code_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

#[allow(dead_code)]
pub(crate) unsafe fn function_trampoline_ptr(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn function_annotations_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn function_set_annotations_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
        *slot = bits;
        if bits != 0 {
            inc_ref_bits(_py, bits);
        }
    }
}

pub(crate) unsafe fn function_annotate_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn function_call_target_ptr(ptr: *mut u8) -> *const () {
    unsafe { *(ptr.add(8 * std::mem::size_of::<u64>()) as *const *const ()) }
}

pub(crate) unsafe fn function_set_annotate_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
        *slot = bits;
        if bits != 0 {
            inc_ref_bits(_py, bits);
        }
    }
}

pub(crate) unsafe fn function_set_call_target_ptr(ptr: *mut u8, target: *const ()) {
    unsafe {
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut *const ()) = target;
    }
}

/// Replace every scalar that selects a function's executable callable.
///
/// The caller must validate closure compatibility and resolve `call_target`
/// before entering this infallible publication step. The owned closure and code
/// edges are deliberately managed by their dedicated setters.
pub(crate) unsafe fn function_replace_callable_identity(
    ptr: *mut u8,
    identity: CodeCallableIdentity,
    call_target: *const (),
) {
    unsafe {
        crate::gil_assert();
        *(ptr as *mut u64) = identity.fn_ptr;
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = identity.arity;
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = identity.trampoline_ptr;
        *(ptr.add(8 * std::mem::size_of::<u64>()) as *mut *const ()) = call_target;
        *(ptr.add(12 * std::mem::size_of::<u64>()) as *mut u64) = identity.call_abi as u64;
    }
}

/// Fallible attachment preparation never changes either owner. Callers must
/// publish immediately, without running callbacks between preparation and
/// publication; the signature facts are borrowed until publication retains them.
pub(crate) unsafe fn prepare_function_code_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
    replacement_identity: Option<CodeCallableIdentity>,
    signature: Option<PreparedCodeSignature>,
) -> Result<PreparedFunctionCode, ()> {
    unsafe {
        crate::gil_assert();
        let mut prepared = PreparedFunctionCode {
            bits,
            code: None,
            signature: None,
            lexical: None,
            execution_kind: None,
        };
        if let Some(code_ptr) = obj_from_bits(bits).as_ptr()
            && object_type_id(code_ptr) == TYPE_ID_CODE
        {
            let identity = replacement_identity.unwrap_or(CodeCallableIdentity {
                fn_ptr: function_fn_ptr(ptr),
                trampoline_ptr: function_trampoline_ptr(ptr),
                arity: function_arity(ptr),
                call_abi: function_call_abi(ptr),
            });
            let identity_was_unpublished = code_callable_identity(code_ptr).is_none();
            if code_validate_callable_identity(code_ptr, identity).is_err() {
                crate::raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "function and code object have conflicting callable identities",
                );
                return Err(());
            }
            if code_published_execution_kind(code_ptr).is_none() {
                prepared.signature = match signature {
                    Some(signature) => Some(signature),
                    None if identity_was_unpublished => {
                        prepare_code_signature_from_function_attrs(_py, ptr)?
                    }
                    None => None,
                };
            }
            prepared.code = Some((code_ptr, identity));
        }
        Ok(prepared)
    }
}

#[must_use]
pub(crate) struct PreparedFunctionCode {
    bits: u64,
    code: Option<(*mut u8, CodeCallableIdentity)>,
    signature: Option<PreparedCodeSignature>,
    lexical: Option<[u64; 2]>,
    execution_kind: Option<CodeExecutionKind>,
}

#[must_use = "displaced code and signature owners must be released after publication"]
pub(crate) struct DisplacedFunctionCode {
    code: u64,
    signature: [u64; 5],
    lexical: [u64; 2],
}

impl PreparedFunctionCode {
    pub(crate) unsafe fn with_lexical_metadata(
        mut self,
        py: &PyToken<'_>,
        freevars: u64,
        cellvars: u64,
        kind: CodeExecutionKind,
    ) -> Result<Self, ()> {
        unsafe {
            let (ptr, _) = self.code.expect("lexical metadata requires a code object");
            validate_code_lexical_metadata(py, Some(ptr), freevars, cellvars)?;
            if code_published_execution_kind(ptr).is_some_and(|published| published != kind) {
                crate::raise_exception::<u64>(
                    py,
                    "TypeError",
                    "code execution kind is already published",
                );
                return Err(());
            }
            self.lexical = Some([freevars, cellvars]);
            self.execution_kind = Some(kind);
            Ok(self)
        }
    }

    /// Infallible, callback-free publication. The caller may publish additional
    /// scalar/epoch/cache state before releasing the returned displaced owners.
    pub(crate) unsafe fn publish(self, py: &PyToken<'_>, ptr: *mut u8) -> DisplacedFunctionCode {
        unsafe {
            crate::gil_assert();
            let slot = ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *slot;
            if old_bits != self.bits && self.bits != 0 {
                inc_ref_bits(py, self.bits);
            }
            let mut displaced_signature = [0; 5];
            let mut displaced_lexical = [0; 2];
            if let Some((code_ptr, identity)) = self.code {
                if let Some(signature) = self.signature {
                    displaced_signature = signature.publish(py, code_ptr);
                }
                if let Some(lexical) = self.lexical {
                    displaced_lexical =
                        publish_code_lexical_metadata_deferred(py, code_ptr, lexical);
                }
                if let Some(kind) = self.execution_kind {
                    code_publish_execution_kind(code_ptr, kind)
                        .expect("prepared code execution kind remains valid without callbacks");
                }
                code_publish_callable_identity_unchecked(code_ptr, identity);
            }
            *slot = self.bits;
            DisplacedFunctionCode {
                code: if old_bits != self.bits { old_bits } else { 0 },
                signature: displaced_signature,
                lexical: displaced_lexical,
            }
        }
    }
}

impl DisplacedFunctionCode {
    pub(crate) fn release(self, py: &PyToken<'_>) {
        for bits in self
            .signature
            .into_iter()
            .chain(self.lexical)
            .chain([self.code])
        {
            if bits != 0 {
                dec_ref_bits(py, bits);
            }
        }
    }
}

#[must_use = "failed code attachment leaves a pending exception and must not report success"]
pub(crate) unsafe fn function_set_code_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe {
        let Ok(prepared) = prepare_function_code_bits(_py, ptr, bits, None, None) else {
            return false;
        };
        prepared.publish(_py, ptr).release(_py);
        true
    }
}

pub(crate) unsafe fn function_set_trampoline_ptr(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

/// Read the captured globals dict bits from function slot 9.
#[allow(dead_code)]
pub(crate) unsafe fn function_globals_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(9 * std::mem::size_of::<u64>()) as *const u64) }
}

/// Store a globals dict on the function object (slot 9).  Takes its own
/// reference — caller must still dec-ref their copy if they own one.
pub(crate) unsafe fn function_set_globals_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
        let builtins_slot = ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64;
        let builtins = crate::builtins::frames::frame_effective_builtins_bits(_py, bits);
        inc_ref_bits(_py, bits);
        inc_ref_bits(_py, builtins);
        let old_globals = std::mem::replace(&mut *slot, bits);
        let old_builtins = std::mem::replace(&mut *builtins_slot, builtins);
        dec_ref_bits(_py, old_globals);
        dec_ref_bits(_py, old_builtins);
    }
}

/// Read the callable-shape mutation version stamp (slot 10).
///
/// 0 means neither defaults nor executable code have changed since creation, so
/// compile-time-baked call shape remains observably correct. Any non-zero value
/// means a defaults or `__code__` reassignment occurred and calls/caches must
/// consult the live function and code authorities.
pub(crate) unsafe fn function_mutation_version(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(10 * std::mem::size_of::<u64>()) as *const u64) }
}

/// Bump the callable-shape mutation version stamp (slot 10).
///
/// Called from user-reachable defaults and `__code__` mutation, never from fresh
/// publication, so a fresh function keeps version 0. The counter is a plain
/// u64; wrap-around requires 2^64 mutations and is harmless for live guards.
pub(crate) unsafe fn bump_function_mutation_version(ptr: *mut u8) {
    unsafe {
        let slot = ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64;
        *slot = (*slot).wrapping_add(1);
    }
}

/// Builtins captured with the function namespace, never re-read at invocation.
pub(crate) unsafe fn function_builtins_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(11 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn ensure_function_code_bits(_py: &PyToken<'_>, func_ptr: *mut u8) -> u64 {
    unsafe {
        let existing = function_code_bits(func_ptr);
        if existing != 0 {
            return existing;
        }
        let mut name_bits = function_name_bits(_py, func_ptr);
        let mut owned_name = false;
        let name_ok = if let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() {
            object_type_id(name_ptr) == TYPE_ID_STRING
        } else {
            false
        };
        if !name_ok {
            let name_ptr = alloc_string(_py, b"<unknown>");
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            name_bits = MoltObject::from_ptr(name_ptr).bits();
            owned_name = true;
        }
        let filename_ptr = alloc_string(_py, b"<molt-builtin>");
        if filename_ptr.is_null() {
            if owned_name {
                dec_ref_bits(_py, name_bits);
            }
            return MoltObject::none().bits();
        }
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let varnames_ptr = alloc_tuple(_py, &[]);
        if varnames_ptr.is_null() {
            dec_ref_bits(_py, filename_bits);
            if owned_name {
                dec_ref_bits(_py, name_bits);
            }
            return MoltObject::none().bits();
        }
        let varnames_bits = MoltObject::from_ptr(varnames_ptr).bits();
        let names_ptr = alloc_tuple(_py, &[]);
        if names_ptr.is_null() {
            dec_ref_bits(_py, varnames_bits);
            dec_ref_bits(_py, filename_bits);
            if owned_name {
                dec_ref_bits(_py, name_bits);
            }
            return MoltObject::none().bits();
        }
        let names_bits = MoltObject::from_ptr(names_ptr).bits();
        let code_ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            0,
            MoltObject::none().bits(),
            varnames_bits,
            names_bits,
            0,
            0,
            0,
        );
        dec_ref_bits(_py, names_bits);
        dec_ref_bits(_py, varnames_bits);
        dec_ref_bits(_py, filename_bits);
        if owned_name {
            dec_ref_bits(_py, name_bits);
        }
        if code_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let code_bits = MoltObject::from_ptr(code_ptr).bits();
        if !function_set_code_bits(_py, func_ptr, code_bits) {
            dec_ref_bits(_py, code_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, code_bits);
        code_bits
    }
}

pub(crate) unsafe fn code_filename_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn code_name_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_firstlineno(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64) }
}

pub(crate) unsafe fn code_linetable_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_varnames_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_names_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_argcount(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_posonlyargcount(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_kwonlyargcount(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(8 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_callable_fn_ptr(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(9 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_callable_trampoline_ptr(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(10 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_callable_arity(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(11 * std::mem::size_of::<u64>()) as *const u64) }
}

const CODE_CALL_ABI_UNPUBLISHED: u64 = u64::MAX;

/// The complete native entry identity retained by a code object.
///
/// `call_abi` is provenance, not an inference from public `co_freevars`: a
/// lexical closure can be safely reconstructed with cells, while an opaque
/// runtime context must never be recreated as a positional Python function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CodeCallableIdentity {
    pub(crate) fn_ptr: u64,
    pub(crate) trampoline_ptr: u64,
    pub(crate) arity: u64,
    pub(crate) call_abi: FunctionCallAbi,
}

pub(crate) unsafe fn code_callable_identity(ptr: *mut u8) -> Option<CodeCallableIdentity> {
    unsafe {
        let fn_ptr = code_callable_fn_ptr(ptr);
        let raw_abi = *(ptr.add(21 * std::mem::size_of::<u64>()) as *const u64);
        if fn_ptr == 0 {
            assert_eq!(
                raw_abi, CODE_CALL_ABI_UNPUBLISHED,
                "code call ABI published without callable target"
            );
            return None;
        }
        let call_abi = match raw_abi {
            0 => FunctionCallAbi::Positional,
            1 => FunctionCallAbi::LexicalClosureFirst,
            2 => FunctionCallAbi::OpaqueContextFirst,
            raw => panic!("invalid code call ABI {raw}"),
        };
        Some(CodeCallableIdentity {
            fn_ptr,
            trampoline_ptr: code_callable_trampoline_ptr(ptr),
            arity: code_callable_arity(ptr),
            call_abi,
        })
    }
}

/// Project immutable signature facts from a published reconstructible code
/// object. Returning `None` means the function's explicit metadata dictionary
/// remains authoritative (runtime/native setup or unpublished code metadata).
pub(crate) unsafe fn function_code_signature_metadata_bits(
    ptr: *mut u8,
    name: &[u8],
) -> Option<u64> {
    unsafe {
        let code_ptr = obj_from_bits(function_code_bits(ptr)).as_ptr()?;
        if object_type_id(code_ptr) != TYPE_ID_CODE
            || !code_callable_identity(code_ptr)?
                .call_abi
                .is_reconstructible()
            || code_arg_names_bits(code_ptr) == 0
            || code_kwonly_names_bits(code_ptr) == 0
        {
            return None;
        }
        match name {
            b"__molt_arg_names__" => Some(code_arg_names_bits(code_ptr)),
            b"__molt_posonly__" => Some(code_signature_posonly_bits(code_ptr)),
            b"__molt_kwonly_names__" => Some(code_kwonly_names_bits(code_ptr)),
            b"__molt_vararg__" => Some(code_vararg_bits(code_ptr)),
            b"__molt_varkw__" => Some(code_varkw_bits(code_ptr)),
            _ => None,
        }
    }
}

/// Immutable execution policy retained by a compiled code object.
///
/// Task closure size and allocation layout remain owned by the generated
/// trampoline.  This fact only selects that trampoline instead of the ordinary
/// fixed-arity entry and preserves Python's function-kind introspection when a
/// new function is reconstructed from the code object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub(crate) enum CodeExecutionKind {
    Direct = 0,
    Generator = 1,
    Coroutine = 2,
    AsyncGenerator = 3,
}

impl CodeExecutionKind {
    #[inline]
    pub(crate) fn requires_task_trampoline(self) -> bool {
        self != Self::Direct
    }
}

const CODE_EXECUTION_KIND_UNPUBLISHED: u64 = u64::MAX;
const CODE_EXECUTION_KIND_MASK: u64 = 0x03;
pub(crate) const CO_OPTIMIZED: u64 = 0x01;
pub(crate) const CO_NEWLOCALS: u64 = 0x02;
pub(crate) const CO_VARARGS: u64 = 0x04;
pub(crate) const CO_VARKEYWORDS: u64 = 0x08;
pub(crate) const CO_GENERATOR: u64 = 0x20;
pub(crate) const CO_COROUTINE: u64 = 0x80;
pub(crate) const CO_ITERABLE_COROUTINE: u64 = 0x100;
pub(crate) const CO_ASYNC_GENERATOR: u64 = 0x200;
const CODE_PROTOCOL_FLAGS_MASK: u64 = CO_ITERABLE_COROUTINE;

#[inline]
fn published_code_policy(raw: u64) -> (CodeExecutionKind, u64) {
    let unknown = raw & !(CODE_EXECUTION_KIND_MASK | CODE_PROTOCOL_FLAGS_MASK);
    assert_eq!(unknown, 0, "invalid published code policy bits 0x{raw:x}");
    let kind = match raw & CODE_EXECUTION_KIND_MASK {
        0 => CodeExecutionKind::Direct,
        1 => CodeExecutionKind::Generator,
        2 => CodeExecutionKind::Coroutine,
        3 => CodeExecutionKind::AsyncGenerator,
        _ => unreachable!("execution-kind mask is exhaustive"),
    };
    (kind, raw & CODE_PROTOCOL_FLAGS_MASK)
}

pub(crate) unsafe fn code_published_execution_kind(ptr: *mut u8) -> Option<CodeExecutionKind> {
    unsafe {
        let raw = *(ptr.add(18 * std::mem::size_of::<u64>()) as *const u64);
        (raw != CODE_EXECUTION_KIND_UNPUBLISHED).then(|| published_code_policy(raw).0)
    }
}

pub(crate) unsafe fn code_execution_kind(ptr: *mut u8) -> CodeExecutionKind {
    unsafe { code_published_execution_kind(ptr).unwrap_or(CodeExecutionKind::Direct) }
}

/// Publish the compiled execution kind once. Replaying the same publication is
/// harmless; attempting to change an already-published kind is rejected.
pub(crate) unsafe fn code_publish_execution_kind(
    ptr: *mut u8,
    kind: CodeExecutionKind,
) -> Result<(), CodeExecutionKind> {
    unsafe {
        let slot = ptr.add(18 * std::mem::size_of::<u64>()) as *mut u64;
        let raw = *slot;
        if raw == CODE_EXECUTION_KIND_UNPUBLISHED {
            *slot = kind as u64;
            Ok(())
        } else {
            let published = published_code_policy(raw).0;
            if published == kind {
                Ok(())
            } else {
                Err(published)
            }
        }
    }
}

pub(crate) unsafe fn code_protocol_flags(ptr: *mut u8) -> u64 {
    unsafe {
        let raw = *(ptr.add(18 * std::mem::size_of::<u64>()) as *const u64);
        if raw == CODE_EXECUTION_KIND_UNPUBLISHED {
            0
        } else {
            published_code_policy(raw).1
        }
    }
}

/// Publish the complete immutable execution policy of a newly cloned code
/// object. Unlike execution-kind replay, protocol flags must match exactly.
pub(crate) unsafe fn code_publish_policy(
    ptr: *mut u8,
    kind: CodeExecutionKind,
    protocol_flags: u64,
) -> Result<(), u64> {
    assert_eq!(
        protocol_flags & !CODE_PROTOCOL_FLAGS_MASK,
        0,
        "unsupported code protocol flags 0x{protocol_flags:x}"
    );
    unsafe {
        let slot = ptr.add(18 * std::mem::size_of::<u64>()) as *mut u64;
        let published = kind as u64 | protocol_flags;
        if *slot == CODE_EXECUTION_KIND_UNPUBLISHED {
            *slot = published;
            Ok(())
        } else if *slot == published {
            Ok(())
        } else {
            Err(*slot)
        }
    }
}

/// Canonical CPython-visible code flags. Execution kind and protocol bits are
/// immutable code policy; signature slots add the standard vararg flags.
pub(crate) unsafe fn code_flags(ptr: *mut u8) -> u64 {
    unsafe {
        let is_module = obj_from_bits(code_name_bits(ptr))
            .as_ptr()
            .is_some_and(|name_ptr| {
                object_type_id(name_ptr) == TYPE_ID_STRING
                    && std::slice::from_raw_parts(string_bytes(name_ptr), string_len(name_ptr))
                        == b"<module>"
            });
        let mut flags = if is_module {
            0
        } else {
            CO_OPTIMIZED | CO_NEWLOCALS
        };
        flags |= match code_execution_kind(ptr) {
            CodeExecutionKind::Direct => 0,
            CodeExecutionKind::Generator => CO_GENERATOR,
            CodeExecutionKind::Coroutine => CO_COROUTINE,
            CodeExecutionKind::AsyncGenerator => CO_ASYNC_GENERATOR,
        };
        flags |= code_protocol_flags(ptr);
        let vararg_bits = code_vararg_bits(ptr);
        if vararg_bits != 0 && !obj_from_bits(vararg_bits).is_none() {
            flags |= CO_VARARGS;
        }
        let varkw_bits = code_varkw_bits(ptr);
        if varkw_bits != 0 && !obj_from_bits(varkw_bits).is_none() {
            flags |= CO_VARKEYWORDS;
        }
        flags
    }
}

/// Exact compiled-entry identity, independent of source filename and code address.
pub(crate) unsafe fn code_frame_slot_id(ptr: *mut u8) -> Option<u64> {
    unsafe { (*(ptr.add(17 * std::mem::size_of::<u64>()) as *const u64)).checked_sub(1) }
}

pub(crate) unsafe fn code_set_frame_slot_id(ptr: *mut u8, id: u64) {
    unsafe {
        *(ptr.add(17 * std::mem::size_of::<u64>()) as *mut u64) =
            id.checked_add(1).expect("validated code slot");
    }
}

pub(crate) unsafe fn code_arg_names_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(12 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_signature_posonly_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(13 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_kwonly_names_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(14 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_vararg_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(15 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_varkw_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(16 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_freevars_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(19 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn code_cellvars_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(20 * std::mem::size_of::<u64>()) as *const u64) }
}

/// Publish the two positional lexical-name contracts as one immutable unit.
/// Both incoming tuples are validated and retained before either slot changes;
/// replaced values are released only after the pair is visible to reentry.
pub(crate) unsafe fn code_publish_lexical_metadata(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    freevars_bits: u64,
    cellvars_bits: u64,
) -> bool {
    unsafe {
        if validate_code_lexical_metadata(_py, Some(ptr), freevars_bits, cellvars_bits).is_err() {
            return false;
        }
        for bits in publish_code_lexical_metadata_deferred(_py, ptr, [freevars_bits, cellvars_bits])
        {
            dec_ref_bits(_py, bits);
        }
        true
    }
}

unsafe fn publish_code_lexical_metadata_deferred(
    py: &PyToken<'_>,
    ptr: *mut u8,
    lexical: [u64; 2],
) -> [u64; 2] {
    unsafe {
        let mut displaced = [0; 2];
        for bits in lexical {
            inc_ref_bits(py, bits);
        }
        for (offset, bits) in lexical.into_iter().enumerate() {
            let slot = ptr.add((19 + offset) * std::mem::size_of::<u64>()) as *mut u64;
            displaced[offset] = std::mem::replace(&mut *slot, bits);
        }
        displaced
    }
}

pub(crate) unsafe fn validate_code_lexical_metadata(
    _py: &PyToken<'_>,
    ptr: Option<*mut u8>,
    freevars_bits: u64,
    cellvars_bits: u64,
) -> Result<(), ()> {
    unsafe {
        crate::gil_assert();
        use crate::object::heap_kinds_generated::HeapAcyclicSlot;
        if !crate::object::builders::acyclic_slot_edge(HeapAcyclicSlot::CodeFreevars, freevars_bits)
        {
            crate::raise_exception::<u64>(_py, "TypeError", "code freevars must be a tuple of str");
            return Err(());
        }
        if !crate::object::builders::acyclic_slot_edge(HeapAcyclicSlot::CodeCellvars, cellvars_bits)
        {
            crate::raise_exception::<u64>(_py, "TypeError", "code cellvars must be a tuple of str");
            return Err(());
        }

        if let Some(ptr) = ptr
            && code_published_execution_kind(ptr).is_some()
        {
            if code_freevars_bits(ptr) == freevars_bits && code_cellvars_bits(ptr) == cellvars_bits
            {
                return Ok(());
            }
            crate::raise_exception::<u64>(
                _py,
                "TypeError",
                "code lexical metadata is already published",
            );
            return Err(());
        }
        Ok(())
    }
}

/// Validated borrowed signature facts. Preparation is callback-free; publication
/// retains every incoming edge before replacing any slot and defers retirement.
#[must_use]
#[derive(Clone, Copy)]
pub(crate) struct PreparedCodeSignature([u64; 5]);

pub(crate) fn prepare_code_signature(
    py: &PyToken<'_>,
    bits: [u64; 5],
) -> Result<PreparedCodeSignature, ()> {
    use crate::object::heap_kinds_generated::HeapAcyclicSlot;
    let slots = [
        HeapAcyclicSlot::CodeArgNames,
        HeapAcyclicSlot::CodePosonly,
        HeapAcyclicSlot::CodeKwonly,
        HeapAcyclicSlot::CodeVararg,
        HeapAcyclicSlot::CodeVarkw,
    ];
    if !slots
        .into_iter()
        .zip(bits)
        .all(|(slot, bits)| crate::object::builders::acyclic_slot_edge(slot, bits))
    {
        crate::raise_exception::<u64>(
            py,
            "SystemError",
            "code signature mutation violated generated code_metadata acyclic capability",
        );
        return Err(());
    }
    Ok(PreparedCodeSignature(bits))
}

impl PreparedCodeSignature {
    unsafe fn publish(self, py: &PyToken<'_>, ptr: *mut u8) -> [u64; 5] {
        unsafe {
            let mut displaced = [0; 5];
            for bits in self.0 {
                inc_ref_bits(py, bits);
            }
            for (offset, bits) in self.0.into_iter().enumerate() {
                let slot = ptr.add((12 + offset) * std::mem::size_of::<u64>()) as *mut u64;
                displaced[offset] = std::mem::replace(&mut *slot, bits);
            }
            displaced
        }
    }
}

pub(crate) unsafe fn code_set_signature_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    arg_names_bits: u64,
    posonly_bits: u64,
    kwonly_bits: u64,
    vararg_bits: u64,
    varkw_bits: u64,
) -> Result<(), ()> {
    unsafe {
        crate::gil_assert();
        if code_published_execution_kind(ptr).is_some() {
            return Ok(());
        }
        let signature = prepare_code_signature(
            _py,
            [
                arg_names_bits,
                posonly_bits,
                kwonly_bits,
                vararg_bits,
                varkw_bits,
            ],
        )?;
        for old_bits in signature.publish(_py, ptr) {
            if old_bits != 0 {
                dec_ref_bits(_py, old_bits);
            }
        }
        Ok(())
    }
}

unsafe fn prepare_code_signature_from_function_attrs(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> Result<Option<PreparedCodeSignature>, ()> {
    unsafe {
        if let Some(classes) = builtin_classes_if_initialized(_py)
            && object_class_bits(func_ptr) == classes.builtin_function_or_method
        {
            return Ok(None);
        }

        let dict_bits = function_dict_bits(func_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return Ok(None);
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return Ok(None);
        }

        // Metadata lookup must not call equality/hash hooks or allocate interned
        // names while holding borrowed edges from this same dictionary.
        let get = |name| crate::object::ops::dict_get_str_bytes_borrowed(_py, dict_ptr, name);
        let Some(arg_names_bits) = get(b"__molt_arg_names__") else {
            return Ok(None);
        };
        let Some(kwonly_bits) = get(b"__molt_kwonly_names__") else {
            return Ok(None);
        };
        let posonly_bits =
            get(b"__molt_posonly__").unwrap_or_else(|| MoltObject::from_int(0).bits());
        let vararg_bits = get(b"__molt_vararg__").unwrap_or_else(|| MoltObject::none().bits());
        let varkw_bits = get(b"__molt_varkw__").unwrap_or_else(|| MoltObject::none().bits());

        prepare_code_signature(
            _py,
            [
                arg_names_bits,
                posonly_bits,
                kwonly_bits,
                vararg_bits,
                varkw_bits,
            ],
        )
        .map(Some)
    }
}

/// Publish the complete callable identity once. Replaying the exact identity is
/// harmless; a conflict is returned without rewriting executable provenance so
/// callers can reject the incoherent function/code pairing.
pub(crate) unsafe fn code_publish_callable_identity(
    ptr: *mut u8,
    identity: CodeCallableIdentity,
) -> Result<(), CodeCallableIdentity> {
    unsafe {
        code_validate_callable_identity(ptr, identity)?;
        code_publish_callable_identity_unchecked(ptr, identity);
        Ok(())
    }
}

unsafe fn code_validate_callable_identity(
    ptr: *mut u8,
    identity: CodeCallableIdentity,
) -> Result<(), CodeCallableIdentity> {
    unsafe {
        if identity.fn_ptr == 0 {
            return Err(identity);
        }
        if let Some(published) = code_callable_identity(ptr) {
            return if published == identity {
                Ok(())
            } else {
                Err(published)
            };
        }
        Ok(())
    }
}

unsafe fn code_publish_callable_identity_unchecked(ptr: *mut u8, identity: CodeCallableIdentity) {
    unsafe {
        *(ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64) = identity.fn_ptr;
        *(ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64) = identity.trampoline_ptr;
        *(ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64) = identity.arity;
        *(ptr.add(21 * std::mem::size_of::<u64>()) as *mut u64) = identity.call_abi as u64;
    }
}

pub(crate) unsafe fn bound_method_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn bound_method_self_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn module_name_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn module_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_name_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Name.load(ptr) }
}

pub(crate) unsafe fn class_set_name_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        ClassReferenceSlot::Name.replace_borrowed(_py, ptr, bits);
    }
}

pub(crate) unsafe fn class_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Dictionary.load(ptr) }
}

pub(crate) unsafe fn class_bases_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Bases.load(ptr) }
}

pub(crate) unsafe fn class_set_bases_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        ClassReferenceSlot::Bases.replace_borrowed(py, ptr, bits);
    }
}

pub(crate) unsafe fn class_mro_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Mro.load(ptr) }
}

pub(crate) unsafe fn class_layout_version_bits(ptr: *mut u8) -> u64 {
    unsafe {
        (*(ptr.add(4 * std::mem::size_of::<u64>()) as *const std::sync::atomic::AtomicU64))
            .load(std::sync::atomic::Ordering::Acquire)
    }
}

pub(crate) unsafe fn class_set_layout_version_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        (*(ptr.add(4 * std::mem::size_of::<u64>()) as *const std::sync::atomic::AtomicU64))
            .fetch_max(bits, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(crate) unsafe fn class_cached_layout_size(ptr: *mut u8) -> Option<usize> {
    unsafe {
        let cached_size = (*(ptr.add(9 * std::mem::size_of::<u64>())
            as *const std::sync::atomic::AtomicUsize))
            .load(std::sync::atomic::Ordering::Acquire);
        (cached_size != 0).then_some(cached_size)
    }
}

/// Original slot declarations belong to the physical class layout, not the
/// mutable Python namespace. Zero exists only while the class is being built;
/// None records absence and a private immutable tuple records declared names.
#[derive(Clone, Copy)]
pub(crate) enum ClassSlotDeclaration {
    Uninitialized,
    Absent,
    Names(u64),
}

pub(crate) const CLASS_FIELD_OFFSETS_WORD: usize = ClassReferenceSlot::FieldOffsets as usize;
pub(crate) const CLASS_PAYLOAD_WORDS: usize = CLASS_FIELD_OFFSETS_WORD + 1;

pub(crate) unsafe fn class_slot_declaration_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::SlotDeclaration.load(ptr) }
}

pub(crate) unsafe fn class_slot_declaration(ptr: *mut u8) -> ClassSlotDeclaration {
    let bits = unsafe { class_slot_declaration_bits(ptr) };
    if bits == 0 {
        ClassSlotDeclaration::Uninitialized
    } else if obj_from_bits(bits).is_none() {
        ClassSlotDeclaration::Absent
    } else {
        ClassSlotDeclaration::Names(bits)
    }
}

/// Publish one owned, validated declaration. The private tuple is never exposed
/// as a Python attribute; later __slots__ assignment cannot alter this record.
pub(crate) unsafe fn class_set_slot_declaration_owned(ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        assert_eq!(
            class_slot_declaration_bits(ptr),
            0,
            "class slots already captured"
        );
        assert!(
            obj_from_bits(bits).is_none()
                || obj_from_bits(bits)
                    .as_ptr()
                    .is_some_and(|tuple| object_type_id(tuple) == TYPE_ID_TUPLE)
        );
        let old = ClassReferenceSlot::SlotDeclaration.exchange_owned(ptr, bits);
        debug_assert_eq!(old, 0);
    }
}

/// Private physical-layout authority retained at class seal. Zero is reserved
/// for unfinished construction, `None` records a sealed class with no map, and
/// every other value is the exact frozen dict also published in the namespace.
pub(crate) unsafe fn class_field_offsets_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::FieldOffsets.load(ptr) }
}

/// Publish one owned, validated field-offset map edge. The caller transfers an
/// existing owned reference; TYPE lifecycle tracing and detachment own it from
/// this point onward.
pub(crate) unsafe fn class_set_field_offsets_owned(ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        assert_eq!(
            class_field_offsets_bits(ptr),
            0,
            "class field offsets already captured"
        );
        assert!(
            obj_from_bits(bits).is_none()
                || obj_from_bits(bits)
                    .as_ptr()
                    .is_some_and(|map| object_type_id(map) == TYPE_ID_DICT)
        );
        let old = ClassReferenceSlot::FieldOffsets.exchange_owned(ptr, bits);
        debug_assert_eq!(old, 0);
    }
}

pub(crate) unsafe fn class_set_cached_layout_size(ptr: *mut u8, size: usize) {
    unsafe {
        let slot =
            &*(ptr.add(9 * std::mem::size_of::<u64>()) as *const std::sync::atomic::AtomicUsize);
        match slot.compare_exchange(
            0,
            size,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(existing) => assert_eq!(existing, size, "immutable class layout size diverged"),
        }
    }
}

pub(crate) unsafe fn class_annotations_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Annotations.load(ptr) }
}

pub(crate) unsafe fn class_set_annotations_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        ClassReferenceSlot::Annotations.replace_borrowed(_py, ptr, bits);
    }
}

pub(crate) unsafe fn class_annotate_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Annotate.load(ptr) }
}

pub(crate) unsafe fn class_set_annotate_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        ClassReferenceSlot::Annotate.replace_borrowed(_py, ptr, bits);
    }
}

pub(crate) unsafe fn class_qualname_bits(ptr: *mut u8) -> u64 {
    unsafe { ClassReferenceSlot::Qualname.load(ptr) }
}

pub(crate) unsafe fn class_set_qualname_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        ClassReferenceSlot::Qualname.replace_borrowed(_py, ptr, bits);
    }
}

pub(crate) unsafe fn class_bump_layout_version(ptr: *mut u8) {
    unsafe {
        (*(ptr.add(4 * std::mem::size_of::<u64>()) as *const std::sync::atomic::AtomicU64))
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |version| version.checked_add(1),
            )
            .expect("class layout generation exhausted");
    }
    // Also bump the global type version so inline caches are invalidated.
    super::bump_type_version();
}

/// The native prefix carried by every exact wrapper and wrapper subclass.
///
/// The physical heap type id is inherited with the class layout and is the
/// sole discriminator for this prefix. Object shapes remain available for
/// orthogonal class-owned payload families; duplicating wrapper identity there
/// would allow the two authorities to drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WrapperKind {
    Classmethod,
    Staticmethod,
    Property,
}

impl WrapperKind {
    #[inline(always)]
    pub(crate) const fn from_type_id(type_id: u32) -> Option<Self> {
        match type_id {
            TYPE_ID_CLASSMETHOD => Some(Self::Classmethod),
            TYPE_ID_STATICMETHOD => Some(Self::Staticmethod),
            TYPE_ID_PROPERTY => Some(Self::Property),
            _ => None,
        }
    }

    #[inline(always)]
    pub(crate) const fn type_id(self) -> u32 {
        match self {
            Self::Classmethod => TYPE_ID_CLASSMETHOD,
            Self::Staticmethod => TYPE_ID_STATICMETHOD,
            Self::Property => TYPE_ID_PROPERTY,
        }
    }

    /// Reference-valued words owned by the native prefix. Property's final
    /// `getter_doc` word is an immediate bool and deliberately excluded.
    #[inline(always)]
    pub(crate) const fn reference_words(self) -> usize {
        match self {
            Self::Classmethod | Self::Staticmethod => 1,
            Self::Property => 5,
        }
    }

    #[inline(always)]
    pub(crate) const fn prefix_words(self) -> usize {
        match self {
            Self::Classmethod | Self::Staticmethod => 1,
            Self::Property => 6,
        }
    }

    #[inline(always)]
    pub(crate) const fn prefix_size(self) -> usize {
        self.prefix_words() * std::mem::size_of::<u64>()
    }
}

#[inline(always)]
pub(crate) const fn wrapper_prefix_size_for_type_id(type_id: u32) -> usize {
    match WrapperKind::from_type_id(type_id) {
        Some(kind) => kind.prefix_size(),
        None => 0,
    }
}

#[inline(always)]
pub(crate) unsafe fn wrapper_reference_bits(ptr: *mut u8, index: usize) -> u64 {
    let kind = WrapperKind::from_type_id(unsafe { object_type_id(ptr) })
        .expect("wrapper reference access requires wrapper storage");
    assert!(
        unsafe { super::object_payload_size(ptr) } >= kind.prefix_size(),
        "wrapper reference access requires a complete native prefix"
    );
    assert!(
        index < kind.reference_words(),
        "wrapper reference index exceeds native prefix"
    );
    unsafe { *ptr.cast::<u64>().add(index) }
}

/// Canonical empty state for one wrapper-owned reference word. Property name
/// deliberately differs from its other references: missing means no explicit
/// `__name__` has been assigned and permits fallback to `fget.__name__`, while
/// an explicitly stored `None` remains observable as `None`.
#[inline]
pub(crate) fn wrapper_empty_reference_bits(
    py: &PyToken<'_>,
    kind: WrapperKind,
    index: usize,
) -> Option<u64> {
    if index >= kind.reference_words() {
        return None;
    }
    if matches!(kind, WrapperKind::Classmethod | WrapperKind::Staticmethod)
        || (kind == WrapperKind::Property && index == 4)
    {
        let missing = crate::missing_bits(py);
        return obj_from_bits(missing).as_ptr().map(|_| missing);
    }
    Some(MoltObject::none().bits())
}

/// Initialize the hidden native prefix before any declared field or GC
/// publication can observe the object. Zero is reserved for allocation failure
/// cleanup and never represents a Python value. Class/static wrappers use the
/// missing singleton so an uninitialized wrapper remains distinct from one
/// explicitly initialized with None; property exposes absent accessors as None.
#[must_use]
pub(crate) unsafe fn wrapper_initialize_prefix_unpublished(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let Some(kind) = WrapperKind::from_type_id(unsafe { object_type_id(ptr) }) else {
        return true;
    };
    if unsafe { super::object_payload_size(ptr) } < kind.prefix_size() {
        return false;
    }
    unsafe {
        for index in 0..kind.reference_words() {
            let Some(empty) = wrapper_empty_reference_bits(py, kind, index) else {
                return false;
            };
            *ptr.cast::<u64>().add(index) = empty;
        }
        if kind == WrapperKind::Property {
            *ptr.cast::<u64>().add(5) = MoltObject::from_bool(false).bits();
        }
    }
    true
}

#[inline]
#[must_use]
unsafe fn wrapper_replace_reference_bits(
    py: &PyToken<'_>,
    ptr: *mut u8,
    expected: WrapperKind,
    index: usize,
    bits: u64,
) -> bool {
    if WrapperKind::from_type_id(unsafe { object_type_id(ptr) }) != Some(expected)
        || index >= expected.reference_words()
        || unsafe { super::object_payload_size(ptr) } < expected.prefix_size()
    {
        return false;
    }
    // Raw zero is the allocation-failure carrier at the runtime ABI. It is not
    // a Python value and must never be normalized into a visible `None` edge.
    if bits == 0 {
        return false;
    }
    unsafe {
        let slot = ptr.cast::<u64>().add(index);
        let old = *slot;
        if old == bits {
            return true;
        }
        // Retain before publishing and release only after the slot owns the new
        // value. A finalizer on the displaced edge may re-enter this wrapper.
        if bits != 0 {
            inc_ref_bits(py, bits);
        }
        *slot = bits;
        if old != 0 {
            dec_ref_bits(py, old);
        }
    }
    true
}

#[must_use]
pub(crate) unsafe fn classmethod_replace_func_bits(
    py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Classmethod, 0, bits) }
}

#[must_use]
pub(crate) unsafe fn staticmethod_replace_func_bits(
    py: &PyToken<'_>,
    ptr: *mut u8,
    bits: u64,
) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Staticmethod, 0, bits) }
}

#[must_use]
pub(crate) unsafe fn property_replace_get_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Property, 0, bits) }
}

#[must_use]
pub(crate) unsafe fn property_replace_set_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Property, 1, bits) }
}

#[must_use]
pub(crate) unsafe fn property_replace_del_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Property, 2, bits) }
}

#[must_use]
pub(crate) unsafe fn property_replace_doc_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Property, 3, bits) }
}

#[must_use]
pub(crate) unsafe fn property_replace_name_bits(py: &PyToken<'_>, ptr: *mut u8, bits: u64) -> bool {
    unsafe { wrapper_replace_reference_bits(py, ptr, WrapperKind::Property, 4, bits) }
}

pub(crate) unsafe fn classmethod_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn staticmethod_func_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn property_get_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn property_set_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn property_del_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn property_doc_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn property_name_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn property_getter_doc(ptr: *mut u8) -> bool {
    unsafe {
        obj_from_bits(*(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64))
            .as_bool()
            .unwrap_or(false)
    }
}

pub(crate) unsafe fn property_set_getter_doc(ptr: *mut u8, getter_doc: bool) {
    unsafe {
        *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) =
            MoltObject::from_bool(getter_doc).bits();
    }
}

/// Immutable representation shared by builtin member and getset descriptors.
/// The class edge owns the public descriptor type identity; this immediate is
/// the protocol/error-policy discriminator and never a Python reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum NativeDescriptorFlavor {
    Member = 1,
    GetSet = 2,
}

impl NativeDescriptorFlavor {
    #[inline]
    pub(crate) const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            1 => Some(Self::Member),
            2 => Some(Self::GetSet),
            _ => None,
        }
    }
}

pub(crate) const NATIVE_DESCRIPTOR_REFERENCE_WORDS: usize = 6;
pub(crate) const NATIVE_DESCRIPTOR_PREFIX_WORDS: usize = 7;
pub(crate) const NATIVE_DESCRIPTOR_PREFIX_SIZE: usize =
    NATIVE_DESCRIPTOR_PREFIX_WORDS * std::mem::size_of::<u64>();

#[inline]
fn native_descriptor_storage_is_valid(ptr: *mut u8) -> bool {
    !ptr.is_null()
        && unsafe { object_type_id(ptr) } == TYPE_ID_NATIVE_DESCRIPTOR
        && unsafe { super::object_payload_size(ptr) } >= NATIVE_DESCRIPTOR_PREFIX_SIZE
}

#[inline]
pub(crate) unsafe fn native_descriptor_reference_bits(ptr: *mut u8, index: usize) -> u64 {
    assert!(
        native_descriptor_storage_is_valid(ptr),
        "native descriptor access requires a complete descriptor prefix"
    );
    assert!(
        index < NATIVE_DESCRIPTOR_REFERENCE_WORDS,
        "native descriptor reference index exceeds its prefix"
    );
    unsafe { *ptr.cast::<u64>().add(index) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_owner_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 0) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_name_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 1) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_doc_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 2) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_getter_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 3) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_setter_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 4) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_deleter_bits(ptr: *mut u8) -> u64 {
    unsafe { native_descriptor_reference_bits(ptr, 5) }
}

#[inline]
pub(crate) unsafe fn native_descriptor_flavor(ptr: *mut u8) -> Option<NativeDescriptorFlavor> {
    if !native_descriptor_storage_is_valid(ptr) {
        return None;
    }
    NativeDescriptorFlavor::from_raw(unsafe {
        *ptr.cast::<u64>().add(NATIVE_DESCRIPTOR_REFERENCE_WORDS)
    })
}

pub(crate) unsafe fn super_type_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn super_obj_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

/// Receiver class resolved once by supercheck, including proxy __class__.
pub(crate) unsafe fn super_receiver_class_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) fn range_len_i64(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            return 0;
        }
        let span = stop - start - 1;
        return 1 + span / step;
    }
    if start <= stop {
        return 0;
    }
    let step_abs = -step;
    let span = start - stop - 1;
    1 + span / step_abs
}

#[cfg(test)]
mod tests {
    use super::{
        ListIntStorage, ensure_function_code_bits, function_arity_usize, function_code_bits,
        function_set_code_bits, zip_set_strict_bits, zip_strict_bits,
    };
    use crate::object::header_from_obj_ptr;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    use crate::{alloc_function_obj, alloc_string, dec_ref_bits, inc_ref_bits, obj_from_bits};
    use molt_obj_model::MoltObject;

    #[test]
    fn function_arity_usize_preserves_the_active_target_width() {
        let words = [0_u64, usize::MAX as u64];
        let ptr = words.as_ptr().cast_mut().cast::<u8>();
        assert_eq!(unsafe { function_arity_usize(ptr) }, Some(usize::MAX));
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn function_arity_usize_rejects_high_bits() {
        let words = [0_u64, u64::from(u32::MAX) + 1];
        let ptr = words.as_ptr().cast_mut().cast::<u8>();
        assert_eq!(unsafe { function_arity_usize(ptr) }, None);
    }

    unsafe fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn alloc_test_code(
        _py: &crate::PyToken<'_>,
        filename_bits: u64,
        name_bits: u64,
        firstlineno: i64,
    ) -> *mut u8 {
        let empty_tuple_ptr = crate::alloc_tuple(_py, &[]);
        let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
        let code_ptr = crate::alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            firstlineno,
            MoltObject::none().bits(),
            empty_tuple_bits,
            empty_tuple_bits,
            0,
            0,
            0,
        );
        dec_ref_bits(_py, empty_tuple_bits);
        code_ptr
    }

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[test]
    fn list_int_storage_denied_growth_keeps_original_buffer() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let owner_bytes = std::mem::size_of::<ListIntStorage>();
        let initial_buffer = 4 * std::mem::size_of::<i64>();
        let replacement_buffer = 16 * std::mem::size_of::<i64>();
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(owner_bytes + initial_buffer + replacement_buffer - 1),
            ..Default::default()
        })));
        let _reset = TrackerReset;

        let ptr = ListIntStorage::from_slice(&[1, 2, 3, 4]).expect("storage");
        unsafe {
            let storage = &mut *ptr;
            let original_data = storage.data;
            assert!(!storage.reserve_for_len(16));
            assert_eq!(storage.cap, 4);
            assert_eq!(storage.data, original_data);
            assert_eq!(
                std::slice::from_raw_parts(storage.data, storage.len),
                &[1, 2, 3, 4]
            );
            drop((*Box::from_raw(ptr)).into_vec());
        }
    }

    #[test]
    fn zip_strict_bits_unaligned_roundtrip() {
        let mut buf = [0u8; 32];
        let ptr = unsafe { buf.as_mut_ptr().add(1) };
        let value = 0xA5A5_5A5A_DEAD_BEEFu64;
        unsafe {
            zip_set_strict_bits(ptr, value);
            assert_eq!(zip_strict_bits(ptr), value);
        }
    }

    #[test]
    fn function_code_slot_retains_and_releases_borrowed_code() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = alloc_function_obj(_py, 0xF00D, 0);
            assert_eq!(unsafe { function_code_bits(func_ptr) }, 0);

            let code_bits = unsafe { ensure_function_code_bits(_py, func_ptr) };
            let code_ptr = obj_from_bits(code_bits).as_ptr().unwrap();
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);
            assert_eq!(unsafe { function_code_bits(func_ptr) }, code_bits);
            inc_ref_bits(_py, code_bits);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);

            let filename_ptr = alloc_string(_py, b"<replacement-code>");
            let name_ptr = alloc_string(_py, b"<replacement-code-name>");
            let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let replacement_ptr = alloc_test_code(_py, filename_bits, name_bits, 3);
            dec_ref_bits(_py, filename_bits);
            dec_ref_bits(_py, name_bits);
            let replacement_bits = MoltObject::from_ptr(replacement_ptr).bits();
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 1);

            assert!(unsafe { function_set_code_bits(_py, func_ptr, replacement_bits) });
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 2);
            dec_ref_bits(_py, code_bits);

            dec_ref_bits(_py, replacement_bits);
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 1);

            inc_ref_bits(_py, replacement_bits);
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 2);
            dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 1);
            dec_ref_bits(_py, replacement_bits);
        })
    }

    #[test]
    fn rejected_signature_attachment_keeps_identity_edges_and_epoch_retryable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let function = alloc_function_obj(py, 0xF00D, 0);
                let function_bits = MoltObject::from_ptr(function).bits();
                let original = ensure_function_code_bits(py, function);
                inc_ref_bits(py, original);
                let original_ptr = obj_from_bits(original).as_ptr().unwrap();
                let name = alloc_string(py, b"signature-attachment-transaction");
                let name_bits = MoltObject::from_ptr(name).bits();
                let code = alloc_test_code(py, name_bits, name_bits, 1);
                let code_bits = MoltObject::from_ptr(code).bits();
                // Mortal tuples expose the retain/retire transitions; the
                // canonical empty tuple is immortal.
                let empty = crate::alloc_tuple(py, &[name_bits]);
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let invalid = crate::alloc_list(py, &[]);
                let invalid_bits = MoltObject::from_ptr(invalid).bits();
                let arg_key = alloc_string(py, b"__molt_arg_names__");
                let kw_key = alloc_string(py, b"__molt_kwonly_names__");
                let varkw_key = alloc_string(py, b"__molt_varkw__");
                let arg_key_bits = MoltObject::from_ptr(arg_key).bits();
                let kw_key_bits = MoltObject::from_ptr(kw_key).bits();
                let varkw_key_bits = MoltObject::from_ptr(varkw_key).bits();
                let dict = crate::alloc_dict_with_pairs(
                    py,
                    &[
                        arg_key_bits,
                        empty_bits,
                        kw_key_bits,
                        empty_bits,
                        varkw_key_bits,
                        invalid_bits,
                    ],
                );
                assert!(!dict.is_null());
                super::function_set_dict_bits(function, MoltObject::from_ptr(dict).bits());
                let epoch = super::function_mutation_version(function);
                let tuple_refs = ref_count(empty);
                let invalid_refs = ref_count(invalid);

                assert!(!function_set_code_bits(py, function, code_bits));
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
                assert_eq!(super::code_callable_identity(code), None);
                assert_eq!(super::code_published_execution_kind(code), None);
                assert_eq!(function_code_bits(function), original);
                assert_eq!(super::function_mutation_version(function), epoch);
                assert_eq!(ref_count(code), 1);
                assert_eq!(ref_count(original_ptr), 2);
                assert_eq!(ref_count(empty), tuple_refs);
                assert_eq!(ref_count(invalid), invalid_refs);
                for offset in 12..=16 {
                    assert_eq!(
                        *(code.add(offset * std::mem::size_of::<u64>()) as *const u64),
                        0
                    );
                }

                crate::dict_set_in_place(py, dict, varkw_key_bits, MoltObject::none().bits());
                assert!(!crate::exception_pending(py));
                assert!(function_set_code_bits(py, function, code_bits));
                assert_eq!(super::code_callable_identity(code).unwrap().fn_ptr, 0xF00D);
                assert_eq!(super::code_arg_names_bits(code), empty_bits);
                assert_eq!(super::code_kwonly_names_bits(code), empty_bits);
                assert_eq!(function_code_bits(function), code_bits);
                assert_eq!(ref_count(code), 2);
                assert_eq!(ref_count(original_ptr), 1);
                assert_eq!(ref_count(empty), tuple_refs + 2);
                assert_eq!(ref_count(invalid), invalid_refs - 1);

                // Once identity is published, the code's signature owns the
                // facts; replay must not reinterpret stale function metadata.
                crate::dict_set_in_place(py, dict, varkw_key_bits, invalid_bits);
                assert!(function_set_code_bits(py, function, code_bits));
                assert!(!crate::exception_pending(py));
                assert_eq!(super::code_varkw_bits(code), MoltObject::none().bits());
                assert_eq!(ref_count(code), 2);
                assert_eq!(ref_count(empty), tuple_refs + 2);

                for bits in [
                    function_bits,
                    original,
                    code_bits,
                    empty_bits,
                    invalid_bits,
                    name_bits,
                    arg_key_bits,
                    kw_key_bits,
                    varkw_key_bits,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn signature_and_identity_preflight_never_publish_partial_sibling_slots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let name = alloc_string(py, b"signature-sibling-transaction");
                let name_bits = MoltObject::from_ptr(name).bits();
                let code = alloc_test_code(py, name_bits, name_bits, 1);
                let code_bits = MoltObject::from_ptr(code).bits();
                let old = crate::alloc_tuple(py, &[name_bits]);
                let next = crate::alloc_tuple(py, &[name_bits, name_bits]);
                let invalid = crate::alloc_list(py, &[]);
                let old_bits = MoltObject::from_ptr(old).bits();
                let next_bits = MoltObject::from_ptr(next).bits();
                let invalid_bits = MoltObject::from_ptr(invalid).bits();
                let none = MoltObject::none().bits();
                let zero = MoltObject::from_int(0).bits();
                super::code_set_signature_bits(py, code, old_bits, zero, old_bits, none, none)
                    .expect("initial signature");
                let old_refs = ref_count(old);
                let next_refs = ref_count(next);
                assert!(
                    super::code_set_signature_bits(
                        py,
                        code,
                        next_bits,
                        MoltObject::from_int(1).bits(),
                        next_bits,
                        name_bits,
                        invalid_bits,
                    )
                    .is_err()
                );
                crate::clear_exception(py);
                assert_eq!(super::code_arg_names_bits(code), old_bits);
                assert_eq!(super::code_signature_posonly_bits(code), zero);
                assert_eq!(super::code_kwonly_names_bits(code), old_bits);
                assert_eq!(super::code_vararg_bits(code), none);
                assert_eq!(super::code_varkw_bits(code), none);
                assert_eq!(ref_count(old), old_refs);
                assert_eq!(ref_count(next), next_refs);

                let owner = alloc_function_obj(py, 0x101, 0);
                let conflicting = alloc_function_obj(py, 0x202, 0);
                assert!(function_set_code_bits(py, owner, code_bits));
                let identity = super::code_callable_identity(code);
                let signature =
                    super::prepare_code_signature(py, [next_bits, zero, next_bits, none, none])
                        .expect("valid alternative signature");
                assert!(
                    super::prepare_function_code_bits(
                        py,
                        conflicting,
                        code_bits,
                        None,
                        Some(signature),
                    )
                    .is_err()
                );
                crate::clear_exception(py);
                assert_eq!(super::code_callable_identity(code), identity);
                assert_eq!(super::code_arg_names_bits(code), old_bits);
                assert_eq!(function_code_bits(conflicting), 0);
                assert_eq!(ref_count(old), old_refs);
                assert_eq!(ref_count(next), next_refs);
                for bits in [
                    MoltObject::from_ptr(owner).bits(),
                    MoltObject::from_ptr(conflicting).bits(),
                    code_bits,
                    old_bits,
                    next_bits,
                    invalid_bits,
                    name_bits,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn metadata_initializers_reject_invalid_signature_before_any_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                for packed in [false, true] {
                    let name = alloc_string(py, b"metadata-signature-transaction");
                    let name_bits = MoltObject::from_ptr(name).bits();
                    let empty = crate::alloc_tuple(py, &[]);
                    let empty_bits = MoltObject::from_ptr(empty).bits();
                    let invalid = crate::alloc_list(py, &[]);
                    let invalid_bits = MoltObject::from_ptr(invalid).bits();
                    let function = alloc_function_obj(py, 0xF00D, 0);
                    let function_bits = MoltObject::from_ptr(function).bits();
                    let code = alloc_test_code(py, name_bits, name_bits, 1);
                    let code_bits = MoltObject::from_ptr(code).bits();
                    let original_freevars = super::code_freevars_bits(code);
                    let original_cellvars = super::code_cellvars_bits(code);
                    let epoch = super::function_mutation_version(function);
                    let empty_refs = ref_count(empty);
                    let none = MoltObject::none().bits();
                    let zero = MoltObject::from_int(0).bits();

                    for varkw in [invalid_bits, none] {
                        if packed {
                            let metadata = crate::alloc_tuple(
                                py,
                                &[
                                    name_bits, name_bits, none, empty_bits, zero, empty_bits, none,
                                    varkw, none, none, none, zero, empty_bits, empty_bits,
                                ],
                            );
                            let metadata_bits = MoltObject::from_ptr(metadata).bits();
                            crate::builtins::functions::molt_function_init_metadata_packed(
                                function_bits,
                                metadata_bits,
                                code_bits,
                                none,
                            );
                            dec_ref_bits(py, metadata_bits);
                        } else {
                            crate::builtins::functions::molt_function_init_metadata(
                                function_bits,
                                name_bits,
                                name_bits,
                                none,
                                empty_bits,
                                zero,
                                empty_bits,
                                none,
                                varkw,
                                none,
                                none,
                                none,
                                code_bits,
                                none,
                            );
                        }
                        if varkw == invalid_bits {
                            assert!(crate::exception_pending(py));
                            crate::clear_exception(py);
                            assert_eq!(super::function_dict_bits(function), 0);
                            assert_eq!(function_code_bits(function), 0);
                            assert_eq!(super::function_mutation_version(function), epoch);
                            assert_eq!(super::code_callable_identity(code), None);
                            assert_eq!(super::code_published_execution_kind(code), None);
                            assert_eq!(super::code_arg_names_bits(code), 0);
                            assert_eq!(super::code_freevars_bits(code), original_freevars);
                            assert_eq!(super::code_cellvars_bits(code), original_cellvars);
                            assert_eq!(ref_count(code), 1);
                            assert_eq!(ref_count(empty), empty_refs);
                            assert_eq!(ref_count(invalid), 1);
                        } else {
                            assert!(!crate::exception_pending(py));
                            assert_eq!(function_code_bits(function), code_bits);
                            assert_eq!(super::code_callable_identity(code).unwrap().fn_ptr, 0xF00D);
                            assert_eq!(super::code_arg_names_bits(code), empty_bits);
                            assert_eq!(super::code_kwonly_names_bits(code), empty_bits);
                            assert_eq!(super::function_mutation_version(function), epoch);
                            if packed {
                                assert_eq!(
                                    super::code_published_execution_kind(code),
                                    Some(super::CodeExecutionKind::Direct)
                                );
                                assert_eq!(super::code_freevars_bits(code), empty_bits);
                                assert_eq!(super::code_cellvars_bits(code), empty_bits);
                            }
                        }
                    }
                    for bits in [
                        function_bits,
                        code_bits,
                        empty_bits,
                        invalid_bits,
                        name_bits,
                    ] {
                        dec_ref_bits(py, bits);
                    }
                }
            }
        });
    }
}
