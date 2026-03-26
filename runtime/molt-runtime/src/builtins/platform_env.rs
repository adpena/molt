use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::*;

use super::platform::{
    env_state, locale_state, os_name_str, process_env_state, sys_platform_str,
    trace_env_get, uuid_node, uuid_v1_bytes, uuid_v3_bytes, uuid_v4_bytes, uuid_v5_bytes,
    ERRNO_CONSTANTS_CACHE, OS_NAME_CACHE, SOCKET_CONSTANTS_CACHE, SYS_PLATFORM_CACHE,
    SOCK_NONBLOCK_FLAG, SOCK_CLOEXEC_FLAG,
};
use super::platform_importlib::{alloc_str_bits, bytes_arg_from_bits, locale_encoding_label};

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_getnode() -> u64 {
    crate::with_gil_entry!(_py, {
        match uuid_node() {
            Ok(node) => MoltObject::from_int(node as i64).bits(),
            Err(err) => raise_exception::<_>(_py, "RuntimeError", &err),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid4_bytes() -> u64 {
    crate::with_gil_entry!(_py, {
        let payload = match uuid_v4_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid1_bytes(node_bits: u64, clock_seq_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "time.wall") && !has_capability(_py, "time") {
            return raise_exception::<_>(_py, "PermissionError", "missing time.wall capability");
        }
        let node_override = if obj_from_bits(node_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, node_bits, "node must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0xFFFF_FFFF_FFFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "node is out of range (need a 48-bit value)",
                );
            }
            Some(value as u64)
        };
        let clock_seq_override = if obj_from_bits(clock_seq_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, clock_seq_bits, "clock_seq must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0x3FFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "clock_seq is out of range (need a 14-bit value)",
                );
            }
            Some(value as u16)
        };
        let payload = match uuid_v1_bytes(node_override, clock_seq_override) {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid3_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v3_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid5_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v5_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_name() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &OS_NAME_CACHE, || {
            let ptr = alloc_string(_py, os_name_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_platform() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &SYS_PLATFORM_CACHE, || {
            let ptr = alloc_string(_py, sys_platform_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_setlocale(_category_bits: u64, locale_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(locale_bits).is_none() {
            let current = locale_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            return match alloc_str_bits(_py, &current) {
                Ok(bits) => bits,
                Err(err_bits) => err_bits,
            };
        }
        let Some(mut locale) = string_obj_to_owned(obj_from_bits(locale_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "locale must be str or None");
        };
        if locale.is_empty() || locale == "C" || locale == "POSIX" {
            locale = String::from("C");
        }
        *locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = locale.clone();
        match alloc_str_bits(_py, &locale) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getpreferredencoding(_do_setlocale_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getlocale(_category_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if current == "C" || current == "POSIX" {
            let tuple_ptr =
                alloc_tuple(_py, &[MoltObject::none().bits(), MoltObject::none().bits()]);
            if tuple_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let locale_bits = match alloc_str_bits(_py, &current) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let encoding_bits = match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => {
                dec_ref_bits(_py, locale_bits);
                return err_bits;
            }
        };
        let tuple_ptr = alloc_tuple(_py, &[locale_bits, encoding_bits]);
        dec_ref_bits(_py, locale_bits);
        dec_ref_bits(_py, encoding_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gettext_gettext(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, message_bits);
        message_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gettext_ngettext(singular_bits: u64, plural_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let one = MoltObject::from_int(1);
        let result_bits = if obj_eq(_py, obj_from_bits(n_bits), one) {
            singular_bits
        } else {
            plural_bits
        };
        inc_ref_bits(_py, result_bits);
        result_bits
    })
}

#[cfg(target_arch = "wasm32")]
fn collect_errno_constants() -> Vec<(&'static str, i64)> {
    vec![
        ("EACCES", libc::EACCES as i64),
        ("EAGAIN", libc::EAGAIN as i64),
        ("EALREADY", libc::EALREADY as i64),
        ("EBADF", libc::EBADF as i64),
        ("ECHILD", libc::ECHILD as i64),
        ("ECONNABORTED", libc::ECONNABORTED as i64),
        ("ECONNREFUSED", libc::ECONNREFUSED as i64),
        ("ECONNRESET", libc::ECONNRESET as i64),
        ("EEXIST", libc::EEXIST as i64),
        ("EHOSTUNREACH", libc::EHOSTUNREACH as i64),
        ("EINPROGRESS", libc::EINPROGRESS as i64),
        ("EINTR", libc::EINTR as i64),
        ("EINVAL", libc::EINVAL as i64),
        ("EISDIR", libc::EISDIR as i64),
        ("ENOENT", libc::ENOENT as i64),
        ("ENOTDIR", libc::ENOTDIR as i64),
        ("EPERM", libc::EPERM as i64),
        ("EPIPE", libc::EPIPE as i64),
        ("ESRCH", libc::ESRCH as i64),
        ("ETIMEDOUT", libc::ETIMEDOUT as i64),
        ("EWOULDBLOCK", libc::EWOULDBLOCK as i64),
    ]
}

#[cfg(not(target_arch = "wasm32"))]
include!(concat!(env!("OUT_DIR"), "/errno_constants.rs"));

fn socket_constants() -> Vec<(&'static str, i64)> {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep wasm socket constants aligned with run_wasm.js host values so
        // stdlib consumers (e.g. socketserver/smtplib) do not observe missing
        // module attributes.
        vec![
            ("AF_UNIX", libc::AF_UNIX as i64),
            ("AF_INET", libc::AF_INET as i64),
            ("AF_INET6", libc::AF_INET6 as i64),
            ("SOCK_STREAM", libc::SOCK_STREAM as i64),
            ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
            ("SOCK_RAW", libc::SOCK_RAW as i64),
            ("SOL_SOCKET", libc::SOL_SOCKET as i64),
            ("SO_REUSEADDR", 2),
            ("SO_KEEPALIVE", 9),
            ("SO_SNDBUF", 7),
            ("SO_RCVBUF", 8),
            ("SO_ERROR", 4),
            ("SO_LINGER", 13),
            ("SO_BROADCAST", 6),
            ("SO_REUSEPORT", 15),
            ("IPPROTO_TCP", 6),
            ("IPPROTO_UDP", 17),
            ("IPPROTO_IPV6", 41),
            ("IPV6_V6ONLY", 26),
            ("TCP_NODELAY", 1),
            ("SHUT_RD", 0),
            ("SHUT_WR", 1),
            ("SHUT_RDWR", 2),
            ("AI_PASSIVE", 0x1),
            ("AI_CANONNAME", 0x2),
            ("AI_NUMERICHOST", 0x4),
            ("AI_NUMERICSERV", 0x400),
            ("NI_NUMERICHOST", 0x1),
            ("NI_NUMERICSERV", 0x2),
            ("MSG_PEEK", 2),
            ("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64),
            ("EAI_AGAIN", 2),
            ("EAI_FAIL", 4),
            ("EAI_FAMILY", 5),
            ("EAI_NONAME", libc::EAI_NONAME as i64),
            ("EAI_SERVICE", 9),
            ("EAI_SOCKTYPE", 10),
        ]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[cfg(target_os = "macos")]
        {
            vec![
                ("AF_APPLETALK", 16_i64),
                ("AF_DECnet", 12_i64),
                ("AF_INET", 2_i64),
                ("AF_INET6", 30_i64),
                ("AF_IPX", 23_i64),
                ("AF_LINK", 18_i64),
                ("AF_ROUTE", 17_i64),
                ("AF_SNA", 11_i64),
                ("AF_SYSTEM", 32_i64),
                ("AF_UNIX", 1_i64),
                ("AF_UNSPEC", 0_i64),
                ("AI_ADDRCONFIG", 1024_i64),
                ("AI_ALL", 256_i64),
                ("AI_CANONNAME", 2_i64),
                ("AI_DEFAULT", 1536_i64),
                ("AI_MASK", 5127_i64),
                ("AI_NUMERICHOST", 4_i64),
                ("AI_NUMERICSERV", 4096_i64),
                ("AI_PASSIVE", 1_i64),
                ("AI_V4MAPPED", 2048_i64),
                ("AI_V4MAPPED_CFG", 512_i64),
                ("EAI_ADDRFAMILY", 1_i64),
                ("EAI_AGAIN", 2_i64),
                ("EAI_BADFLAGS", 3_i64),
                ("EAI_BADHINTS", 12_i64),
                ("EAI_FAIL", 4_i64),
                ("EAI_FAMILY", 5_i64),
                ("EAI_MAX", 15_i64),
                ("EAI_MEMORY", 6_i64),
                ("EAI_NODATA", 7_i64),
                ("EAI_NONAME", 8_i64),
                ("EAI_OVERFLOW", 14_i64),
                ("EAI_PROTOCOL", 13_i64),
                ("EAI_SERVICE", 9_i64),
                ("EAI_SOCKTYPE", 10_i64),
                ("EAI_SYSTEM", 11_i64),
                ("ETHERTYPE_ARP", 2054_i64),
                ("ETHERTYPE_IP", 2048_i64),
                ("ETHERTYPE_IPV6", 34525_i64),
                ("ETHERTYPE_VLAN", 33024_i64),
                ("INADDR_ALLHOSTS_GROUP", 3758096385_i64),
                ("INADDR_ANY", 0_i64),
                ("INADDR_BROADCAST", 4294967295_i64),
                ("INADDR_LOOPBACK", 2130706433_i64),
                ("INADDR_MAX_LOCAL_GROUP", 3758096639_i64),
                ("INADDR_NONE", 4294967295_i64),
                ("INADDR_UNSPEC_GROUP", 3758096384_i64),
                ("IPPORT_RESERVED", 1024_i64),
                ("IPPORT_USERRESERVED", 5000_i64),
                ("IPPROTO_AH", 51_i64),
                ("IPPROTO_DSTOPTS", 60_i64),
                ("IPPROTO_EGP", 8_i64),
                ("IPPROTO_EON", 80_i64),
                ("IPPROTO_ESP", 50_i64),
                ("IPPROTO_FRAGMENT", 44_i64),
                ("IPPROTO_GGP", 3_i64),
                ("IPPROTO_GRE", 47_i64),
                ("IPPROTO_HELLO", 63_i64),
                ("IPPROTO_HOPOPTS", 0_i64),
                ("IPPROTO_ICMP", 1_i64),
                ("IPPROTO_ICMPV6", 58_i64),
                ("IPPROTO_IDP", 22_i64),
                ("IPPROTO_IGMP", 2_i64),
                ("IPPROTO_IP", 0_i64),
                ("IPPROTO_IPCOMP", 108_i64),
                ("IPPROTO_IPIP", 4_i64),
                ("IPPROTO_IPV4", 4_i64),
                ("IPPROTO_IPV6", 41_i64),
                ("IPPROTO_MAX", 256_i64),
                ("IPPROTO_ND", 77_i64),
                ("IPPROTO_NONE", 59_i64),
                ("IPPROTO_PIM", 103_i64),
                ("IPPROTO_PUP", 12_i64),
                ("IPPROTO_RAW", 255_i64),
                ("IPPROTO_ROUTING", 43_i64),
                ("IPPROTO_RSVP", 46_i64),
                ("IPPROTO_SCTP", 132_i64),
                ("IPPROTO_TCP", 6_i64),
                ("IPPROTO_TP", 29_i64),
                ("IPPROTO_UDP", 17_i64),
                ("IPPROTO_XTP", 36_i64),
                ("IPV6_CHECKSUM", 26_i64),
                ("IPV6_DONTFRAG", 62_i64),
                ("IPV6_DSTOPTS", 50_i64),
                ("IPV6_HOPLIMIT", 47_i64),
                ("IPV6_HOPOPTS", 49_i64),
                ("IPV6_JOIN_GROUP", 12_i64),
                ("IPV6_LEAVE_GROUP", 13_i64),
                ("IPV6_MULTICAST_HOPS", 10_i64),
                ("IPV6_MULTICAST_IF", 9_i64),
                ("IPV6_MULTICAST_LOOP", 11_i64),
                ("IPV6_NEXTHOP", 48_i64),
                ("IPV6_PATHMTU", 44_i64),
                ("IPV6_PKTINFO", 46_i64),
                ("IPV6_RECVDSTOPTS", 40_i64),
                ("IPV6_RECVHOPLIMIT", 37_i64),
                ("IPV6_RECVHOPOPTS", 39_i64),
                ("IPV6_RECVPATHMTU", 43_i64),
                ("IPV6_RECVPKTINFO", 61_i64),
                ("IPV6_RECVRTHDR", 38_i64),
                ("IPV6_RECVTCLASS", 35_i64),
                ("IPV6_RTHDR", 51_i64),
                ("IPV6_RTHDRDSTOPTS", 57_i64),
                ("IPV6_RTHDR_TYPE_0", 0_i64),
                ("IPV6_TCLASS", 36_i64),
                ("IPV6_UNICAST_HOPS", 4_i64),
                ("IPV6_USE_MIN_MTU", 42_i64),
                ("IPV6_V6ONLY", 27_i64),
                ("IP_ADD_MEMBERSHIP", 12_i64),
                ("IP_ADD_SOURCE_MEMBERSHIP", 70_i64),
                ("IP_BLOCK_SOURCE", 72_i64),
                ("IP_DEFAULT_MULTICAST_LOOP", 1_i64),
                ("IP_DEFAULT_MULTICAST_TTL", 1_i64),
                ("IP_DROP_MEMBERSHIP", 13_i64),
                ("IP_DROP_SOURCE_MEMBERSHIP", 71_i64),
                ("IP_HDRINCL", 2_i64),
                ("IP_MAX_MEMBERSHIPS", 4095_i64),
                ("IP_MULTICAST_IF", 9_i64),
                ("IP_MULTICAST_LOOP", 11_i64),
                ("IP_MULTICAST_TTL", 10_i64),
                ("IP_OPTIONS", 1_i64),
                ("IP_PKTINFO", 26_i64),
                ("IP_RECVDSTADDR", 7_i64),
                ("IP_RECVOPTS", 5_i64),
                ("IP_RECVRETOPTS", 6_i64),
                ("IP_RECVTOS", 27_i64),
                ("IP_RETOPTS", 8_i64),
                ("IP_TOS", 3_i64),
                ("IP_TTL", 4_i64),
                ("IP_UNBLOCK_SOURCE", 73_i64),
                ("LOCAL_PEERCRED", 1_i64),
                ("MSG_CTRUNC", 32_i64),
                ("MSG_DONTROUTE", 4_i64),
                ("MSG_DONTWAIT", 128_i64),
                ("MSG_EOF", 256_i64),
                ("MSG_EOR", 8_i64),
                ("MSG_NOSIGNAL", 524288_i64),
                ("MSG_OOB", 1_i64),
                ("MSG_PEEK", 2_i64),
                ("MSG_TRUNC", 16_i64),
                ("MSG_WAITALL", 64_i64),
                ("NI_DGRAM", 16_i64),
                ("NI_MAXHOST", 1025_i64),
                ("NI_MAXSERV", 32_i64),
                ("NI_NAMEREQD", 4_i64),
                ("NI_NOFQDN", 1_i64),
                ("NI_NUMERICHOST", 2_i64),
                ("NI_NUMERICSERV", 8_i64),
                ("PF_SYSTEM", 32_i64),
                ("SCM_CREDS", 3_i64),
                ("SCM_RIGHTS", 1_i64),
                ("SHUT_RD", 0_i64),
                ("SHUT_RDWR", 2_i64),
                ("SHUT_WR", 1_i64),
                ("SOCK_DGRAM", 2_i64),
                ("SOCK_RAW", 3_i64),
                ("SOCK_RDM", 4_i64),
                ("SOCK_SEQPACKET", 5_i64),
                ("SOCK_STREAM", 1_i64),
                ("SOL_IP", 0_i64),
                ("SOL_SOCKET", 65535_i64),
                ("SOL_TCP", 6_i64),
                ("SOL_UDP", 17_i64),
                ("SOMAXCONN", 128_i64),
                ("SO_ACCEPTCONN", 2_i64),
                ("SO_BINDTODEVICE", 4404_i64),
                ("SO_BROADCAST", 32_i64),
                ("SO_DEBUG", 1_i64),
                ("SO_DONTROUTE", 16_i64),
                ("SO_ERROR", 4103_i64),
                ("SO_KEEPALIVE", 8_i64),
                ("SO_LINGER", 128_i64),
                ("SO_OOBINLINE", 256_i64),
                ("SO_RCVBUF", 4098_i64),
                ("SO_RCVLOWAT", 4100_i64),
                ("SO_RCVTIMEO", 4102_i64),
                ("SO_REUSEADDR", 4_i64),
                ("SO_REUSEPORT", 512_i64),
                ("SO_SNDBUF", 4097_i64),
                ("SO_SNDLOWAT", 4099_i64),
                ("SO_SNDTIMEO", 4101_i64),
                ("SO_TYPE", 4104_i64),
                ("SO_USELOOPBACK", 64_i64),
                ("SYSPROTO_CONTROL", 2_i64),
                ("TCP_CONNECTION_INFO", 262_i64),
                ("TCP_FASTOPEN", 261_i64),
                ("TCP_KEEPALIVE", 16_i64),
                ("TCP_KEEPCNT", 258_i64),
                ("TCP_KEEPINTVL", 257_i64),
                ("TCP_MAXSEG", 2_i64),
                ("TCP_NODELAY", 1_i64),
                ("TCP_NOTSENT_LOWAT", 513_i64),
            ]
        }
        #[cfg(not(target_os = "macos"))]
        {
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
                out.push(("SCM_RIGHTS", libc::SCM_RIGHTS as i64));
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
            // AF_ALG constants (kernel crypto API, Linux only)
            #[cfg(target_os = "linux")]
            {
                out.push(("AF_ALG", 38_i64));
                out.push(("SOL_ALG", 279_i64));
                out.push(("ALG_SET_KEY", 1_i64));
                out.push(("ALG_SET_IV", 2_i64));
                out.push(("ALG_SET_OP", 3_i64));
                out.push(("ALG_SET_AEAD_ASSOCLEN", 4_i64));
                out.push(("ALG_SET_AEAD_AUTHSIZE", 5_i64));
                out.push(("ALG_OP_DECRYPT", 0_i64));
                out.push(("ALG_OP_ENCRYPT", 1_i64));
            }
            out
        }
    }
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return default_bits,
        };
        let value = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.get(&key).cloned()
        };
        match value {
            Some(val) => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=true");
                }
                let ptr = alloc_string(_py, val.as_bytes());
                if ptr.is_null() {
                    default_bits
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            None => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=false");
                }
                default_bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_set(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_unset(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let removed = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key).is_some()
        };
        MoltObject::from_bool(removed).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_len() -> u64 {
    crate::with_gil_entry!(_py, {
        let len = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.len()
        };
        MoltObject::from_int(len as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_contains(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let contains = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.contains_key(&key)
        };
        MoltObject::from_bool(contains).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_snapshot() -> u64 {
    crate::with_gil_entry!(_py, {
        let env_pairs: Vec<(String, String)> = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .iter()
                .map(|(key, val)| (key.clone(), val.clone()))
                .collect()
        };
        let mut pairs = Vec::with_capacity(env_pairs.len() * 2);
        let mut owned_bits = Vec::with_capacity(env_pairs.len() * 2);
        for (key, val) in env_pairs {
            let key_ptr = alloc_string(_py, key.as_bytes());
            let val_ptr = alloc_string(_py, val.as_bytes());
            if key_ptr.is_null() || val_ptr.is_null() {
                if !key_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
                }
                if !val_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(val_ptr).bits());
                }
                continue;
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let val_bits = MoltObject::from_ptr(val_ptr).bits();
            pairs.push(key_bits);
            pairs.push(val_bits);
            owned_bits.push(key_bits);
            owned_bits.push(val_bits);
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_popitem() -> u64 {
    crate::with_gil_entry!(_py, {
        let (key, value) = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some((key, value)) = guard
                .iter()
                .next_back()
                .map(|(key, value)| (key.clone(), value.clone()))
            else {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            };
            guard.remove(&key);
            (key, value)
        };
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
            return MoltObject::none().bits();
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_clear() -> u64 {
    crate::with_gil_entry!(_py, {
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.clear();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_putenv(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_unsetenv(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key);
        }
        MoltObject::none().bits()
    })
}

