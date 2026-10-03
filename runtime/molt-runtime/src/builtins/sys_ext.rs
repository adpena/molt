// === FILE: runtime/molt-runtime/src/builtins/sys_ext.rs ===
//
// Additional sys intrinsics for CPython 3.12+ parity.
// These supplement the existing sys intrinsics in object/ops.rs, io.rs, and platform.rs.
//
// No capability gates needed: these intrinsics return process metadata and
// language-level constants that are always available.

use crate::builtins::numbers::{index_c_int_from_obj, int_bits_from_i64};
use crate::object::ops_hash::{
    PY_HASH_ALGORITHM, PY_HASH_ALGORITHM_BITS, PY_HASH_CUTOFF, PY_HASH_IMAG, PY_HASH_INF,
    PY_HASH_MODULUS, PY_HASH_NAN, PY_HASH_SEED_BITS, PY_HASH_WIDTH,
};
use crate::object::ops_sys::sys_tuple_from_owned;
use crate::state::runtime_state::{RuntimeState, runtime_state};
use crate::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering as AtomicOrdering};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate a runtime string from a Rust &str slice, returning bits.
/// Returns None bits on allocation failure.
#[inline]
fn str_bits(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        crate::record_memory_error_without_allocation(_py);
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[derive(Clone, Copy)]
struct SysTraceProfileState {
    trace_bits: u64,
    profile_bits: u64,
}

impl SysTraceProfileState {
    fn new() -> Self {
        Self {
            trace_bits: MoltObject::none().bits(),
            profile_bits: MoltObject::none().bits(),
        }
    }
}

pub(crate) struct SysRuntimeState {
    trace_profile: Mutex<SysTraceProfileState>,
    switch_interval_bits: AtomicU64,
    int_max_str_digits: AtomicI64,
    audit_hooks: Mutex<Vec<u64>>,
}

impl SysRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            trace_profile: Mutex::new(SysTraceProfileState::new()),
            switch_interval_bits: AtomicU64::new(DEFAULT_SWITCH_INTERVAL_BITS),
            int_max_str_digits: AtomicI64::new(DEFAULT_INT_MAX_STR_DIGITS),
            audit_hooks: Mutex::new(Vec::new()),
        }
    }
}

fn sys_state(_py: &PyToken<'_>) -> &'static SysRuntimeState {
    &runtime_state(_py).sys_ext
}

pub(crate) fn sys_ext_clear_state(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let (trace_bits, profile_bits) = {
        let mut trace_profile = state
            .sys_ext
            .trace_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let trace_bits = trace_profile.trace_bits;
        let profile_bits = trace_profile.profile_bits;
        *trace_profile = SysTraceProfileState::new();
        (trace_bits, profile_bits)
    };
    let switch_interval_bits = state
        .sys_ext
        .switch_interval_bits
        .swap(DEFAULT_SWITCH_INTERVAL_BITS, AtomicOrdering::Relaxed);
    let int_max_str_digits = state
        .sys_ext
        .int_max_str_digits
        .swap(DEFAULT_INT_MAX_STR_DIGITS, AtomicOrdering::Relaxed);
    let audit_hooks = {
        let mut hooks = state
            .sys_ext
            .audit_hooks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *hooks)
    };
    let changed = (trace_bits != 0 && !obj_from_bits(trace_bits).is_none())
        || (profile_bits != 0 && !obj_from_bits(profile_bits).is_none())
        || !audit_hooks.is_empty()
        || switch_interval_bits != DEFAULT_SWITCH_INTERVAL_BITS
        || int_max_str_digits != DEFAULT_INT_MAX_STR_DIGITS;

    dec_ref_sys_owned_bits(_py, trace_bits);
    dec_ref_sys_owned_bits(_py, profile_bits);
    for bits in audit_hooks {
        dec_ref_sys_owned_bits(_py, bits);
    }
    changed
}

fn dec_ref_sys_owned_bits(_py: &PyToken<'_>, bits: u64) {
    if bits != 0 && !obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
    }
}

fn ensure_trace_or_profile_callable(
    _py: &PyToken<'_>,
    value_bits: u64,
    api_name: &str,
) -> Result<(), u64> {
    if obj_from_bits(value_bits).is_none() {
        return Ok(());
    }
    let is_callable = is_truthy(_py, obj_from_bits(molt_is_callable(value_bits)));
    if !is_callable {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{api_name}() argument must be callable"),
        ));
    }
    Ok(())
}

fn replace_optional_callable(_py: &PyToken<'_>, target: &mut u64, value_bits: u64) -> Option<u64> {
    if *target == value_bits {
        return None;
    }
    if !obj_from_bits(value_bits).is_none() {
        inc_ref_bits(_py, value_bits);
    }
    let old_bits = *target;
    let old_owned = (!obj_from_bits(old_bits).is_none()).then_some(old_bits);
    *target = value_bits;
    old_owned
}

fn release_replaced_optional_callable(_py: &PyToken<'_>, old_bits: Option<u64>) {
    if let Some(bits) = old_bits {
        dec_ref_bits(_py, bits);
    }
}

fn clone_optional_callable(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if !obj_from_bits(value_bits).is_none() {
        inc_ref_bits(_py, value_bits);
    }
    value_bits
}

fn pin_owned_bits(_py: &PyToken<'_>, bits: u64) {
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
}

fn release_owned_bits(_py: &PyToken<'_>, bits: u64) {
    if bits != 0 && !obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
    }
}

// ---------------------------------------------------------------------------
// 1. Scalar constants
// ---------------------------------------------------------------------------

/// `sys.maxsize`: `PY_SSIZE_T_MAX` of the target C data model, the same
/// `Py_ssize_t` the C-API layer and builtin `sum()` use.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_maxsize() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        int_bits_from_i64(_py, molt_cpython_abi::Py_ssize_t::MAX as i64)
    })
}

/// `sys.maxunicode` -> 0x10FFFF (Unicode max code point)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_maxunicode() -> u64 {
    MoltObject::from_int(0x10FFFF).bits()
}

/// `sys.byteorder` -> "little" or "big"
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_byteorder() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let order = if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        };
        str_bits(_py, order)
    })
}

// ---------------------------------------------------------------------------
// 2. Path / prefix constants
// ---------------------------------------------------------------------------

/// `sys.prefix` -> installation prefix path
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_prefix() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Molt compiled binaries are self-contained; prefix is the binary's parent directory
        let prefix = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_default();
        str_bits(_py, &prefix)
    })
}

/// `sys.exec_prefix` -> same as prefix for Molt
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_exec_prefix() -> u64 {
    molt_sys_prefix()
}

/// `sys.base_prefix` -> same as prefix for Molt
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_base_prefix() -> u64 {
    molt_sys_prefix()
}

/// `sys.base_exec_prefix` -> same as prefix for Molt
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_base_exec_prefix() -> u64 {
    molt_sys_prefix()
}

/// Platform library directory name for `sys.platlibdir`.
fn platlibdir_name() -> &'static str {
    #[cfg(windows)]
    {
        "DLLs"
    }
    #[cfg(not(windows))]
    {
        "lib"
    }
}

/// `sys.platlibdir` -> platform library directory name
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_platlibdir() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { str_bits(_py, platlibdir_name()) })
}

// ---------------------------------------------------------------------------
// 3. Structured info tuples
// ---------------------------------------------------------------------------

/// `sys.float_info` -> 11-element tuple of f64 system constants
/// Fields: max, max_exp, max_10_exp, min, min_exp, min_10_exp, dig, mant_dig, epsilon, radix, rounds
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_float_info() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let values: [u64; 11] = [
            MoltObject::from_float(f64::MAX).bits(),
            MoltObject::from_int(f64::MAX_EXP as i64).bits(),
            MoltObject::from_int(f64::MAX_10_EXP as i64).bits(),
            MoltObject::from_float(f64::MIN_POSITIVE).bits(),
            MoltObject::from_int(f64::MIN_EXP as i64).bits(),
            MoltObject::from_int(f64::MIN_10_EXP as i64).bits(),
            MoltObject::from_int(f64::DIGITS as i64).bits(),
            MoltObject::from_int(f64::MANTISSA_DIGITS as i64).bits(),
            MoltObject::from_float(f64::EPSILON).bits(),
            MoltObject::from_int(f64::RADIX as i64).bits(),
            MoltObject::from_int(1).bits(), // FLT_ROUNDS: 1 = round to nearest
        ];
        sys_tuple_from_owned(_py, &values)
    })
}

/// `sys.int_info` -> 4-element tuple
/// Fields: bits_per_digit, sizeof_digit, default_max_str_digits, str_digits_check_threshold
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_int_info() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Molt uses NaN-boxed 47-bit inline ints; for API compat report CPython-compatible values
        let values: [u64; 4] = [
            MoltObject::from_int(30).bits(), // bits_per_digit (CPython default)
            MoltObject::from_int(4).bits(),  // sizeof_digit (4 bytes = uint32)
            MoltObject::from_int(DEFAULT_INT_MAX_STR_DIGITS).bits(),
            MoltObject::from_int(INT_STR_DIGITS_CHECK_THRESHOLD).bits(),
        ];
        sys_tuple_from_owned(_py, &values)
    })
}

/// `sys.hash_info` -> 9-element tuple
/// Fields: width, modulus, inf, nan, imag, algorithm, hash_bits, seed_bits, cutoff
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_hash_info() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let alg_bits = str_bits(_py, PY_HASH_ALGORITHM);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let values: [u64; 9] = [
            MoltObject::from_int(PY_HASH_WIDTH as i64).bits(),
            int_bits_from_i64(_py, PY_HASH_MODULUS as i64),
            MoltObject::from_int(PY_HASH_INF).bits(),
            MoltObject::from_int(PY_HASH_NAN).bits(),
            MoltObject::from_int(PY_HASH_IMAG).bits(),
            alg_bits,
            MoltObject::from_int(PY_HASH_ALGORITHM_BITS as i64).bits(),
            MoltObject::from_int(PY_HASH_SEED_BITS as i64).bits(),
            MoltObject::from_int(PY_HASH_CUTOFF as i64).bits(),
        ];
        sys_tuple_from_owned(_py, &values)
    })
}

/// `sys.thread_info` -> 3-element tuple (name, lock, version)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_thread_info() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_str = if cfg!(target_os = "windows") {
            "nt"
        } else if cfg!(target_arch = "wasm32") {
            "wasm"
        } else {
            // linux, macos, and other POSIX platforms
            "pthread"
        };
        let name_bits = str_bits(_py, name_str);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let lock_bits = str_bits(_py, "mutex+cond");
        let values: [u64; 3] = [
            name_bits,
            lock_bits,
            MoltObject::none().bits(), // version (None = unknown)
        ];
        sys_tuple_from_owned(_py, &values)
    })
}

// ---------------------------------------------------------------------------
// 4. Functions
// ---------------------------------------------------------------------------

/// `sys.is_finalizing()` -> `False` for active compiled execution.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_is_finalizing() -> u64 {
    MoltObject::from_bool(false).bits()
}

/// `sys.getrefcount(obj)` -> best-effort runtime refcount, including call arg ref.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getrefcount(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let count = if let Some(ptr) = obj.as_ptr() {
            let header = unsafe { header_from_obj_ptr(ptr) };
            let rc = unsafe { (*header).ref_count_snapshot() } as i64;
            rc.saturating_add(1)
        } else {
            1
        };
        int_bits_from_i64(_py, count)
    })
}

/// `sys.settrace(tracefunc)` -> store process-level trace hook (or None).
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_settrace(tracefunc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = ensure_trace_or_profile_callable(_py, tracefunc_bits, "settrace") {
            return err;
        }
        let old_bits = {
            let mut trace_profile = sys_state(_py)
                .trace_profile
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            replace_optional_callable(_py, &mut trace_profile.trace_bits, tracefunc_bits)
        };
        release_replaced_optional_callable(_py, old_bits);
        MoltObject::none().bits()
    })
}

/// `sys.gettrace()` -> current process-level trace hook.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_gettrace() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_profile = sys_state(_py)
            .trace_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clone_optional_callable(_py, trace_profile.trace_bits)
    })
}

/// `sys.setprofile(profilefunc)` -> store process-level profile hook (or None).
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_setprofile(profilefunc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = ensure_trace_or_profile_callable(_py, profilefunc_bits, "setprofile") {
            return err;
        }
        let old_bits = {
            let mut trace_profile = sys_state(_py)
                .trace_profile
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            replace_optional_callable(_py, &mut trace_profile.profile_bits, profilefunc_bits)
        };
        release_replaced_optional_callable(_py, old_bits);
        MoltObject::none().bits()
    })
}

/// `sys.getprofile()` -> current process-level profile hook.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getprofile() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_profile = sys_state(_py)
            .trace_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clone_optional_callable(_py, trace_profile.profile_bits)
    })
}

/// `sys.intern(string)` -> interned string
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_intern(s_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let s = match string_obj_to_owned(obj_from_bits(s_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "intern() argument 1 must be str, not other type",
                );
            }
        };
        let ptr = crate::object::builders::alloc_interned_string(_py, s.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `sys.getsizeof(object, default)` -> approximate size in bytes
///
/// Returns CPython-compatible approximate sizes for built-in types.
/// For heap-allocated containers, the size scales with element count.
/// The `default` parameter is returned if the object's `__sizeof__` would
/// raise a TypeError (CPython semantics); Molt's NaN-boxed model never
/// raises here, so `default` is effectively unused but accepted for API
/// compatibility.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getsizeof(obj_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = default_bits; // accepted for API compat; Molt never raises TypeError here
        let obj = obj_from_bits(obj_bits);

        // Inline NaN-boxed values
        if obj.is_none() || obj.is_bool() {
            return MoltObject::from_int(16).bits();
        }
        if obj.is_int() {
            return MoltObject::from_int(28).bits(); // CPython int: 28 bytes
        }
        if obj.is_float() {
            return MoltObject::from_int(24).bits(); // CPython float: 24 bytes
        }

        // Heap-allocated objects — dispatch on type_id
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::from_int(8).bits(); // unknown inline tag
        };
        let type_id = unsafe { object_type_id(ptr) };
        let size: i64 = match type_id {
            TYPE_ID_STRING => {
                let len = unsafe { string_len(ptr) } as i64;
                49 + len + 1 // CPython compact-ASCII str: ~49 + len + NUL
            }
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let len = unsafe { bytes_len(ptr) } as i64;
                33 + len // CPython bytes: ~33 + len
            }
            TYPE_ID_LIST | TYPE_ID_LIST_BUILDER => {
                let len = unsafe { crate::builtins::containers::list_len(ptr) } as i64;
                56 + len * 8 // CPython list: 56 + 8 per element slot
            }
            TYPE_ID_TUPLE => {
                let len = unsafe { crate::builtins::containers::tuple_len(ptr) } as i64;
                40 + len * 8 // CPython tuple: 40 + 8 per element
            }
            TYPE_ID_DICT => {
                let len = unsafe { crate::builtins::containers::dict_len(ptr) } as i64;
                64 + len * 3 * 8 // CPython dict: ~64 + 3*8 per entry (hash, key, value)
            }
            TYPE_ID_SET | TYPE_ID_FROZENSET => {
                let len = unsafe { crate::builtins::containers::set_len(ptr) } as i64;
                200 + len * 8 // CPython set: ~200 + 8 per entry
            }
            TYPE_ID_RANGE => 48,        // CPython range: 48 bytes
            TYPE_ID_SLICE => 56,        // CPython slice: 56 bytes
            TYPE_ID_FUNCTION => 136,    // CPython function: ~136 bytes
            TYPE_ID_BOUND_METHOD => 48, // CPython bound method: ~48 bytes
            TYPE_ID_MODULE => 72,       // CPython module: ~72 bytes
            TYPE_ID_TYPE => 864,        // CPython type: ~864 bytes
            TYPE_ID_COMPLEX => 32,      // CPython complex: 32 bytes
            TYPE_ID_EXCEPTION => unsafe {
                // Runtime exceptions have one compact common prefix plus the exact
                // schema-owned typed tail selected from their real class MRO.
                (std::mem::size_of::<MoltHeader>()
                    + crate::builtins::exceptions::exception_payload_words(ptr)
                        * std::mem::size_of::<u64>()) as i64
            },
            TYPE_ID_BIGINT => 32, // approximation for arbitrary-precision int
            TYPE_ID_CODE => 176,  // CPython code object: ~176 bytes
            _ => 64,              // reasonable default for other heap objects
        };
        int_bits_from_i64(_py, size)
    })
}

// ---------------------------------------------------------------------------
// 5. Module name lists
// ---------------------------------------------------------------------------

/// `sys.stdlib_module_names` -> tuple of stdlib module names
/// Python wrapper converts to frozenset.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdlib_module_names() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let names: &[&str] = &[
            "__future__",
            "_abc",
            "_asyncio",
            "_bisect",
            "_codecs",
            "_collections",
            "_collections_abc",
            "_csv",
            "_datetime",
            "_decimal",
            "_functools",
            "_heapq",
            "_io",
            "_json",
            "_operator",
            "_pickle",
            "_random",
            "_signal",
            "_socket",
            "_sqlite3",
            "_sre",
            "_stat",
            "_statistics",
            "_string",
            "_struct",
            "_thread",
            "_threading_local",
            "_tracemalloc",
            "_weakref",
            "_weakrefset",
            "abc",
            "argparse",
            "ast",
            "asyncio",
            "atexit",
            "base64",
            "binascii",
            "bisect",
            "builtins",
            "calendar",
            "codecs",
            "collections",
            "colorsys",
            "compileall",
            "concurrent",
            "configparser",
            "contextlib",
            "contextvars",
            "copy",
            "copyreg",
            "csv",
            "ctypes",
            "dataclasses",
            "datetime",
            "dbm",
            "decimal",
            "difflib",
            "dis",
            "email",
            "enum",
            "errno",
            "faulthandler",
            "fnmatch",
            "fractions",
            "ftplib",
            "functools",
            "gc",
            "getopt",
            "getpass",
            "glob",
            "graphlib",
            "gzip",
            "hashlib",
            "heapq",
            "hmac",
            "html",
            "http",
            "idlelib",
            "imaplib",
            "importlib",
            "inspect",
            "io",
            "ipaddress",
            "itertools",
            "json",
            "keyword",
            "linecache",
            "locale",
            "logging",
            "lzma",
            "mailbox",
            "marshal",
            "math",
            "mimetypes",
            "multiprocessing",
            "netrc",
            "numbers",
            "operator",
            "os",
            "pathlib",
            "pdb",
            "pickle",
            "pkgutil",
            "platform",
            "plistlib",
            "poplib",
            "posixpath",
            "pprint",
            "profile",
            "pstats",
            "py_compile",
            "pydoc",
            "queue",
            "quopri",
            "random",
            "re",
            "reprlib",
            "resource",
            "rlcompleter",
            "runpy",
            "sched",
            "secrets",
            "select",
            "selectors",
            "shelve",
            "shlex",
            "shutil",
            "signal",
            "site",
            "smtplib",
            "socket",
            "socketserver",
            "sqlite3",
            "ssl",
            "stat",
            "statistics",
            "string",
            "stringprep",
            "struct",
            "subprocess",
            "sys",
            "sysconfig",
            "tarfile",
            "tempfile",
            "test",
            "textwrap",
            "threading",
            "time",
            "timeit",
            "token",
            "tokenize",
            "tomllib",
            "trace",
            "traceback",
            "tracemalloc",
            "types",
            "typing",
            "unicodedata",
            "unittest",
            "urllib",
            "uuid",
            "venv",
            "warnings",
            "wave",
            "weakref",
            "webbrowser",
            "xml",
            "xmlrpc",
            "zipapp",
            "zipfile",
            "zipimport",
            "zlib",
            "zoneinfo",
        ];
        let mut bits_vec: Vec<u64> = Vec::with_capacity(names.len());
        for &name in names {
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                for &b in &bits_vec {
                    dec_ref_bits(_py, b);
                }
                return MoltObject::none().bits();
            }
            bits_vec.push(MoltObject::from_ptr(ptr).bits());
        }
        sys_tuple_from_owned(_py, &bits_vec)
    })
}

/// `sys.builtin_module_names` -> tuple of built-in module names
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_builtin_module_names() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let names: &[&str] = &[
            "_abc",
            "_ast",
            "_codecs",
            "_collections",
            "_functools",
            "_io",
            "_operator",
            "_signal",
            "_sre",
            "_stat",
            "_string",
            "_thread",
            "_tracemalloc",
            "_warnings",
            "_weakref",
            "atexit",
            "builtins",
            "errno",
            "faulthandler",
            "gc",
            "itertools",
            "marshal",
            "posix",
            "sys",
            "time",
        ];
        let mut bits_vec: Vec<u64> = Vec::with_capacity(names.len());
        for &name in names {
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                for &b in &bits_vec {
                    dec_ref_bits(_py, b);
                }
                return MoltObject::none().bits();
            }
            bits_vec.push(MoltObject::from_ptr(ptr).bits());
        }
        sys_tuple_from_owned(_py, &bits_vec)
    })
}

// ---------------------------------------------------------------------------
// 6. Process info
// ---------------------------------------------------------------------------

/// `sys.orig_argv` -> original argv from process start
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_orig_argv() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args: Vec<String> = std::env::args().collect();
        let mut bits_vec: Vec<u64> = Vec::with_capacity(args.len());
        for arg in &args {
            let ptr = alloc_string(_py, arg.as_bytes());
            if ptr.is_null() {
                for &b in &bits_vec {
                    dec_ref_bits(_py, b);
                }
                return MoltObject::none().bits();
            }
            bits_vec.push(MoltObject::from_ptr(ptr).bits());
        }
        let ptr = alloc_list(_py, &bits_vec);
        // alloc_list inc_refs each element, so dec_ref our locals
        for &b in &bits_vec {
            dec_ref_bits(_py, b);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `sys.copyright` -> Molt copyright string
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_copyright() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text = "Copyright (c) Molt contributors.\nAll Rights Reserved.\n\nCopyright (c) 2001-2024 Python Software Foundation.\nAll Rights Reserved.";
        str_bits(_py, text)
    })
}

// ---------------------------------------------------------------------------
// 7. Additional sys intrinsics for full intrinsic-backing
// ---------------------------------------------------------------------------

/// `sys.getdefaultencoding()` -> "utf-8"
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getdefaultencoding() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { str_bits(_py, "utf-8") })
}

/// `sys.getfilesystemencoding()` -> "utf-8"
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getfilesystemencoding() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { str_bits(_py, "utf-8") })
}

// --- Thread switch interval (GIL timeslice stub) ---

const DEFAULT_SWITCH_INTERVAL_BITS: u64 = 0.005f64.to_bits();

/// `sys.getswitchinterval()` -> float seconds
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getswitchinterval() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = sys_state(_py)
            .switch_interval_bits
            .load(AtomicOrdering::Relaxed);
        MoltObject::from_float(f64::from_bits(bits)).bits()
    })
}

/// `sys.setswitchinterval(interval)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_setswitchinterval(interval_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(interval_bits);
        let val = match to_f64(obj) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "a float is required");
            }
        };
        if val <= 0.0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "switch interval must be strictly positive",
            );
        }
        sys_state(_py)
            .switch_interval_bits
            .store(val.to_bits(), AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    })
}

// --- Integer string conversion length limitation ---

const DEFAULT_INT_MAX_STR_DIGITS: i64 = 4300;
const INT_STR_DIGITS_CHECK_THRESHOLD: i64 = 640;

pub(crate) fn current_int_max_str_digits(_py: &PyToken<'_>) -> usize {
    sys_state(_py)
        .int_max_str_digits
        .load(AtomicOrdering::Relaxed)
        .max(0) as usize
}

/// `sys.get_int_max_str_digits()` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_get_int_max_str_digits() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let val = sys_state(_py)
            .int_max_str_digits
            .load(AtomicOrdering::Relaxed);
        int_bits_from_i64(_py, val)
    })
}

/// `sys.set_int_max_str_digits(maxdigits)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_set_int_max_str_digits(maxdigits_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = index_c_int_from_obj(_py, maxdigits_bits) else {
            return MoltObject::none().bits();
        };
        let val = i64::from(value);
        if val != 0 && val < INT_STR_DIGITS_CHECK_THRESHOLD {
            let msg = format!(
                "maxdigits must be 0 or larger than {}",
                INT_STR_DIGITS_CHECK_THRESHOLD
            );
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
        sys_state(_py)
            .int_max_str_digits
            .store(val, AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    })
}

// --- call_tracing ---

/// `sys.call_tracing(func, args)` — validate types in Rust.
/// Returns 0 for "valid, proceed" or raises TypeError.  The actual call
/// is done on the Python side (since the result must be a Python object).
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_call_tracing_validate(func_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !is_truthy(_py, obj_from_bits(molt_is_callable(func_bits))) {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "call_tracing() argument 1 must be callable",
            );
        }
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            let type_id = unsafe { crate::object_type_id(args_ptr) };
            if type_id == crate::TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
        }
        raise_exception::<u64>(
            _py,
            "TypeError",
            "call_tracing() argument 2 must be a tuple",
        )
    })
}

// --- Audit hooks ---

/// `sys.addaudithook(hook)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_addaudithook(hook_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !audit_event_noargs(_py, "sys.addaudithook") {
            let exception = molt_exception_last_pending();
            let suppressed = crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                exception,
                "Exception",
            );
            if suppressed {
                clear_exception(_py);
            }
            dec_ref_bits(_py, exception);
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, hook_bits);
        sys_state(_py)
            .audit_hooks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(hook_bits);
        MoltObject::none().bits()
    })
}

/// Dispatch through the live runtime-owned list: a hook appended by a
/// callback is visited in this event. Pin only the current hook under the owner
/// lock; Python calls and releases always occur after unlocking.
fn dispatch_audit_event(py: &PyToken<'_>, event_bits: u64, args_bits: u64) -> bool {
    let mut index = 0;
    loop {
        let hook = {
            let hooks = sys_state(py)
                .audit_hooks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            hooks
                .get(index)
                .copied()
                .inspect(|&bits| pin_owned_bits(py, bits))
        };
        let Some(hook) = hook else {
            return true;
        };
        index += 1;
        // Optional public lookup still executes descriptors/__getattribute__.
        // AttributeError means missing; other failures and truthiness failures
        // abort dispatch even when no tracing hook is active.
        let Some(name) = attr_name_bits_from_bytes(py, b"__cantrace__") else {
            release_owned_bits(py, hook);
            return false;
        };
        let cantrace = molt_get_attr_name_default(hook, name, MoltObject::from_bool(false).bits());
        dec_ref_bits(py, name);
        if !exception_pending(py) {
            let _enabled = is_truthy(py, obj_from_bits(cantrace));
        }
        dec_ref_bits(py, cantrace);
        if exception_pending(py) {
            release_owned_bits(py, hook);
            return false;
        }
        let result = unsafe { call_callable2(py, hook, event_bits, args_bits) };
        // Canonical result/owner release runs finalizers and weakref callbacks
        // through the unraisable transaction, preserving pending exceptions.
        crate::call::discard_owned_call_result(py, result);
        release_owned_bits(py, hook);
        if exception_pending(py) {
            return false;
        }
    }
}

pub(crate) fn audit_event_noargs(py: &PyToken<'_>, event: &str) -> bool {
    if sys_state(py)
        .audit_hooks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_empty()
    {
        return true;
    }
    crate::builtins::exceptions::with_saved_raised_exception(py, || {
        let event_bits = str_bits(py, event);
        if obj_from_bits(event_bits).is_none() {
            return false;
        }
        let args = alloc_tuple(py, &[]);
        if args.is_null() {
            dec_ref_bits(py, event_bits);
            return false;
        }
        let args_bits = MoltObject::from_ptr(args).bits();
        let completed = dispatch_audit_event(py, event_bits, args_bits);
        dec_ref_bits(py, args_bits);
        dec_ref_bits(py, event_bits);
        completed
    })
}

/// `sys.audit(event, *args)` shares runtime dispatch with intrinsic events.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_audit(event_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // String backing includes str subclasses; admission never invokes
        // __str__ or another user conversion. Hooks receive a plain str.
        let Some(event_ptr) = obj_from_bits(event_bits)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        else {
            let name = type_name(py, obj_from_bits(event_bits));
            let message = if crate::object::ops_sys::runtime_target_minor(py) >= 14 {
                format!("audit() argument 1 must be str, not {name}")
            } else {
                format!("expected str for argument 'event', not {name}")
            };
            return raise_exception::<u64>(py, "TypeError", &message);
        };
        let tuple = obj_from_bits(args_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE });
        if !tuple {
            return raise_exception::<u64>(py, "TypeError", "audit arguments must be a tuple");
        }
        let event_length = unsafe { string_len(event_ptr) };
        let normalized_length = unsafe {
            std::slice::from_raw_parts(string_bytes(event_ptr), event_length)
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(event_length)
        };
        // Python 3.14 validates C-string admission even without installed hooks.
        // Earlier versions truncate at the first NUL when dispatching.
        if normalized_length != event_length
            && crate::object::ops_sys::runtime_target_minor(py) >= 14
        {
            return raise_exception::<u64>(py, "ValueError", "embedded null character");
        }
        if sys_state(py)
            .audit_hooks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
        {
            return MoltObject::none().bits();
        }
        crate::builtins::exceptions::with_saved_raised_exception(py, || {
            let normalized = unsafe {
                let bytes =
                    std::slice::from_raw_parts(string_bytes(event_ptr), string_len(event_ptr));
                alloc_string(py, &bytes[..normalized_length])
            };
            if normalized.is_null() {
                return false;
            }
            let normalized_bits = MoltObject::from_ptr(normalized).bits();
            let completed = dispatch_audit_event(py, normalized_bits, args_bits);
            dec_ref_bits(py, normalized_bits);
            completed
        });
        MoltObject::none().bits()
    })
}

/// `sys.exit(code)` -> raises SystemExit
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_exit(code_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = code_bits;
        raise_exception::<u64>(_py, "SystemExit", "")
    })
}

// --- displayhook / excepthook / unraisablehook delegated to Python ---
// These are complex functions that interact with Python's repr/traceback
// formatting. We provide thin intrinsic stubs that the Python side calls
// to write to stdout/stderr.

/// `sys._displayhook_write(text)` -> None  (write text to stdout)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_displayhook_write(text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(s) = string_obj_to_owned(obj_from_bits(text_bits)) {
            print!("{s}");
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(_py, "TypeError", "expected str")
        }
    })
}

/// `sys._excepthook_write(text)` -> None  (write text to stderr)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_excepthook_write(text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(s) = string_obj_to_owned(obj_from_bits(text_bits)) {
            eprint!("{s}");
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(_py, "TypeError", "expected str")
        }
    })
}

// ---------------------------------------------------------------------------
// 8. Tier-0 gaps for click / trio / httpx support
// ---------------------------------------------------------------------------

/// `sys.argv` → list[str]
///
/// Returns the process command-line arguments.  The Python wrapper stores this
/// as the canonical `sys.argv` list on first access.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_argv() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        crate::object::ops_sys::with_process_argv(_py, |args| {
            let mut bits_vec: Vec<u64> = Vec::with_capacity(args.len());
            for arg in args {
                let ptr = alloc_string(_py, arg);
                if ptr.is_null() {
                    for &bits in &bits_vec {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                bits_vec.push(MoltObject::from_ptr(ptr).bits());
            }
            let ptr = alloc_list(_py, &bits_vec);
            for &bits in &bits_vec {
                dec_ref_bits(_py, bits);
            }
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

/// `sys.modules` → dict[str, module]
///
/// Returns an empty dict that the Python wrapper seeds with the actual module
/// cache.  The real `sys.modules` dict lives on the Python side and is
/// synchronised through `molt_module_import`.  This intrinsic provides the
/// initial empty dict so that the `sys` module object has a `modules`
/// attribute at bootstrap time.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_modules() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

/// `sys.path` → list[str]
///
/// Returns the initial module search path derived from explicit Molt module
/// roots and the executable location. The Python wrapper may mutate this list.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_path() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut entries: Vec<String> = Vec::new();

        // 1. Current directory (empty string = cwd per CPython convention)
        entries.push(String::new());

        // 2. Explicit Molt module roots. Ambient PYTHONPATH belongs to the
        // host Python process used to drive tooling; compiled binaries must
        // not silently import from that host search path.
        if let Ok(module_roots) = std::env::var("MOLT_MODULE_ROOTS") {
            for p in module_roots.split(if cfg!(windows) { ';' } else { ':' }) {
                if !p.is_empty() {
                    entries.push(p.to_string());
                }
            }
        }

        // 3. Executable's parent lib directory
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let lib_dir = parent.join("lib");
            if lib_dir.is_dir() {
                entries.push(lib_dir.to_string_lossy().into_owned());
            }
            entries.push(parent.to_string_lossy().into_owned());
        }

        let mut bits_vec: Vec<u64> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let ptr = alloc_string(_py, entry.as_bytes());
            if ptr.is_null() {
                for &b in &bits_vec {
                    dec_ref_bits(_py, b);
                }
                return MoltObject::none().bits();
            }
            bits_vec.push(MoltObject::from_ptr(ptr).bits());
        }
        let ptr = alloc_list(_py, &bits_vec);
        for &b in &bits_vec {
            dec_ref_bits(_py, b);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::builders::alloc_function_obj;

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn sys_metadata_hash_fields_describe_actual_numeric_hashing() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let bits = molt_sys_hash_info();
            assert!(!exception_pending(py));
            let ptr = obj_from_bits(bits).as_ptr().unwrap();
            unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |fields| {
                    assert_eq!(fields.len(), 9);
                    assert_eq!(to_i64(obj_from_bits(fields[0])), Some(isize::BITS as i64));
                    let modulus = to_i64(obj_from_bits(fields[1])).unwrap();
                    assert_eq!(crate::object::ops_hash::hash_int(modulus), 0);
                    assert_eq!(crate::object::ops_hash::hash_int(modulus + 1), 1);
                    assert_eq!(crate::object::ops_hash::hash_int(-modulus - 1), -2);
                    if let Some(modulus_ptr) = obj_from_bits(fields[1]).as_ptr() {
                        assert_eq!(
                            ref_count(modulus_ptr),
                            1,
                            "tuple owns the sole field reference"
                        );
                    }
                });
            }
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn sys_metadata_owned_publication_releases_fields_on_success_and_failure() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreBudget;
        impl Drop for RestoreBudget {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let field = str_bits(py, "owned sys metadata field");
            let ptr = obj_from_bits(field).as_ptr().unwrap();
            let initial = ref_count(ptr);
            inc_ref_bits(py, field);
            let tuple = sys_tuple_from_owned(py, &[field]);
            assert!(!obj_from_bits(tuple).is_none());
            assert_eq!(ref_count(ptr), initial + 1);
            dec_ref_bits(py, tuple);
            assert_eq!(ref_count(ptr), initial);

            inc_ref_bits(py, field);
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(0),
                ..Default::default()
            })));
            let budget = RestoreBudget;
            assert!(obj_from_bits(sys_tuple_from_owned(py, &[field])).is_none());
            assert!(exception_pending(py));
            assert_eq!(ref_count(ptr), initial);
            drop(budget);
            clear_exception(py);

            raise_exception::<u64>(py, "ValueError", "original metadata error");
            let original = molt_exception_last_pending();
            inc_ref_bits(py, field);
            assert!(obj_from_bits(sys_tuple_from_owned(py, &[field])).is_none());
            let observed = molt_exception_last_pending();
            assert_eq!(
                observed, original,
                "publication must preserve the first error"
            );
            assert_eq!(ref_count(ptr), initial);
            clear_exception(py);
            dec_ref_bits(py, observed);
            dec_ref_bits(py, original);
            dec_ref_bits(py, field);
        });
    }

    #[test]
    fn sys_metadata_constructors_never_publish_partial_results_under_denial() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreBudget;
        impl Drop for RestoreBudget {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for constructor in [
                molt_sys_float_info,
                molt_sys_int_info,
                molt_sys_hash_info,
                molt_sys_thread_info,
                molt_sys_stdlib_module_names,
                molt_sys_builtin_module_names,
                crate::molt_sys_flags_payload,
                crate::molt_sys_implementation_payload,
                crate::molt_sys_version_info,
            ] {
                // Stabilize immortal string interning before exercising every
                // remaining field/tuple allocation boundary.
                let warm = constructor();
                assert!(!exception_pending(py));
                dec_ref_bits(py, warm);
                let mut completed = false;
                let mut failed = false;
                for limit in 0..=64 {
                    set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                        max_allocations: Some(limit),
                        ..Default::default()
                    })));
                    let budget = RestoreBudget;
                    let result = constructor();
                    if obj_from_bits(result).is_none() {
                        assert!(exception_pending(py), "silent metadata failure at {limit}");
                        failed = true;
                    } else {
                        assert!(
                            !exception_pending(py),
                            "partial metadata escaped at {limit}"
                        );
                        completed = true;
                        dec_ref_bits(py, result);
                    }
                    drop(budget);
                    clear_exception(py);
                    if completed {
                        break;
                    }
                }
                assert!(failed && completed);
            }
        });
    }

    #[test]
    fn sys_metadata_version_components_publish_full_width_owned_integers() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let info = crate::state::runtime_state::PythonVersionInfo {
                major: 3,
                minor: 12,
                micro: i64::MAX,
                releaselevel: "final".to_owned(),
                serial: i64::MAX,
            };
            let bits = crate::object::ops_sys::alloc_sys_version_info_tuple(py, &info).unwrap();
            assert!(!exception_pending(py));
            unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(bits).as_ptr().unwrap(),
                    |fields| {
                        for index in [2, 4] {
                            assert_eq!(to_i64(obj_from_bits(fields[index])), Some(i64::MAX));
                            assert_eq!(
                                ref_count(obj_from_bits(fields[index]).as_ptr().unwrap()),
                                1
                            );
                        }
                    },
                );
            }
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn sys_metadata_limit_inputs_reject_wide_values_before_mutating_state() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (setter, getter) in [
                (
                    molt_sys_set_int_max_str_digits as extern "C" fn(u64) -> u64,
                    molt_sys_get_int_max_str_digits as extern "C" fn() -> u64,
                ),
                (
                    crate::molt_setrecursionlimit as extern "C" fn(u64) -> u64,
                    crate::molt_getrecursionlimit as extern "C" fn() -> u64,
                ),
            ] {
                let original = getter();
                setter(MoltObject::from_int(i32::MAX as i64).bits());
                assert!(!exception_pending(py));
                assert_eq!(to_i64(obj_from_bits(getter())), Some(i32::MAX as i64));
                for value in [i32::MAX as i128 + 1, i32::MIN as i128 - 1, 1i128 << 80] {
                    let bits = crate::builtins::numbers::int_bits_from_i128(py, value);
                    setter(bits);
                    dec_ref_bits(py, bits);
                    assert!(exception_pending(py));
                    let error = molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "OverflowError"
                    ));
                    clear_exception(py);
                    dec_ref_bits(py, error);
                    assert_eq!(to_i64(obj_from_bits(getter())), Some(i32::MAX as i64));
                }
                setter(original);
                dec_ref_bits(py, original);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn sys_metadata_environment_fields_preserve_full_width_integer_values() {
        struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                }
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let keys = ["MOLT_SYS_API_VERSION", "PYTHONOPTIMIZE"];
        let _environment = RestoreEnvironment(keys.map(|key| (key, std::env::var_os(key))).into());
        for key in keys {
            unsafe {
                std::env::set_var(key, i64::MAX.to_string());
            }
        }
        crate::with_gil_entry_nopanic!(py, {
            let api = crate::molt_sys_api_version();
            assert_eq!(to_i64(obj_from_bits(api)), Some(i64::MAX));
            dec_ref_bits(py, api);
            let flags = crate::molt_sys_flags_payload();
            assert!(!exception_pending(py));
            let key = str_bits(py, "optimize");
            let value =
                unsafe { dict_get_in_place(py, obj_from_bits(flags).as_ptr().unwrap(), key) }
                    .unwrap();
            assert_eq!(to_i64(obj_from_bits(value)), Some(i64::MAX));
            assert_eq!(ref_count(obj_from_bits(value).as_ptr().unwrap()), 1);
            dec_ref_bits(py, key);
            dec_ref_bits(py, flags);
        });
    }

    static AUDIT_LATE: AtomicU64 = AtomicU64::new(0);
    static AUDIT_VETO: AtomicU64 = AtomicU64::new(0);
    static AUDIT_EVENTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    extern "C" fn audit_first(event: u64, _args: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            match string_obj_to_owned(obj_from_bits(event)).as_deref() {
                Some("molt.audit.live") => {
                    AUDIT_EVENTS.lock().unwrap().push("first-live");
                    molt_sys_addaudithook(AUDIT_LATE.load(AtomicOrdering::Relaxed));
                    assert!(audit_event_noargs(py, "molt.audit.inner"));
                }
                Some("molt.audit.inner") => AUDIT_EVENTS.lock().unwrap().push("first-inner"),
                Some("sys.addaudithook") => AUDIT_EVENTS.lock().unwrap().push("first-add"),
                _ => {}
            }
            MoltObject::none().bits()
        })
    }

    extern "C" fn audit_late(event: u64, _args: u64) -> u64 {
        match string_obj_to_owned(obj_from_bits(event)).as_deref() {
            Some("molt.audit.live") => AUDIT_EVENTS.lock().unwrap().push("late-live"),
            Some("molt.audit.inner") => AUDIT_EVENTS.lock().unwrap().push("late-inner"),
            _ => {}
        }
        MoltObject::none().bits()
    }

    extern "C" fn audit_gate(event: u64, _args: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if string_obj_to_owned(obj_from_bits(event)).as_deref() == Some("sys.addaudithook") {
                match AUDIT_VETO.load(AtomicOrdering::Relaxed) {
                    1 => return raise_exception::<u64>(py, "ValueError", "veto"),
                    2 => return raise_exception::<u64>(py, "KeyboardInterrupt", "veto"),
                    _ => {}
                }
            }
            MoltObject::none().bits()
        })
    }

    fn audit_callback(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            2,
        );
        assert!(!ptr.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(ptr, address) };
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn audit_visits_live_additions_reenters_and_applies_registration_failure_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            AUDIT_EVENTS.lock().unwrap().clear();
            AUDIT_VETO.store(0, AtomicOrdering::Relaxed);
            let first = audit_callback(py, audit_first as *const ());
            let late = audit_callback(py, audit_late as *const ());
            let gate = audit_callback(py, audit_gate as *const ());
            AUDIT_LATE.store(late, AtomicOrdering::Relaxed);
            molt_sys_addaudithook(first);
            assert!(audit_event_noargs(py, "molt.audit.live"));
            assert_eq!(
                *AUDIT_EVENTS.lock().unwrap(),
                [
                    "first-live",
                    "first-add",
                    "first-inner",
                    "late-inner",
                    "late-live",
                ]
            );
            molt_sys_addaudithook(gate);
            let count = sys_state(py).audit_hooks.lock().unwrap().len();
            AUDIT_VETO.store(1, AtomicOrdering::Relaxed);
            molt_sys_addaudithook(late);
            assert!(!exception_pending(py));
            assert_eq!(sys_state(py).audit_hooks.lock().unwrap().len(), count);
            AUDIT_VETO.store(2, AtomicOrdering::Relaxed);
            molt_sys_addaudithook(late);
            assert!(exception_pending(py));
            let exception = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "KeyboardInterrupt"
            ));
            clear_exception(py);
            dec_ref_bits(py, exception);
            assert_eq!(sys_state(py).audit_hooks.lock().unwrap().len(), count);
            AUDIT_VETO.store(0, AtomicOrdering::Relaxed);
            raise_exception::<u64>(py, "ValueError", "incoming");
            let incoming = molt_exception_last_pending();
            assert!(audit_event_noargs(py, "molt.audit.inner"));
            let restored = molt_exception_last_pending();
            assert_eq!(restored, incoming);
            dec_ref_bits(py, restored);
            // Failed audit replaces the incoming raised owner. It is not
            // automatically chained as handled context while the hook runs.
            AUDIT_VETO.store(2, AtomicOrdering::Relaxed);
            assert!(!audit_event_noargs(py, "sys.addaudithook"));
            let replacement = molt_exception_last_pending();
            assert_ne!(replacement, incoming);
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                replacement,
                "KeyboardInterrupt"
            ));
            clear_exception(py);
            dec_ref_bits(py, incoming);
            dec_ref_bits(py, replacement);
            AUDIT_VETO.store(0, AtomicOrdering::Relaxed);
            sys_ext_clear_state(py, runtime_state(py));
            molt_sys_addaudithook(MoltObject::from_int(1).bits());
            assert!(!exception_pending(py));
            assert!(!audit_event_noargs(py, "molt.audit.noncallable"));
            clear_exception(py);
            sys_ext_clear_state(py, runtime_state(py));
            for bits in [first, late, gate] {
                dec_ref_bits(py, bits);
            }
            AUDIT_LATE.store(0, AtomicOrdering::Relaxed);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn sys_platlibdir_matches_platform_contract() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let bits = molt_sys_platlibdir();
            assert_eq!(
                string_obj_to_owned(obj_from_bits(bits)).as_deref(),
                Some(platlibdir_name())
            );
            if cfg!(windows) {
                assert_eq!(platlibdir_name(), "DLLs");
            } else {
                assert_eq!(platlibdir_name(), "lib");
            }
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn sys_ext_state_is_runtime_scoped_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            sys_ext_clear_state(_py, state);

            let input_ptr = alloc_string(_py, b"sys-ext-runtime-intern");
            let input_bits = MoltObject::from_ptr(input_ptr).bits();
            let interned_bits = molt_sys_intern(input_bits);
            let interned_ptr = obj_from_bits(interned_bits).as_ptr().unwrap();
            let interned_refs = ref_count(interned_ptr);
            assert_eq!(molt_sys_intern(input_bits), interned_bits);

            let trace_ptr = alloc_function_obj(_py, 0, 0);
            let trace_bits = MoltObject::from_ptr(trace_ptr).bits();
            let trace_refs_initial = ref_count(trace_ptr);
            assert!(obj_from_bits(molt_sys_settrace(trace_bits)).is_none());
            assert_eq!(ref_count(trace_ptr), trace_refs_initial + 1);
            let returned_trace = molt_sys_gettrace();
            assert_eq!(returned_trace, trace_bits);
            dec_ref_bits(_py, returned_trace);

            let profile_ptr = alloc_function_obj(_py, 0, 0);
            let profile_bits = MoltObject::from_ptr(profile_ptr).bits();
            let profile_refs_initial = ref_count(profile_ptr);
            assert!(obj_from_bits(molt_sys_setprofile(profile_bits)).is_none());
            assert_eq!(ref_count(profile_ptr), profile_refs_initial + 1);
            let returned_profile = molt_sys_getprofile();
            assert_eq!(returned_profile, profile_bits);
            dec_ref_bits(_py, returned_profile);

            let hook_ptr = alloc_function_obj(_py, 0, 0);
            let hook_bits = MoltObject::from_ptr(hook_ptr).bits();
            let hook_refs_initial = ref_count(hook_ptr);
            assert!(obj_from_bits(molt_sys_addaudithook(hook_bits)).is_none());
            assert_eq!(ref_count(hook_ptr), hook_refs_initial + 1);
            assert_eq!(sys_state(_py).audit_hooks.lock().unwrap().len(), 1);

            assert!(
                obj_from_bits(molt_sys_setswitchinterval(
                    MoltObject::from_float(0.25).bits()
                ))
                .is_none()
            );
            assert_eq!(
                to_f64(obj_from_bits(molt_sys_getswitchinterval())),
                Some(0.25)
            );
            assert!(
                obj_from_bits(molt_sys_set_int_max_str_digits(
                    MoltObject::from_int(0).bits()
                ))
                .is_none()
            );
            assert_eq!(
                to_i64(obj_from_bits(molt_sys_get_int_max_str_digits())),
                Some(0)
            );

            sys_ext_clear_state(_py, state);

            assert_eq!(ref_count(interned_ptr), interned_refs);
            assert_eq!(ref_count(trace_ptr), trace_refs_initial);
            assert_eq!(ref_count(profile_ptr), profile_refs_initial);
            assert_eq!(ref_count(hook_ptr), hook_refs_initial);
            assert!(obj_from_bits(molt_sys_gettrace()).is_none());
            assert!(obj_from_bits(molt_sys_getprofile()).is_none());
            assert!(sys_state(_py).audit_hooks.lock().unwrap().is_empty());
            assert_eq!(
                to_f64(obj_from_bits(molt_sys_getswitchinterval())),
                Some(0.005)
            );
            assert_eq!(
                to_i64(obj_from_bits(molt_sys_get_int_max_str_digits())),
                Some(DEFAULT_INT_MAX_STR_DIGITS)
            );

            let input2_ptr = alloc_string(_py, b"sys-ext-runtime-intern-2");
            let input2_bits = MoltObject::from_ptr(input2_ptr).bits();
            let interned2_bits = molt_sys_intern(input2_bits);
            assert_eq!(molt_sys_intern(input2_bits), interned2_bits);

            sys_ext_clear_state(_py, state);
            dec_ref_bits(_py, interned2_bits);
            dec_ref_bits(_py, input2_bits);
            dec_ref_bits(_py, hook_bits);
            dec_ref_bits(_py, profile_bits);
            dec_ref_bits(_py, trace_bits);
            dec_ref_bits(_py, interned_bits);
            dec_ref_bits(_py, input_bits);
            assert!(!exception_pending(_py));
        });
    }
}
