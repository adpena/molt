use std::collections::HashMap;
use std::sync::Mutex;

use crate::PyToken;
use crate::{
    MoltObject, dec_ref_bits, inc_ref_bits, obj_from_bits, raise_exception, runtime_state,
};

fn asyncio_running_loop_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_running_loops
}

fn asyncio_event_loop_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_event_loops
}

fn asyncio_running_loop_get_impl(_py: &PyToken<'_>) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let guard = asyncio_running_loop_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&tid).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_running_loop_set_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let mut guard = asyncio_running_loop_map(_py).lock().unwrap();
    if obj_from_bits(loop_bits).is_none() {
        if let Some(old_bits) = guard.remove(&tid)
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
        return MoltObject::none().bits();
    }

    let old_bits = guard.insert(tid, loop_bits);
    if old_bits != Some(loop_bits) {
        inc_ref_bits(_py, loop_bits);
        if let Some(old_bits) = old_bits
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
    }
    MoltObject::none().bits()
}

fn asyncio_event_loop_get_impl(_py: &PyToken<'_>) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let guard = asyncio_event_loop_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&tid).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_event_loop_get_current_impl(_py: &PyToken<'_>) -> u64 {
    let bits = asyncio_event_loop_get_impl(_py);
    if !obj_from_bits(bits).is_none() {
        return bits;
    }
    raise_exception(
        _py,
        "RuntimeError",
        "There is no current event loop in thread 'MainThread'.",
    )
}

fn asyncio_event_loop_set_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let mut guard = asyncio_event_loop_map(_py).lock().unwrap();
    if obj_from_bits(loop_bits).is_none() {
        if let Some(old_bits) = guard.remove(&tid)
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
        return MoltObject::none().bits();
    }

    let old_bits = guard.insert(tid, loop_bits);
    if old_bits != Some(loop_bits) {
        inc_ref_bits(_py, loop_bits);
        if let Some(old_bits) = old_bits
            && old_bits != 0
            && !obj_from_bits(old_bits).is_none()
        {
            dec_ref_bits(_py, old_bits);
        }
    }
    MoltObject::none().bits()
}

fn asyncio_event_loop_policy_get_impl(_py: &PyToken<'_>) -> u64 {
    let bits = *runtime_state(_py).asyncio_event_loop_policy.lock().unwrap();
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_event_loop_policy_set_impl(_py: &PyToken<'_>, policy_bits: u64) -> u64 {
    let mut guard = runtime_state(_py).asyncio_event_loop_policy.lock().unwrap();
    let old_bits = *guard;
    *guard = policy_bits;
    if policy_bits != 0 && !obj_from_bits(policy_bits).is_none() {
        inc_ref_bits(_py, policy_bits);
    }
    if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
        dec_ref_bits(_py, old_bits);
    }
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_running_loop_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_running_loop_get_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_running_loop_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_running_loop_set_impl(_py, loop_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_loop_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_event_loop_get_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_loop_get_current() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_event_loop_get_current_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_loop_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_event_loop_set_impl(_py, loop_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_loop_policy_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { asyncio_event_loop_policy_get_impl(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_loop_policy_set(policy_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_event_loop_policy_set_impl(_py, policy_bits)
    })
}
