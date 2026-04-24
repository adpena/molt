#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/signal_ext.rs ===
//! Extended signal intrinsics for the Molt `signal` stdlib module.
//!
//! Complements the existing `molt_signal_raise` in `object/ops.rs` with full
//! handler registration, signal constants, and utility functions.
//!
//! Signal handlers are stored in a fixed-size static array indexed by signal
//! number (max NSIG, typically 32 on macOS/Linux).  Python-level handlers are
//! represented as opaque u64 bits; SIG_DFL=0, SIG_IGN=1.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

// ── Constants ─────────────────────────────────────────────────────────────

pub const SIG_DFL_INT: i64 = 0;
pub const SIG_IGN_INT: i64 = 1;

/// Sentinel bits stored in the handler table for SIG_DFL and SIG_IGN.
/// We reserve low integer bits (which are valid NaN-box int representations)
/// and treat them as special magic values.
const HANDLER_SIG_DFL: u64 = 0;
const HANDLER_SIG_IGN: u64 = 1;

/// Maximum signal slot count reserved by the runtime table.
///
/// We keep this slightly above common platform NSIG ranges so we can index
/// fixed-size atomics without heap allocations, while still validating user
/// signal numbers against platform NSIG at API boundaries.
const MAX_SIGNAL: usize = 128;

// ── Handler table ─────────────────────────────────────────────────────────

/// Stores the current Python-level handler bits for each signal number.
/// Value 0 = SIG_DFL, 1 = SIG_IGN, anything else = MoltObject bits of a callable.
static HANDLER_TABLE: [AtomicU64; MAX_SIGNAL] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU64 = AtomicU64::new(HANDLER_SIG_DFL);
    [ZERO; MAX_SIGNAL]
};

/// Set wakeup fd (-1 = disabled).
static WAKEUP_FD: AtomicI32 = AtomicI32::new(-1);

/// Lock protecting sigaction calls on Unix.
static SIGACTION_LOCK: Mutex<()> = Mutex::new(());

// ── Raw C signal handler ──────────────────────────────────────────────────
//
// When a signal arrives with a Python callable handler, we write the signal
// number into the wakeup_fd (if set) and record a pending delivery flag.
// Actual Python callables are called by the Molt scheduler at a safe point.
// This matches CPython's signal handling architecture.

#[cfg(all(unix, not(target_arch = "wasm32")))]
extern "C" fn molt_c_signal_handler(signum: libc::c_int) {
    // Write signal number byte to wakeup fd if configured.
    let fd = WAKEUP_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = signum as u8;
        unsafe {
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
    // Note: We do not call into Rust/GIL here — that would be unsafe from
    // a signal handler. The scheduler polls PENDING_SIGNALS at safe points.
    if (signum as usize) < MAX_SIGNAL {
        PENDING_SIGNALS[signum as usize].store(1, Ordering::SeqCst);
    }
}

// Pending signal flags — the scheduler checks these.
static PENDING_SIGNALS: [AtomicU64; MAX_SIGNAL] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MAX_SIGNAL]
};

// ── Internal helpers ──────────────────────────────────────────────────────

#[inline]
fn effective_nsig() -> i64 {
    #[cfg(target_os = "macos")]
    {
        32_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(target_os = "ios")]
    {
        32_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(all(
        unix,
        not(target_arch = "wasm32"),
        not(any(target_os = "macos", target_os = "ios"))
    ))]
    {
        65_i64.min(MAX_SIGNAL as i64)
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        MAX_SIGNAL as i64
    }
}

fn sig_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i32, u64> {
    let obj = obj_from_bits(bits);
    let nsig = effective_nsig();
    match to_i64(obj) {
        Some(v) if v > 0 && v < nsig => Ok(v as i32),
        Some(v) if v <= 0 => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "signal number must be positive",
        )),
        Some(_) => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            &format!("signal number out of range (max {})", nsig - 1),
        )),
        None => Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "signal number must be int",
        )),
    }
}

/// Convert a handler bits value to the int sentinel visible from Python.
fn handler_bits_to_py(_py: &PyToken<'_>, signum: i32, bits: u64) -> u64 {
    if bits == HANDLER_SIG_DFL {
        let _ = signum;
        return int_bits_from_i64(_py, SIG_DFL_INT);
    }
    if bits == HANDLER_SIG_IGN {
        return int_bits_from_i64(_py, SIG_IGN_INT);
    }
    bits // callable object bits pass through
}

// ── Public intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_signal(signum_bits: u64, handler_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("signal.signal", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for signal operations",
            );
        }
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Determine handler kind.
        let handler_obj = obj_from_bits(handler_bits);
        let new_handler_bits = if let Some(int_val) = to_i64(handler_obj) {
            if int_val == SIG_DFL_INT {
                HANDLER_SIG_DFL
            } else if int_val == SIG_IGN_INT {
                HANDLER_SIG_IGN
            } else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "signal handler must be SIG_DFL, SIG_IGN, or callable",
                );
            }
        } else if handler_obj.is_none() {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "signal handler must be SIG_DFL, SIG_IGN, or callable",
            );
        } else {
            // Assume callable — store bits directly.
            handler_bits
        };

        // Fetch-and-replace old handler.
        let old_bits = HANDLER_TABLE[signum as usize].swap(new_handler_bits, Ordering::SeqCst);

        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let _guard = SIGACTION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                match new_handler_bits {
                    HANDLER_SIG_DFL => {
                        libc::signal(signum, libc::SIG_DFL);
                    }
                    HANDLER_SIG_IGN => {
                        libc::signal(signum, libc::SIG_IGN);
                    }
                    _ => {
                        libc::signal(
                            signum,
                            molt_c_signal_handler as *const () as libc::sighandler_t,
                        );
                    }
                }
            }
        }

        handler_bits_to_py(_py, signum, old_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_getsignal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let bits = HANDLER_TABLE[signum as usize].load(Ordering::SeqCst);
        handler_bits_to_py(_py, signum, bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_raise_signal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("signal.raise", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for signal operations",
            );
        }
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let rc = unsafe { libc::raise(signum) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            // On WASM / Windows: synthetic KeyboardInterrupt for SIGINT only.
            if signum == 2 {
                return raise_exception::<u64>(_py, "KeyboardInterrupt", "signal interrupt");
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_alarm(seconds_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("signal.alarm", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for signal operations",
            );
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let secs = to_i64(obj_from_bits(seconds_bits)).unwrap_or(0).max(0) as u32;
            let prev = unsafe { libc::alarm(secs) };
            int_bits_from_i64(_py, prev as i64)
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = seconds_bits;
            raise_exception::<u64>(
                _py,
                "OSError",
                "signal.alarm not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pause() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("signal.pause", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for signal operations",
            );
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            unsafe { libc::pause() };
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            raise_exception::<u64>(
                _py,
                "OSError",
                "signal.pause not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_set_wakeup_fd(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("signal.set_wakeup_fd", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for signal operations",
            );
        }
        let new_fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(-1) as i32;
        let old_fd = WAKEUP_FD.swap(new_fd, Ordering::SeqCst);
        int_bits_from_i64(_py, old_fd as i64)
    })
}

// ── Signal number constants ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigabrt() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGABRT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigfpe() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGFPE as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigill() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGILL as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigint() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGINT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigsegv() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSEGV as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigterm() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTERM as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sighup() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGHUP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 1_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigquit() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGQUIT as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 3_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr1() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGUSR1 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 10_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr2() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGUSR2 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 12_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigchld() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGCHLD as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 17_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigalrm() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGALRM as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 14_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigpipe() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGPIPE as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 13_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_dfl() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, SIG_DFL_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_ign() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, SIG_IGN_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_nsig() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, effective_nsig()) })
}

// ── Extended signal number constants ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_block() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_BLOCK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 0_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_unblock() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_UNBLOCK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 1_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_setmask() -> u64 {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIG_SETMASK as i64) })
    }
    #[cfg(any(not(unix), target_arch = "wasm32"))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 2_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigbus() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGBUS as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 7_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigcont() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGCONT as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 18_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigstop() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSTOP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 19_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigtstp() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTSTP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 20_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigttin() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTTIN as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 21_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigttou() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGTTOU as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 22_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigxcpu() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGXCPU as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 24_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigxfsz() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGXFSZ as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 25_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigvtalrm() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGVTALRM as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 26_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigprof() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGPROF as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 27_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigwinch() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGWINCH as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 28_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigsys() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, libc::SIGSYS as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry_nopanic!(_py, { int_bits_from_i64(_py, 31_i64) })
    }
}

// ── POSIX signal functions ─────────────────────────────────────────────────

/// Convert a list of signal ints (u64 bits) into a `sigset_t`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn bits_to_sigset(_py: &PyToken<'_>, list_ptr: *mut u8) -> Result<libc::sigset_t, u64> {
    unsafe {
        let nsig = effective_nsig();
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        let len = crate::builtins::containers::list_len(list_ptr);
        for i in 0..len {
            let elem_bits = seq_vec_ref(list_ptr).get(i).copied().unwrap_or(0);
            let elem_obj = obj_from_bits(elem_bits);
            match to_i64(elem_obj) {
                Some(v) if v > 0 && v < nsig => {
                    libc::sigaddset(&mut set, v as libc::c_int);
                }
                _ => {
                    return Err(raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "invalid signal number in set",
                    ));
                }
            }
        }
        Ok(set)
    }
}

/// Convert a `sigset_t` back to a list of signal number bits.
#[cfg(all(unix, not(target_arch = "wasm32")))]
unsafe fn sigset_to_list_bits(_py: &PyToken<'_>, set: &libc::sigset_t) -> u64 {
    unsafe {
        let nsig = effective_nsig() as libc::c_int;
        let mut elems = Vec::new();
        for sig in 1..nsig {
            if libc::sigismember(set, sig) == 1 {
                elems.push(int_bits_from_i64(_py, sig as i64));
            }
        }
        let list_ptr = alloc_list(_py, &elems);
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_default_int_handler(_signum_bits: u64, _frame_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = (_signum_bits, _frame_bits);
        raise_exception::<u64>(_py, "KeyboardInterrupt", "")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_strsignal(signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let cstr = unsafe { libc::strsignal(signum) };
            if cstr.is_null() {
                return MoltObject::none().bits();
            }
            let s = unsafe { std::ffi::CStr::from_ptr(cstr) };
            let bytes = s.to_bytes();
            let ptr = alloc_string(_py, bytes);
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            // Static lookup table for common signal descriptions on WASM.
            let desc: Option<&[u8]> = match signum {
                1 => Some(b"Hangup"),
                2 => Some(b"Interrupt"),
                3 => Some(b"Quit"),
                4 => Some(b"Illegal instruction"),
                5 => Some(b"Trace/BPT trap"),
                6 => Some(b"Aborted"),
                7 => Some(b"Bus error"),
                8 => Some(b"Floating point exception"),
                9 => Some(b"Killed"),
                10 => Some(b"User defined signal 1"),
                11 => Some(b"Segmentation fault"),
                12 => Some(b"User defined signal 2"),
                13 => Some(b"Broken pipe"),
                14 => Some(b"Alarm clock"),
                15 => Some(b"Terminated"),
                17 => Some(b"Child exited"),
                18 => Some(b"Continued"),
                19 => Some(b"Stopped (signal)"),
                20 => Some(b"Stopped"),
                21 => Some(b"Stopped (tty input)"),
                22 => Some(b"Stopped (tty output)"),
                24 => Some(b"CPU time limit exceeded"),
                25 => Some(b"File size limit exceeded"),
                26 => Some(b"Virtual timer expired"),
                27 => Some(b"Profiling timer expired"),
                28 => Some(b"Window changed"),
                31 => Some(b"Bad system call"),
                _ => None,
            };
            match desc {
                Some(bytes) => {
                    let ptr = alloc_string(_py, bytes);
                    MoltObject::from_ptr(ptr).bits()
                }
                None => MoltObject::none().bits(),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pthread_sigmask(how_bits: u64, mask_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let how_obj = obj_from_bits(how_bits);
            let how = match to_i64(how_obj) {
                Some(v) => v as libc::c_int,
                None => {
                    return raise_exception::<u64>(_py, "TypeError", "how must be an integer");
                }
            };
            // Validate how value against platform constants.
            if how != libc::SIG_BLOCK && how != libc::SIG_UNBLOCK && how != libc::SIG_SETMASK {
                return raise_exception::<u64>(_py, "ValueError", "invalid value for how");
            }

            let mask_obj = obj_from_bits(mask_bits);
            let mask_ptr = match mask_obj.as_ptr() {
                Some(p) => p,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "mask must be a list of signal numbers",
                    );
                }
            };
            let new_set = match unsafe { bits_to_sigset(_py, mask_ptr) } {
                Ok(s) => s,
                Err(e) => return e,
            };
            let mut old_set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::pthread_sigmask(how, &new_set, &mut old_set) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            unsafe { sigset_to_list_bits(_py, &old_set) }
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = (how_bits, mask_bits);
            raise_exception::<u64>(
                _py,
                "OSError",
                "pthread_sigmask not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_pthread_kill(thread_id_bits: u64, signum_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let tid_obj = obj_from_bits(thread_id_bits);
            let tid = match to_i64(tid_obj) {
                Some(v) => v as libc::pthread_t,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "thread_id must be an integer",
                    );
                }
            };
            let signum = match sig_from_bits(_py, signum_bits) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let rc = unsafe { libc::pthread_kill(tid, signum) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::from_raw_os_error(rc).to_string(),
                );
            }
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = (thread_id_bits, signum_bits);
            raise_exception::<u64>(
                _py,
                "OSError",
                "pthread_kill not available on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigpending() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::sigpending(&mut set) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            unsafe { sigset_to_list_bits(_py, &set) }
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            raise_exception::<u64>(_py, "OSError", "sigpending not available on this platform")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigwait(sigset_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let sigset_obj = obj_from_bits(sigset_bits);
            let sigset_ptr = match sigset_obj.as_ptr() {
                Some(p) => p,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "sigset must be a list of signal numbers",
                    );
                }
            };
            let wait_set = match unsafe { bits_to_sigset(_py, sigset_ptr) } {
                Ok(s) => s,
                Err(e) => return e,
            };
            let mut sig: libc::c_int = 0;
            let rc = unsafe { libc::sigwait(&wait_set, &mut sig) };
            if rc != 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::from_raw_os_error(rc).to_string(),
                );
            }
            int_bits_from_i64(_py, sig as i64)
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = sigset_bits;
            raise_exception::<u64>(_py, "OSError", "sigwait not available on this platform")
        }
    })
}

// ── Valid signals set ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_valid_signals() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Enumerate known valid signal numbers for this platform.
        let valid: Vec<i64> = {
            #[cfg(unix)]
            {
                let candidates: &[i64] = &[
                    libc::SIGABRT as i64,
                    libc::SIGFPE as i64,
                    libc::SIGHUP as i64,
                    libc::SIGILL as i64,
                    libc::SIGINT as i64,
                    libc::SIGPIPE as i64,
                    libc::SIGQUIT as i64,
                    libc::SIGSEGV as i64,
                    libc::SIGTERM as i64,
                    libc::SIGUSR1 as i64,
                    libc::SIGUSR2 as i64,
                    libc::SIGCHLD as i64,
                    libc::SIGALRM as i64,
                    libc::SIGBUS as i64,
                    libc::SIGTRAP as i64,
                    libc::SIGTSTP as i64,
                    libc::SIGCONT as i64,
                    libc::SIGWINCH as i64,
                    libc::SIGSTOP as i64,
                    libc::SIGTTIN as i64,
                    libc::SIGTTOU as i64,
                    libc::SIGXCPU as i64,
                    libc::SIGXFSZ as i64,
                    libc::SIGVTALRM as i64,
                    libc::SIGPROF as i64,
                    libc::SIGSYS as i64,
                ];
                candidates.to_vec()
            }
            #[cfg(not(unix))]
            {
                vec![
                    libc::SIGABRT as i64,
                    libc::SIGFPE as i64,
                    libc::SIGILL as i64,
                    libc::SIGINT as i64,
                    libc::SIGSEGV as i64,
                    libc::SIGTERM as i64,
                ]
            }
        };
        let int_bits: Vec<u64> = valid.iter().map(|&v| int_bits_from_i64(_py, v)).collect();
        let set_ptr = alloc_set_with_entries(_py, &int_bits);
        if set_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(set_ptr).bits()
    })
}
