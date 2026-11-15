//! The stack compiled Python runs on.
//!
//! CPython 3.11+ keeps Python frames off the C stack, so recursion is bounded
//! by `sys.getrecursionlimit()` and memory alone: a million nested calls run.
//! Molt compiles each Python call to a native call, so every frame uses the
//! machine stack, and the OS default (8 MiB on a main thread, 2 MiB for a Rust
//! thread) failed between 10,000 and 50,000 frames with SIGSEGV.
//!
//! Each thread that runs compiled code therefore runs on a reservation of
//! [`PROGRAM_STACK_RESERVE_BYTES`] of address space. The OS commits pages only
//! as the stack reaches them, so a shallow program pays nothing and a deep one
//! pays for the depth it uses, as a growable stack would, with no check or
//! switch on the call path:
//! * the main thread switches to the reservation once, at the program entry
//!   (`molt_run_program_entry`), and stays the OS main thread (Unix:
//!   `stacker::grow`, which maps the region with guard pages);
//! * Windows reserves the main thread's stack in the PE header instead (the
//!   native link's `/STACK` reservation): a switched stack there is a fiber,
//!   whose size Windows commits rather than reserves;
//! * threads the runtime starts request the same reservation
//!   ([`thread_stack_bytes`]), unless the program set `threading.stack_size()`.
//!
//! References: rustc runs on a large thread stack and grows further with
//! `stacker`; Go and the JVM size goroutine and thread stacks independently of
//! the OS main stack. call_anatomy.md section 4.2 owns the plan.

// The reservation is declared once, in runtime/native_launch_abi.toml. At the
// measured cost of about 170 bytes per compiled Python frame, 1 GiB holds
// millions of frames; pages are committed only as they are reached.
use super::program_stack_generated::PROGRAM_STACK_RESERVE_BYTES;

/// The stack a runtime-started thread requests: the program's
/// `threading.stack_size()` when it set one, else the program reservation.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thread_stack_bytes(configured: usize) -> usize {
    if configured == 0 {
        PROGRAM_STACK_RESERVE_BYTES
    } else {
        configured
    }
}

/// Run the compiled program entry `entry(context)` on the program stack. The
/// launch calls it in place of calling the entry directly (runtime
/// native_launch_abi.toml); the entry returns here and the launch finishes on
/// the thread's own stack.
///
/// # Safety
/// `entry` is the address of the compiled program entry, an
/// `extern "C" fn(u64)`, and `context` is this thread's attached execution
/// context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_run_program_entry(entry: u64, context: u64) {
    // The launch passes the compiled entry's code address; one that the target
    // cannot represent is a broken launch, not a recoverable error.
    let Some(code) = crate::provenance::abi::function_ptr(entry) else {
        std::process::abort();
    };
    // SAFETY: `code` is the compiled entry, an `extern "C" fn(u64)`.
    let entry: unsafe extern "C" fn(u64) =
        unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn(u64)>(code) };
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        // SAFETY: the entry runs to completion on the new stack and returns.
        stacker::grow(PROGRAM_STACK_RESERVE_BYTES, || unsafe { entry(context) });
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        // SAFETY: as above; the main thread's stack already is the reservation.
        unsafe { entry(context) }
    }
}
