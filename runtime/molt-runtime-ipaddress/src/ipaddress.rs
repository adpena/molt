// === FILE: runtime/molt-runtime/src/builtins/ipaddress.rs ===
//
// IP address module intrinsics for IPv4 and IPv6 addresses and networks.
// Uses std::net::{Ipv4Addr, Ipv6Addr} for parsing and validation.

use crate::bridge::{
    alloc_bytes, alloc_list, alloc_string, dec_ref_bits, int_bits_from_bigint, int_bits_from_i64,
    opaque_handle_bits, opaque_handle_ptr_from_bits, raise_exception, release_ptr,
    string_obj_to_owned, to_bigint,
};
use molt_obj_model::MoltObject;
use molt_runtime_core::prelude::*;
use num_bigint::{BigInt, Sign};
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
// IANA special-purpose address registries (CPython 3.12 parity)
//
// `is_private` / `is_global` encode the full iana-ipv4-special-registry and
// iana-ipv6-special-registry exactly as CPython's `ipaddress` module does (see
// CPython Lib/ipaddress.py `_IPv4Constants` / `_IPv6Constants` and the
// `is_private` / `is_global` address properties).  Each entry is a network
// `(base, prefix_len)` where `base` is the network address as a host-order
// integer.  An address is "in" the network iff its high `prefix_len` bits equal
// `base`'s high `prefix_len` bits.
// ---------------------------------------------------------------------------

const V4_PRIVATE_NETWORKS: &[(u32, u8)] = &[
    (0x00000000, 8),  // 0.0.0.0/8
    (0x0A000000, 8),  // 10.0.0.0/8
    (0x7F000000, 8),  // 127.0.0.0/8
    (0xA9FE0000, 16), // 169.254.0.0/16
    (0xAC100000, 12), // 172.16.0.0/12
    (0xC0000000, 24), // 192.0.0.0/24
    (0xC00000AA, 31), // 192.0.0.170/31
    (0xC0000200, 24), // 192.0.2.0/24
    (0xC0A80000, 16), // 192.168.0.0/16
    (0xC6120000, 15), // 198.18.0.0/15
    (0xC6336400, 24), // 198.51.100.0/24
    (0xCB007100, 24), // 203.0.113.0/24
    (0xF0000000, 4),  // 240.0.0.0/4
    (0xFFFFFFFF, 32), // 255.255.255.255/32
];

const V4_PRIVATE_NETWORK_EXCEPTIONS: &[(u32, u8)] = &[
    (0xC0000009, 32), // 192.0.0.9/32
    (0xC000000A, 32), // 192.0.0.10/32
];

// 100.64.0.0/10 — globally unreachable but not "private" (both flags False).
const V4_PUBLIC_NETWORK: (u32, u8) = (0x64400000, 10);

const V6_PRIVATE_NETWORKS: &[(u128, u8)] = &[
    (0x0000_0000_0000_0000_0000_0000_0000_0001, 128), // ::1/128
    (0x0000_0000_0000_0000_0000_0000_0000_0000, 128), // ::/128
    (0x0000_0000_0000_0000_0000_FFFF_0000_0000, 96),  // ::ffff:0.0.0.0/96
    (0x0064_FF9B_0001_0000_0000_0000_0000_0000, 48),  // 64:ff9b:1::/48
    (0x0100_0000_0000_0000_0000_0000_0000_0000, 64),  // 100::/64
    (0x2001_0000_0000_0000_0000_0000_0000_0000, 23),  // 2001::/23
    (0x2001_0DB8_0000_0000_0000_0000_0000_0000, 32),  // 2001:db8::/32
    (0x2002_0000_0000_0000_0000_0000_0000_0000, 16),  // 2002::/16
    (0x3FFF_0000_0000_0000_0000_0000_0000_0000, 20),  // 3fff::/20
    (0xFC00_0000_0000_0000_0000_0000_0000_0000, 7),   // fc00::/7
    (0xFE80_0000_0000_0000_0000_0000_0000_0000, 10),  // fe80::/10
];

const V6_PRIVATE_NETWORK_EXCEPTIONS: &[(u128, u8)] = &[
    (0x2001_0001_0000_0000_0000_0000_0000_0001, 128), // 2001:1::1/128
    (0x2001_0001_0000_0000_0000_0000_0000_0002, 128), // 2001:1::2/128
    (0x2001_0003_0000_0000_0000_0000_0000_0000, 32),  // 2001:3::/32
    (0x2001_0004_0112_0000_0000_0000_0000_0000, 48),  // 2001:4:112::/48
    (0x2001_0020_0000_0000_0000_0000_0000_0000, 28),  // 2001:20::/28
    (0x2001_0030_0000_0000_0000_0000_0000_0000, 28),  // 2001:30::/28
];

#[inline]
fn v4_in_network(addr_int: u32, base: u32, prefix_len: u8) -> bool {
    debug_assert!(prefix_len <= 32);
    if prefix_len == 0 {
        return true;
    }
    if prefix_len >= 32 {
        return addr_int == base;
    }
    let shift = 32 - prefix_len as u32;
    (addr_int >> shift) == (base >> shift)
}

#[inline]
fn v6_in_network(addr_int: u128, base: u128, prefix_len: u8) -> bool {
    debug_assert!(prefix_len <= 128);
    if prefix_len == 0 {
        return true;
    }
    if prefix_len >= 128 {
        return addr_int == base;
    }
    let shift = 128 - prefix_len as u32;
    (addr_int >> shift) == (base >> shift)
}

// ---------------------------------------------------------------------------
// IPv4 classification
// ---------------------------------------------------------------------------

fn ipv4_is_private(addr: Ipv4Addr) -> bool {
    let a = u32::from(addr);
    V4_PRIVATE_NETWORKS
        .iter()
        .any(|&(base, prefix)| v4_in_network(a, base, prefix))
        && V4_PRIVATE_NETWORK_EXCEPTIONS
            .iter()
            .all(|&(base, prefix)| !v4_in_network(a, base, prefix))
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
    let a = u32::from(addr);
    !v4_in_network(a, V4_PUBLIC_NETWORK.0, V4_PUBLIC_NETWORK.1) && !ipv4_is_private(addr)
}

// ---------------------------------------------------------------------------
// IPv6 classification
// ---------------------------------------------------------------------------

/// Return the embedded IPv4 address when `addr` is an IPv4-mapped IPv6 address
/// (`::ffff:0:0/96`), exactly as CPython's `IPv6Address.ipv4_mapped`.  For such
/// addresses `is_private` / `is_global` delegate to the mapped IPv4 semantics.
fn ipv6_ipv4_mapped(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let v = u128::from(addr);
    if (v >> 32) != 0xFFFF {
        return None;
    }
    Some(Ipv4Addr::from((v & 0xFFFF_FFFF) as u32))
}

fn ipv6_is_loopback(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = ipv6_ipv4_mapped(addr) {
        return ipv4_is_loopback(mapped);
    }
    u128::from(addr) == 1
}

fn ipv6_is_multicast(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = ipv6_ipv4_mapped(addr) {
        return ipv4_is_multicast(mapped);
    }
    addr.segments()[0] & 0xFF00 == 0xFF00
}

fn ipv6_is_link_local(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = ipv6_ipv4_mapped(addr) {
        return ipv4_is_link_local(mapped);
    }
    let seg = addr.segments();
    seg[0] & 0xFFC0 == 0xFE80
}

fn ipv6_is_private(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = ipv6_ipv4_mapped(addr) {
        return ipv4_is_private(mapped);
    }
    let a = u128::from(addr);
    V6_PRIVATE_NETWORKS
        .iter()
        .any(|&(base, prefix)| v6_in_network(a, base, prefix))
        && V6_PRIVATE_NETWORK_EXCEPTIONS
            .iter()
            .all(|&(base, prefix)| !v6_in_network(a, base, prefix))
}

fn ipv6_is_global(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = ipv6_ipv4_mapped(addr) {
        return ipv4_is_global(mapped);
    }
    !ipv6_is_private(addr)
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
    let ptr = opaque_handle_ptr_from_bits(bits)?;
    // SAFETY: pointer from Box::into_raw for an Ipv4Handle.
    Some(unsafe { &mut *(ptr as *mut Ipv4Handle) })
}

fn ipv6_handle_from_bits(bits: u64) -> Option<&'static mut Ipv6Handle> {
    let ptr = opaque_handle_ptr_from_bits(bits)?;
    // SAFETY: pointer from Box::into_raw for an Ipv6Handle.
    Some(unsafe { &mut *(ptr as *mut Ipv6Handle) })
}

fn ipv4_network_handle_from_bits(bits: u64) -> Option<&'static mut Ipv4NetworkHandle> {
    let ptr = opaque_handle_ptr_from_bits(bits)?;
    // SAFETY: pointer from Box::into_raw for an Ipv4NetworkHandle.
    Some(unsafe { &mut *(ptr as *mut Ipv4NetworkHandle) })
}

fn ipv4_bits(handle: Ipv4Handle) -> u64 {
    opaque_handle_bits(Box::into_raw(Box::new(handle)) as *mut u8)
}

fn ipv6_bits(handle: Ipv6Handle) -> u64 {
    opaque_handle_bits(Box::into_raw(Box::new(handle)) as *mut u8)
}

fn ipv4_network_bits(handle: Ipv4NetworkHandle) -> u64 {
    opaque_handle_bits(Box::into_raw(Box::new(handle)) as *mut u8)
}

/// Convert a non-negative `BigInt` that fits in `N` bytes into a fixed-width
/// big-endian byte array (the packed network-order representation an IP address
/// expects).  Returns `None` when the value is negative or wider than `N` bytes,
/// i.e. outside `0..=2**(8*N)-1`.
fn bigint_to_fixed_be<const N: usize>(value: &BigInt) -> Option<[u8; N]> {
    let (sign, mag) = value.to_bytes_be();
    match sign {
        // Zero produces a single 0x00 magnitude byte from `to_bytes_be`; it fits.
        Sign::Minus => return None,
        Sign::NoSign | Sign::Plus => {}
    }
    if mag.len() > N {
        return None;
    }
    let mut out = [0u8; N];
    // Right-align the magnitude bytes (big-endian, zero-padded on the left).
    out[N - mag.len()..].copy_from_slice(&mag);
    Some(out)
}

// Parse address from string or integer bits.
//
// The integer path reads the full-precision value via `to_bigint` (NOT `to_i64`,
// which clamps to the i64 range and would reject every IPv6 integer >= 2**63),
// then range-checks `0 <= n < 2**(width)` exactly like CPython's
// `AddressValueError` for out-of-range integers.
fn parse_ipv4_from_bits(_py: &CoreGilToken, addr_bits: u64) -> Result<Ipv4Addr, u64> {
    let obj = obj_from_bits(addr_bits);
    if let Some(s) = string_obj_to_owned(obj) {
        return s
            .parse::<Ipv4Addr>()
            .map_err(|_| raise_exception::<u64>(_py, "ValueError", "invalid IPv4 address"));
    }
    if let Some(v) = to_bigint(obj) {
        let Some(octets) = bigint_to_fixed_be::<4>(&v) else {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "IPv4 address must be in range 0-4294967295",
            ));
        };
        return Ok(Ipv4Addr::from(octets));
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
    if let Some(v) = to_bigint(obj) {
        let Some(octets) = bigint_to_fixed_be::<16>(&v) else {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "IPv6 address must be in range 0-340282366920938463463374607431768211455",
            ));
        };
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
        let Some(ptr) = opaque_handle_ptr_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        unsafe { release_ptr(ptr) };
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
        let Some(ptr) = opaque_handle_ptr_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        unsafe { release_ptr(ptr) };
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
        let Some(ptr) = opaque_handle_ptr_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        unsafe { release_ptr(ptr) };
        // SAFETY: pointer from Box::into_raw for Ipv4NetworkHandle.
        unsafe {
            drop(Box::from_raw(ptr as *mut Ipv4NetworkHandle));
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    //! Classification / parsing parity against CPython 3.12 `ipaddress`.
    //!
    //! Expected values were captured directly from CPython 3.12.13's
    //! `IPv4Address`/`IPv6Address` `is_private` / `is_global` properties.
    use super::*;
    use std::str::FromStr;

    fn v4(s: &str) -> Ipv4Addr {
        Ipv4Addr::from_str(s).unwrap()
    }
    fn v6(s: &str) -> Ipv6Addr {
        Ipv6Addr::from_str(s).unwrap()
    }

    #[test]
    fn v4_private_global_matches_cpython() {
        // (addr, is_private, is_global) — from CPython 3.12.13.
        let cases: &[(&str, bool, bool)] = &[
            ("0.0.0.0", true, false),
            ("10.0.0.1", true, false),
            ("100.64.0.1", false, false), // 100.64/10: both False (CGNAT)
            ("127.0.0.1", true, false),
            ("169.254.0.1", true, false),
            ("172.16.0.1", true, false),
            ("192.0.0.1", true, false),
            ("192.0.0.9", false, true), // exception inside 192.0.0.0/24
            ("192.0.0.10", false, true), // exception inside 192.0.0.0/24
            ("192.0.0.170", true, false),
            ("192.0.2.1", true, false),
            ("192.168.0.1", true, false),
            ("198.18.0.1", true, false),
            ("198.51.100.1", true, false),
            ("203.0.113.1", true, false),
            ("240.0.0.1", true, false),
            ("255.255.255.255", true, false),
            ("8.8.8.8", false, true),
            ("1.1.1.1", false, true),
        ];
        for &(s, priv_, glob) in cases {
            let a = v4(s);
            assert_eq!(ipv4_is_private(a), priv_, "is_private({s})");
            assert_eq!(ipv4_is_global(a), glob, "is_global({s})");
        }
    }

    #[test]
    fn v6_private_global_matches_cpython() {
        let cases: &[(&str, bool, bool)] = &[
            ("::", true, false),
            ("::1", true, false),
            ("2001:db8::1", true, false),
            ("fc00::1", true, false),
            ("fe80::1", true, false),
            ("ff00::1", false, true),
            ("2002::1", true, false),
            ("100::1", true, false),
            ("64:ff9b:1::1", true, false),
            ("2001::1", true, false),
            ("2001:1::1", false, true), // exception inside 2001::/23
            ("2001:db8::", true, false),
            ("3fff::1", true, false),
            ("8000::", false, true), // upper-half address (>= 2**127)
            ("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", false, true),
            // IPv4-mapped: delegates to the embedded IPv4 semantics.
            ("::ffff:192.168.1.1", true, false),
            ("::ffff:8.8.8.8", false, true),
        ];
        for &(s, priv_, glob) in cases {
            let a = v6(s);
            assert_eq!(ipv6_is_private(a), priv_, "is_private({s})");
            assert_eq!(ipv6_is_global(a), glob, "is_global({s})");
        }
    }

    #[test]
    fn bigint_to_fixed_be_v6_round_trip() {
        // 2**127 -> 0x8000...0000
        let n = BigInt::from(1u8) << 127;
        let octets = bigint_to_fixed_be::<16>(&n).expect("2**127 fits in 16 bytes");
        assert_eq!(Ipv6Addr::from(octets), v6("8000::"));
        assert_eq!(u128::from(Ipv6Addr::from(octets)), 1u128 << 127);

        // 2**128 - 1 -> all ones.
        let max = (BigInt::from(1u8) << 128) - 1;
        let octets = bigint_to_fixed_be::<16>(&max).expect("2**128-1 fits");
        assert_eq!(u128::from(Ipv6Addr::from(octets)), u128::MAX);

        // Zero.
        let zero = BigInt::from(0u8);
        assert_eq!(bigint_to_fixed_be::<16>(&zero), Some([0u8; 16]));
    }

    #[test]
    fn bigint_to_fixed_be_rejects_out_of_range() {
        // 2**128 is one past the max -> 17 magnitude bytes -> None.
        let over = BigInt::from(1u8) << 128;
        assert_eq!(bigint_to_fixed_be::<16>(&over), None);
        // Negative -> None.
        let neg = BigInt::from(-1i8);
        assert_eq!(bigint_to_fixed_be::<16>(&neg), None);
        assert_eq!(bigint_to_fixed_be::<4>(&neg), None);
        // IPv4: 2**32 is one past the max -> None.
        let v4_over = BigInt::from(1u64 << 32);
        assert_eq!(bigint_to_fixed_be::<4>(&v4_over), None);
        // IPv4: 2**32 - 1 fits.
        let v4_max = BigInt::from((1u64 << 32) - 1);
        assert_eq!(
            bigint_to_fixed_be::<4>(&v4_max).map(Ipv4Addr::from),
            Some(v4("255.255.255.255"))
        );
    }

    #[test]
    fn in_network_boundaries() {
        // /31 boundary: 192.0.0.170/31 covers .170 and .171 only.
        assert!(v4_in_network(u32::from(v4("192.0.0.170")), 0xC00000AA, 31));
        assert!(v4_in_network(u32::from(v4("192.0.0.171")), 0xC00000AA, 31));
        assert!(!v4_in_network(u32::from(v4("192.0.0.172")), 0xC00000AA, 31));
        assert!(!v4_in_network(u32::from(v4("192.0.0.169")), 0xC00000AA, 31));
        // prefix 0 matches everything; prefix 32 is exact.
        assert!(v4_in_network(0x12345678, 0, 0));
        assert!(v4_in_network(0xFFFFFFFF, 0xFFFFFFFF, 32));
        assert!(!v4_in_network(0xFFFFFFFE, 0xFFFFFFFF, 32));
        // v6 /7 fc00::/7 covers fc00:: and fd00:: but not fe00::.
        assert!(v6_in_network(
            u128::from(v6("fc00::1")),
            0xFC00_0000_0000_0000_0000_0000_0000_0000,
            7
        ));
        assert!(v6_in_network(
            u128::from(v6("fd00::1")),
            0xFC00_0000_0000_0000_0000_0000_0000_0000,
            7
        ));
        assert!(!v6_in_network(
            u128::from(v6("fe00::1")),
            0xFC00_0000_0000_0000_0000_0000_0000_0000,
            7
        ));
    }
}
