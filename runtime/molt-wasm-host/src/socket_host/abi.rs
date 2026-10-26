use super::*;
use guest_socket_abi as guest;
#[cfg(unix)]
use libc as native;
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock as native;

pub(super) fn native_family(family: i32) -> Result<i32, GuestErrno> {
    use guest::GuestAddressFamily as Family;
    let family = match Family::from_wire(family).ok_or(GuestErrno::EAFNOSUPPORT)? {
        Family::AF_UNSPEC => native::AF_UNSPEC,
        Family::AF_INET => native::AF_INET,
        Family::AF_INET6 => native::AF_INET6,
        Family::AF_UNIX => {
            #[cfg(unix)]
            {
                native::AF_UNIX
            }
            #[cfg(windows)]
            {
                return Err(GuestErrno::EAFNOSUPPORT);
            }
        }
    };
    // Winsock uses u16 ADDRESS_FAMILY; Unix already returns c_int.
    #[cfg(windows)]
    let family = i32::from(family);
    Ok(family)
}

pub(super) fn native_type(kind: i32) -> Result<i32, GuestErrno> {
    use guest::GuestSocketType as Kind;
    match Kind::from_wire(kind).ok_or(GuestErrno::EPROTOTYPE)? {
        Kind::SOCK_STREAM => Ok(native::SOCK_STREAM),
        Kind::SOCK_DGRAM => Ok(native::SOCK_DGRAM),
        Kind::SOCK_RAW => Ok(native::SOCK_RAW),
    }
}

pub(super) fn native_protocol(protocol: i32) -> Result<Option<Protocol>, GuestErrno> {
    use guest::GuestProtocol as ProtocolName;
    match ProtocolName::from_wire(protocol).ok_or(GuestErrno::EPROTONOSUPPORT)? {
        ProtocolName::IPPROTO_IP => Ok(None),
        ProtocolName::IPPROTO_TCP => Ok(Some(Protocol::TCP)),
        ProtocolName::IPPROTO_UDP => Ok(Some(Protocol::UDP)),
        ProtocolName::IPPROTO_IPV6 => Ok(Some(Protocol::from(native::IPPROTO_IPV6))),
    }
}

pub(super) fn native_resolver_arguments(
    family: i32,
    kind: i32,
    protocol: i32,
    flags: i32,
) -> Result<(i32, i32, i32, i32), GuestResolverError> {
    let family = native_family(family).map_err(|_| GuestResolverError::EAI_FAMILY)?;
    let kind = if kind == 0 {
        0
    } else {
        native_type(kind).map_err(|_| GuestResolverError::EAI_SOCKTYPE)?
    };
    let protocol = native_protocol(protocol)
        .map_err(|_| GuestResolverError::EAI_SERVICE)?
        .map(i32::from)
        .unwrap_or(0);
    use guest::GuestResolverFlag as Flag;
    let flags = Flag::from_bits(flags).ok_or(GuestResolverError::EAI_BADFLAGS)?;
    let mut translated = 0;
    for flag in flags {
        translated |= match flag {
            Flag::AI_PASSIVE => native::AI_PASSIVE,
            Flag::AI_CANONNAME => native::AI_CANONNAME,
            Flag::AI_NUMERICHOST => native::AI_NUMERICHOST,
            Flag::AI_NUMERICSERV => native::AI_NUMERICSERV,
        };
    }
    // Winsock declares AI_* as u32, but ADDRINFOA.ai_flags is i32.
    #[cfg(windows)]
    let translated = i32::try_from(translated).map_err(|_| GuestResolverError::EAI_BADFLAGS)?;
    Ok((family, kind, protocol, translated))
}

#[derive(Clone, Copy)]
pub(super) enum MessageDirection {
    Receive,
    Send,
}

pub(super) fn native_message_flags(
    flags: i32,
    direction: MessageDirection,
) -> Result<i32, GuestErrno> {
    use guest::GuestMessageFlag as Flag;
    let flags = Flag::from_bits(flags).ok_or(GuestErrno::EOPNOTSUPP)?;
    let mut translated = 0;
    for flag in flags {
        translated |= match (flag, direction) {
            (Flag::MSG_PEEK, MessageDirection::Receive) => native::MSG_PEEK,
            (Flag::MSG_PEEK, MessageDirection::Send) => return Err(GuestErrno::EOPNOTSUPP),
            (Flag::MSG_DONTWAIT, MessageDirection::Send | MessageDirection::Receive) => {
                #[cfg(unix)]
                {
                    native::MSG_DONTWAIT
                }
                // Every host socket is already nonblocking; Winsock has no per-call flag.
                #[cfg(windows)]
                {
                    0
                }
            }
        };
    }
    Ok(translated)
}

#[derive(Clone, Copy)]
pub(super) enum SocketOption {
    Error,
    Linger,
    Native(i32, i32),
}

impl SocketOption {
    pub(super) fn native(self) -> (i32, i32) {
        match self {
            Self::Error => (native::SOL_SOCKET, native::SO_ERROR),
            Self::Linger => (native::SOL_SOCKET, native::SO_LINGER),
            Self::Native(level, name) => (level, name),
        }
    }
}

pub(super) fn native_socket_option(level: i32, name: i32) -> Result<SocketOption, GuestErrno> {
    use guest::GuestSocketOption as OptionName;
    let option = OptionName::from_wire(level, name).ok_or(GuestErrno::ENOPROTOOPT)?;
    let (native_level, native_name) = match option {
        OptionName::SO_ERROR => return Ok(SocketOption::Error),
        OptionName::SO_LINGER => return Ok(SocketOption::Linger),
        OptionName::SO_REUSEADDR => (native::SOL_SOCKET, native::SO_REUSEADDR),
        OptionName::SO_KEEPALIVE => (native::SOL_SOCKET, native::SO_KEEPALIVE),
        OptionName::SO_SNDBUF => (native::SOL_SOCKET, native::SO_SNDBUF),
        OptionName::SO_RCVBUF => (native::SOL_SOCKET, native::SO_RCVBUF),
        OptionName::SO_BROADCAST => (native::SOL_SOCKET, native::SO_BROADCAST),
        OptionName::SO_REUSEPORT => {
            #[cfg(unix)]
            {
                (native::SOL_SOCKET, native::SO_REUSEPORT)
            }
            #[cfg(windows)]
            {
                return Err(GuestErrno::ENOPROTOOPT);
            }
        }
        OptionName::TCP_NODELAY => (native::IPPROTO_TCP, native::TCP_NODELAY),
        OptionName::IPV6_V6ONLY => (native::IPPROTO_IPV6, native::IPV6_V6ONLY),
    };
    Ok(SocketOption::Native(native_level, native_name))
}

pub(super) fn decode_guest_linger(data: &[u8]) -> Result<Option<Duration>, GuestErrno> {
    if data.len() != 8 {
        return Err(GuestErrno::EINVAL);
    }
    let on = i32::from_le_bytes(data[..4].try_into().unwrap());
    if on == 0 {
        return Ok(None);
    }
    let seconds = i32::from_le_bytes(data[4..8].try_into().unwrap());
    let seconds = u64::try_from(seconds).map_err(|_| GuestErrno::EINVAL)?;
    Ok(Some(Duration::from_secs(seconds)))
}

pub(super) fn encode_guest_linger(value: Option<Duration>) -> Result<[u8; 8], GuestErrno> {
    let mut bytes = [0; 8];
    if let Some(value) = value {
        let seconds = i32::try_from(value.as_secs()).map_err(|_| GuestErrno::EOVERFLOW)?;
        bytes[..4].copy_from_slice(&1_i32.to_le_bytes());
        bytes[4..].copy_from_slice(&seconds.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(unix)]
pub(super) fn guest_message_result_flags(flags: i32) -> Result<i32, GuestErrno> {
    if flags == 0 {
        return Ok(0);
    }
    use guest::GuestMessageResultFlag as Flag;
    let mut remaining = flags;
    let mut translated = 0;
    for flag in Flag::ALL {
        let native_bit = match flag {
            Flag::MSG_TRUNC => native::MSG_TRUNC,
            Flag::MSG_CTRUNC => native::MSG_CTRUNC,
            Flag::MSG_EOR => native::MSG_EOR,
            Flag::MSG_OOB => native::MSG_OOB,
        };
        if remaining & native_bit != 0 {
            remaining &= !native_bit;
            translated |= flag.code();
        }
    }
    if remaining != 0 {
        return Err(GuestErrno::EOPNOTSUPP);
    }
    Ok(translated)
}

// Invert the existing native plans; no second native value table is published.
pub(super) fn guest_resolver_type(native_kind: i32) -> Result<i32, GuestResolverError> {
    if native_kind == 0 {
        return Ok(0);
    }
    guest::GuestSocketType::ALL
        .into_iter()
        .find(|kind| native_type(kind.code()) == Ok(native_kind))
        .map(|kind| kind.code())
        .ok_or(GuestResolverError::EAI_SOCKTYPE)
}
pub(super) fn guest_resolver_protocol(native_code: i32) -> Result<i32, GuestResolverError> {
    guest::GuestProtocol::ALL
        .into_iter()
        .find(|protocol| {
            native_protocol(protocol.code()).map(|value| value.map(i32::from).unwrap_or(0))
                == Ok(native_code)
        })
        .map(|protocol| protocol.code())
        .ok_or(GuestResolverError::EAI_SERVICE)
}
