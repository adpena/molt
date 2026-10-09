//! Sequence capabilities for the shared fixture owner, with real edge custody.
use super::*;
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::errors;

unsafe fn type_error(message: &std::ffi::CStr) {
    unsafe {
        errors::PyErr_SetString(
            (&raw mut abi_types::PyExc_TypeError).cast(),
            message.as_ptr(),
        )
    };
}

unsafe extern "C" fn alloc_list() -> u64 {
    unsafe { alloc_list_presized(0) }
}
unsafe extern "C" fn alloc_list_presized(length: usize) -> u64 {
    // The ABI builder independently marks these logical None slots as unreadied.
    allocate(Value::List(vec![MoltObject::none().bits(); length]))
}
unsafe extern "C" fn list_len(bits: u64) -> usize {
    match VALUES.lock().unwrap().get(&bits).map(|entry| &entry.value) {
        Some(Value::List(items)) => items.len(),
        _ => 0,
    }
}
unsafe extern "C" fn list_item(bits: u64, index: usize) -> BorrowedHandleResult {
    let values = VALUES.lock().unwrap();
    let item = match values.get(&bits).map(|entry| &entry.value) {
        Some(Value::List(items)) => items.get(index).copied(),
        _ => None,
    };
    item.map(BorrowedHandleResult::ok)
        .unwrap_or_else(BorrowedHandleResult::missing)
}
unsafe extern "C" fn list_set(bits: u64, index: usize, value: u64) -> OwnedHandleResult {
    unsafe { inc_ref(value) };
    let old = {
        let mut values = VALUES.lock().unwrap();
        match values.get_mut(&bits).map(|entry| &mut entry.value) {
            Some(Value::List(items)) => items
                .get_mut(index)
                .map(|item| std::mem::replace(item, value)),
            _ => None,
        }
    };
    match old {
        Some(old) => OwnedHandleResult::ok(old),
        None => {
            unsafe { dec_ref(value) };
            OwnedHandleResult::error()
        }
    }
}
unsafe extern "C" fn list_append(bits: u64, value: u64, pointer: *mut PyObject) -> i32 {
    unsafe { list_insert(bits, isize::MAX, value, pointer) }
}

unsafe extern "C" fn list_insert(
    bits: u64,
    index: isize,
    value: u64,
    pointer: *mut PyObject,
) -> i32 {
    let length = {
        let values = VALUES.lock().unwrap();
        match values.get(&bits).map(|entry| &entry.value) {
            Some(Value::List(items)) => items.len(),
            _ => return -1,
        }
    };
    let index = if index < 0 {
        length.saturating_sub(index.unsigned_abs())
    } else {
        (index as usize).min(length)
    };
    let Some(prepared) = GLOBAL_BRIDGE.prepare_list_insert_from_pyobj(bits, value, pointer) else {
        return -1;
    };
    unsafe { inc_ref(value) };
    {
        let mut values = VALUES.lock().unwrap();
        let Value::List(items) = &mut values.get_mut(&bits).unwrap().value else {
            unreachable!()
        };
        items.insert(index, value);
    }
    assert!(
        unsafe { prepared.publish_insert(index) },
        "fixture list publication"
    );
    0
}

unsafe extern "C" fn alloc_tuple(length: usize) -> u64 {
    // Missing is a construction state, distinct from the Python None value.
    allocate(Value::Tuple(vec![None; length]))
}
unsafe extern "C" fn tuple_len(bits: u64) -> usize {
    match VALUES.lock().unwrap().get(&bits).map(|entry| &entry.value) {
        Some(Value::Tuple(items)) => items.len(),
        _ => 0,
    }
}
unsafe extern "C" fn tuple_item(bits: u64, index: usize) -> BorrowedHandleResult {
    let values = VALUES.lock().unwrap();
    let item = match values.get(&bits).map(|entry| &entry.value) {
        Some(Value::Tuple(items)) => items.get(index).copied().flatten(),
        _ => None,
    };
    item.map(BorrowedHandleResult::ok)
        .unwrap_or_else(BorrowedHandleResult::missing)
}
unsafe extern "C" fn tuple_set(
    bits: u64,
    index: usize,
    value: u64,
    pointer: *mut PyObject,
) -> OwnedHandleResult {
    let incoming = (!pointer.is_null()).then_some(value);
    if let Some(value) = incoming {
        unsafe { inc_ref(value) };
    }
    let old = {
        let mut values = VALUES.lock().unwrap();
        match values.get_mut(&bits).map(|entry| &mut entry.value) {
            Some(Value::Tuple(items)) => items
                .get_mut(index)
                .map(|item| std::mem::replace(item, incoming)),
            _ => None,
        }
    };
    match old {
        Some(Some(old)) => OwnedHandleResult::ok(old),
        Some(None) => OwnedHandleResult::missing(),
        None => {
            if let Some(value) = incoming {
                unsafe { dec_ref(value) };
            }
            OwnedHandleResult::error()
        }
    }
}

unsafe extern "C" fn object_supports_subscript(bits: u64) -> i32 {
    if let Some(Value::Opaque { supports_subscript }) =
        VALUES.lock().unwrap().get(&bits).map(|entry| &entry.value)
    {
        return i32::from(*supports_subscript);
    }
    // This profile has only the builtin classes above. Storage-owning sibling
    // fixtures may supply their own classifier without splitting class identity.
    let tag = unsafe { molt_cpython_abi::hooks::hooks_or_stubs().classify_heap(bits) };
    i32::from(
        [
            MoltTypeTag::List,
            MoltTypeTag::Tuple,
            MoltTypeTag::Str,
            MoltTypeTag::Dict,
        ]
        .into_iter()
        .any(|candidate| tag == candidate as u8),
    )
}
fn length(values: &HashMap<u64, Entry>, bits: u64) -> Option<usize> {
    match &values.get(&bits)?.value {
        Value::List(items) => Some(items.len()),
        Value::Tuple(items) => Some(items.len()),
        Value::Dict(items) => Some(items.iter().flatten().count()),
        Value::String(bytes) => Some(
            std::str::from_utf8(&bytes[..bytes.len() - 1])
                .expect("fixture UTF-8 string")
                .chars()
                .count(),
        ),
        _ => None,
    }
}
unsafe extern "C" fn object_length(bits: u64) -> isize {
    let length = length(&VALUES.lock().unwrap(), bits);
    match length.and_then(|len| isize::try_from(len).ok()) {
        Some(len) => len,
        None => {
            unsafe { type_error(c"fixture object has no len()") };
            -1
        }
    }
}
unsafe extern "C" fn object_length_hint(bits: u64, default: isize) -> isize {
    let values = VALUES.lock().unwrap();
    let size = match values.get(&bits).map(|entry| &entry.value) {
        Some(Value::Iterator { source, index }) => {
            length(&values, *source).map(|len| len.saturating_sub(*index))
        }
        _ => length(&values, bits),
    };
    size.and_then(|size| isize::try_from(size).ok())
        .unwrap_or(default)
}
unsafe extern "C" fn object_get_iter(bits: u64) -> OwnedHandleResult {
    let kind = {
        let values = VALUES.lock().unwrap();
        match values.get(&bits).map(|entry| &entry.value) {
            Some(Value::Iterator { .. }) => 2,
            Some(Value::List(_) | Value::Tuple(_) | Value::String(_) | Value::Dict(_)) => 1,
            _ => 0,
        }
    };
    if kind == 0 {
        unsafe { type_error(c"fixture object is not iterable") };
        return OwnedHandleResult::error();
    }
    unsafe { inc_ref(bits) };
    if kind == 2 {
        OwnedHandleResult::ok(bits)
    } else {
        OwnedHandleResult::ok(allocate(Value::Iterator {
            source: bits,
            index: 0,
        }))
    }
}
unsafe extern "C" fn iter_check(bits: u64) -> i32 {
    i32::from(matches!(
        VALUES.lock().unwrap().get(&bits).map(|entry| &entry.value),
        Some(Value::Iterator { .. })
    ))
}
unsafe extern "C" fn iter_next(bits: u64, exhausted: *mut i32) -> OwnedHandleResult {
    enum Step {
        Item(u64),
        Text(char),
        Done,
        Invalid,
    }
    let step = {
        let mut values = VALUES.lock().unwrap();
        let Some(Value::Iterator { source, index }) = values.get(&bits).map(|entry| &entry.value)
        else {
            drop(values);
            unsafe { type_error(c"fixture object is not an iterator") };
            return OwnedHandleResult::error();
        };
        let (source, index) = (*source, *index);
        let mut next_index = index.saturating_add(1);
        let step = match values.get(&source).map(|entry| &entry.value) {
            Some(Value::List(items)) => items
                .get(index)
                .copied()
                .map(Step::Item)
                .unwrap_or(Step::Done),
            Some(Value::Tuple(items)) => match items.get(index) {
                Some(Some(item)) => Step::Item(*item),
                Some(None) => Step::Invalid,
                None => Step::Done,
            },
            Some(Value::Dict(items)) => {
                match items
                    .iter()
                    .enumerate()
                    .skip(index)
                    .find_map(|(position, row)| row.as_ref().map(|pair| (position, pair)))
                {
                    Some((position, &(key, _))) => {
                        next_index = position + 1;
                        Step::Item(key)
                    }
                    None => Step::Done,
                }
            }
            Some(Value::String(bytes)) => std::str::from_utf8(&bytes[..bytes.len() - 1])
                .expect("fixture UTF-8 string")
                .chars()
                .nth(index)
                .map(Step::Text)
                .unwrap_or(Step::Done),
            _ => Step::Invalid,
        };
        if matches!(step, Step::Item(_) | Step::Text(_)) {
            let Value::Iterator { index, .. } = &mut values.get_mut(&bits).unwrap().value else {
                unreachable!()
            };
            *index = next_index;
        }
        step
    };
    unsafe { *exhausted = i32::from(matches!(step, Step::Done)) };
    match step {
        Step::Item(item) => {
            unsafe { inc_ref(item) };
            OwnedHandleResult::ok(item)
        }
        Step::Text(character) => {
            let mut buffer = [0; 4];
            let text = character.encode_utf8(&mut buffer);
            OwnedHandleResult::ok(unsafe { alloc_str(text.as_ptr(), text.len()) })
        }
        Step::Done => OwnedHandleResult::ok(MoltObject::none().bits()),
        Step::Invalid => {
            unsafe { errors::PyErr_BadInternalCall() };
            OwnedHandleResult::error()
        }
    }
}

pub(super) fn wire(hooks: &mut RuntimeHooks) {
    hooks.alloc_list = alloc_list;
    hooks.alloc_list_presized = alloc_list_presized;
    hooks.list_len = list_len;
    hooks.list_item = list_item;
    hooks.list_set = list_set;
    hooks.list_append = list_append;
    hooks.list_insert = list_insert;
    hooks.alloc_tuple = Some(alloc_tuple);
    hooks.tuple_len = Some(tuple_len);
    hooks.tuple_item = Some(tuple_item);
    hooks.tuple_set = Some(tuple_set);
    hooks.object_supports_subscript = object_supports_subscript;
    hooks.object_length = object_length;
    hooks.object_length_hint = object_length_hint;
    hooks.object_get_iter = object_get_iter;
    hooks.iter_check = iter_check;
    hooks.iter_next = iter_next;
}
