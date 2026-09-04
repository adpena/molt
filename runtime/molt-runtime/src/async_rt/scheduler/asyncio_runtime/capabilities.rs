use crate::PyToken;
use crate::audit::AuditArgs;
use crate::object::ops::string_obj_to_owned;
use crate::{MoltObject, obj_from_bits, raise_exception, to_i64};

fn asyncio_has_net_capability(_py: &PyToken<'_>) -> bool {
    crate::operation_allowed(_py, crate::OperationId::NetAsyncio, AuditArgs::None)
}

fn asyncio_has_process_capability(_py: &PyToken<'_>) -> bool {
    crate::operation_allowed(_py, crate::OperationId::ProcessAsyncio, AuditArgs::None)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_require_ssl_transport_support() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !asyncio_has_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio SSL transport",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_ssl_transport_orchestrate(
    operation_bits: u64,
    ssl_bits: u64,
    server_hostname_bits: u64,
    server_side_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(operation) = string_obj_to_owned(obj_from_bits(operation_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "asyncio SSL transport operation must be str",
            );
        };
        if operation.is_empty() {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "asyncio SSL transport operation cannot be empty",
            );
        }
        let is_client_operation = matches!(
            operation.as_str(),
            "open_connection"
                | "open_unix_connection"
                | "create_connection"
                | "create_unix_connection"
        );
        let is_server_operation =
            matches!(operation.as_str(), "create_server" | "create_unix_server");
        let is_tls_upgrade = matches!(operation.as_str(), "start_tls");
        if !(is_client_operation || is_server_operation || is_tls_upgrade) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "unsupported asyncio SSL transport operation",
            );
        }
        let bool_true_bits = MoltObject::from_bool(true).bits();
        let bool_false_bits = MoltObject::from_bool(false).bits();
        if obj_from_bits(ssl_bits).is_none() {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "ssl transport requires an ssl context or ssl=True",
            );
        }
        let Some(server_side_raw) = to_i64(obj_from_bits(server_side_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "server_side must be bool");
        };
        if server_side_raw != 0 && server_side_raw != 1 {
            return raise_exception::<u64>(_py, "TypeError", "server_side must be bool");
        }
        let server_side = server_side_raw == 1;
        if is_client_operation && server_side {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "client SSL operations require server_side=False",
            );
        }
        if is_server_operation && !server_side {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "server SSL operations require server_side=True",
            );
        }
        if !obj_from_bits(server_hostname_bits).is_none() {
            let Some(server_hostname) = string_obj_to_owned(obj_from_bits(server_hostname_bits))
            else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "server_hostname must be str or None",
                );
            };
            if server_hostname.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname cannot be an empty string",
                );
            }
            if server_side {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname is only meaningful for client connections",
                );
            }
        }
        if ssl_bits == bool_false_bits {
            if is_tls_upgrade {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "start_tls requires an SSL context",
                );
            }
            if !obj_from_bits(server_hostname_bits).is_none() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname requires an active SSL transport",
                );
            }
            return bool_false_bits;
        }
        if is_client_operation && obj_from_bits(server_hostname_bits).is_none() {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "you have to pass server_hostname when using ssl",
            );
        }
        if !asyncio_has_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio SSL transport",
            );
        }
        if is_client_operation {
            if matches!(operation.as_str(), "open_connection" | "create_connection") {
                return bool_true_bits;
            }
            if matches!(
                operation.as_str(),
                "open_unix_connection" | "create_unix_connection"
            ) {
                #[cfg(all(unix, not(target_arch = "wasm32")))]
                {
                    return bool_true_bits;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on wasm",
                    );
                }
                #[cfg(windows)]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on Windows",
                    );
                }
                #[cfg(all(not(unix), not(target_arch = "wasm32"), not(windows)))]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on this host",
                    );
                }
            }
            let msg = format!(
                "unsupported asyncio SSL transport operation '{}'",
                operation
            );
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
        if is_server_operation {
            return bool_true_bits;
        }
        if is_tls_upgrade {
            return bool_true_bits;
        }
        let msg = format!(
            "asyncio SSL transport operation '{}' is not yet available in this runtime",
            operation
        );
        raise_exception::<u64>(_py, "RuntimeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_require_unix_socket_support() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !asyncio_has_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio unix sockets",
            );
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on wasm",
            )
        }
        #[cfg(all(windows, not(target_arch = "wasm32")))]
        {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on Windows",
            )
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            MoltObject::none().bits()
        }
        #[cfg(not(any(unix, windows, target_arch = "wasm32")))]
        {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on this host",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_require_child_watcher_support() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !asyncio_has_process_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for asyncio child watchers",
            );
        }
        #[cfg(any(target_arch = "wasm32", windows))]
        {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio child watchers are unavailable on this host",
            )
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            MoltObject::none().bits()
        }
        #[cfg(not(any(unix, windows, target_arch = "wasm32")))]
        {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio child watchers are unavailable on this host",
            )
        }
    })
}
