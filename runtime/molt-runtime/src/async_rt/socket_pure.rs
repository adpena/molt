//! Pure socket utility intrinsics.
//!
//! These operations do not require live sockets or host networking. Keep them
//! available for native net, native no-net, and WASM builds through this single
//! authority so feature gates cannot turn pure byte/order/address conversions
//! into unsupported-network stubs.

use crate::PyToken;
use crate::socket_constants::{AF_INET, AF_INET6};
use crate::*;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use std::net::{Ipv4Addr, Ipv6Addr};

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

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

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_inet_pton(family_bits: u64, address_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let addr = match host_from_bits(_py, address_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "address cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if family == AF_INET {
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
        if family == AF_INET6 {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_inet_ntop(family_bits: u64, packed_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(packed_bits);
        let data = if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if let Some(slice) = bytes_like_slice_raw(ptr) {
                    slice.to_vec()
                } else if let Some(slice) = memoryview_bytes_slice(ptr) {
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
            }
        } else {
            return raise_exception::<_>(_py, "TypeError", "packed address must be bytes-like");
        };
        if family == AF_INET {
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
        if family == AF_INET6 {
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
        let msg = format!("{func}: Python int too large to convert to C 16-bit unsigned integer");
        raise_exception::<Option<u16>>(_py, "OverflowError", &msg);
        return None;
    };
    Some(out)
}

fn socket_u32_from_int_only(_py: &PyToken<'_>, value_bits: u64) -> Option<u32> {
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
        raise_exception::<Option<u32>>(
            _py,
            "OverflowError",
            "Python int too large to convert to C unsigned long",
        );
        return None;
    }
    let Some(out) = value.to_u32() else {
        raise_exception::<Option<u32>>(
            _py,
            "OverflowError",
            "Python int too large to convert to C unsigned long",
        );
        return None;
    };
    Some(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_htons(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = socket_u16_from_index(_py, value_bits, "htons") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u16::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_ntohs(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = socket_u16_from_index(_py, value_bits, "ntohs") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u16::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_htonl(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = socket_u32_from_int_only(_py, value_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u32::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_ntohl(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = socket_u32_from_int_only(_py, value_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(u32::to_be(value) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_len(datalen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        #[cfg(unix)]
        {
            let Ok(datalen_u32) = u32::try_from(datalen) else {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "CMSG_LEN() argument out of range",
                );
            };
            let result = unsafe { libc::CMSG_LEN(datalen_u32) } as i64;
            MoltObject::from_int(result).bits()
        }
        #[cfg(not(unix))]
        {
            let header_size = std::mem::size_of::<usize>() * 3;
            let result = (header_size as i64) + datalen;
            MoltObject::from_int(result).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_cmsg_space(datalen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        #[cfg(unix)]
        {
            let Ok(datalen_u32) = u32::try_from(datalen) else {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "CMSG_SPACE() argument out of range",
                );
            };
            let result = unsafe { libc::CMSG_SPACE(datalen_u32) } as i64;
            MoltObject::from_int(result).bits()
        }
        #[cfg(not(unix))]
        {
            let align = std::mem::size_of::<usize>();
            let header_size = align * 3;
            let total = header_size as i64 + datalen;
            let aligned = (total + (align as i64 - 1)) & !(align as i64 - 1);
            MoltObject::from_int(aligned).bits()
        }
    })
}
