// === FILE: runtime/molt-runtime/src/builtins/sys_ext.rs ===
//
// Additional sys intrinsics for CPython 3.12+ parity.
// These supplement the existing sys intrinsics in object/ops.rs, io.rs, and platform.rs.
//
// No capability gates needed: these intrinsics return process metadata and
// language-level constants that are always available.

use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate a runtime string from a Rust &str slice, returning bits.
/// Returns None bits on allocation failure.
#[inline]
fn str_bits(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// String interning table
// ---------------------------------------------------------------------------

static INTERN_TABLE: OnceLock<Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();

fn intern_table() -> &'static Mutex<std::collections::HashMap<String, u64>> {
    INTERN_TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[derive(Clone, Copy)]
struct SysTraceProfileState {
    trace_bits: u64,
    profile_bits: u64,
}

static SYS_TRACE_PROFILE_STATE: OnceLock<Mutex<SysTraceProfileState>> = OnceLock::new();

fn sys_trace_profile_state() -> &'static Mutex<SysTraceProfileState> {
    SYS_TRACE_PROFILE_STATE.get_or_init(|| {
        Mutex::new(SysTraceProfileState {
            trace_bits: MoltObject::none().bits(),
            profile_bits: MoltObject::none().bits(),
        })
    })
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

fn replace_optional_callable(_py: &PyToken<'_>, target: &mut u64, value_bits: u64) {
    if *target == value_bits {
        return;
    }
    if !obj_from_bits(value_bits).is_none() {
        inc_ref_bits(_py, value_bits);
    }
    if !obj_from_bits(*target).is_none() {
        dec_ref_bits(_py, *target);
    }
    *target = value_bits;
}

fn clone_optional_callable(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if !obj_from_bits(value_bits).is_none() {
        inc_ref_bits(_py, value_bits);
    }
    value_bits
}

// ---------------------------------------------------------------------------
// 1. Scalar constants
// ---------------------------------------------------------------------------

/// `sys.maxsize` -> isize::MAX
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_maxsize() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, isize::MAX as i64) })
}

/// `sys.maxunicode` -> 0x10FFFF (Unicode max code point)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_maxunicode() -> u64 {
    MoltObject::from_int(0x10FFFF).bits()
}

/// `sys.byteorder` -> "little" or "big"
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_byteorder() -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

/// `sys.platlibdir` -> "lib" on most platforms
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_platlibdir() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, "lib") })
}

// ---------------------------------------------------------------------------
// 3. Structured info tuples
// ---------------------------------------------------------------------------

/// `sys.float_info` -> 11-element tuple of f64 system constants
/// Fields: max, max_exp, max_10_exp, min, min_exp, min_10_exp, dig, mant_dig, epsilon, radix, rounds
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_float_info() -> u64 {
    crate::with_gil_entry!(_py, {
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
        let ptr = alloc_tuple(_py, &values);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `sys.int_info` -> 4-element tuple
/// Fields: bits_per_digit, sizeof_digit, default_max_str_digits, str_digits_check_threshold
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_int_info() -> u64 {
    crate::with_gil_entry!(_py, {
        // Molt uses NaN-boxed 47-bit inline ints; for API compat report CPython-compatible values
        let values: [u64; 4] = [
            MoltObject::from_int(30).bits(),   // bits_per_digit (CPython default)
            MoltObject::from_int(4).bits(),    // sizeof_digit (4 bytes = uint32)
            MoltObject::from_int(4300).bits(), // default_max_str_digits
            MoltObject::from_int(640).bits(),  // str_digits_check_threshold
        ];
        let ptr = alloc_tuple(_py, &values);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `sys.hash_info` -> 9-element tuple
/// Fields: width, modulus, inf, nan, imag, algorithm, hash_bits, seed_bits, cutoff
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_hash_info() -> u64 {
    crate::with_gil_entry!(_py, {
        let width = if cfg!(target_pointer_width = "64") {
            64i64
        } else {
            32
        };
        let modulus = if cfg!(target_pointer_width = "64") {
            (1i64 << 61) - 1
        } else {
            (1i64 << 31) - 1
        };
        let alg_bits = str_bits(_py, "siphash13");
        let values: [u64; 9] = [
            MoltObject::from_int(width).bits(),   // width
            MoltObject::from_int(modulus).bits(), // modulus
            MoltObject::from_int(314159).bits(),  // inf hash
            MoltObject::from_int(0).bits(),       // nan hash
            MoltObject::from_int(1000003).bits(), // imag multiplier
            alg_bits,                             // algorithm name
            MoltObject::from_int(64).bits(),      // hash_bits
            MoltObject::from_int(128).bits(),     // seed_bits
            MoltObject::from_int(0).bits(),       // cutoff
        ];
        let ptr = alloc_tuple(_py, &values);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `sys.thread_info` -> 3-element tuple (name, lock, version)
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_thread_info() -> u64 {
    crate::with_gil_entry!(_py, {
        let name_str = if cfg!(target_os = "windows") {
            "nt"
        } else if cfg!(target_arch = "wasm32") {
            "wasm"
        } else {
            // linux, macos, and other POSIX platforms
            "pthread"
        };
        let name_bits = str_bits(_py, name_str);
        let lock_bits = str_bits(_py, "mutex+cond");
        let values: [u64; 3] = [
            name_bits,
            lock_bits,
            MoltObject::none().bits(), // version (None = unknown)
        ];
        let ptr = alloc_tuple(_py, &values);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
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
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let count = if let Some(ptr) = obj.as_ptr() {
            let header = unsafe { header_from_obj_ptr(ptr) };
            let rc = unsafe { (*header).ref_count.load(AtomicOrdering::Acquire) } as i64;
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
    crate::with_gil_entry!(_py, {
        if let Err(err) = ensure_trace_or_profile_callable(_py, tracefunc_bits, "settrace") {
            return err;
        }
        let mut state = sys_trace_profile_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        replace_optional_callable(_py, &mut state.trace_bits, tracefunc_bits);
        MoltObject::none().bits()
    })
}

/// `sys.gettrace()` -> current process-level trace hook.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_gettrace() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = sys_trace_profile_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clone_optional_callable(_py, state.trace_bits)
    })
}

/// `sys.setprofile(profilefunc)` -> store process-level profile hook (or None).
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_setprofile(profilefunc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Err(err) = ensure_trace_or_profile_callable(_py, profilefunc_bits, "setprofile") {
            return err;
        }
        let mut state = sys_trace_profile_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        replace_optional_callable(_py, &mut state.profile_bits, profilefunc_bits);
        MoltObject::none().bits()
    })
}

/// `sys.getprofile()` -> current process-level profile hook.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getprofile() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = sys_trace_profile_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clone_optional_callable(_py, state.profile_bits)
    })
}

/// `sys.intern(string)` -> interned string
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_intern(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let mut table = intern_table().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(&bits) = table.get(&s) {
            inc_ref_bits(_py, bits);
            return bits;
        }
        // Allocate new interned string
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        inc_ref_bits(_py, bits); // extra ref for the table
        table.insert(s, bits);
        bits
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
        TYPE_ID_DICT | TYPE_ID_DICT_BUILDER => {
            let len = unsafe { crate::builtins::containers::dict_len(ptr) } as i64;
            64 + len * 3 * 8 // CPython dict: ~64 + 3*8 per entry (hash, key, value)
        }
        TYPE_ID_SET | TYPE_ID_SET_BUILDER | TYPE_ID_FROZENSET => {
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
        TYPE_ID_EXCEPTION => 88,    // CPython BaseException: ~88 bytes
        TYPE_ID_BIGINT => 32,       // approximation for arbitrary-precision int
        TYPE_ID_CODE => 176,        // CPython code object: ~176 bytes
        _ => 64,                    // reasonable default for other heap objects
    };
    MoltObject::from_int(size).bits()
}

// ---------------------------------------------------------------------------
// 5. Module name lists
// ---------------------------------------------------------------------------

/// `sys.stdlib_module_names` -> tuple of stdlib module names
/// Python wrapper converts to frozenset.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdlib_module_names() -> u64 {
    crate::with_gil_entry!(_py, {
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
        let ptr = alloc_tuple(_py, &bits_vec);
        // alloc_tuple inc_refs each element, so dec_ref our locals
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

/// `sys.builtin_module_names` -> tuple of built-in module names
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_builtin_module_names() -> u64 {
    crate::with_gil_entry!(_py, {
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
        let ptr = alloc_tuple(_py, &bits_vec);
        // alloc_tuple inc_refs each element, so dec_ref our locals
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

// ---------------------------------------------------------------------------
// 6. Process info
// ---------------------------------------------------------------------------

/// `sys.orig_argv` -> original argv from process start
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_orig_argv() -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, { str_bits(_py, "utf-8") })
}

/// `sys.getfilesystemencoding()` -> "utf-8"
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getfilesystemencoding() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, "utf-8") })
}

// --- Thread switch interval (GIL timeslice stub) ---

static SWITCH_INTERVAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new({
    // 0.005 as f64 bits
    0.005f64.to_bits()
});

/// `sys.getswitchinterval()` -> float seconds
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getswitchinterval() -> u64 {
    let bits = SWITCH_INTERVAL.load(AtomicOrdering::Relaxed);
    MoltObject::from_float(f64::from_bits(bits)).bits()
}

/// `sys.setswitchinterval(interval)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_setswitchinterval(interval_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        SWITCH_INTERVAL.store(val.to_bits(), AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    })
}

// --- Integer string conversion length limitation ---

static INT_MAX_STR_DIGITS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(4300);
const INT_STR_DIGITS_CHECK_THRESHOLD: i64 = 640;

/// `sys.get_int_max_str_digits()` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_get_int_max_str_digits() -> u64 {
    let val = INT_MAX_STR_DIGITS.load(AtomicOrdering::Relaxed);
    MoltObject::from_int(val).bits()
}

/// `sys.set_int_max_str_digits(maxdigits)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_set_int_max_str_digits(maxdigits_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let val = match to_i64(obj_from_bits(maxdigits_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "set_int_max_str_digits() argument must be a positive integer or zero",
                );
            }
        };
        if val != 0 && val < INT_STR_DIGITS_CHECK_THRESHOLD {
            let msg = format!(
                "maxdigits must be 0 or larger than {}",
                INT_STR_DIGITS_CHECK_THRESHOLD
            );
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
        INT_MAX_STR_DIGITS.store(val, AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    })
}

// --- call_tracing ---

/// `sys.call_tracing(func, args)` — validate types in Rust.
/// Returns 0 for "valid, proceed" or raises TypeError.  The actual call
/// is done on the Python side (since the result must be a Python object).
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_call_tracing_validate(func_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

static AUDIT_HOOKS: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();

fn audit_hooks() -> &'static Mutex<Vec<u64>> {
    AUDIT_HOOKS.get_or_init(|| Mutex::new(Vec::new()))
}

/// `sys.addaudithook(hook)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_addaudithook(hook_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !is_truthy(_py, obj_from_bits(molt_is_callable(hook_bits))) {
            return raise_exception::<u64>(_py, "TypeError", "expected a callable object");
        }
        inc_ref_bits(_py, hook_bits);
        let mut hooks = audit_hooks().lock().unwrap();
        hooks.push(hook_bits);
        MoltObject::none().bits()
    })
}

/// `sys.audit(event, *args)` -> None
/// Returns the hooks list length so the Python side can dispatch.
/// Hooks are stored in Rust; the Python side calls each via the returned
/// handle list.  For simplicity we return count; 0 means no hooks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_audit_hook_count() -> u64 {
    let hooks = audit_hooks().lock().unwrap();
    MoltObject::from_int(hooks.len() as i64).bits()
}

/// `sys._audit_get_hooks()` -> list of callable bits
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_audit_get_hooks() -> u64 {
    crate::with_gil_entry!(_py, {
        let hooks = audit_hooks().lock().unwrap();
        if hooks.is_empty() {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let ptr = alloc_list(_py, &hooks);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `sys.exit(code)` -> raises SystemExit
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_exit(code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

/// `sys.modules` → dict[str, module]
///
/// Returns an empty dict that the Python wrapper seeds with the actual module
/// cache.  The real `sys.modules` dict lives on the Python side and is
/// synchronised through `molt_module_import`.  This intrinsic provides the
/// initial empty dict so that the `sys` module object has a `modules`
/// attribute at bootstrap time.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_modules() -> u64 {
    crate::with_gil_entry!(_py, {
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
/// Returns the initial module search path derived from environment variables
/// and the executable location.  The Python wrapper may mutate this list.
#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_path() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut entries: Vec<String> = Vec::new();

        // 1. Current directory (empty string = cwd per CPython convention)
        entries.push(String::new());

        // 2. PYTHONPATH entries
        if let Ok(pypath) = std::env::var("PYTHONPATH") {
            for p in pypath.split(if cfg!(windows) { ';' } else { ':' }) {
                if !p.is_empty() {
                    entries.push(p.to_string());
                }
            }
        }

        // 3. Executable's parent lib directory
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent() {
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
