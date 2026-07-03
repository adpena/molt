use crate::PyToken;
use crate::{
    MoltObject, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_tuple, bits_from_ptr,
    dec_ref_bits, dict_clear_in_place, dict_del_in_place, dict_get_in_place, dict_set_in_place,
    inc_ref_bits, obj_from_bits, object_type_id, raise_exception, seq_vec_ref, to_i64,
};

fn asyncio_child_watcher_dict_ptr(_py: &PyToken<'_>, callbacks_bits: u64) -> Result<*mut u8, u64> {
    let Some(ptr) = obj_from_bits(callbacks_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "child watcher callbacks must be dict",
        ));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "child watcher callbacks must be dict",
        ));
    }
    Ok(ptr)
}

fn asyncio_child_watcher_pid(_py: &PyToken<'_>, pid_bits: u64) -> Result<i64, u64> {
    let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
        return Err(raise_exception::<u64>(_py, "TypeError", "pid must be int"));
    };
    Ok(pid)
}

fn asyncio_child_watcher_args_tuple_bits(_py: &PyToken<'_>, args_bits: u64) -> Result<u64, u64> {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "args must be tuple or list",
        ));
    };
    let type_id = unsafe { object_type_id(args_ptr) };
    if type_id == TYPE_ID_TUPLE {
        inc_ref_bits(_py, args_bits);
        return Ok(args_bits);
    }
    if type_id == TYPE_ID_LIST {
        let elems = unsafe { seq_vec_ref(args_ptr) };
        let tuple_ptr = alloc_tuple(_py, elems.as_slice());
        if tuple_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        return Ok(bits_from_ptr(tuple_ptr));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "args must be tuple or list",
    ))
}

fn asyncio_child_watcher_add_impl(
    _py: &PyToken<'_>,
    callbacks_bits: u64,
    pid_bits: u64,
    callback_bits: u64,
    args_bits: u64,
) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let args_tuple_bits = match asyncio_child_watcher_args_tuple_bits(_py, args_bits) {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let entry_ptr = alloc_tuple(_py, &[callback_bits, args_tuple_bits]);
    if entry_ptr.is_null() {
        dec_ref_bits(_py, args_tuple_bits);
        return MoltObject::none().bits();
    }
    let entry_bits = bits_from_ptr(entry_ptr);
    unsafe {
        dict_set_in_place(_py, callbacks_ptr, pid_key_bits, entry_bits);
    }
    dec_ref_bits(_py, pid_key_bits);
    dec_ref_bits(_py, entry_bits);
    dec_ref_bits(_py, args_tuple_bits);
    MoltObject::none().bits()
}

fn asyncio_child_watcher_remove_impl(_py: &PyToken<'_>, callbacks_bits: u64, pid_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let removed = unsafe { dict_del_in_place(_py, callbacks_ptr, pid_key_bits) };
    dec_ref_bits(_py, pid_key_bits);
    MoltObject::from_bool(removed).bits()
}

fn asyncio_child_watcher_clear_impl(_py: &PyToken<'_>, callbacks_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    unsafe {
        dict_clear_in_place(_py, callbacks_ptr);
    }
    MoltObject::none().bits()
}

fn asyncio_child_watcher_pop_impl(_py: &PyToken<'_>, callbacks_bits: u64, pid_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, callbacks_ptr, pid_key_bits) };
    let out_bits = if let Some(bits) = entry_bits {
        inc_ref_bits(_py, bits);
        unsafe {
            dict_del_in_place(_py, callbacks_ptr, pid_key_bits);
        }
        bits
    } else {
        MoltObject::none().bits()
    };
    dec_ref_bits(_py, pid_key_bits);
    out_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_child_watcher_add(
    callbacks_bits: u64,
    pid_bits: u64,
    callback_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_child_watcher_add_impl(_py, callbacks_bits, pid_bits, callback_bits, args_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_child_watcher_remove(callbacks_bits: u64, pid_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_child_watcher_remove_impl(_py, callbacks_bits, pid_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_child_watcher_clear(callbacks_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_child_watcher_clear_impl(_py, callbacks_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_child_watcher_pop(callbacks_bits: u64, pid_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_child_watcher_pop_impl(_py, callbacks_bits, pid_bits)
    })
}
