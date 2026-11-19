//! In-process program-counter sampler (feature `pc-sampler`, HF-198).
//!
//! The process profiles itself: `setitimer(ITIMER_PROF)` delivers `SIGPROF`
//! every [`INTERVAL_US`] of CPU time, and the handler records the interrupted
//! program counter plus up to [`CALLERS`] return addresses from the frame
//! pointer chain (arm64 macOS keeps frame pointers by ABI). No other process
//! inspects this one, so no operating-system permission is involved. At exit
//! the runtime writes the samples and the main image's load address to the
//! file named by `MOLT_RUNTIME_PC_SAMPLER_FILE`; `atos -l <load> -o <binary>`
//! symbolizes them offline.
//!
//! The handler is async-signal-safe: it reads registers from the signal
//! context, walks frame records whose addresses lie inside the sampled
//! thread's stack bounds, and stores words into a static array.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// CPU microseconds between samples.
const INTERVAL_US: i64 = 200;
/// Return addresses recorded above the interrupted pc.
const CALLERS: usize = 7;
/// Words per sample: the pc, then the callers (zero past the chain's end).
const STRIDE: usize = 1 + CALLERS;
/// Samples kept; later samples are counted but dropped.
const CAPACITY: usize = 1 << 18;

static WORDS: [AtomicU64; CAPACITY * STRIDE] = [const { AtomicU64::new(0) }; CAPACITY * STRIDE];
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// The interrupted pc and frame pointer of a signal context.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn registers(context: *mut libc::c_void) -> (u64, u64) {
    let context = context.cast::<libc::ucontext_t>();
    let state = unsafe { &(*(*context).uc_mcontext).__ss };
    (state.__pc, state.__fp)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn registers(context: *mut libc::c_void) -> (u64, u64) {
    let context = context.cast::<libc::ucontext_t>();
    let gregs = unsafe { &(*context).uc_mcontext.gregs };
    (gregs[libc::REG_RIP as usize] as u64, gregs[libc::REG_RBP as usize] as u64)
}

extern "C" fn on_sample(_signal: libc::c_int, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    let slot = NEXT.fetch_add(1, Ordering::Relaxed);
    if slot >= CAPACITY {
        return;
    }
    let (pc, mut fp) = unsafe { registers(context) };
    let base = slot * STRIDE;
    WORDS[base].store(pc, Ordering::Relaxed);
    let (low, high) = stack_bounds();
    for depth in 0..CALLERS {
        // A frame record is [previous fp, return address]; stop at a null,
        // misaligned, non-ascending or out-of-stack pointer rather than read
        // memory the thread does not own.
        if fp == 0 || fp % 16 != 0 || fp < low || fp + 16 > high {
            break;
        }
        let record = fp as *const u64;
        let (next, ret) = unsafe { (record.read_volatile(), record.add(1).read_volatile()) };
        WORDS[base + 1 + depth].store(ret, Ordering::Relaxed);
        if next <= fp {
            break;
        }
        fp = next;
    }
}

/// The sampled thread's stack as `[low, high)`; empty when unknown, which
/// records the pc alone.
#[cfg(target_os = "macos")]
fn stack_bounds() -> (u64, u64) {
    unsafe {
        let thread = libc::pthread_self();
        let high = libc::pthread_get_stackaddr_np(thread) as u64;
        let size = libc::pthread_get_stacksize_np(thread) as u64;
        (high.saturating_sub(size), high)
    }
}

#[cfg(not(target_os = "macos"))]
fn stack_bounds() -> (u64, u64) {
    (0, 0)
}

/// Start sampling when `MOLT_RUNTIME_PC_SAMPLER_FILE` is set.
pub(crate) fn start() {
    if std::env::var_os("MOLT_RUNTIME_PC_SAMPLER_FILE").is_none() {
        return;
    }
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_sample as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGPROF, &action, std::ptr::null_mut());
        let interval = libc::timeval { tv_sec: 0, tv_usec: INTERVAL_US as _ };
        let timer = libc::itimerval { it_interval: interval, it_value: interval };
        libc::setitimer(libc::ITIMER_PROF, &timer, std::ptr::null_mut());
    }
}

/// Stop sampling and write `load <hex>`, `taken <n>`, then one line of hex
/// words per sample to `MOLT_RUNTIME_PC_SAMPLER_FILE`.
pub(crate) fn write_report() {
    use std::io::Write;
    let Some(path) = std::env::var_os("MOLT_RUNTIME_PC_SAMPLER_FILE") else {
        return;
    };
    unsafe {
        let off = libc::itimerval {
            it_interval: libc::timeval { tv_sec: 0, tv_usec: 0 },
            it_value: libc::timeval { tv_sec: 0, tv_usec: 0 },
        };
        libc::setitimer(libc::ITIMER_PROF, &off, std::ptr::null_mut());
    }
    let Ok(mut file) = std::fs::File::create(&path) else {
        return;
    };
    let _ = writeln!(file, "load {:#x}", image_load_address());
    let taken = NEXT.load(Ordering::Relaxed);
    let _ = writeln!(file, "taken {taken}");
    for slot in 0..taken.min(CAPACITY) {
        let words: Vec<String> = (0..STRIDE)
            .map(|i| WORDS[slot * STRIDE + i].load(Ordering::Relaxed))
            .take_while(|&w| w != 0)
            .map(|w| format!("{w:#x}"))
            .collect();
        let _ = writeln!(file, "{}", words.join(" "));
    }
}

/// The main executable's load address, for `atos -l`.
#[cfg(target_os = "macos")]
fn image_load_address() -> u64 {
    unsafe extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const libc::c_void;
    }
    unsafe { _dyld_get_image_header(0) as u64 }
}

#[cfg(not(target_os = "macos"))]
fn image_load_address() -> u64 {
    0
}
