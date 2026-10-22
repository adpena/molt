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
//! molt-jit --launch <image> [--start-receipt <path>] -- <argv0> [args...]
//! molt-jit --probe
//! ```
//!
//! The runner is a C `main`, as an AOT executable is, not a Rust `main`:
//! Rust's start-up code ignores `SIGPIPE` and installs `SIGSEGV`/`SIGBUS`
//! handlers with alternate signal stacks, and the program would inherit both.
//! Compilation runs on its own thread, and every compiler thread has exited
//! before the program starts on the main thread.

// The test harness brings its own `main`; the runner exports `process_entry`
// as the C `main` everywhere else.
#![cfg_attr(not(test), no_main)]

mod compiler_output;
mod image;
mod launch;
#[path = "launch_abi_generated.rs"]
mod launch_abi;
mod runtime_image;

use compiler_output::{CapturedStderr, CompilerDiagnostics};
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

const USAGE: &str = "usage: molt-jit --launch <image> [--start-receipt <path>] -- <argv0> [args...]\n       molt-jit --probe";

/// The exact contents of a start receipt (`--start-receipt`). The runner
/// creates it after the program compiled, immediately before the program
/// starts, so its absence means the runner failed first: the JIT tier's form
/// of a failed build (`src/molt/cli/run_tier.py`).
const START_RECEIPT: &str = "{\"schema\":\"molt.jit-start.v1\"}\n";

/// The compiled program's `molt_isolate_bootstrap`, published before the
/// runtime starts. The runtime image imports the symbol from this executable.
static PROGRAM_ISOLATE_BOOTSTRAP: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn program_isolate_bootstrap() -> u64 {
    let address = PROGRAM_ISOLATE_BOOTSTRAP.load(Ordering::Acquire);
    if address == 0 {
        eprintln!(
            "MOLT_APP_BOOTSTRAP_UNAVAILABLE: molt-jit: no program is loaded; molt_isolate_bootstrap cannot initialize a native isolate"
        );
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
    eprintln!("molt-jit: {message}");
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
    let arguments = unsafe { process_arguments(argc, argv) };
    runner_main(arguments)
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

fn runner_main(arguments: Vec<OsString>) -> i32 {
    let mut args = arguments.into_iter();
    match args.next().as_ref().and_then(|arg| arg.to_str()) {
        Some("--probe") if args.next().is_none() => {
            match molt_backend_native::probe_jit_execution() {
                Ok(()) => {
                    eprintln!("molt-jit: executable memory probe passed");
                    0
                }
                Err(error) => fail(format_args!("executable memory probe failed: {error}")),
            }
        }
        Some("--launch") => {
            let Some(image) = args.next().map(PathBuf::from) else {
                return usage();
            };
            let mut start_receipt = None;
            let mut separator = args.next();
            if separator.as_ref().and_then(|arg| arg.to_str()) == Some("--start-receipt") {
                let Some(path) = args.next().map(PathBuf::from) else {
                    return usage();
                };
                start_receipt = Some(path);
                separator = args.next();
            }
            if separator.as_ref().and_then(|arg| arg.to_str()) != Some("--") {
                return usage();
            }
            let argv: Vec<OsString> = args.collect();
            if argv.is_empty() {
                return usage();
            }
            match run(&image, start_receipt.as_deref(), &argv) {
                Ok(never) => match never {},
                Err(message) => fail(message),
            }
        }
        _ => usage(),
    }
}

fn usage() -> i32 {
    eprintln!("{USAGE}");
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
    let capture = match image.header.compiler_diagnostics {
        CompilerDiagnostics::Stream => None,
        // SAFETY: the runner has started no thread yet.
        CompilerDiagnostics::OnFailure => Some(unsafe { CapturedStderr::begin() }?),
    };
    let removed = RemovedBackendEnvironment::capture();
    // SAFETY: the runner is still single-threaded. Backend settings now come
    // only from this compilation's request scope.
    unsafe { backend_environment::serve_requests() };
    let compiled = load_and_compile(&image, request, timing);
    // Every compiler thread has exited: the process holds only this thread.
    if let Some(capture) = capture {
        capture.finish(compiled.is_err());
    }
    let (api, program) = compiled?;
    let entry = program.entry_points().map_err(|error| error.to_string())?;
    if let Some(path) = start_receipt {
        write_start_receipt(path)?;
    }
    // SAFETY: compilation is over and no other thread exists; the runtime
    // starts below, on this thread.
    unsafe { removed.restore() };
    PROGRAM_ISOLATE_BOOTSTRAP.store(entry.isolate_bootstrap as usize, Ordering::Release);
    // The program's code lives for the rest of the process.
    std::mem::forget(program);
    // SAFETY: `entry` belongs to a program compiled against the runtime image
    // `api` was resolved in; the environment is this thread's alone.
    unsafe { launch::run_program(&api, entry, &image.header.launch, argv) }
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
    let decoded = started.elapsed();
    let compiler = std::thread::Builder::new()
        .name("molt-jit-compile".to_string())
        .stack_size(molt_backend_native::tir::pipeline_cache::COMPILE_THREAD_STACK_BYTES)
        .spawn(move || {
            let _scope = backend_environment::enter(request);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile(document, &runtime)))
        })
        .map_err(|error| format!("cannot start the compiler thread: {error}"))?;
    let compiled = compiler.join();
    molt_backend_native::tir::pipeline_cache::retire_compile_thread_pools();
    let program = compiled.ok().and_then(Result::ok).ok_or_else(|| {
        "compilation panicked; the message above names the construct".to_string()
    })??;
    if timing {
        let compiled = started.elapsed();
        eprintln!(
            "MOLT_JIT_TIMING: runtime image load {:.2?}, program decode {:.2?}, compile {:.2?}",
            loaded,
            decoded - loaded,
            compiled - decoded
        );
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

/// Compile the whole program into executable memory against `image`.
fn compile(
    document: molt_ir::ir::BackendIrDocument,
    image: &Arc<RuntimeImage>,
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
    backend.module_registry = module_registry;
    backend.code_slots = Some(code_slots);
    backend.compile_jit(ir).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_process_entry_refuses_a_command_line_it_does_not_know() {
        let argv = [c"molt-jit".as_ptr(), c"--bogus".as_ptr()];
        // SAFETY: two valid NUL-terminated strings.
        let status = unsafe { super::process_entry(2, argv.as_ptr()) };
        assert_eq!(status, super::EXIT_USAGE);
    }
}
