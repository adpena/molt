#[cfg(molt_has_net_io)]
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use super::super::cancel_tokens;
use super::super::sockets::{SendData, send_data_from_bits};
#[cfg(any(target_arch = "wasm32", molt_has_net_io))]
use super::super::{current_token_id, token_id_from_bits};
use super::has_capability;
use crate::audit::{AuditArgs, audit_capability_decision};
use crate::{PyToken, raise_exception, usize_from_bits};
#[cfg(target_arch = "wasm32")]
use crate::{molt_db_exec_host, molt_db_query_host};

#[cfg(molt_has_net_io)]
type DbHostHook = extern "C" fn(*const u8, usize, *mut u64, u64) -> i32;

#[cfg(molt_has_net_io)]
static DB_QUERY_HOOK: AtomicUsize = AtomicUsize::new(0);
#[cfg(molt_has_net_io)]
static DB_EXEC_HOOK: AtomicUsize = AtomicUsize::new(0);

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_db_set_query_hook(ptr: usize) {
    crate::with_gil_entry_nopanic!(_py, {
        DB_QUERY_HOOK.store(ptr, AtomicOrdering::Release);
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_db_set_exec_hook(ptr: usize) {
    crate::with_gil_entry_nopanic!(_py, {
        DB_EXEC_HOOK.store(ptr, AtomicOrdering::Release);
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_query(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        db_query_impl(_py, req_ptr, len_bits, out, token_bits)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_exec(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        db_exec_impl(_py, req_ptr, len_bits, out, token_bits)
    })
}

fn db_query_impl(
    _py: &PyToken<'_>,
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let Some(len) = usize_from_bits(len_bits) else {
        return 1;
    };
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    let db_read_allowed = has_capability(_py, "db.read");
    audit_capability_decision("db.query", "db.read", AuditArgs::None, db_read_allowed);
    if !db_read_allowed {
        return 6;
    }
    cancel_tokens(_py);
    #[cfg(any(target_arch = "wasm32", molt_has_net_io))]
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(not(any(target_arch = "wasm32", molt_has_net_io)))]
    let _ = token_bits;
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { molt_db_query_host(req_ptr as u64, len_bits, out as u64, token_id) }
    }
    #[cfg(molt_has_net_io)]
    {
        let hook_ptr = DB_QUERY_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        // SAFETY: hook_ptr was stored into DB_QUERY_HOOK by the host embedder's
        // registration function, which accepts only `DbHostHook`-typed values.
        // The AtomicUsize load preserves the original function pointer bit pattern.
        // The host must keep the function valid for the process lifetime.
        // A stale or mistyped pointer causes UB on the subsequent call.
        let hook: DbHostHook = unsafe { std::mem::transmute(hook_ptr) };
        hook(req_ptr, len, out, token_id)
    }
    #[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
    {
        7
    }
}

fn db_exec_impl(
    _py: &PyToken<'_>,
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let Some(len) = usize_from_bits(len_bits) else {
        return 1;
    };
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    let db_write_allowed = has_capability(_py, "db.write");
    audit_capability_decision("db.exec", "db.write", AuditArgs::None, db_write_allowed);
    if !db_write_allowed {
        return 6;
    }
    cancel_tokens(_py);
    #[cfg(any(target_arch = "wasm32", molt_has_net_io))]
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(not(any(target_arch = "wasm32", molt_has_net_io)))]
    let _ = token_bits;
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { molt_db_exec_host(req_ptr as u64, len_bits, out as u64, token_id) }
    }
    #[cfg(molt_has_net_io)]
    {
        let hook_ptr = DB_EXEC_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        // SAFETY: hook_ptr was stored into DB_EXEC_HOOK by the host embedder's
        // registration function, which accepts only `DbHostHook`-typed values.
        // The AtomicUsize load preserves the original function pointer bit pattern.
        // The host must keep the function valid for the process lifetime.
        // A stale or mistyped pointer causes UB on the subsequent call.
        let hook: DbHostHook = unsafe { std::mem::transmute(hook_ptr) };
        hook(req_ptr, len, out, token_id)
    }
    #[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
    {
        7
    }
}

fn db_error(_py: &PyToken<'_>, op: &str, code: i32, cap: &str) -> u64 {
    match code {
        1 => raise_exception::<_>(_py, "ValueError", &format!("{op} invalid input")),
        2 => raise_exception::<_>(_py, "RuntimeError", &format!("{op} output pointer invalid")),
        6 => raise_exception::<_>(_py, "PermissionError", &format!("missing {cap} capability")),
        7 => raise_exception::<_>(_py, "RuntimeError", &format!("{op} host unavailable")),
        _ => raise_exception::<_>(_py, "RuntimeError", &format!("{op} failed")),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_db_query_obj(req_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let send_data = match send_data_from_bits(req_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        let mut out = 0u64;
        let rc = db_query_impl(_py, data_ptr, data_len as u64, &mut out, token_bits);
        if rc != 0 {
            return db_error(_py, "db_query", rc, "db.read");
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_db_exec_obj(req_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let send_data = match send_data_from_bits(req_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        let mut out = 0u64;
        let rc = db_exec_impl(_py, data_ptr, data_len as u64, &mut out, token_bits);
        if rc != 0 {
            return db_error(_py, "db_exec", rc, "db.write");
        }
        out
    })
}
