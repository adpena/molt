use crate::PyToken;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::{
    alloc_exception_from_class_bits, alloc_tuple, dec_ref_bits, exception_type_bits_from_name,
    header_from_obj_ptr, obj_from_bits, raise_exception, record_exception, runtime_state,
    seq_vec_ref, string_obj_to_owned, task_exception_baseline_drop, task_exception_depth_drop,
    task_exception_handler_stack_drop, task_exception_stack_drop, task_last_exception_drop,
    type_name, ExceptionSentinel, MoltHeader, MoltObject, PtrSlot, HEADER_FLAG_BLOCK_ON,
    HEADER_FLAG_CANCEL_PENDING, HEADER_FLAG_SPAWN_RETAIN, TYPE_ID_TUPLE,
};

use super::scheduler::{await_waiter_clear, wake_task_ptr};
use super::spawned_task_dec;

pub(crate) struct CancelTokenEntry {
    pub(crate) parent: u64,
    pub(crate) cancelled: bool,
    pub(crate) refs: u64,
}

fn trace_cancel_msg() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_CANCEL_MSG").as_deref() == Ok("1"))
}

fn trace_cancel_token() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_CANCEL_TOKEN").as_deref() == Ok("1"))
}

pub(crate) fn default_cancel_tokens() -> HashMap<u64, CancelTokenEntry> {
    let mut map = HashMap::new();
    map.insert(
        1,
        CancelTokenEntry {
            parent: 0,
            cancelled: false,
            refs: 1,
        },
    );
    map
}

pub(crate) static NEXT_CANCEL_TOKEN_ID: AtomicU64 = AtomicU64::new(2);

thread_local! {
    pub(crate) static CURRENT_TOKEN: Cell<u64> = const { Cell::new(1) };
}

pub(crate) fn cancel_tokens(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, CancelTokenEntry>> {
    &runtime_state(_py).cancel_tokens
}

pub(crate) fn task_tokens(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, u64>> {
    &runtime_state(_py).task_tokens
}

pub(crate) fn task_tokens_by_id(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<u64, HashSet<PtrSlot>>> {
    &runtime_state(_py).task_tokens_by_id
}

pub(crate) fn task_cancel_messages(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, u64>> {
    &runtime_state(_py).task_cancel_messages
}

pub(crate) fn task_has_token(_py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
    let map = task_tokens(_py).lock().unwrap();
    map.contains_key(&PtrSlot(task_ptr))
}

pub(crate) fn task_cancel_message_args(_py: &PyToken<'_>, task_ptr: *mut u8) -> Option<u64> {
    if task_ptr.is_null() {
        return None;
    }
    let map = task_cancel_messages(_py).lock().unwrap();
    map.get(&PtrSlot(task_ptr)).copied()
}

pub(crate) fn task_cancel_message_set(_py: &PyToken<'_>, task_ptr: *mut u8, msg_bits: u64) {
    crate::gil_assert();
    if task_ptr.is_null() {
        return;
    }
    let msg_obj = obj_from_bits(msg_bits);
    if trace_cancel_msg() {
        let msg_desc =
            string_obj_to_owned(msg_obj).unwrap_or_else(|| type_name(_py, msg_obj).to_string());
        eprintln!(
            "molt cancel msg set task=0x{:x} msg={}",
            task_ptr as usize, msg_desc
        );
    }
    let args_ptr = if msg_obj.is_none() {
        alloc_tuple(_py, &[])
    } else {
        alloc_tuple(_py, &[msg_bits])
    };
    if args_ptr.is_null() {
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let mut map = task_cancel_messages(_py).lock().unwrap();
    if let Some(old_bits) = map.insert(PtrSlot(task_ptr), args_bits) {
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn task_cancel_message_clear(_py: &PyToken<'_>, task_ptr: *mut u8) {
    crate::gil_assert();
    if task_ptr.is_null() {
        return;
    }
    let mut map = task_cancel_messages(_py).lock().unwrap();
    if let Some(old_bits) = map.remove(&PtrSlot(task_ptr)) {
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn token_id_from_bits(bits: u64) -> Option<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Some(0);
    }
    obj.as_int()
        .and_then(|val| if val >= 0 { Some(val as u64) } else { None })
}

pub(crate) fn current_token_id() -> u64 {
    CURRENT_TOKEN.with(|cell| cell.get())
}

pub(crate) fn set_current_token(_py: &PyToken<'_>, id: u64) -> u64 {
    retain_token(_py, id);
    let prev = CURRENT_TOKEN.with(|cell| {
        let prev = cell.get();
        cell.set(id);
        prev
    });
    if trace_cancel_token() {
        eprintln!("molt cancel token set_current prev={} new={}", prev, id);
    }
    release_token(_py, prev);
    prev
}

pub(crate) fn retain_token(_py: &PyToken<'_>, id: u64) {
    if id == 0 || id == 1 {
        return;
    }
    let mut map = cancel_tokens(_py).lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.refs = entry.refs.saturating_add(1);
    }
}

pub(crate) fn release_token(_py: &PyToken<'_>, id: u64) {
    if id == 0 || id == 1 {
        return;
    }
    let mut map = cancel_tokens(_py).lock().unwrap();
    if let Some(entry) = map.get_mut(&id) {
        entry.refs = entry.refs.saturating_sub(1);
        if entry.refs == 0 {
            map.remove(&id);
        }
    }
}

pub(crate) fn register_task_token(_py: &PyToken<'_>, task_ptr: *mut u8, token: u64) {
    let task_slot = PtrSlot(task_ptr);
    let mut map = task_tokens(_py).lock().unwrap();
    let mut index = task_tokens_by_id(_py).lock().unwrap();
    if let Some(old) = map.insert(task_slot, token) {
        if let Some(tasks) = index.get_mut(&old) {
            tasks.remove(&task_slot);
            if tasks.is_empty() {
                index.remove(&old);
            }
        }
        release_token(_py, old);
    }
    if token != 0 {
        index.entry(token).or_default().insert(task_slot);
    }
    if trace_cancel_token() {
        eprintln!(
            "molt cancel token register task=0x{:x} token={}",
            task_ptr as usize, token
        );
    }
    retain_token(_py, token);
}

pub(crate) fn ensure_task_token(_py: &PyToken<'_>, task_ptr: *mut u8, fallback: u64) -> u64 {
    let task_slot = PtrSlot(task_ptr);
    let mut map = task_tokens(_py).lock().unwrap();
    if let Some(token) = map.get(&task_slot).copied() {
        return token;
    }
    map.insert(task_slot, fallback);
    retain_token(_py, fallback);
    if fallback != 0 {
        let mut index = task_tokens_by_id(_py).lock().unwrap();
        index.entry(fallback).or_default().insert(task_slot);
    }
    fallback
}

pub(crate) fn clear_task_token(_py: &PyToken<'_>, task_ptr: *mut u8) {
    crate::gil_assert();
    let task_slot = PtrSlot(task_ptr);
    let mut map = task_tokens(_py).lock().unwrap();
    let token = map.remove(&task_slot);
    drop(map);
    if let Some(token) = token {
        let mut index = task_tokens_by_id(_py).lock().unwrap();
        if let Some(tasks) = index.get_mut(&token) {
            tasks.remove(&task_slot);
            if tasks.is_empty() {
                index.remove(&token);
            }
        }
        release_token(_py, token);
    }
    if !task_ptr.is_null() {
        unsafe {
            let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
            if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0 {
                (*header).flags &= !HEADER_FLAG_SPAWN_RETAIN;
                dec_ref_bits(_py, MoltObject::from_ptr(task_ptr).bits());
                spawned_task_dec();
            }
        }
    }
    task_last_exception_drop(_py, task_ptr);
    task_exception_handler_stack_drop(_py, task_ptr);
    task_exception_stack_drop(_py, task_ptr);
    task_exception_depth_drop(_py, task_ptr);
    task_exception_baseline_drop(_py, task_ptr);
    await_waiter_clear(_py, task_ptr);
}

pub(crate) fn task_cancel_pending(task_ptr: *mut u8) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        ((*header).flags & HEADER_FLAG_CANCEL_PENDING) != 0
    }
}

pub(crate) fn task_set_cancel_pending(task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).flags |= HEADER_FLAG_CANCEL_PENDING;
    }
}

pub(crate) fn task_take_cancel_pending(task_ptr: *mut u8) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        let pending = ((*header).flags & HEADER_FLAG_CANCEL_PENDING) != 0;
        if pending {
            (*header).flags &= !HEADER_FLAG_CANCEL_PENDING;
        }
        pending
    }
}

pub(crate) fn raise_cancelled_with_message<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    task_ptr: *mut u8,
) -> T {
    if let Some(args_bits) = task_cancel_message_args(_py, task_ptr) {
        if trace_cancel_msg() {
            let msg_desc = obj_from_bits(args_bits)
                .as_ptr()
                .filter(|ptr| unsafe { crate::object_type_id(*ptr) == TYPE_ID_TUPLE })
                .and_then(|ptr| {
                    let elems = unsafe { seq_vec_ref(ptr) };
                    elems.first().map(|bits| obj_from_bits(*bits))
                })
                .map(|obj| {
                    string_obj_to_owned(obj).unwrap_or_else(|| type_name(_py, obj).to_string())
                })
                .unwrap_or_else(|| "<none>".to_string());
            eprintln!(
                "molt cancel raise task=0x{:x} msg={}",
                task_ptr as usize, msg_desc
            );
        }
        let class_bits = exception_type_bits_from_name(_py, "CancelledError");
        let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
        if !ptr.is_null() {
            record_exception(_py, ptr);
            return T::exception_sentinel();
        }
    }
    raise_exception::<T>(_py, "CancelledError", "")
}

pub(crate) fn wake_tasks_for_cancelled_tokens(_py: &PyToken<'_>) {
    let mut wake_list: Vec<PtrSlot> = Vec::new();
    {
        let map = task_tokens_by_id(_py).lock().unwrap();
        for (token_id, tasks) in map.iter() {
            if token_is_cancelled(_py, *token_id) {
                wake_list.extend(tasks.iter().copied());
            }
        }
    }
    if wake_list.is_empty() {
        return;
    }
    for task_ptr in wake_list {
        let should_wake = unsafe {
            let header = header_from_obj_ptr(task_ptr.0);
            ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0
                || ((*header).flags & HEADER_FLAG_BLOCK_ON) != 0
        };
        if should_wake {
            wake_task_ptr(_py, task_ptr.0);
        }
    }
}

pub(crate) fn token_is_cancelled(_py: &PyToken<'_>, id: u64) -> bool {
    if id == 0 {
        return false;
    }
    let map = cancel_tokens(_py).lock().unwrap();
    let mut current = id;
    let mut depth = 0;
    while current != 0 && depth < 64 {
        let Some(entry) = map.get(&current) else {
            return false;
        };
        if entry.cancelled {
            return true;
        }
        current = entry.parent;
        depth += 1;
    }
    false
}

// --- Cancel token FFI ---

/// # Safety
/// `parent_bits` must be either `None` or an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_new(parent_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        cancel_tokens(_py);
        let parent_id = {
            let parent_obj = obj_from_bits(parent_bits);
            if parent_obj.is_none() {
                current_token_id()
            } else if let Some(val) = parent_obj.as_int() {
                if val == -1 {
                    0
                } else if val >= 0 {
                    val as u64
                } else {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cancel token parent must be >= 0 or -1",
                    );
                }
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cancel token parent must be int or None",
                );
            }
        };
        let id = NEXT_CANCEL_TOKEN_ID.fetch_add(1, AtomicOrdering::Relaxed);
        if trace_cancel_token() {
            eprintln!("molt cancel token new id={} parent={}", id, parent_id);
        }
        let mut map = cancel_tokens(_py).lock().unwrap();
        map.insert(
            id,
            CancelTokenEntry {
                parent: parent_id,
                cancelled: false,
                refs: 1,
            },
        );
        MoltObject::from_int(id as i64).bits()
    })
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_clone(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        retain_token(_py, id);
        MoltObject::none().bits()
    })
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_drop(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        release_token(_py, id);
        MoltObject::none().bits()
    })
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_cancel(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        let mut map = cancel_tokens(_py).lock().unwrap();
        if let Some(entry) = map.get_mut(&id) {
            entry.cancelled = true;
        }
        drop(map);
        wake_tasks_for_cancelled_tokens(_py);
        MoltObject::none().bits()
    })
}

/// # Safety
/// `token_bits` must be an integer token id.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_is_cancelled(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        MoltObject::from_bool(token_is_cancelled(_py, id)).bits()
    })
}

/// # Safety
/// `token_bits` must be an integer token id or `None`.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_set_current(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match token_id_from_bits(token_bits) {
            Some(0) => 1,
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        let prev = set_current_token(_py, id);
        MoltObject::from_int(prev as i64).bits()
    })
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_token_get_current() -> u64 {
    crate::with_gil_entry!(_py, {
        cancel_tokens(_py);
        MoltObject::from_int(current_token_id() as i64).bits()
    })
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancelled() -> u64 {
    crate::with_gil_entry!(_py, {
        cancel_tokens(_py);
        MoltObject::from_bool(token_is_cancelled(_py, current_token_id())).bits()
    })
}

/// # Safety
/// Requires the cancel token tables to be initialized by the runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_cancel_current() -> u64 {
    crate::with_gil_entry!(_py, {
        cancel_tokens(_py);
        let id = current_token_id();
        let mut map = cancel_tokens(_py).lock().unwrap();
        if let Some(entry) = map.get_mut(&id) {
            entry.cancelled = true;
        }
        drop(map);
        wake_tasks_for_cancelled_tokens(_py);
        MoltObject::none().bits()
    })
}
