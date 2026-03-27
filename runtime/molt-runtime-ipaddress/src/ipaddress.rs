// === FILE: runtime/molt-runtime/src/builtins/ipaddress.rs ===
//
// IP address module intrinsics for IPv4 and IPv6 addresses and networks.
// Uses std::net::{Ipv4Addr, Ipv6Addr} for parsing and validation.

use crate::bridge::{
    alloc_bytes, alloc_list, alloc_string, dec_ref_bits, int_bits_from_bigint, int_bits_from_i64,
    raise_exception, release_ptr, string_obj_to_owned, to_i64,
};
use molt_obj_model::MoltObject;
use molt_runtime_core::prelude::*;
use num_bigint::BigInt;
use std::net::{Ipv4Addr, Ipv6Addr};

// ---------------------------------------------------------------------------
// Handle types
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Ipv4Handle {
    addr: Ipv4Addr,
}

#[derive(Clone)]
struct Ipv6Handle {
    addr: Ipv6Addr,
}

#[derive(Clone)]
struct Ipv4NetworkHandle {
    network: Ipv4Addr,
    prefix_len: u8,
}

// ---------------------------------------------------------------------------
// Private address ranges for IPv4 classification
// ---------------------------------------------------------------------------

fn ipv4_is_private(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    // 10.0.0.0/8
    if o[0] == 10 {
        return true;
    }
    // 172.16.0.0/12
    if o[0] == 172 && (16..=31).contains(&o[1]) {
        return true;
    }
    // 192.168.0.0/16
    if o[0] == 192 && o[1] == 168 {
        return true;
    }
    false
}

fn ipv4_is_loopback(addr: Ipv4Addr) -> bool {
    addr.octets()[0] == 127
}

fn ipv4_is_multicast(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] >= 224 && o[0] <= 239
}

fn ipv4_is_reserved(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    // IANA reserved: 240.0.0.0/4
    o[0] >= 240
}

fn ipv4_is_link_local(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 169 && o[1] == 254
}

fn ipv4_is_global(addr: Ipv4Addr) -> bool {
    !ipv4_is_private(addr)
        && !ipv4_is_loopback(addr)
        && !ipv4_is_multicast(addr)
        && !ipv4_is_reserved(addr)
        && !ipv4_is_link_local(addr)
        && addr != Ipv4Addr::BROADCAST
        && addr != Ipv4Addr::UNSPECIFIED
}

// ---------------------------------------------------------------------------
// Private address ranges for IPv6 classification
// ---------------------------------------------------------------------------

fn ipv6_is_loopback(addr: Ipv6Addr) -> bool {
    addr == Ipv6Addr::LOCALHOST
}

fn ipv6_is_multicast(addr: Ipv6Addr) -> bool {
    addr.segments()[0] & 0xFF00 == 0xFF00
}

fn ipv6_is_link_local(addr: Ipv6Addr) -> bool {
    let seg = addr.segments();
    seg[0] & 0xFFC0 == 0xFE80
}

fn ipv6_is_private(addr: Ipv6Addr) -> bool {
    let seg = addr.segments();
    // fc00::/7
    seg[0] & 0xFE00 == 0xFC00
}

fn ipv6_is_global(addr: Ipv6Addr) -> bool {
    !ipv6_is_loopback(addr)
        && !ipv6_is_multicast(addr)
        && !ipv6_is_link_local(addr)
        && !ipv6_is_private(addr)
        && addr != Ipv6Addr::UNSPECIFIED
}

// ---------------------------------------------------------------------------
// Network helpers
// ---------------------------------------------------------------------------

fn ipv4_prefix_mask(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0u32
    } else {
        u32::MAX << (32 - prefix_len)
    }
}

fn ipv4_network_address(network: Ipv4Addr, prefix_len: u8) -> Ipv4Addr {
    let masked = u32::from(network) & ipv4_prefix_mask(prefix_len);
    Ipv4Addr::from(masked)
}

fn ipv4_broadcast_address(network: Ipv4Addr, prefix_len: u8) -> Ipv4Addr {
    let mask = ipv4_prefix_mask(prefix_len);
    let base = u32::from(network) & mask;
    let broadcast = base | !mask;
    Ipv4Addr::from(broadcast)
}

fn parse_ipv4_network(s: &str) -> Result<(Ipv4Addr, u8), &'static str> {
    if let Some(slash) = s.find('/') {
        let host_part = &s[..slash];
        let prefix_str = &s[slash + 1..];
        let prefix_len: u8 = prefix_str.parse().map_err(|_| "invalid prefix length")?;
        if prefix_len > 32 {
            return Err("prefix length must be <= 32");
        }
        let addr: Ipv4Addr = host_part.parse().map_err(|_| "invalid IPv4 address")?;
        Ok((addr, prefix_len))
    } else {
        let addr: Ipv4Addr = s.parse().map_err(|_| "invalid IPv4 address")?;
        Ok((addr, 32))
    }
}

// ---------------------------------------------------------------------------
// Handle from-bits helpers
// ---------------------------------------------------------------------------

fn ipv4_handle_from_bits(bits: u64) -> Option<&'static mut Ipv4Handle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: pointer from Box::into_raw for an Ipv4Handle.
    Some(unsafe { &mut *(ptr as *mut Ipv4Handle) })
}

fn ipv6_handle_from_bits(bits: u64) -> Option<&'static mut Ipv6Handle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: pointer from Box::into_raw for an Ipv6Handle.
    Some(unsafe { &mut *(ptr as *mut Ipv6Handle) })
}

fn ipv4_network_handle_from_bits(bits: u64) -> Option<&'static mut Ipv4NetworkHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: pointer from Box::into_raw for an Ipv4NetworkHandle.
    Some(unsafe { &mut *(ptr as *mut Ipv4NetworkHandle) })
}

fn ipv4_bits(handle: Ipv4Handle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(handle)) as *mut u8)
}

fn ipv6_bits(handle: Ipv6Handle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(handle)) as *mut u8)
}

fn ipv4_network_bits(handle: Ipv4NetworkHandle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(handle)) as *mut u8)
}

// Parse address from string or integer bits.
fn parse_ipv4_from_bits(_py: &CoreGilToken, addr_bits: u64) -> Result<Ipv4Addr, u64> {
    let obj = obj_from_bits(addr_bits);
    if let Some(s) = string_obj_to_owned(obj) {
        return s
            .parse::<Ipv4Addr>()
            .map_err(|_| raise_exception::<u64>(_py, "ValueError", "invalid IPv4 address"));
    }
    if let Some(v) = to_i64(obj) {
        if !(0..=0xFFFF_FFFFi64).contains(&v) {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "IPv4 address must be in range 0-4294967295",
            ));
        }
        return Ok(Ipv4Addr::from(v as u32));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "expected str or int for IPv4 address",
    ))
}

fn parse_ipv6_from_bits(_py: &CoreGilToken, addr_bits: u64) -> Result<Ipv6Addr, u64> {
    let obj = obj_from_bits(addr_bits);
    if let Some(s) = string_obj_to_owned(obj) {
        return s
            .parse::<Ipv6Addr>()
            .map_err(|_| raise_exception::<u64>(_py, "ValueError", "invalid IPv6 address"));
    }
    if let Some(v) = to_i64(obj) {
        if v < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "IPv6 address must be non-negative",
            ));
        }
        let octets = (v as u128).to_be_bytes();
        return Ok(Ipv6Addr::from(octets));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "expected str or int for IPv6 address",
    ))
}

// ---------------------------------------------------------------------------
// IPv4 intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_new(addr_bits: u64) -> u64 {
    with_core_gil!(_py, {
        match parse_ipv4_from_bits(_py, addr_bits) {
            Ok(addr) => ipv4_bits(Ipv4Handle { addr }),
            Err(exc) => exc,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_packed(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        let octets = h.addr.octets();
        let ptr = alloc_bytes(_py, &octets);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_int(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        int_bits_from_i64(_py, u32::from(h.addr) as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_str(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        let s = h.addr.to_string();
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_private(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_private(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_loopback(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_loopback(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_multicast(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_multicast(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_reserved(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_reserved(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_link_local(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_link_local(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_is_global(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        MoltObject::from_bool(ipv4_is_global(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_version(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(_h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        int_bits_from_i64(_py, 4)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_max_prefixlen(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(_h) = ipv4_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 address handle");
        };
        int_bits_from_i64(_py, 32)
    })
}

// ---------------------------------------------------------------------------
// IPv6 intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_new(addr_bits: u64) -> u64 {
    with_core_gil!(_py, {
        match parse_ipv6_from_bits(_py, addr_bits) {
            Ok(addr) => ipv6_bits(Ipv6Handle { addr }),
            Err(exc) => exc,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_packed(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        let octets = h.addr.octets();
        let ptr = alloc_bytes(_py, &octets);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_int(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        let v = u128::from(h.addr);
        let big = BigInt::from(v);
        int_bits_from_bigint(_py, big)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_str(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        let s = h.addr.to_string();
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_is_private(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        MoltObject::from_bool(ipv6_is_private(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_is_loopback(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        MoltObject::from_bool(ipv6_is_loopback(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_is_multicast(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        MoltObject::from_bool(ipv6_is_multicast(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_is_link_local(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        MoltObject::from_bool(ipv6_is_link_local(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_is_global(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        MoltObject::from_bool(ipv6_is_global(h.addr)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_version(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(_h) = ipv6_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv6 address handle");
        };
        int_bits_from_i64(_py, 6)
    })
}

// ---------------------------------------------------------------------------
// IPv4 Network intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_new(addr_bits: u64, strict_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(addr_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "expected str for network address");
        };
        let strict = {
            let obj = obj_from_bits(strict_bits);
            obj.as_bool().unwrap_or(true)
        };
        let (host_addr, prefix_len) = match parse_ipv4_network(&s) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg),
        };
        let network_addr = ipv4_network_address(host_addr, prefix_len);
        if strict && host_addr != network_addr {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "host bits set in network address; use strict=False to suppress",
            );
        }
        ipv4_network_bits(Ipv4NetworkHandle {
            network: network_addr,
            prefix_len,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_hosts(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_network_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 network handle");
        };
        let base = u32::from(h.network);
        let prefix = h.prefix_len;
        // /31 and /32 are special cases per RFC 3021 / CPython behaviour.
        let (first, last): (u32, u32) = if prefix >= 31 {
            (base, u32::from(ipv4_broadcast_address(h.network, prefix)))
        } else {
            (
                base + 1,
                u32::from(ipv4_broadcast_address(h.network, prefix)) - 1,
            )
        };
        let count = if last >= first {
            (last - first + 1) as usize
        } else {
            0
        };
        let mut elems: Vec<u64> = Vec::with_capacity(count);
        for ip_int in first..=last {
            let addr = Ipv4Addr::from(ip_int);
            let addr_handle = Ipv4Handle { addr };
            elems.push(ipv4_bits(addr_handle));
        }
        let list_ptr = alloc_list(_py, &elems);
        // alloc_list inc-refs; dec our refs.
        for b in &elems {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_contains(net_bits: u64, addr_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(net) = ipv4_network_handle_from_bits(net_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 network handle");
        };
        let addr = match parse_ipv4_from_bits(_py, addr_bits) {
            Ok(a) => a,
            Err(exc) => {
                // Try to get address from an Ipv4Handle instead.
                if let Some(h) = ipv4_handle_from_bits(addr_bits) {
                    h.addr
                } else {
                    return exc;
                }
            }
        };
        let mask = ipv4_prefix_mask(net.prefix_len);
        let contained = (u32::from(addr) & mask) == u32::from(net.network);
        MoltObject::from_bool(contained).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_prefixlen(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_network_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 network handle");
        };
        int_bits_from_i64(_py, h.prefix_len as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_str(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_network_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 network handle");
        };
        let s = format!("{}/{}", h.network, h.prefix_len);
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_broadcast(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let Some(h) = ipv4_network_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid IPv4 network handle");
        };
        let broadcast = ipv4_broadcast_address(h.network, h.prefix_len);
        ipv4_bits(Ipv4Handle { addr: broadcast })
    })
}

// ---------------------------------------------------------------------------
// Unified drop (dispatches based on tag embedded in pointer is not available;
// caller must call the correct drop for their handle type)
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_drop(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        // SAFETY: caller must pass the correct handle type.
        // Since IPv4, IPv6, and network handles are independent heap allocs,
        // dropping via Box<u8> is safe to free the memory; actual destructors
        // run on the typed drop wrappers below.
        unsafe {
            // We don't know the original type here, but the provenance release
            // above handles tracking; the allocation itself is dropped via
            // Box<[u8; N]>-style free since all structs are plain Rust values
            // with no Drop side-effects beyond freeing heap storage.
            // Drop as Ipv4Handle (smallest, safe because all handles are
            // plain-data structs that contain no owning heap pointers).
            drop(Box::from_raw(ptr as *mut Ipv4Handle));
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v6_drop(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        // SAFETY: pointer from Box::into_raw for Ipv6Handle.
        unsafe {
            drop(Box::from_raw(ptr as *mut Ipv6Handle));
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ipaddress_v4_network_drop(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        // SAFETY: pointer from Box::into_raw for Ipv4NetworkHandle.
        unsafe {
            drop(Box::from_raw(ptr as *mut Ipv4NetworkHandle));
        }
        MoltObject::none().bits()
    })
}
