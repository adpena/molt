//! `molt-jit` runs a Molt program from memory (HF-371,
//! `docs/design/dev_jit_tier.md`).
//!
//! The CLI publishes one JIT image per program (`image.rs`). The runner loads
//! the runtime image it names, compiles the whole program into executable
//! memory with the codegen that writes AOT objects, and performs the AOT main
//! stub's launch sequence in process. No object, link or output publication
//! happens. Stdout, stderr and the exit status are the program's own.
//!
//! ```text
//! molt-jit [--start-receipt <path>] --image <image> [args...]
//! molt-jit --probe
//! ```
//!
//! (`runtime/native_launch_abi.toml` declares the flags.)
//!
//! The image's first line is `#!<runner> --image`, so `execve` of the image
//! starts the runner directly, with no shell, as `<runner> --image <image>
//! [args...]`; the program's argv is `<image> [args...]`. `molt run` and the
//! differential harness use the same command, adding `--start-receipt`.
//!
//! The runner is a C `main`, as an AOT executable is, not a Rust `main`:
//! Rust's start-up code ignores `SIGPIPE` and installs `SIGSEGV`/`SIGBUS`
//! handlers with alternate signal stacks, and the program would inherit both.
//! Compilation runs on its own thread, and every compiler thread has exited
//! before the program starts on the main thread.
//!
//! Nothing the runner writes for itself can stop the program: its own lines
//! ignore write errors (`report`), compiler output goes to a capture whose
//! writes cannot fail (`compiler_output.rs`), and while it works the runner
//! ignores `SIGPIPE` and `SIGXFSZ` (`RunnerSignals`), as CPython does for its
//! whole life. A write to a broken pipe, past the file size limit or to a full
//! disk then fails only that write.

// The test harness brings its own `main`; the runner exports `process_entry`
// as the C `main` everywhere else.
#![cfg_attr(not(test), no_main)]

mod compiler_output;
mod image;
mod launch;
#[path = "launch_abi_generated.rs"]
mod launch_abi;
mod runtime_image;

use compiler_output::CapturedStderr;
use image::JitImage;
use molt_ir::backend_environment::{self, BackendEnv, BackendEnvironment};
use runtime_image::{RuntimeImage, RuntimeLaunchApi};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The runner's own failure (unreadable image, missing runtime image, a
/// program the JIT tier cannot compile). The AOT main stub uses the same
/// status when it cannot install its launch environment.
const EXIT_RUNNER_FAILURE: i32 = 125;
const EXIT_USAGE: i32 = 2;

// The runner's command line and image format, from the launch ABI's one
// declaration (runtime/native_launch_abi.toml). A start receipt is created
// after the program compiled, immediately before the program starts, so its
// absence means the runner failed first: the JIT tier's form of a failed
// build (src/molt/cli/run_tier.py).
use launch_abi::{IMAGE_FLAG, PROBE_FLAG, START_RECEIPT, START_RECEIPT_FLAG};

/// Write one line to stderr, ignoring a failed write: the runner's own
/// messages never stop it, where `eprintln!` would panic. The line goes out
/// in one write.
pub(crate) fn report(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let line = format!("{message}\n");
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// The signals the runner ignores while it works. Each would end the process
/// on a write that should only fail: `SIGPIPE` on a broken pipe (a forwarded
/// diagnostic, a replay), `SIGXFSZ` past the file size limit (the TIR cache,
/// the start receipt). The program starts with the dispositions the runner
/// inherited.
#[cfg(unix)]
struct RunnerSignals(Vec<(libc::c_int, libc::sigaction)>);

#[cfg(unix)]
impl RunnerSignals {
    const IGNORED: [libc::c_int; 2] = [libc::SIGPIPE, libc::SIGXFSZ];

    /// # Safety
    /// The process must be single-threaded.
    unsafe fn ignore() -> Self {
        let mut replaced = Vec::with_capacity(Self::IGNORED.len());
        // SAFETY: plain signal-set and disposition calls on owned values.
        unsafe {
            let mut pending = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut pending);
            libc::sigpending(&mut pending);
            for signal in Self::IGNORED {
                // A pending signal is a blocked one the program must still
                // receive; ignoring it would discard it. Blocked, it cannot
                // end the runner either.
                if libc::sigismember(&pending, signal) == 1 {
                    continue;
                }
                let mut ignore = std::mem::zeroed::<libc::sigaction>();
                ignore.sa_sigaction = libc::SIG_IGN;
                libc::sigemptyset(&mut ignore.sa_mask);
                let mut previous = std::mem::zeroed::<libc::sigaction>();
                if libc::sigaction(signal, &ignore, &mut previous) == 0 {
                    replaced.push((signal, previous));
                }
            }
        }
        Self(replaced)
    }

    /// # Safety
    /// The process must be single-threaded.
    unsafe fn restore(self) {
        for (signal, previous) in self.0 {
            // SAFETY: `previous` is the disposition `ignore` replaced.
            unsafe { libc::sigaction(signal, &previous, std::ptr::null_mut()) };
        }
    }
}

/// The compiled program's `molt_isolate_bootstrap`, published before the
/// runtime starts. The runtime image imports the symbol from this executable.
static PROGRAM_ISOLATE_BOOTSTRAP: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn program_isolate_bootstrap() -> u64 {
    let address = PROGRAM_ISOLATE_BOOTSTRAP.load(Ordering::Acquire);
    if address == 0 {
        report(format_args!(
            "MOLT_APP_BOOTSTRAP_UNAVAILABLE: molt-jit: no program is loaded; molt_isolate_bootstrap cannot initialize a native isolate"
        ));
        std::process::abort();
    }
    // SAFETY: the address is the finalized `molt_isolate_bootstrap` of the
    // loaded program, an `extern "C" fn() -> u64`.
    let bootstrap: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(address) };
    unsafe { bootstrap() }
}

// The runtime image leaves `molt_isolate_bootstrap` for this executable to
// define, as a direct-link host does (molt-runtime-core `app_bootstrap`).
molt_runtime_core::declare_app_bootstrap!(
    molt_runtime_core::app_bootstrap::AppBootstrapProvider::Initializer(program_isolate_bootstrap)
);

fn fail(message: impl std::fmt::Display) -> i32 {
    report(format_args!("molt-jit: {message}"));
    EXIT_RUNNER_FAILURE
}

/// The process entry: the C `main`, called by the C runtime with the
/// process's arguments.
///
/// # Safety
/// `argv` must hold `argc` valid NUL-terminated strings.
#[cfg_attr(not(test), unsafe(export_name = "main"))]
pub unsafe extern "C" fn process_entry(
    argc: libc::c_int,
    argv: *const *const libc::c_char,
) -> libc::c_int {
    // Keep the exported bootstrap in the executable: the linker strips
    // unreferenced functions, and the runtime image binds it at load.
    std::hint::black_box(molt_isolate_bootstrap as unsafe extern "C" fn() -> u64);
    // SAFETY: the caller passes `argc` valid NUL-terminated strings.
    #[cfg_attr(not(target_os = "macos"), expect(unused_mut))]
    let mut arguments = unsafe { process_arguments(argc, argv) };
    #[cfg(target_os = "macos")]
    let restarted = arguments.first().is_some_and(|argument| {
        argument.as_encoded_bytes() == launch_abi::ALLOCATOR_RESTART_FLAG.as_bytes()
    });
    #[cfg(target_os = "macos")]
    if restarted {
        arguments.remove(0);
        // SAFETY: the process is single-threaded; this restart set it.
        unsafe { std::env::remove_var(LARGE_BLOCK_CACHE) };
        LARGE_BLOCK_CACHE_OFF.store(true, Ordering::Relaxed);
    }
    let Some(invocation) = parse_invocation(arguments) else {
        return usage();
    };
    #[cfg(target_os = "macos")]
    if !restarted && matches!(invocation, Invocation::Run { .. }) {
        // SAFETY: the process is single-threaded; `argv` holds `argc` valid
        // NUL-terminated strings.
        if let Err(message) = unsafe { restart_without_large_block_cache(argc, argv) } {
            return fail(message);
        }
    }
    runner_main(invocation)
}

/// libmalloc's setting the runner restarts itself with.
#[cfg(target_os = "macos")]
const LARGE_BLOCK_CACHE: &str = "MallocLargeCache";

/// Whether this process runs with the large-block cache off, for the timing
/// line (`MOLT_BACKEND_TIMING`).
static LARGE_BLOCK_CACHE_OFF: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Give the program a process without the compiler's freed heap.
///
/// macOS's allocator keeps freed large blocks in a cache for reuse. The
/// compiler frees almost all it built, but the cache keeps those pages dirty,
/// so a program that imports `subprocess` would run with an 885 MB footprint
/// (6.7 MB under AOT). `malloc_zone_pressure_relief` releases none of them,
/// and libmalloc reads its settings only when a process starts. So the runner
/// restarts itself once, as the same process (`execv` keeps the pid,
/// descriptors, signal mask and dispositions), with `MallocLargeCache=0` and
/// its own first argument `ALLOCATOR_RESTART_FLAG`; the restarted runner
/// drops both before anything else runs, so the program sees its caller's
/// environment and arguments. The marker travels in argv, not in the
/// environment, so no variable a caller passes can stand for it. A caller
/// that sets `MallocLargeCache` itself keeps its own setting: no restart.
///
/// The setting holds for the whole process, the program included. Molt's own
/// heap (mimalloc in the runtime image) does not use it, but libc `malloc`
/// does (C extensions, `PyMem_Malloc`): churning large blocks costs 50-200
/// times more with the cache off (docs/design/dev_jit_tier.md, "Costs").
///
/// # Safety
/// The process must be single-threaded, and `argv` must hold `argc` valid
/// NUL-terminated strings.
#[cfg(target_os = "macos")]
unsafe fn restart_without_large_block_cache(
    argc: libc::c_int,
    argv: *const *const libc::c_char,
) -> Result<(), String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the runner reads the allocator setting its caller passed"
    )]
    if std::env::var_os(LARGE_BLOCK_CACHE).is_some() {
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot find the runner to restart it: {error}"))?;
    let executable = std::ffi::CString::new(executable.as_os_str().as_encoded_bytes())
        .map_err(|_| "the runner path contains a NUL byte".to_string())?;
    let flag = std::ffi::CString::new(launch_abi::ALLOCATOR_RESTART_FLAG)
        .map_err(|_| "the allocator restart flag contains a NUL byte".to_string())?;
    let count = usize::try_from(argc).unwrap_or(0);
    let mut restart: Vec<*const libc::c_char> = Vec::with_capacity(count + 2);
    // SAFETY: the caller guarantees `argc` valid strings; argv[0] stays first.
    unsafe {
        restart.push(*argv);
        restart.push(flag.as_ptr());
        restart.extend((1..count).map(|index| *argv.add(index)));
    }
    restart.push(std::ptr::null());
    // SAFETY: the caller guarantees a single-threaded process; `restart` is a
    // NUL-terminated vector of live strings; `execv` returns only on failure.
    unsafe {
        std::env::set_var(LARGE_BLOCK_CACHE, "0");
        libc::execv(executable.as_ptr(), restart.as_ptr());
    }
    Err(format!(
        "cannot restart the runner without the large-block cache: {}",
        std::io::Error::last_os_error()
    ))
}

/// The arguments after argv[0], as the OS passed them.
///
/// # Safety
/// `argv` must hold `argc` valid NUL-terminated strings.
#[cfg(unix)]
unsafe fn process_arguments(argc: libc::c_int, argv: *const *const libc::c_char) -> Vec<OsString> {
    use std::os::unix::ffi::OsStrExt;
    (1..usize::try_from(argc).unwrap_or(0))
        .map(|index| {
            // SAFETY: the caller guarantees `argc` valid strings.
            let argument = unsafe { std::ffi::CStr::from_ptr(*argv.add(index)) };
            std::ffi::OsStr::from_bytes(argument.to_bytes()).to_os_string()
        })
        .collect()
}

#[cfg(not(unix))]
unsafe fn process_arguments(
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> Vec<OsString> {
    std::env::args_os().skip(1).collect()
}

/// What the command line asks for.
enum Invocation {
    Probe,
    /// Start the program in `image`; `argv` is the program's, the image first.
    Run {
        image: PathBuf,
        start_receipt: Option<PathBuf>,
        argv: Vec<OsString>,
    },
}

fn parse_invocation(arguments: Vec<OsString>) -> Option<Invocation> {
    let mut args = arguments.into_iter();
    let mut start_receipt = None;
    loop {
        match args.next().as_ref().and_then(|arg| arg.to_str()) {
            Some(PROBE_FLAG) if start_receipt.is_none() && args.len() == 0 => {
                return Some(Invocation::Probe);
            }
            Some(START_RECEIPT_FLAG) if start_receipt.is_none() => {
                start_receipt = Some(PathBuf::from(args.next()?));
            }
            // Everything after the image is the program's, verbatim.
            Some(IMAGE_FLAG) => {
                let image = args.next()?;
                let argv = std::iter::once(image.clone()).chain(args).collect();
                return Some(Invocation::Run {
                    image: PathBuf::from(image),
                    start_receipt,
                    argv,
                });
            }
            _ => return None,
        }
    }
}

fn runner_main(invocation: Invocation) -> i32 {
    match invocation {
        Invocation::Probe => match molt_backend_native::probe_jit_execution() {
            Ok(()) => {
                report(format_args!("molt-jit: executable memory probe passed"));
                0
            }
            Err(error) => fail(format_args!("executable memory probe failed: {error}")),
        },
        Invocation::Run {
            image,
            start_receipt,
            argv,
        } => match run(&image, start_receipt.as_deref(), &argv) {
            Ok(never) => match never {},
            Err(message) => fail(message),
        },
    }
}

fn usage() -> i32 {
    report(format_args!(
        "usage: molt-jit [{START_RECEIPT_FLAG} <path>] {IMAGE_FLAG} <image> [args...]\n       molt-jit {PROBE_FLAG}"
    ));
    EXIT_USAGE
}

/// Snapshot of the request-scoped backend settings that `serve_requests`
/// removes from the process environment, restored before the program runs so
/// it sees the environment an AOT executable would see.
struct RemovedBackendEnvironment(Vec<(&'static str, OsString)>);

impl RemovedBackendEnvironment {
    fn capture() -> Self {
        Self(
            backend_environment::daemon_request_env_keys()
                .filter_map(|key| {
                    #[expect(
                        clippy::disallowed_methods,
                        reason = "the runner snapshots the program's own environment"
                    )]
                    let value = std::env::var_os(key.name());
                    value.map(|value| (key.name(), value))
                })
                .collect(),
        )
    }

    /// # Safety
    /// No other thread may read or write the process environment.
    unsafe fn restore(self) {
        for (name, value) in self.0 {
            // SAFETY: the caller guarantees exclusive environment access.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

fn run(
    image_path: &Path,
    start_receipt: Option<&Path>,
    argv: &[OsString],
) -> Result<std::convert::Infallible, String> {
    // SAFETY: the runner has started no thread yet.
    let signals = unsafe { RunnerSignals::ignore() };
    let image = JitImage::read(image_path)?;
    let request = Arc::new(
        BackendEnvironment::from_request(&image.header.backend_environment)
            .map_err(|error| format!("JIT image backend environment: {error}"))?,
    );
    let timing = {
        let _scope = backend_environment::enter(Arc::clone(&request));
        backend_environment::var(BackendEnv::MOLT_BACKEND_TIMING)
            .is_ok_and(|value| !value.is_empty() && value != "0")
    };
    // SAFETY: the runner has started no thread yet.
    let capture = match unsafe { CapturedStderr::begin(image.header.compiler_diagnostics) } {
        Ok(capture) => Some(capture),
        // The program still starts; only compiler output goes to stderr.
        Err(reason) => {
            report(format_args!(
                "molt-jit: compiler output goes to stderr: {reason}"
            ));
            None
        }
    };
    let removed = RemovedBackendEnvironment::capture();
    // SAFETY: the runner is still single-threaded. Backend settings now come
    // only from this compilation's request scope.
    unsafe { backend_environment::serve_requests() };
    let compiled = load_and_compile(&image, request, timing);
    // Every compiler thread has exited; `finish` joins the capture's thread,
    // so the process then holds only this thread.
    if let Some(capture) = capture {
        capture.finish(compiled.is_err());
    }
    let (api, program) = compiled?;
    let entry = program.entry_points().map_err(|error| error.to_string())?;
    // The program runs with only its code, the runtime and its launch plan:
    // the image bytes and the compiler's freed heap go back to the OS first.
    let launch = image.header.launch;
    drop(image.bytes);
    release_compiler_memory();
    if let Some(path) = start_receipt {
        write_start_receipt(path)?;
    }
    // SAFETY: compilation is over and no other thread exists; the runtime
    // starts below, on this thread.
    unsafe {
        removed.restore();
        signals.restore();
    }
    PROGRAM_ISOLATE_BOOTSTRAP.store(entry.isolate_bootstrap as usize, Ordering::Release);
    // The program's code lives for the rest of the process.
    std::mem::forget(program);
    // SAFETY: `entry` belongs to a program compiled against the runtime image
    // `api` was resolved in; the environment is this thread's alone.
    unsafe { launch::run_program(&api, entry, &launch, argv) }
}

/// Return the compiler's freed heap to the OS before the program starts, where
/// the allocator does not do so as it frees. glibc keeps freed pages in its
/// arenas until `malloc_trim`. macOS needs no call here: the runner restarts
/// itself with the large-block cache off (`restart_without_large_block_cache`).
fn release_compiler_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> libc::c_int;
        }
        // SAFETY: trims glibc's arenas; no other thread runs.
        unsafe { malloc_trim(0) };
    }
}

/// Load the runtime image, then compile the program against it on a compiler
/// thread. Returns once every compiler thread has exited.
fn load_and_compile(
    image: &JitImage,
    request: Arc<BackendEnvironment>,
    timing: bool,
) -> Result<(RuntimeLaunchApi, molt_backend_native::JitProgram), String> {
    let started = std::time::Instant::now();
    let runtime = Arc::new(RuntimeImage::open(&image.header.runtime_image)?);
    let api = runtime.launch_api()?;
    let loaded = started.elapsed();
    let document = image.program()?;
    let tir_cache_root = image.header.tir_cache_root.clone();
    let decoded = started.elapsed();
    let compiler = std::thread::Builder::new()
        .name("molt-jit-compile".to_string())
        .stack_size(molt_backend_native::tir::pipeline_cache::COMPILE_THREAD_STACK_BYTES)
        .spawn(move || {
            let _scope = backend_environment::enter(request);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                compile(document, &runtime, tir_cache_root)
            }))
        })
        .map_err(|error| format!("cannot start the compiler thread: {error}"))?;
    let compiled = compiler.join();
    molt_backend_native::tir::pipeline_cache::retire_compile_thread_pools();
    let program = compiled.ok().and_then(Result::ok).ok_or_else(|| {
        "compilation panicked; the message above names the construct".to_string()
    })??;
    if timing {
        let compiled = started.elapsed();
        report(format_args!(
            "MOLT_JIT_TIMING: runtime image load {:.2?}, program decode {:.2?}, compile {:.2?}, large-block cache {}",
            loaded,
            decoded - loaded,
            compiled - decoded,
            if LARGE_BLOCK_CACHE_OFF.load(Ordering::Relaxed) {
                "off"
            } else {
                "on"
            }
        ));
    }
    Ok((api, program))
}

/// Record that the program compiled and starts next. Exclusive creation
/// keeps a stale receipt from claiming this start.
fn write_start_receipt(path: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create start receipt {}: {error}", path.display()))?;
    file.write_all(START_RECEIPT.as_bytes())
        .map_err(|error| format!("cannot write start receipt {}: {error}", path.display()))
}

/// Compile the whole program into executable memory against `image`, with
/// the persistent TIR cache under `tir_cache_root` (the image's, never the
/// program's environment or current directory).
fn compile(
    document: molt_ir::ir::BackendIrDocument,
    image: &Arc<RuntimeImage>,
    tir_cache_root: PathBuf,
) -> Result<molt_backend_native::JitProgram, String> {
    let molt_ir::ir::BackendIrDocument {
        mut ir,
        module_registry,
        code_slots,
    } = document;
    molt_backend_native::prepare_native_application_ir(
        &mut ir,
        module_registry.as_ref(),
        &code_slots,
        false,
    )
    .map_err(|diagnostic| format!("program admission failed: {diagnostic}"))?;
    let lookup_image = Arc::clone(image);
    let mut backend = molt_backend_native::SimpleBackend::new_jit(Arc::new(move |name| {
        lookup_image.symbol(name)
    }));
    // The JIT owns the whole program: no shared-stdlib partition exists.
    backend.skip_shared_stdlib_partition = true;
    backend.tir_cache =
        molt_backend_native::tir::pipeline_cache::TirCacheLocation::Root(tir_cache_root);
    backend.module_registry = module_registry;
    backend.code_slots = Some(code_slots);
    backend.compile_jit(ir).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_process_entry_refuses_a_command_line_it_does_not_know() {
        let flag = |name: &str| std::ffi::CString::new(name).expect("a flag has no NUL");
        let (image, receipt, probe) = (
            flag(super::IMAGE_FLAG),
            flag(super::START_RECEIPT_FLAG),
            flag(super::PROBE_FLAG),
        );
        for argv in [
            vec![
                c"molt-jit".as_ptr(),
                c"--launch".as_ptr(),
                c"a.jit".as_ptr(),
            ],
            vec![c"molt-jit".as_ptr(), image.as_ptr()],
            vec![c"molt-jit".as_ptr(), receipt.as_ptr()],
            vec![c"molt-jit".as_ptr(), probe.as_ptr(), c"extra".as_ptr()],
        ] {
            let count = libc::c_int::try_from(argv.len()).unwrap();
            // SAFETY: valid NUL-terminated strings.
            let status = unsafe { super::process_entry(count, argv.as_ptr()) };
            assert_eq!(status, super::EXIT_USAGE);
        }
    }

    #[test]
    fn the_process_entry_refuses_an_unknown_flag() {
        let argv = [c"molt-jit".as_ptr(), c"--bogus".as_ptr()];
        // SAFETY: two valid NUL-terminated strings.
        let status = unsafe { super::process_entry(2, argv.as_ptr()) };
        assert_eq!(status, super::EXIT_USAGE);
    }
}
