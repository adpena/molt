#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/platform_mod.rs ===
//! `platform` module intrinsics for Molt.
//!
//! Provides system identification functions that match the CPython `platform`
//! module API.  All values are determined at compile-time or via OS syscalls
//! and never reflect CPython details (we report "Molt" as the implementation).
//!
//! ABI: NaN-boxed u64 in/out.

#[allow(unused_imports)]
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::sync::OnceLock;

// ── Compile-time platform detection ──────────────────────────────────────

#[cfg(target_os = "macos")]
const PLATFORM_SYSTEM: &str = "Darwin";
#[cfg(target_os = "linux")]
const PLATFORM_SYSTEM: &str = "Linux";
#[cfg(target_os = "windows")]
const PLATFORM_SYSTEM: &str = "Windows";
#[cfg(target_arch = "wasm32")]
const PLATFORM_SYSTEM: &str = "WASM";
#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_arch = "wasm32"
)))]
const PLATFORM_SYSTEM: &str = "Unknown";

#[cfg(target_arch = "x86_64")]
const PLATFORM_MACHINE: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
const PLATFORM_MACHINE: &str = "aarch64";
#[cfg(target_arch = "wasm32")]
const PLATFORM_MACHINE: &str = "wasm32";
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "wasm32"
)))]
const PLATFORM_MACHINE: &str = "unknown";

#[cfg(target_pointer_width = "64")]
const PLATFORM_BITS: &str = "64bit";
#[cfg(target_pointer_width = "32")]
const PLATFORM_BITS: &str = "32bit";

/// Molt's emulated Python version string.
const MOLT_PYTHON_VERSION: &str = "3.12.0";
const MOLT_PYTHON_MAJOR: &str = "3";
const MOLT_PYTHON_MINOR: &str = "12";
const MOLT_PYTHON_MICRO: &str = "0";

// ── Helpers ───────────────────────────────────────────────────────────────

fn return_str(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

fn return_none() -> u64 {
    MoltObject::none().bits()
}

// ── Uname cache (Unix) ────────────────────────────────────────────────────

#[derive(Clone)]
struct UnameInfo {
    sysname: String,
    nodename: String,
    release: String,
    version: String,
    machine: String,
}

static UNAME_CACHE: OnceLock<UnameInfo> = OnceLock::new();

fn get_uname() -> &'static UnameInfo {
    UNAME_CACHE.get_or_init(|| {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            #[allow(unused_unsafe)]
            unsafe {
                let mut buf: libc::utsname = std::mem::zeroed();
                if libc::uname(&mut buf) == 0 {
                    let c_str = |arr: &[libc::c_char]| -> String {
                        let bytes: Vec<u8> = arr
                            .iter()
                            .take_while(|&&c| c != 0)
                            .map(|&c| c as u8)
                            .collect();
                        String::from_utf8_lossy(&bytes).to_string()
                    };
                    return UnameInfo {
                        sysname: c_str(&buf.sysname),
                        nodename: c_str(&buf.nodename),
                        release: c_str(&buf.release),
                        version: c_str(&buf.version),
                        machine: c_str(&buf.machine),
                    };
                }
            }
        }
        // Fallback: compile-time constants.
        UnameInfo {
            sysname: PLATFORM_SYSTEM.to_string(),
            nodename: "localhost".to_string(),
            release: "0.0.0".to_string(),
            version: "#1".to_string(),
            machine: PLATFORM_MACHINE.to_string(),
        }
    })
}

// ── Public intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_system() -> u64 {
    crate::with_gil_entry!(_py, {
        let s = get_uname().sysname.as_str();
        return_str(_py, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_machine() -> u64 {
    crate::with_gil_entry!(_py, {
        let s = get_uname().machine.as_str();
        return_str(_py, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_processor() -> u64 {
    crate::with_gil_entry!(_py, {
        // On macOS arm64 the processor string is "arm" in CPython.
        // We mirror PLATFORM_MACHINE which is the most portable value.
        return_str(_py, PLATFORM_MACHINE)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_architecture() -> u64 {
    crate::with_gil_entry!(_py, {
        let bits_ptr = alloc_string(_py, PLATFORM_BITS.as_bytes());
        let link_ptr = alloc_string(_py, b"");
        if bits_ptr.is_null() || link_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_ptr(bits_ptr).bits(),
                MoltObject::from_ptr(link_ptr).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_node() -> u64 {
    crate::with_gil_entry!(_py, {
        let s = get_uname().nodename.as_str();
        return_str(_py, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_release() -> u64 {
    crate::with_gil_entry!(_py, {
        let s = get_uname().release.as_str();
        return_str(_py, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_version() -> u64 {
    crate::with_gil_entry!(_py, {
        let s = get_uname().version.as_str();
        return_str(_py, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_platform(aliased_bits: u64, terse_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _aliased = is_truthy(_py, obj_from_bits(aliased_bits));
        let terse = is_truthy(_py, obj_from_bits(terse_bits));
        let uname = get_uname();
        let s = if terse {
            format!("{}-{}", uname.sysname, uname.release)
        } else {
            format!(
                "{}-{}-{}-{}",
                uname.sysname, uname.release, uname.version, uname.machine
            )
        };
        return_str(_py, &s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_python_version() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, MOLT_PYTHON_VERSION) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_python_version_tuple() -> u64 {
    crate::with_gil_entry!(_py, {
        let major_ptr = alloc_string(_py, MOLT_PYTHON_MAJOR.as_bytes());
        let minor_ptr = alloc_string(_py, MOLT_PYTHON_MINOR.as_bytes());
        let micro_ptr = alloc_string(_py, MOLT_PYTHON_MICRO.as_bytes());
        if major_ptr.is_null() || minor_ptr.is_null() || micro_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_ptr(major_ptr).bits(),
                MoltObject::from_ptr(minor_ptr).bits(),
                MoltObject::from_ptr(micro_ptr).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_python_implementation() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, "Molt") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_python_compiler() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, "Molt/Cranelift") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_platform_uname() -> u64 {
    crate::with_gil_entry!(_py, {
        let uname = get_uname();
        let make_str = |s: &str| -> u64 {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                0u64
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        };
        let sys_bits = make_str(&uname.sysname);
        let node_bits = make_str(&uname.nodename);
        let rel_bits = make_str(&uname.release);
        let ver_bits = make_str(&uname.version);
        let mach_bits = make_str(&uname.machine);
        if [sys_bits, node_bits, rel_bits, ver_bits, mach_bits].contains(&0) {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let tuple_ptr = alloc_tuple(_py, &[sys_bits, node_bits, rel_bits, ver_bits, mach_bits]);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
