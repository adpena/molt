//! Native CPython-ABI *frontier reproduction* harness.
//!
//! # Why this file exists
//!
//! Every runtime-semantic frontier that the numpy/scipy **wasm witness** hits
//! (a silent `-1`, a wrong answer, a panic, a trap) historically cost a **~20–30
//! minute** wasm build+run E2E to discover and another full cycle per fix
//! iteration. Yet the divergent code lives entirely in
//! `runtime/molt-cpython-abi/` — **platform-independent Rust**. That means the
//! *same* divergence can be reproduced as a plain `cargo test` in **seconds**,
//! with a real backtrace, a debugger, and sanitizers — no wasm, no node, no
//! meson seal.
//!
//! This harness turns the [CPython-ABI Divergence Ledger]
//! (`docs/agent/CPYTHON_ABI_DIVERGENCE_LEDGER.md`) into *executable* native
//! reproductions. Each `frontier_*` test asserts the **CPython 3.12–correct**
//! behavior and is `#[ignore]`d **only** because the fix has not landed yet:
//!
//!   * default `cargo test` skips them → **gates stay green**;
//!   * `cargo test -p molt-lang-cpython-abi --test frontier_repro -- --ignored`
//!     runs them → each **fails loudly with a real backtrace in < 1 s** — that
//!     failure *is* the frontier reproduction the witness used to take 30 min to
//!     surface;
//!   * when a frontier is fixed, delete its one `#[ignore]` line and the test
//!     becomes a **permanent regression guard**.
//!
//! Drive the whole loop with `tools/fast_frontier_cycle.py` (see
//! `docs/agent/FAST_FRONTIER_LOOP.md`).
//!
//! # Adding a new frontier
//!
//! Copy an existing `frontier_*` fn: call the ABI entrypoint the way numpy's C
//! code does, then `assert` the CPython-correct answer. Keep it hook-free where
//! possible (inline ints / raw pointers need no runtime); reach for
//! [`install_min_hooks`] only when you must materialize/read back a str/bytes.

#![allow(non_snake_case)]

use molt_cpython_abi::hooks::RuntimeHooks;
use std::collections::HashMap;
use std::sync::{Mutex, Once};

// ─────────────────────────────────────────────────────────────────────────────
// Minimal fake runtime backend
//
// The ABI is deliberately runtime-agnostic: object allocation is injected via
// the `RuntimeHooks` vtable at load time. Inline ints and raw pointers need no
// hooks, but any frontier that materializes a *str* (repr/str/format paths)
// needs a working `alloc_str`/`str_data` pair to read the result back. We supply
// the smallest possible one — a content-addressed leaked-bytes arena — so these
// tests never depend on the (heavy) full `molt-runtime` crate.
// ─────────────────────────────────────────────────────────────────────────────

static STR_ARENA: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static NEXT_STR_HANDLE: Mutex<u64> = Mutex::new(0x4000_0000);

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    // Store the payload with a trailing NUL, matching CPython's
    // `PyUnicode_AsUTF8` contract (it returns a NUL-terminated C buffer). The
    // recorded length excludes the NUL.
    let payload: &[u8] = if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let mut owned = payload.to_vec();
    owned.push(0);
    // Leak so the pointer handed back through `str_data` stays valid for the
    // whole test process (these tests are short-lived; no reclamation).
    let leaked: &'static [u8] = Box::leak(owned.into_boxed_slice());
    let mut next = NEXT_STR_HANDLE.lock().unwrap();
    let handle = *next;
    *next += 0x10;
    drop(next);
    let mut arena = STR_ARENA.lock().unwrap();
    arena
        .get_or_insert_with(HashMap::new)
        .insert(handle, leaked);
    handle
}

unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let arena = STR_ARENA.lock().unwrap();
    if let Some(bytes) = arena.as_ref().and_then(|m| m.get(&bits)) {
        if !out_len.is_null() {
            // Reported length excludes the trailing NUL byte we stored.
            unsafe { *out_len = bytes.len().saturating_sub(1) };
        }
        bytes.as_ptr()
    } else {
        if !out_len.is_null() {
            unsafe { *out_len = 0 };
        }
        std::ptr::null()
    }
}

static INIT: Once = Once::new();

/// Initialize the ABI and install the minimal str-materializing hook set.
/// Idempotent; safe for every test in this binary to call.
fn install_min_hooks() {
    INIT.call_once(|| {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
        hooks.alloc_str = fake_alloc_str;
        hooks.str_data = fake_str_data;
        // Fresh OnceLock in this dedicated test binary — installs cleanly.
        unsafe {
            molt_cpython_abi::try_set_runtime_hooks(hooks);
        }
    });
}

/// Read a bridge-minted `str` PyObject back to an owned `String`.
unsafe fn read_pystr(op: *mut molt_cpython_abi::abi_types::PyObject) -> String {
    let p = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(op) };
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

// ═════════════════════════════════════════════════════════════════════════════
// GREEN control — proves the harness actually drives real ABI code.
// This one is NOT ignored: it runs in the default gate and would catch a
// regression in the reproduction plumbing (or an accidental fix flip).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn harness_drives_real_abi_code() {
    install_min_hooks();
    // A value inside Molt's inline-int range round-trips correctly today — this
    // is the "the loop is live" sanity check. If this ever breaks, the harness
    // (not a frontier) is broken.
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1_000) };
    assert!(!py.is_null(), "PyLong_FromLong minted a null int");
    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(got, 1_000, "harness cannot round-trip an in-range int");
}

// ═════════════════════════════════════════════════════════════════════════════
// FRONTIER #8 — PyLong_AsLong silent truncation + missing OverflowError
//   Ledger: numbers.rs:424  [H] (np)  SILENT_SENTINEL
//
//   `PyLong_AsLong(op) = py_long_as_i64(op) as c_long`. On every platform where
//   `long` is 32-bit (wasm32, Windows/LLP64) a value above LONG_MAX is silently
//   truncated and, critically, **no exception is set**. CPython raises
//   OverflowError and returns -1, so a C caller using the canonical
//   `x == -1 && PyErr_Occurred()` idiom treats Molt's truncated value as valid —
//   a silent wrong shape/stride/index on numpy's array-construction path.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "FRONTIER: reproduces divergence-ledger #8 (numbers.rs PyLong_AsLong \
            silent truncation, no OverflowError). Delete this #[ignore] once fixed \
            to convert into a regression guard."]
fn frontier_08_pylong_aslong_silent_overflow() {
    install_min_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // 2**31 + 5 — above LONG_MAX on any 32-bit-long platform (wasm32, Windows).
    const BIG: std::os::raw::c_longlong = 2_147_483_653;
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(BIG) };
    assert!(!py.is_null());

    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    let err = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };

    // CPython 3.12 contract on a 32-bit `long` platform: return -1 AND set
    // OverflowError. Molt returns a silently-truncated value and sets nothing.
    eprintln!(
        "FRONTIER #8 REPRODUCED: PyLong_AsLong(2**31+5) -> {got} (err_set={}), \
         CPython -> -1 with OverflowError set",
        !err.is_null()
    );
    assert!(
        !err.is_null(),
        "PyLong_AsLong overflow must set an exception (CPython raises OverflowError); \
         got silent value {got} with no error — silent wrong answer on numpy's \
         shape/stride/index path"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// FRONTIER #6 — PyObject_Str / PyObject_Repr theater
//   Ledger: typeobj.rs:1916  [H] (np)  THEATER
//
//   `PyObject_Repr` ignores its argument and unconditionally returns the literal
//   `"<molt object>"`; `PyObject_Str` delegates to it. So `str(x)`/`repr(x)` of
//   *every* object is corrupted, and because this backs `%S` in PyErr_Format /
//   PyUnicode_FromFormat, numpy's error messages and dtype/array string paths
//   are all wrong.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "FRONTIER: reproduces divergence-ledger #6 (typeobj.rs PyObject_Str/Repr \
            returns the literal '<molt object>'). Delete this #[ignore] once fixed \
            to convert into a regression guard."]
fn frontier_06_pyobject_str_theater() {
    install_min_hooks();

    // str(2147483653) must be its decimal digits, exactly as CPython.
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(2_147_483_653) };
    assert!(!py.is_null());
    let s_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(!s_obj.is_null(), "PyObject_Str returned NULL");
    let s = unsafe { read_pystr(s_obj) };

    eprintln!("FRONTIER #6 REPRODUCED: PyObject_Str(int) -> {s:?}, CPython -> \"2147483653\"");
    assert_eq!(
        s, "2147483653",
        "PyObject_Str must dispatch tp_str, not return the '<molt object>' theater string"
    );
}
