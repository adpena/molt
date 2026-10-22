//! The AOT main stub's launch sequence, performed in process.
//!
//! `src/molt/cli/native_launch_plan.py` owns the launch facts; the JIT image
//! header carries them (`image.rs`), and `src/molt/cli/native_main_stub.py`
//! renders the same facts as the C `main` of an AOT executable. The order
//! below is that stub's order.

use crate::runtime_image::RuntimeLaunchApi;
use molt_backend_native::JitEntryPoints;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::{CString, OsString, c_char};

/// The facts the AOT main stub bakes into `main`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaunchPlan {
    /// The resolved capability policy and execution target, installed with
    /// overwrite before the runtime starts.
    pub(crate) environment: BTreeMap<String, String>,
    /// External native artifact roots, prepended to `MOLT_MODULE_ROOTS`.
    pub(crate) runtime_module_roots: Vec<String>,
    /// The program's own source files, for tracebacks without `fs.read`.
    pub(crate) compiled_source_paths: Vec<String>,
}

/// The stub's exit status when it cannot install the launch environment.
const LAUNCH_ENVIRONMENT_FAILURE: i32 = 125;

fn launch_failure(message: &str) -> ! {
    crate::report(format_args!("molt: {message}"));
    // SAFETY: `_exit` ends the process without unwinding, as the stub does.
    unsafe { libc::_exit(LAUNCH_ENVIRONMENT_FAILURE) }
}

fn c_string(text: &str, what: &str) -> CString {
    CString::new(text).unwrap_or_else(|_| launch_failure(&format!("{what} contains NUL")))
}

/// `setenv(key, value, 1)`, ending the process as the stub does on failure.
///
/// # Safety
/// No other thread may read or write the process environment concurrently.
unsafe fn set_environment(key: &str, value: &str, failure: &str) {
    let key = c_string(key, "launch environment key");
    let value = c_string(value, "launch environment value");
    // SAFETY: both strings are NUL-terminated; the caller owns the environment.
    if unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) } != 0 {
        launch_failure(failure);
    }
}

const MODULE_ROOTS_ENV: &str = "MOLT_MODULE_ROOTS";

#[cfg(windows)]
const MODULE_ROOTS_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const MODULE_ROOTS_SEPARATOR: &str = ":";

/// Run the program: the stub's `main` and `molt_finish`, in the order
/// `runtime/native_launch_abi.toml` declares (its `start` entries, the entry,
/// then its `finish` entries).
///
/// # Safety
/// `entry` must come from a program compiled against the runtime image `api`
/// was resolved in, and no other thread may touch the process environment
/// while the launch environment is installed.
pub(crate) unsafe fn run_program(
    api: &RuntimeLaunchApi,
    entry: JitEntryPoints,
    plan: &LaunchPlan,
    argv: &[OsString],
) -> ! {
    for (key, value) in &plan.environment {
        // SAFETY: the caller guarantees exclusive environment access.
        unsafe { set_environment(key, value, "failed to install resolved capability policy") };
    }
    if !plan.runtime_module_roots.is_empty() {
        let roots = plan.runtime_module_roots.join(MODULE_ROOTS_SEPARATOR);
        #[expect(
            clippy::disallowed_methods,
            reason = "MOLT_MODULE_ROOTS is a program setting, not a backend setting"
        )]
        let merged = match std::env::var(MODULE_ROOTS_ENV) {
            Ok(existing) if !existing.is_empty() => {
                format!("{roots}{MODULE_ROOTS_SEPARATOR}{existing}")
            }
            _ => roots,
        };
        // SAFETY: the caller guarantees exclusive environment access.
        unsafe {
            set_environment(
                MODULE_ROOTS_ENV,
                &merged,
                "failed to set runtime module roots",
            )
        };
    }
    let source_paths: Vec<CString> = plan
        .compiled_source_paths
        .iter()
        .map(|path| c_string(path, "compiled source path"))
        .collect();
    let source_pointers: Vec<*const c_char> =
        source_paths.iter().map(|path| path.as_ptr()).collect();
    let arguments: Vec<CString> = argv.iter().map(os_c_string).collect();
    let mut argument_pointers: Vec<*const c_char> =
        arguments.iter().map(|argument| argument.as_ptr()).collect();
    argument_pointers.push(std::ptr::null());
    let argc = i32::try_from(arguments.len())
        .unwrap_or_else(|_| launch_failure("too many program arguments"));
    // SAFETY: every function pointer was resolved in `runtime` with the C
    // signature the stub declares; `entry` belongs to this runtime's program.
    unsafe {
        (api.molt_register_compiled_sources)(
            if source_pointers.is_empty() {
                std::ptr::null()
            } else {
                source_pointers.as_ptr()
            },
            source_pointers.len() as u64,
        );
        (api.molt_set_app_callable_resolver)(entry.app_callable_resolver as u64);
        (api.molt_module_registry_install)(entry.module_registry_blob);
        (api.molt_runtime_init)();
        (api.molt_runtime_ensure_gil)();
        (api.molt_set_argv)(argc, argument_pointers.as_ptr());
        let main: unsafe extern "C" fn() = std::mem::transmute(entry.main);
        main();
        finish(api)
    }
}

/// The stub's `molt_finish`: report an uncaught exception, then leave through
/// the runtime's process exit.
unsafe fn finish(api: &RuntimeLaunchApi) -> ! {
    // SAFETY: the runtime is initialized and this thread holds its GIL.
    unsafe {
        let pending = (api.molt_exception_pending)();
        #[expect(
            clippy::disallowed_methods,
            reason = "MOLT_DEBUG_MAIN_EXCEPTION is the stub's own debug switch"
        )]
        if std::env::var("MOLT_DEBUG_MAIN_EXCEPTION")
            .is_ok_and(|value| !value.is_empty() && value != "0")
        {
            crate::report(format_args!(
                "molt main finish pending={}",
                i32::from(pending != 0)
            ));
        }
        if pending != 0 {
            let exception = (api.molt_exception_last_pending)();
            let exit_code = (api.molt_exception_report_uncaught)(exception);
            (api.molt_dec_ref_obj)(exception);
            (api.molt_runtime_exit)(exit_code);
            libc::_exit(1)
        }
        (api.molt_runtime_exit)(0);
        libc::_exit(0)
    }
}

#[cfg(unix)]
fn os_c_string(argument: &OsString) -> CString {
    use std::os::unix::ffi::OsStrExt;
    CString::new(argument.as_bytes()).unwrap_or_else(|_| launch_failure("argument contains NUL"))
}

#[cfg(not(unix))]
fn os_c_string(argument: &OsString) -> CString {
    // Windows passes UTF-16 arguments through `molt_set_argv_utf16`; the
    // runner does not support Windows yet (`RuntimeImage::open` refuses it).
    c_string(&argument.to_string_lossy(), "program argument")
}
