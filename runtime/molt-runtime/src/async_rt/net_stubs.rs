//! Native-no-net stubs for when `stdlib_net` is disabled on native targets.
//!
//! The cfg guard is on the `mod` declaration in `async_rt/mod.rs`.

use std::collections::HashMap;
use std::sync::Mutex;
use crate::concurrency::PyToken;
use crate::object::PtrSlot;

pub(crate) struct IoPoller {
    waiters: Mutex<HashMap<PtrSlot, ()>>,
}

impl IoPoller {
    pub(crate) fn new() -> Self {
        Self { waiters: Mutex::new(HashMap::new()) }
    }
    pub(crate) fn start_worker(self: &std::sync::Arc<Self>) {}
    pub(crate) fn wait_blocking(&self, _socket_ptr: *mut u8, _events: u32, _timeout: Option<std::time::Duration>) -> Result<u32, std::io::Error> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "networking not available"))
    }
    pub(crate) fn cancel_waiter(&self, _slot: *mut u8) {}
    pub(crate) fn shutdown(&self) {}
}

pub(crate) fn io_wait_release_socket(_py: &PyToken<'_>, _future_ptr: *mut u8) {}
pub(crate) fn ws_wait_release(_py: &PyToken<'_>, _future_ptr: *mut u8) {}

macro_rules! net_error {
    () => {
        crate::with_gil_entry!(_py, {
            crate::raise_exception::<u64>(
                _py, "OSError", "networking not available (compile with stdlib_net)")
        })
    };
}

#[unsafe(no_mangle)] pub extern "C" fn molt_socket_new(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_close(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_drop(_: u64) {}
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_clone(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_fileno(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gettimeout(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_settimeout(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_setblocking(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getblocking(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_bind(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_listen(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_accept(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_connect(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_connect_ex(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recv(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recv_into(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvfrom(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvfrom_into(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvmsg(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvmsg_into(_: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_send(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendall(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendto(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendmsg(_: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_shutdown(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getsockname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getpeername(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_chan_recv_blocking(_: u64) -> i64 { -1 }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_connect(_: *const u8, _: u64, _: *mut u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_connect_obj(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_wait_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_wait(_: u64) -> i64 { -1 }
#[unsafe(no_mangle)] pub extern "C" fn molt_io_wait_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_io_wait(_: u64) -> i64 { -1 }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_client_connect_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_client_from_fd_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_server_payload(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_server_from_fd_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gethostname() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getprotobyname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getservbyname(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getservbyport(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_has_dualstack_ipv6() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_has_ipv6() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_inet_ntop(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_inet_pton(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getaddrinfo(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socketpair(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getnameinfo(_: u64, _: u64) -> u64 { net_error!() }


#[unsafe(no_mangle)] pub extern "C" fn molt_socket_setsockopt(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_detach(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendfile(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getsockopt(_: u64, _: u64, _: u64) -> u64 { net_error!() }

// Additional sockets_net stubs for no-net native builds
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getfqdn(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gethostbyaddr(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gethostbyname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gethostbyname_ex(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_htonl(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_htons(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_ntohl(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_ntohs(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_if_nameindex() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_if_nametoindex(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_if_indextoname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_cmsg_len(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_cmsg_space(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_send_fds(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recv_fds(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sethostname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendmsg_afalg(_: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
