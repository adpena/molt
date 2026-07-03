use std::collections::HashMap;
use std::sync::Mutex;

use crate::PyToken;
use crate::async_rt::cancellation::current_token_id;
use crate::{
    MoltObject, alloc_list, alloc_string, bits_from_ptr, call_callable0, dec_ref_bits,
    exception_pending, inc_ref_bits, is_missing_bits, is_truthy, missing_bits,
    molt_getattr_builtin, molt_set_add, molt_set_new, obj_from_bits, raise_exception,
    resolve_task_ptr, runtime_state,
};

use super::super::current_task_ptr;
use super::token::asyncio_parse_token_id;

fn asyncio_task_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_tasks
}

fn asyncio_task_registry_set_impl(_py: &PyToken<'_>, token_bits: u64, task_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    if obj_from_bits(task_bits).is_none() {
        if let Some(old_bits) = guard.remove(&token_id)
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
        return MoltObject::none().bits();
    }
    let old_bits = guard.insert(token_id, task_bits);
    if old_bits != Some(task_bits) {
        inc_ref_bits(_py, task_bits);
        if let Some(old_bits) = old_bits
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
    }
    MoltObject::none().bits()
}

fn asyncio_task_registry_get_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let guard = asyncio_task_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&token_id).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_task_registry_contains_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let guard = asyncio_task_map(_py).lock().unwrap();
    MoltObject::from_bool(guard.contains_key(&token_id)).bits()
}

fn asyncio_task_registry_current_impl(_py: &PyToken<'_>) -> u64 {
    let token_id = current_token_id();
    {
        let guard = asyncio_task_map(_py).lock().unwrap();
        if let Some(bits) = guard.get(&token_id).copied() {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
    }
    let task_ptr = current_task_ptr();
    if task_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let task_bits = MoltObject::from_ptr(task_ptr).bits();
    inc_ref_bits(_py, task_bits);
    task_bits
}

fn asyncio_task_registry_current_for_loop_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let task_bits = asyncio_task_registry_current_impl(_py);
    if obj_from_bits(task_bits).is_none() || obj_from_bits(loop_bits).is_none() {
        return task_bits;
    }
    let loop_name_ptr = alloc_string(_py, b"_loop");
    if loop_name_ptr.is_null() {
        dec_ref_bits(_py, task_bits);
        return MoltObject::none().bits();
    }
    let loop_name_bits = MoltObject::from_ptr(loop_name_ptr).bits();
    let missing = missing_bits(_py);
    let loop_attr_bits = molt_getattr_builtin(task_bits, loop_name_bits, missing);
    dec_ref_bits(_py, loop_name_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, task_bits);
        return MoltObject::none().bits();
    }
    let matches = !is_missing_bits(_py, loop_attr_bits) && loop_attr_bits == loop_bits;
    if !obj_from_bits(loop_attr_bits).is_none() {
        dec_ref_bits(_py, loop_attr_bits);
    }
    if matches {
        task_bits
    } else {
        dec_ref_bits(_py, task_bits);
        MoltObject::none().bits()
    }
}

fn asyncio_task_registry_pop_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    guard
        .remove(&token_id)
        .unwrap_or_else(|| MoltObject::none().bits())
}

fn asyncio_task_last_exception_clear_impl(_py: &PyToken<'_>, task_bits: u64) -> u64 {
    let Some(task_ptr) = resolve_task_ptr(task_bits) else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    if matches!(
        std::env::var("MOLT_TRACE_TASK_LAST_EXCEPTION_CLEAR")
            .ok()
            .as_deref(),
        Some("1")
    ) {
        eprintln!(
            "molt asyncio task_last_exception_clear task=0x{:x}",
            task_ptr as usize
        );
    }
    crate::task_last_exception_drop(_py, task_ptr);
    MoltObject::none().bits()
}

fn asyncio_task_registry_move_impl(
    _py: &PyToken<'_>,
    old_token_bits: u64,
    new_token_bits: u64,
) -> u64 {
    let old_token = match asyncio_parse_token_id(_py, old_token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let new_token = match asyncio_parse_token_id(_py, new_token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    if old_token == new_token {
        return MoltObject::from_bool(false).bits();
    }
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    let Some(old_bits) = guard.remove(&old_token) else {
        return MoltObject::from_bool(false).bits();
    };
    if let Some(replaced_bits) = guard.insert(new_token, old_bits)
        && replaced_bits != 0
        && !obj_from_bits(replaced_bits).is_none()
    {
        dec_ref_bits(_py, replaced_bits);
    }
    MoltObject::from_bool(true).bits()
}

fn asyncio_task_registry_values_impl(_py: &PyToken<'_>) -> u64 {
    let guard = asyncio_task_map(_py).lock().unwrap();
    let values = guard.values().copied().collect::<Vec<_>>();
    drop(guard);
    let ptr = alloc_list(_py, values.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    bits_from_ptr(ptr)
}

fn asyncio_task_registry_live_values_impl(
    _py: &PyToken<'_>,
    loop_bits: u64,
) -> Result<Vec<u64>, u64> {
    let target_loop = if obj_from_bits(loop_bits).is_none() {
        None
    } else {
        Some(loop_bits)
    };
    let values: Vec<u64> = {
        let guard = asyncio_task_map(_py).lock().unwrap();
        guard.values().copied().collect()
    };
    let done_name_ptr = alloc_string(_py, b"done");
    if done_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let done_name_bits = MoltObject::from_ptr(done_name_ptr).bits();
    let loop_name_ptr = alloc_string(_py, b"_loop");
    if loop_name_ptr.is_null() {
        dec_ref_bits(_py, done_name_bits);
        return Err(MoltObject::none().bits());
    }
    let loop_name_bits = MoltObject::from_ptr(loop_name_ptr).bits();
    let missing = missing_bits(_py);
    let mut out_bits: Vec<u64> = Vec::new();

    for task_bits in values {
        if task_bits == 0 || obj_from_bits(task_bits).is_none() {
            continue;
        }
        if let Some(loop_filter) = target_loop {
            let loop_attr_bits = molt_getattr_builtin(task_bits, loop_name_bits, missing);
            if exception_pending(_py) {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, done_name_bits);
                dec_ref_bits(_py, loop_name_bits);
                return Err(MoltObject::none().bits());
            }
            let matches = !is_missing_bits(_py, loop_attr_bits) && loop_attr_bits == loop_filter;
            if !obj_from_bits(loop_attr_bits).is_none() {
                dec_ref_bits(_py, loop_attr_bits);
            }
            if !matches {
                continue;
            }
        }
        let done_method_bits = molt_getattr_builtin(task_bits, done_name_bits, missing);
        if exception_pending(_py) {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, done_name_bits);
            dec_ref_bits(_py, loop_name_bits);
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, done_method_bits) {
            if !obj_from_bits(done_method_bits).is_none() {
                dec_ref_bits(_py, done_method_bits);
            }
            continue;
        }
        let done_bits = unsafe { call_callable0(_py, done_method_bits) };
        dec_ref_bits(_py, done_method_bits);
        if exception_pending(_py) {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, done_name_bits);
            dec_ref_bits(_py, loop_name_bits);
            return Err(MoltObject::none().bits());
        }
        let is_done = is_truthy(_py, obj_from_bits(done_bits));
        if !obj_from_bits(done_bits).is_none() {
            dec_ref_bits(_py, done_bits);
        }
        if !is_done {
            inc_ref_bits(_py, task_bits);
            out_bits.push(task_bits);
        }
    }

    dec_ref_bits(_py, done_name_bits);
    dec_ref_bits(_py, loop_name_bits);
    Ok(out_bits)
}

fn asyncio_task_registry_live_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let out_bits = match asyncio_task_registry_live_values_impl(_py, loop_bits) {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let list_ptr = alloc_list(_py, out_bits.as_slice());
    for bits in out_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        bits_from_ptr(list_ptr)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_set(token_bits: u64, task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_task_registry_set_impl(_py, token_bits, task_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_get(token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_task_registry_get_impl(_py, token_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_contains(token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_task_registry_contains_impl(_py, token_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_current() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_task_registry_current_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_current_for_loop(loop_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_task_registry_current_for_loop_impl(_py, loop_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_pop(token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_task_registry_pop_impl(_py, token_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_last_exception_clear(task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_task_last_exception_clear_impl(_py, task_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_move(old_token_bits: u64, new_token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_task_registry_move_impl(_py, old_token_bits, new_token_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_values() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_task_registry_values_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_live(loop_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_task_registry_live_impl(_py, loop_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_task_registry_live_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tasks = match asyncio_task_registry_live_values_impl(_py, loop_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let set_bits = molt_set_new(tasks.len() as u64);
        if obj_from_bits(set_bits).is_none() {
            for task_bits in tasks {
                dec_ref_bits(_py, task_bits);
            }
            return MoltObject::none().bits();
        }
        for &task_bits in &tasks {
            let _ = molt_set_add(set_bits, task_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                for live_task_bits in tasks {
                    dec_ref_bits(_py, live_task_bits);
                }
                return MoltObject::none().bits();
            }
        }
        for task_bits in tasks {
            dec_ref_bits(_py, task_bits);
        }
        set_bits
    })
}

// --- _asyncio C-extension surface: _enter_task / _leave_task / _register_task / _unregister_task ---

fn asyncio_current_tasks_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_current_tasks
}

/// CPython `_asyncio._enter_task(loop, task)`:
/// Sets `task` as the current task for `loop`. Raises RuntimeError if a task is already set.
fn asyncio_enter_task_impl(_py: &PyToken<'_>, loop_bits: u64, task_bits: u64) -> u64 {
    if obj_from_bits(loop_bits).is_none() || obj_from_bits(task_bits).is_none() {
        return raise_exception::<u64>(
            _py,
            "TypeError",
            "_enter_task requires non-None loop and task",
        );
    }
    let mut guard = asyncio_current_tasks_map(_py).lock().unwrap();
    if let Some(existing_bits) = guard.get(&loop_bits).copied()
        && existing_bits != 0
        && !obj_from_bits(existing_bits).is_none()
    {
        drop(guard);
        return raise_exception::<u64>(
            _py,
            "RuntimeError",
            "Cannot enter into task while another task is being executed",
        );
    }
    guard.insert(loop_bits, task_bits);
    inc_ref_bits(_py, loop_bits);
    inc_ref_bits(_py, task_bits);
    MoltObject::none().bits()
}

/// CPython `_asyncio._leave_task(loop, task)`:
/// Clears the current task for `loop`. Raises RuntimeError if the current task is not `task`.
fn asyncio_leave_task_impl(_py: &PyToken<'_>, loop_bits: u64, task_bits: u64) -> u64 {
    if obj_from_bits(loop_bits).is_none() || obj_from_bits(task_bits).is_none() {
        return raise_exception::<u64>(
            _py,
            "TypeError",
            "_leave_task requires non-None loop and task",
        );
    }
    let mut guard = asyncio_current_tasks_map(_py).lock().unwrap();
    let current = guard.get(&loop_bits).copied();
    match current {
        Some(current_bits) if current_bits == task_bits => {
            guard.remove(&loop_bits);
            drop(guard);
            dec_ref_bits(_py, loop_bits);
            dec_ref_bits(_py, task_bits);
            MoltObject::none().bits()
        }
        _ => {
            drop(guard);
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "Leaving a task that is not the current task",
            )
        }
    }
}

/// CPython `_asyncio._register_task(task)`:
/// Adds task to the global task registry. Uses the task's id() as the key.
fn asyncio_register_task_impl(_py: &PyToken<'_>, task_bits: u64) -> u64 {
    if obj_from_bits(task_bits).is_none() {
        return MoltObject::none().bits();
    }
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    // Use the raw bits as the key (acts as id)
    let old = guard.insert(task_bits, task_bits);
    if old != Some(task_bits) {
        inc_ref_bits(_py, task_bits);
        if let Some(old_bits) = old
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
    }
    MoltObject::none().bits()
}

/// CPython `_asyncio._unregister_task(task)`:
/// Removes task from the global task registry.
fn asyncio_unregister_task_impl(_py: &PyToken<'_>, task_bits: u64) -> u64 {
    if obj_from_bits(task_bits).is_none() {
        return MoltObject::none().bits();
    }
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    if let Some(old_bits) = guard.remove(&task_bits) {
        drop(guard);
        if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
            dec_ref_bits(_py, old_bits);
        }
    }
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_enter_task(loop_bits: u64, task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_enter_task_impl(_py, loop_bits, task_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_leave_task(loop_bits: u64, task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_leave_task_impl(_py, loop_bits, task_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_register_task(task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_register_task_impl(_py, task_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_unregister_task(task_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_unregister_task_impl(_py, task_bits) })
}
