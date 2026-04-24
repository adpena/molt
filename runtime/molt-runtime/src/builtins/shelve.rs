//! `shelve` module intrinsics for Molt.
//!
//! Provides a combined dbm+pickle shelf as a single opaque handle.
//! The Python wrapper delegates to these intrinsics for all I/O.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

static NEXT_SHELF_ID: AtomicI64 = AtomicI64::new(1);
fn next_shelf_id() -> i64 {
    NEXT_SHELF_ID.fetch_add(1, Ordering::Relaxed)
}

/// A shelf entry: dbm handle + protocol.
struct ShelfState {
    dbm_handle: i64,
    protocol: i64,
}

thread_local! {
    static SHELF_MAP: RefCell<HashMap<i64, ShelfState>> = RefCell::new(HashMap::new());
}

// ── open(dbm_handle, protocol) -> shelf_handle ──────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_shelve_open(dbm_handle_bits: u64, protocol_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(dbm_handle) = to_i64(obj_from_bits(dbm_handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "dbm_handle must be int");
        };
        let protocol = to_i64(obj_from_bits(protocol_bits)).unwrap_or(4);
        let id = next_shelf_id();
        SHELF_MAP.with(|m| {
            m.borrow_mut().insert(id, ShelfState { dbm_handle, protocol });
        });
        MoltObject::from_int(id).bits()
    })
}

// ── close(shelf_handle) -> None ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_shelve_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid handle");
        };
        SHELF_MAP.with(|m| { m.borrow_mut().remove(&id); });
        MoltObject::none().bits()
    })
}

// ── get_dbm_handle(shelf_handle) -> dbm_handle ─────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_shelve_get_dbm_handle(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid handle");
        };
        SHELF_MAP.with(|m| {
            let map = m.borrow();
            match map.get(&id) {
                Some(state) => MoltObject::from_int(state.dbm_handle).bits(),
                None => raise_exception::<u64>(_py, "ValueError", "shelf is closed"),
            }
        })
    })
}

// ── get_protocol(shelf_handle) -> protocol ──────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_shelve_get_protocol(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid handle");
        };
        SHELF_MAP.with(|m| {
            let map = m.borrow();
            match map.get(&id) {
                Some(state) => MoltObject::from_int(state.protocol).bits(),
                None => raise_exception::<u64>(_py, "ValueError", "shelf is closed"),
            }
        })
    })
}
