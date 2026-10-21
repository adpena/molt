//! `molt-jit` runs a Molt program from memory (HF-371,
//! `docs/design/dev_jit_tier.md`).
//!
//! The CLI writes a launch manifest beside the program's backend IR. The
//! runner loads the runtime image the manifest names, compiles the whole
//! program into executable memory with the codegen that writes AOT objects,
//! and performs the AOT main stub's launch sequence in process. No object,
//! link or output publication happens. Stdout, stderr and the exit status are
//! the program's own.
//!
//! ```text
//! molt-jit --launch <launch.json> -- <argv0> [args...]
//! molt-jit --probe
//! ```

mod compiler_output;
mod launch;
mod runtime_image;

use compiler_output::{CapturedStderr, CompilerDiagnostics};
use launch::LaunchManifest;
use molt_ir::backend_environment::{self, BackendEnv, BackendEnvironment};
use runtime_image::{RuntimeImage, RuntimeLaunchApi};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The runner's own failure (unreadable manifest, missing runtime image,
/// a program the JIT tier cannot compile). The AOT main stub uses the same
/// status when it cannot install its launch environment.
const EXIT_RUNNER_FAILURE: u8 = 125;
const EXIT_USAGE: u8 = 2;

const USAGE: &str = "usage: molt-jit --launch <launch.json> [--start-receipt <path>] -- <argv0> [args...]\n       molt-jit --probe";

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

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("molt-jit: {message}");
    ExitCode::from(EXIT_RUNNER_FAILURE)
}

fn main() -> ExitCode {
    // Keep the exported bootstrap in the executable: the linker strips
    // unreferenced functions, and the runtime image binds it at load.
    std::hint::black_box(molt_isolate_bootstrap as unsafe extern "C" fn() -> u64);
    let mut args = std::env::args_os().skip(1);
    match args.next().as_ref().and_then(|arg| arg.to_str()) {
        Some("--probe") if args.next().is_none() => {
            match molt_backend_native::probe_jit_execution() {
                Ok(()) => {
                    eprintln!("molt-jit: executable memory probe passed");
                    ExitCode::SUCCESS
                }
                Err(error) => fail(format_args!("executable memory probe failed: {error}")),
            }
        }
        Some("--launch") => {
            let Some(manifest) = args.next().map(PathBuf::from) else {
                eprintln!("{USAGE}");
                return ExitCode::from(EXIT_USAGE);
            };
            let mut start_receipt = None;
            let mut separator = args.next();
            if separator.as_ref().and_then(|arg| arg.to_str()) == Some("--start-receipt") {
                let Some(path) = args.next().map(PathBuf::from) else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(EXIT_USAGE);
                };
                start_receipt = Some(path);
                separator = args.next();
            }
            if separator.as_ref().and_then(|arg| arg.to_str()) != Some("--") {
                eprintln!("{USAGE}");
                return ExitCode::from(EXIT_USAGE);
            }
            let argv: Vec<OsString> = args.collect();
            if argv.is_empty() {
                eprintln!("{USAGE}");
                return ExitCode::from(EXIT_USAGE);
            }
            match run(&manifest, start_receipt.as_deref(), &argv) {
                Ok(never) => match never {},
                Err(message) => fail(message),
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
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
    manifest_path: &std::path::Path,
    start_receipt: Option<&std::path::Path>,
    argv: &[OsString],
) -> Result<std::convert::Infallible, String> {
    let manifest = LaunchManifest::read(manifest_path)?;
    let request = BackendEnvironment::from_request(&manifest.backend_environment)
        .map_err(|error| format!("launch manifest backend environment: {error}"))?;
    let removed = RemovedBackendEnvironment::capture();
    // SAFETY: the runner is still single-threaded. Backend settings now come
    // only from this compilation's request scope.
    unsafe { backend_environment::serve_requests() };
    let scope = backend_environment::enter(Arc::new(request));
    let timing = backend_environment::var(BackendEnv::MOLT_BACKEND_TIMING)
        .is_ok_and(|value| !value.is_empty() && value != "0");

    let started = std::time::Instant::now();
    let image = Arc::new(RuntimeImage::open(&manifest.runtime_image)?);
    let api = RuntimeLaunchApi::resolve(&image)?;
    let loaded = started.elapsed();

    let document = read_program(&manifest)?;
    let decoded = started.elapsed();
    let capture = match manifest.compiler_diagnostics {
        CompilerDiagnostics::Stream => None,
        CompilerDiagnostics::OnFailure => Some(CapturedStderr::begin()?),
    };
    let compiled = compile(document, &image);
    if let Some(capture) = capture {
        capture.finish(compiled.is_err());
    }
    let (program, entry) = compiled?;
    if timing {
        let compiled = started.elapsed();
        eprintln!(
            "MOLT_JIT_TIMING: runtime image load {:.2?}, program decode {:.2?}, compile {:.2?}",
            loaded,
            decoded - loaded,
            compiled - decoded
        );
    }
    drop(scope);
    if let Some(path) = start_receipt {
        write_start_receipt(path)?;
    }
    // SAFETY: compilation is over; no thread reads the environment until the
    // runtime starts below, on this thread.
    unsafe { removed.restore() };
    PROGRAM_ISOLATE_BOOTSTRAP.store(entry.isolate_bootstrap as usize, Ordering::Release);
    // The program's code lives for the rest of the process.
    std::mem::forget(program);
    // SAFETY: `entry` belongs to a program compiled against `image`, whose
    // launch API `api` is; the environment is this thread's alone.
    unsafe { launch::run_program(&api, entry, &manifest.launch, argv) }
}

/// Record that the program compiled and starts next. Exclusive creation
/// keeps a stale receipt from claiming this start.
fn write_start_receipt(path: &std::path::Path) -> Result<(), String> {
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
) -> Result<
    (
        molt_backend_native::JitProgram,
        molt_backend_native::JitEntryPoints,
    ),
    String,
> {
    let molt_ir::ir::BackendIrDocument {
        mut ir,
        module_registry,
        code_slots,
    } = document;
    molt_backend_native::prepare_native_application_ir(&mut ir, module_registry.as_ref(), false);
    let lookup_image = Arc::clone(image);
    let mut backend = molt_backend_native::SimpleBackend::new_jit(Arc::new(move |name| {
        lookup_image.symbol(name)
    }));
    // The JIT owns the whole program: no shared-stdlib partition exists.
    backend.skip_shared_stdlib_partition = true;
    backend.module_registry = module_registry;
    backend.code_slots = Some(code_slots);
    let program =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.compile_jit(ir)))
            .map_err(|_| "compilation panicked; the message above names the construct".to_string())?
            .map_err(|error| error.to_string())?;
    let entry = program.entry_points().map_err(|error| error.to_string())?;
    Ok((program, entry))
}

/// Read the program document the manifest names, refusing any other bytes.
fn read_program(manifest: &LaunchManifest) -> Result<molt_ir::ir::BackendIrDocument, String> {
    use sha2::Digest;
    use std::io::Read;
    let path = &manifest.program;
    let mut file = molt_artifact_publish::open_regular_artifact(path)
        .map_err(|error| format!("cannot open program {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read program {}: {error}", path.display()))?;
    let digest: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != manifest.program_sha256 {
        return Err(format!(
            "program {} has SHA-256 {digest}; its launch manifest names {}",
            path.display(),
            manifest.program_sha256
        ));
    }
    molt_ir::ir::BackendIrDocument::from_msgpack_reader(&mut bytes.as_slice())
        .map_err(|error| format!("invalid program {}: {error}", path.display()))
}
