use std::sync::atomic::AtomicU64;

use crate::*;

// --- Platform constants ---

pub(crate) const IO_EVENT_READ: u32 = 1;
pub(crate) const IO_EVENT_WRITE: u32 = 1 << 1;
pub(crate) const IO_EVENT_ERROR: u32 = 1 << 2;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(crate) const SOCK_NONBLOCK_FLAG: i32 = libc::SOCK_NONBLOCK;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
pub(crate) const SOCK_NONBLOCK_FLAG: i32 = 0;
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(crate) const SOCK_CLOEXEC_FLAG: i32 = libc::SOCK_CLOEXEC;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
pub(crate) const SOCK_CLOEXEC_FLAG: i32 = 0;

// --- errno/socket/env helpers ---

#[no_mangle]
pub extern "C" fn molt_bridge_unavailable(msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        eprintln!("Molt bridge unavailable: {msg}");
        std::process::exit(1);
    })
}

static ERRNO_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);
static SOCKET_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);

fn base_errno_constants() -> Vec<(&'static str, i64)> {
    vec![
        ("EACCES", libc::EACCES as i64),
        ("EAGAIN", libc::EAGAIN as i64),
        ("EALREADY", libc::EALREADY as i64),
        ("ECHILD", libc::ECHILD as i64),
        ("ECONNABORTED", libc::ECONNABORTED as i64),
        ("ECONNREFUSED", libc::ECONNREFUSED as i64),
        ("ECONNRESET", libc::ECONNRESET as i64),
        ("EEXIST", libc::EEXIST as i64),
        ("EINPROGRESS", libc::EINPROGRESS as i64),
        ("EINTR", libc::EINTR as i64),
        ("EISDIR", libc::EISDIR as i64),
        ("ENOENT", libc::ENOENT as i64),
        ("ENOTDIR", libc::ENOTDIR as i64),
        ("EPERM", libc::EPERM as i64),
        ("EPIPE", libc::EPIPE as i64),
        ("ESRCH", libc::ESRCH as i64),
        ("ETIMEDOUT", libc::ETIMEDOUT as i64),
        ("EWOULDBLOCK", libc::EWOULDBLOCK as i64),
        ("ESHUTDOWN", libc::ESHUTDOWN as i64),
    ]
}

fn collect_errno_constants() -> Vec<(&'static str, i64)> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL2, priority:P1, status:partial): expand errno constants to match CPython's full table on each platform.
    #[cfg(target_os = "freebsd")]
    {
        let mut out = base_errno_constants();
        out.push(("ENOTCAPABLE", libc::ENOTCAPABLE as i64));
        out
    }
    #[cfg(not(target_os = "freebsd"))]
    {
        base_errno_constants()
    }
}

fn socket_constants() -> Vec<(&'static str, i64)> {
    let mut out = vec![
        ("AF_INET", libc::AF_INET as i64),
        ("AF_INET6", libc::AF_INET6 as i64),
        ("SOCK_STREAM", libc::SOCK_STREAM as i64),
        ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
        ("SOCK_RAW", libc::SOCK_RAW as i64),
        ("SOL_SOCKET", libc::SOL_SOCKET as i64),
        ("SO_REUSEADDR", libc::SO_REUSEADDR as i64),
        ("SO_KEEPALIVE", libc::SO_KEEPALIVE as i64),
        ("SO_SNDBUF", libc::SO_SNDBUF as i64),
        ("SO_RCVBUF", libc::SO_RCVBUF as i64),
        ("SO_ERROR", libc::SO_ERROR as i64),
        ("SO_LINGER", libc::SO_LINGER as i64),
        ("SO_BROADCAST", libc::SO_BROADCAST as i64),
        ("IPPROTO_TCP", libc::IPPROTO_TCP as i64),
        ("IPPROTO_UDP", libc::IPPROTO_UDP as i64),
        ("IPPROTO_IPV6", libc::IPPROTO_IPV6 as i64),
        ("IPV6_V6ONLY", libc::IPV6_V6ONLY as i64),
        ("TCP_NODELAY", libc::TCP_NODELAY as i64),
        ("SHUT_RD", libc::SHUT_RD as i64),
        ("SHUT_WR", libc::SHUT_WR as i64),
        ("SHUT_RDWR", libc::SHUT_RDWR as i64),
        ("AI_PASSIVE", libc::AI_PASSIVE as i64),
        ("AI_CANONNAME", libc::AI_CANONNAME as i64),
        ("AI_NUMERICHOST", libc::AI_NUMERICHOST as i64),
        ("AI_NUMERICSERV", libc::AI_NUMERICSERV as i64),
        ("NI_NUMERICHOST", libc::NI_NUMERICHOST as i64),
        ("NI_NUMERICSERV", libc::NI_NUMERICSERV as i64),
        ("MSG_PEEK", libc::MSG_PEEK as i64),
    ];
    #[cfg(unix)]
    {
        out.push(("AF_UNIX", libc::AF_UNIX as i64));
    }
    #[cfg(unix)]
    {
        if SOCK_NONBLOCK_FLAG != 0 {
            out.push(("SOCK_NONBLOCK", SOCK_NONBLOCK_FLAG as i64));
        }
        if SOCK_CLOEXEC_FLAG != 0 {
            out.push(("SOCK_CLOEXEC", SOCK_CLOEXEC_FLAG as i64));
        }
    }
    #[cfg(unix)]
    {
        out.push(("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64));
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        out.push(("SO_REUSEPORT", libc::SO_REUSEPORT as i64));
    }
    out.push(("EAI_AGAIN", libc::EAI_AGAIN as i64));
    out.push(("EAI_FAIL", libc::EAI_FAIL as i64));
    out.push(("EAI_FAMILY", libc::EAI_FAMILY as i64));
    out.push(("EAI_NONAME", libc::EAI_NONAME as i64));
    out.push(("EAI_SERVICE", libc::EAI_SERVICE as i64));
    out.push(("EAI_SOCKTYPE", libc::EAI_SOCKTYPE as i64));
    out
}

#[no_mangle]
pub extern "C" fn molt_errno_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &ERRNO_CONSTANTS_CACHE, || {
            let constants = collect_errno_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut reverse_pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                reverse_pairs.push(value_bits);
                reverse_pairs.push(name_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let reverse_ptr = alloc_dict_with_pairs(_py, &reverse_pairs);
            if reverse_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(dict_ptr).bits());
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let reverse_bits = MoltObject::from_ptr(reverse_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[dict_bits, reverse_bits]);
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, reverse_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_socket_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &SOCKET_CONSTANTS_CACHE, || {
            let constants = socket_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dict_bits
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return default_bits,
        };
        #[cfg(target_arch = "wasm32")]
        {
            let Some(bytes) = wasm_env_get_bytes(&key) else {
                return default_bits;
            };
            let Ok(val) = std::str::from_utf8(&bytes) else {
                return default_bits;
            };
            let ptr = alloc_string(_py, val.as_bytes());
            if ptr.is_null() {
                default_bits
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            match std::env::var(key) {
                Ok(val) => {
                    let ptr = alloc_string(_py, val.as_bytes());
                    if ptr.is_null() {
                        default_bits
                    } else {
                        MoltObject::from_ptr(ptr).bits()
                    }
                }
                Err(_) => default_bits,
            }
        }
    })
}

#[cfg(target_arch = "wasm32")]
fn wasm_env_get_bytes(key: &str) -> Option<Vec<u8>> {
    let mut env_count = 0u32;
    let mut buf_size = 0u32;
    let rc = unsafe { environ_sizes_get(&mut env_count, &mut buf_size) };
    if rc != 0 || env_count == 0 || buf_size == 0 {
        return None;
    }
    let env_count = usize::try_from(env_count).ok()?;
    let buf_size = usize::try_from(buf_size).ok()?;
    let mut ptrs = vec![std::ptr::null_mut(); env_count];
    let mut buf = vec![0u8; buf_size];
    let rc = unsafe { environ_get(ptrs.as_mut_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let base = buf.as_ptr();
    let key_bytes = key.as_bytes();
    for ptr in ptrs {
        if ptr.is_null() {
            continue;
        }
        let offset = unsafe { ptr.offset_from(base) };
        if offset < 0 {
            continue;
        }
        let offset = offset as usize;
        if offset >= buf.len() {
            continue;
        }
        let slice = &buf[offset..];
        let end = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
        let entry = &slice[..end];
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        if &entry[..eq] == key_bytes {
            return Some(entry[eq + 1..].to_vec());
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}
