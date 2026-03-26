//! Network utility functions: socketpair, getaddrinfo, getnameinfo, gethostname,
//! gethostbyname, inet_pton/ntop, byte-order helpers, interface utilities,
//! sendfile, AF_ALG, etc.
//!
//! Split from sockets.rs to reduce file size.

use super::channels::has_capability;
use crate::PyToken;
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
#[cfg(molt_has_net_io)]
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::ffi::{CStr, CString};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(molt_has_net_io)]
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::BorrowedFd;
use std::os::raw::{c_int, c_void};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, RawSocket};
use std::sync::Mutex;
use std::time::Duration;

use super::sockets::{
    host_from_bits, iter_values_from_bits, libc_socket, port_from_bits,
    service_from_bits, sock_addr_from_storage, sockaddr_from_bits, sockaddr_to_bits,
    socket_timeout, socket_wait_ready, with_socket_mut,
};
#[cfg(target_arch = "wasm32")]
use super::sockets::{decode_sockaddr, errno_from_rc, wasm_socket_meta_insert};
#[cfg(all(molt_has_net_io, not(unix)))]
use super::sockets::socket_register_peer_pair;
#[cfg(all(molt_has_net_io, windows))]
use super::sockets::{socket_close_raw_windows, socketpair_windows_loopback_raw};

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socketpair(family_bits: u64, type_bits: u64, proto_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
                .is_err()
            {
                return MoltObject::none().bits();
            }
            let family = if obj_from_bits(family_bits).is_none() {
                #[cfg(unix)]
                {
                    libc::AF_UNIX
                }
                #[cfg(not(unix))]
                {
                    libc::AF_INET
                }
            } else {
                match to_i64(obj_from_bits(family_bits)) {
                    Some(val) => val as i32,
                    None => raise_exception::<_>(_py, "TypeError", "family must be int or None"),
                }
            };
            let sock_type = if obj_from_bits(type_bits).is_none() {
                libc::SOCK_STREAM
            } else {
                match to_i64(obj_from_bits(type_bits)) {
                    Some(val) => val as i32,
                    None => raise_exception::<_>(_py, "TypeError", "type must be int or None"),
                }
            };
            let proto = if obj_from_bits(proto_bits).is_none() {
                0
            } else {
                match to_i64(obj_from_bits(proto_bits)) {
                    Some(val) => val as i32,
                    None => raise_exception::<_>(_py, "TypeError", "proto must be int or None"),
                }
            };
            #[cfg(unix)]
            {
                if family != libc::AF_UNIX {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EAFNOSUPPORT as i64,
                        "socketpair family",
                    );
                }
                let mut fds = [0 as libc::c_int; 2];
                let ret = libc::socketpair(family, sock_type, proto, fds.as_mut_ptr());
                if ret != 0 {
                    return raise_os_error::<u64>(
                        _py,
                        std::io::Error::last_os_error(),
                        "socketpair",
                    );
                }
                let left_bits = molt_socket_new(
                    MoltObject::from_int(family as i64).bits(),
                    MoltObject::from_int(sock_type as i64).bits(),
                    MoltObject::from_int(proto as i64).bits(),
                    MoltObject::from_int(fds[0] as i64).bits(),
                );
                let right_bits = molt_socket_new(
                    MoltObject::from_int(family as i64).bits(),
                    MoltObject::from_int(sock_type as i64).bits(),
                    MoltObject::from_int(proto as i64).bits(),
                    MoltObject::from_int(fds[1] as i64).bits(),
                );
                let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            #[cfg(windows)]
            {
                if family != libc::AF_INET && family != libc::AF_INET6 {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EAFNOSUPPORT as i64,
                        "socketpair family",
                    );
                }
                if sock_type != libc::SOCK_STREAM {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EPROTOTYPE as i64,
                        "socketpair type",
                    );
                }
                if proto != 0 && proto != libc::IPPROTO_TCP {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EPROTONOSUPPORT as i64,
                        "socketpair proto",
                    );
                }
                // NOTE(runtime/windows): `socketpair` currently uses loopback TCP to preserve
                // deterministic behavior. The native WSAPROTOCOL_INFO/AF_UNIX perf lane remains
                // tracked in docs/spec/STATUS.md and ROADMAP.md (RT2/P2).
                let (left_fd, right_fd) = match socketpair_windows_loopback_raw(family) {
                    Ok(pair) => pair,
                    Err(err) => return raise_os_error::<u64>(_py, err, "socketpair"),
                };
                let left_bits = molt_socket_new(
                    MoltObject::from_int(family as i64).bits(),
                    MoltObject::from_int(sock_type as i64).bits(),
                    MoltObject::from_int(proto as i64).bits(),
                    MoltObject::from_int(left_fd as i64).bits(),
                );
                if obj_from_bits(left_bits).is_none() {
                    socket_close_raw_windows(right_fd);
                    return MoltObject::none().bits();
                }
                let right_bits = molt_socket_new(
                    MoltObject::from_int(family as i64).bits(),
                    MoltObject::from_int(sock_type as i64).bits(),
                    MoltObject::from_int(proto as i64).bits(),
                    MoltObject::from_int(right_fd as i64).bits(),
                );
                if obj_from_bits(right_bits).is_none() {
                    let _ = molt_socket_drop(left_bits);
                    return MoltObject::none().bits();
                }
                socket_register_peer_pair(left_fd, right_fd);
                let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
                if tuple_ptr.is_null() {
                    let _ = molt_socket_drop(left_bits);
                    let _ = molt_socket_drop(right_bits);
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socketpair(_family_bits: u64, _type_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = if obj_from_bits(_family_bits).is_none() {
            #[cfg(unix)]
            {
                libc::AF_UNIX
            }
            #[cfg(not(unix))]
            {
                libc::AF_INET
            }
        } else {
            match to_i64(obj_from_bits(_family_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
            }
        };
        let sock_type = if obj_from_bits(_type_bits).is_none() {
            libc::SOCK_STREAM
        } else {
            match to_i64(obj_from_bits(_type_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
            }
        };
        let proto = if obj_from_bits(_proto_bits).is_none() {
            0
        } else {
            match to_i64(obj_from_bits(_proto_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "proto must be int"),
            }
        };
        let mut left: u64 = 0;
        let mut right: u64 = 0;
        let rc = unsafe {
            crate::molt_socket_socketpair_host(
                family,
                sock_type,
                proto,
                (&mut left) as *mut u64 as u32,
                (&mut right) as *mut u64 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "socketpair");
        }
        wasm_socket_meta_insert(
            left as i64,
            WasmSocketMeta {
                family,
                sock_type,
                proto,
                timeout: None,
                connect_pending: false,
            },
        );
        wasm_socket_meta_insert(
            right as i64,
            WasmSocketMeta {
                family,
                sock_type,
                proto,
                timeout: None,
                connect_pending: false,
            },
        );
        let left_bits = MoltObject::from_int(left as i64).bits();
        let right_bits = MoltObject::from_int(right as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getaddrinfo(
    host_bits: u64,
    port_bits: u64,
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    flags_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.connect", "net.bind", "net"])
                .is_err()
            {
                return MoltObject::none().bits();
            }
            let host = match host_from_bits(_py, host_bits) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let service = match service_from_bits(_py, port_bits) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
            let sock_type = to_i64(obj_from_bits(type_bits)).unwrap_or(0) as i32;
            let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;

            let host_cstr = host
                .as_ref()
                .and_then(|val| CString::new(val.as_str()).ok());
            let service_cstr = service
                .as_ref()
                .and_then(|val| CString::new(val.as_str()).ok());
            if host.is_some() && host_cstr.is_none() {
                return raise_exception::<u64>(_py, "TypeError", "host contains NUL byte");
            }
            if service.is_some() && service_cstr.is_none() {
                return raise_exception::<u64>(_py, "TypeError", "service contains NUL byte");
            }
            let mut hints: libc::addrinfo = std::mem::zeroed();
            hints.ai_family = family;
            hints.ai_socktype = sock_type;
            hints.ai_protocol = proto;
            hints.ai_flags = flags;

            let mut res: *mut libc::addrinfo = std::ptr::null_mut();
            let err = libc::getaddrinfo(
                host_cstr
                    .as_ref()
                    .map(|s| s.as_ptr())
                    .unwrap_or(std::ptr::null()),
                service_cstr
                    .as_ref()
                    .map(|s| s.as_ptr())
                    .unwrap_or(std::ptr::null()),
                &hints as *const libc::addrinfo,
                &mut res as *mut *mut libc::addrinfo,
            );
            if err != 0 {
                let msg = CStr::from_ptr(libc::gai_strerror(err))
                    .to_string_lossy()
                    .to_string();
                let msg = format!("[Errno {err}] {msg}");
                return raise_os_error_errno::<u64>(_py, err as i64, &msg);
            }

            let builder_bits = molt_list_builder_new(MoltObject::from_int(0).bits());
            if builder_bits == 0 {
                libc::freeaddrinfo(res);
                return MoltObject::none().bits();
            }
            let mut cur = res;
            while !cur.is_null() {
                let ai = &*cur;
                let mut storage: libc::sockaddr_storage = std::mem::zeroed();
                let len = ai.ai_addrlen;
                std::ptr::copy_nonoverlapping(
                    ai.ai_addr as *const u8,
                    &mut storage as *mut _ as *mut u8,
                    len as usize,
                );
                let sockaddr = sock_addr_from_storage(storage, len);
                let sockaddr_bits = sockaddr_to_bits(_py, &sockaddr);
                let canon_bits = if !ai.ai_canonname.is_null() {
                    let name = CStr::from_ptr(ai.ai_canonname).to_string_lossy();
                    let ptr = alloc_string(_py, name.as_bytes());
                    if ptr.is_null() {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_ptr(ptr).bits()
                    }
                } else {
                    MoltObject::none().bits()
                };
                let family_bits = MoltObject::from_int(ai.ai_family as i64).bits();
                let sock_type_bits = MoltObject::from_int(ai.ai_socktype as i64).bits();
                let proto_bits = MoltObject::from_int(ai.ai_protocol as i64).bits();
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[
                        family_bits,
                        sock_type_bits,
                        proto_bits,
                        canon_bits,
                        sockaddr_bits,
                    ],
                );
                if tuple_ptr.is_null() {
                    dec_ref_bits(_py, canon_bits);
                    dec_ref_bits(_py, sockaddr_bits);
                    break;
                }
                let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                molt_list_builder_append(builder_bits, tuple_bits);
                dec_ref_bits(_py, canon_bits);
                dec_ref_bits(_py, sockaddr_bits);
                cur = ai.ai_next;
            }
            libc::freeaddrinfo(res);
            molt_list_builder_finish_owned(builder_bits)
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getaddrinfo(
    _host_bits: u64,
    _port_bits: u64,
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let host = match host_from_bits(_py, _host_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let service = match service_from_bits(_py, _port_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let sock_type = to_i64(obj_from_bits(_type_bits)).unwrap_or(0) as i32;
        let proto = to_i64(obj_from_bits(_proto_bits)).unwrap_or(0) as i32;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        let host_bytes = host.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        let service_bytes = service.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if host_bytes.contains(&0) {
            return raise_exception::<_>(_py, "TypeError", "host contains NUL byte");
        }
        if service_bytes.contains(&0) {
            return raise_exception::<_>(_py, "TypeError", "service contains NUL byte");
        }
        let mut cap = 4096usize;
        let mut buf = vec![0u8; cap];
        let mut out_len: u32 = 0;
        let mut fallback_numeric = false;
        loop {
            let rc = unsafe {
                crate::molt_socket_getaddrinfo_host(
                    host_bytes.as_ptr() as u32,
                    host_bytes.len() as u32,
                    service_bytes.as_ptr() as u32,
                    service_bytes.len() as u32,
                    family,
                    sock_type,
                    proto,
                    flags,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
            if rc == 0 {
                break;
            }
            let errno = errno_from_rc(rc);
            if errno == libc::ENOSYS {
                fallback_numeric = true;
                break;
            }
            if errno == libc::ENOMEM && out_len as usize > cap {
                cap = out_len as usize;
                buf.resize(cap, 0);
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "getaddrinfo");
        }
        if fallback_numeric {
            let ip = if let Some(host) = host.as_ref() {
                match host.parse::<IpAddr>() {
                    Ok(ip) => ip,
                    Err(_) => {
                        return raise_os_error_errno::<u64>(
                            _py,
                            libc::EAI_NONAME as i64,
                            "name or service not known",
                        );
                    }
                }
            } else if family == libc::AF_INET6 {
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::UNSPECIFIED)
            };
            let port = match service.as_ref() {
                Some(val) => match val.parse::<u16>() {
                    Ok(port) => port,
                    Err(_) => {
                        return raise_exception::<_>(_py, "TypeError", "service must be int");
                    }
                },
                None => 0,
            };
            let sockaddr = match ip {
                IpAddr::V4(ip) => {
                    let addr = Ipv4Addr::from(ip.octets());
                    let host = addr.to_string();
                    let host_ptr = alloc_string(_py, host.as_bytes());
                    if host_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let host_bits = MoltObject::from_ptr(host_ptr).bits();
                    let port_bits = MoltObject::from_int(port as i64).bits();
                    let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
                    dec_ref_bits(_py, host_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
                IpAddr::V6(ip) => {
                    let addr = Ipv6Addr::from(ip.octets());
                    let host = addr.to_string();
                    let host_ptr = alloc_string(_py, host.as_bytes());
                    if host_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let host_bits = MoltObject::from_ptr(host_ptr).bits();
                    let port_bits = MoltObject::from_int(port as i64).bits();
                    let flow_bits = MoltObject::from_int(0).bits();
                    let scope_bits = MoltObject::from_int(0).bits();
                    let tuple_ptr =
                        alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
                    dec_ref_bits(_py, host_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            };
            let family_bits = MoltObject::from_int(family as i64).bits();
            let sock_type_bits = MoltObject::from_int(sock_type as i64).bits();
            let proto_bits = MoltObject::from_int(proto as i64).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    family_bits,
                    sock_type_bits,
                    proto_bits,
                    MoltObject::none().bits(),
                    sockaddr,
                ],
            );
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
            if list_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
                return MoltObject::none().bits();
            }
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
            return MoltObject::from_ptr(list_ptr).bits();
        }
        let data = &buf[..out_len as usize];
        if data.len() < 4 {
            return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
        }
        let mut offset = 0usize;
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        offset += 4;
        let mut out: Vec<u64> = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 12 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let family = i32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let sock_type = i32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let proto = i32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            offset += 12;
            if offset + 4 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let canon_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + canon_len > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let canon_bits = if canon_len > 0 {
                let ptr = alloc_string(_py, &data[offset..offset + canon_len]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            } else {
                MoltObject::none().bits()
            };
            offset += canon_len;
            if offset + 4 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let addr_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + addr_len > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let sockaddr_bits = match decode_sockaddr(_py, &data[offset..offset + addr_len]) {
                Ok(bits) => bits,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            offset += addr_len;
            let family_bits = MoltObject::from_int(family as i64).bits();
            let sock_type_bits = MoltObject::from_int(sock_type as i64).bits();
            let proto_bits = MoltObject::from_int(proto as i64).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    family_bits,
                    sock_type_bits,
                    proto_bits,
                    canon_bits,
                    sockaddr_bits,
                ],
            );
            dec_ref_bits(_py, canon_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &out);
        if list_ptr.is_null() {
            for bits in out {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        for bits in out {
            dec_ref_bits(_py, bits);
        }
        list_bits
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getnameinfo(addr_bits: u64, flags_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
            let obj = obj_from_bits(addr_bits);
            let Some(ptr) = obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
            };
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
            }
            let elems = seq_vec_ref(ptr);
            let family = if elems.len() >= 4 {
                libc::AF_INET6
            } else {
                libc::AF_INET
            };
            let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
                Ok(addr) => addr,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let mut host_buf = vec![0u8; libc::NI_MAXHOST as usize + 1];
            let mut serv_buf = vec![0u8; libc::NI_MAXSERV as usize + 1];
            let ret = libc::getnameinfo(
                sockaddr.as_ptr() as *const libc::sockaddr,
                sockaddr.len(),
                host_buf.as_mut_ptr() as *mut libc::c_char,
                host_buf.len() as libc::socklen_t,
                serv_buf.as_mut_ptr() as *mut libc::c_char,
                serv_buf.len() as libc::socklen_t,
                flags,
            );
            if ret != 0 {
                let msg = CStr::from_ptr(libc::gai_strerror(ret))
                    .to_string_lossy()
                    .to_string();
                let msg = format!("[Errno {ret}] {msg}");
                return raise_os_error_errno::<u64>(_py, ret as i64, &msg);
            }
            let host = CStr::from_ptr(host_buf.as_ptr() as *const libc::c_char).to_string_lossy();
            let serv = CStr::from_ptr(serv_buf.as_ptr() as *const libc::c_char).to_string_lossy();
            let host_ptr = alloc_string(_py, host.as_bytes());
            if host_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let serv_ptr = alloc_string(_py, serv.as_bytes());
            if serv_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(host_ptr).bits());
                return MoltObject::none().bits();
            }
            let host_bits = MoltObject::from_ptr(host_ptr).bits();
            let serv_bits = MoltObject::from_ptr(serv_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[host_bits, serv_bits]);
            dec_ref_bits(_py, host_bits);
            dec_ref_bits(_py, serv_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getnameinfo(_addr_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_addr_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        if elems.len() < 2 {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be (host, port)");
        }
        let host = match host_from_bits(_py, elems[0]) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "host cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let port = match port_from_bits(_py, elems[1]) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let serv_ptr = alloc_string(_py, port.to_string().as_bytes());
        if serv_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(host_ptr).bits());
            return MoltObject::none().bits();
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let serv_bits = MoltObject::from_ptr(serv_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, serv_bits]);
        dec_ref_bits(_py, host_bits);
        dec_ref_bits(_py, serv_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_gethostname() -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let mut buf = vec![0u8; 256];
            let ret = libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len());
            if ret != 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "gethostname");
            }
            if let Some(pos) = buf.iter().position(|b| *b == 0) {
                buf.truncate(pos);
            }
            let ptr = alloc_string(_py, &buf);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_gethostname() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut buf = vec![0u8; 256];
        let mut out_len: u32 = 0;
        let mut rc = unsafe {
            crate::molt_socket_gethostname_host(
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc == -libc::ENOMEM && out_len as usize > buf.len() {
            buf.resize(out_len as usize, 0);
            rc = unsafe {
                crate::molt_socket_gethostname_host(
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
        }
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "gethostname");
        }
        let len = out_len as usize;
        let ptr = alloc_string(_py, &buf[..len]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[inline]
fn socket_af_unspec() -> i32 {
    #[cfg(target_arch = "wasm32")]
    { 0 }
    #[cfg(molt_has_net_io)]
    { libc::AF_UNSPEC }
    #[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
    { 0 }
}

#[inline]
fn socket_getaddrinfo_call(
    host_bits: u64,
    port_bits: u64,
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    flags_bits: u64,
) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        molt_socket_getaddrinfo(
            host_bits,
            port_bits,
            family_bits,
            type_bits,
            proto_bits,
            flags_bits,
        )
    }
    #[cfg(molt_has_net_io)]
    {
        unsafe {
            molt_socket_getaddrinfo(
                host_bits,
                port_bits,
                family_bits,
                type_bits,
                proto_bits,
                flags_bits,
            )
        }
    }
    #[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
    {
        crate::molt_socket_getaddrinfo(host_bits, port_bits, family_bits, type_bits, proto_bits, flags_bits)
    }
}

#[inline]
fn socket_gethostname_call() -> u64 {
    #[cfg(target_arch = "wasm32")]
    { molt_socket_gethostname() }
    #[cfg(molt_has_net_io)]
    { unsafe { molt_socket_gethostname() } }
    #[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]
    { crate::molt_socket_gethostname() }
}

fn socket_addrinfo_first_host_bits(_py: &PyToken<'_>, info_bits: u64) -> Result<u64, u64> {
    let Some(info_ptr) = obj_from_bits(info_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "getaddrinfo returned invalid value",
        ));
    };
    let info_type = unsafe { object_type_id(info_ptr) };
    if info_type != TYPE_ID_LIST && info_type != TYPE_ID_TUPLE {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "getaddrinfo returned invalid value",
        ));
    }
    let info_entries = unsafe { seq_vec_ref(info_ptr) };
    for entry_bits in info_entries {
        let Some(entry_ptr) = obj_from_bits(*entry_bits).as_ptr() else {
            continue;
        };
        let entry_type = unsafe { object_type_id(entry_ptr) };
        if entry_type != TYPE_ID_LIST && entry_type != TYPE_ID_TUPLE {
            continue;
        }
        let entry = unsafe { seq_vec_ref(entry_ptr) };
        if entry.len() < 5 {
            continue;
        }
        let Some(sockaddr_ptr) = obj_from_bits(entry[4]).as_ptr() else {
            continue;
        };
        let sockaddr_type = unsafe { object_type_id(sockaddr_ptr) };
        if sockaddr_type != TYPE_ID_LIST && sockaddr_type != TYPE_ID_TUPLE {
            continue;
        }
        let sockaddr = unsafe { seq_vec_ref(sockaddr_ptr) };
        if sockaddr.is_empty() {
            continue;
        }
        let host_bits = sockaddr[0];
        if string_obj_to_owned(obj_from_bits(host_bits)).is_some() {
            inc_ref_bits(_py, host_bits);
            return Ok(host_bits);
        }
    }
    Err(raise_os_error_errno::<u64>(
        _py,
        libc::EAI_NONAME as i64,
        "gethostbyname failed",
    ))
}

fn socket_tuple_first_string(_py: &PyToken<'_>, value_bits: u64) -> Option<String> {
    let ptr = obj_from_bits(value_bits).as_ptr()?;
    let value_type = unsafe { object_type_id(ptr) };
    if value_type != TYPE_ID_LIST && value_type != TYPE_ID_TUPLE {
        return None;
    }
    let items = unsafe { seq_vec_ref(ptr) };
    if items.is_empty() {
        return None;
    }
    string_obj_to_owned(obj_from_bits(items[0]))
}

fn socket_push_unique(values: &mut Vec<String>, value: String) {
    if value.is_empty() {
        return;
    }
    if values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
}

fn socket_alloc_string_list(_py: &PyToken<'_>, values: &[String]) -> Option<u64> {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        item_bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, &item_bits);
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn socket_collect_reverse_lookup_details(
    _py: &PyToken<'_>,
    query: &str,
    primary_name: &str,
    family_hint: Option<i32>,
) -> (Vec<String>, Vec<String>) {
    let mut aliases: Vec<String> = Vec::new();
    let mut addresses: Vec<String> = Vec::new();
    let query_ptr = alloc_string(_py, query.as_bytes());
    if query_ptr.is_null() {
        return (aliases, addresses);
    }
    let query_bits = MoltObject::from_ptr(query_ptr).bits();
    let none_bits = MoltObject::none().bits();
    let family_bits = MoltObject::from_int(family_hint.unwrap_or(socket_af_unspec()) as i64).bits();
    let zero_bits = MoltObject::from_int(0).bits();
    let info_bits = socket_getaddrinfo_call(
        query_bits,
        none_bits,
        family_bits,
        zero_bits,
        zero_bits,
        zero_bits,
    );
    dec_ref_bits(_py, query_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return (aliases, addresses);
    }
    let Some(info_ptr) = obj_from_bits(info_bits).as_ptr() else {
        if !obj_from_bits(info_bits).is_none() {
            dec_ref_bits(_py, info_bits);
        }
        return (aliases, addresses);
    };
    let info_type = unsafe { object_type_id(info_ptr) };
    if info_type != TYPE_ID_LIST && info_type != TYPE_ID_TUPLE {
        dec_ref_bits(_py, info_bits);
        return (aliases, addresses);
    }
    let entries = unsafe { seq_vec_ref(info_ptr) };
    for entry_bits in entries {
        let Some(entry_ptr) = obj_from_bits(*entry_bits).as_ptr() else {
            continue;
        };
        let entry_type = unsafe { object_type_id(entry_ptr) };
        if entry_type != TYPE_ID_LIST && entry_type != TYPE_ID_TUPLE {
            continue;
        }
        let entry = unsafe { seq_vec_ref(entry_ptr) };
        if entry.len() < 5 {
            continue;
        }
        if let Some(canon) = string_obj_to_owned(obj_from_bits(entry[3]))
            && canon != primary_name
            && canon.parse::<IpAddr>().is_err()
        {
            socket_push_unique(&mut aliases, canon);
        }
        let Some(sockaddr_ptr) = obj_from_bits(entry[4]).as_ptr() else {
            continue;
        };
        let sockaddr_type = unsafe { object_type_id(sockaddr_ptr) };
        if sockaddr_type != TYPE_ID_LIST && sockaddr_type != TYPE_ID_TUPLE {
            continue;
        }
        let sockaddr = unsafe { seq_vec_ref(sockaddr_ptr) };
        if sockaddr.is_empty() {
            continue;
        }
        let Some(host) = string_obj_to_owned(obj_from_bits(sockaddr[0])) else {
            continue;
        };
        if host.parse::<IpAddr>().is_ok() {
            socket_push_unique(&mut addresses, host);
        } else if host != primary_name {
            socket_push_unique(&mut aliases, host);
        }
    }
    dec_ref_bits(_py, info_bits);

    if addresses.is_empty() {
        let primary_ptr = alloc_string(_py, primary_name.as_bytes());
        if !primary_ptr.is_null() {
            let primary_bits = MoltObject::from_ptr(primary_ptr).bits();
            let fallback_bits = molt_socket_gethostbyname(primary_bits);
            dec_ref_bits(_py, primary_bits);
            if exception_pending(_py) {
                clear_exception(_py);
            } else if let Some(host) = string_obj_to_owned(obj_from_bits(fallback_bits))
                && host.parse::<IpAddr>().is_ok()
            {
                socket_push_unique(&mut addresses, host);
            }
            if !obj_from_bits(fallback_bits).is_none() {
                dec_ref_bits(_py, fallback_bits);
            }
        }
    }
    (aliases, addresses)
}

fn socket_reverse_pointer_name(addr: &IpAddr) -> String {
    match addr {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            format!(
                "{}.{}.{}.{}.in-addr.arpa",
                octets[3], octets[2], octets[1], octets[0]
            )
        }
        IpAddr::V6(v6) => {
            let mut out = String::new();
            for byte in v6.octets().iter().rev() {
                let lo = byte & 0x0F;
                let hi = byte >> 4;
                out.push(char::from(b"0123456789abcdef"[lo as usize]));
                out.push('.');
                out.push(char::from(b"0123456789abcdef"[hi as usize]));
                out.push('.');
            }
            out.push_str("ip6.arpa");
            out
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_gethostbyname(host_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family_bits = MoltObject::from_int(libc::AF_INET as i64).bits();
        let zero_bits = MoltObject::from_int(0).bits();
        let none_bits = MoltObject::none().bits();
        let info_bits = socket_getaddrinfo_call(
            host_bits,
            none_bits,
            family_bits,
            zero_bits,
            zero_bits,
            zero_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out = socket_addrinfo_first_host_bits(_py, info_bits);
        if !obj_from_bits(info_bits).is_none() {
            dec_ref_bits(_py, info_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_gethostbyname_ex(host_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let host = match host_from_bits(_py, host_bits) {
            Ok(Some(val)) => val,
            Ok(None) => {
                return raise_exception::<_>(_py, "TypeError", "host name cannot be None");
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };

        // Resolve the primary address first via gethostbyname.
        let primary_bits = molt_socket_gethostbyname(host_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let primary_ip = string_obj_to_owned(obj_from_bits(primary_bits)).unwrap_or_default();
        if !obj_from_bits(primary_bits).is_none() {
            dec_ref_bits(_py, primary_bits);
        }

        // Use getaddrinfo-based collection to gather aliases and all addresses.
        let canonical = host.clone();
        let (aliases, mut addresses) =
            socket_collect_reverse_lookup_details(_py, &host, &canonical, None);
        // Ensure the primary IP is in the addresses list.
        if !primary_ip.is_empty() {
            socket_push_unique(&mut addresses, primary_ip);
        }

        let host_name_ptr = alloc_string(_py, canonical.as_bytes());
        if host_name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let host_name_bits = MoltObject::from_ptr(host_name_ptr).bits();
        let Some(aliases_bits) = socket_alloc_string_list(_py, &aliases) else {
            dec_ref_bits(_py, host_name_bits);
            return MoltObject::none().bits();
        };
        let Some(addr_list_bits) = socket_alloc_string_list(_py, &addresses) else {
            dec_ref_bits(_py, host_name_bits);
            dec_ref_bits(_py, aliases_bits);
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(_py, &[host_name_bits, aliases_bits, addr_list_bits]);
        dec_ref_bits(_py, host_name_bits);
        dec_ref_bits(_py, aliases_bits);
        dec_ref_bits(_py, addr_list_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_gethostbyaddr(host_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let host = match host_from_bits(_py, host_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "host name cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let parsed_host_ip = host.parse::<IpAddr>().ok();
        let mut resolved = host.clone();

        if parsed_host_ip.is_some() {
            #[cfg(molt_has_net_io)]
            {
                let host_ptr = alloc_string(_py, host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_value_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(0).bits();
                let addr_tuple_ptr = alloc_tuple(_py, &[host_value_bits, port_bits]);
                dec_ref_bits(_py, host_value_bits);
                if addr_tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let addr_bits = MoltObject::from_ptr(addr_tuple_ptr).bits();
                let flags_bits = MoltObject::from_int(libc::NI_NAMEREQD as i64).bits();
                let nameinfo_bits = unsafe { molt_socket_getnameinfo(addr_bits, flags_bits) };
                dec_ref_bits(_py, addr_bits);
                if exception_pending(_py) {
                    clear_exception(_py);
                    return raise_exception::<_>(_py, "OSError", "host name lookup failure");
                }
                if let Some(hostname) = socket_tuple_first_string(_py, nameinfo_bits) {
                    resolved = hostname;
                }
                if !obj_from_bits(nameinfo_bits).is_none() {
                    dec_ref_bits(_py, nameinfo_bits);
                }
            }
        } else {
            // Match CPython behavior: unresolved/invalid non-IP inputs should surface getaddrinfo
            // errors (mapped to socket.gaierror in the Python wrapper).
            let query_ptr = alloc_string(_py, host.as_bytes());
            if query_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let query_bits = MoltObject::from_ptr(query_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let family_bits = MoltObject::from_int(socket_af_unspec() as i64).bits();
            let zero_bits = MoltObject::from_int(0).bits();
            let info_bits = socket_getaddrinfo_call(
                query_bits,
                none_bits,
                family_bits,
                zero_bits,
                zero_bits,
                zero_bits,
            );
            dec_ref_bits(_py, query_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }

            if let Some(info_ptr) = obj_from_bits(info_bits).as_ptr() {
                let info_type = unsafe { object_type_id(info_ptr) };
                if info_type == TYPE_ID_LIST || info_type == TYPE_ID_TUPLE {
                    let entries = unsafe { seq_vec_ref(info_ptr) };
                    for entry_bits in entries {
                        let Some(entry_ptr) = obj_from_bits(*entry_bits).as_ptr() else {
                            continue;
                        };
                        let entry_type = unsafe { object_type_id(entry_ptr) };
                        if entry_type != TYPE_ID_LIST && entry_type != TYPE_ID_TUPLE {
                            continue;
                        }
                        let entry = unsafe { seq_vec_ref(entry_ptr) };
                        if entry.len() < 4 {
                            continue;
                        }
                        if let Some(canon) = string_obj_to_owned(obj_from_bits(entry[3]))
                            && !canon.is_empty()
                        {
                            resolved = canon;
                            break;
                        }
                    }
                }
            }
            if !obj_from_bits(info_bits).is_none() {
                dec_ref_bits(_py, info_bits);
            }
        }

        let family_hint = parsed_host_ip.as_ref().map(|ip| match ip {
            IpAddr::V4(_) => libc::AF_INET,
            IpAddr::V6(_) => libc::AF_INET6,
        });
        let (mut aliases, mut addr_list) =
            socket_collect_reverse_lookup_details(_py, &resolved, &resolved, family_hint);
        if let Some(ip) = parsed_host_ip.as_ref() {
            socket_push_unique(&mut aliases, socket_reverse_pointer_name(ip));
        }
        if host != resolved && host.parse::<IpAddr>().is_err() {
            socket_push_unique(&mut aliases, host.clone());
        }
        if addr_list.is_empty() {
            if host.parse::<IpAddr>().is_ok() {
                socket_push_unique(&mut addr_list, host.clone());
            } else if resolved.parse::<IpAddr>().is_ok() {
                socket_push_unique(&mut addr_list, resolved.clone());
            }
        }
        if addr_list.is_empty() {
            socket_push_unique(&mut addr_list, host.clone());
        }

        let host_name_ptr = alloc_string(_py, resolved.as_bytes());
        if host_name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let host_name_bits = MoltObject::from_ptr(host_name_ptr).bits();
        let Some(aliases_bits) = socket_alloc_string_list(_py, &aliases) else {
            dec_ref_bits(_py, host_name_bits);
            return MoltObject::none().bits();
        };
        let Some(addr_list_bits) = socket_alloc_string_list(_py, &addr_list) else {
            dec_ref_bits(_py, host_name_bits);
            dec_ref_bits(_py, aliases_bits);
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(_py, &[host_name_bits, aliases_bits, addr_list_bits]);
        dec_ref_bits(_py, host_name_bits);
        dec_ref_bits(_py, aliases_bits);
        dec_ref_bits(_py, addr_list_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getfqdn(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut target = match host_from_bits(_py, name_bits) {
            Ok(value) => value.unwrap_or_default(),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if target.is_empty() || target == "0.0.0.0" {
            let hostname_bits = socket_gethostname_call();
            if exception_pending(_py) {
                clear_exception(_py);
                target.clear();
            } else if let Some(hostname) = string_obj_to_owned(obj_from_bits(hostname_bits)) {
                target = hostname;
            } else {
                target.clear();
            }
            if !obj_from_bits(hostname_bits).is_none() {
                dec_ref_bits(_py, hostname_bits);
            }
        }

        if target.is_empty() {
            let empty_ptr = alloc_string(_py, b"");
            if empty_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(empty_ptr).bits();
        }

        let target_ptr = alloc_string(_py, target.as_bytes());
        if target_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let target_bits = MoltObject::from_ptr(target_ptr).bits();
        let hostbyaddr_bits = molt_socket_gethostbyaddr(target_bits);
        dec_ref_bits(_py, target_bits);

        let fqdn = if exception_pending(_py) {
            clear_exception(_py);
            target
        } else {
            let mut selected = socket_tuple_first_string(_py, hostbyaddr_bits).unwrap_or(target);
            let selected_has_dot = selected.contains('.');
            if !selected_has_dot {
                let maybe_alias_with_dot = match obj_from_bits(hostbyaddr_bits).as_ptr() {
                    None => None,
                    Some(tuple_ptr) => {
                        let tuple_type = unsafe { object_type_id(tuple_ptr) };
                        if tuple_type != TYPE_ID_LIST && tuple_type != TYPE_ID_TUPLE {
                            None
                        } else {
                            let items = unsafe { seq_vec_ref(tuple_ptr) };
                            if items.len() < 2 {
                                None
                            } else {
                                match obj_from_bits(items[1]).as_ptr() {
                                    None => None,
                                    Some(alias_ptr) => {
                                        let alias_type = unsafe { object_type_id(alias_ptr) };
                                        if alias_type != TYPE_ID_LIST && alias_type != TYPE_ID_TUPLE
                                        {
                                            None
                                        } else {
                                            let alias_items = unsafe { seq_vec_ref(alias_ptr) };
                                            let mut found: Option<String> = None;
                                            for alias_bits in alias_items {
                                                if let Some(alias) =
                                                    string_obj_to_owned(obj_from_bits(*alias_bits))
                                                    && alias.contains('.')
                                                {
                                                    found = Some(alias);
                                                    break;
                                                }
                                            }
                                            found
                                        }
                                    }
                                }
                            }
                        }
                    }
                };
                if let Some(alias) = maybe_alias_with_dot {
                    selected = alias;
                }
            }
            selected
        };
        if !obj_from_bits(hostbyaddr_bits).is_none() {
            dec_ref_bits(_py, hostbyaddr_bits);
        }

        let out_ptr = alloc_string(_py, fqdn.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getservbyname(name_bits: u64, proto_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let name = match host_from_bits(_py, name_bits) {
                Ok(Some(val)) => val,
                Ok(None) => {
                    return raise_exception::<_>(_py, "TypeError", "service name cannot be None");
                }
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let proto = match host_from_bits(_py, proto_bits) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let name_cstr = CString::new(name).map_err(|_| ()).ok();
            if name_cstr.is_none() {
                return raise_exception::<_>(_py, "TypeError", "service name contains NUL byte");
            }
            let proto_cstr = proto
                .as_ref()
                .and_then(|val| CString::new(val.as_str()).ok());
            if proto.is_some() && proto_cstr.is_none() {
                return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
            }
            let serv = libc::getservbyname(
                name_cstr.as_ref().unwrap().as_ptr(),
                proto_cstr
                    .as_ref()
                    .map(|s| s.as_ptr())
                    .unwrap_or(std::ptr::null()),
            );
            if serv.is_null() {
                return raise_os_error_errno::<u64>(_py, libc::ENOENT as i64, "service not found");
            }
            let port = libc::ntohs((*serv).s_port as u16) as i64;
            MoltObject::from_int(port).bits()
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getservbyname(_name_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match host_from_bits(_py, _name_bits) {
            Ok(Some(val)) => val,
            Ok(None) => {
                return raise_exception::<_>(_py, "TypeError", "service name cannot be None");
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto = match host_from_bits(_py, _proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let name_bytes = name.as_bytes();
        let proto_bytes = proto.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if name_bytes.contains(&0) {
            return raise_exception::<_>(_py, "TypeError", "service name contains NUL byte");
        }
        if proto_bytes.contains(&0) {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let rc = unsafe {
            crate::molt_socket_getservbyname_host(
                name_bytes.as_ptr() as u32,
                name_bytes.len() as u32,
                proto_bytes.as_ptr() as u32,
                proto_bytes.len() as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getservbyname");
        }
        MoltObject::from_int(rc as i64).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getservbyport(port_bits: u64, proto_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let port = match to_i64(obj_from_bits(port_bits)) {
                Some(val) if val >= 0 && val <= u16::MAX as i64 => val as u16,
                _ => return raise_exception::<_>(_py, "TypeError", "port must be int"),
            };
            let proto = match host_from_bits(_py, proto_bits) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let proto_cstr = proto
                .as_ref()
                .and_then(|val| CString::new(val.as_str()).ok());
            if proto.is_some() && proto_cstr.is_none() {
                return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
            }
            let serv = libc::getservbyport(
                libc::htons(port) as i32,
                proto_cstr
                    .as_ref()
                    .map(|s| s.as_ptr())
                    .unwrap_or(std::ptr::null()),
            );
            if serv.is_null() {
                return raise_os_error_errno::<u64>(_py, libc::ENOENT as i64, "service not found");
            }
            let name = CStr::from_ptr((*serv).s_name).to_string_lossy();
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getservbyport(_port_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let port = match to_i64(obj_from_bits(_port_bits)) {
            Some(val) if val >= 0 && val <= u16::MAX as i64 => val as u16,
            _ => return raise_exception::<_>(_py, "TypeError", "port must be int"),
        };
        let proto = match host_from_bits(_py, _proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto_bytes = proto.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if proto_bytes.contains(&0) {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let mut buf = vec![0u8; 256];
        let mut out_len: u32 = 0;
        let mut rc = unsafe {
            crate::molt_socket_getservbyport_host(
                port as i32,
                proto_bytes.as_ptr() as u32,
                proto_bytes.len() as u32,
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc == -libc::ENOMEM && out_len as usize > buf.len() {
            buf.resize(out_len as usize, 0);
            rc = unsafe {
                crate::molt_socket_getservbyport_host(
                    port as i32,
                    proto_bytes.as_ptr() as u32,
                    proto_bytes.len() as u32,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
        }
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getservbyport");
        }
        let ptr = alloc_string(_py, &buf[..out_len as usize]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getprotobyname(name_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let name = match host_from_bits(_py, name_bits) {
                Ok(Some(val)) => val,
                Ok(None) => {
                    return raise_exception::<_>(_py, "TypeError", "protocol name cannot be None");
                }
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let name_cstr = match CString::new(name.as_str()) {
                Ok(cs) => cs,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "protocol name contains NUL byte",
                    );
                }
            };
            let proto = libc::getprotobyname(name_cstr.as_ptr());
            if proto.is_null() {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::ENOENT as i64,
                    &format!("protocol not found: '{name}'"),
                );
            }
            MoltObject::from_int((*proto).p_proto as i64).bits()
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_getprotobyname(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match host_from_bits(_py, name_bits) {
            Ok(Some(val)) => val,
            Ok(None) => {
                return raise_exception::<_>(_py, "TypeError", "protocol name cannot be None");
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let key = name.to_ascii_lowercase();
        let proto = match key.as_str() {
            "icmp" => 1,
            "igmp" => 2,
            "tcp" => 6,
            "udp" => 17,
            "ipv6" => 41,
            "gre" => 47,
            "esp" => 50,
            "ah" => 51,
            "icmpv6" => 58,
            "sctp" => 132,
            _ => {
                return raise_os_error_errno::<u64>(
                    _py,
                    22, // EINVAL
                    &format!("protocol not found: '{name}'"),
                );
            }
        };
        MoltObject::from_int(proto).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_inet_pton(family_bits: u64, address_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let addr = match host_from_bits(_py, address_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "address cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if family == libc::AF_INET {
            let ip: Ipv4Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv4 address",
                    );
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            let ip: Ipv6Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv6 address",
                    );
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_inet_pton(_family_bits: u64, _address_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let addr = match host_from_bits(_py, _address_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "address cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if family == libc::AF_INET {
            let ip: Ipv4Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv4 address",
                    );
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            let ip: Ipv6Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv6 address",
                    );
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_inet_ntop(family_bits: u64, packed_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
            let obj = obj_from_bits(packed_bits);
            let data = if let Some(ptr) = obj.as_ptr() {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let slice = std::slice::from_raw_parts(bytes_data(ptr), len);
                    slice.to_vec()
                } else if type_id == TYPE_ID_MEMORYVIEW {
                    if let Some(slice) = memoryview_bytes_slice(ptr) {
                        slice.to_vec()
                    } else if let Some(vec) = memoryview_collect_bytes(ptr) {
                        vec
                    } else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "packed address must be bytes-like",
                        );
                    }
                } else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "packed address must be bytes-like",
                    );
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "packed address must be bytes-like");
            };
            if family == libc::AF_INET {
                if data.len() != 4 {
                    return raise_exception::<_>(_py, "ValueError", "invalid IPv4 packed length");
                }
                let addr = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                let text = addr.to_string();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if family == libc::AF_INET6 {
                if data.len() != 16 {
                    return raise_exception::<_>(_py, "ValueError", "invalid IPv6 packed length");
                }
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[..16]);
                let addr = Ipv6Addr::from(octets);
                let text = addr.to_string();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            raise_exception::<_>(_py, "ValueError", "unsupported address family")
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_inet_ntop(_family_bits: u64, _packed_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_packed_bits);
        let data = if let Some(ptr) = obj.as_ptr() {
            let bytes = unsafe { bytes_like_slice_raw(ptr) };
            let Some(bytes) = bytes else {
                return raise_exception::<_>(_py, "TypeError", "packed must be bytes-like");
            };
            bytes.to_vec()
        } else {
            return raise_exception::<_>(_py, "TypeError", "packed must be bytes-like");
        };
        if family == libc::AF_INET {
            if data.len() != 4 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv4 packed length");
            }
            let mut octets = [0u8; 4];
            octets.copy_from_slice(&data[..4]);
            let addr = Ipv4Addr::from(octets);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            if data.len() != 16 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv6 packed length");
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[..16]);
            let addr = Ipv6Addr::from(octets);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

fn socket_u16_from_index(_py: &PyToken<'_>, value_bits: u64, func: &str) -> Option<u16> {
    let obj = obj_from_bits(value_bits);
    let err = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(_py, obj)
    );
    let value = index_bigint_from_obj(_py, value_bits, &err)?;
    if value.is_negative() {
        let msg = format!("{func}: can't convert negative Python int to C 16-bit unsigned integer");
        raise_exception::<Option<u16>>(_py, "OverflowError", &msg);
        return None;
    }
    if value > BigInt::from(u16::MAX) {
        let msg = format!("{func}: Python int too large to convert to C 16-bit unsigned integer");
        raise_exception::<Option<u16>>(_py, "OverflowError", &msg);
        return None;
    }
    let Some(out) = value.to_u16() else {
        // Should be unreachable after bounds checks, but keep error-shape deterministic.
        let msg = format!("{func}: Python int too large to convert to C 16-bit unsigned integer");
        raise_exception::<Option<u16>>(_py, "OverflowError", &msg);
        return None;
    };
    Some(out)
}

fn socket_u32_from_int_only(_py: &PyToken<'_>, value_bits: u64) -> Option<u32> {
    // CPython's socket.htonl/ntohl require `int` (not generic __index__).
    let obj = obj_from_bits(value_bits);
    let Some(value) = to_bigint(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, value_bits));
        let msg = format!("expected int, {type_name} found");
        raise_exception::<Option<u32>>(_py, "TypeError", &msg);
        return None;
    };
    if value.is_negative() {
        raise_exception::<Option<u32>>(
            _py,
            "OverflowError",
            "can't convert negative value to unsigned int",
        );
        return None;
    }
    if value > BigInt::from(u32::MAX) {
        raise_exception::<Option<u32>>(_py, "OverflowError", "int larger than 32 bits");
        return None;
    }
    let Some(out) = value.to_u32() else {
        raise_exception::<Option<u32>>(_py, "OverflowError", "int larger than 32 bits");
        return None;
    };
    Some(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_htons(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = socket_u16_from_index(_py, value_bits, "htons") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u16::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_ntohs(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = socket_u16_from_index(_py, value_bits, "ntohs") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u16::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_htonl(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = socket_u32_from_int_only(_py, value_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u32::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_ntohl(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = socket_u32_from_int_only(_py, value_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u32::to_be(value) as i64).bits()
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = std::net::TcpListener::bind("[::1]:0").is_ok();
        MoltObject::from_bool(supported).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = unsafe { crate::molt_socket_has_ipv6_host() };
        MoltObject::from_bool(supported != 0).bits()
    })
}

// --- if_nameindex / if_nametoindex / if_indextoname ---

#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_nameindex() -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let head = libc::if_nameindex();
            if head.is_null() {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "if_nameindex");
            }
            let mut items: Vec<u64> = Vec::new();
            let mut cur = head;
            while (*cur).if_index != 0 || !(*cur).if_name.is_null() {
                if (*cur).if_name.is_null() {
                    cur = cur.add(1);
                    continue;
                }
                let idx = (*cur).if_index as i64;
                let name = CStr::from_ptr((*cur).if_name);
                let name_bytes = name.to_bytes();
                let name_ptr = alloc_string(_py, name_bytes);
                if name_ptr.is_null() {
                    libc::if_freenameindex(head);
                    return MoltObject::none().bits();
                }
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_int(idx).bits(),
                        MoltObject::from_ptr(name_ptr).bits(),
                    ],
                );
                if tuple_ptr.is_null() {
                    libc::if_freenameindex(head);
                    return MoltObject::none().bits();
                }
                items.push(MoltObject::from_ptr(tuple_ptr).bits());
                cur = cur.add(1);
            }
            libc::if_freenameindex(head);
            let list_ptr = alloc_list(_py, &items);
            if list_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_nameindex() -> u64 {
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list(_py, &[]);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_nametoindex(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "if_nametoindex() argument must be a string",
                );
            }
        };
        let name_cstr = match CString::new(name) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "interface name contains NUL byte",
                );
            }
        };
        unsafe {
            let idx = libc::if_nametoindex(name_cstr.as_ptr());
            if idx == 0 {
                return raise_os_error::<u64>(
                    _py,
                    std::io::Error::last_os_error(),
                    "if_nametoindex",
                );
            }
            MoltObject::from_int(idx as i64).bits()
        }
    })
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_nametoindex(_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "OSError",
            "if_nametoindex is not supported on this platform",
        )
    })
}

#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_indextoname(index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let index = match to_i64(obj_from_bits(index_bits)) {
            Some(val) if val >= 0 && val <= u32::MAX as i64 => val as libc::c_uint,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "if_indextoname() argument out of range",
                );
            }
        };
        unsafe {
            let mut buf = [0u8; libc::IF_NAMESIZE];
            let ret = libc::if_indextoname(index, buf.as_mut_ptr() as *mut libc::c_char);
            if ret.is_null() {
                return raise_os_error::<u64>(
                    _py,
                    std::io::Error::last_os_error(),
                    "if_indextoname",
                );
            }
            let name = CStr::from_ptr(buf.as_ptr() as *const libc::c_char);
            let name_ptr = alloc_string(_py, name.to_bytes());
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(name_ptr).bits()
        }
    })
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_if_indextoname(_index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "OSError",
            "if_indextoname is not supported on this platform",
        )
    })
}

// --- CMSG_LEN / CMSG_SPACE ---

#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_len(datalen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let datalen = match to_i64(obj_from_bits(datalen_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "datalen must be an integer"),
        };
        if datalen < 0 {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_LEN() argument out of range",
            );
        }
        let Ok(datalen_u32) = u32::try_from(datalen) else {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_LEN() argument out of range",
            );
        };
        let result = unsafe { libc::CMSG_LEN(datalen_u32) } as i64;
        MoltObject::from_int(result).bits()
    })
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_len(datalen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let datalen = match to_i64(obj_from_bits(datalen_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "datalen must be an integer"),
        };
        if datalen < 0 {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_LEN() argument out of range",
            );
        }
        let header_size = std::mem::size_of::<usize>() * 3;
        let result = (header_size as i64) + datalen;
        MoltObject::from_int(result).bits()
    })
}

#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_space(datalen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let datalen = match to_i64(obj_from_bits(datalen_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "datalen must be an integer"),
        };
        if datalen < 0 {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_SPACE() argument out of range",
            );
        }
        let Ok(datalen_u32) = u32::try_from(datalen) else {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_SPACE() argument out of range",
            );
        };
        let result = unsafe { libc::CMSG_SPACE(datalen_u32) } as i64;
        MoltObject::from_int(result).bits()
    })
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_space(datalen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let datalen = match to_i64(obj_from_bits(datalen_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "datalen must be an integer"),
        };
        if datalen < 0 {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "CMSG_SPACE() argument out of range",
            );
        }
        let align = std::mem::size_of::<usize>();
        let header_size = align * 3;
        let total = header_size as i64 + datalen;
        let aligned = (total + (align as i64 - 1)) & !(align as i64 - 1);
        MoltObject::from_int(aligned).bits()
    })
}

// --- has_dualstack_ipv6 ---

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_has_dualstack_ipv6() -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = (|| -> bool {
            let sock = match Socket::new(Domain::IPV6, socket2::Type::STREAM, None) {
                Ok(s) => s,
                Err(_) => return false,
            };
            sock.set_only_v6(false).is_ok()
        })();
        MoltObject::from_bool(supported).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_has_dualstack_ipv6() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(false).bits() })
}

// --- send_fds / recv_fds high-level helpers ---

#[cfg(unix)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_send_fds(
    sock_bits: u64,
    buffers_bits: u64,
    fds_bits: u64,
    flags_bits: u64,
    address_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let fd_values = match iter_values_from_bits(_py, fds_bits) {
                Ok(vals) => vals,
                Err(bits) => return bits,
            };
            let mut raw_fds: Vec<i32> = Vec::with_capacity(fd_values.len());
            for fd_bits in &fd_values {
                match to_i64(obj_from_bits(*fd_bits)) {
                    Some(v) => raw_fds.push(v as i32),
                    None => {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "file descriptors must be integers",
                        );
                    }
                }
            }
            // Build SCM_RIGHTS ancillary data
            let packed_fds: Vec<u8> = raw_fds.iter().flat_map(|fd| fd.to_ne_bytes()).collect();
            let fds_bytes_ptr = alloc_bytes(_py, &packed_fds);
            if fds_bytes_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let fds_bytes_bits = MoltObject::from_ptr(fds_bytes_ptr).bits();
            let level_bits = MoltObject::from_int(libc::SOL_SOCKET as i64).bits();
            let type_bits = MoltObject::from_int(libc::SCM_RIGHTS as i64).bits();
            let anc_tuple = alloc_tuple(_py, &[level_bits, type_bits, fds_bytes_bits]);
            dec_ref_bits(_py, fds_bytes_bits);
            if anc_tuple.is_null() {
                return MoltObject::none().bits();
            }
            let anc_tuple_bits = MoltObject::from_ptr(anc_tuple).bits();
            let anc_list = alloc_list(_py, &[anc_tuple_bits]);
            dec_ref_bits(_py, anc_tuple_bits);
            if anc_list.is_null() {
                return MoltObject::none().bits();
            }
            let anc_list_bits = MoltObject::from_ptr(anc_list).bits();
            let result = molt_socket_sendmsg(
                sock_bits,
                buffers_bits,
                anc_list_bits,
                flags_bits,
                address_bits,
            );
            dec_ref_bits(_py, anc_list_bits);
            result
        })
    }
}

#[cfg(not(unix))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_send_fds(
    _sock_bits: u64,
    _buffers_bits: u64,
    _fds_bits: u64,
    _flags_bits: u64,
    _address_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "send_fds is not supported on this platform")
    })
}

#[cfg(unix)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recv_fds(
    sock_bits: u64,
    bufsize_bits: u64,
    maxfds_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxfds = match to_i64(obj_from_bits(maxfds_bits)) {
            Some(v) if v > 0 => v as usize,
            _ => return raise_exception::<u64>(_py, "ValueError", "maxfds must be at least 1"),
        };
        let fd_size = std::mem::size_of::<c_int>();
        let fds_payload_size = maxfds * fd_size;
        let Ok(payload_u32) = u32::try_from(fds_payload_size) else {
            return raise_exception::<u64>(_py, "OverflowError", "maxfds too large");
        };
        let ancbufsize = unsafe { libc::CMSG_SPACE(payload_u32) as i64 };
        let ancbufsize_bits = MoltObject::from_int(ancbufsize).bits();
        let result_bits =
            unsafe { molt_socket_recvmsg(sock_bits, bufsize_bits, ancbufsize_bits, flags_bits) };
        let result_ptr = match obj_from_bits(result_bits).as_ptr() {
            Some(p) => p,
            None => return result_bits,
        };
        let result_type = unsafe { object_type_id(result_ptr) };
        if result_type != TYPE_ID_TUPLE {
            return result_bits;
        }
        let parts = unsafe { seq_vec_ref(result_ptr) };
        if parts.len() != 4 {
            dec_ref_bits(_py, result_bits);
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "recvmsg returned unexpected result",
            );
        }
        let data_bits = parts[0];
        let ancdata_bits = parts[1];
        let msg_flags_bits = parts[2];
        let address_bits = parts[3];
        // Parse fds from SCM_RIGHTS ancillary data
        let mut received_fds: Vec<i32> = Vec::new();
        let anc_entries = match iter_values_from_bits(_py, ancdata_bits) {
            Ok(vals) => vals,
            Err(bits) => {
                dec_ref_bits(_py, result_bits);
                return bits;
            }
        };
        for entry_bits in &anc_entries {
            let Some(entry_ptr) = obj_from_bits(*entry_bits).as_ptr() else {
                continue;
            };
            let entry_parts = unsafe { seq_vec_ref(entry_ptr) };
            if entry_parts.len() != 3 {
                continue;
            }
            let level = to_i64(obj_from_bits(entry_parts[0])).unwrap_or(-1) as i32;
            let ctype = to_i64(obj_from_bits(entry_parts[1])).unwrap_or(-1) as i32;
            if level == libc::SOL_SOCKET && ctype == libc::SCM_RIGHTS {
                let payload_ptr = match obj_from_bits(entry_parts[2]).as_ptr() {
                    Some(p) => p,
                    None => continue,
                };
                let payload_type = unsafe { object_type_id(payload_ptr) };
                if payload_type == TYPE_ID_BYTES || payload_type == TYPE_ID_BYTEARRAY {
                    let data_len = unsafe { bytes_len(payload_ptr) };
                    let data_raw = unsafe { bytes_data(payload_ptr) };
                    let data = unsafe { std::slice::from_raw_parts(data_raw, data_len) };
                    let num_fds = data.len() / fd_size;
                    for i in 0..num_fds {
                        if received_fds.len() >= maxfds {
                            break;
                        }
                        let offset = i * fd_size;
                        if offset + fd_size <= data.len() {
                            let mut buf = [0u8; 4];
                            buf.copy_from_slice(&data[offset..offset + fd_size]);
                            received_fds.push(i32::from_ne_bytes(buf));
                        }
                    }
                }
            }
        }
        let mut fds_list: Vec<u64> = Vec::with_capacity(received_fds.len());
        for fd in &received_fds {
            fds_list.push(MoltObject::from_int(*fd as i64).bits());
        }
        let fds_list_ptr = alloc_list(_py, &fds_list);
        if fds_list_ptr.is_null() {
            dec_ref_bits(_py, result_bits);
            return MoltObject::none().bits();
        }
        let fds_list_bits = MoltObject::from_ptr(fds_list_ptr).bits();
        inc_ref_bits(_py, data_bits);
        inc_ref_bits(_py, msg_flags_bits);
        inc_ref_bits(_py, address_bits);
        let out = alloc_tuple(
            _py,
            &[data_bits, fds_list_bits, msg_flags_bits, address_bits],
        );
        dec_ref_bits(_py, result_bits);
        if out.is_null() {
            dec_ref_bits(_py, fds_list_bits);
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out).bits()
    })
}

#[cfg(not(unix))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recv_fds(
    _sock_bits: u64,
    _bufsize_bits: u64,
    _maxfds_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "recv_fds is not supported on this platform")
    })
}

// ---------------------------------------------------------------------------
// sendfile – efficient file-to-socket transmission
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "linux", molt_has_net_io))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendfile(
    sock_bits: u64,
    file_fd_bits: u64,
    offset_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "invalid socket handle");
        }
        let file_fd = match to_i64(obj_from_bits(file_fd_bits)) {
            Some(v) => v as c_int,
            None => return raise_exception::<u64>(_py, "TypeError", "file_fd must be an integer"),
        };
        let mut offset = to_i64(obj_from_bits(offset_bits)).unwrap_or(0) as libc::off_t;
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(0);
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);

        let mut total_sent: i64 = 0;
        loop {
            let chunk = if count > 0 {
                (count - total_sent).min(0x7fff_f000) as usize
            } else {
                0x7fff_f000_usize
            };
            if count > 0 && total_sent >= count {
                break;
            }
            let res = with_socket_mut(socket_ptr, |inner| {
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe { libc::sendfile(libc_socket(fd), file_fd, &mut offset, chunk) };
                if ret >= 0 {
                    Ok(ret as i64)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(0) => break,
                Ok(n) => total_sent += n,
                Err(err)
                    if err.raw_os_error() == Some(libc::EAGAIN)
                        || err.raw_os_error() == Some(libc::EWOULDBLOCK) =>
                {
                    if nonblocking {
                        if total_sent > 0 {
                            break;
                        }
                        return raise_os_error::<u64>(_py, err, "sendfile");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "sendfile");
                    }
                }
                Err(err) if err.raw_os_error() == Some(libc::EINTR) => continue,
                Err(err) => return raise_os_error::<u64>(_py, err, "sendfile"),
            }
        }
        MoltObject::from_int(total_sent).bits()
    })
}

#[cfg(all(target_os = "macos", molt_has_net_io))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendfile(
    sock_bits: u64,
    file_fd_bits: u64,
    offset_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "invalid socket handle");
        }
        let file_fd = match to_i64(obj_from_bits(file_fd_bits)) {
            Some(v) => v as c_int,
            None => return raise_exception::<u64>(_py, "TypeError", "file_fd must be an integer"),
        };
        let offset = to_i64(obj_from_bits(offset_bits)).unwrap_or(0) as libc::off_t;
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(0);
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);

        let mut total_sent: i64 = 0;
        let mut cur_offset = offset;
        loop {
            let chunk: libc::off_t = if count > 0 {
                (count - total_sent).min(0x7fff_f000) as libc::off_t
            } else {
                0 // macOS: 0 means "send until EOF"
            };
            if count > 0 && total_sent >= count {
                break;
            }
            let mut len: libc::off_t = chunk;
            let res = with_socket_mut(socket_ptr, |inner| {
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                // macOS sendfile: sendfile(in_fd, out_fd, offset, &mut len, hdtr, flags)
                let ret = unsafe {
                    libc::sendfile(
                        file_fd,
                        libc_socket(fd),
                        cur_offset,
                        &mut len,
                        std::ptr::null_mut(),
                        0,
                    )
                };
                if ret == 0 || (ret == -1 && len > 0) {
                    // macOS: sendfile can return -1 with EAGAIN but still have sent data
                    Ok(len as i64)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(0) => break,
                Ok(n) => {
                    total_sent += n;
                    cur_offset += n as libc::off_t;
                }
                Err(err)
                    if err.raw_os_error() == Some(libc::EAGAIN)
                        || err.raw_os_error() == Some(libc::EWOULDBLOCK) =>
                {
                    if nonblocking {
                        if total_sent > 0 {
                            break;
                        }
                        return raise_os_error::<u64>(_py, err, "sendfile");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "sendfile");
                    }
                }
                Err(err) if err.raw_os_error() == Some(libc::EINTR) => continue,
                Err(err) => return raise_os_error::<u64>(_py, err, "sendfile"),
            }
        }
        MoltObject::from_int(total_sent).bits()
    })
}

#[cfg(all(
    unix,
    molt_has_net_io,
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(target_arch = "wasm32")
))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendfile(
    sock_bits: u64,
    file_fd_bits: u64,
    offset_bits: u64,
    count_bits: u64,
) -> u64 {
    // Fallback: read from file fd + send to socket in chunks
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "invalid socket handle");
        }
        let file_fd = match to_i64(obj_from_bits(file_fd_bits)) {
            Some(v) => v as c_int,
            None => return raise_exception::<u64>(_py, "TypeError", "file_fd must be an integer"),
        };
        let offset = to_i64(obj_from_bits(offset_bits)).unwrap_or(0);
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(0);

        // Seek to offset if nonzero
        if offset > 0 {
            let ret = unsafe { libc::lseek(file_fd, offset as libc::off_t, libc::SEEK_SET) };
            if ret == -1 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "sendfile");
            }
        }

        let mut buf = [0u8; 8192];
        let mut total_sent: i64 = 0;
        loop {
            let to_read = if count > 0 {
                ((count - total_sent) as usize).min(buf.len())
            } else {
                buf.len()
            };
            if count > 0 && total_sent >= count {
                break;
            }
            let nread = unsafe { libc::read(file_fd, buf.as_mut_ptr() as *mut c_void, to_read) };
            if nread < 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "sendfile");
            }
            if nread == 0 {
                break;
            }
            let mut offset = 0usize;
            while offset < nread as usize {
                let res = with_socket_mut(socket_ptr, |inner| {
                    let fd = inner.raw_fd().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    let ret = unsafe {
                        libc::send(
                            libc_socket(fd),
                            buf.as_ptr().add(offset) as *const c_void,
                            nread as usize - offset,
                            0,
                        )
                    };
                    if ret >= 0 {
                        Ok(ret as usize)
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
                match res {
                    Ok(0) => {
                        return raise_os_error_errno::<u64>(_py, libc::EPIPE as i64, "broken pipe");
                    }
                    Ok(n) => offset += n,
                    Err(err) => return raise_os_error::<u64>(_py, err, "sendfile"),
                }
            }
            total_sent += nread as i64;
        }
        MoltObject::from_int(total_sent).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sendfile(
    _sock_bits: u64,
    _file_fd_bits: u64,
    _offset_bits: u64,
    _count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "sendfile is not supported on this platform")
    })
}

// ---------------------------------------------------------------------------
// sethostname – set the system hostname (requires privileges)
// ---------------------------------------------------------------------------

#[cfg(unix)]
/// # Safety
/// Caller must pass valid runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sethostname(name_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                Some(s) => s,
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "sethostname() argument must be str",
                    );
                }
            };
            let ret = libc::sethostname(
                name.as_ptr() as *const libc::c_char,
                name.len() as libc::c_int,
            );
            if ret != 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "sethostname");
            }
            MoltObject::none().bits()
        })
    }
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sethostname(_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "OSError",
            "sethostname is not supported on this platform",
        )
    })
}

// ---------------------------------------------------------------------------
// sendmsg_afalg – Linux AF_ALG crypto socket sendmsg (kernel crypto API)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendmsg_afalg(
    sock_bits: u64,
    msg_bits: u64,
    op_bits: u64,
    iv_bits: u64,
    assoclen_bits: u64,
    flags_bits: u64,
) -> u64 {
    // Linux AF_ALG constants (from <linux/if_alg.h>)
    const SOL_ALG: c_int = 279;
    const ALG_SET_OP: c_int = 3;
    const ALG_SET_IV: c_int = 2;
    const ALG_SET_AEAD_ASSOCLEN: c_int = 4;

    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "invalid socket handle");
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as c_int;

        // Extract message data
        let msg_obj = obj_from_bits(msg_bits);
        let msg_data: Vec<u8> = if msg_obj.is_none() {
            Vec::new()
        } else {
            let msg_ptr = msg_obj.as_ptr();
            if msg_ptr.is_null() {
                Vec::new()
            } else {
                let type_id = unsafe { object_type_id(msg_ptr) };
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = unsafe { bytes_len(msg_ptr) };
                    let data = unsafe { bytes_data(msg_ptr) };
                    unsafe { std::slice::from_raw_parts(data, len).to_vec() }
                } else {
                    Vec::new()
                }
            }
        };

        // Build ancillary data (cmsg)
        let op = to_i64(obj_from_bits(op_bits)).unwrap_or(0) as u32;
        let op_bytes = op.to_ne_bytes();

        // Calculate total ancillary buffer size
        let mut ancdata_size = unsafe { libc::CMSG_SPACE(4) } as usize; // ALG_SET_OP (u32)

        let iv_obj = obj_from_bits(iv_bits);
        let iv_data: Option<Vec<u8>> = if iv_obj.is_none() {
            None
        } else {
            let iv_ptr = iv_obj.as_ptr();
            if iv_ptr.is_null() {
                None
            } else {
                let type_id = unsafe { object_type_id(iv_ptr) };
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = unsafe { bytes_len(iv_ptr) };
                    let data = unsafe { bytes_data(iv_ptr) };
                    let raw = unsafe { std::slice::from_raw_parts(data, len) };
                    // AF_ALG IV header: 4 bytes length prefix + iv data
                    let mut iv_buf = Vec::with_capacity(4 + raw.len());
                    iv_buf.extend_from_slice(&(raw.len() as u32).to_ne_bytes());
                    iv_buf.extend_from_slice(raw);
                    ancdata_size += unsafe { libc::CMSG_SPACE(iv_buf.len() as u32) } as usize;
                    Some(iv_buf)
                } else {
                    None
                }
            }
        };

        let assoclen = to_i64(obj_from_bits(assoclen_bits)).unwrap_or(-1);
        let assoclen_bytes: Option<[u8; 4]> = if assoclen >= 0 {
            ancdata_size += unsafe { libc::CMSG_SPACE(4) } as usize;
            Some((assoclen as u32).to_ne_bytes())
        } else {
            None
        };

        // Build the ancillary buffer
        let mut anc_buf = vec![0u8; ancdata_size];
        let mut iov = libc::iovec {
            iov_base: if msg_data.is_empty() {
                std::ptr::null_mut()
            } else {
                msg_data.as_ptr() as *mut c_void
            },
            iov_len: msg_data.len(),
        };

        let mut msghdr: libc::msghdr = unsafe { std::mem::zeroed() };
        msghdr.msg_iov = &mut iov;
        msghdr.msg_iovlen = 1;
        msghdr.msg_control = anc_buf.as_mut_ptr() as *mut c_void;
        msghdr.msg_controllen = ancdata_size as _;

        // Fill in the cmsg entries
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msghdr);

            // ALG_SET_OP
            if !cmsg.is_null() {
                (*cmsg).cmsg_level = SOL_ALG;
                (*cmsg).cmsg_type = ALG_SET_OP;
                (*cmsg).cmsg_len = libc::CMSG_LEN(4) as _;
                std::ptr::copy_nonoverlapping(op_bytes.as_ptr(), libc::CMSG_DATA(cmsg), 4);
                cmsg = libc::CMSG_NXTHDR(&msghdr, cmsg);
            }

            // ALG_SET_IV (optional)
            if let Some(ref iv) = iv_data {
                if !cmsg.is_null() {
                    (*cmsg).cmsg_level = SOL_ALG;
                    (*cmsg).cmsg_type = ALG_SET_IV;
                    (*cmsg).cmsg_len = libc::CMSG_LEN(iv.len() as u32) as _;
                    std::ptr::copy_nonoverlapping(iv.as_ptr(), libc::CMSG_DATA(cmsg), iv.len());
                    cmsg = libc::CMSG_NXTHDR(&msghdr, cmsg);
                }
            }

            // ALG_SET_AEAD_ASSOCLEN (optional)
            if let Some(ref assoc) = assoclen_bytes {
                if !cmsg.is_null() {
                    (*cmsg).cmsg_level = SOL_ALG;
                    (*cmsg).cmsg_type = ALG_SET_AEAD_ASSOCLEN;
                    (*cmsg).cmsg_len = libc::CMSG_LEN(4) as _;
                    std::ptr::copy_nonoverlapping(assoc.as_ptr(), libc::CMSG_DATA(cmsg), 4);
                }
            }
        }

        let res = with_socket_mut(socket_ptr, |inner| {
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe { libc::sendmsg(libc_socket(fd), &msghdr, flags) };
            if ret >= 0 {
                Ok(ret as i64)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });

        match res {
            Ok(n) => MoltObject::from_int(n).bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "sendmsg_afalg"),
        }
    })
}

#[cfg(not(target_os = "linux"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_sendmsg_afalg(
    _sock_bits: u64,
    _msg_bits: u64,
    _op_bits: u64,
    _iv_bits: u64,
    _assoclen_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "sendmsg_afalg is only supported on Linux")
    })
}
