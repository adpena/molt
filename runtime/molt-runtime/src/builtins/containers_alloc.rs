use crate::{
    alloc_object, dict_len, dict_table_capacity, dict_update_apply, dict_update_set_in_place,
    exception_pending, is_truthy, maybe_ptr_from_bits, molt_iter, molt_iter_next, obj_from_bits,
    object_type_id, raise_exception, seq_vec_ref, set_table_capacity, usize_from_bits, MoltHeader,
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_FROZENSET, TYPE_ID_LIST, TYPE_ID_SET, TYPE_ID_TUPLE,
};

#[no_mangle]
pub extern "C" fn molt_dict_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<*mut Vec<usize>>();
        let ptr = alloc_object(_py, total, TYPE_ID_DICT);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "dict allocation failed");
        }
        unsafe {
            let capacity_hint = usize_from_bits(capacity_bits);
            let mut order = Vec::new();
            if capacity_hint > 0 {
                let order_cap = capacity_hint
                    .checked_mul(2)
                    .ok_or(())
                    .and_then(|val| order.try_reserve(val).map_err(|_| ()));
                if order_cap.is_err() {
                    return raise_exception::<_>(_py, "MemoryError", "dict allocation failed");
                }
            }
            let mut table = Vec::new();
            if capacity_hint > 0 {
                let table_cap = dict_table_capacity(capacity_hint);
                if table.try_reserve(table_cap).is_err() {
                    return raise_exception::<_>(_py, "MemoryError", "dict allocation failed");
                }
                table.resize(table_cap, 0);
            }
            let order_ptr = Box::into_raw(Box::new(order));
            let table_ptr = Box::into_raw(Box::new(table));
            *(ptr as *mut *mut Vec<u64>) = order_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DictSeqError {
    NotIterable,
    BadLen(usize),
    Exception,
}

pub(crate) fn dict_pair_from_item(
    _py: &PyToken<'_>,
    item_bits: u64,
) -> Result<(u64, u64), DictSeqError> {
    let item_obj = obj_from_bits(item_bits);
    if let Some(item_ptr) = item_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(item_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(item_ptr);
                if elems.len() != 2 {
                    return Err(DictSeqError::BadLen(elems.len()));
                }
                return Ok((elems[0], elems[1]));
            }
        }
    }
    let iter_bits = molt_iter(item_bits);
    if obj_from_bits(iter_bits).is_none() {
        return Err(DictSeqError::NotIterable);
    }
    let mut elems = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending(_py) {
            return Err(DictSeqError::Exception);
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return Err(DictSeqError::Exception);
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(DictSeqError::Exception);
            }
            let pair_elems = seq_vec_ref(pair_ptr);
            if pair_elems.len() < 2 {
                return Err(DictSeqError::Exception);
            }
            let done_bits = pair_elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            elems.push(pair_elems[0]);
        }
    }
    if elems.len() != 2 {
        return Err(DictSeqError::BadLen(elems.len()));
    }
    Ok((elems[0], elems[1]))
}

#[no_mangle]
pub extern "C" fn molt_dict_from_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let mut capacity = 0usize;
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    capacity = dict_len(ptr);
                }
            }
        }
        let dict_bits = molt_dict_new(capacity as u64);
        if obj_from_bits(dict_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some(_dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, obj_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        dict_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_set_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<*mut Vec<usize>>();
        let ptr = alloc_object(_py, total, TYPE_ID_SET);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let capacity_hint = usize_from_bits(capacity_bits);
            let order = Vec::with_capacity(capacity_hint);
            let mut table = Vec::new();
            if capacity_hint > 0 {
                table.resize(set_table_capacity(capacity_hint), 0);
            }
            let order_ptr = Box::into_raw(Box::new(order));
            let table_ptr = Box::into_raw(Box::new(table));
            *(ptr as *mut *mut Vec<u64>) = order_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<*mut Vec<usize>>();
        let ptr = alloc_object(_py, total, TYPE_ID_FROZENSET);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let capacity_hint = usize_from_bits(capacity_bits);
            let order = Vec::with_capacity(capacity_hint);
            let mut table = Vec::new();
            if capacity_hint > 0 {
                table.resize(set_table_capacity(capacity_hint), 0);
            }
            let order_ptr = Box::into_raw(Box::new(order));
            let table_ptr = Box::into_raw(Box::new(table));
            *(ptr as *mut *mut Vec<u64>) = order_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
