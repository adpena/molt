// Socket address conversion authority for native SockAddr and WASM wire buffers.
// Owns host/service parsing, sockaddr materialization, and runtime tuple encoding.

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use super::*;
#[cfg(molt_has_net_io)]
use socket2::SockAddr;
#[cfg(molt_has_net_io)]
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::net::{Ipv4Addr, Ipv6Addr};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn host_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "host bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("host must be str, bytes, or None, not {obj_type}"))
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn port_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<u16, String> {
    let obj = obj_from_bits(bits);
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(port as u16);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        let port = text
            .parse::<u16>()
            .map_err(|_| "port must be int".to_string())?;
        return Ok(port);
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("port must be int or str, not {obj_type}"))
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn service_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(Some(port.to_string()));
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "service bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("service must be int or str, not {obj_type}"))
}

#[cfg(all(molt_has_net_io, unix))]
fn unix_path_from_bits(_py: &PyToken<'_>, addr_bits: u64) -> Result<std::path::PathBuf, String> {
    let obj = obj_from_bits(addr_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(std::path::PathBuf::from(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                use std::os::unix::ffi::OsStringExt;
                let path = std::ffi::OsString::from_vec(bytes.to_vec());
                return Ok(std::path::PathBuf::from(path));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, addr_bits));
    Err(format!("a bytes-like object is required, not '{obj_type}'"))
}

#[cfg(molt_has_net_io)]
pub(crate) fn sockaddr_from_bits(
    _py: &PyToken<'_>,
    addr_bits: u64,
    family: i32,
) -> Result<SockAddr, String> {
    if family == libc::AF_UNIX {
        #[cfg(all(unix, molt_has_net_io))]
        {
            let path = unix_path_from_bits(_py, addr_bits)?;
            return SockAddr::unix(path).map_err(|err| err.to_string());
        }
        #[cfg(not(unix))]
        {
            return Err("AF_UNIX is unsupported".to_string());
        }
    }
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("address must be tuple".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return Err("address must be tuple".to_string());
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return Err("address must be (host, port)".to_string());
        }
        let host = host_from_bits(_py, elems[0])?;
        let port = port_from_bits(_py, elems[1])?;
        if family == libc::AF_INET {
            let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
            let ip = host
                .parse::<Ipv4Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V4(v4) => Some(v4),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv4 address".to_string())?;
            return Ok(SockAddr::from(SocketAddr::new(IpAddr::V4(ip), port)));
        }
        if family == libc::AF_INET6 {
            let host = host.unwrap_or_else(|| "::".to_string());
            let mut flowinfo = 0u32;
            let mut scope_id = 0u32;
            if elems.len() >= 3 {
                flowinfo = to_i64(obj_from_bits(elems[2])).unwrap_or(0) as u32;
            }
            if elems.len() >= 4 {
                scope_id = to_i64(obj_from_bits(elems[3])).unwrap_or(0) as u32;
            }
            let ip = host
                .parse::<Ipv6Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V6(v6) => Some(v6),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv6 address".to_string())?;
            let addr = SocketAddr::V6(std::net::SocketAddrV6::new(ip, port, flowinfo, scope_id));
            return Ok(SockAddr::from(addr));
        }
    }
    Err("unsupported address family".to_string())
}

#[cfg(molt_has_net_io)]
pub(crate) fn sockaddr_to_bits(_py: &PyToken<'_>, addr: &SockAddr) -> u64 {
    if let Some(sockaddr) = addr.as_socket() {
        match sockaddr {
            SocketAddr::V4(v4) => {
                let host = v4.ip().to_string();
                let host_ptr = alloc_string(_py, host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v4.port() as i64).bits();
                let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
                dec_ref_bits(_py, host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
            SocketAddr::V6(v6) => {
                let host = v6.ip().to_string();
                let host_ptr = alloc_string(_py, host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v6.port() as i64).bits();
                let flow_bits = MoltObject::from_int(v6.flowinfo() as i64).bits();
                let scope_bits = MoltObject::from_int(v6.scope_id() as i64).bits();
                let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
                dec_ref_bits(_py, host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        }
    } else {
        #[cfg(unix)]
        {
            if let Some(path) = addr.as_pathname() {
                let text = path.to_string_lossy();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                MoltObject::none().bits()
            }
        }
        #[cfg(not(unix))]
        {
            MoltObject::none().bits()
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn encode_sockaddr(
    _py: &PyToken<'_>,
    addr_bits: u64,
    family: i32,
) -> Result<Vec<u8>, String> {
    if family == libc::AF_UNIX {
        return Err("AF_UNIX is unsupported".to_string());
    }
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("address must be tuple".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return Err("address must be tuple".to_string());
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return Err("address must be (host, port)".to_string());
        }
        let host = host_from_bits(_py, elems[0])?;
        let port = port_from_bits(_py, elems[1])?;
        let mut out = Vec::new();
        out.extend_from_slice(&(family as u16).to_le_bytes());
        out.extend_from_slice(&port.to_le_bytes());
        if family == libc::AF_INET {
            let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
            let ip = host
                .parse::<Ipv4Addr>()
                .map_err(|_| "invalid IPv4 address".to_string())?;
            out.extend_from_slice(&ip.octets());
            return Ok(out);
        }
        if family == libc::AF_INET6 {
            let host = host.unwrap_or_else(|| "::".to_string());
            let ip = host
                .parse::<Ipv6Addr>()
                .map_err(|_| "invalid IPv6 address".to_string())?;
            let mut flowinfo = 0u32;
            let mut scope_id = 0u32;
            if elems.len() >= 3 {
                flowinfo = to_i64(obj_from_bits(elems[2])).unwrap_or(0) as u32;
            }
            if elems.len() >= 4 {
                scope_id = to_i64(obj_from_bits(elems[3])).unwrap_or(0) as u32;
            }
            out.extend_from_slice(&flowinfo.to_le_bytes());
            out.extend_from_slice(&scope_id.to_le_bytes());
            out.extend_from_slice(&ip.octets());
            return Ok(out);
        }
        Err("unsupported address family".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn decode_sockaddr(_py: &PyToken<'_>, buf: &[u8]) -> Result<u64, String> {
    if buf.len() < 4 {
        return Err("invalid sockaddr".to_string());
    }
    let family = u16::from_le_bytes([buf[0], buf[1]]) as i32;
    let port = u16::from_le_bytes([buf[2], buf[3]]);
    if family == libc::AF_INET {
        if buf.len() < 8 {
            return Err("invalid IPv4 sockaddr".to_string());
        }
        let mut octets = [0u8; 4];
        octets.copy_from_slice(&buf[4..8]);
        let host = Ipv4Addr::from(octets).to_string();
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let port_bits = MoltObject::from_int(port as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
        dec_ref_bits(_py, host_bits);
        if tuple_ptr.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            Ok(MoltObject::from_ptr(tuple_ptr).bits())
        }
    } else if family == libc::AF_INET6 {
        if buf.len() < 28 {
            return Err("invalid IPv6 sockaddr".to_string());
        }
        let flowinfo = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let scope_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&buf[12..28]);
        let host = Ipv6Addr::from(octets).to_string();
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let port_bits = MoltObject::from_int(port as i64).bits();
        let flow_bits = MoltObject::from_int(flowinfo as i64).bits();
        let scope_bits = MoltObject::from_int(scope_id as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
        dec_ref_bits(_py, host_bits);
        if tuple_ptr.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            Ok(MoltObject::from_ptr(tuple_ptr).bits())
        }
    } else if family == libc::AF_UNIX {
        Err("AF_UNIX is unsupported".to_string())
    } else {
        Err("unsupported address family".to_string())
    }
}
