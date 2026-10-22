//! Physical `PyListObject` storage owned by the native PyMem allocator.
//!
//! Callers admit a genuine native list and hold the runtime GIL through
//! `ListRead`. Managed views retain their Rust backing and never enter here.
//! Pointer moves transfer existing references; callbacks only run after the
//! list has a complete, published shape.

use crate::abi_types::{Py_ssize_t, PyListObject, PyObject};
use crate::api::{errors, memory, refcount, typeobj};
use molt_lang_obj_model::timsort::{RunDetection, TimsortError, timsort};
use std::ptr;

/// Release an already-detached buffer, preserving CPython's reverse item order.
/// Clear, sort restoration and terminal deallocation share this ownership step.
pub(crate) unsafe fn release_buffer(items: *mut *mut PyObject, len: Py_ssize_t) {
    if items.is_null() {
        return;
    }
    for index in (0..len.max(0) as usize).rev() {
        unsafe { refcount::Py_XDECREF(*items.add(index)) };
    }
    unsafe { memory::PyMem_Free(items.cast()) };
}

/// Publish empty before releasing any item. Reentrant additions survive.
pub(crate) unsafe fn clear_native(list: *mut PyListObject) -> std::os::raw::c_int {
    unsafe {
        let items = (*list).ob_item;
        if !items.is_null() {
            let len = (*list).ob_base.ob_size;
            (*list).ob_base.ob_size = 0;
            (*list).ob_item = ptr::null_mut();
            (*list).allocated = 0;
            release_buffer(items, len);
        }
    }
    0
}

/// The native list's sole capacity/length transaction. Realloc failure leaves
/// the old pointer, length and capacity intact. `-1` is the sort-empty sentinel.
unsafe fn resize(list: *mut PyListObject, new_len: usize) -> bool {
    unsafe {
        let old_len = (*list).ob_base.ob_size as usize;
        let allocated = (*list).allocated;
        if new_len > Py_ssize_t::MAX as usize {
            errors::PyErr_NoMemory();
            return false;
        }
        if allocated >= 0 && allocated as usize >= new_len && new_len >= (allocated as usize >> 1) {
            (*list).ob_base.ob_size = new_len as Py_ssize_t;
            return true;
        }
        let mut capacity = (new_len + (new_len >> 3) + 6) & !3;
        if new_len.saturating_sub(old_len) > capacity - new_len {
            capacity = (new_len + 3) & !3;
        }
        if new_len == 0 {
            capacity = 0;
        }
        if capacity > Py_ssize_t::MAX as usize / size_of::<*mut PyObject>() {
            errors::PyErr_NoMemory();
            return false;
        }
        let items = memory::PyMem_Realloc(
            (*list).ob_item.cast(),
            capacity * size_of::<*mut PyObject>(),
        )
        .cast::<*mut PyObject>();
        if items.is_null() {
            errors::PyErr_NoMemory();
            return false;
        }
        (*list).ob_item = items;
        (*list).ob_base.ob_size = new_len as Py_ssize_t;
        (*list).allocated = capacity as Py_ssize_t;
        true
    }
}

/// Empty exact/estimated extension allocation, with no visible NULL slots.
unsafe fn preallocate(list: *mut PyListObject, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let capacity = (len + 1) & !1;
    if capacity > Py_ssize_t::MAX as usize / size_of::<*mut PyObject>() {
        unsafe { errors::PyErr_NoMemory() };
        return false;
    }
    let items = unsafe { memory::PyMem_Malloc(capacity * size_of::<*mut PyObject>()) }
        .cast::<*mut PyObject>();
    if items.is_null() {
        unsafe { errors::PyErr_NoMemory() };
        return false;
    }
    unsafe {
        (*list).ob_item = items;
        (*list).allocated = capacity as Py_ssize_t;
    }
    true
}

pub(super) unsafe fn reserve_additional(list: *mut PyListObject, additional: usize) -> bool {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        let Some(required) = len.checked_add(additional) else {
            errors::PyErr_NoMemory();
            return false;
        };
        if required > Py_ssize_t::MAX as usize {
            errors::PyErr_NoMemory();
            return false;
        }
        if (*list).ob_item.is_null() {
            return preallocate(list, additional);
        }
        if !resize(list, required) {
            return false;
        }
        (*list).ob_base.ob_size = len as Py_ssize_t;
        true
    }
}

pub(super) unsafe fn finish_extend(list: *mut PyListObject) -> bool {
    unsafe {
        if (*list).ob_base.ob_size < (*list).allocated {
            resize(list, (*list).ob_base.ob_size as usize)
        } else {
            true
        }
    }
}

/// Consumes one reference even when capacity admission fails.
pub(super) unsafe fn append_owned(list: *mut PyListObject, item: *mut PyObject) -> bool {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        if (len as Py_ssize_t) < (*list).allocated {
            *(*list).ob_item.add(len) = item;
            (*list).ob_base.ob_size += 1;
            return true;
        }
        if !resize(list, len + 1) {
            refcount::Py_DECREF(item);
            return false;
        }
        *(*list).ob_item.add(len) = item;
        true
    }
}

pub(super) unsafe fn append(list: *mut PyListObject, item: *mut PyObject) -> bool {
    unsafe {
        refcount::Py_INCREF(item);
        append_owned(list, item)
    }
}

pub(super) unsafe fn insert(
    list: *mut PyListObject,
    index: Py_ssize_t,
    item: *mut PyObject,
) -> bool {
    unsafe {
        let len = (*list).ob_base.ob_size;
        if !resize(list, len as usize + 1) {
            return false;
        }
        let index = if index < 0 {
            (index + len).max(0)
        } else {
            index.min(len)
        } as usize;
        let items = (*list).ob_item;
        ptr::copy(items.add(index), items.add(index + 1), len as usize - index);
        refcount::Py_INCREF(item);
        *items.add(index) = item;
        true
    }
}

pub(super) unsafe fn set_item(
    list: *mut PyListObject,
    index: Py_ssize_t,
    item: *mut PyObject,
) -> std::os::raw::c_int {
    unsafe {
        if index < 0 || index >= (*list).ob_base.ob_size {
            refcount::Py_XDECREF(item);
            errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_IndexError).cast(),
                c"list assignment index out of range".as_ptr(),
            );
            return -1;
        }
        let old = (*list).ob_item.add(index as usize).replace(item);
        refcount::Py_XDECREF(old);
    }
    0
}

/// Replacement pointers are borrowed from the existing iterable materializer.
/// The old slice has a small stack recycle buffer, so short deletion needs no
/// separate allocation. Shrink failure restores the exact original contents.
pub(super) unsafe fn splice(
    list: *mut PyListObject,
    low: Py_ssize_t,
    high: Py_ssize_t,
    replacement: &[*mut PyObject],
) -> std::os::raw::c_int {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        let low = low.clamp(0, len as Py_ssize_t) as usize;
        let high = high.clamp(low as Py_ssize_t, len as Py_ssize_t) as usize;
        let removed = high - low;
        let Some(new_len) = (len - removed).checked_add(replacement.len()) else {
            errors::PyErr_NoMemory();
            return -1;
        };
        if new_len > Py_ssize_t::MAX as usize {
            errors::PyErr_NoMemory();
            return -1;
        }
        if new_len == 0 {
            return clear_native(list);
        }
        let mut on_stack = [ptr::null_mut::<PyObject>(); 8];
        let recycled = if removed <= on_stack.len() {
            on_stack.as_mut_ptr()
        } else {
            let storage =
                memory::PyMem_Malloc(removed * size_of::<*mut PyObject>()).cast::<*mut PyObject>();
            if storage.is_null() {
                errors::PyErr_NoMemory();
                return -1;
            }
            storage
        };
        let items = (*list).ob_item;
        if removed > 0 {
            ptr::copy_nonoverlapping(items.add(low), recycled, removed);
        }
        let mut ok = true;
        if new_len < len {
            ptr::copy(
                items.add(high),
                items.add(low + replacement.len()),
                len - high,
            );
            if !resize(list, new_len) {
                ptr::copy(
                    items.add(low + replacement.len()),
                    items.add(high),
                    len - high,
                );
                ptr::copy_nonoverlapping(recycled, items.add(low), removed);
                ok = false;
            }
        } else if new_len > len {
            if resize(list, new_len) {
                let items = (*list).ob_item;
                ptr::copy(
                    items.add(high),
                    items.add(low + replacement.len()),
                    len - high,
                );
            } else {
                ok = false;
            }
        }
        if ok {
            let items = (*list).ob_item;
            for (offset, &item) in replacement.iter().enumerate() {
                refcount::Py_XINCREF(item);
                *items.add(low + offset) = item;
            }
            for index in (0..removed).rev() {
                refcount::Py_XDECREF(*recycled.add(index));
            }
        }
        if removed > on_stack.len() {
            memory::PyMem_Free(recycled.cast());
        }
        if ok { 0 } else { -1 }
    }
}

pub(super) unsafe fn reverse(list: *mut PyListObject) {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        if len > 1 {
            std::slice::from_raw_parts_mut((*list).ob_item, len).reverse();
        }
    }
}

/// Fixed-length extended assignment. The caller has checked the replacement
/// length and owns its elements. Allocation failure has no selected error yet:
/// CPython retires the replacement sequence before creating MemoryError here.
pub(super) unsafe fn replace_extended(
    list: *mut PyListObject,
    start: Py_ssize_t,
    step: Py_ssize_t,
    replacement: &[*mut PyObject],
) -> bool {
    unsafe {
        if replacement.is_empty() {
            return true;
        }
        let garbage =
            memory::PyMem_Malloc(std::mem::size_of_val(replacement)).cast::<*mut PyObject>();
        if garbage.is_null() {
            return false;
        }
        let items = (*list).ob_item;
        for (offset, &item) in replacement.iter().enumerate() {
            let index = (start + offset as Py_ssize_t * step) as usize;
            *garbage.add(offset) = *items.add(index);
            refcount::Py_INCREF(item);
            *items.add(index) = item;
        }
        for offset in 0..replacement.len() {
            refcount::Py_DECREF(*garbage.add(offset));
        }
        memory::PyMem_Free(garbage.cast());
        true
    }
}

/// Extended deletion retires in ascending storage order, even for a negative
/// slice. The compacted shape is visible before shrinking and item callbacks;
/// a failed shrink retains the already-deleted contents, as CPython does.
pub(super) unsafe fn delete_extended(
    list: *mut PyListObject,
    start: Py_ssize_t,
    step: Py_ssize_t,
    count: usize,
) -> std::os::raw::c_int {
    unsafe {
        if count == 0 {
            return 0;
        }
        let garbage =
            memory::PyMem_Malloc(count * size_of::<*mut PyObject>()).cast::<*mut PyObject>();
        if garbage.is_null() {
            errors::PyErr_NoMemory();
            return -1;
        }
        let (first, stride) = if step < 0 {
            (
                (start + (count - 1) as Py_ssize_t * step) as usize,
                (-step) as usize,
            )
        } else {
            (start as usize, step as usize)
        };
        let len = (*list).ob_base.ob_size as usize;
        let items = (*list).ob_item;
        let mut read = first;
        let mut write = first;
        for offset in 0..count {
            let index = first + offset * stride;
            ptr::copy(items.add(read), items.add(write), index - read);
            write += index - read;
            *garbage.add(offset) = *items.add(index);
            read = index + 1;
        }
        ptr::copy(items.add(read), items.add(write), len - read);
        (*list).ob_base.ob_size = (len - count) as Py_ssize_t;
        let resized = resize(list, len - count);
        for offset in 0..count {
            refcount::Py_DECREF(*garbage.add(offset));
        }
        memory::PyMem_Free(garbage.cast());
        if resized { 0 } else { -1 }
    }
}

pub(super) unsafe fn repeat(list: *mut PyListObject, count: Py_ssize_t) -> bool {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        if len == 0 || count == 1 {
            return true;
        }
        if count < 1 {
            clear_native(list);
            return true;
        }
        let Some(new_len) = len.checked_mul(count as usize) else {
            errors::PyErr_NoMemory();
            return false;
        };
        if !resize(list, new_len) {
            return false;
        }
        let items = (*list).ob_item;
        for index in len..new_len {
            let item = *items.add(index % len);
            refcount::Py_INCREF(item);
            *items.add(index) = item;
        }
        true
    }
}

struct SortStorage {
    list: *mut PyListObject,
    items: *mut *mut PyObject,
    len: Py_ssize_t,
    allocated: Py_ssize_t,
}

impl Drop for SortStorage {
    fn drop(&mut self) {
        unsafe {
            let added = (*self.list).ob_item;
            let added_len = (*self.list).ob_base.ob_size;
            (*self.list).ob_item = self.items;
            (*self.list).ob_base.ob_size = self.len;
            (*self.list).allocated = self.allocated;
            release_buffer(added, added_len);
        }
    }
}

/// A pop transfers the removed list reference. Failed shrinking restores the
/// original physical array before returning the selected allocation failure.
pub(super) unsafe fn pop(list: *mut PyListObject, index: usize) -> *mut PyObject {
    unsafe {
        let len = (*list).ob_base.ob_size as usize;
        let items = (*list).ob_item;
        let value = *items.add(index);
        if len == 1 {
            refcount::Py_INCREF(value);
            clear_native(list);
            return value;
        }
        ptr::copy(items.add(index + 1), items.add(index), len - index - 1);
        if resize(list, len - 1) {
            return value;
        }
        ptr::copy(items.add(index), items.add(index + 1), len - index - 1);
        *items.add(index) = value;
        ptr::null_mut()
    }
}

#[derive(Clone, Copy)]
struct KeyedItem {
    key: *mut PyObject,
    value: *mut PyObject,
}

pub(super) unsafe fn sort(
    list: *mut PyListObject,
    key: *mut PyObject,
    reverse: bool,
) -> std::os::raw::c_int {
    unsafe {
        let saved = SortStorage {
            list,
            items: (*list).ob_item,
            len: (*list).ob_base.ob_size,
            allocated: (*list).allocated,
        };
        (*list).ob_item = ptr::null_mut();
        (*list).ob_base.ob_size = 0;
        (*list).allocated = -1;
        let runs = if (crate::hooks::hooks_or_stubs().target_python_at_least)(3, 13) {
            RunDetection::WeaklyDescending
        } else {
            RunDetection::StrictlyDescending
        };
        let compare = |left: *mut PyObject, right: *mut PyObject| {
            let less = typeobj::PyObject_RichCompareBool(left, right, 0);
            if less < 0 { Err(()) } else { Ok(less != 0) }
        };
        // The no-key C API path sorts the actual detached pointers directly;
        // only a key function needs parallel key/value custody.
        let mut keys: Vec<KeyedItem> = Vec::new();
        let outcome = if key.is_null() {
            if saved.len < 2 {
                Ok(())
            } else {
                let items = std::slice::from_raw_parts_mut(saved.items, saved.len as usize);
                if reverse {
                    items.reverse();
                }
                timsort(items, runs, |left, right| compare(*left, *right))
            }
        } else {
            if keys.try_reserve_exact(saved.len as usize).is_err() {
                errors::PyErr_NoMemory();
                return -1;
            }
            for index in 0..saved.len as usize {
                let value = *saved.items.add(index);
                let result = crate::api::object::PyObject_CallOneArg(key, value);
                if result.is_null() {
                    for item in keys.iter().rev() {
                        refcount::Py_DECREF(item.key);
                    }
                    return -1;
                }
                keys.push(KeyedItem { key: result, value });
            }
            if reverse {
                keys.reverse();
            }
            timsort(&mut keys, runs, |left, right| compare(left.key, right.key))
        };
        if matches!(outcome, Err(TimsortError::Memory)) {
            errors::PyErr_NoMemory();
        }
        // Successful keys retire in their sorted order, before the mutation
        // check and reverse-back. Their deallocators can mutate the empty list.
        for item in &keys {
            refcount::Py_DECREF(item.key);
        }
        let result = if outcome.is_ok() && (*list).allocated != -1 {
            errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                c"list modified during sort".as_ptr(),
            );
            -1
        } else if outcome.is_err() {
            -1
        } else {
            0
        };
        if !key.is_null() {
            if reverse {
                keys.reverse();
            }
            for (index, item) in keys.iter().enumerate() {
                *saved.items.add(index) = item.value;
            }
        } else if reverse && saved.len > 1 {
            std::slice::from_raw_parts_mut(saved.items, saved.len as usize).reverse();
        }
        drop(saved);
        result
    }
}
