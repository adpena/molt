//! Native-no-net stubs for when `stdlib_net` is disabled on native targets.
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

#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_client_connect_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_client_from_fd_new(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_server_from_fd_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_tls_server_payload(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_asyncio_to_thread(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_abort(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_broken(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_n_waiting(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_new(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_parties(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_reset(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_barrier_wait(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_chan_recv_blocking(_: u64) -> i64 { -1 }
#[unsafe(no_mangle)] pub extern "C" fn molt_condition_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_condition_new() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_condition_notify(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_condition_wait(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_condition_wait_for(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_clear(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_is_set(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_new() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_set(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_event_wait(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_f_getfd() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_f_getfl() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_f_setfd() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_f_setfl() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_fd_cloexec() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_fcntl_o_nonblock() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_io_wait(_: u64) -> i64 { -1 }
#[unsafe(no_mangle)] pub extern "C" fn molt_io_wait_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_local_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_local_get_dict(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_local_new() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_lock_acquire(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_lock_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_lock_locked(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_lock_new() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_lock_release(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_get_terminal_size(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getegid() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_geteuid() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getgid() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getloadavg() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getlogin() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getppid() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_getuid() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_link(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_path_samefile(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_symlink(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_truncate(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_umask(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_os_uname() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_pathlib_hardlink_to(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_pathlib_symlink_to(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_kill(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_pid(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_poll(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_returncode(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_spawn(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_spawn_ex(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_stderr(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_stdin(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_stdout(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_terminate(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_process_wait_future(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_empty(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_full(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_get(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_is_shutdown(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_join(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_lifo_new(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_new(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_priority_new(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_put(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_qsize(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_shutdown(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_queue_task_done(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_acquire(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_acquire_restore(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_is_owned(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_locked(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_new() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_release(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_rlock_release_save(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_semaphore_acquire(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_semaphore_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_semaphore_new(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_semaphore_release(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_shutil_chown(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_shutil_disk_usage(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_shutil_get_terminal_size(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_shutil_make_archive(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_shutil_unpack_archive(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_signal_raise(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_signal_sig_setmask() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_signal_sig_unblock() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_signal_sigbus() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_signal_sigwait(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_accept(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_bind(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_clone(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_close(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_connect(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_connect_ex(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_detach(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_drop(_: u64) {}
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_fileno(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getaddrinfo(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getblocking(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gethostname() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getnameinfo(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getpeername(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getprotobyname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getservbyname(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getservbyport(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getsockname(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_getsockopt(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_gettimeout(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_has_dualstack_ipv6() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_has_ipv6() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_inet_ntop(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_inet_pton(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_listen(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_new(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recv(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recv_into(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvfrom(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvfrom_into(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvmsg(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_recvmsg_into(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_send(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendall(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendmsg(_: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_sendto(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_setblocking(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_setsockopt(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_settimeout(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socket_shutdown(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_socketpair(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_spawn(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ssl_socket_close(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ssl_socket_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ssl_socket_unwrap(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_subprocess_check_call(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_subprocess_check_output(_: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_subprocess_run(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_tempfile_mkstemp(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_tempfile_named(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_current_ident() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_current_native_id() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_drop(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_ident(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_is_alive(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_join(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_native_id(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_active_count() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_current() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_forget(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_register(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_set_main(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_registry_snapshot() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_spawn(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_spawn_shared(_: u64, _: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_stack_size_get() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_stack_size_set(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_thread_submit(_: u64, _: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_time_process_time() -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_time_strftime(_: u64, _: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_connect_obj(_: u64) -> u64 { net_error!() }
#[unsafe(no_mangle)] pub extern "C" fn molt_ws_wait_new(_: u64, _: u64, _: u64) -> u64 { net_error!() }
