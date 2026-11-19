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
//! * on Linux and the BSDs the main thread switches to the reservation once, at
//!   the program entry (`molt_run_program_entry`), and stays the OS main thread
//!   (one mapping with a guard page, entered with `psm::on_stack`);
//! * Windows reserves the main thread's stack in the PE header instead (the
//!   native link's `/STACK` reservation): a switched stack there is a fiber,
//!   whose size Windows commits rather than reserves;
//! * macOS sizes the main thread's own stack in the Mach-O header (the native
//!   link's `-stack_size`, which ld caps at 512 MiB on arm64): on a switched
//!   stack `sample` and Instruments cannot walk the frames and attribute every
//!   sample to `_dyld_start`, and 512 MiB holds about three million frames;
//! * threads the runtime starts request the same reservation
//!   ([`thread_stack_bytes`]), unless the program set `threading.stack_size()`.
//!
//! References: rustc runs on a large thread stack and grows further with
//! `stacker`, which Molt does not use; Go and the JVM size goroutine and thread stacks independently of
//! the OS main stack. call_anatomy.md section 4.2 owns the plan.

// The reservation is declared once, in runtime/native_launch_abi.toml. At the
// measured cost of about 170 bytes per compiled Python frame, 1 GiB holds
// millions of frames; pages are committed only as they are reached.
use super::program_stack_generated::PROGRAM_STACK_RESERVE_BYTES;

/// The stack a runtime-started thread requests: the program's
/// `threading.stack_size()` when it set one, else the program reservation.
///
/// Linux charges a thread's stack mapping in full to a finite `RLIMIT_DATA`
/// (the memory backstop, `initialize_runtime_resources`) when the thread
/// starts, so under one a thread requests the C library's thread stack (the
/// `RLIMIT_STACK` soft limit, else 8 MiB) instead of the reservation.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thread_stack_bytes(configured: usize) -> usize {
    if configured != 0 {
        return configured;
    }
    #[cfg(target_os = "linux")]
    {
        let soft = |resource| {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: getrlimit with a valid resource id and an initialized out-param.
            (unsafe { libc::getrlimit(resource, &mut limit) } == 0
                && limit.rlim_cur != libc::RLIM_INFINITY)
                .then_some(limit.rlim_cur as usize)
        };
        if soft(libc::RLIMIT_DATA).is_some() {
            return soft(libc::RLIMIT_STACK).unwrap_or(8 << 20);
        }
    }
    PROGRAM_STACK_RESERVE_BYTES
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
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    {
        let stack = reserve_main_stack();
        // SAFETY: `stack` is a live, writable mapping owned for the process
        // lifetime; the entry runs to completion on it and returns.
        unsafe { psm::on_stack(stack.base as *mut u8, stack.size, || entry(context)) };
    }
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_arch = "wasm32"))))]
    {
        // SAFETY: as above; the main thread's stack already is the reservation
        // (the Windows PE header, the Mach-O header).
        unsafe { entry(context) }
    }
}

/// The main thread's program stack: one mapping for the process lifetime.
#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
#[derive(Clone, Copy)]
pub(crate) struct MainStack {
    /// Lowest usable address (above the guard page).
    base: usize,
    size: usize,
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
static MAIN_STACK: std::sync::OnceLock<MainStack> = std::sync::OnceLock::new();

/// Map the main thread's program stack once. Runtime init calls this before it
/// installs the Linux `RLIMIT_DATA` backstop (`initialize_runtime_resources`),
/// because Linux charges a writable private mapping in full to `RLIMIT_DATA`
/// when it is made: a reservation made after the backstop exceeds it, and one
/// made before is part of the startup footprint the backstop is measured from,
/// as allocator arenas are. Pages are committed only as the stack reaches them.
///
/// The size is the program reservation, bounded by a finite `RLIMIT_AS` (a
/// quarter of it, so a process under `ulimit -v` keeps room for its heap). A
/// mapping that fails under that bound is a broken environment: the process
/// stops with the reason rather than run on a stack it did not ask for.
#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
pub(crate) fn reserve_main_stack() -> MainStack {
    *MAIN_STACK.get_or_init(|| {
        // SAFETY: sysconf/getrlimit/mmap/mprotect with valid arguments; the
        // mapping is never unmapped.
        unsafe {
            let page = libc::sysconf(libc::_SC_PAGESIZE).max(4096) as usize;
            let mut size = PROGRAM_STACK_RESERVE_BYTES;
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_AS, &mut limit) == 0
                && limit.rlim_cur != libc::RLIM_INFINITY
            {
                size = size.min((limit.rlim_cur as usize / 4) & !(page - 1));
            }
            #[cfg(target_os = "linux")]
            let flags = libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE | libc::MAP_STACK;
            #[cfg(not(target_os = "linux"))]
            let flags = libc::MAP_PRIVATE | libc::MAP_ANON;
            let mapping = libc::mmap(
                std::ptr::null_mut(),
                size + page,
                libc::PROT_READ | libc::PROT_WRITE,
                flags,
                -1,
                0,
            );
            if mapping == libc::MAP_FAILED || libc::mprotect(mapping, page, libc::PROT_NONE) != 0 {
                eprintln!(
                    "molt: cannot map the {size}-byte program stack: {}",
                    std::io::Error::last_os_error()
                );
                std::process::abort();
            }
            MainStack {
                base: mapping as usize + page,
                size,
            }
        }
    })
}
