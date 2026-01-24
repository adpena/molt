use crate::*;

pub(crate) fn alloc_dict_with_pairs(pairs: &[u64]) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, TYPE_ID_DICT);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(pairs.len());
        let table = Vec::new();
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for pair in pairs.chunks(2) {
            if pair.len() == 2 {
                dict_set_in_place(ptr, pair[0], pair[1]);
            }
        }
    }
    ptr
}

pub(crate) fn alloc_set_like_with_entries(entries: &[u64], type_id: u32) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<*mut Vec<usize>>();
    let ptr = alloc_object(total, type_id);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        let order = Vec::with_capacity(entries.len());
        let mut table = Vec::new();
        if !entries.is_empty() {
            table.resize(set_table_capacity(entries.len()), 0);
        }
        let order_ptr = Box::into_raw(Box::new(order));
        let table_ptr = Box::into_raw(Box::new(table));
        *(ptr as *mut *mut Vec<u64>) = order_ptr;
        *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        for &entry in entries {
            set_add_in_place(ptr, entry);
        }
    }
    ptr
}

pub(crate) fn alloc_set_with_entries(entries: &[u64]) -> *mut u8 {
    alloc_set_like_with_entries(entries, TYPE_ID_SET)
}

#[no_mangle]
pub extern "C" fn molt_list_builder_new(capacity_bits: u64) -> u64 {
    // Allocate wrapper object
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>(); // Store pointer to Vec
    let ptr = alloc_object(total, TYPE_ID_LIST_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Box::new(Vec::<u64>::with_capacity(capacity_hint));
        let vec_ptr = Box::into_raw(vec);
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

pub(crate) struct PtrDropGuard {
    ptr: *mut u8,
    active: bool,
}

impl PtrDropGuard {
    pub(crate) fn new(ptr: *mut u8) -> Self {
        Self {
            ptr,
            active: !ptr.is_null(),
        }
    }

    pub(crate) fn release(&mut self) {
        self.active = false;
    }
}

impl Drop for PtrDropGuard {
    fn drop(&mut self) {
        if self.active && !self.ptr.is_null() {
            unsafe {
                molt_dec_ref(self.ptr);
            }
        }
    }
}

pub unsafe extern "C" fn molt_list_builder_append(builder_bits: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub unsafe extern "C" fn molt_list_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    // Reconstruct Box to drop it later, but we need the data
    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let list_ptr = alloc_list_with_capacity(slice, capacity);

    // Builder object will be cleaned up by GC/Ref counting eventually,
    // but the Vec heap allocation is owned by the Box we just reconstructed.
    // So dropping 'vec' here frees the temporary buffer. Correct.

    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a tuple builder.
pub unsafe extern "C" fn molt_tuple_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();

    let vec = Box::from_raw(vec_ptr);
    let slice = vec.as_slice();
    let capacity = vec.capacity().max(MAX_SMALL_LIST);
    let tuple_ptr = alloc_tuple_with_capacity(slice, capacity);

    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_dict_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_DICT_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint * 2);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_append(builder_bits: u64, key: u64, val: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
    vec.push(val);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a dict builder.
pub unsafe extern "C" fn molt_dict_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_dict_with_pairs(vec.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_set_builder_new(capacity_bits: u64) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
    let ptr = alloc_object(total, TYPE_ID_SET_BUILDER);
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let capacity_hint = usize_from_bits(capacity_bits);
        let vec = Vec::with_capacity(capacity_hint);
        let vec_ptr = Box::into_raw(Box::new(vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
    }
    bits_from_ptr(ptr)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_append(builder_bits: u64, key: u64) {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return;
    }
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return;
    }
    let vec = &mut *vec_ptr;
    vec.push(key);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a set builder.
pub unsafe extern "C" fn molt_set_builder_finish(builder_bits: u64) -> u64 {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let _guard = PtrDropGuard::new(builder_ptr);
    let vec_ptr = *(builder_ptr as *mut *mut Vec<u64>);
    if vec_ptr.is_null() {
        return MoltObject::none().bits();
    }
    *(builder_ptr as *mut *mut Vec<u64>) = std::ptr::null_mut();
    let vec = Box::from_raw(vec_ptr);
    let ptr = alloc_set_with_entries(vec.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}
