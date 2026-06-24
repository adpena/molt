use crate::{
    MoltObject, PyToken, TYPE_ID_CODE, TYPE_ID_DICT, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_code_obj,
    alloc_string, alloc_tuple, builtin_classes_if_initialized, dec_ref_bits, dict_get_in_place,
    fn_ptr_code_set, inc_ref_bits, intern_static_name, obj_from_bits, object_class_bits,
    object_type_id, runtime_state,
};

pub(crate) unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn seq_vec(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = seq_vec_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    unsafe {
        let vec_ptr = seq_vec_ptr(ptr);
        &*vec_ptr
    }
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

pub(crate) unsafe fn function_set_closure_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
        if bits != 0 {
            inc_ref_bits(_py, bits);
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

pub(crate) unsafe fn function_set_code_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != bits {
            if bits != 0 {
                inc_ref_bits(_py, bits);
            }
            *slot = bits;
        }
        let fn_ptr = function_fn_ptr(ptr);
        if let Some(code_ptr) = obj_from_bits(bits).as_ptr()
            && object_type_id(code_ptr) == TYPE_ID_CODE
        {
            code_set_callable_identity_if_empty(
                code_ptr,
                fn_ptr,
                function_trampoline_ptr(ptr),
                function_arity(ptr),
            );
            code_set_signature_bits_from_function_attrs(_py, code_ptr, ptr);
        }
        fn_ptr_code_set(_py, fn_ptr, bits);
        if old_bits != bits && old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
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

/// Read the `__defaults__`/`__kwdefaults__` mutation version stamp (slot 10).
///
/// 0 means the function's defaults have never been mutated since creation, so a
/// compile-time-baked literal default is still observably correct. Any
/// non-zero value means a `func.__defaults__ = ...` / `func.__kwdefaults__ = ...`
/// reassignment has occurred and a call must read the LIVE tuple/dict instead.
pub(crate) unsafe fn function_defaults_version(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(10 * std::mem::size_of::<u64>()) as *const u64) }
}

/// Bump the `__defaults__`/`__kwdefaults__` mutation version stamp (slot 10).
///
/// Called from the single user-reachable mutation site (the generic function
/// attribute setter for `__defaults__`/`__kwdefaults__`). NOT called from the
/// function-creation path, so a freshly-created function keeps version 0. The
/// counter is a plain u64; wrap-around requires 2^64 mutations and is harmless
/// (the guard only distinguishes 0 from non-0).
pub(crate) unsafe fn function_bump_defaults_version(ptr: *mut u8) {
    unsafe {
        let slot = ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64;
        *slot = (*slot).wrapping_add(1);
    }
}

pub(crate) unsafe fn function_globals_override_enabled(ptr: *mut u8) -> bool {
    unsafe { *(ptr.add(11 * std::mem::size_of::<u64>()) as *const u64) != 0 }
}

pub(crate) unsafe fn function_set_globals_override_enabled(ptr: *mut u8, enabled: bool) {
    unsafe {
        *(ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64) = u64::from(enabled);
    }
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
        function_set_code_bits(_py, func_ptr, code_bits);
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

pub(crate) unsafe fn code_set_signature_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    arg_names_bits: u64,
    posonly_bits: u64,
    kwonly_bits: u64,
    vararg_bits: u64,
    varkw_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        for (idx, bits) in [
            (12usize, arg_names_bits),
            (13usize, posonly_bits),
            (14usize, kwonly_bits),
            (15usize, vararg_bits),
            (16usize, varkw_bits),
        ] {
            let slot = ptr.add(idx * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *slot;
            if old_bits == bits {
                continue;
            }
            if bits != 0 {
                inc_ref_bits(_py, bits);
            }
            *slot = bits;
            if old_bits != 0 {
                dec_ref_bits(_py, old_bits);
            }
        }
    }
}

unsafe fn code_set_signature_bits_from_function_attrs(
    _py: &PyToken<'_>,
    code_ptr: *mut u8,
    func_ptr: *mut u8,
) {
    unsafe {
        if let Some(classes) = builtin_classes_if_initialized(_py)
            && object_class_bits(func_ptr) == classes.builtin_function_or_method
        {
            return;
        }

        let dict_bits = function_dict_bits(func_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }

        let interned = &runtime_state(_py).interned;
        let arg_names_attr =
            intern_static_name(_py, &interned.molt_arg_names, b"__molt_arg_names__");
        let Some(arg_names_bits) = dict_get_in_place(_py, dict_ptr, arg_names_attr) else {
            return;
        };
        let Some(arg_names_ptr) = obj_from_bits(arg_names_bits).as_ptr() else {
            return;
        };
        if object_type_id(arg_names_ptr) != TYPE_ID_TUPLE {
            return;
        }

        let kwonly_attr =
            intern_static_name(_py, &interned.molt_kwonly_names, b"__molt_kwonly_names__");
        let Some(kwonly_bits) = dict_get_in_place(_py, dict_ptr, kwonly_attr) else {
            return;
        };
        let Some(kwonly_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
            return;
        };
        if object_type_id(kwonly_ptr) != TYPE_ID_TUPLE {
            return;
        }

        let posonly_attr = intern_static_name(_py, &interned.molt_posonly, b"__molt_posonly__");
        let posonly_bits = dict_get_in_place(_py, dict_ptr, posonly_attr)
            .unwrap_or_else(|| MoltObject::from_int(0).bits());
        let vararg_attr = intern_static_name(_py, &interned.molt_vararg, b"__molt_vararg__");
        let vararg_bits = dict_get_in_place(_py, dict_ptr, vararg_attr)
            .unwrap_or_else(|| MoltObject::none().bits());
        let varkw_attr = intern_static_name(_py, &interned.molt_varkw, b"__molt_varkw__");
        let varkw_bits = dict_get_in_place(_py, dict_ptr, varkw_attr)
            .unwrap_or_else(|| MoltObject::none().bits());

        code_set_signature_bits(
            _py,
            code_ptr,
            arg_names_bits,
            posonly_bits,
            kwonly_bits,
            vararg_bits,
            varkw_bits,
        );
    }
}

unsafe fn code_set_callable_identity_if_empty(
    ptr: *mut u8,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
) {
    unsafe {
        if code_callable_fn_ptr(ptr) != 0 || fn_ptr == 0 {
            return;
        }
        *(ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64) = fn_ptr;
        *(ptr.add(10 * std::mem::size_of::<u64>()) as *mut u64) = trampoline_ptr;
        *(ptr.add(11 * std::mem::size_of::<u64>()) as *mut u64) = arity;
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
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn class_set_name_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr as *mut u64;
        let old_bits = *slot;
        if old_bits != bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
        }
    }
}

pub(crate) unsafe fn class_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_bases_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_bases_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

pub(crate) unsafe fn class_mro_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_mro_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

pub(crate) unsafe fn class_layout_version_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_layout_version_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

pub(crate) unsafe fn class_annotations_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_annotations_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64;
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

pub(crate) unsafe fn class_annotate_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_annotate_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
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

pub(crate) unsafe fn class_qualname_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64) }
}

pub(crate) unsafe fn class_set_qualname_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
        }
    }
}

pub(crate) unsafe fn class_bump_layout_version(ptr: *mut u8) {
    unsafe {
        let current = class_layout_version_bits(ptr);
        class_set_layout_version_bits(ptr, current.wrapping_add(1));
    }
    // Also bump the global type version so inline caches are invalidated.
    super::bump_type_version();
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

pub(crate) unsafe fn super_type_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn super_obj_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
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
        ListIntStorage, ensure_function_code_bits, function_code_bits, function_set_code_bits,
        zip_set_strict_bits, zip_strict_bits,
    };
    use crate::object::header_from_obj_ptr;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    use crate::{
        alloc_function_obj, alloc_string, dec_ref_bits, fn_ptr_code_get, fn_ptr_code_set,
        inc_ref_bits, obj_from_bits,
    };
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    unsafe fn ref_count(ptr: *mut u8) -> u32 {
        unsafe {
            (*header_from_obj_ptr(ptr))
                .ref_count
                .load(Ordering::Relaxed)
        }
    }

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[test]
    fn list_int_storage_denied_growth_keeps_original_buffer() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
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
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = alloc_function_obj(_py, 0, 0);
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
            let replacement_ptr = crate::alloc_code_obj(
                _py,
                filename_bits,
                name_bits,
                3,
                MoltObject::none().bits(),
                0,
                0,
                0,
                0,
                0,
            );
            dec_ref_bits(_py, filename_bits);
            dec_ref_bits(_py, name_bits);
            let replacement_bits = MoltObject::from_ptr(replacement_ptr).bits();
            assert_eq!(unsafe { ref_count(replacement_ptr) }, 1);

            unsafe { function_set_code_bits(_py, func_ptr, replacement_bits) };
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
    fn fn_ptr_code_map_retain_replace_and_clear_release_outside_slot_owner() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let key = 0xF00D_C0DE_5151_0001;
            fn_ptr_code_set(_py, key, 0);

            let filename_a_ptr = alloc_string(_py, b"<fn-ptr-code-a>");
            let name_a_ptr = alloc_string(_py, b"<fn-ptr-code-a-name>");
            let filename_a_bits = MoltObject::from_ptr(filename_a_ptr).bits();
            let name_a_bits = MoltObject::from_ptr(name_a_ptr).bits();
            let code_a_ptr = crate::alloc_code_obj(
                _py,
                filename_a_bits,
                name_a_bits,
                5,
                MoltObject::none().bits(),
                0,
                0,
                0,
                0,
                0,
            );
            dec_ref_bits(_py, filename_a_bits);
            dec_ref_bits(_py, name_a_bits);
            let code_a_bits = MoltObject::from_ptr(code_a_ptr).bits();
            inc_ref_bits(_py, code_a_bits);

            let filename_b_ptr = alloc_string(_py, b"<fn-ptr-code-b>");
            let name_b_ptr = alloc_string(_py, b"<fn-ptr-code-b-name>");
            let filename_b_bits = MoltObject::from_ptr(filename_b_ptr).bits();
            let name_b_bits = MoltObject::from_ptr(name_b_ptr).bits();
            let code_b_ptr = crate::alloc_code_obj(
                _py,
                filename_b_bits,
                name_b_bits,
                9,
                MoltObject::none().bits(),
                0,
                0,
                0,
                0,
                0,
            );
            dec_ref_bits(_py, filename_b_bits);
            dec_ref_bits(_py, name_b_bits);
            let code_b_bits = MoltObject::from_ptr(code_b_ptr).bits();
            inc_ref_bits(_py, code_b_bits);

            assert_eq!(unsafe { ref_count(code_a_ptr) }, 2);
            fn_ptr_code_set(_py, key, code_a_bits);
            assert_eq!(fn_ptr_code_get(_py, key), code_a_bits);
            assert_eq!(unsafe { ref_count(code_a_ptr) }, 3);
            dec_ref_bits(_py, code_a_bits);
            assert_eq!(unsafe { ref_count(code_a_ptr) }, 2);

            fn_ptr_code_set(_py, key, code_b_bits);
            assert_eq!(fn_ptr_code_get(_py, key), code_b_bits);
            assert_eq!(unsafe { ref_count(code_b_ptr) }, 3);
            assert_eq!(unsafe { ref_count(code_a_ptr) }, 1);
            dec_ref_bits(_py, code_a_bits);
            dec_ref_bits(_py, code_b_bits);
            assert_eq!(unsafe { ref_count(code_b_ptr) }, 2);

            fn_ptr_code_set(_py, key, 0);
            assert_eq!(fn_ptr_code_get(_py, key), 0);
            assert_eq!(unsafe { ref_count(code_b_ptr) }, 1);
            dec_ref_bits(_py, code_b_bits);
        })
    }
}
