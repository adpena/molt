use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::PyToken;
use crate::{
    MoltObject, alloc_list, bits_from_ptr, dec_ref_bits, inc_ref_bits, obj_from_bits, runtime_state,
};

use super::token::asyncio_parse_token_id;

fn asyncio_event_waiters_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, Vec<u64>>> {
    &runtime_state(_py).asyncio_event_waiters
}

#[derive(Default)]
pub(crate) struct AsyncioEventWaiterIndex {
    positions: HashMap<u64, VecDeque<usize>>,
    live: usize,
    slots_len: usize,
}

fn asyncio_event_waiter_index_map(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<u64, AsyncioEventWaiterIndex>> {
    &runtime_state(_py).asyncio_event_waiter_index
}

fn rebuild_asyncio_event_waiter_index(waiters: &[u64]) -> AsyncioEventWaiterIndex {
    let mut index = AsyncioEventWaiterIndex {
        positions: HashMap::new(),
        live: 0,
        slots_len: waiters.len(),
    };
    for (slot, waiter_bits) in waiters.iter().copied().enumerate() {
        if waiter_bits == 0 {
            continue;
        }
        index
            .positions
            .entry(waiter_bits)
            .or_default()
            .push_back(slot);
        index.live += 1;
    }
    index
}

fn asyncio_event_waiter_pop_slot(
    index: &mut AsyncioEventWaiterIndex,
    waiter_bits: u64,
) -> Option<usize> {
    let drop_key;
    let slot = {
        let positions = index.positions.get_mut(&waiter_bits)?;
        let slot = positions.pop_front()?;
        drop_key = positions.is_empty();
        slot
    };
    if drop_key {
        index.positions.remove(&waiter_bits);
    }
    Some(slot)
}

fn maybe_compact_asyncio_event_waiters(
    waiters: &mut Vec<u64>,
    index: &mut AsyncioEventWaiterIndex,
) {
    let tombstones = waiters.len().saturating_sub(index.live);
    if waiters.len() < 64 || tombstones <= index.live {
        return;
    }
    let mut compacted = Vec::with_capacity(index.live);
    for waiter_bits in waiters.drain(..) {
        if waiter_bits != 0 {
            compacted.push(waiter_bits);
        }
    }
    *waiters = compacted;
    *index = rebuild_asyncio_event_waiter_index(waiters.as_slice());
}

fn asyncio_event_waiters_register_impl(
    _py: &PyToken<'_>,
    token_bits: u64,
    waiter_bits: u64,
) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    if obj_from_bits(waiter_bits).is_none() {
        return MoltObject::none().bits();
    }
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let mut index_guard = asyncio_event_waiter_index_map(_py).lock().unwrap();
    let waiters = guard.entry(token_id).or_default();
    let waiter_index = index_guard
        .entry(token_id)
        .or_insert_with(|| rebuild_asyncio_event_waiter_index(waiters.as_slice()));
    if waiter_index.slots_len != waiters.len() {
        *waiter_index = rebuild_asyncio_event_waiter_index(waiters.as_slice());
    }
    let slot = waiters.len();
    waiters.push(waiter_bits);
    waiter_index
        .positions
        .entry(waiter_bits)
        .or_default()
        .push_back(slot);
    waiter_index.live += 1;
    waiter_index.slots_len = waiters.len();
    inc_ref_bits(_py, waiter_bits);
    MoltObject::none().bits()
}

fn asyncio_event_waiters_unregister_impl(
    _py: &PyToken<'_>,
    token_bits: u64,
    waiter_bits: u64,
) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let mut index_guard = asyncio_event_waiter_index_map(_py).lock().unwrap();
    let Some(waiters) = guard.get_mut(&token_id) else {
        return MoltObject::from_bool(false).bits();
    };
    let waiter_index = index_guard
        .entry(token_id)
        .or_insert_with(|| rebuild_asyncio_event_waiter_index(waiters.as_slice()));
    if waiter_index.slots_len != waiters.len() {
        *waiter_index = rebuild_asyncio_event_waiter_index(waiters.as_slice());
    }
    let Some(mut slot) = asyncio_event_waiter_pop_slot(waiter_index, waiter_bits) else {
        return MoltObject::from_bool(false).bits();
    };
    if waiters.get(slot).copied() != Some(waiter_bits) {
        *waiter_index = rebuild_asyncio_event_waiter_index(waiters.as_slice());
        slot = match asyncio_event_waiter_pop_slot(waiter_index, waiter_bits) {
            Some(slot) => slot,
            None => return MoltObject::from_bool(false).bits(),
        };
        if waiters.get(slot).copied() != Some(waiter_bits) {
            return MoltObject::from_bool(false).bits();
        }
    }
    waiters[slot] = 0;
    waiter_index.live = waiter_index.live.saturating_sub(1);
    let mut drop_token = false;
    if waiter_index.live == 0 {
        drop_token = true;
    } else {
        maybe_compact_asyncio_event_waiters(waiters, waiter_index);
    }
    waiter_index.slots_len = waiters.len();
    if drop_token {
        guard.remove(&token_id);
        index_guard.remove(&token_id);
    }
    if waiter_bits != 0 && !obj_from_bits(waiter_bits).is_none() {
        dec_ref_bits(_py, waiter_bits);
    }
    MoltObject::from_bool(true).bits()
}

fn asyncio_event_waiters_cleanup_token_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let mut index_guard = asyncio_event_waiter_index_map(_py).lock().unwrap();
    let Some(raw_waiters) = guard.remove(&token_id) else {
        return MoltObject::from_int(0).bits();
    };
    index_guard.remove(&token_id);
    let waiters: Vec<u64> = raw_waiters.into_iter().filter(|bits| *bits != 0).collect();
    if waiters.is_empty() {
        return MoltObject::from_int(0).bits();
    }
    drop(index_guard);
    drop(guard);
    let list_ptr = alloc_list(_py, waiters.as_slice());
    if list_ptr.is_null() {
        for bits in waiters {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        return MoltObject::none().bits();
    }
    let list_bits = bits_from_ptr(list_ptr);
    let out_bits = unsafe { crate::molt_asyncio_event_waiters_cleanup(list_bits) };
    dec_ref_bits(_py, list_bits);
    for bits in waiters {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    out_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_waiters_register(token_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_event_waiters_register_impl(_py, token_bits, waiter_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_waiters_unregister(token_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_event_waiters_unregister_impl(_py, token_bits, waiter_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_event_waiters_cleanup_token(token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        asyncio_event_waiters_cleanup_token_impl(_py, token_bits)
    })
}
