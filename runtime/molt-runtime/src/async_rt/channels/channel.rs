use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded, unbounded};

use crate::{
    GilReleaseGuard, MoltObject, PyToken, dec_ref_bits, inc_ref_bits, obj_from_bits,
    opaque_handle_bits, pending_bits_i64, ptr_from_bits, raise_exception, release_ptr, to_i64,
};

pub struct MoltChannel {
    pub sender: Sender<i64>,
    pub receiver: Receiver<i64>,
}

type ChanHandle = u64;

#[inline]
fn chan_handle_from_ptr(ptr: *mut u8) -> ChanHandle {
    opaque_handle_bits(ptr)
}

#[inline]
unsafe fn chan_ptr_from_handle(handle: ChanHandle) -> *mut u8 {
    ptr_from_bits(handle)
}

#[inline]
unsafe fn chan_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

fn chan_try_send_impl(_py: &PyToken<'_>, chan: &MoltChannel, val: i64) -> i64 {
    let ok_bits = MoltObject::from_int(0).bits() as i64;
    let bits = val as u64;
    inc_ref_bits(_py, bits);
    match chan.sender.try_send(val) {
        Ok(_) => ok_bits,
        Err(TrySendError::Full(_)) => {
            dec_ref_bits(_py, bits);
            pending_bits_i64()
        }
        Err(TrySendError::Disconnected(_)) => {
            dec_ref_bits(_py, bits);
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

fn chan_try_recv_impl(_py: &PyToken<'_>, chan: &MoltChannel) -> i64 {
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(TryRecvError::Empty) => pending_bits_i64(),
        Err(TryRecvError::Disconnected) => {
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

fn chan_send_blocking_impl(_py: &PyToken<'_>, chan: &MoltChannel, val: i64) -> i64 {
    let ok_bits = MoltObject::from_int(0).bits() as i64;
    let bits = val as u64;
    inc_ref_bits(_py, bits);
    match chan.sender.try_send(val) {
        Ok(_) => ok_bits,
        Err(TrySendError::Full(_)) => {
            let _release = GilReleaseGuard::new();
            match chan.sender.send(val) {
                Ok(_) => ok_bits,
                Err(_) => {
                    dec_ref_bits(_py, bits);
                    raise_exception::<i64>(_py, "RuntimeError", "channel send failed")
                }
            }
        }
        Err(TrySendError::Disconnected(_)) => {
            dec_ref_bits(_py, bits);
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

#[cfg(molt_has_net_io)]
fn chan_recv_blocking_impl(_py: &PyToken<'_>, chan: &MoltChannel) -> i64 {
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(TryRecvError::Empty) => {
            let _release = GilReleaseGuard::new();
            match chan.receiver.recv() {
                Ok(val) => val,
                Err(_) => raise_exception::<i64>(_py, "RuntimeError", "channel recv failed"),
            }
        }
        Err(TryRecvError::Disconnected) => {
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> ChanHandle {
    crate::with_gil_entry_nopanic!(_py, {
        let capacity = match to_i64(obj_from_bits(capacity_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "channel capacity must be an integer",
                );
            }
        };
        if capacity < 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "channel capacity must be non-negative",
            );
        }
        let capacity = capacity as usize;
        let (s, r) = if capacity == 0 {
            unbounded()
        } else {
            bounded(capacity)
        };
        let chan = Box::new(MoltChannel {
            sender: s,
            receiver: r,
        });
        chan_handle_from_ptr(Box::into_raw(chan) as *mut u8)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_handle: ChanHandle) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` came from `molt_chan_new`.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        if chan_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // SAFETY: `chan_ptr` is non-null and uniquely owned on drop.
        let chan = unsafe { Box::from_raw(chan_ptr as *mut MoltChannel) };
        while let Ok(val) = chan.receiver.try_recv() {
            dec_ref_bits(_py, val as u64);
        }
        // SAFETY: ownership is transferred back to the runtime pointer registry.
        unsafe { chan_release_ptr(chan_ptr) };
        drop(chan);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_try_send_impl(_py, chan, val)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_try_send(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_try_send_impl(_py, chan, val)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send_blocking(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_send_blocking_impl(_py, chan, val)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_try_recv_impl(_py, chan)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_try_recv(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_try_recv_impl(_py, chan)
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv_blocking(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_try_recv_impl(_py, chan)
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv_blocking(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY: caller guarantees `chan_handle` is valid for this call.
        let chan_ptr = unsafe { chan_ptr_from_handle(chan_handle) };
        // SAFETY: `chan_ptr` is expected to reference a live `MoltChannel`.
        let chan = unsafe { &*(chan_ptr as *mut MoltChannel) };
        chan_recv_blocking_impl(_py, chan)
    })
}
