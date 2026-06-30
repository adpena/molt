use super::*;

type AddrInfoEntry = (i32, i32, i32, Vec<u8>, Vec<u8>);

fn decode_sockaddr(buf: &[u8]) -> Result<SockAddr> {
    if buf.len() < 4 {
        bail!("sockaddr buffer too small");
    }
    let family = u16::from_le_bytes([buf[0], buf[1]]) as i32;
    let port = u16::from_le_bytes([buf[2], buf[3]]);
    if family == HOST_AF_INET {
        if buf.len() < 8 {
            bail!("invalid IPv4 sockaddr");
        }
        let mut octets = [0u8; 4];
        octets.copy_from_slice(&buf[4..8]);
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(octets), port));
        return Ok(SockAddr::from(addr));
    }
    if family == HOST_AF_INET6 {
        if buf.len() < 28 {
            bail!("invalid IPv6 sockaddr");
        }
        let flowinfo = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let scope_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&buf[12..28]);
        let addr = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(octets),
            port,
            flowinfo,
            scope_id,
        ));
        return Ok(SockAddr::from(addr));
    }
    bail!("unsupported address family");
}

fn encode_sockaddr(addr: &SockAddr) -> Result<Vec<u8>> {
    let Some(socket_addr) = addr.as_socket() else {
        bail!("unsupported sockaddr");
    };
    let mut out = Vec::new();
    match socket_addr {
        SocketAddr::V4(addr) => {
            out.extend_from_slice(&(HOST_AF_INET as u16).to_le_bytes());
            out.extend_from_slice(&addr.port().to_le_bytes());
            out.extend_from_slice(&addr.ip().octets());
        }
        SocketAddr::V6(addr) => {
            out.extend_from_slice(&(HOST_AF_INET6 as u16).to_le_bytes());
            out.extend_from_slice(&addr.port().to_le_bytes());
            out.extend_from_slice(&addr.flowinfo().to_le_bytes());
            out.extend_from_slice(&addr.scope_id().to_le_bytes());
            out.extend_from_slice(&addr.ip().octets());
        }
    }
    Ok(out)
}

fn encode_addrinfo_entries(entries: Vec<AddrInfoEntry>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (family, sock_type, proto, canon, addr) in entries {
        payload.extend_from_slice(&family.to_le_bytes());
        payload.extend_from_slice(&sock_type.to_le_bytes());
        payload.extend_from_slice(&proto.to_le_bytes());
        payload.extend_from_slice(&(canon.len() as u32).to_le_bytes());
        payload.extend_from_slice(&canon);
        payload.extend_from_slice(&(addr.len() as u32).to_le_bytes());
        payload.extend_from_slice(&addr);
    }
    payload
}

#[cfg(unix)]
fn host_getaddrinfo_payload(
    host_cstr: Option<&CString>,
    serv_cstr: Option<&CString>,
    family: i32,
    sock_type: i32,
    proto: i32,
    flags: i32,
) -> std::result::Result<Vec<u8>, i32> {
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = family;
    hints.ai_socktype = sock_type;
    hints.ai_protocol = proto;
    hints.ai_flags = flags;
    let mut res: *mut libc::addrinfo = std::ptr::null_mut();
    let err = unsafe {
        libc::getaddrinfo(
            host_cstr.map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            serv_cstr.map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
            &hints as *const libc::addrinfo,
            &mut res as *mut *mut libc::addrinfo,
        )
    };
    if err != 0 {
        return Err(err);
    }
    let mut entries: Vec<AddrInfoEntry> = Vec::new();
    let mut cur = res;
    while !cur.is_null() {
        let ai = unsafe { &*cur };
        if ai.ai_addr.is_null() {
            cur = ai.ai_next;
            continue;
        }
        if ai.ai_family != HOST_AF_INET && ai.ai_family != HOST_AF_INET6 {
            cur = ai.ai_next;
            continue;
        }
        let mut storage = SockAddrStorage::zeroed();
        unsafe {
            std::ptr::copy_nonoverlapping(
                ai.ai_addr as *const u8,
                storage.view_as::<libc::sockaddr_storage>() as *mut _ as *mut u8,
                ai.ai_addrlen as usize,
            );
        }
        let sockaddr = unsafe { SockAddr::new(storage, ai.ai_addrlen) };
        let addr_bytes = match encode_sockaddr(&sockaddr) {
            Ok(val) => val,
            Err(_) => {
                cur = ai.ai_next;
                continue;
            }
        };
        let canon = if !ai.ai_canonname.is_null() {
            unsafe { CStr::from_ptr(ai.ai_canonname) }
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else {
            Vec::new()
        };
        entries.push((
            ai.ai_family,
            ai.ai_socktype,
            ai.ai_protocol,
            canon,
            addr_bytes,
        ));
        cur = ai.ai_next;
    }
    unsafe { libc::freeaddrinfo(res) };
    Ok(encode_addrinfo_entries(entries))
}

#[cfg(windows)]
fn host_getaddrinfo_payload(
    host_cstr: Option<&CString>,
    serv_cstr: Option<&CString>,
    family: i32,
    sock_type: i32,
    proto: i32,
    flags: i32,
) -> std::result::Result<Vec<u8>, i32> {
    let mut hints = winsock::ADDRINFOA {
        ai_flags: flags,
        ai_family: family,
        ai_socktype: sock_type,
        ai_protocol: proto,
        ai_addrlen: 0,
        ai_canonname: std::ptr::null_mut(),
        ai_addr: std::ptr::null_mut(),
        ai_next: std::ptr::null_mut(),
    };
    let mut res: *mut winsock::ADDRINFOA = std::ptr::null_mut();
    let err = unsafe {
        winsock::getaddrinfo(
            host_cstr
                .map(|s| s.as_ptr().cast())
                .unwrap_or(std::ptr::null()),
            serv_cstr
                .map(|s| s.as_ptr().cast())
                .unwrap_or(std::ptr::null()),
            &mut hints as *mut winsock::ADDRINFOA,
            &mut res as *mut *mut winsock::ADDRINFOA,
        )
    };
    if err != 0 {
        return Err(err);
    }
    let mut entries: Vec<AddrInfoEntry> = Vec::new();
    let mut cur = res;
    while !cur.is_null() {
        let ai = unsafe { &*cur };
        if ai.ai_addr.is_null() {
            cur = ai.ai_next;
            continue;
        }
        if ai.ai_family != HOST_AF_INET && ai.ai_family != HOST_AF_INET6 {
            cur = ai.ai_next;
            continue;
        }
        let mut storage = SockAddrStorage::zeroed();
        unsafe {
            std::ptr::copy_nonoverlapping(
                ai.ai_addr as *const u8,
                storage.view_as::<winsock::SOCKADDR_STORAGE>() as *mut _ as *mut u8,
                ai.ai_addrlen,
            );
        }
        let sockaddr = unsafe { SockAddr::new(storage, ai.ai_addrlen as socklen_t) };
        let addr_bytes = match encode_sockaddr(&sockaddr) {
            Ok(val) => val,
            Err(_) => {
                cur = ai.ai_next;
                continue;
            }
        };
        let canon = if !ai.ai_canonname.is_null() {
            unsafe { CStr::from_ptr(ai.ai_canonname.cast()) }
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else {
            Vec::new()
        };
        entries.push((
            ai.ai_family,
            ai.ai_socktype,
            ai.ai_protocol,
            canon,
            addr_bytes,
        ));
        cur = ai.ai_next;
    }
    unsafe { winsock::freeaddrinfo(res) };
    Ok(encode_addrinfo_entries(entries))
}

#[cfg(unix)]
fn host_gethostname_bytes() -> std::result::Result<Vec<u8>, i32> {
    let mut buf = vec![0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return Err(map_io_error(&std::io::Error::last_os_error()));
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    buf.truncate(len);
    Ok(buf)
}

#[cfg(windows)]
fn host_gethostname_bytes() -> std::result::Result<Vec<u8>, i32> {
    let mut buf = vec![0u8; 256];
    let rc = unsafe { winsock::gethostname(buf.as_mut_ptr(), buf.len() as i32) };
    if rc != 0 {
        return Err(map_io_error(&std::io::Error::last_os_error()));
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    buf.truncate(len);
    Ok(buf)
}

#[cfg(unix)]
fn host_getservbyname_port(
    name_cstr: &CString,
    proto_cstr: Option<&CString>,
) -> std::result::Result<i32, i32> {
    let serv = unsafe {
        libc::getservbyname(
            name_cstr.as_ptr(),
            proto_cstr.map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
        )
    };
    if serv.is_null() {
        return Err(libc::ENOENT);
    }
    Ok(unsafe { libc::ntohs((*serv).s_port as u16) as i32 })
}

#[cfg(windows)]
fn host_getservbyname_port(
    name_cstr: &CString,
    proto_cstr: Option<&CString>,
) -> std::result::Result<i32, i32> {
    let serv = unsafe {
        winsock::getservbyname(
            name_cstr.as_ptr().cast(),
            proto_cstr
                .map(|s| s.as_ptr().cast())
                .unwrap_or(std::ptr::null()),
        )
    };
    if serv.is_null() {
        return Err(libc::ENOENT);
    }
    Ok(unsafe { winsock::ntohs((*serv).s_port as u16) as i32 })
}

#[cfg(unix)]
fn host_getservbyport_name(
    port: i32,
    proto_cstr: Option<&CString>,
) -> std::result::Result<Vec<u8>, i32> {
    let serv = unsafe {
        libc::getservbyport(
            libc::htons(port as u16) as i32,
            proto_cstr.map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
        )
    };
    if serv.is_null() {
        return Err(libc::ENOENT);
    }
    Ok(unsafe { CStr::from_ptr((*serv).s_name) }
        .to_bytes()
        .to_vec())
}

#[cfg(windows)]
fn host_getservbyport_name(
    port: i32,
    proto_cstr: Option<&CString>,
) -> std::result::Result<Vec<u8>, i32> {
    let serv = unsafe {
        winsock::getservbyport(
            winsock::htons(port as u16) as i32,
            proto_cstr
                .map(|s| s.as_ptr().cast())
                .unwrap_or(std::ptr::null()),
        )
    };
    if serv.is_null() {
        return Err(libc::ENOENT);
    }
    Ok(unsafe { CStr::from_ptr((*serv).s_name.cast()) }
        .to_bytes()
        .to_vec())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
type MsgControlLen = usize;

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
type MsgControlLen = libc::socklen_t;

#[cfg(unix)]
fn msg_controllen_from_usize(len: usize) -> Option<MsgControlLen> {
    MsgControlLen::try_from(len).ok()
}

#[cfg(unix)]
fn msg_controllen_to_guest_len(len: MsgControlLen) -> Option<u32> {
    u32::try_from(len).ok()
}

fn socket_get_mut(state: &mut HostState, handle: i64) -> Result<&mut Socket, i32> {
    if handle <= 0 {
        return Err(libc::EBADF);
    }
    state
        .socket_manager
        .get_mut(handle as u64)
        .ok_or(libc::EBADF)
}

fn poll_socket(socket: &Socket, events: u32, timeout_ms: i32) -> Result<u32, i32> {
    let mut poll_events: i16 = 0;
    if (events & 1) != 0 {
        poll_events |= HOST_POLLIN;
    }
    if (events & 2) != 0 {
        poll_events |= HOST_POLLOUT;
    }
    if poll_events == 0 {
        poll_events |= HOST_POLLIN;
    }
    #[cfg(unix)]
    {
        let fd = socket.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= 4 | 1 | 2;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= 1;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= 2;
        }
        Ok(ready)
    }
    #[cfg(windows)]
    {
        let fd = socket.as_raw_socket() as usize;
        let mut pfd = winsock::WSAPOLLFD {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { winsock::WSAPoll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= 4 | 1 | 2;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= 1;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= 2;
        }
        Ok(ready)
    }
}

pub(super) fn define_socket_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let socket_new = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         family: i32,
         sock_type: i32,
         proto: i32,
         fileno: i64|
         -> i64 {
            let domain = match family {
                x if x == HOST_AF_INET => Domain::IPV4,
                x if x == HOST_AF_INET6 => Domain::IPV6,
                x if x == HOST_AF_UNIX => {
                    #[cfg(unix)]
                    {
                        Domain::UNIX
                    }
                    #[cfg(not(unix))]
                    {
                        return -(libc::EAFNOSUPPORT as i64);
                    }
                }
                _ => return -(libc::EAFNOSUPPORT as i64),
            };
            let ty = Type::from(sock_type);
            let protocol = if proto == 0 {
                None
            } else {
                Some(Protocol::from(proto))
            };
            let socket = if fileno >= 0 {
                #[cfg(unix)]
                unsafe {
                    Socket::from_raw_fd(fileno as RawFd)
                }
                #[cfg(windows)]
                unsafe {
                    Socket::from_raw_socket(fileno as RawSocket)
                }
            } else {
                match Socket::new(domain, ty, protocol) {
                    Ok(sock) => sock,
                    Err(err) => return -(map_io_error(&err) as i64),
                }
            };
            if let Err(err) = socket.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let id = caller.data_mut().socket_manager.insert(socket);
            id as i64
        },
    );
    let socket_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            if caller
                .data_mut()
                .socket_manager
                .remove(handle as u64)
                .is_none()
            {
                return -libc::EBADF;
            }
            0
        },
    );
    let socket_clone = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i64 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -(errno as i64),
            };
            let cloned = match socket.try_clone() {
                Ok(sock) => sock,
                Err(err) => return -(map_io_error(&err) as i64),
            };
            if let Err(err) = cloned.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let id = caller.data_mut().socket_manager.insert(cloned);
            id as i64
        },
    );
    let socket_bind = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, addr_ptr: i32, addr_len: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.bind(&addr) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_listen = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, backlog: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.listen(backlog) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_accept = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i64 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -(libc::EFAULT as i64),
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -(errno as i64),
            };
            let (accepted, addr) = match socket.accept() {
                Ok(pair) => pair,
                Err(err) => return -(map_io_error(&err) as i64),
            };
            if let Err(err) = accepted.set_nonblocking(true) {
                return -(map_io_error(&err) as i64);
            }
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -(libc::ENOMEM as i64);
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -(libc::EFAULT as i64);
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            let id = caller.data_mut().socket_manager.insert(accepted);
            id as i64
        },
    );
    let socket_connect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, addr_ptr: i32, addr_len: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.connect(&addr) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_connect_ex = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.take_error() {
                Ok(None) => 0,
                Ok(Some(err)) => map_io_error(&err),
                Err(err) => map_io_error(&err),
            }
        },
    );
    let socket_recv = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![MaybeUninit::<u8>::uninit(); buf_len.max(0) as usize];
            match socket.recv_with_flags(&mut buf, flags) {
                Ok(n) => {
                    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n) };
                    if write_bytes(&mut caller, &memory, buf_ptr, bytes).is_err() {
                        return -libc::EFAULT;
                    }
                    n as i32
                }
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_send = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, buf_ptr, buf_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.send_with_flags(&data, flags) {
                Ok(n) => n as i32,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_sendto = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, buf_ptr, buf_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = match decode_sockaddr(&addr_bytes) {
                Ok(addr) => addr,
                Err(_) => return -libc::EAFNOSUPPORT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match socket.send_to_with_flags(&data, &addr, flags) {
                Ok(n) => n as i32,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_sendmsg = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_len: i32,
         anc_ptr: i32,
         anc_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, buf_ptr, buf_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let addr = if addr_len > 0 {
                let addr_bytes = match read_bytes(&mut caller, &memory, addr_ptr, addr_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                };
                match decode_sockaddr(&addr_bytes) {
                    Ok(addr) => Some(addr),
                    Err(_) => return -libc::EAFNOSUPPORT,
                }
            } else {
                None
            };
            let ancillary = if anc_len > 0 {
                match read_bytes(&mut caller, &memory, anc_ptr, anc_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                Vec::new()
            };
            #[cfg(unix)]
            let mut ancillary = ancillary;
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            #[cfg(windows)]
            {
                let _ = (socket, data, addr, ancillary, flags);
                -libc::ENOSYS
            }
            #[cfg(unix)]
            {
                let rc = {
                    let fd = socket.as_raw_fd();
                    let mut iov = libc::iovec {
                        iov_base: data.as_ptr() as *mut libc::c_void,
                        iov_len: data.len(),
                    };
                    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
                    msg.msg_iov = &mut iov as *mut libc::iovec;
                    msg.msg_iovlen = 1;
                    if let Some(ref addr) = addr {
                        msg.msg_name = addr.as_ptr() as *mut libc::c_void;
                        msg.msg_namelen = addr.len();
                    }
                    if !ancillary.is_empty() {
                        msg.msg_control = ancillary.as_mut_ptr() as *mut libc::c_void;
                        msg.msg_controllen = match msg_controllen_from_usize(ancillary.len()) {
                            Some(len) => len,
                            None => return -libc::EOVERFLOW,
                        };
                    }
                    unsafe { libc::sendmsg(fd, &msg as *const libc::msghdr, flags) }
                };
                if rc >= 0 {
                    return rc as i32;
                }
                -map_io_error(&std::io::Error::last_os_error())
            }
        },
    );
    let socket_recvfrom = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![MaybeUninit::<u8>::uninit(); buf_len.max(0) as usize];
            match socket.recv_from_with_flags(&mut buf, flags) {
                Ok((n, addr)) => {
                    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n) };
                    if write_bytes(&mut caller, &memory, buf_ptr, bytes).is_err() {
                        return -libc::EFAULT;
                    }
                    let encoded = encode_sockaddr(&addr).unwrap_or_default();
                    if encoded.len() > addr_cap as usize {
                        let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                        return -libc::ENOMEM;
                    }
                    if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                        return -libc::EFAULT;
                    }
                    let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                    n as i32
                }
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_recvmsg = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_len: i32,
         flags: i32,
         addr_ptr: i32,
         addr_cap: i32,
         out_addr_len_ptr: i32,
         anc_ptr: i32,
         anc_cap: i32,
         out_anc_len_ptr: i32,
         out_msg_flags_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            #[cfg(windows)]
            {
                let _ = (
                    socket,
                    buf_ptr,
                    buf_len,
                    flags,
                    addr_ptr,
                    addr_cap,
                    out_addr_len_ptr,
                    anc_ptr,
                    anc_cap,
                    out_anc_len_ptr,
                    out_msg_flags_ptr,
                );
                if out_addr_len_ptr != 0 {
                    let _ = write_u32(&mut caller, &memory, out_addr_len_ptr, 0);
                }
                if out_anc_len_ptr != 0 {
                    let _ = write_u32(&mut caller, &memory, out_anc_len_ptr, 0);
                }
                if out_msg_flags_ptr != 0 {
                    let _ = write_u32(&mut caller, &memory, out_msg_flags_ptr, 0);
                }
                -libc::ENOSYS
            }
            #[cfg(unix)]
            {
                let mut buf = vec![0u8; buf_len.max(0) as usize];
                let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut name_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let mut ancillary = vec![0u8; anc_cap.max(0) as usize];
                let rc = {
                    let fd = socket.as_raw_fd();
                    let mut iov = libc::iovec {
                        iov_base: buf.as_mut_ptr() as *mut libc::c_void,
                        iov_len: buf.len(),
                    };
                    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
                    msg.msg_name = &mut storage as *mut _ as *mut libc::c_void;
                    msg.msg_namelen = name_len;
                    msg.msg_iov = &mut iov as *mut libc::iovec;
                    msg.msg_iovlen = 1;
                    if !ancillary.is_empty() {
                        msg.msg_control = ancillary.as_mut_ptr() as *mut libc::c_void;
                        msg.msg_controllen = match msg_controllen_from_usize(ancillary.len()) {
                            Some(len) => len,
                            None => return -libc::EOVERFLOW,
                        };
                    }
                    let rc = unsafe { libc::recvmsg(fd, &mut msg as *mut libc::msghdr, flags) };
                    name_len = msg.msg_namelen;
                    if out_msg_flags_ptr != 0 {
                        let _ = write_u32(
                            &mut caller,
                            &memory,
                            out_msg_flags_ptr,
                            msg.msg_flags as u32,
                        );
                    }
                    if out_anc_len_ptr != 0 {
                        let Some(guest_anc_len) = msg_controllen_to_guest_len(msg.msg_controllen)
                        else {
                            return -libc::EOVERFLOW;
                        };
                        let _ = write_u32(&mut caller, &memory, out_anc_len_ptr, guest_anc_len);
                    }
                    rc
                };
                if rc >= 0 {
                    let n = rc as usize;
                    if write_bytes(&mut caller, &memory, buf_ptr, &buf[..n]).is_err() {
                        return -libc::EFAULT;
                    }
                    let mut addr_storage = SockAddrStorage::zeroed();
                    unsafe {
                        *addr_storage.view_as::<libc::sockaddr_storage>() = storage;
                    }
                    let addr = unsafe { SockAddr::new(addr_storage, name_len) };
                    let encoded = encode_sockaddr(&addr).unwrap_or_default();
                    if out_addr_len_ptr != 0 {
                        let _ =
                            write_u32(&mut caller, &memory, out_addr_len_ptr, encoded.len() as u32);
                    }
                    if encoded.len() > addr_cap.max(0) as usize {
                        return -libc::ENOMEM;
                    }
                    if !encoded.is_empty()
                        && write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err()
                    {
                        return -libc::EFAULT;
                    }
                    let anc_len_usize = ancillary.len().min(anc_cap.max(0) as usize);
                    if anc_len_usize > 0
                        && write_bytes(&mut caller, &memory, anc_ptr, &ancillary[..anc_len_usize])
                            .is_err()
                    {
                        return -libc::EFAULT;
                    }
                    return n as i32;
                }
                -map_io_error(&std::io::Error::last_os_error())
            }
        },
    );
    let socket_shutdown = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, how: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let how = match how {
                x if x == HOST_SHUT_RD => std::net::Shutdown::Read,
                x if x == HOST_SHUT_WR => std::net::Shutdown::Write,
                _ => std::net::Shutdown::Both,
            };
            match socket.shutdown(how) {
                Ok(()) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );
    let socket_getsockname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let addr = match socket.local_addr() {
                Ok(addr) => addr,
                Err(err) => return -map_io_error(&err),
            };
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            0
        },
    );
    let socket_getpeername = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         addr_ptr: i32,
         addr_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let addr = match socket.peer_addr() {
                Ok(addr) => addr,
                Err(err) => return -map_io_error(&err),
            };
            let encoded = encode_sockaddr(&addr).unwrap_or_default();
            if encoded.len() > addr_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, addr_ptr, &encoded).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, encoded.len() as u32);
            0
        },
    );
    let socket_setsockopt = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         level: i32,
         optname: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let data = match read_bytes(&mut caller, &memory, val_ptr, val_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::setsockopt(
                            fd,
                            level,
                            optname,
                            data.as_ptr() as *const libc::c_void,
                            data.len() as libc::socklen_t,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::setsockopt(
                            fd as _,
                            level,
                            optname,
                            data.as_ptr() as *const _,
                            data.len() as i32,
                        )
                    }
                }
            };
            if rc == 0 {
                0
            } else {
                -map_io_error(&std::io::Error::last_os_error())
            }
        },
    );
    let socket_getsockopt = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         level: i32,
         optname: i32,
         val_ptr: i32,
         val_len: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let mut buf = vec![0u8; val_len.max(0) as usize];
            let mut len = buf.len() as socklen_t;
            let rc = {
                #[cfg(unix)]
                {
                    let fd = socket.as_raw_fd();
                    unsafe {
                        libc::getsockopt(
                            fd,
                            level,
                            optname,
                            buf.as_mut_ptr() as *mut libc::c_void,
                            &mut len,
                        )
                    }
                }
                #[cfg(windows)]
                {
                    let fd = socket.as_raw_socket() as usize;
                    unsafe {
                        libc::getsockopt(
                            fd as _,
                            level,
                            optname,
                            buf.as_mut_ptr() as *mut _,
                            &mut len,
                        )
                    }
                }
            };
            if rc != 0 {
                return -map_io_error(&std::io::Error::last_os_error());
            }
            if write_bytes(&mut caller, &memory, val_ptr, &buf[..len as usize]).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, len as u32);
            0
        },
    );
    let socket_detach = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i64 {
            let socket = match caller.data_mut().socket_manager.remove(handle as u64) {
                Some(sock) => sock,
                None => return -(libc::EBADF as i64),
            };
            #[cfg(unix)]
            {
                let raw = socket.into_raw_fd();
                raw as i64
            }
            #[cfg(windows)]
            {
                let raw = socket.into_raw_socket();
                raw as i64
            }
        },
    );
    let os_close = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, HostState>, fd: i64| -> i32 {
            if fd < 0 {
                return -libc::EBADF;
            }
            #[cfg(unix)]
            {
                let rc = unsafe { libc::close(fd as libc::c_int) };
                if rc == 0 {
                    return 0;
                }
                -map_io_error(&std::io::Error::last_os_error())
            }
            #[cfg(windows)]
            {
                let sock_rc = unsafe { winsock::closesocket(fd as winsock::SOCKET) };
                if sock_rc == 0 {
                    return 0;
                }
                let sock_err = unsafe { winsock::WSAGetLastError() };
                if sock_err == winsock::WSAENOTSOCK {
                    let rc = unsafe { libc::close(fd as libc::c_int) };
                    if rc == 0 {
                        return 0;
                    }
                    return -map_io_error(&std::io::Error::last_os_error());
                }
                -(sock_err as i32)
            }
        },
    );
    let socketpair = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, HostState>,
         family: i32,
         sock_type: i32,
         proto: i32,
         out_left_ptr: i32,
         out_right_ptr: i32|
         -> i32 {
            #[cfg(not(unix))]
            {
                let _ = (family, sock_type, proto, out_left_ptr, out_right_ptr);
                -libc::ENOSYS
            }
            #[cfg(unix)]
            {
                let mut caller = _caller;
                let memory = match ensure_memory(&mut caller) {
                    Ok(mem) => mem,
                    Err(_) => return -libc::EFAULT,
                };
                let domain = match family {
                    x if x == HOST_AF_UNIX => Domain::UNIX,
                    x if x == HOST_AF_INET => Domain::IPV4,
                    x if x == HOST_AF_INET6 => Domain::IPV6,
                    _ => return -libc::EAFNOSUPPORT,
                };
                let ty = Type::from(sock_type);
                let protocol = if proto == 0 {
                    None
                } else {
                    Some(Protocol::from(proto))
                };
                let (left, right) = match Socket::pair(domain, ty, protocol) {
                    Ok(pair) => pair,
                    Err(err) => return -map_io_error(&err),
                };
                let _ = left.set_nonblocking(true);
                let _ = right.set_nonblocking(true);
                let left_id = caller.data_mut().socket_manager.insert(left);
                let right_id = caller.data_mut().socket_manager.insert(right);
                let _ = write_u64(&mut caller, &memory, out_left_ptr, left_id);
                let _ = write_u64(&mut caller, &memory, out_right_ptr, right_id);
                0
            }
        },
    );
    let socket_getaddrinfo = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         host_ptr: i32,
         host_len: i32,
         serv_ptr: i32,
         serv_len: i32,
         family: i32,
         sock_type: i32,
         proto: i32,
         flags: i32,
         out_ptr: i32,
         out_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let host = if host_len > 0 {
                match read_bytes(&mut caller, &memory, host_ptr, host_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let service = if serv_len > 0 {
                match read_bytes(&mut caller, &memory, serv_ptr, serv_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let host_cstr = host
                .as_ref()
                .map(|val| CString::new(val.as_slice()))
                .transpose()
                .map_err(|_| -libc::EINVAL);
            let serv_cstr = service
                .as_ref()
                .map(|val| CString::new(val.as_slice()))
                .transpose()
                .map_err(|_| -libc::EINVAL);
            let host_cstr = match host_cstr {
                Ok(val) => val,
                Err(err) => return err,
            };
            let serv_cstr = match serv_cstr {
                Ok(val) => val,
                Err(err) => return err,
            };
            let payload = match host_getaddrinfo_payload(
                host_cstr.as_ref(),
                serv_cstr.as_ref(),
                family,
                sock_type,
                proto,
                flags,
            ) {
                Ok(payload) => payload,
                Err(err) => return -err,
            };
            if payload.len() > out_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, payload.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, out_ptr, &payload).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, payload.len() as u32);
            0
        },
    );
    let socket_gethostname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_cap: i32, out_len_ptr: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let bytes = match host_gethostname_bytes() {
                Ok(bytes) => bytes,
                Err(err) => return -err,
            };
            if bytes.len() > buf_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, buf_ptr, &bytes).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
            0
        },
    );
    let socket_getservbyname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         name_ptr: i32,
         name_len: i32,
         proto_ptr: i32,
         proto_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let name = match read_bytes(&mut caller, &memory, name_ptr, name_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let proto = if proto_len > 0 {
                match read_bytes(&mut caller, &memory, proto_ptr, proto_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let name_cstr = match CString::new(name) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            let proto_cstr = match proto {
                Some(buf) => match CString::new(buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                },
                None => None,
            };
            match host_getservbyname_port(&name_cstr, proto_cstr.as_ref()) {
                Ok(port) => port,
                Err(err) => -err,
            }
        },
    );
    let socket_getservbyport = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         port: i32,
         proto_ptr: i32,
         proto_len: i32,
         buf_ptr: i32,
         buf_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let proto = if proto_len > 0 {
                match read_bytes(&mut caller, &memory, proto_ptr, proto_len) {
                    Ok(buf) => Some(buf),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let proto_cstr = match proto {
                Some(buf) => match CString::new(buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                },
                None => None,
            };
            let bytes = match host_getservbyport_name(port, proto_cstr.as_ref()) {
                Ok(bytes) => bytes,
                Err(err) => return -err,
            };
            if bytes.len() > buf_cap as usize {
                let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
                return -libc::ENOMEM;
            }
            if write_bytes(&mut caller, &memory, buf_ptr, &bytes).is_err() {
                return -libc::EFAULT;
            }
            let _ = write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32);
            0
        },
    );
    let socket_poll = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            match poll_socket(socket, events as u32, 0) {
                Ok(mask) => mask as i32,
                Err(errno) => -errno,
            }
        },
    );
    let socket_wait = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32, timeout_ms: i64| -> i32 {
            let socket = match socket_get_mut(caller.data_mut(), handle) {
                Ok(sock) => sock,
                Err(errno) => return -errno,
            };
            let timeout = if timeout_ms < 0 {
                -1
            } else if timeout_ms > i32::MAX as i64 {
                i32::MAX
            } else {
                timeout_ms as i32
            };
            match poll_socket(socket, events as u32, timeout) {
                Ok(mask) => {
                    if mask == 0 {
                        return -libc::ETIMEDOUT;
                    }
                    0
                }
                Err(errno) => -errno,
            }
        },
    );
    let socket_has_ipv6 = Func::wrap(&mut *store, || -> i32 {
        let listener = std::net::TcpListener::bind("[::1]:0");
        if listener.is_ok() { 1 } else { 0 }
    });

    linker.define(&mut *store, "env", "molt_socket_new_host", socket_new)?;
    linker.define(&mut *store, "env", "molt_socket_close_host", socket_close)?;
    linker.define(&mut *store, "env", "molt_socket_clone_host", socket_clone)?;
    linker.define(&mut *store, "env", "molt_socket_bind_host", socket_bind)?;
    linker.define(&mut *store, "env", "molt_socket_listen_host", socket_listen)?;
    linker.define(&mut *store, "env", "molt_socket_accept_host", socket_accept)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_connect_host",
        socket_connect,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_connect_ex_host",
        socket_connect_ex,
    )?;
    linker.define(&mut *store, "env", "molt_socket_recv_host", socket_recv)?;
    linker.define(&mut *store, "env", "molt_socket_send_host", socket_send)?;
    linker.define(&mut *store, "env", "molt_socket_sendto_host", socket_sendto)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_sendmsg_host",
        socket_sendmsg,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_recvfrom_host",
        socket_recvfrom,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_recvmsg_host",
        socket_recvmsg,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_shutdown_host",
        socket_shutdown,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getsockname_host",
        socket_getsockname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getpeername_host",
        socket_getpeername,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_setsockopt_host",
        socket_setsockopt,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getsockopt_host",
        socket_getsockopt,
    )?;
    linker.define(&mut *store, "env", "molt_socket_detach_host", socket_detach)?;
    linker.define(&mut *store, "env", "molt_os_close_host", os_close)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_socketpair_host",
        socketpair,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getaddrinfo_host",
        socket_getaddrinfo,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_gethostname_host",
        socket_gethostname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getservbyname_host",
        socket_getservbyname,
    )?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_getservbyport_host",
        socket_getservbyport,
    )?;
    linker.define(&mut *store, "env", "molt_socket_poll_host", socket_poll)?;
    linker.define(&mut *store, "env", "molt_socket_wait_host", socket_wait)?;
    linker.define(
        &mut *store,
        "env",
        "molt_socket_has_ipv6_host",
        socket_has_ipv6,
    )?;
    Ok(())
}
