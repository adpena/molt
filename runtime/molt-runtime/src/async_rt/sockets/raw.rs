// Native raw socket adapter authority.
// Owns OS fd/socket casts, socket2 borrowing, raw connect/listen/error shims, and Windows loopback pairs.

#[cfg(molt_has_net_io)]
use super::SocketFd;
#[cfg(molt_has_net_io)]
use socket2::{SockAddr, SockAddrStorage, SockRef, Socket};
#[cfg(all(molt_has_net_io, unix))]
use std::os::fd::BorrowedFd;
#[cfg(all(molt_has_net_io, unix))]
use std::os::raw::{c_int, c_void};
#[cfg(all(molt_has_net_io, unix))]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(all(molt_has_net_io, windows))]
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, RawSocket};

#[cfg(all(unix, molt_has_net_io))]
type LibcSocket = c_int;
#[cfg(all(windows, molt_has_net_io))]
type LibcSocket = libc::SOCKET;

#[cfg(all(unix, molt_has_net_io))]
pub(crate) fn libc_socket(fd: RawFd) -> LibcSocket {
    fd
}
#[cfg(all(windows, molt_has_net_io))]
pub(crate) fn libc_socket(fd: RawSocket) -> LibcSocket {
    fd as LibcSocket
}

#[cfg(molt_has_net_io)]
pub(super) fn connect_raw_socket(fd: SocketFd, sockaddr: &SockAddr) -> std::io::Result<()> {
    let ret = unsafe {
        libc::connect(
            libc_socket(fd),
            sockaddr.as_ptr() as *const libc::sockaddr,
            sockaddr.len(),
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(molt_has_net_io)]
pub(crate) fn sock_addr_from_storage(
    storage: libc::sockaddr_storage,
    len: libc::socklen_t,
) -> SockAddr {
    let mut addr_storage = SockAddrStorage::zeroed();
    unsafe {
        *addr_storage.view_as::<libc::sockaddr_storage>() = storage;
        SockAddr::new(addr_storage, len)
    }
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn socket_is_acceptor(socket: &Socket) -> bool {
    let fd = socket.as_raw_fd();
    let mut val: c_int = 0;
    let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            &mut val as *mut _ as *mut c_void,
            &mut len,
        )
    };
    ret == 0 && val != 0
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn socket_is_acceptor(_socket: &Socket) -> bool {
    false
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn socket_relisten(fd: RawFd, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(fd, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn socket_relisten(socket: RawSocket, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(socket, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn with_sockref<T, F>(fd: RawFd, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn with_sockref<T, F>(socket: RawSocket, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedSocket::borrow_raw(socket) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(all(unix, molt_has_net_io))]
fn take_error_raw(fd: RawFd) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(fd, |sock_ref| sock_ref.take_error())
}

#[cfg(all(windows, molt_has_net_io))]
fn take_error_raw(socket: RawSocket) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(socket, |sock_ref| sock_ref.take_error())
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn take_error_mio<T: AsRawFd>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_fd())
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn take_error_mio<T: AsRawSocket>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_socket())
}

#[cfg(all(molt_has_net_io, windows))]
pub(crate) fn socket_close_raw_windows(raw: RawSocket) {
    unsafe {
        drop(Socket::from_raw_socket(raw));
    }
}

#[cfg(all(molt_has_net_io, windows))]
pub(crate) fn socketpair_windows_loopback_raw(
    family: i32,
) -> Result<(RawSocket, RawSocket), std::io::Error> {
    let loopback = if family == libc::AF_INET6 {
        "[::1]:0"
    } else {
        "127.0.0.1:0"
    };
    let listener = std::net::TcpListener::bind(loopback)?;
    let addr = listener.local_addr()?;
    let client = std::net::TcpStream::connect(addr)?;
    let (server, _) = listener.accept()?;
    Ok((client.into_raw_socket(), server.into_raw_socket()))
}
