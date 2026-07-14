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

unsafe extern "C" {
    fn PyErr_Occurred() -> *mut std::os::raw::c_void;
    fn PyErr_Fetch(
        ptype: *mut *mut std::os::raw::c_void,
        pvalue: *mut *mut std::os::raw::c_void,
        ptraceback: *mut *mut std::os::raw::c_void,
    );
    fn PyErr_Print();
    fn PyObject_Str(o: *mut std::os::raw::c_void) -> *mut std::os::raw::c_void;
    fn PyUnicode_AsUTF8(o: *mut std::os::raw::c_void) -> *const c_char;
}

/// Print the pending CPython-ABI exception (type + stringified value) — this is
/// the message that names the exact init-time semantic frontier numpy hit.
unsafe fn dump_pending_exception() {
    let occ = unsafe { PyErr_Occurred() };
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
    // Secondary: try tp_name at the CPython PyTypeObject offset (24).
    let tp_name_ptr = unsafe { *(occ.cast::<u8>().add(24) as *const *const c_char) };
    if !tp_name_ptr.is_null() {
        let name = unsafe { CStr::from_ptr(tp_name_ptr) }.to_string_lossy();
        if !name.is_empty() {
            eprintln!("===MOLT_DISCOVERY_EXC_TYPE (tp_name): {name}");
        }
    }
    let mut ptype = std::ptr::null_mut();
    let mut pvalue = std::ptr::null_mut();
    let mut ptb = std::ptr::null_mut();
    unsafe { PyErr_Fetch(&mut ptype, &mut pvalue, &mut ptb) };
    let mut printed = false;
    if !pvalue.is_null() {
        let s = unsafe { PyObject_Str(pvalue) };
        if !s.is_null() {
            let utf8 = unsafe { PyUnicode_AsUTF8(s) };
            if !utf8.is_null() {
                let msg = unsafe { CStr::from_ptr(utf8) }
                    .to_string_lossy()
                    .into_owned();
                eprintln!("===MOLT_DISCOVERY_EXC: pending exception value = {msg:?}");
                printed = true;
            }
        }
    }
    if !printed {
        eprintln!(
            "===MOLT_DISCOVERY_EXC: pending exception present (type ptr={ptype:p}, value ptr={pvalue:p}); could not stringify via PyObject_Str/PyUnicode_AsUTF8"
        );
    }
    // Restore + let the ABI's own printer render it (traceback etc.).
    // (PyErr_Fetch cleared it; re-raise so PyErr_Print has something to show.)
    eprintln!("===MOLT_DISCOVERY_EXC_PRINT (PyErr_Print):");
    // Best-effort: molt's PyErr_Print may route to its own sys.stderr.
    unsafe { PyErr_Print() };
}
