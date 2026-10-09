//! Native CPython-ABI C-extension DISCOVERY harness.
//!
//! Purpose
//! -------
//! Turn the ~30-minute "one leaf per wasm-witness cycle" numpy-import discovery
//! loop into a native, re-runnable, **seconds-per-frontier** sweep. Every
//! runtime-semantic frontier the numpy/scipy wasm witness hits (a silent `-1`,
//! a wrong answer, a panic, a trap, a missing symbol) lives in
//! `runtime/molt-cpython-abi` + its `molt-runtime` hooks — platform-independent
//! Rust. So the *same* divergence reproduces natively with a real backtrace.
//!
//! Single-static-pool design
//! -------------------------
//! This crate is a `cdylib` that links BOTH `molt-runtime` (which owns
//! `register_cpython_hooks`) AND `molt-lang-cpython-abi` (the ABI shim + the
//! `loader`) into ONE image. Consequences:
//!   * there is exactly ONE `molt_cpython_abi` static pool in the image;
//!   * `molt_cext_discovery_init` registers the REAL runtime hooks into it
//!     (not the no-op `STUB_HOOKS` the `cext_integration` test used);
//!   * because it is a `cdylib`, the `#[unsafe(no_mangle)]` `Py*` ABI symbols are
//!     exported into the image's dynamic symbol table.
//!
//! A tiny C driver (`tools/native_cext_driver.c`) `dlopen`s this image with
//! `RTLD_GLOBAL`, calls `molt_cext_discovery_init`, then `molt_cext_discovery_load`.
//! The loader `dlopen`s the real prebuilt extension `.so`; its
//! `-undefined dynamic_lookup` (macOS) / flat-namespace (Linux) `Py*` imports
//! resolve against THIS image — so the extension drives the exact same ABI +
//! hooks the wasm witness drives, but natively, in seconds.

// Punch-through instrumentation: canonical CPython symbols numpy links that
// molt's ABI is missing or exports under a different name. See the module doc.
mod discovery_stubs;

// Discovery loads extensions but has no compiler-generated application image.
molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-cext-discovery"
));

use molt_cpython_abi::abi_types::PyTypeObject;
use molt_cpython_abi::api::{errors, refcount::OwnedPyObject, strings, typeobj};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Register molt-runtime's REAL CPython-ABI hooks into the single
/// `molt_cpython_abi` static pool contained in THIS image, and install a panic
/// hook that captures a full backtrace as a frontier marker.
///
/// Idempotent. Must be called before `molt_cext_discovery_load`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cext_discovery_init() {
    if INIT_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n===MOLT_FRONTIER_PANIC===");
        eprintln!("panic: {info}");
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("backtrace:\n{bt}");
        eprintln!("===END_MOLT_FRONTIER_PANIC===");
    }));
    // Registers ~60 real hooks (alloc_str/int/list/dict/tuple, exceptions,
    // modules, number ops, object call, foreign_new, ...) into the ABI shim.
    molt_runtime::cpython_abi_hooks::register_cpython_hooks();
    eprintln!("===MOLT_DISCOVERY: real cpython-abi hooks registered (single static pool)");
}

/// Drive `PyInit_<name>()` of the prebuilt extension at `so_path` against the
/// molt ABI. Returns:
///   * `0`  on Ok(module handle),
///   * `-1` on a captured `LoadError` (loud, structured — the frontier),
///   * `-2` on a caught Rust panic (full backtrace already printed above),
///   * `-3` on a bad-argument error.
///
/// Hard native crashes (SIGSEGV/SIGABRT inside numpy C code assuming CPython
/// memory layout, or a call to an unresolved ABI symbol) are NOT catchable
/// here — run the driver under `lldb --batch -o run -o bt` to capture those.
///
/// # Safety
/// `so_path` and `name` must be valid NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cext_discovery_load(
    so_path: *const c_char,
    name: *const c_char,
) -> i32 {
    let so = match unsafe { CStr::from_ptr(so_path) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => {
            eprintln!("===MOLT_DISCOVERY_FRONTIER: so_path is not valid UTF-8");
            return -3;
        }
    };
    let nm = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => {
            eprintln!("===MOLT_DISCOVERY_FRONTIER: module name is not valid UTF-8");
            return -3;
        }
    };
    eprintln!("===MOLT_DISCOVERY: driving PyInit_{nm} from {so}");

    let res = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        molt_cpython_abi::loader::load_cpython_extension(Path::new(&so), &nm)
    }));

    match res {
        Ok(Ok(bits)) => {
            eprintln!(
                "===MOLT_DISCOVERY_OK: PyInit_{nm} returned a molt module handle bits={bits:#018x}"
            );
            0
        }
        Ok(Err(e)) => {
            eprintln!("===MOLT_DISCOVERY_FRONTIER (LoadError): {e:?}");
            eprintln!("===MOLT_DISCOVERY_FRONTIER_DISPLAY: {e}");
            // A NULL return should carry a pending exception naming the exact
            // init-time semantic frontier numpy hit. Surface it.
            unsafe { dump_pending_exception() };
            -1
        }
        Err(_) => {
            eprintln!("===MOLT_DISCOVERY_FRONTIER: Rust panic captured (backtrace above)");
            -2
        }
    }
}

/// Print the pending CPython-ABI exception (type + stringified value) — this is
/// the message that names the exact init-time semantic frontier numpy hit.
unsafe fn dump_pending_exception() {
    let occ = unsafe { errors::PyErr_Occurred() };
    if occ.is_null() {
        eprintln!(
            "===MOLT_DISCOVERY_EXC: no pending exception on NULL return (numpy bailed silently — likely a failed ABI call that did not set an exception; run under lldb to localise)"
        );
        return;
    }
    // Name the exception TYPE by resolving its pointer to the nearest exported
    // symbol (e.g. `PyExc_ImportError`) — robust regardless of molt's internal
    // exception representation, and needs no debugger.
    #[cfg(unix)]
    {
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        if unsafe { libc::dladdr(occ, &mut info) } != 0 && !info.dli_sname.is_null() {
            let sym = unsafe { CStr::from_ptr(info.dli_sname) }.to_string_lossy();
            eprintln!("===MOLT_DISCOVERY_EXC_TYPE (dladdr symbol): {sym}");
        } else {
            eprintln!(
                "===MOLT_DISCOVERY_EXC_TYPE: exception type at {occ:p} (no symbol via dladdr)"
            );
        }
    }
    #[cfg(not(unix))]
    eprintln!("===MOLT_DISCOVERY_EXC_TYPE: exception type at {occ:p}");
    // Secondary: use the canonical C type layout for its declared name.
    let tp_name_ptr = unsafe { (*occ.cast::<PyTypeObject>()).tp_name };
    if !tp_name_ptr.is_null() {
        let name = unsafe { CStr::from_ptr(tp_name_ptr) }.to_string_lossy();
        if !name.is_empty() {
            eprintln!("===MOLT_DISCOVERY_EXC_TYPE (tp_name): {name}");
        }
    }
    let mut ptype = std::ptr::null_mut();
    let mut pvalue = std::ptr::null_mut();
    let mut ptb = std::ptr::null_mut();
    unsafe { errors::PyErr_Fetch(&mut ptype, &mut pvalue, &mut ptb) };
    let type_owner = unsafe { OwnedPyObject::from_owned(ptype) };
    let value_owner = unsafe { OwnedPyObject::from_owned(pvalue) };
    let traceback_owner = unsafe { OwnedPyObject::from_owned(ptb) };
    // witness_iter consumes this summary marker independently of the
    // ABI printer. A rendering/encoding failure must not replace the original.
    let printed = !pvalue.is_null()
        && errors::with_preserved_error(|| unsafe {
            let rendered = OwnedPyObject::from_owned(typeobj::PyObject_Str(pvalue));
            if rendered.as_ptr().is_null() {
                return false;
            }
            let utf8 = strings::PyUnicode_AsUTF8(rendered.as_ptr());
            if utf8.is_null() {
                return false;
            }
            let message = CStr::from_ptr(utf8).to_string_lossy();
            eprintln!("===MOLT_DISCOVERY_EXC: pending exception value = {message:?}");
            true
        });
    if !printed {
        eprintln!(
            "===MOLT_DISCOVERY_EXC: pending exception present (type ptr={ptype:p}, value ptr={pvalue:p}); could not stringify via PyObject_Str/PyUnicode_AsUTF8"
        );
    }
    // An allocation-free emergency indicator deliberately yields no Fetch
    // outputs. Preserve that indicator; otherwise hand the exact triple back
    // to the ABI printer, including its original traceback and type.
    if !ptype.is_null() || !pvalue.is_null() || !ptb.is_null() {
        unsafe {
            errors::PyErr_Restore(
                type_owner.into_ptr(),
                value_owner.into_ptr(),
                traceback_owner.into_ptr(),
            )
        };
    }
    eprintln!("===MOLT_DISCOVERY_EXC_PRINT (PyErr_Print):");
    unsafe { errors::PyErr_Print() };
}

#[cfg(test)]
#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[cfg(test)]
mod captured_runtime_children {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/captured_runtime_children.rs"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::PyExc_ValueError;
    use molt_cpython_abi::api::sequences;

    #[test]
    fn pending_diagnostic_prints_and_retires_original_error_after_summary_encoding_failure() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "tests::pending_diagnostic_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]);
        let output = captured_runtime_children::capture(
            &mut command,
            "discovery-pending-diagnostic",
            "render-and-drain",
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "discovery child failed: {stderr}");
        for expected in [
            "[molt-cpython-abi] PyErr_Print: discovery exception custody\n",
            "[molt-cpython-abi] PyErr_Print: \\ud800\n",
            "===MOLT_DISCOVERY_EXC: pending exception value = \"discovery exception custody\"\n",
            "===MOLT_DISCOVERY_EXC: no pending exception on NULL return",
        ] {
            assert!(stderr.contains(expected), "missing {expected:?}: {stderr}");
        }
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("discovery diagnostic final drain verified\n")
        );
    }

    #[test]
    #[ignore = "executed by the captured discovery diagnostic owner"]
    fn pending_diagnostic_child() {
        assert_eq!(molt_runtime::lifecycle::init(), 1);
        assert!(molt_runtime::cpython_abi_hooks::register_cpython_hooks());
        molt_runtime::lifecycle::with_ready_execution(|| unsafe {
            // Ordinary text exercises the machine-readable summary. The
            // lone surrogate rejects its strict UTF-8 path, while the ABI
            // printer must still consume the original ValueError safely.
            for surrogate in [false, true] {
                let args = OwnedPyObject::from_owned(sequences::PyTuple_New(1));
                assert!(!args.as_ptr().is_null());
                let text = OwnedPyObject::from_owned(if surrogate {
                    strings::PyUnicode_FromOrdinal(0xd800)
                } else {
                    strings::PyUnicode_FromString(c"discovery exception custody".as_ptr())
                });
                assert!(!text.as_ptr().is_null());
                assert_eq!(
                    sequences::PyTuple_SetItem(args.as_ptr(), 0, text.into_ptr()),
                    0
                );
                let error = OwnedPyObject::from_owned(errors::molt_native_exception_new(
                    &raw mut PyExc_ValueError,
                    args.as_ptr(),
                    std::ptr::null_mut(),
                ));
                assert!(!error.as_ptr().is_null());
                errors::PyErr_SetRaisedException(error.into_ptr());
                dump_pending_exception();
                assert!(errors::PyErr_Occurred().is_null());
            }
            dump_pending_exception();
            assert!(errors::PyErr_Occurred().is_null());
        })
        .expect("initialized discovery runtime must admit execution");
        // Use the real embedding drain: sys.last_* can legitimately retain
        // the printed value, but no unowned Fetch/rendering reference can
        // survive final native/class retirement. Make no calls after shutdown.
        assert_eq!(molt_runtime::lifecycle::shutdown(), 1);
        println!("discovery diagnostic final drain verified");
    }
}
