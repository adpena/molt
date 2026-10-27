//! Platform socket constants shared by stdlib exports and runtime helpers.

#[cfg(not(target_arch = "wasm32"))]
pub const AF_INET: i32 = 2;
#[cfg(target_arch = "wasm32")]
pub use crate::wasm_socket_abi::AF_INET;

#[cfg(target_arch = "wasm32")]
pub const AF_INET6: i32 = crate::libc_compat::AF_INET6;

#[cfg(target_os = "macos")]
pub const AF_INET6: i32 = 30;

#[cfg(windows)]
pub const AF_INET6: i32 = 23;

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos"), not(windows)))]
pub const AF_INET6: i32 = libc::AF_INET6;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub const SOCK_NONBLOCK_FLAG: i32 = libc::SOCK_NONBLOCK;
#[cfg(not(all(unix, any(target_os = "linux", target_os = "android"))))]
pub const SOCK_NONBLOCK_FLAG: i32 = 0;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub const SOCK_CLOEXEC_FLAG: i32 = libc::SOCK_CLOEXEC;
#[cfg(not(all(unix, any(target_os = "linux", target_os = "android"))))]
pub const SOCK_CLOEXEC_FLAG: i32 = 0;

#[cfg(target_arch = "wasm32")]
pub fn errno_constants(_python_version: u16) -> impl Iterator<Item = (&'static str, i64)> + Clone {
    crate::libc_compat::ERRNO_CONSTANTS.iter().copied()
}

#[cfg(not(target_arch = "wasm32"))]
include!(concat!(env!("OUT_DIR"), "/errno_constants.rs"));

fn socket_abi_constants() -> &'static [(&'static str, i64)] {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep wasm socket constants aligned with wasm/run_wasm.js host values so
        // stdlib consumers (e.g. socketserver/smtplib) do not observe missing
        // module attributes.
        crate::wasm_socket_abi::SOCKET_CONSTANTS
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[cfg(target_os = "macos")]
        {
            &[
                ("AF_APPLETALK", 16_i64),
                ("AF_DECnet", 12_i64),
                ("AF_INET", AF_INET as i64),
                ("AF_INET6", AF_INET6 as i64),
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
        #[cfg(windows)]
        {
            &[
                // Winsock address families, socket types, option levels, and
                // getaddrinfo/getnameinfo flags are ABI constants, not CRT
                // errno values; the Windows libc crate intentionally does not
                // expose them.
                ("AF_INET", AF_INET as i64),
                ("AF_INET6", AF_INET6 as i64),
                ("SOCK_STREAM", 1_i64),
                ("SOCK_DGRAM", 2_i64),
                ("SOCK_RAW", 3_i64),
                ("SOL_SOCKET", 0xffff_i64),
                ("SO_REUSEADDR", 0x0004_i64),
                ("SO_KEEPALIVE", 0x0008_i64),
                ("SO_SNDBUF", 0x1001_i64),
                ("SO_RCVBUF", 0x1002_i64),
                ("SO_ERROR", 0x1007_i64),
                ("SO_LINGER", 0x0080_i64),
                ("SO_BROADCAST", 0x0020_i64),
                ("IPPROTO_TCP", 6_i64),
                ("IPPROTO_UDP", 17_i64),
                ("IPPROTO_IPV6", 41_i64),
                ("IPV6_V6ONLY", 27_i64),
                ("TCP_NODELAY", 1_i64),
                ("SHUT_RD", 0_i64),
                ("SHUT_WR", 1_i64),
                ("SHUT_RDWR", 2_i64),
                ("AI_PASSIVE", 0x0001_i64),
                ("AI_CANONNAME", 0x0002_i64),
                ("AI_NUMERICHOST", 0x0004_i64),
                ("AI_NUMERICSERV", 0x0008_i64),
                ("NI_NUMERICHOST", 0x0002_i64),
                ("NI_NUMERICSERV", 0x0008_i64),
                ("MSG_PEEK", 0x0002_i64),
            ]
        }
        #[cfg(all(not(target_os = "macos"), not(windows)))]
        {
            &[
                ("AF_INET", AF_INET as i64),
                ("AF_INET6", AF_INET6 as i64),
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
                #[cfg(unix)]
                ("AF_UNIX", libc::AF_UNIX as i64),
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
                ("SCM_RIGHTS", libc::SCM_RIGHTS as i64),
                #[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
                ("SOCK_NONBLOCK", SOCK_NONBLOCK_FLAG as i64),
                #[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
                ("SOCK_CLOEXEC", SOCK_CLOEXEC_FLAG as i64),
                #[cfg(unix)]
                ("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64),
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
                ("SO_REUSEPORT", libc::SO_REUSEPORT as i64),
                #[cfg(target_os = "linux")]
                ("AF_ALG", 38_i64),
                #[cfg(target_os = "linux")]
                ("SOL_ALG", 279_i64),
                #[cfg(target_os = "linux")]
                ("ALG_SET_KEY", 1_i64),
                #[cfg(target_os = "linux")]
                ("ALG_SET_IV", 2_i64),
                #[cfg(target_os = "linux")]
                ("ALG_SET_OP", 3_i64),
                #[cfg(target_os = "linux")]
                ("ALG_SET_AEAD_ASSOCLEN", 4_i64),
                #[cfg(target_os = "linux")]
                ("ALG_SET_AEAD_AUTHSIZE", 5_i64),
                #[cfg(target_os = "linux")]
                ("ALG_OP_DECRYPT", 0_i64),
                #[cfg(target_os = "linux")]
                ("ALG_OP_ENCRYPT", 1_i64),
            ]
        }
    }
}

/// Borrow the selected ABI rows and the same SDK resolver values used at the
/// native resolver boundary; no allocation or copied EAI number table.
pub fn socket_constants() -> impl Iterator<Item = (&'static str, i64)> + Clone {
    let rows = socket_abi_constants().iter().copied();
    #[cfg(target_arch = "wasm32")]
    {
        rows
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        rows.chain(crate::native_error_codes::EAI_CONSTANTS.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(constants: &[(&'static str, i64)], name: &str) -> Option<i64> {
        constants
            .iter()
            .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
    }

    #[test]
    fn errno_constants_include_core_os_errors() {
        let constants: Vec<_> = errno_constants(312).collect();
        assert!(find(&constants, "EACCES").is_some());
        assert!(find(&constants, "ENOENT").is_some());
        assert!(find(&constants, "EWOULDBLOCK").is_some());
    }

    #[test]
    fn socket_constants_include_core_address_and_type_values() {
        let constants: Vec<_> = socket_constants().collect();
        assert_eq!(find(&constants, "AF_INET"), Some(AF_INET as i64));
        assert_eq!(find(&constants, "AF_INET6"), Some(AF_INET6 as i64));
        assert!(find(&constants, "SOCK_STREAM").is_some());
        assert!(find(&constants, "EAI_NONAME").is_some());
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(
            constants
                .iter()
                .copied()
                .filter(|(name, _)| name.starts_with("EAI_"))
                .collect::<Vec<_>>(),
            crate::native_error_codes::EAI_CONSTANTS,
            "publish every present SDK resolver symbol once, with no copied defaults"
        );
    }

    #[test]
    fn socket_flags_are_present_only_when_supported() {
        let constants: Vec<_> = socket_constants().collect();
        assert_eq!(
            find(&constants, "SOCK_NONBLOCK"),
            (SOCK_NONBLOCK_FLAG != 0).then_some(SOCK_NONBLOCK_FLAG as i64)
        );
        assert_eq!(
            find(&constants, "SOCK_CLOEXEC"),
            (SOCK_CLOEXEC_FLAG != 0).then_some(SOCK_CLOEXEC_FLAG as i64)
        );
    }
}
