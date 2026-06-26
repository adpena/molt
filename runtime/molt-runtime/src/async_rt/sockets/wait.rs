// Socket readiness, timeout, and host errno authority.
// Owns native poll/io-poller waiting and WASM host wait/would-block normalization.

use super::*;

#[cfg(target_arch = "wasm32")]
pub(crate) fn errno_from_rc(rc: i32) -> i32 {
    if rc < 0 { -rc } else { 0 }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn would_block_errno(errno: i32) -> bool {
    errno == libc::EAGAIN || errno == libc::EWOULDBLOCK
}

#[cfg(molt_has_net_io)]
pub(crate) fn socket_wait_ready(
    _py: &PyToken<'_>,
    socket_ptr: *mut u8,
    events: u32,
) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(socket_ptr);
    #[cfg(unix)]
    {
        let fd = with_socket_mut(socket_ptr, |inner| {
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            Ok(fd)
        })?;
        socket_wait_ready_poll(fd, events, timeout)
    }
    #[cfg(not(unix))]
    {
        if let Some(timeout) = timeout {
            if timeout == Duration::ZERO {
                return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
            }
            runtime_state(_py)
                .io_poller()
                .wait_blocking(socket_ptr, events, Some(timeout))
                .map(|_| ())
        } else {
            runtime_state(_py)
                .io_poller()
                .wait_blocking(socket_ptr, events, None)
                .map(|_| ())
        }
    }
}

#[cfg(all(molt_has_net_io, unix))]
pub(crate) fn socket_wait_ready_poll(
    fd: RawFd,
    events: u32,
    timeout: Option<Duration>,
) -> Result<(), std::io::Error> {
    let mut poll_events = 0;
    if (events & IO_EVENT_READ) != 0 {
        poll_events |= libc::POLLIN;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        poll_events |= libc::POLLOUT;
    }
    let timeout_ms = match timeout {
        Some(val) if val == Duration::ZERO => {
            return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
        }
        Some(val) => val.as_millis().min(i32::MAX as u128) as i32,
        None => -1,
    };
    let mut poll_fd = libc::pollfd {
        fd,
        events: poll_events,
        revents: 0,
    };
    let rc = {
        let _release = crate::concurrency::GilReleaseGuard::new();
        unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) }
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if rc == 0 {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn socket_wait_ready(
    _py: &PyToken<'_>,
    handle: i64,
    events: u32,
) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(handle);
    if let Some(timeout) = timeout {
        if timeout == Duration::ZERO {
            return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
        }
        let ms = timeout.as_millis().min(i64::MAX as u128) as i64;
        let rc = unsafe { crate::molt_socket_wait_host(handle, events, ms) };
        if rc == 0 {
            return Ok(());
        }
        if rc == -libc::ETIMEDOUT {
            return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
        }
        return Err(std::io::Error::from_raw_os_error(-rc));
    }
    let rc = unsafe { crate::molt_socket_wait_host(handle, events, -1) };
    if rc == 0 {
        return Ok(());
    }
    if rc == -libc::ETIMEDOUT {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Err(std::io::Error::from_raw_os_error(-rc))
}
