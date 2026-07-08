use super::*;

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recv(sock_bits: u64, size_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::recv(
                        libc_socket(fd),
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len(),
                        flags,
                    )
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(n) => {
                    if trace_socket_recv() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!("molt socket recv: fd={} len={}", fd, n);
                    }
                    let ptr = alloc_bytes(_py, &buf[..n]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                Err(err) => {
                    let raw = err.raw_os_error();
                    let would_block_raw = matches!(
                        raw,
                        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
                    );
                    let would_block = err.kind() == ErrorKind::WouldBlock || would_block_raw;
                    if trace_socket_recv() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!(
                            "molt socket recv error: fd={} kind={:?} raw={raw:?} dontwait={dontwait} nonblocking={} msg={}",
                            fd,
                            err.kind(),
                            nonblocking,
                            err
                        );
                    }
                    if would_block {
                        if dontwait || nonblocking {
                            let errno = raw.unwrap_or(libc::EWOULDBLOCK) as i64;
                            return raise_os_error_errno::<u64>(_py, errno, "recv: would block");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "recv");
                        }
                        continue;
                    }
                    return raise_os_error::<u64>(_py, err, "recv");
                }
            }
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recv_into(
    sock_bits: u64,
    buffer_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(0).bits();
            }
            let buffer_obj = obj_from_bits(buffer_bits);
            let buffer_ptr = buffer_obj.as_ptr();
            if buffer_ptr.is_none() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recv_into requires a writable buffer",
                );
            }
            let buffer_ptr = buffer_ptr.unwrap();
            let size = to_i64(obj_from_bits(size_bits)).unwrap_or(-1);
            let target_len;
            let mut use_memoryview = false;
            let type_id = object_type_id(buffer_ptr);
            if type_id == TYPE_ID_BYTEARRAY {
                target_len = bytearray_len(buffer_ptr);
            } else if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_released(buffer_ptr) {
                    return raise_released_memoryview(_py);
                }
                if memoryview_readonly(buffer_ptr) {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "recv_into requires a writable buffer",
                    );
                }
                target_len = memoryview_len(buffer_ptr);
                use_memoryview = true;
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recv_into requires a writable buffer",
                );
            }
            let size = if size < 0 {
                target_len
            } else {
                (size as usize).min(target_len)
            };
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
            #[cfg(unix)]
            let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
            #[cfg(not(unix))]
            let dontwait = false;
            loop {
                let res = with_socket_mut(socket_ptr, |inner| {
                    #[cfg(unix)]
                    let fd = inner.raw_fd().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    #[cfg(windows)]
                    let fd = inner.raw_socket().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    if use_memoryview {
                        if let Some(slice) = memoryview_bytes_slice_mut(buffer_ptr) {
                            let len = size.min(slice.len());
                            let ret = libc::recv(
                                libc_socket(fd),
                                slice.as_mut_ptr() as *mut c_void,
                                len,
                                flags,
                            );
                            if ret >= 0 {
                                Ok((ret as usize, None))
                            } else {
                                Err(std::io::Error::last_os_error())
                            }
                        } else {
                            let mut tmp = vec![0u8; size];
                            let ret = libc::recv(
                                libc_socket(fd),
                                tmp.as_mut_ptr() as *mut c_void,
                                tmp.len(),
                                flags,
                            );
                            if ret >= 0 {
                                Ok((ret as usize, Some(tmp)))
                            } else {
                                Err(std::io::Error::last_os_error())
                            }
                        }
                    } else {
                        let buf = bytearray_vec(buffer_ptr);
                        let ret = libc::recv(
                            libc_socket(fd),
                            buf.as_mut_ptr() as *mut c_void,
                            size,
                            flags,
                        );
                        if ret >= 0 {
                            Ok((ret as usize, None))
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    }
                });
                match res {
                    Ok((n, tmp)) => {
                        if use_memoryview
                            && let Some(tmp) = tmp.as_ref()
                            && let Err(msg) = memoryview_write_bytes(buffer_ptr, &tmp[..n])
                        {
                            return raise_exception::<u64>(_py, "TypeError", &msg);
                        }
                        return MoltObject::from_int(n as i64).bits();
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        if dontwait {
                            return raise_os_error::<u64>(_py, err, "recv_into");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "recv_into");
                        }
                        continue;
                    }
                    Err(err) => return raise_os_error::<u64>(_py, err, "recv_into"),
                }
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_send(sock_bits: u64, data_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        if data_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::send(libc_socket(fd), data_ptr as *const c_void, data_len, flags)
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(n) => {
                    if trace_socket_send() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!("molt socket send: fd={} len={} sent={}", fd, data_len, n);
                    }
                    return MoltObject::from_int(n as i64).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if trace_socket_send() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!(
                            "molt socket send would_block: fd={} nonblocking={} dontwait={}",
                            fd, nonblocking, dontwait
                        );
                    }
                    if dontwait || nonblocking {
                        return raise_os_error::<u64>(_py, err, "send");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            continue;
                        }
                        return raise_os_error::<u64>(_py, wait_err, "send");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "send"),
            }
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendall(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        let mut offset = 0usize;
        while offset < data_len {
            let slice_ptr = unsafe { data_ptr.add(offset) };
            let slice_len = data_len - offset;
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::send(
                        libc_socket(fd),
                        slice_ptr as *const c_void,
                        slice_len,
                        flags,
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
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait || nonblocking {
                        return raise_os_error::<u64>(_py, err, "sendall");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            continue;
                        }
                        return raise_os_error::<u64>(_py, wait_err, "sendall");
                    }
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "sendall"),
            }
        }
        MoltObject::none().bits()
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendto(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
    addr_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(0).bits();
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
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
            #[cfg(unix)]
            let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
            #[cfg(not(unix))]
            let dontwait = false;
            let nonblocking =
                matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
            let send_data = match send_data_from_bits(data_bits) {
                Ok(data) => data,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
                SendData::Borrowed(ptr, len) => (ptr, len, None),
                SendData::Owned(vec) => {
                    let ptr = vec.as_ptr();
                    let len = vec.len();
                    (ptr, len, Some(vec))
                }
            };
            let _owned_guard = owned;
            loop {
                let res = with_socket_mut(socket_ptr, |inner| {
                    #[cfg(unix)]
                    let fd = inner.raw_fd().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    #[cfg(windows)]
                    let fd = inner.raw_socket().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    let ret = libc::sendto(
                        libc_socket(fd),
                        data_ptr as *const c_void,
                        data_len,
                        flags,
                        sockaddr.as_ptr() as *const libc::sockaddr,
                        sockaddr.len(),
                    );
                    if ret >= 0 {
                        Ok(ret as usize)
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
                match res {
                    Ok(n) => return MoltObject::from_int(n as i64).bits(),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        if dontwait || nonblocking {
                            return raise_os_error::<u64>(_py, err, "sendto");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "sendto");
                        }
                        continue;
                    }
                    Err(err) => return raise_os_error::<u64>(_py, err, "sendto"),
                }
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recvfrom(
    sock_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let mut buf = vec![0u8; size];
        loop {
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
                    libc::recvfrom(
                        libc_socket(fd),
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len(),
                        flags,
                        &mut storage as *mut _ as *mut libc::sockaddr,
                        &mut len,
                    )
                };
                if ret >= 0 {
                    let addr = sock_addr_from_storage(storage, len);
                    Ok((ret as usize, addr))
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok((n, addr)) => {
                    let data_ptr = alloc_bytes(_py, &buf[..n]);
                    if data_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let data_bits = MoltObject::from_ptr(data_ptr).bits();
                    let addr_bits = sockaddr_to_bits(_py, &addr);
                    let tuple_ptr = alloc_tuple(_py, &[data_bits, addr_bits]);
                    dec_ref_bits(_py, data_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait {
                        return raise_os_error::<u64>(_py, err, "recvfrom");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            continue;
                        }
                        return raise_os_error::<u64>(_py, wait_err, "recvfrom");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "recvfrom"),
            }
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recvfrom_into(
    sock_bits: u64,
    buffer_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let buffer_obj = obj_from_bits(buffer_bits);
        let buffer_ptr = match buffer_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recvfrom_into requires a writable buffer",
                );
            }
        };
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(-1);
        let target_len;
        let mut use_memoryview = false;
        let type_id = unsafe { object_type_id(buffer_ptr) };
        if type_id == TYPE_ID_BYTEARRAY {
            target_len = unsafe { bytearray_len(buffer_ptr) };
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if unsafe { memoryview_released(buffer_ptr) } {
                return raise_released_memoryview(_py);
            }
            if unsafe { memoryview_readonly(buffer_ptr) } {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recvfrom_into requires a writable buffer",
                );
            }
            target_len = unsafe { memoryview_len(buffer_ptr) };
            use_memoryview = true;
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "recvfrom_into requires a writable buffer",
            );
        }
        let size = if size < 0 {
            target_len
        } else {
            (size as usize).min(target_len)
        };
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        loop {
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
                if use_memoryview {
                    if let Some(slice) = unsafe { memoryview_bytes_slice_mut(buffer_ptr) } {
                        let recv_len = size.min(slice.len());
                        let ret = unsafe {
                            libc::recvfrom(
                                libc_socket(fd),
                                slice.as_mut_ptr() as *mut c_void,
                                recv_len,
                                flags,
                                &mut storage as *mut _ as *mut libc::sockaddr,
                                &mut len,
                            )
                        };
                        if ret >= 0 {
                            Ok((ret as usize, sock_addr_from_storage(storage, len), None))
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    } else {
                        let mut tmp = vec![0u8; size];
                        let ret = unsafe {
                            libc::recvfrom(
                                libc_socket(fd),
                                tmp.as_mut_ptr() as *mut c_void,
                                tmp.len(),
                                flags,
                                &mut storage as *mut _ as *mut libc::sockaddr,
                                &mut len,
                            )
                        };
                        if ret >= 0 {
                            Ok((
                                ret as usize,
                                sock_addr_from_storage(storage, len),
                                Some(tmp),
                            ))
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    }
                } else {
                    let buf = unsafe { bytearray_vec(buffer_ptr) };
                    let recv_len = size.min(buf.len());
                    let ret = unsafe {
                        libc::recvfrom(
                            libc_socket(fd),
                            buf.as_mut_ptr() as *mut c_void,
                            recv_len,
                            flags,
                            &mut storage as *mut _ as *mut libc::sockaddr,
                            &mut len,
                        )
                    };
                    if ret >= 0 {
                        Ok((ret as usize, sock_addr_from_storage(storage, len), None))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
            });
            match res {
                Ok((n, addr, tmp)) => {
                    if use_memoryview
                        && let Some(tmp) = tmp.as_ref()
                        && let Err(msg) = unsafe { memoryview_write_bytes(buffer_ptr, &tmp[..n]) }
                    {
                        return raise_exception::<u64>(_py, "TypeError", &msg);
                    }
                    let n_bits = MoltObject::from_int(n as i64).bits();
                    let addr_bits = sockaddr_to_bits(_py, &addr);
                    let tuple_ptr = alloc_tuple(_py, &[n_bits, addr_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait {
                        return raise_os_error::<u64>(_py, err, "recvfrom_into");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        if wait_err.kind() == ErrorKind::WouldBlock {
                            continue;
                        }
                        return raise_os_error::<u64>(_py, wait_err, "recvfrom_into");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "recvfrom_into"),
            }
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_sendmsg(
    sock_bits: u64,
    buffers_bits: u64,
    ancdata_bits: u64,
    flags_bits: u64,
    address_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let socket_ptr = ptr_from_bits(sock_bits);
            if socket_ptr.is_null() {
                return MoltObject::from_int(0).bits();
            }
            let ancillary_items = match parse_sendmsg_ancillary_items(_py, ancdata_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            };
            #[cfg(unix)]
            let mut ancillary_control = match encode_sendmsg_ancillary_buffer(&ancillary_items) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<u64>(_py, "RuntimeError", &msg),
            };
            let mut payload_chunks = match collect_sendmsg_payload(_py, buffers_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            };
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
            #[cfg(unix)]
            let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
            #[cfg(not(unix))]
            let dontwait = false;
            let nonblocking =
                matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
            let sockaddr = if obj_from_bits(address_bits).is_none() {
                None
            } else {
                let family = {
                    let socket = &*(socket_ptr as *mut MoltSocket);
                    let guard = socket.inner.lock().unwrap();
                    guard.family
                };
                match sockaddr_from_bits(_py, address_bits, family) {
                    Ok(addr) => Some(addr),
                    Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
                }
            };
            #[cfg(not(unix))]
            if !ancillary_items.is_empty() {
                if sockaddr.is_some() {
                    return raise_os_error_errno::<u64>(_py, libc::EOPNOTSUPP as i64, "sendmsg");
                }
                let preflight = with_socket_mut(socket_ptr, |inner| {
                    #[cfg(windows)]
                    let fd = inner.raw_socket().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    #[cfg(windows)]
                    {
                        Ok((fd, inner.is_stream()))
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = inner;
                        Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))
                    }
                });
                let (fd, is_stream) = match preflight {
                    Ok(val) => val,
                    Err(err) => return raise_os_error::<u64>(_py, err, "sendmsg"),
                };
                if !is_stream || !socket_peer_available(fd) {
                    return raise_os_error_errno::<u64>(_py, libc::EOPNOTSUPP as i64, "sendmsg");
                }
            }
            #[cfg(unix)]
            let mut iovecs: Vec<libc::iovec> = payload_chunks
                .iter_mut()
                .map(|chunk| libc::iovec {
                    iov_base: chunk.as_mut_ptr() as *mut c_void,
                    iov_len: chunk.len(),
                })
                .collect();
            #[cfg(not(unix))]
            let payload: Vec<u8> = payload_chunks.concat();
            loop {
                let res = with_socket_mut(socket_ptr, |inner| {
                    #[cfg(unix)]
                    let fd = inner.raw_fd().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    #[cfg(windows)]
                    let fd = inner.raw_socket().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                    })?;
                    #[cfg(unix)]
                    {
                        let mut msg: libc::msghdr = std::mem::zeroed();
                        if let Some(addr) = sockaddr.as_ref() {
                            msg.msg_name = addr.as_ptr() as *mut c_void;
                            msg.msg_namelen = addr.len();
                        }
                        if iovecs.is_empty() {
                            msg.msg_iov = std::ptr::null_mut();
                            msg.msg_iovlen = 0;
                        } else {
                            msg.msg_iov = iovecs.as_mut_ptr();
                            msg.msg_iovlen = iovecs.len().try_into().map_err(|_| {
                                std::io::Error::new(ErrorKind::InvalidInput, "too many iovecs")
                            })?;
                        }
                        if ancillary_control.is_empty() {
                            msg.msg_control = std::ptr::null_mut();
                            msg.msg_controllen = 0;
                        } else {
                            msg.msg_control = ancillary_control.as_mut_ptr() as *mut c_void;
                            msg.msg_controllen =
                                ancillary_control.len().try_into().map_err(|_| {
                                    std::io::Error::new(
                                        ErrorKind::InvalidInput,
                                        "ancillary too large",
                                    )
                                })?;
                        }
                        let ret = libc::sendmsg(libc_socket(fd), &msg as *const _, flags);
                        if ret >= 0 {
                            Ok(ret as usize)
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let ret = if let Some(addr) = sockaddr.as_ref() {
                            unsafe {
                                libc::sendto(
                                    libc_socket(fd),
                                    payload.as_ptr() as *const c_void,
                                    payload.len(),
                                    flags,
                                    addr.as_ptr(),
                                    addr.len(),
                                )
                            }
                        } else {
                            unsafe {
                                libc::send(
                                    libc_socket(fd),
                                    payload.as_ptr() as *const c_void,
                                    payload.len(),
                                    flags,
                                )
                            }
                        };
                        if ret >= 0 {
                            Ok(ret as usize)
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    }
                });
                match res {
                    Ok(n) => {
                        #[cfg(not(unix))]
                        if !ancillary_items.is_empty() && n > 0 {
                            let queue_res = with_socket_mut(socket_ptr, |inner| {
                                #[cfg(windows)]
                                let fd = inner.raw_socket().ok_or_else(|| {
                                    std::io::Error::new(ErrorKind::NotConnected, "socket closed")
                                })?;
                                #[cfg(windows)]
                                {
                                    socket_enqueue_stream_ancillary(
                                        fd,
                                        n,
                                        ancillary_items.as_slice(),
                                    )
                                }
                                #[cfg(not(windows))]
                                {
                                    let _ = inner;
                                    Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))
                                }
                            });
                            if let Err(err) = queue_res {
                                return raise_os_error::<u64>(_py, err, "sendmsg");
                            }
                        }
                        return MoltObject::from_int(n as i64).bits();
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        if dontwait || nonblocking {
                            return raise_os_error::<u64>(_py, err, "sendmsg");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "sendmsg");
                        }
                        continue;
                    }
                    Err(err) => return raise_os_error::<u64>(_py, err, "sendmsg"),
                }
            }
        })
    }
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recvmsg(
    sock_bits: u64,
    bufsize_bits: u64,
    ancbufsize_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bufsize = to_i64(obj_from_bits(bufsize_bits)).unwrap_or(0);
        if bufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative buffersize in recvmsg");
        }
        let ancbufsize = to_i64(obj_from_bits(ancbufsize_bits)).unwrap_or(0);
        if ancbufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative ancbufsize in recvmsg");
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        #[cfg(not(unix))]
        let peek = (flags & libc::MSG_PEEK) != 0;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; bufsize as usize];
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(unix)]
                {
                    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
                    msg.msg_name = (&mut storage as *mut libc::sockaddr_storage).cast();
                    msg.msg_namelen =
                        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                    let mut control = if ancbufsize > 0 {
                        vec![0u8; ancbufsize as usize]
                    } else {
                        Vec::new()
                    };
                    let mut iov = libc::iovec {
                        iov_base: if buf.is_empty() {
                            std::ptr::null_mut()
                        } else {
                            buf.as_mut_ptr() as *mut c_void
                        },
                        iov_len: buf.len(),
                    };
                    if buf.is_empty() {
                        msg.msg_iov = std::ptr::null_mut();
                        msg.msg_iovlen = 0;
                    } else {
                        msg.msg_iov = (&mut iov as *mut libc::iovec).cast();
                        msg.msg_iovlen = 1;
                    }
                    if control.is_empty() {
                        msg.msg_control = std::ptr::null_mut();
                        msg.msg_controllen = 0;
                    } else {
                        msg.msg_control = control.as_mut_ptr() as *mut c_void;
                        msg.msg_controllen = control.len().try_into().map_err(|_| {
                            std::io::Error::new(ErrorKind::InvalidInput, "ancillary too large")
                        })?;
                    }
                    let ret = unsafe { libc::recvmsg(libc_socket(fd), &mut msg as *mut _, flags) };
                    if ret >= 0 {
                        let addr_bits = if msg.msg_namelen > 0 {
                            let addr = sock_addr_from_storage(storage, msg.msg_namelen);
                            sockaddr_to_bits(_py, &addr)
                        } else {
                            MoltObject::none().bits()
                        };
                        let ancillary_items = parse_recvmsg_ancillary_items(&msg);
                        Ok((ret as usize, msg.msg_flags, addr_bits, ancillary_items))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                    let mut namelen =
                        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                    let ret = unsafe {
                        libc::recvfrom(
                            libc_socket(fd),
                            if buf.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                buf.as_mut_ptr() as *mut c_void
                            },
                            buf.len(),
                            flags,
                            (&mut storage as *mut libc::sockaddr_storage).cast(),
                            &mut namelen as *mut libc::socklen_t,
                        )
                    };
                    if ret >= 0 {
                        let ancillary_raw = socket_take_stream_ancillary(fd, ret as usize, peek);
                        let (ancillary_items, truncated) =
                            socket_clip_ancillary_for_bufsize(ancillary_raw, ancbufsize);
                        let mut msg_flags = 0i32;
                        if truncated {
                            msg_flags |= libc::MSG_CTRUNC;
                        }
                        let addr_bits = if namelen > 0 {
                            let addr = sock_addr_from_storage(storage, namelen);
                            sockaddr_to_bits(_py, &addr)
                        } else {
                            MoltObject::none().bits()
                        };
                        Ok((ret as usize, msg_flags, addr_bits, ancillary_items))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
            });
            match res {
                Ok((n, msg_flags, addr_bits, ancillary_items)) => {
                    let anc_bits = match build_ancillary_list_bits(_py, ancillary_items.as_slice())
                    {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, addr_bits);
                            return bits;
                        }
                    };
                    return build_recvmsg_result_with_anc(
                        _py,
                        &buf[..n],
                        msg_flags,
                        addr_bits,
                        anc_bits,
                    );
                }
                Err(err) => {
                    let raw = err.raw_os_error();
                    let would_block_raw = matches!(
                        raw,
                        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
                    );
                    let would_block = err.kind() == ErrorKind::WouldBlock || would_block_raw;
                    if would_block {
                        if dontwait || nonblocking {
                            return raise_os_error::<u64>(_py, err, "recvmsg");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "recvmsg");
                        }
                        continue;
                    }
                    return raise_os_error::<u64>(_py, err, "recvmsg");
                }
            }
        }
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_socket_recvmsg_into(
    sock_bits: u64,
    buffers_bits: u64,
    ancbufsize_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let ancbufsize = to_i64(obj_from_bits(ancbufsize_bits)).unwrap_or(0);
        if ancbufsize < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative ancbufsize in recvmsg");
        }
        let targets = match collect_recvmsg_into_targets(_py, buffers_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let total_len = targets
            .iter()
            .fold(0usize, |acc, target| acc.saturating_add(target.len()));
        let mut tmp = vec![0u8; total_len];
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        #[cfg(not(unix))]
        let peek = (flags & libc::MSG_PEEK) != 0;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(unix)]
                {
                    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
                    msg.msg_name = (&mut storage as *mut libc::sockaddr_storage).cast();
                    msg.msg_namelen =
                        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                    let mut control = if ancbufsize > 0 {
                        vec![0u8; ancbufsize as usize]
                    } else {
                        Vec::new()
                    };
                    let mut iov = libc::iovec {
                        iov_base: if tmp.is_empty() {
                            std::ptr::null_mut()
                        } else {
                            tmp.as_mut_ptr() as *mut c_void
                        },
                        iov_len: tmp.len(),
                    };
                    if tmp.is_empty() {
                        msg.msg_iov = std::ptr::null_mut();
                        msg.msg_iovlen = 0;
                    } else {
                        msg.msg_iov = (&mut iov as *mut libc::iovec).cast();
                        msg.msg_iovlen = 1;
                    }
                    if control.is_empty() {
                        msg.msg_control = std::ptr::null_mut();
                        msg.msg_controllen = 0;
                    } else {
                        msg.msg_control = control.as_mut_ptr() as *mut c_void;
                        msg.msg_controllen = control.len().try_into().map_err(|_| {
                            std::io::Error::new(ErrorKind::InvalidInput, "ancillary too large")
                        })?;
                    }
                    let ret = unsafe { libc::recvmsg(libc_socket(fd), &mut msg as *mut _, flags) };
                    if ret >= 0 {
                        let addr_bits = if msg.msg_namelen > 0 {
                            let addr = sock_addr_from_storage(storage, msg.msg_namelen);
                            sockaddr_to_bits(_py, &addr)
                        } else {
                            MoltObject::none().bits()
                        };
                        let ancillary_items = parse_recvmsg_ancillary_items(&msg);
                        Ok((ret as usize, msg.msg_flags, addr_bits, ancillary_items))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                    let mut namelen =
                        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                    let ret = unsafe {
                        libc::recvfrom(
                            libc_socket(fd),
                            if tmp.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                tmp.as_mut_ptr() as *mut c_void
                            },
                            tmp.len(),
                            flags,
                            (&mut storage as *mut libc::sockaddr_storage).cast(),
                            &mut namelen as *mut libc::socklen_t,
                        )
                    };
                    if ret >= 0 {
                        let ancillary_raw = socket_take_stream_ancillary(fd, ret as usize, peek);
                        let (ancillary_items, truncated) =
                            socket_clip_ancillary_for_bufsize(ancillary_raw, ancbufsize);
                        let mut msg_flags = 0i32;
                        if truncated {
                            msg_flags |= libc::MSG_CTRUNC;
                        }
                        let addr_bits = if namelen > 0 {
                            let addr = sock_addr_from_storage(storage, namelen);
                            sockaddr_to_bits(_py, &addr)
                        } else {
                            MoltObject::none().bits()
                        };
                        Ok((ret as usize, msg_flags, addr_bits, ancillary_items))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
            });
            match res {
                Ok((n, msg_flags, addr_bits, ancillary_items)) => {
                    if let Err(bits) = write_recvmsg_into_targets(_py, &targets, &tmp[..n]) {
                        dec_ref_bits(_py, addr_bits);
                        return bits;
                    }
                    let anc_bits = match build_ancillary_list_bits(_py, ancillary_items.as_slice())
                    {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, addr_bits);
                            return bits;
                        }
                    };
                    let n_bits = MoltObject::from_int(n as i64).bits();
                    let flags_bits = MoltObject::from_int(msg_flags as i64).bits();
                    let tuple_ptr = alloc_tuple(_py, &[n_bits, anc_bits, flags_bits, addr_bits]);
                    dec_ref_bits(_py, anc_bits);
                    dec_ref_bits(_py, addr_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                Err(err) => {
                    let raw = err.raw_os_error();
                    let would_block_raw = matches!(
                        raw,
                        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
                    );
                    let would_block = err.kind() == ErrorKind::WouldBlock || would_block_raw;
                    if would_block {
                        if dontwait || nonblocking {
                            return raise_os_error::<u64>(_py, err, "recvmsg_into");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            if wait_err.kind() == ErrorKind::WouldBlock {
                                continue;
                            }
                            return raise_os_error::<u64>(_py, wait_err, "recvmsg_into");
                        }
                        continue;
                    }
                    return raise_os_error::<u64>(_py, err, "recvmsg_into");
                }
            }
        }
    })
}
