use super::target_capability::{base_socket_type, socket_type_requests_nonblocking};
use super::*;

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_new(
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    fileno_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = match to_i64(obj_from_bits(family_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
        };
        let sock_type = match to_i64(obj_from_bits(type_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
        };
        let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
        let fileno = if obj_from_bits(fileno_bits).is_none() {
            None
        } else {
            match to_i64(obj_from_bits(fileno_bits)) {
                Some(val) => Some(val),
                None => {
                    return raise_exception::<_>(_py, "TypeError", "fileno must be int or None");
                }
            }
        };
        let domain = match family {
            val if val == libc::AF_INET => Domain::IPV4,
            val if val == libc::AF_INET6 => Domain::IPV6,
            #[cfg(unix)]
            val if val == libc::AF_UNIX => Domain::UNIX,
            _ => {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EAFNOSUPPORT as i64,
                    "address family not supported",
                );
            }
        };
        let base_type = base_socket_type(sock_type);
        let socket_type = match base_type {
            val if val == libc::SOCK_STREAM => Type::STREAM,
            val if val == libc::SOCK_DGRAM => Type::DGRAM,
            val if val == libc::SOCK_RAW => Type::from(val),
            _ => {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EPROTOTYPE as i64,
                    "unsupported socket type",
                );
            }
        };
        let socket = match fileno {
            Some(raw) => unsafe {
                if raw < 0 {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EBADF as i64,
                        "bad file descriptor",
                    );
                }
                #[cfg(all(unix, molt_has_net_io))]
                {
                    Socket::from_raw_fd(raw as RawFd)
                }
                #[cfg(all(windows, molt_has_net_io))]
                {
                    Socket::from_raw_socket(raw as RawSocket)
                }
            },
            None => match Socket::new(domain, socket_type, Some(Protocol::from(proto))) {
                Ok(sock) => sock,
                Err(err) => return raise_os_error::<u64>(_py, err, "socket"),
            },
        };
        if let Err(err) = socket.set_nonblocking(true) {
            return raise_os_error::<u64>(_py, err, "socket");
        }
        let timeout = socket_type_requests_nonblocking(sock_type).then_some(Duration::ZERO);
        let connect_pending = false;
        let kind = if base_type == libc::SOCK_DGRAM {
            #[cfg(unix)]
            if family == libc::AF_UNIX {
                let raw_fd = socket.into_raw_fd();
                let std_sock = unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(raw_fd) };
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UnixDatagram(mio::net::UnixDatagram::from_std(std_sock))
            } else {
                let std_sock: std::net::UdpSocket = socket.into();
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
            }
            #[cfg(all(not(unix), molt_has_net_io))]
            {
                let std_sock: std::net::UdpSocket = socket.into();
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
            }
        } else if let Some(_raw) = fileno {
            let acceptor = socket_is_acceptor(&socket);
            if acceptor {
                #[cfg(unix)]
                if family == libc::AF_UNIX {
                    let raw_fd = socket.into_raw_fd();
                    let std_listener =
                        unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::UnixListener(mio::net::UnixListener::from_std(std_listener))
                } else {
                    let std_listener: std::net::TcpListener = socket.into();
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
                }
                #[cfg(not(unix))]
                {
                    let std_listener: std::net::TcpListener = socket.into();
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
                }
            } else {
                #[cfg(unix)]
                if family == libc::AF_UNIX {
                    let raw_fd = socket.into_raw_fd();
                    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::UnixStream(mio::net::UnixStream::from_std(std_stream))
                } else {
                    let std_stream: std::net::TcpStream = socket.into();
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
                }
                #[cfg(not(unix))]
                {
                    let std_stream: std::net::TcpStream = socket.into();
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
                }
            }
        } else {
            MoltSocketKind::Pending(socket)
        };
        let socket_ptr = socket_alloc(kind, family, base_type, proto, timeout, connect_pending);
        opaque_handle_bits(socket_ptr)
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_close(sock_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::none().bits();
            }
            socket_close_ptr(_py, socket_ptr);
            MoltObject::none().bits()
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_drop(sock_bits: u64) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return;
            }
            socket_close_ptr(_py, socket_ptr);
            socket_ref_dec(_py, socket_ptr);
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_clone(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        socket_ref_inc(socket_ptr);
        sock_bits
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_fileno(sock_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(-1).bits();
            }
            let socket = &*(socket_ptr as *mut MoltSocket);
            if socket.closed.load(AtomicOrdering::Relaxed) {
                return MoltObject::from_int(-1).bits();
            }
            let guard = socket.inner.lock().unwrap();
            #[cfg(all(unix, molt_has_net_io))]
            let fd = match &guard.kind {
                MoltSocketKind::Pending(sock) => sock.as_raw_fd() as i64,
                MoltSocketKind::TcpStream(sock) => sock.as_raw_fd() as i64,
                MoltSocketKind::TcpListener(sock) => sock.as_raw_fd() as i64,
                MoltSocketKind::UdpSocket(sock) => sock.as_raw_fd() as i64,
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixStream(sock) => sock.as_raw_fd() as i64,
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixListener(sock) => sock.as_raw_fd() as i64,
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixDatagram(sock) => sock.as_raw_fd() as i64,
                MoltSocketKind::Closed => -1,
            };
            #[cfg(all(windows, molt_has_net_io))]
            let fd = match &guard.kind {
                MoltSocketKind::Pending(sock) => sock.as_raw_socket() as i64,
                MoltSocketKind::TcpStream(sock) => sock.as_raw_socket() as i64,
                MoltSocketKind::TcpListener(sock) => sock.as_raw_socket() as i64,
                MoltSocketKind::UdpSocket(sock) => sock.as_raw_socket() as i64,
                MoltSocketKind::Closed => -1,
            };
            MoltObject::from_int(fd).bits()
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_gettimeout(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let timeout = socket_timeout(socket_ptr);
        match timeout {
            None => MoltObject::none().bits(),
            Some(val) => MoltObject::from_float(val.as_secs_f64()).bits(),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_settimeout(sock_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(timeout_bits);
        if obj.is_none() {
            socket_set_timeout(socket_ptr, None);
            return MoltObject::none().bits();
        }
        let Some(timeout) = to_f64(obj) else {
            return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
        };
        if !timeout.is_finite() || timeout < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "timeout must be non-negative");
        }
        let duration = if timeout == 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(timeout)
        };
        socket_set_timeout(socket_ptr, Some(duration));
        MoltObject::none().bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_setblocking(sock_bits: u64, flag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let flag = obj_from_bits(flag_bits).as_bool().unwrap_or(false);
        if flag {
            socket_set_timeout(socket_ptr, None);
        } else {
            socket_set_timeout(socket_ptr, Some(Duration::ZERO));
        }
        MoltObject::none().bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getblocking(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_bool(false).bits();
        }
        let timeout = socket_timeout(socket_ptr);
        let blocking = match timeout {
            None => true,
            Some(val) if val == Duration::ZERO => false,
            Some(_) => true,
        };
        MoltObject::from_bool(blocking).bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_bind(sock_bits: u64, addr_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.bind", "net"]).is_err() {
                return MoltObject::none().bits();
            }
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let family = {
                let socket = &*(socket_ptr as *mut MoltSocket);
                let guard = socket.inner.lock().unwrap();
                guard.family
            };
            let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
                Ok(addr) => addr,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let res = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                MoltSocketKind::Pending(sock) => sock.bind(&sockaddr),
                MoltSocketKind::TcpListener(_) | MoltSocketKind::TcpStream(_) => Err(
                    std::io::Error::new(ErrorKind::InvalidInput, "socket already bound"),
                ),
                MoltSocketKind::UdpSocket(sock) => {
                    #[cfg(unix)]
                    let raw = sock.as_raw_fd();
                    #[cfg(windows)]
                    let raw = sock.as_raw_socket();
                    with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
                }
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => Err(
                    std::io::Error::new(ErrorKind::InvalidInput, "socket already bound"),
                ),
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixDatagram(sock) => {
                    #[cfg(unix)]
                    let raw = sock.as_raw_fd();
                    #[cfg(all(windows, molt_has_net_io))]
                    let raw = sock.as_raw_socket();
                    with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
                }
                MoltSocketKind::Closed => Err(std::io::Error::new(
                    ErrorKind::NotConnected,
                    "socket closed",
                )),
            });
            match res {
                Ok(_) => MoltObject::none().bits(),
                Err(err) => raise_os_error::<u64>(_py, err, "bind"),
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_listen(sock_bits: u64, backlog_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let backlog = to_i64(obj_from_bits(backlog_bits)).unwrap_or(128).max(0) as i32;
        let res = with_socket_mut(socket_ptr, |inner| {
            match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                MoltSocketKind::Pending(sock) => match sock.listen(backlog) {
                    Ok(_) => {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_listener =
                                unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixListener(
                                mio::net::UnixListener::from_std(std_listener),
                            );
                        } else {
                            let std_listener: std::net::TcpListener = sock.into();
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpListener(
                                mio::net::TcpListener::from_std(std_listener),
                            );
                        }
                        #[cfg(all(not(unix), molt_has_net_io))]
                        {
                            let std_listener: std::net::TcpListener = sock.into();
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpListener(
                                mio::net::TcpListener::from_std(std_listener),
                            );
                        }
                        Ok(())
                    }
                    Err(err) => {
                        inner.kind = MoltSocketKind::Pending(sock);
                        Err(err)
                    }
                },
                MoltSocketKind::TcpListener(listener) => {
                    let res = {
                        #[cfg(unix)]
                        {
                            socket_relisten(listener.as_raw_fd(), backlog)
                        }
                        #[cfg(windows)]
                        {
                            socket_relisten(listener.as_raw_socket(), backlog)
                        }
                    };
                    inner.kind = MoltSocketKind::TcpListener(listener);
                    res
                }
                #[cfg(all(unix, molt_has_net_io))]
                MoltSocketKind::UnixListener(listener) => {
                    let res = socket_relisten(listener.as_raw_fd(), backlog);
                    inner.kind = MoltSocketKind::UnixListener(listener);
                    res
                }
                other => {
                    inner.kind = other;
                    Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "socket not in listenable state",
                    ))
                }
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "listen"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_accept(sock_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
                return MoltObject::none().bits();
            }
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::none().bits();
            }
            loop {
                let timeout = socket_timeout(socket_ptr);
                let (accepted_kind, addr_bits, family) = {
                    let socket = &*(socket_ptr as *mut MoltSocket);
                    let mut guard = socket.inner.lock().unwrap();
                    match &mut guard.kind {
                        MoltSocketKind::TcpListener(listener) => match listener.accept() {
                            Ok((stream, addr)) => (
                                MoltSocketKind::TcpStream(stream),
                                sockaddr_to_bits(_py, &SockAddr::from(addr)),
                                guard.family,
                            ),
                            Err(err) => {
                                if err.kind() == ErrorKind::WouldBlock {
                                    (MoltSocketKind::Closed, 0, guard.family)
                                } else {
                                    return raise_os_error::<u64>(_py, err, "accept");
                                }
                            }
                        },
                        #[cfg(all(unix, molt_has_net_io))]
                        MoltSocketKind::UnixListener(listener) => match listener.accept() {
                            Ok((stream, addr)) => {
                                let addr_bits = if let Some(path) = addr.as_pathname() {
                                    let text = path.to_string_lossy();
                                    let ptr = alloc_string(_py, text.as_bytes());
                                    if ptr.is_null() {
                                        MoltObject::none().bits()
                                    } else {
                                        MoltObject::from_ptr(ptr).bits()
                                    }
                                } else {
                                    MoltObject::none().bits()
                                };
                                (MoltSocketKind::UnixStream(stream), addr_bits, guard.family)
                            }
                            Err(err) => {
                                if err.kind() == ErrorKind::WouldBlock {
                                    (MoltSocketKind::Closed, 0, guard.family)
                                } else {
                                    return raise_os_error::<u64>(_py, err, "accept");
                                }
                            }
                        },
                        _ => {
                            return raise_os_error_errno::<u64>(
                                _py,
                                libc::EINVAL as i64,
                                "socket not listening",
                            );
                        }
                    }
                };
                if addr_bits == 0 {
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            if matches!(timeout, Some(val) if val == Duration::ZERO) {
                                return raise_os_error_errno::<u64>(
                                    _py,
                                    libc::EWOULDBLOCK as i64,
                                    "accept: would block",
                                );
                            }
                            continue;
                        }
                        return raise_os_error::<u64>(_py, wait_err, "accept");
                    }
                    continue;
                }
                let socket_ptr =
                    socket_alloc(accepted_kind, family, libc::SOCK_STREAM, 0, timeout, false);
                let handle_bits = opaque_handle_bits(socket_ptr);
                let tuple_ptr = alloc_tuple(_py, &[handle_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_connect(sock_bits: u64, addr_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
                return MoltObject::none().bits();
            }
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let family = {
                let socket = &*(socket_ptr as *mut MoltSocket);
                let guard = socket.inner.lock().unwrap();
                guard.family
            };
            let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
                Ok(addr) => addr,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let timeout = socket_timeout(socket_ptr);
            let res = with_socket_mut(socket_ptr, |inner| {
                if inner.connect_pending {
                    return Ok(true);
                }
                match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                    MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                        Ok(_) => {
                            #[cfg(unix)]
                            if inner.family == libc::AF_UNIX {
                                let raw_fd = sock.into_raw_fd();
                                let std_stream =
                                    std::os::unix::net::UnixStream::from_raw_fd(raw_fd);
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::UnixStream(
                                    mio::net::UnixStream::from_std(std_stream),
                                );
                            } else {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            #[cfg(all(not(unix), molt_has_net_io))]
                            {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            inner.connect_pending = false;
                            Ok(false)
                        }
                        Err(err) => {
                            if err.kind() == ErrorKind::WouldBlock
                                || err.raw_os_error() == Some(libc::EINPROGRESS)
                            {
                                #[cfg(unix)]
                                if inner.family == libc::AF_UNIX {
                                    let raw_fd = sock.into_raw_fd();
                                    let std_stream =
                                        std::os::unix::net::UnixStream::from_raw_fd(raw_fd);
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::UnixStream(
                                        mio::net::UnixStream::from_std(std_stream),
                                    );
                                } else {
                                    let std_stream: std::net::TcpStream = sock.into();
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::TcpStream(
                                        mio::net::TcpStream::from_std(std_stream),
                                    );
                                }
                                #[cfg(all(not(unix), molt_has_net_io))]
                                {
                                    let std_stream: std::net::TcpStream = sock.into();
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::TcpStream(
                                        mio::net::TcpStream::from_std(std_stream),
                                    );
                                }
                                inner.connect_pending = true;
                            } else {
                                inner.kind = MoltSocketKind::Pending(sock);
                            }
                            Err(err)
                        }
                    },
                    MoltSocketKind::UdpSocket(sock) => {
                        #[cfg(unix)]
                        let fd = sock.as_raw_fd();
                        #[cfg(all(windows, molt_has_net_io))]
                        let fd = sock.as_raw_socket();
                        let res = connect_raw_socket(fd, &sockaddr);
                        inner.kind = MoltSocketKind::UdpSocket(sock);
                        match res {
                            Ok(()) => {
                                inner.connect_pending = false;
                                Ok(false)
                            }
                            Err(err) => Err(err),
                        }
                    }
                    #[cfg(all(unix, molt_has_net_io))]
                    MoltSocketKind::UnixDatagram(sock) => {
                        let fd = sock.as_raw_fd();
                        let res = connect_raw_socket(fd, &sockaddr);
                        inner.kind = MoltSocketKind::UnixDatagram(sock);
                        match res {
                            Ok(()) => {
                                inner.connect_pending = false;
                                Ok(false)
                            }
                            Err(err) => Err(err),
                        }
                    }
                    other => {
                        inner.kind = other;
                        Err(std::io::Error::new(
                            ErrorKind::InvalidInput,
                            "socket already connected",
                        ))
                    }
                }
            });
            match res {
                Ok(pending) => {
                    if pending {
                        if let Err(err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                            if err.kind() == ErrorKind::TimedOut {
                                return raise_os_error_errno::<u64>(
                                    _py,
                                    libc::ETIMEDOUT as i64,
                                    "timed out",
                                );
                            }
                            if err.kind() == ErrorKind::WouldBlock {
                                return raise_os_error_errno::<u64>(
                                    _py,
                                    libc::EINPROGRESS as i64,
                                    "operation in progress",
                                );
                            }
                            return raise_os_error::<u64>(_py, err, "connect");
                        }
                        let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                            MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                            #[cfg(all(unix, molt_has_net_io))]
                            MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                            _ => Ok(None),
                        });
                        match err {
                            Ok(None) => MoltObject::none().bits(),
                            Ok(Some(err)) => raise_os_error::<u64>(_py, err, "connect"),
                            Err(err) => raise_os_error::<u64>(_py, err, "connect"),
                        }
                    } else {
                        MoltObject::none().bits()
                    }
                }
                Err(err)
                    if err.kind() == ErrorKind::WouldBlock
                        || err.raw_os_error() == Some(libc::EINPROGRESS) =>
                {
                    match timeout {
                        Some(val) if val == Duration::ZERO => raise_os_error_errno::<u64>(
                            _py,
                            libc::EINPROGRESS as i64,
                            "operation in progress",
                        ),
                        _ => {
                            if let Err(wait_err) =
                                socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE)
                            {
                                if wait_err.kind() == ErrorKind::TimedOut {
                                    return raise_os_error_errno::<u64>(
                                        _py,
                                        libc::ETIMEDOUT as i64,
                                        "timed out",
                                    );
                                }
                                return raise_os_error::<u64>(_py, wait_err, "connect");
                            }
                            let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                                MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                                #[cfg(all(unix, molt_has_net_io))]
                                MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                                _ => Ok(None),
                            });
                            match err {
                                Ok(None) => MoltObject::none().bits(),
                                Ok(Some(err)) => raise_os_error::<u64>(_py, err, "connect"),
                                Err(err) => raise_os_error::<u64>(_py, err, "connect"),
                            }
                        }
                    }
                }
                Err(err) => raise_os_error::<u64>(_py, err, "connect"),
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_connect_ex(sock_bits: u64, addr_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
                return MoltObject::none().bits();
            }
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(libc::EBADF as i64).bits();
            }
            let family = {
                let socket = &*(socket_ptr as *mut MoltSocket);
                let guard = socket.inner.lock().unwrap();
                guard.family
            };
            let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
                Ok(addr) => addr,
                Err(_msg) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
            };
            let timeout = socket_timeout(socket_ptr);
            enum ConnectExOutcome {
                Done(i64),
                Pending(i64),
            }
            let res = with_socket_mut(socket_ptr, |inner| {
                if inner.connect_pending {
                    let err = match &inner.kind {
                        MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                        #[cfg(all(unix, molt_has_net_io))]
                        MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                        _ => Ok(None),
                    };
                    return match err {
                        Ok(None) => {
                            inner.connect_pending = false;
                            Ok(ConnectExOutcome::Done(0))
                        }
                        Ok(Some(err)) => {
                            inner.connect_pending = false;
                            Ok(ConnectExOutcome::Done(
                                err.raw_os_error().unwrap_or(libc::EIO) as i64,
                            ))
                        }
                        Err(err) => {
                            let errno = err.raw_os_error().unwrap_or(libc::EIO) as i64;
                            if err.kind() == ErrorKind::WouldBlock
                                || errno == libc::EINPROGRESS as i64
                                || errno == libc::EALREADY as i64
                                || errno == libc::EAGAIN as i64
                                || errno == libc::EWOULDBLOCK as i64
                            {
                                Ok(ConnectExOutcome::Pending(libc::EINPROGRESS as i64))
                            } else {
                                inner.connect_pending = false;
                                Ok(ConnectExOutcome::Done(errno))
                            }
                        }
                    };
                }
                match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                    MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                        Ok(_) => {
                            #[cfg(unix)]
                            if inner.family == libc::AF_UNIX {
                                let raw_fd = sock.into_raw_fd();
                                let std_stream =
                                    std::os::unix::net::UnixStream::from_raw_fd(raw_fd);
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::UnixStream(
                                    mio::net::UnixStream::from_std(std_stream),
                                );
                            } else {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            #[cfg(all(not(unix), molt_has_net_io))]
                            {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            inner.connect_pending = false;
                            Ok(ConnectExOutcome::Done(0))
                        }
                        Err(err) => {
                            let errno = err.raw_os_error().unwrap_or(libc::EIO);
                            if err.kind() == ErrorKind::WouldBlock
                                || errno == libc::EINPROGRESS
                                || errno == libc::EALREADY
                            {
                                #[cfg(unix)]
                                if inner.family == libc::AF_UNIX {
                                    let raw_fd = sock.into_raw_fd();
                                    let std_stream =
                                        std::os::unix::net::UnixStream::from_raw_fd(raw_fd);
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::UnixStream(
                                        mio::net::UnixStream::from_std(std_stream),
                                    );
                                } else {
                                    let std_stream: std::net::TcpStream = sock.into();
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::TcpStream(
                                        mio::net::TcpStream::from_std(std_stream),
                                    );
                                }
                                #[cfg(all(not(unix), molt_has_net_io))]
                                {
                                    let std_stream: std::net::TcpStream = sock.into();
                                    std_stream.set_nonblocking(true)?;
                                    inner.kind = MoltSocketKind::TcpStream(
                                        mio::net::TcpStream::from_std(std_stream),
                                    );
                                }
                                inner.connect_pending = true;
                                Ok(ConnectExOutcome::Pending(errno as i64))
                            } else {
                                inner.kind = MoltSocketKind::Pending(sock);
                                Ok(ConnectExOutcome::Done(errno as i64))
                            }
                        }
                    },
                    MoltSocketKind::UdpSocket(sock) => {
                        #[cfg(unix)]
                        let fd = sock.as_raw_fd();
                        #[cfg(all(windows, molt_has_net_io))]
                        let fd = sock.as_raw_socket();
                        let res = connect_raw_socket(fd, &sockaddr);
                        inner.kind = MoltSocketKind::UdpSocket(sock);
                        match res {
                            Ok(()) => Ok(ConnectExOutcome::Done(0)),
                            Err(err) => Ok(ConnectExOutcome::Done(
                                err.raw_os_error().unwrap_or(libc::EIO) as i64,
                            )),
                        }
                    }
                    #[cfg(all(unix, molt_has_net_io))]
                    MoltSocketKind::UnixDatagram(sock) => {
                        let fd = sock.as_raw_fd();
                        let res = connect_raw_socket(fd, &sockaddr);
                        inner.kind = MoltSocketKind::UnixDatagram(sock);
                        match res {
                            Ok(()) => Ok(ConnectExOutcome::Done(0)),
                            Err(err) => Ok(ConnectExOutcome::Done(
                                err.raw_os_error().unwrap_or(libc::EIO) as i64,
                            )),
                        }
                    }
                    other => {
                        inner.kind = other;
                        Ok(ConnectExOutcome::Done(libc::EISCONN as i64))
                    }
                }
            });
            match res {
                Ok(ConnectExOutcome::Done(val)) => MoltObject::from_int(val).bits(),
                Ok(ConnectExOutcome::Pending(val)) => {
                    if matches!(timeout, Some(val) if val == Duration::ZERO) {
                        return MoltObject::from_int(val).bits();
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return MoltObject::from_int(libc::ETIMEDOUT as i64).bits();
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            return MoltObject::from_int(libc::EINPROGRESS as i64).bits();
                        }
                        return MoltObject::from_int(
                            wait_err.raw_os_error().unwrap_or(libc::EIO) as i64
                        )
                        .bits();
                    }
                    let err = with_socket_mut(socket_ptr, |inner| {
                        let err = match &inner.kind {
                            MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                            #[cfg(all(unix, molt_has_net_io))]
                            MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                            _ => Ok(None),
                        };
                        inner.connect_pending = false;
                        err
                    });
                    match err {
                        Ok(None) => MoltObject::from_int(0).bits(),
                        Ok(Some(err)) => {
                            MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64)
                                .bits()
                        }
                        Err(err) => {
                            MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64)
                                .bits()
                        }
                    }
                }
                Err(err) => {
                    MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64).bits()
                }
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_shutdown(sock_bits: u64, how_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let how = to_i64(obj_from_bits(how_bits)).unwrap_or(2) as i32;
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe { libc::shutdown(libc_socket(fd), how) };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "shutdown"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getsockname(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let ret = unsafe {
                libc::getsockname(
                    libc_socket(fd),
                    &mut storage as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if ret == 0 {
                Ok(sock_addr_from_storage(storage, len))
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(addr) => sockaddr_to_bits(_py, &addr),
            Err(err) => raise_os_error::<u64>(_py, err, "getsockname"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getpeername(sock_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let ret = unsafe {
                libc::getpeername(
                    libc_socket(fd),
                    &mut storage as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if ret == 0 {
                Ok(sock_addr_from_storage(storage, len))
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(addr) => sockaddr_to_bits(_py, &addr),
            Err(err) => raise_os_error::<u64>(_py, err, "getpeername"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_setsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(value_bits);
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            if let Some(val) = to_i64(obj) {
                let val = val as c_int;
                let ret = unsafe {
                    libc::setsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        &val as *const _ as *const c_void,
                        std::mem::size_of::<c_int>() as libc::socklen_t,
                    )
                };
                if ret == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else if let Some(ptr) = obj.as_ptr() {
                let bytes = unsafe { bytes_like_slice_raw(ptr) }.ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "invalid optval")
                })?;
                let ret = unsafe {
                    libc::setsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        bytes.as_ptr() as *const c_void,
                        bytes.len() as libc::socklen_t,
                    )
                };
                if ret == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else {
                Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "invalid optval",
                ))
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "setsockopt"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_getsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    buflen_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
        let buflen = if obj_from_bits(buflen_bits).is_none() {
            None
        } else {
            Some(to_i64(obj_from_bits(buflen_bits)).unwrap_or(0).max(0) as usize)
        };
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            if let Some(buflen) = buflen {
                let mut buf = vec![0u8; buflen];
                let mut len = buflen as libc::socklen_t;
                let ret = unsafe {
                    libc::getsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        buf.as_mut_ptr() as *mut c_void,
                        &mut len,
                    )
                };
                if ret == 0 {
                    let ptr = alloc_bytes(_py, &buf[..len as usize]);
                    if ptr.is_null() {
                        Err(std::io::Error::other("allocation failed"))
                    } else {
                        Ok(MoltObject::from_ptr(ptr).bits())
                    }
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else {
                let mut val: c_int = 0;
                let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
                let ret = unsafe {
                    libc::getsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        &mut val as *mut _ as *mut c_void,
                        &mut len,
                    )
                };
                if ret == 0 {
                    Ok(MoltObject::from_int(val as i64).bits())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            }
        });
        match res {
            Ok(bits) => bits,
            Err(err) => raise_os_error::<u64>(_py, err, "getsockopt"),
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_detach(sock_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(-1).bits();
            }
            let raw = socket_detach_raw(_py, socket_ptr);
            MoltObject::from_int(raw).bits()
        })
    }
}
