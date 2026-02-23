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

/// Maximum supported signal number (Linux/macOS NSIG is 32 or 65).
const MAX_SIGNAL: usize = 64;

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

fn sig_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i32, u64> {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) if v > 0 && v < MAX_SIGNAL as i64 => Ok(v as i32),
        Some(v) if v <= 0 => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "signal number must be positive",
        )),
        Some(_) => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            &format!("signal number out of range (max {})", MAX_SIGNAL - 1),
        )),
        None => Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "signal number must be int",
        )),
    }
}

/// Convert a handler bits value to the int sentinel visible from Python.
fn handler_bits_to_py(_py: &PyToken<'_>, bits: u64) -> u64 {
    if bits == HANDLER_SIG_DFL {
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
    crate::with_gil_entry!(_py, {
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

        handler_bits_to_py(_py, old_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_getsignal(signum_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let signum = match sig_from_bits(_py, signum_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let bits = HANDLER_TABLE[signum as usize].load(Ordering::SeqCst);
        handler_bits_to_py(_py, bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_raise_signal(signum_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let new_fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(-1) as i32;
        let old_fd = WAKEUP_FD.swap(new_fd, Ordering::SeqCst);
        int_bits_from_i64(_py, old_fd as i64)
    })
}

// ── Signal number constants ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigabrt() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGABRT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigfpe() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGFPE as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigill() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGILL as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigint() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGINT as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigsegv() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGSEGV as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigterm() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGTERM as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sighup() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGHUP as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 1_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigquit() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGQUIT as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 3_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr1() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGUSR1 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 10_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigusr2() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGUSR2 as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 12_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigchld() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGCHLD as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 17_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigalrm() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGALRM as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 14_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sigpipe() -> u64 {
    #[cfg(unix)]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::SIGPIPE as i64) })
    }
    #[cfg(not(unix))]
    {
        crate::with_gil_entry!(_py, { int_bits_from_i64(_py, 13_i64) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_dfl() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, SIG_DFL_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_sig_ign() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, SIG_IGN_INT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_nsig() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, MAX_SIGNAL as i64) })
}

// ── Valid signals set ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_valid_signals() -> u64 {
    crate::with_gil_entry!(_py, {
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
